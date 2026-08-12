//! Vertical alignment and final token placement.

use super::*;

/// Applies a token's own alignment and then the movement of each enclosing
/// inline box. The property value itself is never inherited: an aligned parent
/// moves its complete fragment, including descendants, as a unit.
fn hierarchical_vertical_align_offset(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    token: &InlineToken,
    runs: &[InlineTokenMetrics],
    line_height: f64,
    baseline: f64,
) -> f64 {
    let metrics = token.run_metrics(runs);
    let own_is_group_alignment = matches!(
        token.kind(),
        InlineTokenKind::Glyph { .. }
            | InlineTokenKind::Ellipsis { .. }
            | InlineTokenKind::Discretionary { .. }
    ) && matches!(
        metrics.vertical_align,
        VerticalAlignValue::Top | VerticalAlignValue::Bottom
    );
    let mut offset = if own_is_group_alignment {
        0.0
    } else {
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let half_leading = match token.kind() {
            InlineTokenKind::Glyph { .. }
            | InlineTokenKind::Ellipsis { .. }
            | InlineTokenKind::Discretionary { .. } => {
                (token.line_height(runs) - ascent - descent) / 2.0
            }
            _ => 0.0,
        };
        let (parent_ascent, parent_descent) = if metrics.owner_box_idx == u32::MAX {
            (ascent, descent)
        } else {
            parent_font_content_extents(engine, metrics.owner_box_idx as usize)
        };
        vertical_align_metrics_offset(
            metrics.vertical_align,
            ascent,
            descent,
            token.line_height(runs),
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
    let mut current = engine.reader.get_parent(metrics.owner_box_idx as usize);
    while let Some(box_idx) = current {
        if !matches!(
            engine.reader.box_layout_mode(box_idx),
            Some(LayoutMode::Inline(_))
        ) {
            break;
        }
        let style = engine.reader.style(box_idx);
        let value = style.vertical_align();
        // `top` and `bottom` align the complete aligned subtree, not this
        // ancestor's font strut independently. Their shared group correction
        // is resolved after the final line box is known.
        if !value.is_initial()
            && !matches!(value, VerticalAlignValue::Top | VerticalAlignValue::Bottom)
        {
            let own_line_height = resolved_line_height(style);
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
        current = engine.reader.get_parent(box_idx);
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

fn line_relative_alignment_for_box(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    mut box_idx: usize,
) -> Option<(usize, VerticalAlignValue)> {
    loop {
        if !matches!(
            engine.reader.box_layout_mode(box_idx),
            Some(LayoutMode::Inline(_))
        ) {
            return None;
        }
        let alignment = engine.reader.style(box_idx).vertical_align();
        if matches!(
            alignment,
            VerticalAlignValue::Top | VerticalAlignValue::Bottom
        ) {
            return Some((box_idx, alignment));
        }
        box_idx = engine.reader.get_parent(box_idx)?;
    }
}

fn line_relative_alignment_for_token(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    token: &InlineToken,
    runs: &[InlineTokenMetrics],
) -> Option<(usize, VerticalAlignValue)> {
    let metrics = token.run_metrics(runs);
    if metrics.owner_box_idx == u32::MAX {
        return None;
    }
    if matches!(
        token.kind(),
        InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }
    ) && matches!(
        metrics.vertical_align,
        VerticalAlignValue::Top | VerticalAlignValue::Bottom
    ) {
        return Some((metrics.owner_box_idx as usize, metrics.vertical_align));
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

fn line_relative_groups(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    span: &[InlineToken],
    runs: &[InlineTokenMetrics],
    inline_struts: &[InlineBoxStrut],
    line_height: f64,
    baseline: f64,
) -> Vec<LineRelativeGroupMetrics> {
    let mut groups = Vec::<LineRelativeGroupMetrics>::new();
    let mut add_bounds = |box_idx, alignment, top: f64, bottom: f64| {
        if let Some(group) = groups.iter_mut().find(|group| group.box_idx == box_idx) {
            group.top = group.top.min(top);
            group.bottom = group.bottom.max(bottom);
        } else {
            groups.push(LineRelativeGroupMetrics {
                box_idx,
                alignment,
                top,
                bottom,
            });
        }
    };
    for token in span {
        if matches!(
            token.kind(),
            InlineTokenKind::Opportunity
                | InlineTokenKind::FloatAnchor { .. }
                | InlineTokenKind::AbsoluteAnchor { .. }
        ) {
            continue;
        }
        let Some((box_idx, alignment)) = line_relative_alignment_for_token(engine, token, runs)
        else {
            continue;
        };
        let metrics = token.run_metrics(runs);
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let half_leading = if matches!(
            token.kind(),
            InlineTokenKind::Glyph { .. }
                | InlineTokenKind::Ellipsis { .. }
                | InlineTokenKind::Discretionary { .. }
        ) {
            (metrics.line_height - ascent - descent) * 0.5
        } else {
            0.0
        };
        let offset = hierarchical_vertical_align_offset(engine, token, runs, line_height, baseline);
        add_bounds(
            box_idx,
            alignment,
            baseline - ascent - half_leading - offset,
            baseline + descent + half_leading - offset,
        );
    }
    for strut in inline_struts {
        let Some((box_idx, alignment)) = line_relative_alignment_for_box(engine, strut.box_idx)
        else {
            continue;
        };
        let offset = inline_strut_vertical_align_offset(engine, *strut, line_height, baseline);
        add_bounds(
            box_idx,
            alignment,
            baseline - strut.ascent - offset,
            baseline + strut.descent - offset,
        );
    }
    groups
}

/// Recomputes the line box after per-token vertical alignment offsets are applied.
pub(super) fn adjusted_line_metrics(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    span: &[InlineToken],
    runs: &[InlineTokenMetrics],
    inline_struts: &[InlineBoxStrut],
    container_box_idx: usize,
    line_height: f64,
    baseline: f64,
) -> (f64, f64) {
    let line_relative_groups =
        line_relative_groups(engine, span, runs, inline_struts, line_height, baseline);
    if !line_relative_groups.is_empty() {
        let container_strut = strut_for_inline_box(engine, container_box_idx);
        let mut adjusted_ascent = container_strut.ascent;
        let mut adjusted_descent = container_strut.descent;
        for token in span {
            if matches!(
                token.kind(),
                InlineTokenKind::Opportunity
                    | InlineTokenKind::FloatAnchor { .. }
                    | InlineTokenKind::AbsoluteAnchor { .. }
            ) || line_relative_alignment_for_token(engine, token, runs).is_some()
            {
                continue;
            }
            let metrics = token.run_metrics(runs);
            let ascent = metrics.ascent as f64;
            let descent = metrics.descent as f64;
            let offset =
                hierarchical_vertical_align_offset(engine, token, runs, line_height, baseline);
            let half_leading = match token.kind() {
                InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } => {
                    (metrics.line_height - ascent - descent) * 0.5
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
    let needs_container_strut = span.iter().any(|token| {
        let metrics = token.run_metrics(runs);
        matches!(token.kind(), InlineTokenKind::Image { .. })
            || (!matches!(metrics.vertical_align, VerticalAlignValue::Baseline)
                && metrics.owner_box_idx != u32::MAX
                && matches!(
                    engine
                        .reader
                        .box_layout_mode(metrics.owner_box_idx as usize),
                    Some(LayoutMode::Inline(_))
                ))
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
    for token in span {
        if matches!(
            token.kind(),
            InlineTokenKind::Opportunity
                | InlineTokenKind::FloatAnchor { .. }
                | InlineTokenKind::AbsoluteAnchor { .. }
        ) {
            continue;
        }
        let metrics = token.run_metrics(runs);
        let mut ascent = metrics.ascent as f64;
        let mut descent = metrics.descent as f64;
        // A baseline-aligned atomic inline has its baseline at its bottom
        // margin edge, while the parent line still contains a font strut.
        // Put the strut's descent below that baseline instead of splitting
        // spare line-height above and below the atomic box.
        if matches!(token.kind(), InlineTokenKind::AtomicBox { .. })
            && matches!(metrics.vertical_align, VerticalAlignValue::Baseline)
            && descent <= f64::EPSILON
        {
            ascent = ascent.max(token.line_height(runs) * 0.8);
            descent = descent.max(token.line_height(runs) * 0.2);
        }
        let offset = hierarchical_vertical_align_offset(engine, token, runs, line_height, baseline);
        let half_leading = match token.kind() {
            InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } => {
                (token.line_height(runs) - (ascent + descent)) / 2.0
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
    engine: &crate::layout::LayoutEngine<'_, '_>,
    strut: InlineBoxStrut,
    line_height: f64,
    baseline: f64,
) -> f64 {
    let mut offset = 0.0;
    let mut current = Some(strut.box_idx);
    let mut own = true;
    while let Some(box_idx) = current {
        if !matches!(
            engine.reader.box_layout_mode(box_idx),
            Some(LayoutMode::Inline(_))
        ) {
            break;
        }
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
        current = engine.reader.get_parent(box_idx);
    }
    offset
}

/// Builds final per-token placements for a measured line box.
pub(super) fn build_token_placements(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    span: &[InlineToken],
    runs: &[InlineTokenMetrics],
    tab_origin: f64,
    line_height: f64,
    baseline: f64,
    containing_width: f64,
) -> Vec<TokenPlacement> {
    #[derive(Clone, Copy)]
    struct LineRelativeGroup {
        box_idx: usize,
        alignment: VerticalAlignValue,
        top: f64,
        bottom: f64,
    }

    let aligned_ancestor = |token: &InlineToken| {
        let metrics = token.run_metrics(runs);
        if metrics.owner_box_idx == u32::MAX {
            return None;
        }
        // A replaced/atomic token carries its own top/bottom alignment in its
        // token metrics. This grouping is for non-replaced inline ancestors.
        let mut current = if matches!(
            token.kind(),
            InlineTokenKind::Glyph { .. }
                | InlineTokenKind::Ellipsis { .. }
                | InlineTokenKind::Discretionary { .. }
        ) {
            Some(metrics.owner_box_idx as usize)
        } else {
            engine.reader.get_parent(metrics.owner_box_idx as usize)
        };
        while let Some(box_idx) = current {
            if !matches!(
                engine.reader.box_layout_mode(box_idx),
                Some(LayoutMode::Inline(_))
            ) {
                break;
            }
            let alignment = engine.reader.style(box_idx).vertical_align();
            if matches!(
                alignment,
                VerticalAlignValue::Top | VerticalAlignValue::Bottom
            ) {
                return Some((box_idx, alignment));
            }
            current = engine.reader.get_parent(box_idx);
        }
        None
    };

    // Measure every top/bottom-aligned subtree with its ordinary descendant
    // offsets first. One shared correction then moves the subtree as a unit.
    let mut groups = Vec::<LineRelativeGroup>::new();
    for token in span {
        let Some((box_idx, alignment)) = aligned_ancestor(token) else {
            continue;
        };
        if matches!(
            token.kind(),
            InlineTokenKind::Opportunity
                | InlineTokenKind::FloatAnchor { .. }
                | InlineTokenKind::AbsoluteAnchor { .. }
        ) {
            continue;
        }
        let metrics = token.run_metrics(runs);
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let half_leading = if matches!(
            token.kind(),
            InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. }
        ) {
            (token.line_height(runs) - ascent - descent) * 0.5
        } else {
            0.0
        };
        let offset = hierarchical_vertical_align_offset(engine, token, runs, line_height, baseline);
        let top = baseline - ascent - half_leading - offset;
        let bottom = baseline + descent + half_leading - offset;
        if let Some(group) = groups.iter_mut().find(|group| group.box_idx == box_idx) {
            group.top = group.top.min(top);
            group.bottom = group.bottom.max(bottom);
        } else {
            groups.push(LineRelativeGroup {
                box_idx,
                alignment,
                top,
                bottom,
            });
        }
    }

    let mut curr_x = 0.0;
    span.iter()
        .map(|token| {
            let advance = token.advance_at(runs, tab_origin + curr_x);
            let owner = token.run_metrics(runs).owner_box_idx;
            let relative_offset = if owner == u32::MAX {
                Vec2::ZERO
            } else {
                let start = match token.kind() {
                    InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. } => {
                        engine.reader.get_parent(owner as usize)
                    }
                    _ => Some(owner as usize),
                };
                start.map_or(Vec2::ZERO, |box_idx| {
                    inline_relative_position_offset(engine, box_idx, containing_width)
                })
            };
            let group_offset = aligned_ancestor(token)
                .and_then(|(box_idx, _)| groups.iter().find(|group| group.box_idx == box_idx))
                .map_or(0.0, |group| match group.alignment {
                    VerticalAlignValue::Top => group.top,
                    VerticalAlignValue::Bottom => group.bottom - line_height,
                    _ => 0.0,
                });
            let placement = TokenPlacement {
                x: curr_x,
                advance,
                y_offset: (hierarchical_vertical_align_offset(
                    engine,
                    token,
                    runs,
                    line_height,
                    baseline,
                ) + group_offset
                    - relative_offset.y) as f32,
                relative_offset,
            };
            curr_x += advance;
            placement
        })
        .collect()
}
