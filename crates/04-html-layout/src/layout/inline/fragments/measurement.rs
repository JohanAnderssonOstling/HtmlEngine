//! Token-span measurement, optical margins, and justification.

use super::*;
use crate::layout::inline::justification::{tracking_boundary_limit, tracking_width_limit};
use html_text::is_other_space_separator;
use unicode_bidi::{BidiInfo, Level};

pub(super) fn measure_line(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    tokens: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    mut summary: TokenSpanSummary,
    line: &BrokenLine,
    empty_break_metrics: Option<InlineTokenMetrics>,
    inline_struts: &[InlineBoxStrut],
    container_box_idx: usize,
    containing_width: f64,
    direction: TextDirection,
) -> LineLayout {
    profile_scope!("lines.measure");
    engine.record_timing(|timings| timings.measured_lines += 1);
    let timing_started = engine.start_timing();
    let hyphen =
        line.hyphen
            .map(|(glyph, width)| (glyph, f64::from(width)))
            .or_else(|| {
                tokens.len().checked_sub(1).and_then(|index| {
                    match tokens.metadata_at(index).kind() {
                        InlineTokenKind::Discretionary { glyph } => {
                            Some((glyph, f64::from(tokens.stored_width_at(index))))
                        }
                        _ => None,
                    }
                })
            });
    if let Some((_, width)) = hyphen {
        summary.width += width;
    }
    let (trailing_start, hanging_width) =
        hanging_space_width(engine, tokens, metrics, line, summary.width);
    summary.width -= hanging_width;
    if let Some(metrics) = empty_break_metrics {
        let ascent = f64::from(metrics.ascent);
        let descent = f64::from(metrics.descent);
        let half_leading = (f64::from(metrics.line_height) - (ascent + descent)) / 2.0;
        summary.line_height = f64::from(metrics.line_height);
        summary.baseline = ascent + half_leading;
    }
    let (optical_start, optical_end) = optical_margin_protrusion(engine, &summary, line.align);
    // Authored hanging already participates in line selection and the line
    // origin. Reader-mode optical margins must not hang the same edge twice.
    let style = engine.reader.style(container_box_idx);
    let hanging = style.hanging_punctuation();
    let (start, end) = if style.direction() == TextDirection::Ltr {
        (hanging.first, hanging.last)
    } else {
        (hanging.last, hanging.first)
    };
    let optical_start = if start { 0.0 } else { optical_start };
    let optical_end = if end { 0.0 } else { optical_end };
    let hanging_tab = hanging_width > 0.0
        && tokens
            .slice(trailing_start..tokens.len())
            .metadata_iter()
            .any(|token| token.is_tab());
    let justification_width = if hanging_tab {
        // All content precedes the final tab stop. There is no suffix to
        // expand, even though hanging-space measurement excludes that tab.
        summary.width
    } else if line.align == TextAlign::Justify {
        line.width_limit + optical_start + optical_end
    } else {
        line.width_limit
    };
    let (line_height, baseline, spacing) = base_line_metrics(
        engine,
        if hanging_width > 0.0 {
            tokens.slice(0..trailing_start)
        } else {
            tokens
        },
        &summary,
        justification_width,
        line.is_last_line,
        line.align,
    );
    let JustificationSpacing {
        word_spacing,
        letter_spacing,
        word_adjustment,
        tracking_adjustment,
        mut punctuation_space_advances,
    } = spacing;
    // Hanging end spaces keep natural advances during justification. CSS
    // removes trailing Ogham in collapsing modes and permits visual collapse
    // of hanging pre-wrap spaces. Retain all source positions as overrides.
    let adjust_trailing = hanging_width > 0.0 && (word_spacing != 0.0 || letter_spacing != 0.0);
    // Phase II trims Ogham only when it is actually trailing. An Ogham
    // followed by another kind of space separator belongs to the retained
    // hanging sequence and must still occupy its background/paint advance.
    let mut trim_ogham_start = tokens.len();
    for index in (trailing_start..tokens.len()).rev() {
        let token = tokens.metadata_at(index);
        if matches!(token.kind(), InlineTokenKind::Opportunity)
            || (matches!(token.kind(), InlineTokenKind::InlineBoundary { .. })
                && tokens.stored_width_at(index) == 0.0) { continue; }
        let InlineTokenKind::Glyph { glyph_idx } = token.kind() else { break; };
        if metrics.at(tokens, index).white_space.collapses_spaces()
            && matches!(engine.text.character(glyph_idx), ' ' | '\t' | '\u{1680}') {
            trim_ogham_start = index;
        } else { break; }
    }
    let mut removed_width = 0.0;
    let mut position = line.tab_origin + summary.width + word_adjustment + tracking_adjustment;
    for index in trailing_start..tokens.len() {
        let token = tokens.metadata_at(index);
        let white_space = metrics.at(tokens, index).white_space;
        let natural = token.advance_with_metrics(
            tokens.stored_width_at(index),
            &engine.reader,
            metrics.at(tokens, index),
            position,
        );
        let mut advance = natural;
        if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
            let character = engine.text.character(glyph_idx);
            let remove = (character == '\u{1680}' && white_space.collapses_spaces()
                && index >= trim_ogham_start)
                || (character == ' '
                    && white_space == WhiteSpace::PreWrap
                    && line.align == TextAlign::Justify
                    && hanging_width > 0.0);
            if remove {
                removed_width += natural;
                advance = 0.0;
            }
            if remove || adjust_trailing {
                punctuation_space_advances.push((glyph_idx, advance as f32));
            }
        }
        position += advance;
    }
    let (line_height, baseline) = if summary.plain_text {
        (line_height, baseline)
    } else {
        adjusted_line_metrics(
            engine,
            tokens,
            metrics,
            inline_struts,
            container_box_idx,
            line_height,
            baseline,
        )
    };
    let x_offset = line_x_offset(line.width_limit,
        summary.width + word_adjustment + tracking_adjustment, line.align, direction)
        - if style.direction() == TextDirection::Rtl {
            hanging_width - removed_width
        } else {
            0.0
        };
    let optical_offset_x = match line.align {
        TextAlign::Left | TextAlign::Justify => -optical_start,
        TextAlign::Right => optical_end,
        TextAlign::Center => 0.0,
    };
    let placements = (!summary.plain_text).then(|| {
        if direction == TextDirection::Ltr
            && !summary.bidi.has_odd_level
            && let Some(publish) = can_stream_text_placements(engine, tokens, metrics)
        {
            return LinePlacements::Sequential {
                tab_origin: line.tab_origin,
                publish,
            };
        }
        LinePlacements::Full(
            build_token_placements(
                engine,
                tokens,
                metrics,
                line.tab_origin,
                line_height,
                baseline,
                containing_width,
            )
            .into_boxed_slice(),
        )
    });

    let adjusted_width = summary.width + word_adjustment + tracking_adjustment;
    let hyphen = hyphen.map(|(glyph, width)| (glyph, (adjusted_width - width).max(0.0)));
    let layout = LineLayout {
        glyph_range: summary.glyph_range,
        owns_glyph_range: summary.text_coverage == TextCoverage::WholeRange,
        width: summary.width,
        used_width: adjusted_width,
        x_offset,
        line_height,
        baseline,
        word_spacing,
        letter_spacing,
        punctuation_space_advances,
        optical_offset_x,
        placements,
        hyphen,
        publish_empty: empty_break_metrics.is_some(),
    };
    engine.record_timing(|t| t.measure_line += timing_started.elapsed());
    layout
}

