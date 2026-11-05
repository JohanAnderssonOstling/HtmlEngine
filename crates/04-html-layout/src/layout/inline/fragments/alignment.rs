//! Vertical alignment and final token placement.

use super::*;
use crate::layout::LayoutEngine;
use std::ptr;

/// Applies a token's own alignment and then the movement of each enclosing
/// inline box. The property value itself is never inherited: an aligned parent
/// moves its complete fragment, including descendants, as a unit.
fn hierarchical_vertical_align_offset(
    engine: &LayoutEngine<'_, '_>,
    token: &TokenMetadata,
    metrics: &InlineTokenMetrics,
    line_height: f64,
    baseline: f64,
) -> f64 {
    let own_is_group_alignment = matches!(
        token.kind(),
        InlineTokenKind::Glyph { .. }
            | InlineTokenKind::Ellipsis { .. }
            | InlineTokenKind::Discretionary { .. }
    ) && matches!(
        metrics.vertical_align,
        InlineVerticalAlign::Top | InlineVerticalAlign::Bottom
    );
    let mut offset = if own_is_group_alignment
        || matches!(metrics.vertical_align, InlineVerticalAlign::Baseline)
    {
        0.0
    } else {
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let half_leading = match token.kind() {
            InlineTokenKind::Glyph { .. }
            | InlineTokenKind::Ellipsis { .. }
            | InlineTokenKind::Discretionary { .. } => {
                (f64::from(metrics.line_height) - ascent - descent) / 2.0
            }
            _ => 0.0,
        };
        let (parent_ascent, parent_descent) = if metrics.owner_box_idx == u32::MAX {
            (ascent, descent)
        } else {
            parent_font_content_extents(engine, metrics.owner_box_idx as usize)
        };
        vertical_align_metrics_offset(
            metrics.vertical_align.as_style_value(),
            ascent,
            descent,
            f64::from(metrics.line_height),
            metrics.font_size as f64,
            half_leading,
            line_height,
            baseline,
            parent_ascent,
            parent_descent,
        )
    };
    if metrics.owner_box_idx == u32::MAX {
        return offset;
    }
    // Glyph metrics describe the conceptual anonymous inline surrounding text,
    // whose vertical-align is baseline. The retained owner is its element
    // parent, so include that owner when accumulating enclosing-box movement.
    let start = engine.reader.get_parent(metrics.owner_box_idx as usize);
    for box_idx in inline_ancestors(&engine.reader, start) {
        let style = engine.reader.style(box_idx);
        let value = style.vertical_align();
        // `top` and `bottom` align the complete aligned subtree, not this
        // ancestor's font strut independently. Their shared group correction
        // is resolved after the final line box is known.
        if !value.is_initial()
            && !matches!(value, VerticalAlignValue::Top | VerticalAlignValue::Bottom)
        {
            let own_line_height = style.resolved_line_height();
            let ascent = own_line_height * 0.8;
            let descent = own_line_height - ascent;
            let (parent_ascent, parent_descent) = parent_font_content_extents(engine, box_idx);
            offset += vertical_align_metrics_offset(
                value,
                ascent,
                descent,
                own_line_height,
                style.font_size() as f64,
                0.0,
                line_height,
                baseline,
                parent_ascent,
                parent_descent,
            );
        }
    }
    offset
}

#[derive(Clone, Copy)]
struct LineRelativeGroupMetrics {
    box_idx: usize,
    alignment: VerticalAlignValue,
    top: f64,
    bottom: f64,
}

impl LineRelativeGroupMetrics {
    fn merge_into(self, groups: &mut Vec<Self>) {
        if let Some(group) = groups
            .iter_mut()
            .find(|group| group.box_idx == self.box_idx)
        {
            group.top = group.top.min(self.top);
            group.bottom = group.bottom.max(self.bottom);
        } else {
            groups.push(self);
        }
    }
}

