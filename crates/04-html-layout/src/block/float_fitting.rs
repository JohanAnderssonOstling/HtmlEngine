use super::BlockFlowInput;
use crate::measurement::measure_resolved_box_isolated;
use crate::{LayoutEngine, ResolvedBoxSizing};
use kurbo::Point;

/// Measurement uses isolated scratch, so each query can borrow the active float
/// context again instead of cloning its band vector across recursive probes.
pub(super) fn fit_beside_floats(
    engine: &mut LayoutEngine<'_, '_>,
    input: &BlockFlowInput,
    resolved: ResolvedBoxSizing,
    point: Point,
    child_indent: f64,
) -> (ResolvedBoxSizing, Point) {
    let container = input.position.x..input.position.x + input.available_width;
    let original_y = point.y;
    let horizontal = resolved.horizontal_margins();
    let mut candidate_y = original_y;
    let mut probe_height = measure_resolved_box_isolated(engine, resolved)
        .size_with_floats()
        .height
        .max(1.0);
    for _ in 0..engine.scratch.floats.current_float_context().band_count() + 2 {
        let (band_left, band_right) = engine.scratch.floats.current_float_context().available(
            candidate_y,
            probe_height,
            container.clone(),
        );
        let band_width = (band_right - band_left).max(0.0);
        let candidate = engine
            .resolve_box_sizing(
                input
                    .child_request(resolved.box_idx())
                    .with_auto_width_limit(band_width),
            )
            .with_first_line_indent(child_indent);
        let candidate_horizontal = candidate.horizontal_margins();
        let measured = measure_resolved_box_isolated(engine, candidate).size_with_floats();
        let candidate_height = measured.height.max(1.0);
        let (stable_left, stable_right) = engine.scratch.floats.current_float_context().available(
            candidate_y,
            candidate_height,
            container.clone(),
        );
        if (stable_left - band_left).abs() > 0.001 || (stable_right - band_right).abs() > 0.001 {
            probe_height = candidate_height;
            continue;
        }

        // The margin-box edge adjoining a float must stay outside that
        // float. A trailing margin at the containing block edge may
        // overflow that edge, though, and must not force the BFC below
        // an otherwise fitting float (CSS2 new-fc-beside-float-with-margin).
        let trailing_float_margin = if stable_right < container.end - 0.001 {
            candidate_horizontal.total - candidate_horizontal.left
        } else {
            0.0
        };
        let required_width =
            (measured.width + candidate_horizontal.left + trailing_float_margin).max(0.0);
        let stable_width = (stable_right - stable_left).max(0.0);
        let next_y =
            if stable_left <= stable_right + 0.001 && required_width <= stable_width + 0.001 {
                candidate_y
            } else {
                engine
                    .scratch
                    .floats
                    .current_float_context()
                    .next_y_fitting(
                        candidate_y,
                        required_width,
                        candidate_height,
                        container.clone(),
                    )
            };
        if next_y <= candidate_y + 0.001 {
            return (
                candidate,
                Point::new(stable_left + candidate_horizontal.left, candidate_y),
            );
        }
        candidate_y = next_y;
        probe_height = candidate_height;
    }
    let measured = measure_resolved_box_isolated(engine, resolved).size_with_floats();
    let placement = engine
        .scratch
        .floats
        .current_float_context()
        .place_margin_box(
            original_y,
            measured.width + horizontal.total,
            measured.height.max(1.0),
            container.clone(),
        );
    (
        resolved,
        Point::new(placement.left + horizontal.left, placement.y),
    )
}