fn hanging_space_width(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    tokens: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    line: &BrokenLine,
    width: f64,
) -> (usize, f64) {
    let mut start = tokens.len();
    let mut unconditional = 0.0;
    let mut conditional = 0.0;
    let mut has_tab = false;
    while start > 0 {
        let index = start - 1;
        let token = tokens.metadata_at(index);
        if matches!(token.kind(), InlineTokenKind::Opportunity)
            || (matches!(token.kind(), InlineTokenKind::InlineBoundary { .. })
                && tokens.stored_width_at(index) == 0.0)
        {
            start = index;
            continue;
        }
        let InlineTokenKind::Glyph { glyph_idx } = token.kind() else { break };
        let character = engine.text.character(glyph_idx);
        let white_space = metrics.at(tokens, index).white_space;
        if !(matches!(character, ' ' | '\t') || is_other_space_separator(character))
            || !(white_space.collapses_spaces() || white_space == WhiteSpace::PreWrap) {
            break;
        }
        has_tab |= token.is_tab();
        let advance = token.width(tokens.stored_width_at(index));
        if white_space == WhiteSpace::PreWrap && line.is_last_line {
            conditional += advance;
        } else {
            unconditional += advance;
        }
        start = index;
    }
    if has_tab {
        // Tab advances depend on their position from the block's start edge.
        unconditional = 0.0;
        conditional = 0.0;
        let mut position = line.tab_origin;
        for index in 0..tokens.len() {
            let token = tokens.metadata_at(index);
            let style = metrics.at(tokens, index);
            let advance = token.advance_with_metrics(
                tokens.stored_width_at(index), &engine.reader, style, position,
            );
            position += advance;
            if index >= start {
                if style.white_space == WhiteSpace::PreWrap && line.is_last_line {
                    conditional += advance;
                } else {
                    unconditional += advance;
                }
            }
        }
    }
    (start, unconditional + conditional.min((width - unconditional - line.width_limit).max(0.0)))
}

