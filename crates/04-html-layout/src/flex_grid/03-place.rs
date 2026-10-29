//! Execute final child boxes at solver-assigned dimensions.
use super::style::layout_style;
use super::{BaselineItem, FlexGridInput, FlexGridPolicy, TaffyContainerKind};
use crate::layout::{BoxLayoutRequest, LayoutEngine};
use html_style_model::{ItemAlignment, UsedPreferredSize as PreferredSize};
use kurbo::{Point, Size};
use taffy::prelude::Layout;

pub(super) fn place(
    session: &mut LayoutEngine<'_, '_>,
    input: &FlexGridInput<'_>,
    policy: &FlexGridPolicy,
    items: Vec<(Layout, u32)>,
) -> Vec<BaselineItem> {
    let &FlexGridPolicy {
        flex_row,
        parent_align_items,
        child_containing_height,
        ..
    } = policy;
    let mut baseline_items = Vec::new();
    for (layout, child) in items {
        let position = Point::new(
            input.content_origin.x + layout.location.x as f64,
            input.content_origin.y + layout.location.y as f64,
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
            | PreferredSize::Content
            | PreferredSize::MinContent
            | PreferredSize::MaxContent
            | PreferredSize::FitContent => false,
        };
        let assigned_height_is_definite = if input.kind == TaffyContainerKind::Grid {
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
        let result = session.layout_box(BoxLayoutRequest::assigned(
            child as usize,
            input.content_width,
            child_containing_height,
            assigned,
            assigned_height_is_definite,
        ));
        if !has_cross_axis_auto_margin
            && (input.kind == TaffyContainerKind::Grid || flex_row)
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
