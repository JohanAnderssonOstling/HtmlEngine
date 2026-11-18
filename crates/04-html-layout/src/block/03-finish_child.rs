//! Advance continuous sibling flow state.
use super::margins::{MarginStrut, remaining_parent_margin, used_after_margin};
use super::{BlockFlowInput, BlockFlowState, LaidOutChild};
use crate::{ContentLayoutResult, ResolvedBoxSizing};
use kurbo::Size;

pub(super) fn finish_child(flow: &mut BlockFlowState, child: LaidOutChild) {
    let LaidOutChild {
        profile,
        runtime_leading_clearance,
        top_collapses_with_parent,
        clearance_on_collapsed_through_box,
        horizontal_margin,
        layout,
    } = child;
    let child_size = layout.size;
    let trailing_margin = if runtime_leading_clearance {
        profile.after
    } else {
        layout.after_margin
    };

    if profile.collapses_through {
        flow.trailing_margin.merge(trailing_margin);
        flow.trailing_margin.merge(profile.before);
    } else {
        flow.trailing_margin = trailing_margin;
    }

    flow.max_width = flow.max_width.max(child_size.width + horizontal_margin);

    if profile.collapses_through {
        if !top_collapses_with_parent {
            flow.pending_margin.merge(profile.after);
        } else {
            flow.collapsed_parent_top_margin.merge(profile.before);
            flow.collapsed_parent_top_margin.merge(profile.after);
        }
        if profile.adjoining_float_sides() != 0 {
            flow.latest_adjoining_float_sides = profile.adjoining_float_sides();
        }
        return;
    }

    flow.y_offset += child_size.height;
    if clearance_on_collapsed_through_box {
        flow.pending_margin.merge(profile.after);
        return;
    }

    flow.pending_margin.merge(trailing_margin);
    flow.clearance_consumed_margin = None;
    flow.started_flow = true;
    flow.collapsed_parent_top_margin = MarginStrut::default();
    flow.latest_adjoining_float_sides = 0;
}

pub(super) fn finish_flow(
    resolved: &ResolvedBoxSizing,
    input: &BlockFlowInput,
    flow: &mut BlockFlowState,
) -> ContentLayoutResult {
    flow.y_offset += remaining_parent_margin(
        flow.pending_margin,
        input.parent_ctx,
        flow.clearance_consumed_margin,
    );
    let size = Size::new(flow.max_width, flow.y_offset);
    let after_margin = if flow.clearance_consumed_margin.is_some() {
        // Clearance consumes this chain into the parent's natural height.
        MarginStrut::from_margin(resolved.resolved_bottom_margin)
    } else {
        used_after_margin(
            resolved,
            input.parent_ctx,
            size.height,
            flow.trailing_margin,
        )
    };
    ContentLayoutResult {
        size,
        after_margin: Some(after_margin),
        ..ContentLayoutResult::default()
    }
}