fn line_relative_alignment_for_box(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
) -> Option<(usize, VerticalAlignValue)> {
    for box_idx in inline_ancestors(&engine.reader, Some(box_idx)) {
        let alignment = engine.reader.style(box_idx).vertical_align();
        if matches!(
            alignment,
            VerticalAlignValue::Top | VerticalAlignValue::Bottom
        ) {
            return Some((box_idx, alignment));
        }
    }
    None
}

fn line_relative_alignment_for_token(
    engine: &LayoutEngine<'_, '_>,
    token: &TokenMetadata,
    metrics: &InlineTokenMetrics,
) -> Option<(usize, VerticalAlignValue)> {
    if metrics.owner_box_idx == u32::MAX {
        return None;
    }
    if matches!(
        token.kind(),
        InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }
    ) && matches!(
        metrics.vertical_align,
        InlineVerticalAlign::Top | InlineVerticalAlign::Bottom
    ) {
        return Some((
            metrics.owner_box_idx as usize,
            metrics.vertical_align.as_style_value(),
        ));
    }
    line_relative_alignment_for_ancestor(engine, &token, metrics)
}

fn line_relative_alignment_for_ancestor(
    engine: &LayoutEngine<'_, '_>,
    token: &TokenMetadata,
    metrics: &InlineTokenMetrics,
) -> Option<(usize, VerticalAlignValue)> {
    if metrics.owner_box_idx == u32::MAX {
        return None;
    }
    let start = if matches!(
        token.kind(),
        InlineTokenKind::Glyph { .. }
            | InlineTokenKind::Ellipsis { .. }
            | InlineTokenKind::Discretionary { .. }
    ) {
        metrics.owner_box_idx as usize
    } else {
        engine.reader.get_parent(metrics.owner_box_idx as usize)?
    };
    line_relative_alignment_for_box(engine, start)
}

/// A shared metrics range can rule out group alignment for every token kind.
/// Text starts at its owner; other content starts at the owner's parent.
fn metrics_may_have_line_relative_alignment(
    engine: &LayoutEngine<'_, '_>,
    metrics: &InlineTokenMetrics,
) -> bool {
    if metrics.owner_box_idx == u32::MAX {
        return false;
    }
    // Replaced tokens can align themselves independently of their ancestors.
    if matches!(
        metrics.vertical_align,
        InlineVerticalAlign::Top | InlineVerticalAlign::Bottom
    ) {
        return true;
    }
    let owner = metrics.owner_box_idx as usize;
    let start = if engine.reader.is_inline_box(owner) {
        Some(owner)
    } else {
        engine.reader.get_parent(owner)
    };
    start.is_some_and(|box_idx| line_relative_alignment_for_box(engine, box_idx).is_some())
}

fn line_relative_groups(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    inline_struts: &[InlineBoxStrut],
    line_height: f64,
    baseline: f64,
) -> Vec<LineRelativeGroupMetrics> {
    let mut groups = Vec::<LineRelativeGroupMetrics>::new();
    for (tokens, metrics) in metrics_view.chunks(span) {
        // Single-token ranges retain their direct token check. Shared ranges
        // without a possible group need no header reads or per-token walks.
        if tokens.len() > 1 && !metrics_may_have_line_relative_alignment(engine, metrics) {
            continue;
        }
        for token in tokens.metadata_iter() {
            if !token_contributes_line_metrics(&token, metrics) {
                continue;
            }
            let Some((box_idx, alignment)) =
                line_relative_alignment_for_token(engine, &token, metrics)
            else {
                continue;
            };
            let ascent = metrics.ascent as f64;
            let descent = metrics.descent as f64;
            let half_leading = if matches!(
                token.kind(),
                InlineTokenKind::Glyph { .. }
                    | InlineTokenKind::Ellipsis { .. }
                    | InlineTokenKind::Discretionary { .. }
            ) {
                (f64::from(metrics.line_height) - ascent - descent) * 0.5
            } else {
                0.0
            };
            let offset =
                hierarchical_vertical_align_offset(engine, &token, metrics, line_height, baseline);
            LineRelativeGroupMetrics {
                box_idx,
                alignment,
                top: baseline - ascent - half_leading - offset,
                bottom: baseline + descent + half_leading - offset,
            }
            .merge_into(&mut groups);
        }
    }
    for strut in inline_struts {
        let Some((box_idx, alignment)) = line_relative_alignment_for_box(engine, strut.box_idx)
        else {
            continue;
        };
        let offset = inline_strut_vertical_align_offset(engine, *strut, line_height, baseline);
        LineRelativeGroupMetrics {
            box_idx,
            alignment,
            top: baseline - strut.ascent - offset,
            bottom: baseline + strut.descent - offset,
        }
        .merge_into(&mut groups);
    }
    groups
}

