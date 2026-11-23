//! Turn selected breaks into line geometry, applying overflow, pseudo styles, and bidi.
//! Float-aware composition can request one line at a time before choosing the next break.
use super::InlineFormattingInput;
use super::fragments::{
    InlineBoxStrut, LineLayout, LinePlacements, TextCoverage, TokenPlacement, TokenSpanSummary,
    inline_ancestors, inline_relative_position_offset, parent_font_content_extents,
    token_contributes_line_metrics,
};
use super::s05_emit::LinePublication;
use super::{
    INLINE_SUMMARY_BLOCK_TOKENS, InlineBidiSummary, InlineSummaryBlock, InlineToken,
    InlineTokenMetrics, LayoutReader, Range, ReplacedToken, TokenMetadata, TokenSpan, Vec2,
    VerticalAlignValue, WhiteSpace, compute_baseline, has_aligned_inline_ancestor,
    may_be_bidi_separator, text_vertical_align, vertical_align_metrics_offset,
};
mod alignment;
mod bidi;
mod measurement;
mod preparation;
use super::metrics::MetricsView;
use super::paragraph::{LineAdjustment, ParagraphLayoutResult, PreparedLine, line_view};
use super::s02_paragraphs::{InlineOverflow, ParagraphPolicy};
use super::tokens::{InlineTokenKind, InlineTokens, InlineVerticalAlign};
use crate::LayoutEngine;
use crate::inline::s03_compose::wrapping::BrokenLine;
use alignment::build_token_placements;
use alignment::{adjusted_line_metrics, can_stream_text_placements};
use bidi::place_visual_line;
use html_engine_model::paragraph_unicode_bidi;
use html_style_model::{LogicalTextAlign, TextAlign, TextDirection, UnicodeBidi};
use kurbo::{Point, Size};
#[cfg(any(test, feature = "testing"))]
pub(super) use measurement::measured_tracking_boundary_capacity;
use measurement::{measure_line, summarize_token_span};
use preparation::{
    apply_first_line_pseudo_styles, apply_inline_overflow, include_container_strut,
    inline_box_struts, strut_for_inline_box,
};

/// Used alignment can differ from the selected break for plaintext paragraphs.
#[derive(Clone, Copy)]
struct LineAlignment {
    direction: TextDirection,
    align: TextAlign,
}

/// Paragraph-wide inputs shared by every line emitted from one token plan.
pub(super) struct LinePreparationContext<'a> {
    pub(super) tokens: &'a InlineTokens<'a>,
    pub(super) input: &'a InlineFormattingInput,
    pub(super) policy: &'a ParagraphPolicy,
    rewrite_first_line_metrics: bool,
    plaintext_align: Option<[LogicalTextAlign; 2]>,
}

impl<'a> LinePreparationContext<'a> {
    pub(super) fn new(
        reader: &LayoutReader<'_>,
        tokens: &'a InlineTokens<'a>,
        input: &'a InlineFormattingInput,
        policy: &'a ParagraphPolicy,
    ) -> Self {
        // Text metrics resolve baseline alignment at token construction;
        // it cannot change with height. Prove unchanged metrics once for
        // this paragraph, without reading token headers or owner styles.
        let rewrite_first_line_metrics = policy.first_letter_height.is_some()
            || policy.first_line_height.is_some_and(|height| {
                tokens.runs.iter().any(|metrics| {
                    metrics.line_height.to_bits() != (height as f32).to_bits()
                        || (metrics.owner_box_idx != u32::MAX
                            && !matches!(metrics.vertical_align, InlineVerticalAlign::Baseline))
                })
            });
        Self {
            tokens,
            input,
            policy,
            rewrite_first_line_metrics,
            plaintext_align: (paragraph_unicode_bidi(
                reader.styles(),
                reader.layout_tree(),
                input.container_box_idx,
            ) == UnicodeBidi::Plaintext)
                .then(|| {
                    let style = reader.style(input.container_box_idx);
                    [style.text_align_logical(), style.text_align_last_logical()]
                }),
        }
    }
}

/// Build a position-independent result without touching published output.
pub(super) fn prepare_selected_paragraph(
    engine: &mut LayoutEngine<'_, '_>,
    context: &LinePreparationContext<'_>,
    lines: &[BrokenLine],
    paragraph: &mut ParagraphLayoutResult,
) {
    paragraph.lines.clear();
    paragraph.adjustments.clear();
    // Selected lines already supply the exact size. Avoid growing and then
    // shrinking the retained vector during cold composition.
    if paragraph.lines.capacity() < lines.len() {
        paragraph.lines.reserve_exact(lines.len());
    }
    let mut y = 0.0;
    let mut width = 0.0f64;
    for (index, selected) in lines.iter().enumerate() {
        let (line, measured_width) = prepare_line(
            engine,
            context,
            selected,
            index == 0,
            Point::new(0.0, y),
            index,
            &mut paragraph.adjustments,
        );

        y += line.height();
        paragraph.lines.push(line);
        width = width.max(measured_width);
    }
    paragraph.size = Size::new(width, y);
}

