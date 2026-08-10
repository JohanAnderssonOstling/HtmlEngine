use super::measurement::preferred_aspect_ratio;
use super::style::{grid_definite_inline_minimum_exceeds_track_limit, layout_style, taffy_container_style, taffy_item_style};
use super::tracks::template_tracks_have_percentage_calc;
use super::{TaffyContainerKind, ensure_grid_algorithm_root, finite_f32};
use crate::layout::{LayoutEngine, measure_replaced_content};
use html_style_model::{GridRepeatCount, OverflowMode, PositionMode, UsedGridTemplateTrack, UsedGridTrackBreadth, UsedGridTrackSize, UsedPreferredSize as PreferredSize};
use taffy::geometry::Size as TaffySize;
use taffy::prelude::{AvailableSpace, Dimension, TaffyTree};

struct FlexIntrinsicItem {
    min_contribution: f64,
    max_contribution: f64,
    outer_flex_base: f64,
    outer_min: f64,
    outer_max: f64,
    flex_grow: f64,
}

pub(crate) fn intrinsic_widths(session: &LayoutEngine<'_, '_>, container_idx: usize, children: &[u32], kind: TaffyContainerKind) -> (f64, f64) {
    intrinsic_widths_with_constraints(session, container_idx, children, kind, None, None)
}

pub(crate) fn intrinsic_widths_with_available(session: &LayoutEngine<'_, '_>, container_idx: usize, children: &[u32], kind: TaffyContainerKind, available_width: Option<f64>) -> (f64, f64) {
    intrinsic_widths_with_constraints(session, container_idx, children, kind, available_width, None)
}

