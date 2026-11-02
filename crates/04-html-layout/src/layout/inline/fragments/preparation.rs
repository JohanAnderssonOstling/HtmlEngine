//! Per-line pseudo, overflow, and inline-strut preparation.
use html_text::whitespace_context_root;
use rustc_data_structures::fx::FxHashSet;

use super::*;
use crate::layout::LayoutEngine;

pub(super) fn strut_for_inline_box(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
) -> InlineBoxStrut {
    let style = engine.reader.style(box_idx);
    let font_size = style.font_size() as f64;
    let line_height = if style.line_height_is_normal() && engine.reader.box_uses_ahem(box_idx) {
        font_size
    } else {
        style.resolved_line_height()
    };
    let (font_ascent, font_descent) = engine
        .reader
        .font_metrics(box_idx)
        .line_box_extents(font_size);
    let half_leading = (line_height - font_ascent - font_descent) * 0.5;
    InlineBoxStrut {
        box_idx,
        ascent: font_ascent + half_leading,
        descent: font_descent + half_leading,
        line_height,
        font_size,
    }
}

pub(super) fn parent_font_content_extents(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
) -> (f64, f64) {
    let parent_idx = engine.reader.get_parent(box_idx).unwrap_or(box_idx);
    let style = engine.reader.style(parent_idx);
    let font_size = style.font_size() as f64;
    engine
        .reader
        .font_metrics(parent_idx)
        .line_box_extents(font_size)
}

pub(super) fn inline_box_struts(
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
            engine.reader.get_parent(owner as usize)
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
        for box_idx in inline_ancestors(&engine.reader, start) {
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
pub(super) fn include_container_strut(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    first_line_height: Option<f64>,
    summary: &mut TokenSpanSummary,
) {
    let style = engine.reader.style(container_box_idx);
    let font_size = style.font_size() as f64;
    let line_height = first_line_height.unwrap_or_else(|| {
        if style.line_height_is_normal() && engine.reader.box_uses_ahem(container_box_idx) {
            font_size
        } else {
            style.resolved_line_height()
        }
    });
    if !summary.has_baseline_relative_image && summary.line_height + f64::EPSILON >= line_height {
        return;
    }
    let (ascent, descent) = engine
        .reader
        .font_metrics(container_box_idx)
        .line_box_extents(font_size);
    let half_leading = (line_height - ascent - descent) / 2.0;
    let max_ascent = summary.baseline.max(ascent + half_leading);
    let max_descent = (summary.line_height - summary.baseline).max(descent + half_leading);
    (summary.line_height, summary.baseline) = compute_baseline(max_ascent, max_descent, 0.0);
}

pub(in crate::layout::inline) fn first_line_color(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
) -> Option<u32> {
    let styles = engine.reader.styles();
    let tree = engine.reader.layout_tree();
    let root = whitespace_context_root(tree, container_box_idx);
    engine
        .text
        .content()
        .pseudo_styles()
        .first_line(root)
        .and_then(|style| styles.view(style))
        .map(|style| style.color())
}

/// Returns pseudo-element line heights for the element that establishes
/// this inline formatting context. Anonymous flex/grid items deliberately
/// have no DOM node, so pseudo-elements on their container cannot leak into
/// the anonymous item's text.
pub(in crate::layout::inline) fn pseudo_line_heights(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
) -> (Option<f64>, Option<f64>) {
    let tree = engine.reader.layout_tree();
    let root = whitespace_context_root(tree, container_box_idx);
    let line = engine
        .text
        .content()
        .pseudo_styles()
        .first_line(root)
        .map(|style| engine.reader.used_style(style).resolved_line_height());
    let letter = engine
        .text
        .content()
        .pseudo_styles()
        .first_letter(root)
        .map(|style| engine.reader.used_style(style).resolved_line_height());
    (line, letter)
}

/// Applies pseudo-element line-height after line breaking. The shaper-aware
/// stage separately refines width-affecting `::first-line` font and spacing
/// properties, while this pass establishes the final line box strut.
pub(super) fn apply_first_line_pseudo_styles<'b>(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'b>,
    runs: &'b [InlineTokenMetrics],
    first_line_height: Option<f64>,
    first_letter_height: Option<f64>,
    storage: &'b mut Vec<InlineToken>,
    run_storage: &'b mut Vec<InlineTokenMetrics>,
) -> (TokenSpan<'b>, &'b [InlineTokenMetrics]) {
    if first_line_height.is_none() && first_letter_height.is_none() {
        return (span, runs);
    }

    let first_line_normal = span.metrics_view(runs).chunks(span).next()
        .filter(|(_, metrics)| metrics.owner_box_idx != u32::MAX)
        .map(|(_, metrics)| whitespace_context_root(engine.reader.layout_tree(), metrics.owner_box_idx as usize))
        .and_then(|root| engine.text.content().pseudo_styles().first_line(root))
        .is_some_and(|indices| engine.reader.used_style(indices).line_height_is_normal());

    // A uniform text line only needs a new metrics record. Its glyph IDs,
    // flags and widths still belong to the canonical paragraph columns.
    if let Some(height) = first_line_height
        && first_letter_height.is_none()
        && !span.is_empty()
        && let MetricsView::Uniform(metrics) = span.metrics_view(runs)
        && span.metadata_iter().all(|token| {
            matches!(
                token.kind(),
                InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. }
            )
        })
    {
        run_storage.push(metrics_with_line_height(engine, *metrics, height, first_line_normal));
        return (span, run_storage.as_slice());
    }

    span.append_to(storage);
    run_storage.extend_from_slice(runs);
    if let Some(height) = first_line_height {
        let mut previous_metrics = None;
        for token in storage.iter_mut().filter(|token| {
            matches!(
                token.kind(),
                InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. }
            )
        }) {
            let source = token.run_idx();
            if let Some((previous, rewritten)) = previous_metrics
                && source == previous
            {
                token.set_run_idx(rewritten);
            } else {
                let rewritten = set_token_line_height(engine, token, run_storage, height, first_line_normal);
                previous_metrics = Some((source, rewritten));
            }
        }
    }
    if first_letter_height.is_some() {
        for token in storage.iter_mut() {
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                continue;
            };
            if let Some(style) = engine.text.first_letter_style(glyph_idx) {
                let style = engine.reader.used_style(style);
                set_token_line_height(engine, token, run_storage, style.resolved_line_height(), style.line_height_is_normal());
            }
        }
    }
    (storage.as_slice().into(), run_storage.as_slice())
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
    let height = if normal { height.max(f64::from(metrics.line_height)) } else { height };
    metrics.line_height = height as f32;
    if metrics.owner_box_idx != u32::MAX {
        metrics.vertical_align = InlineVerticalAlign::resolve(
            text_vertical_align(engine, metrics.owner_box_idx as usize),
            height,
            metrics.font_size,
        );
    }
    metrics
}

