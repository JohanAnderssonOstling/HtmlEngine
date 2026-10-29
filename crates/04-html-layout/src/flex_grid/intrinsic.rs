use super::item_sizing::ordered_children;
use super::measurement::{MeasuredItem, measure_intrinsic_item};
use super::style::{
    grid_definite_inline_minimum_exceeds_track_limit, layout_style, taffy_container_style,
    taffy_item_style,
};
use super::taffy_layout::TaffyLayoutTree;
use super::tracks::template_tracks_have_percentage_calc;
use super::{TaffyContainerKind, ensure_grid_algorithm_root, finite_f32};
use crate::layout::{LayoutEngine, resolve_definite_content_size};
use html_style_model::{
    GridRepeatCount, OverflowMode, PositionMode, UsedGridTemplateTrack, UsedGridTrackBreadth,
    UsedGridTrackSize, UsedPreferredSize as PreferredSize,
};
use taffy::geometry::Size as TaffySize;
use taffy::prelude::{AvailableSpace, Dimension};

struct FlexIntrinsicItem {
    min_contribution: f64,
    max_contribution: f64,
    outer_flex_base: f64,
    outer_min: f64,
    outer_max: f64,
    flex_grow: f64,
}

/// Intrinsic contributions with explicit optional inline and block-size bases.
pub fn intrinsic_widths(
    session: &LayoutEngine<'_, '_>,
    container_idx: usize,
    children: &[u32],
    kind: TaffyContainerKind,
    available_width: Option<f64>,
    definite_height: Option<f64>,
) -> (f64, f64) {
    let size_containment =
        kind == TaffyContainerKind::Grid && session.reader.style(container_idx).size_containment();
    let sizing_children = if size_containment { &[][..] } else { children };
    let definite_height = definite_height.or_else(|| {
        let used = session.reader.style(container_idx);
        let inline_basis = available_width.unwrap_or(0.0);
        let border_box_inset = if used.box_sizing() == html_style_model::BoxSizing::BorderBox {
            used.get_vertical_padding(inline_basis)
                + used.border_top_width() as f64
                + used.border_bottom_width() as f64
        } else {
            0.0
        };
        crate::layout::resolve_vertical_size(used.height(), None, border_box_inset)
    });
    let container_layout = layout_style(session, container_idx);
    let flex_row = matches!(
        container_layout.flex_direction,
        html_style_model::FlexDirection::Row | html_style_model::FlexDirection::RowReverse
    );
    if kind == TaffyContainerKind::Flex && flex_row {
        return row_flex_intrinsic_widths(
            session,
            children,
            container_layout.flex_wrap,
            session
                .reader
                .style(container_idx)
                .column_gap()
                .resolve(0.0),
        );
    }
    let ordered_children = ordered_children(session, sizing_children);
    let compute = |available_space: AvailableSpace| {
        // A definite block size determines how many columns contribute to a
        // column flex container's max-content width. Its min-content cross
        // size remains the largest single line contribution rather than the
        // sum of every column.
        let probe_height = matches!(available_space, AvailableSpace::MaxContent)
            .then_some(definite_height)
            .flatten();
        let mut child_styles: Vec<_> = ordered_children
            .iter()
            .map(|&child| {
                (
                    taffy_item_style(
                        session,
                        child as usize,
                        0.0,
                        probe_height,
                        kind,
                        None,
                        None,
                        probe_height,
                        true,
                    ),
                    Some(child),
                )
            })
            .collect();
        let has_indefinite_calc_track = kind == TaffyContainerKind::Grid
            && available_width.is_none()
            && template_tracks_have_percentage_calc(
                session.reader.style(container_idx).grid_template_columns(),
            );
        if !has_indefinite_calc_track {
            ensure_grid_algorithm_root(&mut child_styles, kind);
        }
        let floor_fixed_grid_column_maxima = grid_definite_inline_minimum_exceeds_track_limit(
            session,
            container_idx,
            sizing_children,
            kind,
            0.0,
        );
        let mut root_style = taffy_container_style(
            session,
            container_idx,
            0.0,
            probe_height,
            None,
            None,
            kind,
            floor_fixed_grid_column_maxima,
            true,
        );
        root_style.size.width = Dimension::auto();
        if kind == TaffyContainerKind::Grid {
            let used = session.reader.style(container_idx);
            let has_auto_repeat = used.grid_template_columns().any(|track| {
                matches!(
                    track,
                    UsedGridTemplateTrack::Repeat {
                        count: GridRepeatCount::AutoFill | GridRepeatCount::AutoFit,
                        ..
                    }
                )
            });
            let has_intrinsic_min_fixed_max = used.grid_template_columns().any(|track| {
                matches!(
                    track,
                    UsedGridTemplateTrack::Single(UsedGridTrackSize::MinMax {
                        min: UsedGridTrackBreadth::MinContent | UsedGridTrackBreadth::MaxContent,
                        max: UsedGridTrackBreadth::Length(_),
                    })
                )
            });
            if has_auto_repeat {
                if available_width.is_some()
                    || matches!(
                        used.width(),
                        PreferredSize::MinContent
                            | PreferredSize::MaxContent
                            | PreferredSize::FitContent
                    )
                {
                    root_style.min_size.width =
                        intrinsic_grid_inline_constraint(used.min_width(), available_width, 0.0);
                }
                // An indefinite auto-repeat uses the container's definite
                // maximum to choose its repetition count. This applies to
                // fixed maxima as well as percentages resolved against an
                // available inline size.
                // Taffy's root represents our content box, so a border-box
                // maximum must first lose its fixed padding and borders.
                // Percentage padding remains cyclic and contributes zero at
                // this intrinsic-sizing stage.
                let intrinsic_width_keyword = matches!(
                    used.width(),
                    PreferredSize::MinContent
                        | PreferredSize::MaxContent
                        | PreferredSize::FitContent
                );
                let padding_basis = if intrinsic_width_keyword && available_width.is_some() {
                    let maximum = used.max_width();
                    if maximum.percentage_dependent() {
                        available_width
                            .map(|basis| {
                                html_style_model::resolve_used_preferred_size(maximum, 0.0, basis)
                            })
                            .unwrap_or(0.0)
                    } else if matches!(maximum, PreferredSize::Auto) {
                        0.0
                    } else {
                        html_style_model::resolve_used_preferred_size(maximum, 0.0, 0.0)
                    }
                } else {
                    0.0
                };
                let border_box_inset =
                    if used.box_sizing() == html_style_model::BoxSizing::BorderBox {
                        used.get_horizontal_padding(padding_basis)
                            + used.border_left_width() as f64
                            + used.border_right_width() as f64
                    } else {
                        0.0
                    };
                root_style.max_size.width = intrinsic_grid_inline_constraint(
                    used.max_width(),
                    available_width,
                    border_box_inset,
                );
                // Taffy starts an indefinite auto-repeat with one repetition.
                // Preserve the definite contribution of an in-flow item as
                // the intrinsic sizing floor so Taffy can choose the repeat
                // count against the same inline size used by final layout.
                if has_intrinsic_min_fixed_max
                    && matches!(
                        used.width(),
                        PreferredSize::MinContent | PreferredSize::MaxContent
                    )
                {
                    let use_max_contribution = !matches!(used.width(), PreferredSize::MinContent);
                    let item_floor = sizing_children
                        .iter()
                        .filter(|&&child| {
                            session.reader.style(child as usize).position()
                                != PositionMode::Absolute
                        })
                        .map(|&child| {
                            let (minimum, maximum) =
                                crate::layout::box_intrinsic_widths(session, child as usize);
                            if use_max_contribution {
                                maximum
                            } else {
                                minimum
                            }
                        })
                        .fold(0.0f64, f64::max);
                    if item_floor > 0.0 {
                        root_style.min_size.width = Dimension::length(finite_f32(item_floor));
                    }
                }
            }
        }
        let mut taffy = TaffyLayoutTree::new(
            |box_idx, known, available| MeasuredItem {
                size: measure_intrinsic_item(session, box_idx, known, available),
                first_baseline: None,
            },
            kind,
            root_style,
            child_styles,
        );
        taffy.compute(TaffySize {
            width: available_space,
            height: probe_height
                .map(|height| AvailableSpace::Definite(finite_f32(height)))
                .unwrap_or(AvailableSpace::MaxContent),
        });
        taffy.root_layout().size.width.max(0.0) as f64
    };
    let mut min_width = compute(AvailableSpace::MinContent);
    if kind == TaffyContainerKind::Grid {
        let fit_content_floor = sizing_children
            .iter()
            .filter(|&&child| {
                matches!(
                    session.reader.style(child as usize).width(),
                    PreferredSize::FitContent
                )
            })
            .map(|&child| crate::layout::box_intrinsic_widths(session, child as usize).0)
            .fold(0.0f64, f64::max);
        min_width = min_width.max(fit_content_floor);
    }
    let max_width = compute(AvailableSpace::MaxContent).max(min_width);
    (min_width, max_width)
}

