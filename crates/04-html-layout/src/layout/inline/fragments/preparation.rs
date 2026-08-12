//! Per-line pseudo, overflow, and inline-strut preparation.

use super::*;

pub(super) fn strut_for_inline_box(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    box_idx: usize,
) -> InlineBoxStrut {
    let style = engine.reader.style(box_idx);
    let font_size = style.font_size() as f64;
    let line_height = if style.line_height_is_normal() && engine.reader.box_uses_ahem(box_idx) {
        font_size
    } else {
        resolved_line_height(style)
    };
    let (font_ascent, font_descent) = engine
        .reader
        .font_metrics(box_idx)
        .line_box_ratios()
        .map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| {
            (font_size * ascent as f64, font_size * descent as f64)
        });
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
    engine: &crate::layout::LayoutEngine<'_, '_>,
    box_idx: usize,
) -> (f64, f64) {
    let parent_idx = engine.reader.get_parent(box_idx).unwrap_or(box_idx);
    let style = engine.reader.style(parent_idx);
    let font_size = style.font_size() as f64;
    engine
        .reader
        .font_metrics(parent_idx)
        .line_box_ratios()
        .map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| {
            (font_size * ascent as f64, font_size * descent as f64)
        })
}

pub(super) fn inline_box_struts(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    container_box_idx: usize,
    tokens: &[InlineToken],
    runs: &[InlineTokenMetrics],
) -> Vec<InlineBoxStrut> {
    let mut struts = Vec::new();
    for token in tokens {
        if matches!(
            token.kind(),
            InlineTokenKind::Opportunity
                | InlineTokenKind::FloatAnchor { .. }
                | InlineTokenKind::AbsoluteAnchor { .. }
        ) {
            continue;
        }
        let owner = token.run_metrics(runs).owner_box_idx;
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
        // The token metrics are the direct owner's strut (or the replaced
        // element's own geometry). Explicit struts fill only the otherwise
        // missing non-replaced inline ancestors.
        let mut current = engine.reader.get_parent(owner as usize);
        while let Some(box_idx) = current {
            if box_idx == container_box_idx {
                break;
            }
            if !matches!(
                engine.reader.box_layout_mode(box_idx),
                Some(LayoutMode::Inline(_))
            ) {
                break;
            }
            if !struts
                .iter()
                .any(|strut: &InlineBoxStrut| strut.box_idx == box_idx)
            {
                struts.push(strut_for_inline_box(engine, box_idx));
            }
            current = engine.reader.get_parent(box_idx);
        }
    }
    struts
}

/// Every CSS line box starts with the inline formatting root's zero-width
/// font strut. Descendant text can enlarge it, but a descendant with
/// `font-size: 0` cannot remove the parent's normal line height.
pub(super) fn include_container_strut(
    engine: &crate::layout::LayoutEngine<'_, '_>,
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
            resolved_line_height(style)
        }
    });
    if !summary.has_baseline_relative_image && summary.line_height + f64::EPSILON >= line_height {
        return;
    }
    let ascent = engine
        .reader
        .font_metrics(container_box_idx)
        .ascent_ratio()
        .map_or(font_size * 0.8, |ratio| font_size * ratio as f64);
    let descent = font_size - ascent;
    let half_leading = (line_height - font_size) / 2.0;
    let max_ascent = summary.baseline.max(ascent + half_leading);
    let max_descent = (summary.line_height - summary.baseline).max(descent + half_leading);
    (summary.line_height, summary.baseline) = compute_baseline(max_ascent, max_descent, 0.0);
}

pub(super) fn first_line_color(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    container_box_idx: usize,
) -> Option<u32> {
    let styles = engine.reader.styles();
    let tree = engine.reader.layout_tree();
    let root = crate::shaping::whitespace_context_root(tree, container_box_idx);
    crate::shaping::first_line_style_for_inline_root(
        engine.reader.document(),
        styles,
        tree,
        engine.text.content(),
        root,
    )
    .and_then(|style| styles.view(style))
    .map(|style| style.color())
}

/// Returns pseudo-element line heights for the element that establishes
/// this inline formatting context. Anonymous flex/grid items deliberately
/// have no DOM node, so pseudo-elements on their container cannot leak into
/// the anonymous item's text.
pub(in crate::layout::inline) fn pseudo_line_heights(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    container_box_idx: usize,
) -> (Option<f64>, Option<f64>) {
    let styles = engine.reader.styles();
    let tree = engine.reader.layout_tree();
    let root = crate::shaping::whitespace_context_root(tree, container_box_idx);
    let resolve = |style: html_style_model::StyleView<'_>| {
        if style.line_height_is_normal() {
            style.font_size() as f64 * 1.2
        } else {
            style.line_height().max(0.0) as f64
        }
    };
    let line = crate::shaping::first_line_style_for_inline_root(
        engine.reader.document(),
        styles,
        tree,
        engine.text.content(),
        root,
    )
    .and_then(|style| styles.view(style))
    .map(resolve);
    let letter = crate::shaping::first_letter_style_for_inline_root(
        engine.reader.document(),
        styles,
        tree,
        engine.text.content(),
        root,
    )
    .and_then(|style| styles.view(style))
    .map(resolve);
    (line, letter)
}

