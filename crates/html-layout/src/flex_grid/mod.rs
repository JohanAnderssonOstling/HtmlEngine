use crate::layout::LayoutEngine;
use html_style_model::{ItemAlignment, UsedPreferredSize as PreferredSize};
use kurbo::{Point, Size};
use std::time::Instant;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::{AvailableSpace, Style, TaffyTree};
use taffy::style::Display as TaffyDisplay;

mod intrinsic;
mod measurement;
mod style;
mod tracks;
pub(crate) use intrinsic::intrinsic_widths;
pub(crate) use measurement::FlexGridState;
use measurement::measure_item;
use style::{grid_definite_inline_minimum_exceeds_track_limit, layout_style, resolve_intrinsic_flex_basis, taffy_container_style, taffy_item_style};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaffyContainerKind {
    Flex,
    Grid,
}

pub(crate) fn layout(
    session: &mut LayoutEngine<'_, '_>, container_idx: usize, children: &[u32], content_origin: Point, content_width: f64, explicit_height: Option<f64>, min_height: Option<f64>, max_height: Option<f64>, kind: TaffyContainerKind,
) -> Size {
    let timing_started = Instant::now();
    struct FinalItem {
        box_idx: usize,
        original_y: f64,
        height: f64,
        baseline_group_end: f64,
        baseline: f64,
        output: crate::layout::OutputRanges,
        baseline_aligned: bool,
    }

    let container_layout = layout_style(session, container_idx);
    let flex_row = matches!(container_layout.flex_direction, html_style_model::FlexDirection::Row | html_style_model::FlexDirection::RowReverse);
    let parent_align_items = container_layout.align_items;
    let mut ordered_children: Vec<(usize, u32)> = children.iter().copied().enumerate().collect();
    ordered_children.sort_by_key(|(source_order, child)| (layout_style(session, *child as usize).order, *source_order));

    let mut taffy = TaffyTree::<u32>::with_capacity(ordered_children.len() + 1);
    taffy.disable_rounding();
    let mut child_nodes = Vec::with_capacity(ordered_children.len());
    for &(_, child) in &ordered_children {
        let resolved_flex_basis = if kind == TaffyContainerKind::Flex { resolve_intrinsic_flex_basis(session, child as usize, content_width, flex_row) } else { None };
        let node = taffy.new_leaf_with_context(taffy_item_style(session, child as usize, content_width, explicit_height, kind, resolved_flex_basis), child).expect("a bounded in-memory Taffy tree cannot reject a leaf");
        child_nodes.push((node, child));
    }
    // Taffy clamps definite item contributions to a fixed `minmax(auto, …)`
    // maximum. CSS instead floors that maximum when the definite outer
    // minimum is larger, which is equivalent to fit-content at this boundary.
    let floor_fixed_grid_column_maxima = grid_definite_inline_minimum_exceeds_track_limit(session, container_idx, children, kind, content_width);
    let root_style = taffy_container_style(session, container_idx, content_width, explicit_height, min_height, max_height, kind, floor_fixed_grid_column_maxima);
    let mut root_children = child_nodes.iter().map(|(node, _)| *node).collect::<Vec<_>>();
    // Taffy dispatches every childless node through generic leaf layout,
    // even when its display mode is grid. A non-generated sentinel keeps
    // empty grids on the grid path so explicit tracks and their gaps still
    // contribute to intrinsic size without occupying an auto-fit track.
    if kind == TaffyContainerKind::Grid && root_children.is_empty() {
        let sentinel = taffy.new_leaf(Style { display: TaffyDisplay::None, ..Style::default() }).expect("a bounded in-memory Taffy tree cannot reject an empty-grid sentinel");
        root_children.push(sentinel);
    }
    let root = taffy.new_with_children(root_style, &root_children).expect("a bounded in-memory Taffy tree cannot reject its root");
    let available = TaffySize { width: AvailableSpace::Definite(finite_f32(content_width)), height: explicit_height.map(|value| AvailableSpace::Definite(finite_f32(value))).unwrap_or(AvailableSpace::MaxContent) };

    taffy
        .compute_layout_with_measure(root, available, |known, available, _, context, _| {
            let Some(box_idx) = context.copied() else { return TaffySize::ZERO };
            measure_item(session, box_idx as usize, known, available)
        })
        .expect("Taffy layout over a valid local tree must succeed");

    let mut final_items = Vec::with_capacity(child_nodes.len());
    for (node, child) in child_nodes {
        let layout = *taffy.layout(node).expect("computed Taffy child layout exists");
        let position = Point::new(content_origin.x + layout.location.x as f64, content_origin.y + layout.location.y as f64);
        session.geometry.set_point(child as usize, position);
        let assigned = Size::new(layout.size.width.max(0.0) as f64, layout.size.height.max(0.0) as f64);
        let child_layout = layout_style(session, child as usize);
        let align_self = child_layout.align_self;
        let effective_cross_alignment = if align_self == ItemAlignment::Auto { parent_align_items } else { align_self };
        let has_cross_axis_auto_margin = child_layout.margin_top_auto || child_layout.margin_bottom_auto;
        let authored_height_is_definite = match session.reader.style(child as usize).height() {
            PreferredSize::Px(_) => true,
            PreferredSize::Percent(_) | PreferredSize::Stretch => explicit_height.is_some(),
            PreferredSize::Calc { .. } => explicit_height.is_some(),
            PreferredSize::Comparison { .. } => explicit_height.is_some(),
            PreferredSize::Auto | PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => false,
        };
        let assigned_height_is_definite = if kind == TaffyContainerKind::Grid {
            true
        } else if flex_row {
            authored_height_is_definite || (!has_cross_axis_auto_margin && matches!(effective_cross_alignment, ItemAlignment::Normal | ItemAlignment::Stretch))
        } else {
            authored_height_is_definite || explicit_height.is_some()
        };
        let output = session.layout_box(crate::layout::BoxLayoutRequest::taffy_assigned(child as usize, content_width, explicit_height, assigned, assigned_height_is_definite)).output;
        let baseline = session.fragments.first_baseline_offset(output.lines.clone(), position.y).unwrap_or(assigned.height);
        let baseline_aligned = !has_cross_axis_auto_margin && (kind == TaffyContainerKind::Grid || flex_row) && (align_self == ItemAlignment::Baseline || (align_self == ItemAlignment::Auto && parent_align_items == ItemAlignment::Baseline));
        final_items.push(FinalItem { box_idx: child as usize, original_y: position.y, height: assigned.height, baseline_group_end: position.y + assigned.height, baseline, output, baseline_aligned });
    }

    let root_layout = taffy.layout(root).expect("computed Taffy root layout exists");
    let mut result = Size::new(root_layout.size.width.max(0.0) as f64, root_layout.size.height.max(0.0) as f64);
    let mut aligned: Vec<usize> = final_items.iter().enumerate().filter_map(|(idx, item)| item.baseline_aligned.then_some(idx)).collect();
    aligned.sort_unstable_by(|&left, &right| final_items[left].baseline_group_end.total_cmp(&final_items[right].baseline_group_end));
    let mut group_start = 0usize;
    while group_start < aligned.len() {
        let group_end_coordinate = final_items[aligned[group_start]].baseline_group_end;
        let group_end = aligned[group_start..].iter().position(|&idx| (final_items[idx].baseline_group_end - group_end_coordinate).abs() > 0.01).map(|offset| group_start + offset).unwrap_or(aligned.len());
        let group = &aligned[group_start..group_end];
        if group.len() <= 1 {
            group_start = group_end;
            continue;
        }
        let baseline_origin = group.iter().map(|&idx| final_items[idx].original_y).fold(f64::INFINITY, f64::min);
        let max_baseline = group.iter().map(|&idx| final_items[idx].baseline).fold(0.0f64, f64::max);
        for &idx in group {
            let item = &final_items[idx];
            let desired_y = baseline_origin + max_baseline - item.baseline;
            let delta = desired_y - item.original_y;
            if delta.abs() > f64::EPSILON {
                crate::layout::translate_laid_out_subtree_output(session, item.box_idx, true, &item.output, kurbo::Vec2::new(0.0, delta));
            }
            result.height = result.height.max(desired_y + item.height - content_origin.y);
        }
        group_start = group_end;
    }
    session.record_timing(|timings| timings.layout_flex_grid += timing_started.elapsed());
    result
}

fn finite_f32(value: f64) -> f32 {
    value.clamp(0.0, f32::MAX as f64) as f32
}

include!("tests.rs");
