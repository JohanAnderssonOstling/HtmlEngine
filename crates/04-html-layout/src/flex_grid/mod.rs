use crate::layout::LayoutEngine;
use html_style_model::{ItemAlignment, UsedPreferredSize as PreferredSize};
use kurbo::{Point, Size};
use taffy::geometry::Size as TaffySize;
use taffy::prelude::{AvailableSpace, Style, TaffyTree};
use taffy::style::Display as TaffyDisplay;

mod intrinsic;
mod item_sizing;
mod measurement;
mod style;
mod taffy_layout;
mod tracks;
pub(crate) use intrinsic::{
    intrinsic_widths, intrinsic_widths_with_available, intrinsic_widths_with_constraints,
};
use item_sizing::resolve_intrinsic_flex_basis;
pub(crate) use measurement::FlexGridState;
use style::{
    grid_definite_inline_minimum_exceeds_track_limit, layout_style, taffy_container_style,
    taffy_item_style,
};
use taffy_layout::TaffyLayoutTree;
use tracks::template_tracks_use_percentage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TaffyContainerKind {
    Flex,
    Grid,
}

pub(crate) fn layout(
    session: &mut LayoutEngine<'_, '_>,
    container_idx: usize,
    children: &[u32],
    content_origin: Point,
    content_width: f64,
    explicit_height: Option<f64>,
    min_height: Option<f64>,
    max_height: Option<f64>,
    kind: TaffyContainerKind,
) -> Size {
    let timing_started = session.start_timing();
    struct FinalItem {
        original_y: f64,
        height: f64,
        baseline_group_end: f64,
        baseline: f64,
        output: crate::layout::OutputRanges,
        baseline_aligned: bool,
    }

    let (flex_row, parent_align_items) = {
        let container_layout = layout_style(session, container_idx);
        (
            matches!(
                container_layout.flex_direction,
                html_style_model::FlexDirection::Row | html_style_model::FlexDirection::RowReverse
            ),
            container_layout.align_items,
        )
    };
    let size_containment =
        kind == TaffyContainerKind::Grid && session.reader.style(container_idx).size_containment();
    let contained_auto_height = (size_containment && explicit_height.is_none())
        .then(|| {
            contained_grid_auto_height(
                session,
                container_idx,
                content_width,
                min_height,
                max_height,
            )
        })
        .flatten();
    let rerun_indefinite_percentage_rows = kind == TaffyContainerKind::Grid
        && explicit_height.is_none()
        && template_tracks_use_percentage(session.reader.style(container_idx).grid_template_rows());
    // A single-line row flex container can establish its stretched line
    // cross size from a definite max block-size even while its own preferred
    // block-size remains auto. Keep this basis separate from percentage
    // resolution, for which the containing block is still indefinite.
    let stretch_height_basis = if kind == TaffyContainerKind::Flex && flex_row {
        explicit_height.or(max_height)
    } else {
        explicit_height
    };
    let compatible_auto_column_height =
        if kind == TaffyContainerKind::Flex && !flex_row && explicit_height.is_none() {
            fixed_basis_auto_column_height(session, children, content_width)
        } else {
            None
        };
    let root_layout_height = explicit_height
        .or(contained_auto_height)
        .or(compatible_auto_column_height);
    let child_containing_height = if size_containment {
        root_layout_height
    } else {
        explicit_height
    };
    let mut ordered_children: Vec<(usize, u32)> = children.iter().copied().enumerate().collect();
    ordered_children.sort_by_key(|(source_order, child)| {
        (layout_style(session, *child as usize).order, *source_order)
    });

    let mut child_styles = Vec::with_capacity(ordered_children.len().max(1));
    for &(_, child) in &ordered_children {
        let resolved_flex_basis = if kind == TaffyContainerKind::Flex {
            resolve_intrinsic_flex_basis(session, child as usize, content_width, flex_row)
        } else {
            None
        };
        let child_style = session.reader.style(child as usize);
        let needs_intrinsic_block_size = [
            child_style.height(),
            child_style.min_height(),
            child_style.max_height(),
        ]
        .into_iter()
        .any(|size| {
            matches!(
                size,
                PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent
            )
        }) || (kind == TaffyContainerKind::Flex
            && !flex_row
            && matches!(child_style.min_height(), PreferredSize::Auto));
        let intrinsic_block_size = needs_intrinsic_block_size.then(|| {
            // A row flex item's pre-flex auto inline size is its
            // max-content contribution. Measuring intrinsic block-size
            // keywords at the container width would introduce wrapping
            // that `flex: none` never receives in the real flex pass.
            let measure_width = if kind == TaffyContainerKind::Flex && flex_row {
                crate::layout::box_intrinsic_widths(session, child as usize).1
            } else {
                content_width
            };
            crate::layout::measure_intrinsic_block_size_isolated(
                session,
                child as usize,
                measure_width,
            )
            .height
        });
        child_styles.push((
            taffy_item_style(
                session,
                child as usize,
                content_width,
                child_containing_height,
                kind,
                resolved_flex_basis,
                intrinsic_block_size,
                stretch_height_basis,
                false,
            ),
            Some(child),
        ));
    }
    // Taffy clamps definite item contributions to a fixed `minmax(auto, …)`
    // maximum. CSS instead floors that maximum when the definite outer
    // minimum is larger, which is equivalent to fit-content at this boundary.
    let floor_fixed_grid_column_maxima = grid_definite_inline_minimum_exceeds_track_limit(
        session,
        container_idx,
        children,
        kind,
        content_width,
    );
    let root_style = taffy_container_style(
        session,
        container_idx,
        content_width,
        root_layout_height,
        min_height,
        max_height,
        kind,
        floor_fixed_grid_column_maxima,
        false,
    );
    if kind == TaffyContainerKind::Grid && child_styles.is_empty() {
        child_styles.push((
            Style {
                display: TaffyDisplay::None,
                ..Style::default()
            },
            None,
        ));
    }
    let available = TaffySize {
        width: AvailableSpace::Definite(finite_f32(content_width)),
        height: root_layout_height
            .map(|value| AvailableSpace::Definite(finite_f32(value)))
            .unwrap_or(AvailableSpace::MaxContent),
    };
    let mut taffy = TaffyLayoutTree::new(session, kind, root_style, child_styles);
    taffy.compute(available);
    if rerun_indefinite_percentage_rows {
        // CSS Grid resolves percentage rows as auto while finding an
        // intrinsic block size, then sizes the same tracks once more against
        // that now-definite size. Taffy owns both track-sizing passes; the
        // adapter supplies the first pass's result as the second pass basis.
        let intrinsic_height = taffy.layout(taffy.root()).size.height.max(0.0);
        let second_pass_available = TaffySize {
            width: available.width,
            height: AvailableSpace::Definite(intrinsic_height),
        };
        taffy.compute_with_definite_root_height(second_pass_available, intrinsic_height);
    }

    let mut taffy_items = Vec::with_capacity(ordered_children.len());
    for (index, &(_, child)) in ordered_children.iter().enumerate() {
        taffy_items.push((taffy.layout(taffy.child(index)), child));
    }
    let root_layout = taffy.layout(taffy.root());
    let root_first_baseline = taffy.root_first_baseline().map(f64::from);
    drop(taffy);
    session
        .flex_grid
        .set_container_first_baseline(container_idx, root_first_baseline);

    let mut final_items = Vec::with_capacity(taffy_items.len());
    for (layout, child) in taffy_items {
        let position = Point::new(
            content_origin.x + layout.location.x as f64,
            content_origin.y + layout.location.y as f64,
        );
        session.geometry.set_point(child as usize, position);
        let assigned = Size::new(
            layout.size.width.max(0.0) as f64,
            layout.size.height.max(0.0) as f64,
        );
        let child_layout = layout_style(session, child as usize);
        let align_self = child_layout.align_self;
        let effective_cross_alignment = if align_self == ItemAlignment::Auto {
            parent_align_items
        } else {
            align_self
        };
        let has_cross_axis_auto_margin =
            child_layout.margin_top_auto || child_layout.margin_bottom_auto;
        let authored_height_is_definite = match session.reader.style(child as usize).height() {
            PreferredSize::Px(_) => true,
            PreferredSize::Percent(_) | PreferredSize::Stretch => child_containing_height.is_some(),
            PreferredSize::Calc { .. } => child_containing_height.is_some(),
            PreferredSize::Comparison { .. } => child_containing_height.is_some(),
            PreferredSize::Auto
            | PreferredSize::MinContent
            | PreferredSize::MaxContent
            | PreferredSize::FitContent => false,
        };
        let assigned_height_is_definite = if kind == TaffyContainerKind::Grid {
            true
        } else if flex_row {
            authored_height_is_definite
                || (!has_cross_axis_auto_margin
                    && matches!(
                        effective_cross_alignment,
                        ItemAlignment::Normal | ItemAlignment::Stretch
                    ))
        } else {
            authored_height_is_definite || child_containing_height.is_some()
        };
        let output = session
            .layout_box(crate::layout::BoxLayoutRequest::taffy_assigned(
                child as usize,
                content_width,
                child_containing_height,
                assigned,
                assigned_height_is_definite,
            ))
            .output;
        let baseline = session
            .fragments
            .first_baseline_offset(output.lines.clone(), position.y)
            .unwrap_or(assigned.height);
        let baseline_aligned = !has_cross_axis_auto_margin
            && (kind == TaffyContainerKind::Grid || flex_row)
            && (align_self == ItemAlignment::Baseline
                || (align_self == ItemAlignment::Auto
                    && parent_align_items == ItemAlignment::Baseline));
        final_items.push(FinalItem {
            original_y: position.y,
            height: assigned.height,
            baseline_group_end: position.y + assigned.height,
            baseline,
            output,
            baseline_aligned,
        });
    }

    let mut result = Size::new(
        root_layout.size.width.max(0.0) as f64,
        root_layout.size.height.max(0.0) as f64,
    );
    let mut aligned: Vec<usize> = final_items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| item.baseline_aligned.then_some(idx))
        .collect();
    aligned.sort_unstable_by(|&left, &right| {
        final_items[left]
            .baseline_group_end
            .total_cmp(&final_items[right].baseline_group_end)
    });
    let mut group_start = 0usize;
    while group_start < aligned.len() {
        let group_end_coordinate = final_items[aligned[group_start]].baseline_group_end;
        let group_end = aligned[group_start..]
            .iter()
            .position(|&idx| {
                (final_items[idx].baseline_group_end - group_end_coordinate).abs() > 0.01
            })
            .map(|offset| group_start + offset)
            .unwrap_or(aligned.len());
        let group = &aligned[group_start..group_end];
        if group.len() <= 1 {
            group_start = group_end;
            continue;
        }
        let baseline_origin = group
            .iter()
            .map(|&idx| final_items[idx].original_y)
            .fold(f64::INFINITY, f64::min);
        let max_baseline = group
            .iter()
            .map(|&idx| final_items[idx].baseline)
            .fold(0.0f64, f64::max);
        for &idx in group {
            let item = &final_items[idx];
            let desired_y = baseline_origin + max_baseline - item.baseline;
            let delta = desired_y - item.original_y;
            if delta.abs() > f64::EPSILON {
                crate::layout::translate_laid_out_output(
                    session,
                    &item.output,
                    kurbo::Vec2::new(0.0, delta),
                );
            }
            if !size_containment {
                result.height = result
                    .height
                    .max(desired_y + item.height - content_origin.y);
            }
        }
        group_start = group_end;
    }
    session.record_timing(|timings| timings.layout_flex_grid += timing_started.elapsed());
    result
}

