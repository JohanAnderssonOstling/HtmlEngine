//! Segment prose and resolve paragraph policy before composition.
//! Stage 00 owns execution and restores token storage after each segment.
use super::InlineFormattingInput;
use super::reader::InlineReader;
use super::tokens::{InlineToken, InlineTokenMetrics, InlineVerticalAlign};
use html_engine_model::GlyphId;
use html_style_model::TextOverflow;
use html_style_model::{UsedStyleView, Visibility};
use html_text::TextStructure;

use super::tokens::{BreakKind, InlineTokenKind, InlineTokens, TokenMetadata};
use crate::engine::{LayoutEngine, LayoutReader};

/// Inputs shared by line selection and line geometry preparation.
pub(super) struct CompositionOptions {
    pub(super) has_float_exclusions: bool,
    pub(super) cache_break_plan: bool,
    pub(super) policy: ParagraphPolicy,
}

/// Resolved line policy shared by composition, line preparation, and cache identity.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ParagraphPolicy {
    pub(super) overflow: InlineOverflow,
    pub(super) first_line_height: Option<f64>,
    pub(super) first_letter_height: Option<f64>,
}

/// Resolve composition policy and bind synthetic ellipsis metrics before read-only layout.
pub(super) fn prepare_paragraph(
    engine: &LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
    tokens: &mut InlineTokens,
    has_float_exclusions: bool,
    cache_break_plan: bool,
) -> CompositionOptions {
    let overflow = inline_overflow(engine, input.container_box_idx, tokens);
    let (first_line_height, first_letter_height) = if input.starts_block {
        pseudo_line_heights(engine, input.container_box_idx)
    } else {
        (None, None)
    };
    CompositionOptions {
        has_float_exclusions,
        cache_break_plan,
        policy: ParagraphPolicy {
            overflow,
            first_line_height,
            first_letter_height,
        },
    }
}

/// Only exactly two authored BRs between prose passages form a paragraph
/// boundary. Preserve single breaks, larger runs, preformatted text and verse.
pub(super) fn prose_separators(
    reader: &LayoutReader<'_>,
    text: &InlineReader<'_>,
    input: &InlineFormattingInput,
    tokens: &InlineTokens,
) -> Vec<(usize, usize, f64)> {
    if !reader.is_reader_prose(input.container_box_idx)
        || tokens
            .metrics_view()
            .iter(tokens.span())
            .any(|(token, metrics)| {
                let ws = metrics.white_space;
                // Anchors are positioned before line composition; keep their
                // source flow intact instead of relocating already placed floats.
                ws.preserves_spaces()
                    || ws.preserves_newlines()
                    || matches!(
                        token.kind(),
                        InlineTokenKind::FloatAnchor { .. }
                            | InlineTokenKind::AbsoluteAnchor { .. }
                    )
            })
    {
        return Vec::new();
    }
    let authored_break = |token: &TokenMetadata| {
        token.is_authored_break()
            && matches!(
                token.kind(),
                InlineTokenKind::Break {
                    clear: html_style_model::Clear::None
                }
            )
    };
    let prose = |token: &TokenMetadata| {
        let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
            return false;
        };
        text.character(glyph_idx).is_alphanumeric()
    };
    let mut separators = Vec::new();
    let mut index = 0;
    let mut passage_start = 0;
    while index < tokens.len() {
        if !authored_break(&tokens.span().metadata_at(index)) {
            index += 1;
            continue;
        }
        let first = index;
        let mut last = index;
        let mut count = 0;
        while index < tokens.len() {
            let token = tokens.span().metadata_at(index);
            if authored_break(&token) {
                count += 1;
                last = index;
            } else if !(token.break_kind() == BreakKind::Soft
                && !tokens
                    .metrics_view()
                    .at(tokens.span(), index)
                    .white_space
                    .preserves_spaces())
                && !matches!(token.kind(), InlineTokenKind::InlineBoundary { .. })
            {
                break;
            }
            index += 1;
        }
        if count == 2
            && tokens
                .span()
                .slice(passage_start..first)
                .metadata_iter()
                .any(|token| prose(&token))
            && tokens
                .span()
                .slice(index..)
                .metadata_iter()
                .take_while(|token| !matches!(token.kind(), InlineTokenKind::Break { .. }))
                .any(|token| prose(&token))
        {
            let gap = reader
                .reader_paragraph_gap(input.container_box_idx)
                .unwrap_or_else(|| {
                    f64::from(tokens.metrics_view().at(tokens.span(), last).line_height)
                });
            separators.push((first, last, gap));
        }
        passage_start = last + 1;
    }
    separators
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct InlineOverflow {
    pub(super) clips: bool,
    pub(super) ellipsis: Option<InlineToken>,
}

fn inline_overflow(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    tokens: &mut InlineTokens,
) -> InlineOverflow {
    let style = engine.read.reader.style(container_box_idx);
    let clips = style.overflow_x().clips();
    let ellipsis = (clips && style.text_overflow() == TextOverflow::Ellipsis)
        .then(|| engine.read.text.ellipsis_glyph(container_box_idx))
        .flatten()
        .map(|glyph| {
            let (token, metrics) = create_ellipsis_token(engine, glyph, style);
            tokens.bind_metrics(token, metrics)
        });
    InlineOverflow { clips, ellipsis }
}

/// Returns pseudo-element line heights for the element that establishes
/// this inline formatting context. Anonymous flex/grid items deliberately
/// have no DOM node, so pseudo-elements on their container cannot leak into
/// the anonymous item's text.
fn pseudo_line_heights(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
) -> (Option<f64>, Option<f64>) {
    let tree = engine.read.reader.layout_tree();
    let root = tree.flow_root(container_box_idx);
    let line = engine
        .read
        .text
        .content()
        .pseudo_styles()
        .first_line(root)
        .map(|style| engine.read.reader.used_style(style).resolved_line_height());
    let letter = engine
        .read
        .text
        .content()
        .pseudo_styles()
        .first_letter(root)
        .map(|style| engine.read.reader.used_style(style).resolved_line_height());
    (line, letter)
}

fn create_ellipsis_token(
    engine: &LayoutEngine<'_, '_>,
    glyph: GlyphId,
    style: UsedStyleView,
) -> (InlineToken, InlineTokenMetrics) {
    let metric = engine.read.text.glyph_metric(glyph);
    let line_height = style.resolved_line_height();
    (
        InlineToken::new(
            InlineTokenKind::Ellipsis { glyph },
            metric.advance() as f64 + style.letter_spacing() as f64,
            BreakKind::None,
            false,
            true,
        ),
        InlineTokenMetrics {
            owner_box_idx: u32::MAX,
            ascent: metric.ascent(),
            descent: metric.descent(),
            line_height: line_height as f32,
            font_size: style.font_size(),
            vertical_align: InlineVerticalAlign::Baseline,
            white_space: style.white_space(),
            contributes_line_metrics: true,
            visible: style.visibility() == Visibility::Visible,
            placement_required: style.letter_spacing() != 0.0,
        },
    )
}
