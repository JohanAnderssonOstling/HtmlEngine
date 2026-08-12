use super::LayoutEngine;
use super::placement::PlacementState;
use crate::layout::absolute_positioning::AbsolutePositioningState;
use crate::layout::block::FloatState;
use crate::layout_model::{BoxGeometry, Children, LayoutMode, LayoutState};
use html_style_model::{Float, PositionMode};
use kurbo::{Point, Size};

#[derive(Default)]
struct PassOutput {
    geometry: BoxGeometry,
    state: LayoutState,
    placement: PlacementState,
    line_owners: Vec<u32>,
    block_decoration_owners: Vec<u32>,
    last_inline_fragments: Vec<u32>,
    floats: FloatState,
    absolute_positioning: AbsolutePositioningState,
}

impl PassOutput {
    fn swap_with(&mut self, engine: &mut LayoutEngine<'_, '_>) {
        engine.geometry.swap_storage(&mut self.geometry);
        engine.fragments.swap_scratch(
            &mut self.state,
            &mut self.line_owners,
            &mut self.block_decoration_owners,
            &mut self.last_inline_fragments,
        );
        std::mem::swap(&mut engine.placement, &mut self.placement);
        std::mem::swap(&mut engine.floats, &mut self.floats);
        std::mem::swap(
            &mut engine.absolute_positioning,
            &mut self.absolute_positioning,
        );
    }
}

#[derive(Default)]
pub(super) struct MeasurementScratch {
    output: PassOutput,
}

/// Runs recursive layout against reusable scratch output, restoring the active
/// pass even when the measured formatting context emits lines or fragments.
pub(crate) fn with_isolated_measurement<T>(
    engine: &mut LayoutEngine<'_, '_>,
    measure: impl FnOnce(&mut LayoutEngine<'_, '_>) -> T,
) -> (T, f64) {
    let mut scratch = std::mem::take(&mut engine.measurement);
    scratch.output.swap_with(engine);

    engine.clear_layout_output();
    engine.floats.reset();
    engine.absolute_positioning.clear();
    let measured = measure(engine);
    let float_bottom = engine
        .floats
        .current_float_context()
        .map(crate::layout::FloatContext::max_bottom)
        .unwrap_or(0.0);

    scratch.output.swap_with(engine);
    engine.measurement = scratch;
    (measured, float_bottom)
}

/// Measures a box in disposable output state so intrinsic measurement cannot
/// publish geometry or fragments into the active layout pass.
pub(crate) fn measure_box_isolated(
    session: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: f64,
    parent_height: Option<f64>,
) -> Size {
    let (measured, float_bottom) = with_isolated_measurement(session, |session| {
        session.geometry.set_point(box_idx, Point::ZERO);
        session.layout_box(crate::layout::BoxLayoutRequest::normal(
            box_idx,
            available_width.max(0.0),
            parent_height,
        ))
    });
    let mut measured = measured.size;
    measured.height = measured.height.max(float_bottom);
    measured
}

/// Measures the natural border-box block size without allowing authored
/// height/min/max-height to turn the probe into a specified-size measurement.
/// Flexbox uses this for a non-replaced item's content-size suggestion.
pub(crate) fn measure_intrinsic_block_size_isolated(
    session: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: f64,
) -> Size {
    let (measured, float_bottom) = with_isolated_measurement(session, |session| {
        session.geometry.set_point(box_idx, Point::ZERO);
        session.layout_box(
            crate::layout::BoxLayoutRequest::normal(box_idx, available_width.max(0.0), None)
                .for_intrinsic_block_measurement(),
        )
    });
    let mut measured = measured.size;
    measured.height = measured.height.max(float_bottom);
    measured
}

