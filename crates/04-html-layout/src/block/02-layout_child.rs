//! Own float-fit probes and recursive execution for one child.
use super::float_fitting::fit_beside_floats;
use super::margins::{MarginStrut, collapse_before_child};
use super::{BlockFlowInput, BlockFlowState, LaidOutChild, PreparedChild};
use crate::LayoutEngine;
use html_engine_model::BoxType;
use html_style_model::{Display, PositionMode};
use kurbo::Vec2;

pub(super) fn layout_child(
    engine: &mut LayoutEngine<'_, '_>,
    input: &BlockFlowInput,
    flow: &mut BlockFlowState,
    prepared: PreparedChild,
) -> LaidOutChild {
    let PreparedChild {
        child_idx,
        profile,
        margin_adjoins_parent_top,
        cleared_offset,
        leading_clearance_rewind,
        clearance_on_collapsed_through_box,
    } = prepared;
    let child_style = engine.read.reader.style(child_idx);
    let clearance_chain_leading_margin = if !flow.collapsed_parent_top_margin.is_empty() {
        flow.collapsed_parent_top_margin.resolve()
    } else if profile.own_after.abs() > 0.001 {
        MarginStrut::from_margin(profile.own_before).resolve()
    } else {
        0.0
    };
    if let Some(clearance) = cleared_offset {
        flow.y_offset = clearance.offset;
        flow.pending_margin = MarginStrut::default();
        if clearance_on_collapsed_through_box {
            // The box's adjoining margins still collapse with each other,
            // but its top component lies above the cleared border edge.
            // Carry that consumed component until the chain is resolved.
            let cleared_border_y = input.position.y + clearance.offset;
            flow.clearance_consumed_margin =
                Some(if clearance.hypothetical_y > cleared_border_y + 0.001 {
                    0.0
                } else {
                    clearance_chain_leading_margin
                        - engine.scratch.floats.hypothetical_clearance_rewind()
                });
        }
    }

    let (top_collapses_with_parent, mut margin_offset) = collapse_before_child(
        &mut flow.pending_margin,
        profile,
        margin_adjoins_parent_top && flow.clearance_consumed_margin.is_none(),
        cleared_offset.is_some(),
    );
    if !profile.collapses_through
        && !clearance_on_collapsed_through_box
        && let Some(consumed_margin) = flow.clearance_consumed_margin.take()
    {
        margin_offset -= consumed_margin;
    }
    flow.y_offset += margin_offset;

    // Preserve the child's hypothetical normal-flow margin position for
    // descendant floats. A collapse-through wrapper does not consume the
    // pending adjoining strut into `y_offset`, but floats sourced inside
    // it must still start after that strut. For ordinary children the
    // strut was consumed and cleared above, so this expression reduces to
    // the already-resolved border position.
    engine
        .scratch
        .floats
        .current_float_context_mut()
        .note_source_position(input.position.y + flow.y_offset + flow.pending_margin.resolve());

    let child_indent = engine
        .read
        .reader
        .box_layout_mode(child_idx)
        .map(|mode| calculate_child_indent(mode, &mut flow.indent_remaining))
        .unwrap_or(0.0);
    let mut resolved = engine
        .resolve_box_sizing(input.child_request(child_idx))
        .with_first_line_indent(child_indent);
    // A collapse-through box still has a hypothetical border position.
    // When it establishes a containing block via `position: relative`,
    // that origin includes the adjoining margin even though the margin
    // remains pending in normal  Floats and absolute descendants
    // must therefore agree on the same source position.
    let relative_collapse_through_offset =
        if profile.collapses_through && child_style.position() == PositionMode::Relative {
            flow.pending_margin.resolve()
        } else {
            0.0
        };
    let inline_offset = crate::boxes::block_inline_alignment_offset(engine, &resolved);
    let mut child_pos = input.position
        + Vec2::new(
            inline_offset,
            flow.y_offset + relative_collapse_through_offset,
        );

    if resolved.isolates_floats && !engine.scratch.floats.current_float_context().is_empty() {
        let original_y = child_pos.y;
        (resolved, child_pos) = fit_beside_floats(engine, input, resolved, child_pos, child_indent);
        flow.y_offset += child_pos.y - original_y;
    }
    engine.geometry.set_point(child_idx, child_pos);
    let inherited_hypothetical_margin = (profile.collapses_through
        && !flow.pending_margin.is_empty())
    .then_some(input.position.y + flow.y_offset + flow.pending_margin.resolve());
    if let Some(floor) = inherited_hypothetical_margin {
        engine
            .scratch
            .floats
            .push_hypothetical_clearance_floor(floor);
    }
    if let Some(rewind) = leading_clearance_rewind.filter(|rewind| rewind.abs() > 0.001) {
        engine
            .scratch
            .floats
            .push_hypothetical_clearance_rewind(rewind);
    }
    let child_layout = engine.layout_resolved_box(resolved);
    if engine.read.reader.box_facts(child_idx).display == Display::Table {
        crate::table::align_in_block_flow(engine, &resolved, &child_layout);
    }
    if leading_clearance_rewind.is_some_and(|rewind| rewind.abs() > 0.001) {
        engine.scratch.floats.pop_hypothetical_clearance_rewind();
    }
    if inherited_hypothetical_margin.is_some() {
        engine.scratch.floats.pop_hypothetical_clearance_floor();
    }
    LaidOutChild {
        profile,
        runtime_leading_clearance: leading_clearance_rewind.is_some(),
        top_collapses_with_parent,
        clearance_on_collapsed_through_box,
        horizontal_margin: resolved.horizontal_margins().total,
        layout: child_layout,
    }
}

fn calculate_child_indent(box_type: &BoxType, indent_remaining: &mut f64) -> f64 {
    let indent = std::mem::take(indent_remaining);
    if matches!(box_type, BoxType::Anonymous(_)) {
        indent
    } else {
        0.0
    }
}