/// Returns bounded start/end punctuation protrusion without changing the
/// breaker's width model. Only plain text in book mode participates.
fn optical_margin_protrusion(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    summary: &TokenSpanSummary,
    align: TextAlign,
) -> (f64, f64) {
    if !engine.config.book_optimized_text()
        || !summary.plain_text
        || summary.glyph_range.is_empty()
        || align == TextAlign::Center
    {
        return (0.0, 0.0);
    }
    let visible = |index: u32| {
        let character = engine.text.character(index);
        if character.is_whitespace() || character == '\u{00ad}' {
            return None;
        }
        let metric = engine
            .text
            .glyph_metric(engine.text.glyph_at(index as usize).unwrap_or_default());
        Some((index, metric))
    };
    let (first, last) = if summary.bidi.has_odd_level
        && summary
            .bidi
            .uniform_level()
            .is_some_and(|level| Level::new(level).is_ok())
    {
        // Uniform odd levels reverse the logical glyph order. Finding the
        // visible edges needs neither a per-glyph level lookup nor a map.
        (
            summary.glyph_range.clone().rev().find_map(visible),
            summary.glyph_range.clone().find_map(visible),
        )
    } else if summary.bidi.has_odd_level {
        let levels: Vec<_> = summary
            .glyph_range
            .clone()
            .map(|index| Level::new(engine.text.bidi_level(index)).unwrap_or(Level::ltr()))
            .collect();
        let order = BidiInfo::reorder_visual(&levels);
        (
            order
                .iter()
                .find_map(|&index| visible(summary.glyph_range.start + index as u32)),
            order
                .iter()
                .rev()
                .find_map(|&index| visible(summary.glyph_range.start + index as u32)),
        )
    } else {
        (
            summary.glyph_range.clone().find_map(visible),
            summary.glyph_range.clone().rev().find_map(visible),
        )
    };
    let protrusion = |entry: Option<(u32, crate::layout_model::GlyphMetric)>, start: bool| {
        let Some((index, metric)) = entry else {
            return 0.0;
        };
        let character = engine.text.character(index);
        let character = if engine.text.bidi_level(index) % 2 == 1 {
            unicode_bidi_mirroring::get_mirrored(character).unwrap_or(character)
        } else {
            character
        };
        let fraction = match (start, character) {
            (true, '“' | '‘' | '"' | '\'') | (false, '”' | '’' | '"' | '\'') => 0.65,
            (true, '«' | '‹') | (false, '»' | '›') => 0.5,
            (false, '.' | ',') => 0.5,
            (false, '…') => 0.4,
            (false, ':' | ';' | '!' | '?') => 0.3,
            (false, '-' | '‐' | '‑' | '–') => 0.25,
            (false, '—' | ')' | ']' | '}') => 0.2,
            _ => 0.0,
        };
        let advance = engine.text.text_advance(index as usize, metric.advance()) as f64;
        (advance * fraction)
            .min((metric.ascent() + metric.descent()) as f64 * 0.35)
            .max(0.0)
    };
    (protrusion(first, true), protrusion(last, false))
}