/// Measures an already-resolved box without repeating style, intrinsic, or
/// replaced-size resolution in the disposable pass.
pub(crate) fn measure_resolved_box_isolated(
    session: &mut LayoutEngine<'_, '_>,
    resolved: super::box_sizing::ResolvedBoxSizing,
) -> Size {
    let box_idx = resolved.box_idx();
    let (measured, float_bottom) = with_isolated_measurement(session, |session| {
        session.geometry.set_point(box_idx, Point::ZERO);
        session.layout_resolved_box(resolved)
    });
    let mut measured = measured.size;
    measured.height = measured.height.max(float_bottom);
    measured
}

/// Measures an assigned-width atomic box together with the first and last
/// in-flow line baselines it publishes. Inline tables use the first baseline;
/// inline-blocks use the last when overflow remains visible.
pub(crate) fn measure_box_width_and_baselines_isolated(
    session: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    containing_width: f64,
    assigned_width: f64,
    parent_content_height: Option<f64>,
) -> (Size, Option<f64>, Option<f64>) {
    measure_box_with_baselines_isolated(
        session,
        box_idx,
        crate::layout::BoxLayoutRequest::width_assigned(
            box_idx,
            containing_width.max(0.0),
            assigned_width.max(0.0),
            parent_content_height,
        ),
    )
}

/// Measures an atomic box using its normal formatting-context sizing while
/// retaining the baselines needed by inline placement.
pub(crate) fn measure_box_and_baselines_isolated(
    session: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    available_width: f64,
    parent_content_height: Option<f64>,
) -> (Size, Option<f64>, Option<f64>) {
    measure_box_with_baselines_isolated(
        session,
        box_idx,
        crate::layout::BoxLayoutRequest::normal(
            box_idx,
            available_width.max(0.0),
            parent_content_height,
        ),
    )
}

fn measure_box_with_baselines_isolated(
    session: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    request: crate::layout::BoxLayoutRequest,
) -> (Size, Option<f64>, Option<f64>) {
    with_isolated_measurement(session, |session| {
        session.geometry.set_point(box_idx, Point::ZERO);
        let layout = session.layout_box(request);
        let first_baseline = session
            .fragments
            .first_baseline_offset(layout.output.lines.clone(), 0.0)
            .or_else(|| session.flex_grid.container_first_baseline(box_idx))
            .or_else(|| {
                // An inline-table exposes the first row baseline. If that row has
                // no in-flow line box, CSS synthesizes it at the row's bottom
                // content edge. Capture it while isolated table geometry is still
                // authoritative; the scratch swap below deliberately discards it.
                let crate::layout_model::LayoutMode::Table(table) =
                    session.reader.box_layout_mode(box_idx)?
                else {
                    return None;
                };
                let row_idx = *table.rows.first()? as usize;
                let table_y = session.geometry.point(box_idx).y;
                let row_point = session.geometry.point(row_idx);
                let row_size = session.geometry.size(row_idx);
                Some(row_point.y + row_size.height - table_y)
            });
        let last_baseline = session
            .fragments
            .last_baseline_offset(layout.output.lines, 0.0)
            .or_else(|| last_in_flow_block_child_bottom(session, box_idx));
        (layout.size, first_baseline, last_baseline)
    })
    .0
}

/// An empty terminal block child publishes no line fragment, but it still
/// supplies the synthesized baseline used by an enclosing inline-block. The
/// baseline is its bottom border edge; padding after the child belongs to the
/// enclosing box and must not move it.
fn last_in_flow_block_child_bottom(session: &LayoutEngine<'_, '_>, box_idx: usize) -> Option<f64> {
    let LayoutMode::Block(block) = session.reader.box_layout_mode(box_idx)? else {
        return None;
    };
    let Children::Blocks(children) = &block.children else {
        return None;
    };
    let child_idx = children
        .iter()
        .rev()
        .map(|child| *child as usize)
        .find(|child| {
            let style = session.reader.style(*child);
            style.position() != PositionMode::Absolute && style.float() == Float::None
        })?;
    let parent_y = session.geometry.point(box_idx).y;
    let child_point = session.geometry.point(child_idx);
    let child_size = session.geometry.size(child_idx);
    Some(child_point.y + child_size.height - parent_y)
}
