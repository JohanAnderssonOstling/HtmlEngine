//! Classify one child and derive its margin/clearance requirements.
use super::floats::clearance_offset;
use super::margins::{FlowParticipation, MarginStrut, clear_matches_float_sides};
use super::{BlockFlowInput, BlockFlowState, PreparedChild};
use crate::layout::{BoxLayoutRequest, LayoutEngine};
use crate::layout_model::BoxType;
use html_style_model::{Float, PositionMode};
use kurbo::Vec2;

pub(super) fn prepare_child(
    engine: &mut LayoutEngine<'_, '_>,
    input: &BlockFlowInput,
    flow: &mut BlockFlowState,
    child_idx: usize,
) -> Option<PreparedChild> {
    if engine.reader.style(child_idx).position() == PositionMode::Absolute {
        // The hypothetical static position lies after the adjoining
        // margin accumulated from preceding in-flow content. Resolving it
        // here does not consume that margin; the next in-flow sibling
        // still collapses against the same strut.
        engine.defer_absolute_box(
            child_idx,
            input.position + Vec2::new(0.0, flow.y_offset + flow.pending_margin.resolve()),
        );
        return None;
    }
    if engine.reader.is_inline_box(child_idx) {
        return None;
    }

    let is_anonymous = matches!(
        engine.reader.box_layout_mode(child_idx),
        Some(BoxType::Anonymous(_))
    );
    let mut profile =
        engine
            .margins
            .margin_profile(&engine.reader, child_idx, input.available_width);
    let child_style = engine.reader.style(child_idx);
    let child_float = child_style.float();
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
    let mut runtime_leading_clearance = false;
    let mut runtime_leading_clearance_rewind = 0.0;
    if child_clear == html_style_model::Clear::None
        && leading_clear != html_style_model::Clear::None
    {
        let leading_clearance = clearance_offset(
            &engine.floats,
            input.position.y,
            flow.y_offset,
            flow.pending_margin,
            profile.before,
            0.0,
            leading_clear,
            !flow.started_flow && input.parent_ctx.top_open && !is_anonymous,
            clear_matches_float_sides(leading_clear, flow.latest_adjoining_float_sides),
        );
        if leading_clearance.is_some() {
            // Keep the adjoining margins that occur before the clear
            // candidate exposed at the wrapper's open top edge. The
            // candidate's own top margin remains inside: clearance is
            // inserted between that margin and the preceding margin
            // chain (CSS 2.1 8.3.1 and 9.5.2).
            runtime_leading_clearance_rewind = profile.before_clear.resolve()
                - MarginStrut::from_margin(profile.own_before).resolve();
            profile.before = profile.before_clear;
            // The cached profile describes the no-clearance case. Once a
            // leading descendant actually clears an external float this
            // wrapper acquires used height and can no longer collapse
            // through as a zero-height margin-only box.
            profile.collapses_through = false;
            profile.after = MarginStrut::from_margin(profile.own_after);
            runtime_leading_clearance = true;
        }
    }

    // A leading BFC whose margin remains adjoining to an earlier nested
    // float behaves like it has clearance when its border box cannot fit
    // in any band beside that float. Resolve the propagated candidate at
    // the real containing width before allowing its margin to escape
    // through the wrapper.
    let has_active_float = engine
        .floats
        .current_float_context()
        .is_some_and(|context| !context.is_empty());
    if let Some(leading_bfc_idx) = profile
        .leading_bfc_to_probe(has_active_float)
        .filter(|leading_bfc_idx| *leading_bfc_idx != child_idx)
    {
        let candidate = engine.resolve_box_sizing(BoxLayoutRequest::normal(
            leading_bfc_idx,
            input.available_width,
            input.parent_content_height,
        ));
        let horizontal = candidate.horizontal_margins();
        let measured =
            crate::layout::measure_resolved_box_isolated(engine, candidate).size_with_floats();
        if measured.width + horizontal.total >= input.available_width - 0.001 {
            profile.before = profile.before_clear;
            profile.collapses_through = false;
        }
    }

    let margin_adjoins_parent_top =
        !flow.started_flow && input.parent_ctx.top_open && !is_anonymous;
    // A self-collapsing box exposes its combined top/bottom strut to
    // siblings, but clearance is calculated at its top border edge. Its
    // bottom margin must therefore not move the hypothetical clear
    // position.
    let clearance_before =
        if child_clear != html_style_model::Clear::None && profile.collapses_through {
            MarginStrut::from_margin(profile.own_before)
        } else {
            profile.before
        };
    let cleared_offset = clearance_offset(
        &engine.floats,
        input.position.y,
        flow.y_offset,
        flow.pending_margin,
        clearance_before,
        profile.own_before,
        child_clear,
        margin_adjoins_parent_top,
        clear_matches_float_sides(child_clear, flow.latest_adjoining_float_sides),
    );

    Some(PreparedChild {
        child_idx,
        profile,
        is_anonymous,
        cleared_offset,
        runtime_leading_clearance,
        runtime_leading_clearance_rewind,
    })
}
