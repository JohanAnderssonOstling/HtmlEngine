//! Classify one child and derive its margin/clearance requirements.
use super::floats::clearance_offset;
use super::margins::{FlowParticipation, MarginStrut, clear_matches_float_sides, margin_profile};
use super::{BlockFlowInput, BlockFlowState, PreparedChild};
use crate::LayoutEngine;
use html_engine_model::BoxType;
use html_style_model::{Clear, Float};

pub(super) fn prepare_child(
    engine: &mut LayoutEngine<'_, '_>,
    input: &BlockFlowInput,
    flow: &BlockFlowState,
    child_idx: usize,
) -> PreparedChild {
    let is_anonymous = matches!(
        engine.read.reader.box_layout_mode(child_idx),
        Some(BoxType::Anonymous(_))
    );
    let margin_adjoins_parent_top =
        !flow.started_flow && input.parent_ctx.top_open && !is_anonymous;
    let mut profile = margin_profile(
        engine,
        child_idx,
        (input.available_width, input.parent_content_height),
    );
    let child_style = engine.read.reader.style(child_idx);
    let child_float = engine.read.reader.box_facts(child_idx).float;
    let child_clear = child_style.clear();
    debug_assert_eq!(
        matches!(profile.participation, FlowParticipation::Float),
        matches!(child_float, Float::Left | Float::Right)
    );

    // A descendant clear behind open before edges exposes a different
    // margin when a matching float is active. Leave the clear candidate's
    // own margin inside the child so its runtime clearance contributes to
    // the child's used height instead of shifting the whole child.
    let leading_clear = profile.leading_clear();
    let mut leading_clearance_rewind = None;
    if child_clear == Clear::None && leading_clear != Clear::None {
        let leading_clearance = clearance_offset(
            &engine.scratch.floats,
            input.position.y,
            flow.y_offset,
            flow.pending_margin,
            profile.before,
            0.0,
            leading_clear,
            margin_adjoins_parent_top,
            clear_matches_float_sides(leading_clear, flow.latest_adjoining_float_sides),
        );
        if leading_clearance.is_some() {
            // Keep the adjoining margins that occur before the clear
            // candidate exposed at the wrapper's open top edge. The
            // candidate's own top margin remains inside: clearance is
            // inserted between that margin and the preceding margin
            // chain (CSS 2.1 8.3.1 and 9.5.2).
            leading_clearance_rewind = Some(
                profile.before_clear.resolve()
                    - MarginStrut::from_margin(profile.own_before).resolve(),
            );
            profile.before = profile.before_clear;
            // The cached profile describes the no-clearance case. Once a
            // leading descendant actually clears an external float this
            // wrapper acquires used height and can no longer collapse
            // through as a zero-height margin-only box.
            profile.collapses_through = false;
            profile.after = MarginStrut::from_margin(profile.own_after);
        }
    }

    // A leading BFC whose margin remains adjoining to an earlier nested
    // float behaves like it has clearance when its border box cannot fit
    // in any band beside that float. Resolve the propagated candidate at
    // the real containing width before allowing its margin to escape
    // through the wrapper.
    let has_active_float = !engine.scratch.floats.current_float_context().is_empty();
    if let Some(leading_bfc_idx) = profile
        .leading_bfc_to_probe(has_active_float)
        .filter(|leading_bfc_idx| *leading_bfc_idx != child_idx)
    {
        let candidate = engine.resolve_box_sizing(input.child_request(leading_bfc_idx));
        let horizontal = candidate.horizontal_margins();
        let measured =
            crate::measurement::measure_resolved_box_isolated(engine, candidate).size_with_floats();
        if measured.width + horizontal.total >= input.available_width - 0.001 {
            profile.before = profile.before_clear;
            profile.collapses_through = false;
        }
    }

    // A self-collapsing box exposes its combined top/bottom strut to
    // siblings, but clearance is calculated at its top border edge. Its
    // bottom margin must therefore not move the hypothetical clear
    // position.
    let clearance_before = if child_clear != Clear::None && profile.collapses_through {
        MarginStrut::from_margin(profile.own_before)
    } else {
        profile.before
    };
    let cleared_offset = clearance_offset(
        &engine.scratch.floats,
        input.position.y,
        flow.y_offset,
        flow.pending_margin,
        clearance_before,
        profile.own_before,
        child_clear,
        margin_adjoins_parent_top,
        clear_matches_float_sides(child_clear, flow.latest_adjoining_float_sides),
    );

    // Finish the effective profile here; execution only consumes the decision.
    // Floats use clearance for positioning without consuming sibling margins.
    let clearance_on_collapsed_through_box = cleared_offset.is_some()
        && profile.collapses_through
        && !matches!(profile.participation, FlowParticipation::Float);
    if clearance_on_collapsed_through_box {
        profile.before = MarginStrut::from_margin(profile.own_before);
        profile.after = MarginStrut::from_margin(profile.own_after);
        profile.collapses_through = false;
    }

    PreparedChild {
        child_idx,
        profile,
        margin_adjoins_parent_top,
        cleared_offset,
        leading_clearance_rewind,
        clearance_on_collapsed_through_box,
    }
}
