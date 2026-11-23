//! Per-line pseudo, overflow, and inline-strut preparation.
use html_text::TextStructure;
use rustc_data_structures::fx::FxHashSet;

use super::{
    InlineBoxStrut, InlineOverflow, InlineToken, InlineTokenKind, InlineTokenMetrics, InlineTokens,
    InlineVerticalAlign, LayoutReader, MetricsView, Range, TokenSpan, TokenSpanSummary,
    compute_baseline, inline_ancestors, text_vertical_align, token_contributes_line_metrics,
};
use crate::LayoutEngine;
use crate::inline::paragraph::{LineAdjustment, line_view};
use crate::inline::s02_paragraphs::ParagraphPolicy;
use crate::inline::storage::TokenData;

pub(super) fn strut_for_inline_box(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
) -> InlineBoxStrut {
    let style = engine.read.reader.style(box_idx);
    let font_size = style.font_size() as f64;
    let line_height = style.resolved_line_height();
    let (font_ascent, font_descent) =
        (engine.read.reader.font_metrics(box_idx)).line_box_extents(font_size);
    let half_leading = (line_height - font_ascent - font_descent) * 0.5;
    InlineBoxStrut {
        box_idx,
        ascent: font_ascent + half_leading,
        descent: font_descent + half_leading,
        line_height,
        font_size,
    }
}

pub(in crate::inline) fn inline_box_struts(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    tokens: TokenSpan<'_>,
    metrics: MetricsView<'_>,
) -> Vec<InlineBoxStrut> {
    let mut struts = Vec::new();
    let all_tokens = tokens;
    let all_metrics = metrics;
    let mut owners_with_boxes = None;
    for (tokens, metrics) in metrics.chunks(tokens) {
        let owner = metrics.owner_box_idx;
        if owner == u32::MAX {
            continue;
        }
        // Text owned by the formatting-context root has no inline ancestors
        // inside this context.  Walking from its parent would cross the
        // atomic/block boundary and incorrectly import an outer inline
        // strut into every line of an inline-block.
        if owner as usize == container_box_idx {
            continue;
        }
        let start = if metrics.contributes_line_metrics {
            if !tokens
                .metadata_iter()
                .any(|token| token_contributes_line_metrics(&token, metrics))
            {
                continue;
            }
            // Glyph boxes supply the direct owner's metrics; explicit struts
            // fill only the otherwise missing inline ancestors.
            engine.read.reader.get_parent(owner as usize)
        } else {
            // A control-only inline still has its CSS font strut. A sibling
            // glyph box or boundary already supplies that owner's metrics.
            // Build this lookup only for controls inside non-root inlines.
            let owners = owners_with_boxes.get_or_insert_with(|| {
                all_metrics
                    .chunks(all_tokens)
                    .filter_map(|(tokens, metrics)| {
                        tokens
                            .metadata_iter()
                            .any(|token| token_contributes_line_metrics(&token, metrics))
                            .then_some(metrics.owner_box_idx)
                    })
                    .collect::<FxHashSet<_>>()
            });
            if owners.contains(&owner) {
                continue;
            }
            Some(owner as usize)
        };
        for box_idx in inline_ancestors(&engine.read.reader, start) {
            if box_idx == container_box_idx {
                break;
            }
            if struts
                .iter()
                .any(|strut: &InlineBoxStrut| strut.box_idx == box_idx)
            {
                // An earlier walk already collected this ancestor and its parents.
                break;
            }
            struts.push(strut_for_inline_box(engine, box_idx));
        }
    }
    struts
}

/// Every CSS line box starts with the inline formatting root's zero-width
/// font strut. Descendant text can enlarge it, but a descendant with
/// `font-size: 0` cannot remove the parent's normal line height.
pub(in crate::inline) fn include_container_strut(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    first_line_height: Option<f64>,
    summary: &mut TokenSpanSummary,
) {
    let style = engine.read.reader.style(container_box_idx);
    let font_size = style.font_size() as f64;
    let line_height = first_line_height.unwrap_or_else(|| style.resolved_line_height());
    if !summary.has_baseline_relative_image && summary.line_height + f64::EPSILON >= line_height {
        return;
    }
    let (ascent, descent) =
        (engine.read.reader.font_metrics(container_box_idx)).line_box_extents(font_size);
    let half_leading = (line_height - ascent - descent) / 2.0;
    let max_ascent = summary.baseline.max(ascent + half_leading);
    let max_descent = (summary.line_height - summary.baseline).max(descent + half_leading);
    (summary.line_height, summary.baseline) = compute_baseline(max_ascent, max_descent, 0.0);
}

