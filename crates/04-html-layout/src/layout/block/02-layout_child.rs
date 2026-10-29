//! Own float-fit probes and recursive execution for one child.
use super::floats::layout_float;
use super::fragmentation::forced_break_offset;
use super::margins::{FlowParticipation, MarginStrut, collapse_before_child};
use super::{BlockFlowInput, BlockFlowState, LaidOutChild, PreparedChild};
use crate::layout::{BoxLayoutRequest, LayoutEngine, measure_resolved_box_isolated};
use crate::layout_model::BoxType;
use html_style_model::PositionMode;
use kurbo::{Insets, Point, Size, Vec2};

pub(super) fn layout_child(
    engine: &mut LayoutEngine<'_, '_>,
    input: &BlockFlowInput,
    flow: &mut BlockFlowState,
    prepared: PreparedChild,
) -> Option<LaidOutChild> {
    let PreparedChild {
        child_idx,
        mut profile,
        is_anonymous,
        cleared_offset,
        runtime_leading_clearance,
        runtime_leading_clearance_rewind,
    } = prepared;
    let child_style = engine.reader.style(child_idx);
    if matches!(profile.participation, FlowParticipation::Float) {
        // `clear` changes this float's hypothetical position, but a
        // float remains out of flow: its clearance must not advance the
        // parent's normal-flow cursor or consume the adjoining margin
        // that a later in-flow sibling still sees.
        let float_margin_y = cleared_offset
            .map(|clearance| clearance.offset)
            .unwrap_or_else(|| flow.y_offset + flow.pending_margin.resolve());
        let resolved = engine.resolve_box_sizing(BoxLayoutRequest::normal(
            child_idx,
            input.available_width,
            input.parent_content_height,
        ));
        let horizontal = resolved.horizontal_margins();
        let child_size = layout_float(
            engine,
            resolved,
            input.position + Vec2::new(0.0, float_margin_y),
            Insets::new(
                horizontal.left,
                profile.own_before,
                horizontal.total - horizontal.left,
                profile.own_after,
            ),
            Size::ZERO,
        );
        flow.max_width = flow.max_width.max(child_size.width + horizontal.total);
        flow.latest_adjoining_float_sides = profile.adjoining_float_sides();
        return None;
    }

    let mut has_clearance = false;
    let mut clearance_consumed_before_margin = false;
    let clearance_on_collapsed_through_box = cleared_offset.is_some() && profile.collapses_through;
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
        has_clearance = true;
        clearance_consumed_before_margin = true;
        if clearance_on_collapsed_through_box {
            // The box's adjoining margins still collapse with each other,
            // but its top component lies above the cleared border edge.
            // Carry that consumed component until the chain is resolved.
            flow.clearance_margin_barrier = true;
            let cleared_border_y = input.position.y + clearance.offset;
            flow.clearance_consumed_margin = if clearance.hypothetical_y > cleared_border_y + 0.001
            {
                0.0
            } else {
                clearance_chain_leading_margin - engine.floats.hypothetical_clearance_rewind()
            };
            profile.before = MarginStrut::from_margin(profile.own_before);
            profile.after = MarginStrut::from_margin(profile.own_after);
            profile.collapses_through = false;
        }
    }

    let (top_collapses_with_parent, mut margin_offset) = collapse_before_child(
        &mut flow.pending_margin,
        profile,
        flow.started_flow || flow.clearance_margin_barrier,
        has_clearance,
        clearance_consumed_before_margin,
        input.parent_ctx,
        is_anonymous,
    );
    if flow.clearance_margin_barrier
        && !profile.collapses_through
        && !clearance_on_collapsed_through_box
    {
        margin_offset -= flow.clearance_consumed_margin;
        flow.clearance_margin_barrier = false;
        flow.clearance_consumed_margin = 0.0;
    }
    flow.y_offset += margin_offset;

    // Preserve the child's hypothetical normal-flow margin position for
    // descendant floats. A collapse-through wrapper does not consume the
    // pending adjoining strut into `y_offset`, but floats sourced inside
    // it must still start after that strut. For ordinary children the
    // strut was consumed and cleared above, so this expression reduces to
    // the already-resolved border position.
    if let Some(context) = engine.floats.current_float_context_mut() {
        context
            .note_source_position(input.position.y + flow.y_offset + flow.pending_margin.resolve());
    }

    flow.y_offset += forced_break_offset(
        child_style.break_before(),
        input.position.y + flow.y_offset,
        input.fragment_height,
    );

    let child_indent = engine
        .reader
        .box_layout_mode(child_idx)
        .map(|mode| calculate_child_indent(mode, &mut flow.indent_remaining))
        .unwrap_or(0.0);
    let mut resolved = engine
        .resolve_box_sizing(BoxLayoutRequest::normal(
            child_idx,
            input.available_width,
            input.parent_content_height,
        ))
        .with_first_line_indent(child_indent);
    let mut horizontal = resolved.horizontal_margins();
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
    let inline_offset = crate::layout::box_sizing::block_inline_alignment_offset(engine, &resolved);
    let mut child_pos = input.position
        + Vec2::new(
            inline_offset,
            flow.y_offset + relative_collapse_through_offset,
        );
    if engine.reader.box_uses_float_context(child_idx)
        && let Some(float_context) = engine
            .floats
            .current_float_context()
            .filter(|context| !context.is_empty())
            .cloned()
    {
        let original_y = child_pos.y;
        let mut candidate_y = original_y;
        let mut probe_height = measure_resolved_box_isolated(engine, resolved)
            .size_with_floats()
            .height
            .max(1.0);
        let mut selected = false;
        for _ in 0..float_context.band_count() + 2 {
            let (band_left, band_right) = float_context.available(
                candidate_y,
                probe_height,
                input.position.x,
                input.position.x + input.available_width,
            );
            let band_width = (band_right - band_left).max(0.0);
            let candidate = engine
                .resolve_box_sizing(
                    BoxLayoutRequest::normal(
                        child_idx,
                        input.available_width,
                        input.parent_content_height,
                    )
                    .with_auto_width_limit(band_width),
                )
                .with_first_line_indent(child_indent);
            let candidate_horizontal = candidate.horizontal_margins();
            let measured = measure_resolved_box_isolated(engine, candidate).size_with_floats();
            let candidate_height = measured.height.max(1.0);
            let (stable_left, stable_right) = float_context.available(
                candidate_y,
                candidate_height,
                input.position.x,
                input.position.x + input.available_width,
            );
            if (stable_left - band_left).abs() > 0.001 || (stable_right - band_right).abs() > 0.001
            {
                probe_height = candidate_height;
                continue;
            }

            // The margin-box edge adjoining a float must stay outside that
            // float. A trailing margin at the containing block edge may
            // overflow that edge, though, and must not force the BFC below
            // an otherwise fitting float (CSS2 new-fc-beside-float-with-margin).
            let trailing_float_margin =
                if stable_right < input.position.x + input.available_width - 0.001 {
                    candidate_horizontal.total - candidate_horizontal.left
                } else {
                    0.0
                };
            let required_width =
                (measured.width + candidate_horizontal.left + trailing_float_margin).max(0.0);
            let stable_width = (stable_right - stable_left).max(0.0);
            if stable_left <= stable_right + 0.001 && required_width <= stable_width + 0.001 {
                resolved = candidate;
                horizontal = candidate_horizontal;
                child_pos = Point::new(stable_left + horizontal.left, candidate_y);
                selected = true;
                break;
            }

            let next_y = float_context.next_y_fitting(
                candidate_y,
                required_width,
                candidate_height,
                input.position.x,
                input.position.x + input.available_width,
            );
            if next_y <= candidate_y + 0.001 {
                resolved = candidate;
                horizontal = candidate_horizontal;
                child_pos = Point::new(stable_left + horizontal.left, candidate_y);
                selected = true;
                break;
            }
            candidate_y = next_y;
            probe_height = candidate_height;
        }
        if !selected {
            let measured = measure_resolved_box_isolated(engine, resolved).size_with_floats();
            let placement = float_context.place_margin_box(
                original_y,
                measured.width + horizontal.total,
                measured.height.max(1.0),
                input.position.x,
                input.position.x + input.available_width,
            );
            child_pos = Point::new(placement.left + horizontal.left, placement.y);
        }
        flow.y_offset += child_pos.y - original_y;
    }
    engine.geometry.set_point(child_idx, child_pos);
    let inherited_hypothetical_margin = (profile.collapses_through
        && !flow.pending_margin.is_empty())
    .then_some(input.position.y + flow.y_offset + flow.pending_margin.resolve());
    if let Some(floor) = inherited_hypothetical_margin {
        engine.floats.push_hypothetical_clearance_floor(floor);
    }
    if runtime_leading_clearance_rewind.abs() > 0.001 {
        engine
            .floats
            .push_hypothetical_clearance_rewind(runtime_leading_clearance_rewind);
    }
    let child_layout = engine.layout_resolved_box(resolved);
    if runtime_leading_clearance_rewind.abs() > 0.001 {
        engine.floats.pop_hypothetical_clearance_rewind();
    }
    if inherited_hypothetical_margin.is_some() {
        engine.floats.pop_hypothetical_clearance_floor();
    }
    Some(LaidOutChild {
        child_idx,
        profile,
        runtime_leading_clearance,
        top_collapses_with_parent,
        clearance_on_collapsed_through_box,
        point: child_pos,
        horizontal_margin: horizontal.total,
        layout: child_layout,
    })
}

fn calculate_child_indent(box_type: &BoxType, indent_remaining: &mut f64) -> f64 {
    match box_type {
        BoxType::Anonymous(_) => {
            let indent = *indent_remaining;
            *indent_remaining = 0.0;
            indent
        }
        _ => {
            *indent_remaining = 0.0;
            0.0
        }
    }
}