pub(crate) fn intrinsic_widths_with_constraints(
    session: &LayoutEngine<'_, '_>, container_idx: usize, children: &[u32], kind: TaffyContainerKind, available_width: Option<f64>, definite_height: Option<f64>,
) -> (f64, f64) {
    let size_containment = kind == TaffyContainerKind::Grid && session.reader.style(container_idx).size_containment();
    let sizing_children = if size_containment { &[][..] } else { children };
    let definite_height = definite_height.or_else(|| {
        let used = session.reader.style(container_idx);
        let inline_basis = available_width.unwrap_or(0.0);
        let border_box_inset = if used.box_sizing() == html_style_model::BoxSizing::BorderBox {
            used.get_vertical_padding(inline_basis) + used.border_top_width() as f64 + used.border_bottom_width() as f64
        } else {
            0.0
        };
        crate::layout::resolve_vertical_size(used.height(), None, border_box_inset)
    });
    let container_layout = layout_style(session, container_idx);
    let flex_row = matches!(container_layout.flex_direction, html_style_model::FlexDirection::Row | html_style_model::FlexDirection::RowReverse);
    if kind == TaffyContainerKind::Flex && flex_row {
        return row_flex_intrinsic_widths(session, children, container_layout.flex_wrap, session.reader.style(container_idx).column_gap().resolve(0.0));
    }
    let compute = |available_space: AvailableSpace| {
        // A definite block size determines how many columns contribute to a
        // column flex container's max-content width. Its min-content cross
        // size remains the largest single line contribution rather than the
        // sum of every column.
        let probe_height = matches!(available_space, AvailableSpace::MaxContent).then_some(definite_height).flatten();
        let mut ordered_children: Vec<(usize, u32)> = sizing_children.iter().copied().enumerate().collect();
        ordered_children.sort_by_key(|(source_order, child)| (layout_style(session, *child as usize).order, *source_order));
        let mut taffy = TaffyTree::<u32>::with_capacity(ordered_children.len() + 1);
        taffy.disable_rounding();
        let mut child_nodes: Vec<_> = ordered_children
            .iter()
            .map(|(_, child)| {
                taffy
                    .new_leaf_with_context(taffy_item_style(session, *child as usize, 0.0, probe_height, kind, None, None, probe_height, true), *child)
                    .expect("intrinsic Taffy leaf is valid")
            })
            .collect();
        let has_indefinite_calc_track = kind == TaffyContainerKind::Grid
            && available_width.is_none()
            && template_tracks_have_percentage_calc(session.reader.style(container_idx).grid_template_columns());
        if !has_indefinite_calc_track {
            ensure_grid_algorithm_root(&mut taffy, &mut child_nodes, kind);
        }
        let floor_fixed_grid_column_maxima = grid_definite_inline_minimum_exceeds_track_limit(session, container_idx, sizing_children, kind, 0.0);
        let mut root_style = taffy_container_style(session, container_idx, 0.0, probe_height, None, None, kind, floor_fixed_grid_column_maxima, true);
        root_style.size.width = Dimension::auto();
        if kind == TaffyContainerKind::Grid {
            let used = session.reader.style(container_idx);
            let has_auto_repeat = used
                .grid_template_columns()
                .any(|track| matches!(track, UsedGridTemplateTrack::Repeat { count: GridRepeatCount::AutoFill | GridRepeatCount::AutoFit, .. }));
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
                if available_width.is_some() || matches!(used.width(), PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent) {
                    root_style.min_size.width = intrinsic_grid_inline_constraint(used.min_width(), available_width, 0.0);
                }
                // An indefinite auto-repeat uses the container's definite
                // maximum to choose its repetition count. This applies to
                // fixed maxima as well as percentages resolved against an
                // available inline size.
                // Taffy's root represents our content box, so a border-box
                // maximum must first lose its fixed padding and borders.
                // Percentage padding remains cyclic and contributes zero at
                // this intrinsic-sizing stage.
                let intrinsic_width_keyword = matches!(used.width(), PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent);
                let padding_basis = if intrinsic_width_keyword && available_width.is_some() {
                    let maximum = used.max_width();
                    if maximum.percentage_dependent() {
                        available_width.map(|basis| html_style_model::resolve_used_preferred_size(maximum, 0.0, basis)).unwrap_or(0.0)
                    } else if matches!(maximum, PreferredSize::Auto) {
                        0.0
                    } else {
                        html_style_model::resolve_used_preferred_size(maximum, 0.0, 0.0)
                    }
                } else {
                    0.0
                };
                let border_box_inset = if used.box_sizing() == html_style_model::BoxSizing::BorderBox {
                    used.get_horizontal_padding(padding_basis) + used.border_left_width() as f64 + used.border_right_width() as f64
                } else {
                    0.0
                };
                root_style.max_size.width = intrinsic_grid_inline_constraint(used.max_width(), available_width, border_box_inset);
                // Taffy starts an indefinite auto-repeat with one repetition.
                // Preserve the definite contribution of an in-flow item as
                // the intrinsic sizing floor so Taffy can choose the repeat
                // count against the same inline size used by final layout.
                if has_intrinsic_min_fixed_max && matches!(used.width(), PreferredSize::MinContent | PreferredSize::MaxContent) {
                    let use_max_contribution = !matches!(used.width(), PreferredSize::MinContent);
                    let item_floor = sizing_children
                        .iter()
                        .filter(|&&child| session.reader.style(child as usize).position() != PositionMode::Absolute)
                        .map(|&child| {
                            let (minimum, maximum) = crate::layout::box_intrinsic_widths(session, child as usize);
                            if use_max_contribution { maximum } else { minimum }
                        })
                        .fold(0.0f64, f64::max);
                    if item_floor > 0.0 {
                        root_style.min_size.width = Dimension::length(finite_f32(item_floor));
                    }
                }
            }
        }
        let root = taffy.new_with_children(root_style, &child_nodes).expect("intrinsic Taffy root is valid");
        taffy
            .compute_layout_with_measure(
                root,
                TaffySize {
                    width: available_space,
                    height: probe_height.map(|height| AvailableSpace::Definite(finite_f32(height))).unwrap_or(AvailableSpace::MaxContent),
                },
                |known, available, _, context, _| {
                let Some(box_idx) = context.copied() else { return TaffySize::ZERO };
                measure_item(session, box_idx as usize, known, available)
                },
            )
            .expect("intrinsic Taffy layout succeeds");
        taffy.layout(root).expect("intrinsic Taffy root layout exists").size.width.max(0.0) as f64
    };
    let mut min_width = compute(AvailableSpace::MinContent);
    if kind == TaffyContainerKind::Grid {
        let fit_content_floor =
            sizing_children.iter().filter(|&&child| matches!(session.reader.style(child as usize).width(), PreferredSize::FitContent)).map(|&child| crate::layout::box_intrinsic_widths(session, child as usize).0).fold(0.0f64, f64::max);
        min_width = min_width.max(fit_content_floor);
    }
    let max_width = compute(AvailableSpace::MaxContent).max(min_width);
    (min_width, max_width)
}