/// Applies pseudo-element line-height after line breaking. The shaper-aware
/// stage separately refines width-affecting `::first-line` font and spacing
/// properties, while this pass establishes the final line box strut.
pub(in crate::inline) fn apply_first_line_pseudo_styles(
    engine: &LayoutEngine<'_, '_>,
    tokens: &InlineTokens<'_>,
    range: Range<usize>,
    policy: &ParagraphPolicy,
    adjustment: &mut LineAdjustment,
) {
    if policy.first_line_height.is_none() && policy.first_letter_height.is_none() {
        return;
    }
    let (span, runs) = line_view(tokens, range, Some(adjustment));
    let first_line_normal = span
        .metrics_view(runs)
        .chunks(span)
        .next()
        .filter(|(_, metrics)| metrics.owner_box_idx != u32::MAX)
        .map(|(_, metrics)| {
            engine
                .read
                .reader
                .layout_tree()
                .flow_root(metrics.owner_box_idx as usize)
        })
        .and_then(|root| engine.read.text.content().pseudo_styles().first_line(root))
        .is_some_and(|indices| {
            engine
                .read
                .reader
                .used_style(indices)
                .line_height_is_normal()
        });

    // A uniform text line only needs a new metrics record. Its glyph IDs,
    // flags and widths still belong to the canonical paragraph columns.
    if let Some(height) = policy.first_line_height
        && policy.first_letter_height.is_none()
        && !span.is_empty()
        && let MetricsView::Uniform(metrics) = span.metrics_view(runs)
        && span.metadata_iter().all(|token| {
            matches!(
                token.kind(),
                InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. }
            )
        })
    {
        adjustment.runs = Some(
            vec![metrics_with_line_height(
                engine,
                *metrics,
                height,
                first_line_normal,
            )]
            .into_boxed_slice(),
        );
        return;
    }

    let mut storage = TokenData::default();
    span.append_to(&mut storage);
    let mut run_storage = runs.to_vec();
    if let Some(height) = policy.first_line_height {
        let mut previous_metrics = None;
        for index in 0..storage.len() {
            let mut token = storage.span().token_at(index);
            if !matches!(
                token.kind(),
                InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. }
            ) {
                continue;
            }
            let source = token.run_idx();
            if let Some((previous, rewritten)) = previous_metrics
                && source == previous
            {
                token.set_run_idx(rewritten);
            } else {
                let rewritten = set_token_line_height(
                    engine,
                    &mut token,
                    &mut run_storage,
                    height,
                    first_line_normal,
                );
                previous_metrics = Some((source, rewritten));
            }
            storage.update_header(index, |stored| stored.set_run_idx(token.run_idx()));
        }
    }
    if policy.first_letter_height.is_some() {
        for index in 0..storage.len() {
            let mut token = storage.span().token_at(index);
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                continue;
            };
            if let Some(style) = engine.read.text.first_letter_style(glyph_idx) {
                let style = engine.read.reader.used_style(style);
                set_token_line_height(
                    engine,
                    &mut token,
                    &mut run_storage,
                    style.resolved_line_height(),
                    style.line_height_is_normal(),
                );
                storage.update_header(index, |stored| stored.set_run_idx(token.run_idx()));
            }
        }
    }
    adjustment.tokens = Some(storage);
    adjustment.runs = Some(run_storage.into_boxed_slice());
}

fn set_token_line_height(
    engine: &LayoutEngine<'_, '_>,
    token: &mut InlineToken,
    runs: &mut Vec<InlineTokenMetrics>,
    height: f64,
    normal: bool,
) -> u32 {
    let metrics = metrics_with_line_height(engine, *token.run_metrics(runs), height, normal);
    let run_idx =
        u32::try_from(runs.len()).expect("inline pseudo run-metrics arena capacity exhausted");
    runs.push(metrics);
    token.set_run_idx(run_idx);
    run_idx
}

fn metrics_with_line_height(
    engine: &LayoutEngine<'_, '_>,
    mut metrics: InlineTokenMetrics,
    height: f64,
    normal: bool,
) -> InlineTokenMetrics {
    // A normal pseudo line height must not discard the actual fallback
    // font's larger layout bounds already established during tokenization.
    let height = if normal {
        height.max(f64::from(metrics.line_height))
    } else {
        height
    };
    metrics.line_height = height as f32;
    if metrics.owner_box_idx != u32::MAX {
        metrics.vertical_align = InlineVerticalAlign::resolve(
            text_vertical_align(&engine.read.reader, metrics.owner_box_idx as usize),
            height,
            metrics.font_size,
        );
    }
    metrics
}

/// Applies the block container's inline overflow policy to one already
/// broken line. Returns replacement columns only when an ellipsis rewrites it.
pub(in crate::inline) fn apply_inline_overflow(
    reader: &LayoutReader<'_>,
    span: TokenSpan<'_>,
    runs: &[InlineTokenMetrics],
    tab_origin: f64,
    line_width_limit: f64,
    overflow: InlineOverflow,
) -> Option<TokenData<'static>> {
    if !overflow.clips {
        return None;
    }

    let metrics = span.metrics_view(runs);
    let widths = span.widths();
    let mut full_width = 0.0;
    for (index, (token, record)) in metrics.iter(span).enumerate() {
        full_width +=
            token.advance_with_metrics(widths[index], reader, record, tab_origin + full_width);
    }
    if full_width <= line_width_limit {
        return None;
    }

    // `text-overflow: clip` clips paint at the content edge; it does not
    // remove inline-level boxes from layout. Keeping the original span is
    // especially important for an oversized atomic inline, whose subtree
    // must still be laid out before the ancestor overflow clip is applied.
    let Some(marker) = overflow.ellipsis else {
        return None;
    };

    let marker_width = marker.advance_at(reader, runs, tab_origin);
    let show_marker = marker_width <= line_width_limit;
    let prefix_limit = if show_marker {
        (line_width_limit - marker_width).max(0.0)
    } else {
        line_width_limit
    };
    let mut width = 0.0;
    let mut end = 0usize;
    for (idx, (token, record)) in metrics.iter(span).enumerate() {
        let advance = token.advance_with_metrics(widths[idx], reader, record, tab_origin + width);
        if width + advance > prefix_limit {
            break;
        }
        width += advance;
        end = idx + 1;
    }

    let mut storage = TokenData::default();
    storage.reserve(end + usize::from(show_marker));
    span.slice(..end).append_to(&mut storage);
    if show_marker {
        storage.push(marker);
    }
    Some(storage)
}
