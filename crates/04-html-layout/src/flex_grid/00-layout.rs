//! Pass prepared inputs through solving, child placement, and alignment.
use super::{FlexGridInput, s04_align, s03_place, s01_prepare, s02_solve};
use crate::layout::{ContentLayoutResult, LayoutEngine};

pub(crate) fn layout(
    engine: &mut LayoutEngine<'_, '_>,
    input: FlexGridInput<'_>,
) -> ContentLayoutResult {
    let (policy, prepared) = s01_prepare::prepare(engine, &input);
    let solved = s02_solve::solve(engine, &input, prepared);
    let baseline_items = s03_place::place(engine, &input, &policy, solved.items);
    let size = s04_align::align(
        engine,
        &input,
        policy.size_containment,
        baseline_items,
        solved.size,
    );
    ContentLayoutResult {
        size,
        first_baseline: solved.first_baseline,
        ..ContentLayoutResult::default()
    }
}