/// Recomputes the line box after per-token vertical alignment offsets are applied.
pub(super) fn adjusted_line_metrics(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    inline_struts: &[InlineBoxStrut],
    container_box_idx: usize,
    line_height: f64,
    baseline: f64,
) -> (f64, f64) {
    let line_relative_groups = line_relative_groups(
        engine,
        span,
        metrics_view,
        inline_struts,
        line_height,
        baseline,
    );
    if !line_relative_groups.is_empty() {
        let container_strut = strut_for_inline_box(engine, container_box_idx);
        let mut adjusted_ascent = container_strut.ascent;
        let mut adjusted_descent = container_strut.descent;
        for (token, metrics) in metrics_view.iter(span) {
            if !token_contributes_line_metrics(&token, metrics)
                || line_relative_alignment_for_token(engine, &token, metrics).is_some()
            {
                continue;
            }
            let ascent = metrics.ascent as f64;
            let descent = metrics.descent as f64;
            let offset =
                hierarchical_vertical_align_offset(engine, &token, metrics, line_height, baseline);
            let half_leading = match token.kind() {
                InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } => {
                    (f64::from(metrics.line_height) - ascent - descent) * 0.5
                }
                _ => 0.0,
            };
            adjusted_ascent = adjusted_ascent.max(ascent + half_leading + offset);
            adjusted_descent = adjusted_descent.max(descent + half_leading - offset);
        }
        for strut in inline_struts {
            if line_relative_alignment_for_box(engine, strut.box_idx).is_some() {
                continue;
            }
            let offset = inline_strut_vertical_align_offset(engine, *strut, line_height, baseline);
            adjusted_ascent = adjusted_ascent.max(strut.ascent + offset);
            adjusted_descent = adjusted_descent.max(strut.descent - offset);
        }
        let (mut resolved_height, mut resolved_baseline) =
            compute_baseline(adjusted_ascent, adjusted_descent, 0.0);
        let tallest_top = line_relative_groups
            .iter()
            .filter(|group| group.alignment == VerticalAlignValue::Top)
            .map(|group| group.bottom - group.top)
            .fold(0.0, f64::max);
        resolved_height = resolved_height.max(tallest_top);
        let tallest_bottom = line_relative_groups
            .iter()
            .filter(|group| group.alignment == VerticalAlignValue::Bottom)
            .map(|group| group.bottom - group.top)
            .fold(0.0, f64::max);
        if tallest_bottom > resolved_height {
            resolved_baseline += tallest_bottom - resolved_height;
            resolved_height = tallest_bottom;
        }
        return (resolved_height, resolved_baseline);
    }

    // Baseline images and genuinely aligned inline content need the
    // formatting root's ascent and descent as independent constraints. Plain
    // baseline text already carries that strut in its token metrics.
    let needs_container_strut = metrics_view.any(span, |token, metrics| {
        metrics.contributes_line_metrics
            && (matches!(token.kind(), InlineTokenKind::Image { .. })
                || (matches!(token.kind(), InlineTokenKind::AtomicBox { .. })
                    && metrics.descent == 0.0)
                || (!matches!(metrics.vertical_align, InlineVerticalAlign::Baseline)
                    && metrics.owner_box_idx != u32::MAX
                    && engine.reader.is_inline_box(metrics.owner_box_idx as usize)))
    });
    let container_strut = strut_for_inline_box(engine, container_box_idx);
    let mut adjusted_ascent = if needs_container_strut {
        container_strut.ascent
    } else {
        f64::NEG_INFINITY
    };
    let mut adjusted_descent = if needs_container_strut {
        container_strut.descent
    } else {
        f64::NEG_INFINITY
    };
    for (token, metrics) in metrics_view.iter(span) {
        if !token_contributes_line_metrics(&token, metrics) {
            continue;
        }
        // Atomic boxes contribute their actual margin-box baseline. The
        // containing font strut above supplies the surrounding text metrics.
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let offset =
            hierarchical_vertical_align_offset(engine, &token, metrics, line_height, baseline);
        let half_leading = match token.kind() {
            InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } => {
                (f64::from(metrics.line_height) - (ascent + descent)) / 2.0
            }
            _ => 0.0,
        };
        adjusted_ascent = adjusted_ascent.max(ascent + half_leading + offset);
        adjusted_descent = adjusted_descent.max(descent + half_leading - offset);
    }
    for strut in inline_struts {
        let offset = inline_strut_vertical_align_offset(engine, *strut, line_height, baseline);
        adjusted_ascent = adjusted_ascent.max(strut.ascent + offset);
        adjusted_descent = adjusted_descent.max(strut.descent - offset);
    }
    if !adjusted_ascent.is_finite() {
        adjusted_ascent = 0.0;
    }
    if !adjusted_descent.is_finite() {
        adjusted_descent = 0.0;
    }
    let (resolved_height, resolved_baseline) =
        compute_baseline(adjusted_ascent, adjusted_descent, line_height);
    if resolved_height.abs() < 0.001 {
        (0.0, resolved_baseline)
    } else {
        (resolved_height, resolved_baseline)
    }
}