/// Applies the block container's inline overflow policy to one already
/// broken line. The borrowed source span is returned unchanged on the hot
/// path; storage is populated only when a line actually overflows.
pub(super) fn apply_inline_overflow<'b>(
    reader: &LayoutReader<'_>,
    span: TokenSpan<'b>,
    runs: &[InlineTokenMetrics],
    tab_origin: f64,
    line_width_limit: f64,
    overflow: InlineOverflow,
    storage: &'b mut Vec<InlineToken>,
) -> TokenSpan<'b> {
    if !overflow.clips {
        return span;
    }

    let metrics = span.metrics_view(runs);
    let widths = span.widths();
    let mut full_width = 0.0;
    for (index, (token, record)) in metrics.iter(span).enumerate() {
        full_width +=
            token.advance_with_metrics(widths.at(index), reader, record, tab_origin + full_width);
    }
    if full_width <= line_width_limit {
        return span;
    }

    // `text-overflow: clip` clips paint at the content edge; it does not
    // remove inline-level boxes from layout. Keeping the original span is
    // especially important for an oversized atomic inline, whose subtree
    // must still be laid out before the ancestor overflow clip is applied.
    let Some(marker) = overflow.ellipsis else {
        return span;
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
        let advance =
            token.advance_with_metrics(widths.at(idx), reader, record, tab_origin + width);
        if width + advance > prefix_limit {
            break;
        }
        width += advance;
        end = idx + 1;
    }

    storage.reserve(end + usize::from(show_marker));
    span.slice(..end).append_to(storage);
    if show_marker {
        storage.push(marker);
    }
    storage.as_slice().into()
}