/// Computes the horizontal start offset for non-justified aligned lines.
///
/// Inline placement uses this to support `left`, `center`, and `right`
/// alignment instead of always starting each line at x=0.
fn line_x_offset(line_width_limit: f64, line_width: f64, text_align: TextAlign, direction: TextDirection) -> f64 {
    let remaining = line_width_limit - line_width;
    // CSS Text aligns overflowing content to the line's start edge,
    // irrespective of its requested physical alignment.
    if remaining < 0.0 {
        return if direction == TextDirection::Rtl { remaining } else { 0.0 };
    }
    match text_align {
        TextAlign::Center => remaining / 2.0,
        TextAlign::Right => remaining,
        _ => 0.0,
    }
}

/// Summarizes the content mix and glyph coverage of a token span.
pub(super) fn summarize_token_span(
    reader: &LayoutReader<'_>,
    tokens: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    summary_blocks: &[InlineSummaryBlock],
    summary_offset: Option<usize>,
    tab_origin: f64,
    inline_struts: &[InlineBoxStrut],
) -> TokenSpanSummary {
    match tokens {
        TokenSpan::Wide(tokens) => summarize_token_span_with(
            reader,
            tokens,
            metrics,
            summary_blocks,
            summary_offset,
            tab_origin,
            inline_struts,
        ),
        TokenSpan::Compact { source, start, len } => summarize_token_span_with(
            reader,
            CompactTokens::new(source, start, len),
            metrics,
            summary_blocks,
            summary_offset,
            tab_origin,
            inline_struts,
        ),
    }
}

/// Plain text is justifiable, and justifiable text has contiguous glyphs.
/// Track this implication chain with one monotonic state.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TextClass {
    Plain,
    Justifiable,
    Contiguous,
    Gapped,
}

