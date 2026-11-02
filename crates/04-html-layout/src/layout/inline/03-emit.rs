//! Publish paragraph-local results through the shared line-emission path.
use super::InlineFormattingInput;
use super::fragments::{first_line_color, write_line_fragments};
use super::metrics::MetricsView;
use super::paragraph::ParagraphLayoutResult;
use super::tokens::InlineTokens;
use html_style_model::TextDirection;

/// Project a paragraph-local result into the current chapter placement. Image,
/// atomic-box, decoration and absolute-anchor publication still runs here.
pub(super) fn emit_prepared_paragraph(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
    tokens: &InlineTokens,
    paragraph: &ParagraphLayoutResult,
) {
    profile_scope!("lines.emit_paragraph");
    let started = engine.start_timing();
    let mut adjustments = paragraph.adjustments.iter().peekable();
    for (index, line) in paragraph.lines.iter().enumerate() {
        let adjustment = adjustments
            .peek()
            .filter(|entry| entry.line == index)
            .copied();
        let (span, metrics) = if let Some(entry) = adjustment {
            let span = entry.tokens.as_deref().map_or_else(
                || tokens.span().slice(line.tokens.clone()),
                |tokens| tokens.into(),
            );
            let runs = entry.runs.as_deref().unwrap_or(&tokens.runs);
            (span, MetricsView::direct(runs))
        } else {
            (
                tokens.span().slice(line.tokens.clone()),
                tokens.metrics_view().shifted(line.tokens.start),
            )
        };
        let color = (index == 0 && input.starts_block)
            .then(|| first_line_color(engine, input.container_box_idx))
            .flatten();
        write_line_fragments(
            engine,
            input.container_box_idx,
            span,
            metrics,
            &tokens.replaced,
            input.area.origin + line.origin.to_vec2(),
            &line.layout,
            color,
            input.area.width,
            input.area.origin.x + if input.paragraph.direction == TextDirection::Rtl {
                input.area.width
            } else {
                0.0
            },
        );
        if adjustment.is_some() {
            adjustments.next();
        }
    }
    engine.record_timing(|timings| timings.emit_lines += started.elapsed());
}