pub(super) fn prepare_line(
    engine: &mut crate::LayoutEngine<'_, '_>,
    context: &LinePreparationContext<'_>,
    line: &BrokenLine,
    is_first_line: bool,
    origin: Point,
    line_index: usize,
    adjustments: &mut Vec<LineAdjustment>,
) -> (PreparedLine, f64) {
    let original_span = context.tokens.span().slice(line.start..line.end);
    let direction = if context.plaintext_align.is_some() {
        original_span
            .metadata_iter()
            .find_map(|token| match token.kind() {
                InlineTokenKind::Glyph { glyph_idx } => {
                    Some(engine.read.text.paragraph_direction(glyph_idx))
                }
                _ => None,
            })
            .unwrap_or(context.input.paragraph.direction)
    } else {
        context.input.paragraph.direction
    };
    let alignment = LineAlignment {
        direction,
        align: context.plaintext_align.map_or(line.align, |align| {
            match (align[usize::from(line.is_last_line)], direction) {
                (LogicalTextAlign::Start, TextDirection::Rtl)
                | (LogicalTextAlign::End, TextDirection::Ltr) => TextAlign::Right,
                (LogicalTextAlign::Start, TextDirection::Ltr)
                | (LogicalTextAlign::End, TextDirection::Rtl) => TextAlign::Left,
                _ => line.align,
            }
        }),
    };
    let mut adjustment = LineAdjustment {
        line: line_index,
        tokens: apply_inline_overflow(
            &engine.read.reader,
            original_span,
            &context.tokens.runs,
            line.tab_origin,
            line.width_limit,
            context.policy.overflow,
        ),
        runs: None,
    };
    let is_first_line = is_first_line && context.input.starts_block;
    if is_first_line && context.rewrite_first_line_metrics {
        apply_first_line_pseudo_styles(
            engine,
            context.tokens,
            line.start..line.end,
            context.policy,
            &mut adjustment,
        );
    }
    let (span, line_runs) = line_view(context.tokens, line.start..line.end, Some(&adjustment));
    let summary_offset = (!adjustment.is_rewritten()).then_some(line.start);
    let metrics = MetricsView::direct(line_runs);
    let inline_struts = inline_box_struts(engine, context.input.container_box_idx, span, metrics);
    let mut summary = summarize_token_span(
        &engine.read.reader,
        &engine.read.text,
        &context.tokens.replaced,
        span,
        metrics,
        &context.tokens.summary_blocks,
        summary_offset,
        line.tab_origin,
        &inline_struts,
    );
    include_container_strut(
        engine,
        context.input.container_box_idx,
        is_first_line
            .then_some(context.policy.first_line_height)
            .flatten(),
        &mut summary,
    );
    let bidi = summary.bidi;
    let mut layout = measure_line(
        engine,
        span,
        metrics,
        summary,
        line,
        &inline_struts,
        context,
        alignment,
    );
    place_visual_line(
        engine,
        span,
        line_runs,
        line,
        context,
        alignment,
        bidi,
        &mut layout,
    );
    layout.publishes_line =
        line_publishes(engine, span, metrics, &context.tokens.replaced, &layout);
    let width = layout.width + line.indent;
    if adjustment.is_rewritten() {
        adjustments.push(adjustment);
    }
    let indent = if direction == TextDirection::Rtl {
        0.0
    } else {
        line.indent
    };
    (
        PreparedLine {
            tokens: line.start..line.end,
            origin: Point::new(origin.x + indent, origin.y),
            layout,
            publication: LinePublication::default(),
        },
        width,
    )
}

fn line_publishes(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
    line: &LineLayout,
) -> bool {
    if line.placements.is_none() && !line.glyph_range.is_empty() {
        return true;
    }
    let widths = span.widths();
    line.publish_empty
        || metrics
            .iter(span)
            .enumerate()
            .any(|(index, (token, metrics))| match token.kind() {
                InlineTokenKind::FloatAnchor { .. }
                | InlineTokenKind::AbsoluteAnchor { .. }
                | InlineTokenKind::ZeroWidthBreak => false,
                InlineTokenKind::InlineBoundary { payload_idx } => {
                    match replaced.get(payload_idx as usize) {
                        Some(ReplacedToken::InlineBoundary {
                            box_idx,
                            inline_start,
                            inline_end,
                            ..
                        }) => {
                            let style = engine.read.reader.style(*box_idx as usize);
                            widths[index] != 0.0
                                || (*inline_start
                                    && (!style.padding_left().is_zero()
                                        || style.border_left_width() != 0.0))
                                || (*inline_end
                                    && (!style.padding_right().is_zero()
                                        || style.border_right_width() != 0.0))
                                || !style.margin_top().is_zero()
                                || !style.margin_bottom().is_zero()
                                || !style.padding_top().is_zero()
                                || !style.padding_bottom().is_zero()
                                || style.border_top_width() != 0.0
                                || style.border_bottom_width() != 0.0
                        }
                        _ => false,
                    }
                }
                InlineTokenKind::Glyph { glyph_idx } => {
                    let character = engine.read.text.character(glyph_idx);
                    widths[index] != 0.0
                        || !character.is_whitespace()
                        || metrics.white_space.preserves_spaces()
                }
                _ => true,
            })
}