fn intrinsic_grid_inline_constraint(
    value: PreferredSize,
    available_width: Option<f64>,
    border_box_inset: f64,
) -> Dimension {
    let basis = if value.percentage_dependent() {
        let Some(available_width) = available_width else {
            return Dimension::auto();
        };
        available_width
    } else {
        0.0
    };
    match value {
        PreferredSize::Auto
        | PreferredSize::MinContent
        | PreferredSize::MaxContent
        | PreferredSize::FitContent
        | PreferredSize::Stretch => Dimension::auto(),
        _ => Dimension::length(finite_f32(
            (html_style_model::resolve_used_preferred_size(value, 0.0, basis) - border_box_inset)
                .max(0.0),
        )),
    }
}

fn row_flex_intrinsic_widths(
    session: &LayoutEngine<'_, '_>,
    children: &[u32],
    flex_wrap: html_style_model::FlexWrap,
    gap: f64,
) -> (f64, f64) {
    let items: Vec<_> = children
        .iter()
        .map(|&child| flex_intrinsic_item(session, child as usize))
        .collect();
    if items.is_empty() {
        return (0.0, 0.0);
    }
    let line_gaps = gap.max(0.0) * items.len().saturating_sub(1) as f64;
    let max_width = flex_intrinsic_line_width(&items, false) + line_gaps;
    let min_width = if matches!(flex_wrap, html_style_model::FlexWrap::NoWrap) {
        flex_intrinsic_line_width(&items, true) + line_gaps
    } else {
        items
            .iter()
            .map(|item| item.min_contribution)
            .fold(0.0f64, f64::max)
    };
    (min_width.max(0.0), max_width.max(min_width))
}