/// Compute the auto block size of a size-contained Grid as though it had no
/// in-flow contents. The display:none sentinel makes Taffy run Grid track
/// sizing for genuinely empty grids without creating an occupying item.
fn contained_grid_auto_height(
    session: &mut LayoutEngine<'_, '_>,
    container_idx: usize,
    content_width: f64,
    min_height: Option<f64>,
    max_height: Option<f64>,
) -> Option<f64> {
    let root_style = taffy_container_style(
        session,
        container_idx,
        content_width,
        None,
        min_height,
        max_height,
        TaffyContainerKind::Grid,
        false,
        true,
    );
    let sentinel = Style {
        display: TaffyDisplay::None,
        ..Style::default()
    };
    let available = TaffySize {
        width: AvailableSpace::Definite(finite_f32(content_width)),
        height: AvailableSpace::MaxContent,
    };
    let mut taffy = TaffyLayoutTree::new(
        session,
        TaffyContainerKind::Grid,
        root_style,
        vec![(sentinel, None)],
    );
    taffy.compute(available);
    Some(taffy.layout(taffy.root()).size.height.max(0.0) as f64)
}

fn finite_f32(value: f64) -> f32 {
    value.clamp(0.0, f32::MAX as f64) as f32
}

/// Preserve the interoperable intrinsic main-size behavior for an auto-height
/// single-line column whose items all have definite flex bases and minimums.
/// This is intentionally narrower than the general intrinsic main-size
/// algorithm: uncertain content-based minimums remain Taffy-owned.
fn fixed_basis_auto_column_height(
    session: &LayoutEngine<'_, '_>,
    children: &[u32],
    containing_width: f64,
) -> Option<f64> {
    let parent = children
        .first()
        .and_then(|child| session.reader.get_parent(*child as usize))?;
    if !matches!(
        layout_style(session, parent).flex_wrap,
        html_style_model::FlexWrap::NoWrap
    ) {
        return None;
    }
    let mut item_count = 0usize;
    let mut total = 0.0;
    for &child in children {
        let child_idx = child as usize;
        let style = session.reader.style(child_idx);
        if style.position() == html_style_model::PositionMode::Absolute {
            continue;
        }
        let PreferredSize::Px(basis) = style.flex_basis() else {
            return None;
        };
        let PreferredSize::Px(minimum) = style.min_height() else {
            return None;
        };
        let padding_border = style.get_vertical_padding(containing_width)
            + style.border_top_width() as f64
            + style.border_bottom_width() as f64;
        let as_content = |value: f32| match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => value.max(0.0) as f64,
            html_style_model::BoxSizing::BorderBox => {
                (value.max(0.0) as f64 - padding_border).max(0.0)
            }
        };
        let mut content = as_content(basis).max(as_content(minimum));
        if let PreferredSize::Px(maximum) = style.max_height() {
            content = content.min(as_content(maximum).max(as_content(minimum)));
        } else if !matches!(style.max_height(), PreferredSize::Auto) {
            return None;
        }
        total += content
            + padding_border
            + style.margin_top().resolve(containing_width)
            + style.margin_bottom().resolve(containing_width);
        item_count += 1;
    }
    if item_count > 1 {
        total += session
            .reader
            .style(parent)
            .row_gap()
            .resolve(containing_width)
            * (item_count - 1) as f64;
    }
    (item_count > 0).then_some(total.max(0.0))
}

/// Taffy dispatches every childless node through generic leaf layout even
/// when its display mode is Grid. A non-generated sentinel keeps empty grids
/// on the Grid path without occupying an auto-fit track.
fn ensure_grid_algorithm_root(
    taffy: &mut TaffyTree<u32>,
    root_children: &mut Vec<taffy::tree::NodeId>,
    kind: TaffyContainerKind,
) {
    if kind == TaffyContainerKind::Grid && root_children.is_empty() {
        let sentinel = taffy
            .new_leaf(Style {
                display: TaffyDisplay::None,
                ..Style::default()
            })
            .expect("a bounded in-memory Taffy tree cannot reject an empty-grid sentinel");
        root_children.push(sentinel);
    }
}

include!("tests.rs");
