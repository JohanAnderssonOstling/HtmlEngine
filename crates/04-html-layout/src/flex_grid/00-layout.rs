//! Pass prepared inputs through solving, child placement, and alignment.
use super::{FlexGridInput, TaffyContainerKind, alignment, placement, preparation, solver};
use crate::layout::{ContentLayoutResult, LayoutEngine};
use kurbo::Point;

pub fn layout(
    session: &mut LayoutEngine<'_, '_>,
    container_idx: usize,
    children: &[u32],
    content_origin: Point,
    content_width: f64,
    explicit_height: Option<f64>,
    min_height: Option<f64>,
    max_height: Option<f64>,
    kind: TaffyContainerKind,
) -> ContentLayoutResult {
    let started = session.start_timing();
    let input = FlexGridInput {
        container_idx,
        children,
        content_origin,
        content_width,
        explicit_height,
        min_height,
        max_height,
        kind,
    };
    let (policy, prepared) = preparation::prepare(session, &input);
    let solved = solver::solve(session, &input, prepared);
    let baseline_items = placement::place(session, &input, &policy, solved.items);
    let size = alignment::align(
        session,
        &input,
        policy.size_containment,
        baseline_items,
        solved.size,
    );
    session.record_timing(|t| t.layout_flex_grid += started.elapsed());
    ContentLayoutResult {
        size,
        first_baseline: solved.first_baseline,
        ..ContentLayoutResult::default()
    }
}
