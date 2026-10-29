//! Prepare item styles and the solver policy.
use super::item_sizing::{ordered_children, resolve_intrinsic_flex_basis};
use super::measurement::measure_item_with_baseline;
use super::style::{
    grid_definite_inline_minimum_exceeds_track_limit, layout_style, taffy_container_style,
    taffy_item_style,
};
use super::taffy_layout::TaffyLayoutTree;
use super::tracks::template_tracks_use_percentage;
use super::{
    FlexGridInput, FlexGridPolicy, PreparedFlexGrid, TaffyContainerKind,
    ensure_grid_algorithm_root, finite_f32,
};
use crate::layout::{BoxLayoutRequest, LayoutEngine, measure_box_isolated};
use html_style_model::UsedPreferredSize as PreferredSize;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::AvailableSpace;

pub(super) fn prepare(
    session: &mut LayoutEngine<'_, '_>,
    input: &FlexGridInput<'_>,
) -> (FlexGridPolicy, PreparedFlexGrid) {
    let (flex_row, parent_align_items) = {
        let container_layout = layout_style(session, input.container_idx);
        (
            matches!(
                container_layout.flex_direction,
                html_style_model::FlexDirection::Row | html_style_model::FlexDirection::RowReverse
            ),
            container_layout.align_items,
        )
    };
    let size_containment = input.kind == TaffyContainerKind::Grid
        && session.reader.style(input.container_idx).size_containment();
    let contained_auto_height = (size_containment && input.explicit_height.is_none())
        .then(|| {
            contained_grid_auto_height(
                session,
                input.container_idx,
                input.content_width,
                input.min_height,
                input.max_height,
            )
        })
        .flatten();
    let rerun_indefinite_percentage_rows = input.kind == TaffyContainerKind::Grid
        && input.explicit_height.is_none()
        && template_tracks_use_percentage(
            session
                .reader
                .style(input.container_idx)
                .grid_template_rows(),
        );
    // A single-line row flex container can establish its stretched line
    // cross size from a definite max block-size even while its own preferred
    // block-size remains auto. Keep this basis separate from percentage
    // resolution, for which the containing block is still indefinite.
    let stretch_height_basis = if input.kind == TaffyContainerKind::Flex && flex_row {
        input.explicit_height.or(input.max_height)
    } else {
        input.explicit_height
    };
    let compatible_auto_column_height =
        if input.kind == TaffyContainerKind::Flex && !flex_row && input.explicit_height.is_none() {
            fixed_basis_auto_column_height(session, input.children, input.content_width)
        } else {
            None
        };
    let root_layout_height = input
        .explicit_height
        .or(contained_auto_height)
        .or(compatible_auto_column_height);
    let child_containing_height = if size_containment {
        root_layout_height
    } else {
        input.explicit_height
    };
    let ordered_children = ordered_children(session, input.children);

    let mut child_styles = Vec::with_capacity(ordered_children.len().max(1));
    for &child in &ordered_children {
        let resolved_flex_basis = if input.kind == TaffyContainerKind::Flex {
            resolve_intrinsic_flex_basis(
                session,
                child as usize,
                input.content_width,
                child_containing_height,
                flex_row,
            )
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
        }) || (input.kind == TaffyContainerKind::Flex
            && !flex_row
            && matches!(child_style.min_height(), PreferredSize::Auto));
        let intrinsic_block_size = needs_intrinsic_block_size.then(|| {
            // A row flex item's pre-flex auto inline size is its
            // max-content contribution. Measuring intrinsic block-size
            // keywords at the container width would introduce wrapping
            // that `flex: none` never receives in the real flex pass.
            let measure_width = if input.kind == TaffyContainerKind::Flex && flex_row {
                crate::layout::box_intrinsic_widths(session, child as usize).1
            } else {
                input.content_width
            };
            measure_box_isolated(
                session,
                BoxLayoutRequest::normal(child as usize, measure_width.max(0.0), None)
                    .for_intrinsic_block_measurement(),
            )
            .size_with_floats()
            .height
        });
        child_styles.push((
            taffy_item_style(
                session,
                child as usize,
                input.content_width,
                child_containing_height,
                input.kind,
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
        input.container_idx,
        input.children,
        input.kind,
        input.content_width,
    );
    let root_style = taffy_container_style(
        session,
        input.container_idx,
        input.content_width,
        root_layout_height,
        input.min_height,
        input.max_height,
        input.kind,
        floor_fixed_grid_column_maxima,
        false,
    );
    ensure_grid_algorithm_root(&mut child_styles, input.kind);
    let available = TaffySize {
        width: AvailableSpace::Definite(finite_f32(input.content_width)),
        height: root_layout_height
            .map(|value| AvailableSpace::Definite(finite_f32(value)))
            .unwrap_or(AvailableSpace::MaxContent),
    };
    let policy = FlexGridPolicy {
        flex_row,
        parent_align_items,
        size_containment,
        child_containing_height,
    };
    let prepared = PreparedFlexGrid {
        child_styles,
        root_style,
        available,
        rerun_indefinite_percentage_rows,
    };
    (policy, prepared)
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
    let mut child_styles = Vec::new();
    ensure_grid_algorithm_root(&mut child_styles, TaffyContainerKind::Grid);
    let available = TaffySize {
        width: AvailableSpace::Definite(finite_f32(content_width)),
        height: AvailableSpace::MaxContent,
    };
    let mut taffy = TaffyLayoutTree::new(
        |box_idx, known, available| measure_item_with_baseline(session, box_idx, known, available),
        TaffyContainerKind::Grid,
        root_style,
        child_styles,
    );
    taffy.compute(available);
    Some(taffy.root_layout().size.height.max(0.0) as f64)
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
