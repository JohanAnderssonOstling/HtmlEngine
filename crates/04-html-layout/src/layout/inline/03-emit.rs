//! Publish paragraph-local results through the shared line-emission path.
use super::InlineFormattingInput;
use super::fragments::{first_line_color, write_line_fragments};
use super::paragraph::ParagraphLayoutResult;
use super::tokens::InlineTokens;

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
        let span = adjustment.map_or_else(
            || &tokens[line.tokens.clone()],
            |entry| entry.tokens.as_ref(),
        );
        let runs = adjustment
            .and_then(|entry| entry.runs.as_deref())
            .unwrap_or(&tokens.runs);
        let color = (index == 0 && input.starts_block)
            .then(|| first_line_color(engine, input.container_box_idx))
            .flatten();
        write_line_fragments(
            engine,
            input.container_box_idx,
            span,
            runs,
            &tokens.replaced,
            input.area.origin + line.origin.to_vec2(),
            &line.layout,
            color,
            input.area.width,
        );
        if adjustment.is_some() {
            adjustments.next();
        }
    }
    engine.record_timing(|timings| timings.emit_lines += started.elapsed());
}
