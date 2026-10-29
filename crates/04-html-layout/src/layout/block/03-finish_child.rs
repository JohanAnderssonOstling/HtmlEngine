//! Apply fragmentation and advance sibling flow state.
use super::fragmentation::{forced_break_offset, offset_after_layout};
use super::margins::{MarginStrut, remaining_parent_margin, used_after_margin};
use super::{BlockFlowInput, BlockFlowState, LaidOutChild, PreviousBlock};
use crate::layout::{ContentLayoutResult, LayoutEngine};
use kurbo::Size;

pub(super) fn finish_child(
    engine: &mut LayoutEngine<'_, '_>,
    input: &BlockFlowInput,
    flow: &mut BlockFlowState,
    child: LaidOutChild,
) {
    let LaidOutChild {
        child_idx,
        profile,
        runtime_leading_clearance,
        top_collapses_with_parent,
        clearance_on_collapsed_through_box,
        point: mut child_pos,
        horizontal_margin,
        layout,
    } = child;
    let child_style = engine.reader.style(child_idx);
    let child_size = layout.size;
    let used_after_margin = if runtime_leading_clearance {
        profile.after
    } else {
        layout.after_margin
    };
    let output = layout.output;

    if profile.collapses_through {
        flow.trailing_margin.merge(used_after_margin);
        flow.trailing_margin.merge(profile.before);
    } else {
        flow.trailing_margin = used_after_margin;
    }

    flow.y_offset += offset_after_layout(
        engine,
        child_idx,
        child_size,
        &output,
        flow.previous_block.as_ref(),
        input.fragment_height,
        &mut child_pos,
    );

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
    flow.y_offset += forced_break_offset(
        child_style.break_after(),
        input.position.y + flow.y_offset,
        input.fragment_height,
    );
    flow.pending_margin.merge(used_after_margin);
    flow.clearance_margin_barrier = false;
    flow.clearance_consumed_margin = 0.0;
    flow.started_flow = true;
    flow.collapsed_parent_top_margin = MarginStrut::default();
    flow.latest_adjoining_float_sides = 0;
    flow.previous_block = input.fragment_height.map(|_| PreviousBlock {
        top: child_pos.y,
        placement: output.placement.box_group,
        break_after: child_style.break_after(),
    });
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