fn flex_intrinsic_item(session: &LayoutEngine<'_, '_>, box_idx: usize) -> FlexIntrinsicItem {
    let style = session.reader.style(box_idx);
    let layout = layout_style(session, box_idx);
    let margin = style.get_horizontal_margin(0.0);
    let padding_border = style.get_horizontal_padding(0.0)
        + style.border_left_width() as f64
        + style.border_right_width() as f64;
    let outer_inset = margin + padding_border;
    let (content_min, content_max) = crate::layout::box_content_intrinsic_widths(session, box_idx, None);
    let raw_min = (content_min + outer_inset).max(0.0);
    let raw_max = (content_max + outer_inset).max(raw_min);
    let outer_preferred = outer_preferred_width(session, box_idx, style.width(), raw_min, raw_max);
    let outer_flex_base =
        outer_preferred_width(session, box_idx, style.flex_basis(), raw_min, raw_max)
            .or_else(|| {
                matches!(style.flex_basis(), PreferredSize::Auto)
                    .then(|| {
                        outer_preferred_width(session, box_idx, style.width(), raw_min, raw_max)
                    })
                    .flatten()
            })
            .unwrap_or(raw_max);
    let automatic_min = if matches!(
        style.overflow_x(),
        OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto
    ) {
        margin
    } else {
        outer_preferred.map_or(raw_min, |preferred| raw_min.min(preferred))
    };
    let outer_min = match style.min_width() {
        PreferredSize::Auto => automatic_min,
        value => outer_preferred_width(session, box_idx, value, raw_min, raw_max).unwrap_or(0.0),
    };
    let outer_max = match style.max_width() {
        PreferredSize::Auto | PreferredSize::Percent(_) | PreferredSize::Stretch => f64::INFINITY,
        value => outer_preferred_width(session, box_idx, value, raw_min, raw_max)
            .unwrap_or(f64::INFINITY),
    }
    .max(outer_min);
    let constraint_dependent_basis = style.flex_basis().percentage_dependent()
        || matches!(
            style.flex_basis(),
            PreferredSize::Content | PreferredSize::FitContent | PreferredSize::Stretch
        );
    let contribution = |raw: f64, min_content: bool| {
        let mut value = outer_preferred.map_or(raw, |preferred| raw.max(preferred));
        if layout.flex_grow == 0.0 {
            value = value.min(outer_flex_base);
        }
        if layout.flex_shrink == 0.0 && !(min_content && constraint_dependent_basis) {
            value = value.max(outer_flex_base);
        }
        value.clamp(outer_min, outer_max)
    };
    FlexIntrinsicItem {
        min_contribution: contribution(raw_min, true),
        max_contribution: contribution(raw_max, false),
        outer_flex_base,
        outer_min,
        outer_max,
        flex_grow: layout.flex_grow as f64,
    }
}

