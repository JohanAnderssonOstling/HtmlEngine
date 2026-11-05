use html_style_model::computed_style;
use super::super::border_geometry::{
    emit_horizontal_pattern, emit_outline_fragments, physical_borders,
};
use super::super::decorations::DecorationEmitter;
use crate::layout_model::{Line, LineInlineBoxFragment};
use html_style_model::BorderStyle;
use kurbo::Rect;

impl DecorationEmitter<'_, '_, '_> {
    pub(in crate::layout) fn collect_inline_decorations(&mut self) {
        if self.inline_content.inline_items_empty() {
            return;
        }

        // Emission appends paint output without changing the source line/fragment arenas.
        for line_idx in 0..self.fragments.line_len() {
            let output = &self.fragments.state().line_output;
            let line = output.lines[line_idx].clone();
            let inline_range = line.inline_box_fragments(&output.line_details);
            let start = inline_range.start as usize;
            let count = output
                .inline_box_fragments
                .get(start..inline_range.end as usize)
                .map_or(0, |fragments| fragments.len());
            for fragment_idx in start..start + count {
                let fragment =
                    self.fragments.state().line_output.inline_box_fragments[fragment_idx];
                self.emit_inline_segment(line_idx, &line, &fragment);
            }
        }
    }

    fn emit_inline_segment(
        &mut self,
        line_idx: usize,
        line: &Line,
        fragment: &LineInlineBoxFragment,
    ) {
        let box_idx = fragment.box_idx as usize;
        let start_x = line.point.x + fragment.start_x as f64;
        let end_x = line.point.x + fragment.end_x as f64;
        if start_x > end_x {
            return;
        }
        let fragment_top = line.point.y + fragment.top as f64;
        let fragment_bottom = line.point.y + fragment.bottom as f64;
        let fragment_baseline = line.point.y + fragment.baseline as f64;
        let inline_start = fragment.flags & LineInlineBoxFragment::INLINE_START != 0;
        let inline_end = fragment.flags & LineInlineBoxFragment::INLINE_END != 0;
        let border_box_bounds = fragment.flags & LineInlineBoxFragment::BORDER_BOX_BOUNDS != 0;
        let is_inline_box = self.reader.is_inline_box(box_idx);
        let style = self.reader.paint_style(box_idx);
        if style.visibility() == html_style_model::Visibility::Hidden {
            return;
        }
        let background_color = style.background_color();
        let color_alpha = background_color & 0xFF;
        let borders = physical_borders(&style);
        let top_border = borders.top.solid();
        let right_border = inline_end.then(|| borders.right.solid()).flatten();
        let bottom_border = borders.bottom.solid();
        let left_border = inline_start.then(|| borders.left.solid()).flatten();
        let uniform_border = (inline_start && inline_end)
            .then(|| borders.uniform().and_then(|border| border.solid()))
            .flatten();
        let solid_borders = [top_border, right_border, bottom_border, left_border];
        let has_any_border = solid_borders.iter().any(Option::is_some);
        let cb_width = self.geometry.size(box_idx).width;
        let pad_left = if inline_start {
            style.padding_left().resolve(cb_width)
        } else {
            0.0
        };
        let pad_right = if inline_end {
            style.padding_right().resolve(cb_width)
        } else {
            0.0
        };
        let pad_top = style.padding_top().resolve(cb_width);
        let pad_bottom = style.padding_bottom().resolve(cb_width);
        let text_decoration = style.text_decoration();
        let text_decoration_color = style.text_decoration_color();
        let font_size = style.font_size();
        let outline = style.outline();
        let outline_color = style.outline_color();

        if !is_inline_box && computed_style::text_decoration_lines_is_empty(text_decoration.lines) {
            return;
        }
        if color_alpha == 0
            && !has_any_border
            && computed_style::text_decoration_lines_is_empty(text_decoration.lines)
            && (outline.style == BorderStyle::None || outline.width() <= 0.0)
        {
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
            self.push_background(
                rect,
                background_color,
                true,
                (!computed_style::corner_radii_is_zero(radii)).then_some(radii),
            );
        }
        if is_inline_box && has_any_border {
            let radii = style.border_radii().resolve(rect.width(), rect.height());
            self.emit_solid_border(rect, radii, uniform_border, solid_borders, true);
        }
        if is_inline_box && outline.style != BorderStyle::None && outline.width() > 0.0 {
            emit_outline_fragments(
                self.fragments.decoration_store_mut(),
                rect,
                outline.width() as f64,
                outline.offset() as f64,
                outline_color,
                outline.style,
                true,
            );
        }
        if !computed_style::text_decoration_lines_is_empty(text_decoration.lines) && text_decoration_color & 0xFF != 0 {
            let thickness = text_decoration.thickness.resolve(font_size) as f64;
            if thickness > 0.0 {
                let baseline = fragment_baseline;
                let font_metrics = self.reader.font_metrics(box_idx);
                let ascent = font_metrics
                    .line_metrics
                    .map(|metrics| metrics.ascent_ratio)
                    .map_or(line.baseline, |ratio| f64::from(font_size * ratio));
                let x_height = f64::from(font_size * font_metrics.x_height_ratio);
                let fragments = self.fragments.decoration_store_mut();
                let mut emit = |y, foreground| {
                    emit_horizontal_pattern(
                        fragments,
                        Rect::new(start_x, y, end_x, y + thickness),
                        text_decoration_color,
                        text_decoration.style,
                        true,
                        foreground,
                        false,
                    );
                };
                if computed_style::text_decoration_lines_underline(text_decoration.lines) {
                    emit(baseline + thickness * 0.5, false);
                }
                if computed_style::text_decoration_lines_overline(text_decoration.lines) {
                    // The overline follows the selected font's ascent from
                    // the fragment baseline. It may sit above the nominal
                    // line top when the font metrics exceed the line-height.
                    emit(baseline - ascent, false);
                }
                if computed_style::text_decoration_lines_line_through(text_decoration.lines) {
                    emit(baseline - x_height * 0.5 - thickness * 0.5, true);
                }
            }
        }
        if self.reader.tracks_overflow_clips() {
            self.fragments
                .record_decoration_owner_since(box_idx as u32, decoration_start);
        }
        self.fragments
            .inherit_line_layer_for_decorations_since(line_idx, decoration_start);
        self.fragments
            .record_decoration_line_since(line_idx, decoration_start);
        self.fragments
            .record_decoration_paint_order_since(fragment.paint_order, decoration_start);
    }
}
