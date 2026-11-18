//! Replay inline boxes and text decorations from finalized line fragments.
use super::borders::{emit_horizontal_pattern, emit_outline_fragments};
use super::decorations::{DecorationEmitter, emit_solid_border, push_background};
use crate::decorations::{inline_border_rect, physical_borders};
use crate::output::DecorationMetadata;
use html_engine_model::{DecorationStore, FontRelativeMetrics, LineInlineBoxFragment};
use html_style_model::{UsedStyleView, Visibility, computed_style};
use kurbo::{Point, Rect};

impl DecorationEmitter<'_, '_, '_> {
    pub(super) fn collect_inline_decorations(&mut self) {
        if self.inline_content.inline_items_empty() {
            return;
        }
        for line_idx in 0..self.fragments.line_len() {
            let output = &self.fragments.state().line_output;
            let point = output.lines[line_idx].point;
            let baseline = output.lines[line_idx].baseline;
            let containing_width = output.lines[line_idx]
                .details(&output.line_details)
                .map_or(0.0, |details| details.inline_containing_width);
            let range = output.lines[line_idx].inline_box_fragments(&output.line_details);
            for fragment_idx in range.start..range.end {
                let fragment =
                    self.fragments.state().line_output.inline_box_fragments[fragment_idx as usize];
                let style = self.reader.paint_style(fragment.box_idx as usize);
                if style.visibility() == Visibility::Hidden || fragment.start_x > fragment.end_x {
                    continue;
                }
                let rect = inline_border_rect(style, point, &fragment, containing_width);
                if rect.width() <= 0.0 || rect.height() <= 0.0 {
                    continue;
                }
                let inline_box = self.reader.is_inline_box(fragment.box_idx as usize);
                let font_metrics = self.reader.font_metrics(fragment.box_idx as usize);
                let metadata = DecorationMetadata {
                    block_owner: None,
                    line: u32::try_from(line_idx).expect("line index fits u32"),
                    paint_order: fragment.paint_order,
                    layers: self
                        .fragments
                        .state()
                        .line_output
                        .paint_layers
                        .get(line_idx)
                        .copied()
                        .unwrap_or_default(),
                    clip: self.clip(fragment.box_idx),
                };
                self.fragments.write_decorations(metadata, |fragments| {
                    if inline_box {
                        emit_inline_box(fragments, style, rect, fragment.flags);
                    }
                    emit_text_decoration(
                        fragments,
                        style,
                        font_metrics,
                        point,
                        baseline,
                        &fragment,
                    );
                });
            }
        }
    }
}

fn emit_inline_box(
    fragments: &mut DecorationStore,
    style: UsedStyleView<'_>,
    rect: Rect,
    flags: u8,
) {
    let borders = physical_borders(&style);
    let start = flags & LineInlineBoxFragment::INLINE_START != 0;
    let end = flags & LineInlineBoxFragment::INLINE_END != 0;
    let solid = borders.solid_edges(start, end);
    let uniform = (start && end)
        .then(|| borders.uniform().and_then(|border| border.solid()))
        .flatten();
    let radii = style.border_radii().resolve(rect.width(), rect.height());
    if style.background_color() & 0xff != 0 {
        push_background(fragments, rect, style.background_color(), true, radii);
    }
    emit_solid_border(fragments, rect, radii, uniform, solid, true);
    let outline = style.outline();
    emit_outline_fragments(
        fragments,
        rect,
        f64::from(outline.width()),
        f64::from(outline.offset()),
        style.outline_color(),
        outline.style,
        true,
    );
}

fn emit_text_decoration(
    fragments: &mut DecorationStore,
    style: UsedStyleView<'_>,
    metrics: FontRelativeMetrics,
    point: Point,
    line_baseline: f64,
    fragment: &LineInlineBoxFragment,
) {
    let decoration = style.text_decoration();
    if computed_style::text_decoration_lines_is_empty(decoration.lines)
        || style.text_decoration_color() & 0xff == 0
    {
        return;
    }
    let thickness = f64::from(decoration.thickness.resolve(style.font_size()));
    if thickness <= 0.0 {
        return;
    }
    let baseline = point.y + f64::from(fragment.baseline);
    let ascent = metrics.line_metrics.map_or(line_baseline, |metrics| {
        f64::from(style.font_size() * metrics.ascent_ratio)
    });
    let x_height = f64::from(style.font_size() * metrics.x_height_ratio);
    let mut emit = |y, foreground| {
        emit_horizontal_pattern(
            fragments,
            Rect::new(
                point.x + f64::from(fragment.start_x),
                y,
                point.x + f64::from(fragment.end_x),
                y + thickness,
            ),
            style.text_decoration_color(),
            decoration.style,
            true,
            foreground,
            false,
        )
    };
    if computed_style::text_decoration_lines_underline(decoration.lines) {
        emit(baseline + thickness * 0.5, false);
    }
    if computed_style::text_decoration_lines_overline(decoration.lines) {
        emit(baseline - ascent, false);
    }
    if computed_style::text_decoration_lines_line_through(decoration.lines) {
        emit(baseline - x_height * 0.5 - thickness * 0.5, true);
    }
}
