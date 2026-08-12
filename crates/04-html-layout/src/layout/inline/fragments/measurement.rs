//! Token-span measurement, optical margins, and justification.

use super::*;

pub(super) fn measure_line(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    tokens: &[InlineToken],
    runs: &[InlineTokenMetrics],
    mut summary: TokenSpanSummary,
    line: &BrokenLine,
    empty_break_metrics: Option<InlineTokenMetrics>,
    inline_struts: &[InlineBoxStrut],
    container_box_idx: usize,
    containing_width: f64,
) -> LineLayout {
    let timing_started = engine.start_timing();
    let hyphen = tokens.last().and_then(|token| match token.kind() {
        InlineTokenKind::Discretionary { glyph } => Some((glyph, token.discretionary_width())),
        _ => None,
    });
    if let Some((_, width)) = hyphen {
        summary.width += width;
    }
    if let Some(metrics) = empty_break_metrics {
        let ascent = f64::from(metrics.ascent);
        let descent = f64::from(metrics.descent);
        let half_leading = (metrics.line_height - (ascent + descent)) / 2.0;
        summary.line_height = metrics.line_height;
        summary.baseline = ascent + half_leading;
    }
    let (optical_start, optical_end) = optical_margin_protrusion(engine, &summary, line.align);
    let justification_width = if line.align == TextAlign::Justify {
        line.width_limit + optical_start + optical_end
    } else {
        line.width_limit
    };
    let (line_height, baseline, spacing) = base_line_metrics(
        engine,
        tokens,
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
        punctuation_space_advances,
    } = spacing;
    let (line_height, baseline) = if summary.plain_text {
        (line_height, baseline)
    } else {
        adjusted_line_metrics(
            engine,
            tokens,
            runs,
            inline_struts,
            container_box_idx,
            line_height,
            baseline,
        )
    };
    let x_offset = line_x_offset(line.width_limit, summary.width, line.align);
    let optical_offset_x = match line.align {
        TextAlign::Left | TextAlign::Justify => -optical_start,
        TextAlign::Right => optical_end,
        TextAlign::Center => 0.0,
    };
    let placements = (!summary.plain_text).then(|| {
        build_token_placements(
            engine,
            tokens,
            runs,
            line.tab_origin,
            line_height,
            baseline,
            containing_width,
        )
    });

    let adjusted_width = summary.width + word_adjustment + tracking_adjustment;
    let hyphen = hyphen.map(|(glyph, width)| (glyph, (adjusted_width - width).max(0.0)));
    let layout = LineLayout {
        glyph_range: summary.glyph_range,
        width: summary.width,
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
        let metric = engine
            .text
            .glyph_metric(engine.text.glyph_at(index as usize).unwrap_or_default());
        (!metric.ch().is_whitespace() && metric.ch() != '\u{00ad}').then_some((index, metric))
    };
    let first = summary.glyph_range.clone().find_map(visible);
    let last = summary.glyph_range.clone().rev().find_map(visible);
    let protrusion = |entry: Option<(u32, crate::layout_model::GlyphMetric)>, start: bool| {
        let Some((index, metric)) = entry else {
            return 0.0;
        };
        let fraction = match (start, metric.ch()) {
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
fn line_x_offset(line_width_limit: f64, line_width: f64, text_align: TextAlign) -> f64 {
    let remaining = (line_width_limit - line_width).max(0.0);
    match text_align {
        TextAlign::Center => remaining / 2.0,
        TextAlign::Right => remaining,
        _ => 0.0,
    }
}

/// Summarizes the content mix and glyph coverage of a token span.
pub(super) fn summarize_token_span(
    tokens: &[InlineToken],
    runs: &[InlineTokenMetrics],
    summary_blocks: &[InlineSummaryBlock],
    summary_offset: Option<usize>,
    tab_origin: f64,
    inline_struts: &[InlineBoxStrut],
) -> TokenSpanSummary {
    let mut glyph_start = u32::MAX;
    let mut glyph_end = 0;
    let mut expected_glyph = None;
    let mut has_text = false;
    let mut has_images = false;
    let mut has_baseline_relative_image = false;
    let mut plain_text = true;
    let mut preserves_newlines = false;
    let mut width = 0.0;
    let mut max_ascent = f64::NEG_INFINITY;
    let mut max_descent = f64::NEG_INFINITY;
    let mut max_line_height = 0.0f64;

    let mut token_index = 0usize;
    while token_index < tokens.len() {
        if let Some(global_index) = summary_offset.map(|offset| offset + token_index)
            && global_index % INLINE_SUMMARY_BLOCK_TOKENS == 0
            && tokens.len() - token_index >= INLINE_SUMMARY_BLOCK_TOKENS
            && let Some(block) = summary_blocks.get(global_index / INLINE_SUMMARY_BLOCK_TOKENS)
            && block.plain_text
        {
            glyph_start = glyph_start.min(block.glyph_start);
            glyph_end = glyph_end.max(block.glyph_end);
            has_text = true;
            plain_text &= expected_glyph.is_none_or(|expected| expected == block.glyph_start);
            expected_glyph = Some(block.glyph_end);
            preserves_newlines |= block.preserves_newlines;
            width += block.width;
            max_ascent = max_ascent.max(block.max_ascent);
            max_descent = max_descent.max(block.max_descent);
            token_index += INLINE_SUMMARY_BLOCK_TOKENS;
            continue;
        }

        let token = &tokens[token_index];
        let kind = token.kind();
        let metrics = token.run_metrics(runs);
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let token_line_height = metrics.line_height;
        preserves_newlines |= metrics.white_space.preserves_newlines();

        match kind {
            InlineTokenKind::Glyph { glyph_idx } => {
                glyph_start = glyph_start.min(glyph_idx);
                glyph_end = glyph_end.max(glyph_idx + 1);
                has_text = true;
                plain_text &= expected_glyph.is_none_or(|expected| expected == glyph_idx)
                    && metrics.tab_interval < 0.0
                    && !metrics.placement_required
                    && metrics.visible
                    && matches!(metrics.vertical_align, VerticalAlignValue::Baseline);
                expected_glyph = Some(glyph_idx + 1);
                let half_leading = (token_line_height - (ascent + descent)) / 2.0;
                max_ascent = max_ascent.max(ascent + half_leading);
                max_descent = max_descent.max(descent + half_leading);
            }
            InlineTokenKind::Ellipsis { .. } => {
                has_text = true;
                plain_text = false;
                let half_leading = (token_line_height - (ascent + descent)) / 2.0;
                max_ascent = max_ascent.max(ascent + half_leading);
                max_descent = max_descent.max(descent + half_leading);
            }
            InlineTokenKind::Discretionary { .. } | InlineTokenKind::Opportunity => {}
            InlineTokenKind::Image { .. }
            | InlineTokenKind::AtomicBox { .. }
            | InlineTokenKind::InlineBoundary { .. } => {
                has_images = true;
                has_baseline_relative_image |= matches!(kind, InlineTokenKind::Image { .. })
                    && !matches!(
                        metrics.vertical_align,
                        VerticalAlignValue::Top | VerticalAlignValue::Bottom
                    );
                plain_text = false;
                // Line-relative top/bottom replaced boxes constrain the line's
                // total height, but do not participate in choosing its
                // baseline. Including a tall top-aligned image's ascent here
                // adds the font strut's descent a second time.
                let line_relative_replaced = matches!(
                    kind,
                    InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }
                ) && matches!(
                    metrics.vertical_align,
                    VerticalAlignValue::Top | VerticalAlignValue::Bottom
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
                plain_text = false;
                max_ascent = max_ascent.max(ascent);
                max_descent = max_descent.max(descent);
                max_line_height = max_line_height.max(token_line_height);
            }
            InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. } => {
                plain_text = false;
            }
        }
        width += if metrics.tab_interval < 0.0 {
            token.width()
        } else {
            token.advance_at(runs, tab_origin + width)
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
        has_text,
        has_images,
        has_baseline_relative_image,
        plain_text: plain_text && has_text,
        preserves_newlines,
        width,
        line_height,
        baseline,
    }
}

fn tracking_boundary_count(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    glyphs: Range<u32>,
) -> usize {
    (glyphs.start..glyphs.end.saturating_sub(1))
        .filter(|&glyph_idx| has_tracking_boundary_after(engine, glyph_idx, glyphs.end))
        .count()
}

fn has_tracking_boundary_after(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    glyph_idx: u32,
    line_end: u32,
) -> bool {
    if glyph_idx + 1 >= line_end || !engine.text.is_cluster_boundary(glyph_idx as usize + 1) {
        return false;
    }
    let current = engine
        .text
        .glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default())
        .ch();
    let next = engine
        .text
        .glyph_metric(
            engine
                .text
                .glyph_at(glyph_idx as usize + 1)
                .unwrap_or_default(),
        )
        .ch();
    current != '\u{00ad}' && next != '\u{00ad}'
}

#[derive(Default)]
struct JustificationSpacing {
    word_spacing: f64,
    letter_spacing: f64,
    word_adjustment: f64,
    tracking_adjustment: f64,
    punctuation_space_advances: Vec<(u32, f32)>,
}

/// Computes bounded space adjustment followed by very small tracking at
/// shaped-cluster boundaries. Any remaining positive slack is deliberately
/// left on the right by the emergency underfull-line pass.
fn justification_spacing(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    tokens: &[InlineToken],
    summary: &TokenSpanSummary,
    available_width: f64,
    is_last_line: bool,
    text_align: TextAlign,
) -> JustificationSpacing {
    let should_justify = text_align == TextAlign::Justify
        || (engine.config.force_justify() && text_align == TextAlign::Left);
    if is_last_line || summary.preserves_newlines || !should_justify || !summary.plain_text {
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
    for token in tokens {
        if !token.is_space() {
            continue;
        }
        let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
            continue;
        };
        if summary.glyph_range.contains(&glyph_idx) {
            let glue_width = if punctuation_aware {
                token.width()
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
                    token.width(),
                    token.space_glue_class(),
                    stretch,
                    shrink,
                ));
            }
        }
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
    let tracking_capacity = summary.width.max(0.0) * super::wrapping::MICRO_TRACKING_FRACTION;
    let tracking_adjustment =
        (extra_space - word_adjustment).clamp(-tracking_capacity, tracking_capacity);
    let tracking_count = if tracking_adjustment == 0.0 {
        0
    } else {
        tracking_boundary_count(engine, summary.glyph_range.clone())
    };
    let per_boundary_limit =
        summary.line_height.max(0.0) * super::wrapping::MICRO_TRACKING_FRACTION;
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
        punctuation_space_advances: Vec::new(),
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
                let tracking =
                    if has_tracking_boundary_after(engine, glyph_idx, summary.glyph_range.end) {
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
    tokens: &[InlineToken],
    summary: &TokenSpanSummary,
    line_width_limit: f64,
    is_last_line: bool,
    text_align: TextAlign,
) -> (f64, f64, JustificationSpacing) {
    let (line_height, baseline) = (summary.line_height, summary.baseline);
    if summary.has_text && !summary.has_images {
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