/// Applies pseudo-element line-height after line breaking. The shaper-aware
/// stage separately refines width-affecting `::first-line` font and spacing
/// properties, while this pass establishes the final line box strut.
pub(super) fn apply_first_line_pseudo_styles<'b>(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    span: &'b [InlineToken],
    runs: &'b [InlineTokenMetrics],
    first_line_height: Option<f64>,
    first_letter_height: Option<f64>,
    storage: &'b mut Vec<InlineToken>,
    run_storage: &'b mut Vec<InlineTokenMetrics>,
) -> (&'b [InlineToken], &'b [InlineTokenMetrics]) {
    if first_line_height.is_none() && first_letter_height.is_none() {
        return (span, runs);
    }

    storage.extend_from_slice(span);
    run_storage.extend_from_slice(runs);
    if let Some(height) = first_line_height {
        for token in storage.iter_mut().filter(|token| {
            matches!(
                token.kind(),
                InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. }
            )
        }) {
            set_token_line_height(token, run_storage, height);
        }
    }
    if let Some(height) = first_letter_height {
        let mut selected_core = false;
        let mut selection_started = false;
        for token in storage.iter_mut() {
            let character = match token.kind() {
                InlineTokenKind::Glyph { glyph_idx } => {
                    let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
                    engine.text.glyph_metric(glyph).ch()
                }
                InlineTokenKind::Ellipsis { .. } => '\u{2026}',
                InlineTokenKind::InlineBoundary { .. } | InlineTokenKind::Opportunity => continue,
                _ if !selection_started => continue,
                _ => break,
            };
            if !selection_started && character.is_whitespace() {
                continue;
            }
            let qualifying_punctuation = is_first_letter_punctuation(character);
            let selected = if !selection_started {
                selection_started = true;
                if !qualifying_punctuation {
                    selected_core = true;
                }
                true
            } else if !selected_core && qualifying_punctuation {
                true
            } else if !selected_core {
                selected_core = true;
                true
            } else {
                qualifying_punctuation || character.is_mark()
            };
            if !selected {
                break;
            }
            set_token_line_height(token, run_storage, height);
        }
    }
    (storage.as_slice(), run_storage.as_slice())
}

fn is_first_letter_punctuation(character: char) -> bool {
    character.is_punctuation_open()
        || character.is_punctuation_close()
        || character.is_punctuation_initial_quote()
        || character.is_punctuation_final_quote()
        || character.is_punctuation_other()
}

fn set_token_line_height(token: &mut InlineToken, runs: &mut Vec<InlineTokenMetrics>, height: f64) {
    let mut metrics = *token.run_metrics(runs);
    metrics.line_height = height;
    let run_idx =
        u32::try_from(runs.len()).expect("inline pseudo run-metrics arena capacity exhausted");
    runs.push(metrics);
    token.run_idx = run_idx;
}

/// Applies the block container's inline overflow policy to one already
/// broken line. The borrowed source span is returned unchanged on the hot
/// path; storage is populated only when a line actually overflows.
pub(super) fn apply_inline_overflow<'b>(
    span: &'b [InlineToken],
    runs: &[InlineTokenMetrics],
    tab_origin: f64,
    line_width_limit: f64,
    overflow: InlineOverflow,
    storage: &'b mut Vec<InlineToken>,
) -> &'b [InlineToken] {
    if !overflow.clips {
        return span;
    }

    let mut full_width = 0.0;
    for token in span {
        full_width += token.advance_at(runs, tab_origin + full_width);
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

    let marker_width = marker.advance_at(runs, tab_origin);
    let show_marker = marker_width <= line_width_limit;
    let prefix_limit = if show_marker {
        (line_width_limit - marker_width).max(0.0)
    } else {
        line_width_limit
    };
    let mut width = 0.0;
    let mut end = 0usize;
    for (idx, token) in span.iter().enumerate() {
        let advance = token.advance_at(runs, tab_origin + width);
        if width + advance > prefix_limit {
            break;
        }
        width += advance;
        end = idx + 1;
    }

    storage.extend_from_slice(&span[..end]);
    if show_marker {
        storage.push(marker);
    }
    storage.as_slice()
}
