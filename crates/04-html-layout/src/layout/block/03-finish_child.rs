//! Advance continuous sibling flow state.
use super::margins::{MarginStrut, remaining_parent_margin, used_after_margin};
use super::{BlockFlowInput, BlockFlowState, LaidOutChild};
use crate::layout::{ContentLayoutResult, LayoutEngine};
use kurbo::Size;

pub(super) fn finish_child(
    _engine: &mut LayoutEngine<'_, '_>,
    _input: &BlockFlowInput,
    flow: &mut BlockFlowState,
    child: LaidOutChild,
) {
    let LaidOutChild {
        child_idx: _,
        profile,
        runtime_leading_clearance,
        top_collapses_with_parent,
        clearance_on_collapsed_through_box,
        point: _,
        horizontal_margin,
        layout,
    } = child;
    let child_size = layout.size;
    let used_after_margin = if runtime_leading_clearance {
        profile.after
    } else {
        layout.after_margin
    };

    if profile.collapses_through {
        flow.trailing_margin.merge(used_after_margin);
        flow.trailing_margin.merge(profile.before);
    } else {
        flow.trailing_margin = used_after_margin;
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

    if clearance_on_collapsed_through_box {
        flow.y_offset += child_size.height;
        flow.pending_margin.merge(profile.after);
        return;
    }

    flow.y_offset += child_size.height;
    flow.pending_margin.merge(used_after_margin);
    flow.clearance_margin_barrier = false;
    flow.clearance_consumed_margin = 0.0;
    flow.started_flow = true;
    flow.collapsed_parent_top_margin = MarginStrut::default();
    flow.latest_adjoining_float_sides = 0;

}

pub(super) fn finish_flow(
    engine: &mut LayoutEngine<'_, '_>,
    input: &BlockFlowInput,
    flow: &mut BlockFlowState,
) -> ContentLayoutResult {
    flow.y_offset += remaining_parent_margin(
        flow.pending_margin,
        input.parent_ctx,
        flow.clearance_margin_barrier,
        flow.clearance_consumed_margin,
    );
    let size = Size::new(flow.max_width, flow.y_offset);
    let after_margin = if flow.clearance_margin_barrier {
        // Clearance consumes this chain into the parent's natural height.
        MarginStrut::from_margin(
            engine
                .reader
                .vertical_margins(input.parent_idx, input.parent_cb_width)
                .1,
        )
    } else {
        used_after_margin(
            &engine.reader,
            input.parent_idx,
            input.parent_cb_width,
            input.parent_content_height,
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