fn inline_strut_vertical_align_offset(
    engine: &LayoutEngine<'_, '_>,
    strut: InlineBoxStrut,
    line_height: f64,
    baseline: f64,
) -> f64 {
    let mut offset = 0.0;
    let mut own = true;
    for box_idx in inline_ancestors(&engine.reader, Some(strut.box_idx)) {
        let style = engine.reader.style(box_idx);
        let value = style.vertical_align();
        if !value.is_initial()
            && !matches!(value, VerticalAlignValue::Top | VerticalAlignValue::Bottom)
        {
            let metrics = if own {
                strut
            } else {
                strut_for_inline_box(engine, box_idx)
            };
            let (parent_ascent, parent_descent) = parent_font_content_extents(engine, box_idx);
            offset += vertical_align_metrics_offset(
                value,
                metrics.ascent,
                metrics.descent,
                metrics.line_height,
                metrics.font_size,
                0.0,
                line_height,
                baseline,
                parent_ascent,
                parent_descent,
            );
        }
        own = false;
    }
    offset
}

/// Logical text without vertical or relative offsets can reconstruct its
/// positions from canonical widths, including context-dependent tab advances.
pub(super) fn can_stream_text_placements(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
) -> Option<bool> {
    let mut publish = false;
    for (_, metrics) in metrics.chunks(span) {
        if !matches!(metrics.vertical_align, InlineVerticalAlign::Baseline)
            || (metrics.placement_required
                && metrics.owner_box_idx != u32::MAX
                && has_aligned_inline_ancestor(engine, metrics.owner_box_idx as usize))
        {
            return None;
        }
        if metrics.owner_box_idx != u32::MAX {
            let style = engine.reader.style(metrics.owner_box_idx as usize);
            publish |= style.word_spacing() != 0.0 || style.word_spacing_factor() != 1.0;
        }
    }
    for token in span.metadata_iter() {
        if !matches!(
            token.kind(),
            InlineTokenKind::Glyph { .. }
                | InlineTokenKind::Discretionary { .. }
                | InlineTokenKind::Opportunity
        ) {
            return None;
        }
        publish |= token.is_tab();
    }
    Some(publish)
}

