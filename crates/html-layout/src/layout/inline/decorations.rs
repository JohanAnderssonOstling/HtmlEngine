use super::super::border_geometry::{emit_border_fragments, emit_horizontal_pattern, emit_outline_fragments, uniform_solid_border};
use super::super::decorations::DecorationEmitter;
use crate::layout_model::{LayoutMode, Line, RoundedDecoration};
use html_style_model::BorderStyle;
use kurbo::Rect;

impl DecorationEmitter<'_, '_, '_> {
    pub(in crate::layout) fn collect_inline_decorations(&mut self) {
        if self.fragments.state().line_output.lines.is_empty() || self.inline_content.inline_items_empty() {
            return;
        }

        let lines = self.fragments.state().line_output.lines.to_vec();
        for (line_idx, line) in lines.into_iter().enumerate() {
            let fragment_range = line.inline_box_fragments.start as usize..line.inline_box_fragments.end as usize;
            let fragments = self.fragments.state().line_output.inline_box_fragments.get(fragment_range).unwrap_or_default().to_vec();
            for fragment in fragments {
                self.emit_inline_segment(
                    fragment.box_idx as usize,
                    line_idx,
                    &line,
                    line.point.x + fragment.start_x as f64,
                    line.point.x + fragment.end_x as f64,
                    line.point.y + fragment.top as f64,
                    line.point.y + fragment.bottom as f64,
                    line.point.y + fragment.baseline as f64,
                    fragment.flags & crate::layout_model::LineInlineBoxFragment::INLINE_START != 0,
                    fragment.flags & crate::layout_model::LineInlineBoxFragment::INLINE_END != 0,
                    fragment.flags & crate::layout_model::LineInlineBoxFragment::BORDER_BOX_BOUNDS != 0,
                    fragment.paint_order,
                );
            }
        }
    }

    fn emit_inline_segment(
        &mut self, box_idx: usize, line_idx: usize, line: &Line, start_x: f64, end_x: f64, fragment_top: f64, fragment_bottom: f64, fragment_baseline: f64, inline_start: bool, inline_end: bool, border_box_bounds: bool, paint_order: u32,
    ) {
        if start_x > end_x {
            return;
        }
        let is_inline_box = matches!(self.reader.box_layout_mode(box_idx), Some(LayoutMode::Inline(_)));
        let style = self.reader.style(box_idx);
        let background_color = style.background_color();
        let color_alpha = background_color & 0xFF;
        let top_border = matches!(style.border_top_style(), BorderStyle::Solid).then_some((style.border_top_width() as f64, style.border_top_color()));
        let right_border = (inline_end && matches!(style.border_right_style(), BorderStyle::Solid)).then_some((style.border_right_width() as f64, style.border_right_color()));
        let bottom_border = matches!(style.border_bottom_style(), BorderStyle::Solid).then_some((style.border_bottom_width() as f64, style.border_bottom_color()));
        let left_border = (inline_start && matches!(style.border_left_style(), BorderStyle::Solid)).then_some((style.border_left_width() as f64, style.border_left_color()));
        let uniform_border = (inline_start && inline_end).then(|| uniform_solid_border(&style)).flatten();
        let has_any_border = top_border.is_some() || right_border.is_some() || bottom_border.is_some() || left_border.is_some();
        let cb_width = self.geometry.size(box_idx).width;
        let pad_left = if inline_start { style.padding_left().resolve(cb_width) } else { 0.0 };
        let pad_right = if inline_end { style.padding_right().resolve(cb_width) } else { 0.0 };
        let pad_top = style.padding_top().resolve(cb_width);
        let pad_bottom = style.padding_bottom().resolve(cb_width);
        let text_decoration = style.text_decoration();
        let text_decoration_color = style.text_decoration_color();
        let font_size = style.font_size();
        let outline = style.outline();
        let outline_color = style.outline_color();

        if !is_inline_box && text_decoration.lines.is_empty() {
            return;
        }
        if color_alpha == 0 && !has_any_border && text_decoration.lines.is_empty() && (outline.style == BorderStyle::None || outline.width() <= 0.0) {
            return;
        }

        let (x0, x1, y0, y1) = if border_box_bounds {
            (start_x, end_x, fragment_top, fragment_bottom)
        } else {
            (
                start_x - pad_left - left_border.map_or(0.0, |border| border.0),
                end_x + pad_right + right_border.map_or(0.0, |border| border.0),
                fragment_top - pad_top - top_border.map_or(0.0, |border| border.0),
                fragment_bottom + pad_bottom + bottom_border.map_or(0.0, |border| border.0),
            )
        };
        if x1 <= x0 || y1 <= y0 {
            return;
        }

        let rect = Rect::new(x0, y0, x1, y1);
        let decoration_start = self.fragments.decoration_len();
        if is_inline_box && color_alpha != 0 {
            let radii = style.border_radii().resolve(rect.width(), rect.height());
            self.push_background(rect, background_color, true, (!radii.is_zero()).then_some(RoundedDecoration { radii, border_width: None }));
        }
        if is_inline_box && has_any_border {
            let radii = style.border_radii().resolve(rect.width(), rect.height());
            if let Some((border_width, border_color)) = uniform_border.filter(|_| !radii.is_zero()) {
                self.push_decoration(rect, border_color, true, Some(RoundedDecoration { radii, border_width: Some(border_width as f32) }));
            } else {
                emit_border_fragments(self.fragments.decorations_mut(), rect, top_border, right_border, bottom_border, left_border, true);
            }
        }
        if is_inline_box && outline.style != BorderStyle::None && outline.width() > 0.0 {
            emit_outline_fragments(self.fragments.decorations_mut(), rect, outline.width() as f64, outline_color, outline.style, true);
        }
        if !text_decoration.lines.is_empty() && text_decoration_color & 0xFF != 0 {
            let thickness = text_decoration.thickness.resolve(font_size) as f64;
            if thickness > 0.0 {
                let baseline = fragment_baseline;
                let font_metrics = self.reader.font_metrics(box_idx);
                let ascent = font_metrics.ascent_ratio().map_or(line.baseline, |ratio| f64::from(font_size * ratio));
                let x_height = f64::from(font_size * font_metrics.x_height_ratio());
                let fragments = self.fragments.decorations_mut();
                if text_decoration.lines.underline() {
                    let y = baseline + thickness * 0.5;
                    emit_horizontal_pattern(fragments, Rect::new(start_x, y, end_x, y + thickness), text_decoration_color, text_decoration.style, true, false, false);
                }
                if text_decoration.lines.overline() {
                    let y = (baseline - ascent).max(line.point.y);
                    emit_horizontal_pattern(fragments, Rect::new(start_x, y, end_x, y + thickness), text_decoration_color, text_decoration.style, true, false, false);
                }
                if text_decoration.lines.line_through() {
                    let y = baseline - x_height * 0.5 - thickness * 0.5;
                    emit_horizontal_pattern(fragments, Rect::new(start_x, y, end_x, y + thickness), text_decoration_color, text_decoration.style, true, true, false);
                }
            }
        }
        if self.track_overflow_clips {
            self.fragments.record_decoration_owner_since(box_idx as u32, decoration_start);
        }
        self.fragments.inherit_line_layer_for_decorations_since(line_idx, decoration_start);
        self.fragments.record_decoration_line_since(line_idx, decoration_start);
        self.fragments.record_decoration_paint_order_since(paint_order, decoration_start);
    }
}
