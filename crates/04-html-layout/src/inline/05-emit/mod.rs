//! Publish paragraph-local results through the shared line-emission path.
use super::InlineFormattingInput;
use crate::LayoutEngine;
use html_text::TextStructure;
use kurbo::Point;
mod fragments;
mod output;
mod positions;
use super::metrics::MetricsView;
use super::paragraph::{ParagraphLayoutResult, line_view};
use super::tokens::InlineTokens;
pub(super) use fragments::LinePublication;
use html_style_model::TextDirection;

/// Project a paragraph-local result into the current chapter placement. Image,
/// atomic-box, decoration and absolute-anchor publication still runs here.
pub(super) fn emit_prepared_paragraph(
    engine: &mut LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
    tokens: &InlineTokens,
    paragraph: &mut ParagraphLayoutResult,
    retain_fragments: bool,
) {
    let mut adjustments = paragraph.adjustments.iter().peekable();
    for (index, line) in paragraph.lines.iter_mut().enumerate() {
        let adjustment = adjustments
            .peek()
            .filter(|entry| entry.line == index)
            .copied();
        let (span, runs) = line_view(tokens, line.tokens.clone(), adjustment);
        let metrics = MetricsView::direct(runs);
        let color = (index == 0 && input.starts_block)
            .then(|| first_line_color(engine, input.container_box_idx))
            .flatten();
        let origin = input.area.origin + line.origin.to_vec2();
        let point = Point::new(origin.x + line.layout.x_offset, origin.y);
        let line_idx = engine.fragments.line_len();
        let (text_fragments, inline_box_fragments) = fragments::prepare_fragments(
            engine,
            input.container_box_idx,
            span,
            metrics,
            &tokens.replaced,
            &line.layout,
            &mut line.publication,
            retain_fragments,
            input.area.width,
        );
        output::publish_line(
            engine,
            input.container_box_idx,
            span,
            metrics,
            point,
            line_idx,
            &line.layout,
            color,
            text_fragments,
            &inline_box_fragments,
            input.area.width,
        );
        output::publish_placements(
            engine,
            span,
            metrics,
            &tokens.replaced,
            point,
            line_idx,
            &line.layout,
            input.area.origin.x
                + if input.paragraph.direction == TextDirection::Rtl {
                    input.area.width
                } else {
                    0.0
                },
        );
        if adjustment.is_some() {
            adjustments.next();
        }
    }
}

fn first_line_color(engine: &LayoutEngine<'_, '_>, container_box_idx: usize) -> Option<u32> {
    let styles = engine.read.reader.styles();
    let tree = engine.read.reader.layout_tree();
    let root = tree.flow_root(container_box_idx);
    engine
        .read
        .text
        .content()
        .pseudo_styles()
        .first_line(root)
        .and_then(|style| styles.view(style))
        .map(|style| style.color())
}