/// Builds final per-token placements for a measured line box.
pub(super) fn build_token_placements(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics_view: MetricsView<'_>,
    tab_origin: f64,
    line_height: f64,
    baseline: f64,
    containing_width: f64,
) -> Vec<TokenPlacement> {
    // Measure every top/bottom-aligned subtree with its ordinary descendant
    // offsets first. One shared correction then moves the subtree as a unit.
    let mut groups = Vec::<LineRelativeGroupMetrics>::new();
    for (tokens, metrics) in metrics_view.chunks(span) {
        // Single-token ranges retain their direct token check. Shared ranges
        // without a possible group need no header reads or per-token walks.
        if tokens.len() > 1 && !metrics_may_have_line_relative_alignment(engine, metrics) {
            continue;
        }
        for token in tokens.metadata_iter() {
            let Some((box_idx, alignment)) =
                line_relative_alignment_for_ancestor(engine, &token, metrics)
            else {
                continue;
            };
            if !token_contributes_line_metrics(&token, metrics) {
                continue;
            }
            let ascent = metrics.ascent as f64;
            let descent = metrics.descent as f64;
            let half_leading = if matches!(
                token.kind(),
                InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. }
            ) {
                (f64::from(metrics.line_height) - ascent - descent) * 0.5
            } else {
                0.0
            };
            let offset =
                hierarchical_vertical_align_offset(engine, &token, metrics, line_height, baseline);
            LineRelativeGroupMetrics {
                box_idx,
                alignment,
                top: baseline - ascent - half_leading - offset,
                bottom: baseline + descent + half_leading - offset,
            }
            .merge_into(&mut groups);
        }
    }

    let mut curr_x = 0.0;
    let mut placements = Vec::with_capacity(span.len());
    let widths = span.widths();
    // Text offsets depend on the metrics record and this line, not the glyph
    // payload or advance. Keep the last text result across other token kinds.
    let mut cached_text_offsets: Option<(&InlineTokenMetrics, Vec2, f32)> = None;
    placements.extend(
        metrics_view
            .iter(span)
            .enumerate()
            .map(|(index, (token, metrics))| {
                let advance = token.advance_with_metrics(
                    widths.at(index),
                    &engine.reader,
                    metrics,
                    tab_origin + curr_x,
                );
                let is_text = matches!(
                    token.kind(),
                    InlineTokenKind::Glyph { .. }
                        | InlineTokenKind::Ellipsis { .. }
                        | InlineTokenKind::Discretionary { .. }
                );
                let (relative_offset, y_offset) = match cached_text_offsets {
                    Some((previous, relative_offset, y_offset))
                        if is_text && ptr::eq(previous, metrics) =>
                    {
                        (relative_offset, y_offset)
                    }
                    _ => {
                        let owner = metrics.owner_box_idx;
                        let relative_offset = if owner == u32::MAX {
                            Vec2::ZERO
                        } else {
                            let start = match token.kind() {
                                InlineTokenKind::Image { .. }
                                | InlineTokenKind::AtomicBox { .. } => {
                                    engine.reader.get_parent(owner as usize)
                                }
                                _ => Some(owner as usize),
                            };
                            start.map_or(Vec2::ZERO, |box_idx| {
                                inline_relative_position_offset(engine, box_idx, containing_width)
                            })
                        };
                        let group_offset =
                            line_relative_alignment_for_ancestor(engine, &token, metrics)
                                .and_then(|(box_idx, _)| {
                                    groups.iter().find(|group| group.box_idx == box_idx)
                                })
                                .map_or(0.0, |group| match group.alignment {
                                    VerticalAlignValue::Top => group.top,
                                    VerticalAlignValue::Bottom => group.bottom - line_height,
                                    _ => 0.0,
                                });
                        let y_offset = (hierarchical_vertical_align_offset(
                            engine,
                            &token,
                            metrics,
                            line_height,
                            baseline,
                        ) + group_offset
                            - relative_offset.y) as f32;
                        if is_text {
                            cached_text_offsets = Some((metrics, relative_offset, y_offset));
                        }
                        (relative_offset, y_offset)
                    }
                };
                let placement = TokenPlacement {
                    x: curr_x,
                    advance,
                    y_offset,
                    relative_offset,
                };
                curr_x += advance;
                placement
            }),
    );
    placements
}