fn intrinsic_grid_inline_constraint(value: PreferredSize, available_width: Option<f64>, border_box_inset: f64) -> Dimension {
    let basis = if value.percentage_dependent() {
        let Some(available_width) = available_width else { return Dimension::auto() };
        available_width
    } else {
        0.0
    };
    match value {
        PreferredSize::Auto | PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent | PreferredSize::Stretch => Dimension::auto(),
        _ => Dimension::length(finite_f32((html_style_model::resolve_used_preferred_size(value, 0.0, basis) - border_box_inset).max(0.0))),
    }
}

fn row_flex_intrinsic_widths(session: &LayoutEngine<'_, '_>, children: &[u32], flex_wrap: html_style_model::FlexWrap, gap: f64) -> (f64, f64) {
    let items: Vec<_> = children.iter().map(|&child| flex_intrinsic_item(session, child as usize)).collect();
    if items.is_empty() {
        return (0.0, 0.0);
    }
    let line_gaps = gap.max(0.0) * items.len().saturating_sub(1) as f64;
    let max_width = flex_intrinsic_line_width(&items, false) + line_gaps;
    let min_width = if matches!(flex_wrap, html_style_model::FlexWrap::NoWrap) { flex_intrinsic_line_width(&items, true) + line_gaps } else { items.iter().map(|item| item.min_contribution).fold(0.0f64, f64::max) };
    (min_width.max(0.0), max_width.max(min_width))
}

fn flex_intrinsic_item(session: &LayoutEngine<'_, '_>, box_idx: usize) -> FlexIntrinsicItem {
    let style = session.reader.style(box_idx);
    let layout = layout_style(session, box_idx);
    let margin = style.get_horizontal_margin(0.0);
    let padding_border = style.get_horizontal_padding(0.0) + style.border_left_width() as f64 + style.border_right_width() as f64;
    let outer_inset = margin + padding_border;
    let (content_min, content_max) = crate::layout::box_content_intrinsic_widths(session, box_idx);
    let raw_min = (content_min + outer_inset).max(0.0);
    let raw_max = (content_max + outer_inset).max(raw_min);
    let outer_preferred = outer_preferred_width(session, box_idx, style.width(), raw_min, raw_max);
    let outer_flex_base = outer_preferred_width(session, box_idx, style.flex_basis(), raw_min, raw_max).or_else(|| outer_preferred_width(session, box_idx, style.width(), raw_min, raw_max)).unwrap_or(raw_max);
    let automatic_min = if matches!(style.overflow_x(), OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto) { margin } else { outer_preferred.map_or(raw_min, |preferred| raw_min.min(preferred)) };
    let outer_min = match style.min_width() {
        PreferredSize::Auto => automatic_min,
        value => outer_preferred_width(session, box_idx, value, raw_min, raw_max).unwrap_or(0.0),
    };
    let outer_max = match style.max_width() {
        PreferredSize::Auto | PreferredSize::Percent(_) | PreferredSize::Stretch => f64::INFINITY,
        value => outer_preferred_width(session, box_idx, value, raw_min, raw_max).unwrap_or(f64::INFINITY),
    }
    .max(outer_min);
    let percentage_dependent_basis = style.flex_basis().percentage_dependent();
    let contribution = |raw: f64, min_content: bool| {
        let mut value = outer_preferred.map_or(raw, |preferred| raw.max(preferred));
        if layout.flex_grow == 0.0 {
            value = value.min(outer_flex_base);
        }
        if layout.flex_shrink == 0.0 && !(min_content && percentage_dependent_basis) {
            value = value.max(outer_flex_base);
        }
        value.clamp(outer_min, outer_max)
    };
    FlexIntrinsicItem { min_contribution: contribution(raw_min, true), max_contribution: contribution(raw_max, false), outer_flex_base, outer_min, outer_max, flex_grow: layout.flex_grow as f64 }
}