fn outer_preferred_width(
    session: &LayoutEngine<'_, '_>,
    box_idx: usize,
    value: PreferredSize,
    raw_min: f64,
    raw_max: f64,
) -> Option<f64> {
    let style = session.reader.style(box_idx);
    let margin = style.get_horizontal_margin(0.0);
    let padding_border = style.get_horizontal_padding(0.0)
        + style.border_left_width() as f64
        + style.border_right_width() as f64;
    match value {
        PreferredSize::MinContent => Some(raw_min),
        PreferredSize::Content | PreferredSize::MaxContent | PreferredSize::FitContent => {
            Some(raw_max)
        }
        _ => resolve_definite_content_size(value, None, margin, padding_border, style.box_sizing())
            .map(|content| content + padding_border + margin),
    }
}

fn flex_intrinsic_line_width(items: &[FlexIntrinsicItem], min_content: bool) -> f64 {
    let contribution = |item: &FlexIntrinsicItem| {
        if min_content {
            item.min_contribution
        } else {
            item.max_contribution
        }
    };
    let compatible_width: f64 = items.iter().map(contribution).sum();
    let chosen_fraction = items
        .iter()
        .map(|item| {
            let free_space = contribution(item) - item.outer_flex_base;
            if free_space > 0.0 {
                if item.flex_grow >= 1.0 {
                    free_space / item.flex_grow
                } else {
                    free_space * item.flex_grow
                }
            } else {
                free_space
            }
        })
        .fold(f64::NEG_INFINITY, f64::max);
    if chosen_fraction <= 0.0 || !chosen_fraction.is_finite() {
        return compatible_width;
    }
    let grow_sum: f64 = items.iter().map(|item| item.flex_grow).sum();
    let chosen_fraction = if grow_sum > 0.0 && grow_sum < 1.0 {
        chosen_fraction / grow_sum
    } else {
        chosen_fraction
    };
    items
        .iter()
        .map(|item| {
            (item.outer_flex_base + item.flex_grow * chosen_fraction)
                .clamp(item.outer_min, item.outer_max)
        })
        .sum()
}
