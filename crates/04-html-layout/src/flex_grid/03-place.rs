//! Execute final child boxes at solver-assigned dimensions.
use super::sizing::Axis;
use super::{BaselineItem, FlexGridInput, FlexGridPolicy, TaffyContainerKind, nonnegative_size};
use crate::{BoxLayoutRequest, LayoutEngine};
use html_style_model::{ItemAlignment, UsedPreferredSize as PreferredSize};
use kurbo::Point;
use taffy::prelude::Layout;

pub(super) fn place(
    engine: &mut LayoutEngine<'_, '_>,
    input: &FlexGridInput<'_>,
    policy: &FlexGridPolicy,
    items: Vec<(Layout, u32)>,
) -> Vec<BaselineItem> {
    let mut baseline_items = Vec::new();
    for (layout, child) in items {
        let position = Point::new(
            input.content_origin.x + layout.location.x as f64,
            input.content_origin.y + layout.location.y as f64,
        );
        engine.geometry.set_point(child as usize, position);
        let assigned = nonnegative_size(layout.size);
        let child_layout = engine.read.reader.layout_style(child as usize);
        let effective_cross_alignment = input.container.alignment(child_layout);
        let has_cross_axis_auto_margin = Axis::Vertical.auto_margin(child_layout);
        let authored_height_is_definite = match engine.read.reader.style(child as usize).height() {
            PreferredSize::Px(_) => true,
            PreferredSize::Percent(_)
            | PreferredSize::Stretch
            | PreferredSize::Calc { .. }
            | PreferredSize::Comparison { .. } => policy.child_containing_height.is_some(),
            PreferredSize::Auto
            | PreferredSize::Content
            | PreferredSize::MinContent
            | PreferredSize::MaxContent
            | PreferredSize::FitContent => false,
        };
        let assigned_height_is_definite = if input.container.kind == TaffyContainerKind::Grid {
            true
        } else if input.container.flex_row() {
            authored_height_is_definite || input.container.cross_axis_stretches(child_layout)
        } else {
            authored_height_is_definite || policy.child_containing_height.is_some()
        };
        let result = engine.layout_box(BoxLayoutRequest::assigned(
            child as usize,
            input.content_width,
            policy.child_containing_height,
            assigned,
            assigned_height_is_definite,
        ));
        if !has_cross_axis_auto_margin
            && (input.container.kind == TaffyContainerKind::Grid || input.container.flex_row())
            && effective_cross_alignment == ItemAlignment::Baseline
        {
            baseline_items.push(BaselineItem {
                original_y: position.y,
                height: assigned.height,
                baseline: result.first_baseline.unwrap_or(assigned.height),
                placement: result.output.placement.box_group,
            });
        }
    }

    baseline_items
}