fn summarize_token_span_with<S: TokenRead>(
    reader: &LayoutReader<'_>,
    tokens: S,
    metrics: MetricsView<'_>,
    summary_blocks: &[InlineSummaryBlock],
    summary_offset: Option<usize>,
    tab_origin: f64,
    inline_struts: &[InlineBoxStrut],
) -> TokenSpanSummary {
    profile_scope!("lines.summarize");
    let mut glyph_start = u32::MAX;
    let mut glyph_end = 0;
    let mut expected_glyph = None;
    let mut text_class = TextClass::Plain;
    let mut has_text = false;
    let mut has_images = false;
    let mut has_baseline_relative_image = false;
    let mut preserves_newlines = false;
    // A uniform reader's first level describes the entire span, including
    // nonzero RTL levels. Later level merges compile away for that reader.
    let mut bidi = InlineBidiSummary::from_level(if tokens.len() != 0 {
        tokens.metadata_at(0).bidi_level()
    } else {
        0
    });
    let mut width = 0.0;
    let mut max_ascent = f64::NEG_INFINITY;
    let mut max_descent = f64::NEG_INFINITY;
    let mut max_line_height = 0.0f64;

    let mut token_index = 0usize;
    let mut scalar_until = 0;
    let mut metrics_cursor = metrics.cursor(tokens.len());
    while token_index < tokens.len() {
        // A block's plain-text proof also covers a clipped line edge. Failed
        // proofs are retried only at the next block, rather than per token.
        if token_index >= scalar_until {
            if let Some(global_index) = summary_offset.map(|offset| offset + token_index)
                && let Some(block) = summary_blocks.get(global_index / INLINE_SUMMARY_BLOCK_TOKENS)
            {
                let within = global_index % INLINE_SUMMARY_BLOCK_TOKENS;
                let remaining =
                    (INLINE_SUMMARY_BLOCK_TOKENS - within).min(tokens.len() - token_index);
                if block.plain_text {
                    if within == 0 && remaining == INLINE_SUMMARY_BLOCK_TOKENS {
                        glyph_start = glyph_start.min(block.glyph_start);
                        let block_end = block.glyph_start + INLINE_SUMMARY_BLOCK_TOKENS as u32;
                        glyph_end = glyph_end.max(block_end);
                        has_text = true;
                        let contiguous =
                            expected_glyph.is_none_or(|expected| expected == block.glyph_start);
                        if !contiguous {
                            text_class = TextClass::Gapped;
                        }
                        expected_glyph = Some(block_end);
                        preserves_newlines |= block.preserves_newlines;
                        if S::MAY_HAVE_MIXED_BIDI_LEVELS {
                            bidi.merge(block.bidi);
                        }
                        width += block.width;
                        max_ascent = max_ascent.max(block.max_ascent);
                        max_descent = max_descent.max(block.max_descent);
                        token_index += INLINE_SUMMARY_BLOCK_TOKENS;
                        continue;
                    }
                    if let Some((metrics_end, metrics)) =
                        metrics_cursor.shared_range(token_index)
                    {
                        let count = remaining.min(metrics_end - token_index);
                        let first = block.glyph_start + within as u32;
                        let end = first + count as u32;
                        glyph_start = glyph_start.min(first);
                        glyph_end = glyph_end.max(end);
                        has_text = true;
                        let contiguous = expected_glyph.is_none_or(|expected| expected == first);
                        if !contiguous {
                            text_class = TextClass::Gapped;
                        }
                        expected_glyph = Some(end);
                        // Only this metrics range contributes; a clipped edge
                        // must not inherit extents or whitespace from its
                        // neighbouring tokens in the same summary block.
                        preserves_newlines |= metrics.white_space.preserves_newlines();
                        if S::MAY_HAVE_MIXED_BIDI_LEVELS {
                            if block.bidi.uniform_level().is_some() {
                                bidi.merge(block.bidi);
                            } else {
                                // Mixed clipped spans need individual levels,
                                // but their glyph and metrics proofs still
                                // avoid all other per-token decoding.
                                for index in token_index..token_index + count {
                                    bidi.merge(InlineBidiSummary::from_level(
                                        tokens.metadata_at(index).bidi_level(),
                                    ));
                                }
                            }
                        }
                        let ascent = f64::from(metrics.ascent);
                        let descent = f64::from(metrics.descent);
                        let half_leading =
                            (f64::from(metrics.line_height) - (ascent + descent)) / 2.0;
                        max_ascent = max_ascent.max(ascent + half_leading);
                        max_descent = max_descent.max(descent + half_leading);
                        // Preserve the scalar path's addition order. Only
                        // width-column loads occur inside this reduction.
                        width = tokens.add_stored_widths(token_index..token_index + count, width);
                        token_index += count;
                        continue;
                    }
                }
                scalar_until = token_index + remaining;
            } else {
                scalar_until = tokens.len();
            }
        }

        let token = tokens.metadata_at(token_index);
        if S::MAY_HAVE_MIXED_BIDI_LEVELS {
            bidi.merge(InlineBidiSummary::from_level(token.bidi_level()));
        }
        let kind = token.kind();
        let metrics = metrics_cursor.at(token_index, token);
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let token_line_height = f64::from(metrics.line_height);
        preserves_newlines |= metrics.white_space.preserves_newlines();

        match kind {
            InlineTokenKind::Glyph { glyph_idx } => {
                glyph_start = glyph_start.min(glyph_idx);
                glyph_end = glyph_end.max(glyph_idx + 1);
                has_text = true;
                let contiguous = expected_glyph.is_none_or(|expected| expected == glyph_idx);
                if !contiguous {
                    text_class = TextClass::Gapped;
                } else if !metrics.visible
                    || !matches!(metrics.vertical_align, InlineVerticalAlign::Baseline)
                {
                    text_class = text_class.max(TextClass::Contiguous);
                } else if token.is_tab() || metrics.placement_required {
                    text_class = text_class.max(TextClass::Justifiable);
                }
                expected_glyph = Some(glyph_idx + 1);
                if metrics.contributes_line_metrics {
                    let half_leading = (token_line_height - (ascent + descent)) / 2.0;
                    max_ascent = max_ascent.max(ascent + half_leading);
                    max_descent = max_descent.max(descent + half_leading);
                }
            }
            InlineTokenKind::Ellipsis { .. } => {
                has_text = true;
                text_class = text_class.max(TextClass::Contiguous);
                let half_leading = (token_line_height - (ascent + descent)) / 2.0;
                max_ascent = max_ascent.max(ascent + half_leading);
                max_descent = max_descent.max(descent + half_leading);
            }
            InlineTokenKind::Discretionary { .. } | InlineTokenKind::Opportunity => {}
            InlineTokenKind::Image { .. }
            | InlineTokenKind::AtomicBox { .. }
            | InlineTokenKind::InlineBoundary { .. } => {
                let boundary = matches!(kind, InlineTokenKind::InlineBoundary { .. });
                has_images |= !boundary;
                has_baseline_relative_image |= matches!(kind, InlineTokenKind::Image { .. })
                    && !matches!(
                        metrics.vertical_align,
                        InlineVerticalAlign::Top | InlineVerticalAlign::Bottom
                    );
                text_class = text_class.max(if boundary { TextClass::Justifiable } else { TextClass::Contiguous });
                // Line-relative top/bottom replaced boxes constrain the line's
                // total height, but do not participate in choosing its
                // baseline. Including a tall top-aligned image's ascent here
                // adds the font strut's descent a second time.
                let line_relative_replaced = matches!(
                    kind,
                    InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }
                ) && matches!(
                    metrics.vertical_align,
                    InlineVerticalAlign::Top | InlineVerticalAlign::Bottom
                );
                if !line_relative_replaced {
                    max_ascent = max_ascent.max(ascent);
                    max_descent = max_descent.max(descent);
                }
                // An image retains `line-height` for percentage
                // `vertical-align`, but that value does not size the replaced
                // box and must not become a minimum height for the line.
                if !matches!(kind, InlineTokenKind::Image { .. }) {
                    max_line_height = max_line_height.max(token_line_height);
                }
            }
            InlineTokenKind::Break { .. } => {
                text_class = text_class.max(TextClass::Contiguous);
                max_ascent = max_ascent.max(ascent);
                max_descent = max_descent.max(descent);
                max_line_height = max_line_height.max(token_line_height);
            }
            InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. } => {
                text_class = text_class.max(TextClass::Contiguous);
            }
        }
        width += if !token.is_tab() {
            token.width(tokens.stored_width_at(token_index))
        } else {
            token.advance_with_metrics(
                tokens.stored_width_at(token_index),
                reader,
                metrics,
                tab_origin + width,
            )
        };
        token_index += 1;
    }

    if !max_ascent.is_finite() {
        max_ascent = 0.0;
    }
    if !max_descent.is_finite() {
        max_descent = 0.0;
    }
    for strut in inline_struts {
        max_ascent = max_ascent.max(strut.ascent);
        max_descent = max_descent.max(strut.descent);
        max_line_height = max_line_height.max(strut.line_height);
    }

    // Each text metric already contains its computed CSS line-height. The
    // caller-provided fallback is deliberately not applied as a minimum.
    let (line_height, baseline) = compute_baseline(max_ascent, max_descent, max_line_height);
    TokenSpanSummary {
        glyph_range: if has_text {
            glyph_start..glyph_end
        } else {
            0..0
        },
        text_coverage: if !has_text {
            TextCoverage::None
        } else if text_class != TextClass::Gapped && expected_glyph.is_some() {
            TextCoverage::WholeRange
        } else {
            TextCoverage::Fragments
        },
        has_images,
        has_baseline_relative_image,
        plain_text: text_class == TextClass::Plain && has_text,
        justifiable_text: text_class <= TextClass::Justifiable && has_text,
        preserves_newlines,
        bidi,
        width,
        line_height,
        baseline,
    }
}