fn outer_preferred_width(session: &LayoutEngine<'_, '_>, box_idx: usize, value: PreferredSize, raw_min: f64, raw_max: f64) -> Option<f64> {
    let style = session.reader.style(box_idx);
    let margin = style.get_horizontal_margin(0.0);
    let padding_border = style.get_horizontal_padding(0.0) + style.border_left_width() as f64 + style.border_right_width() as f64;
    let specified = |width: f64| match style.box_sizing() {
        html_style_model::BoxSizing::ContentBox => width.max(0.0) + margin + padding_border,
        html_style_model::BoxSizing::BorderBox => width.max(padding_border) + margin,
    };
    match value {
        PreferredSize::Auto | PreferredSize::Percent(_) | PreferredSize::Stretch => None,
        PreferredSize::Px(width) => Some(specified(width as f64)),
        PreferredSize::Calc { absolute_px, percentage_dependent: false, .. } => Some(specified(absolute_px as f64)),
        PreferredSize::Calc { percentage_dependent: true, .. } => None,
        PreferredSize::Comparison { .. } if value.percentage_dependent() => None,
        PreferredSize::Comparison { .. } => Some(specified(html_style_model::resolve_used_preferred_size(value, 0.0, 0.0))),
        PreferredSize::MinContent => Some(raw_min),
        PreferredSize::MaxContent | PreferredSize::FitContent => Some(raw_max),
    }
}

fn measure_item(session: &LayoutEngine<'_, '_>, box_idx: usize, known: TaffySize<Option<f32>>, available: TaffySize<AvailableSpace>) -> TaffySize<f32> {
    if let Some(intrinsic) = session.reader.image_intrinsic_size(box_idx) {
        return measure_replaced_content(intrinsic, preferred_aspect_ratio(session, box_idx, intrinsic), known, available);
    }
    let style = session.reader.style(box_idx);
    let (outer_min_width, outer_max_width) = crate::layout::box_intrinsic_widths(session, box_idx);
    let horizontal_inset = style.get_horizontal_margin_padding(0.0) + style.border_left_width() as f64 + style.border_right_width() as f64;
    let mut min_width = (outer_min_width - horizontal_inset).max(0.0);
    let mut max_width = (outer_max_width - horizontal_inset).max(min_width);
    let vertical_inset = style.get_vertical_padding(0.0) + style.border_top_width() as f64 + style.border_bottom_width() as f64;
    let available_height = known
        .height
        .or(match available.height {
            AvailableSpace::Definite(value) => Some(value),
            AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
        })
        .map(|value| finite_f32((f64::from(value) - vertical_inset).max(0.0)));
    if let Some(width) = available_height.and_then(|height| crate::layout::percentage_height_image_width(session, box_idx, f64::from(height))) {
        min_width = width;
        max_width = width;
    }
    let width = known.width.unwrap_or_else(|| match available.width {
        AvailableSpace::Definite(value) => value,
        AvailableSpace::MinContent => finite_f32(min_width),
        AvailableSpace::MaxContent => finite_f32(max_width),
    });
    TaffySize { width, height: known.height.unwrap_or(0.0) }
}

fn flex_intrinsic_line_width(items: &[FlexIntrinsicItem], min_content: bool) -> f64 {
    let contribution = |item: &FlexIntrinsicItem| if min_content { item.min_contribution } else { item.max_contribution };
    let compatible_width: f64 = items.iter().map(contribution).sum();
    let chosen_fraction = items
        .iter()
        .map(|item| {
            let free_space = contribution(item) - item.outer_flex_base;
            if free_space > 0.0 { if item.flex_grow >= 1.0 { free_space / item.flex_grow } else { free_space * item.flex_grow } } else { free_space }
        })
        .fold(f64::NEG_INFINITY, f64::max);
    if chosen_fraction <= 0.0 || !chosen_fraction.is_finite() {
        return compatible_width;
    }
    let grow_sum: f64 = items.iter().map(|item| item.flex_grow).sum();
    let chosen_fraction = if grow_sum > 0.0 && grow_sum < 1.0 { chosen_fraction / grow_sum } else { chosen_fraction };
    items.iter().map(|item| (item.outer_flex_base + item.flex_grow * chosen_fraction).clamp(item.outer_min, item.outer_max)).sum()
}