#[cfg(any(test, feature = "testing"))]
pub(in crate::layout::inline) fn measured_tracking_boundary_capacity(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    tokens: TokenSpan<'_>,
    runs: &[InlineTokenMetrics],
) -> f64 {
    let summary = summarize_token_span(
        &engine.reader,
        tokens,
        tokens.metrics_view(runs),
        &[],
        None,
        0.0,
        &[],
    );
    if !summary.plain_text || summary.preserves_newlines {
        return 0.0;
    }
    tracking_boundary_limit(summary.line_height)
        * tracking_boundary_count(engine, summary.glyph_range) as f64
}

fn tracking_boundary_count(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    glyphs: Range<u32>,
) -> usize {
    (glyphs.start..glyphs.end.saturating_sub(1))
        .filter(|&glyph_idx| {
            engine
                .text
                .has_tracking_boundary_after(glyph_idx, glyphs.end)
        })
        .count()
}

#[derive(Default)]
struct JustificationSpacing {
    word_spacing: f64,
    letter_spacing: f64,
    word_adjustment: f64,
    tracking_adjustment: f64,
    punctuation_space_advances: Vec<(u32, f32)>,
}

/// Computes space and tracking adjustment. Book composition keeps preferred
/// expansion limits; web composition fills lines requiring larger gaps.
fn justification_spacing(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    tokens: TokenSpan<'_>,
    summary: &TokenSpanSummary,
    available_width: f64,
    is_last_line: bool,
    text_align: TextAlign,
) -> JustificationSpacing {
    profile_scope!("lines.justification");
    let should_justify = text_align == TextAlign::Justify
        || (engine.config.force_justify() && text_align == TextAlign::Left);
    if (is_last_line && text_align != TextAlign::Justify)
        || (summary.preserves_newlines && engine.config.book_optimized_text())
        || !should_justify
        || !summary.justifiable_text
    {
        return JustificationSpacing::default();
    }

    // `summary.width` includes authored letter and word spacing, so this
    // slack matches the width that line placement will actually consume.
    let extra_space = available_width - summary.width;
    let punctuation_aware = engine.config.book_optimized_text();
    let mut spaces = punctuation_aware.then(Vec::new);
    let mut space_count = 0usize;
    let mut expansion_limit = 0.0;
    let mut shrink_capacity = 0.0;
    let widths = tokens.widths();
    // Justify only the suffix after the last preserved tab. Expansion before
    // that stop would otherwise change tab advances and move the tab columns.
    let last_tab = tokens.metadata_iter().rposition(|token| token.is_tab());
    let mut fixed_advances = Vec::new();
    for (index, token) in tokens.metadata_iter().enumerate() {
        let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
            continue;
        };
        if !html_text::is_word_separator(engine.text.character(glyph_idx)) {
            continue;
        }
        if last_tab.is_some_and(|last| index < last) {
            fixed_advances.push((glyph_idx, token.width(widths.at(index)) as f32));
            continue;
        }
        if summary.glyph_range.contains(&glyph_idx) {
            let glue_width = if punctuation_aware {
                token.width(widths.at(index))
            } else {
                let metric = engine
                    .text
                    .glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default());
                engine
                    .text
                    .text_advance(glyph_idx as usize, metric.advance()) as f64
            };
            let (stretch, shrink) = super::justification::glue_capacities(
                glue_width,
                token.space_glue_class(),
                punctuation_aware,
            );
            space_count += 1;
            expansion_limit += stretch;
            shrink_capacity += shrink;
            if let Some(spaces) = &mut spaces {
                spaces.push((
                    glyph_idx,
                    token.width(widths.at(index)),
                    token.space_glue_class(),
                    stretch,
                    shrink,
                ));
            }
        }
    }
    // Unspaced East Asian letters justify between shaped character units.
    // The micro-tracking limits below are a book-composition fallback, not
    // the web's full inter-character expansion. Punctuation retains its
    // existing policy until language-specific opportunities are available.
    if !punctuation_aware
        && space_count == 0
        && last_tab.is_none()
        && extra_space > 0.0
        && summary.glyph_range.clone().all(|index| {
            let character = engine.text.character(index);
            (html_style_model::is_cjk_character(character) && character.is_alphabetic())
                || html_text::suppresses_glyph(character)
        })
    {
        let count = tracking_boundary_count(engine, summary.glyph_range.clone());
        if count > 0 {
            return JustificationSpacing {
                letter_spacing: extra_space / count as f64,
                tracking_adjustment: extra_space,
                ..Default::default()
            };
        }
    }
    // Browser-compatible justification must fill the line even when the
    // required gap exceeds book composition's preferred glue/tracking limits.
    if !punctuation_aware
        && space_count > 0
        && ((last_tab.is_some() && extra_space >= 0.0)
            || extra_space > expansion_limit + tracking_width_limit(summary.width))
    {
        return JustificationSpacing {
            word_spacing: extra_space / space_count as f64,
            word_adjustment: extra_space,
            punctuation_space_advances: fixed_advances,
            ..Default::default()
        };
    }
    // Use the same per-space glue capacities as Knuth--Plass. This also
    // protects greedy fallback lines with unbreakable content from producing
    // zero or negative space advances.
    let shrink_limit = -shrink_capacity;
    let word_adjustment = extra_space.clamp(shrink_limit, expansion_limit);
    let word_spacing = if space_count == 0 {
        0.0
    } else {
        word_adjustment / space_count as f64
    };
    let tracking_capacity = if last_tab.is_some() {
        0.0
    } else {
        tracking_width_limit(summary.width)
    };
    let tracking_adjustment =
        (extra_space - word_adjustment).clamp(-tracking_capacity, tracking_capacity);
    let tracking_count = if tracking_adjustment == 0.0 {
        0
    } else {
        tracking_boundary_count(engine, summary.glyph_range.clone())
    };
    let per_boundary_limit = tracking_boundary_limit(summary.line_height);
    let letter_spacing = if tracking_count > 0 {
        (tracking_adjustment / tracking_count as f64).clamp(-per_boundary_limit, per_boundary_limit)
    } else {
        0.0
    };
    let tracking_adjustment = letter_spacing * tracking_count as f64;
    let mut spacing = JustificationSpacing {
        word_spacing,
        letter_spacing,
        word_adjustment,
        tracking_adjustment,
        punctuation_space_advances: fixed_advances,
    };

    if punctuation_aware && space_count > 1 && word_adjustment != 0.0 {
        let expanding = word_adjustment > 0.0;
        let capacity = if expanding {
            expansion_limit
        } else {
            shrink_capacity
        };
        let spaces = spaces.as_deref().unwrap_or_default();
        let first_capacity = spaces
            .first()
            .map(|space| if expanding { space.3 } else { space.4 })
            .unwrap_or_default();
        let requires_exact_advances = spaces.iter().any(|space| {
            space.2 != super::justification::SpaceGlueClass::Ordinary
                || ((if expanding { space.3 } else { space.4 }) - first_capacity).abs() > 1.0e-9
        });
        if requires_exact_advances && capacity > 0.0 {
            let ratio = word_adjustment.abs() / capacity;
            let mut advances = Vec::with_capacity(space_count);
            for &(glyph_idx, width, _, stretch, shrink) in spaces {
                let glue_adjustment = ratio * if expanding { stretch } else { -shrink };
                let tracking = if engine
                    .text
                    .has_tracking_boundary_after(glyph_idx, summary.glyph_range.end)
                {
                    letter_spacing
                } else {
                    0.0
                };
                let advance = width + glue_adjustment + tracking;
                if !advance.is_finite() || advance < 0.0 {
                    advances.clear();
                    break;
                }
                advances.push((glyph_idx, advance as f32));
            }
            if advances.len() == space_count {
                spacing.word_spacing = 0.0;
                spacing.punctuation_space_advances = advances;
            }
        }
    }
    spacing
}

/// Chooses the initial line metrics before vertical-align adjustments are applied.
fn base_line_metrics(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    tokens: TokenSpan<'_>,
    summary: &TokenSpanSummary,
    line_width_limit: f64,
    is_last_line: bool,
    text_align: TextAlign,
) -> (f64, f64, JustificationSpacing) {
    let (line_height, baseline) = (summary.line_height, summary.baseline);
    if summary.text_coverage != TextCoverage::None && !summary.has_images {
        let spacing = justification_spacing(
            engine,
            tokens,
            summary,
            line_width_limit,
            is_last_line,
            text_align,
        );
        (line_height, baseline, spacing)
    } else {
        (line_height, baseline, JustificationSpacing::default())
    }
}
