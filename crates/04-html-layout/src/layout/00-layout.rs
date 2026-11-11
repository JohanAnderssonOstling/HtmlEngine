//! Coordinate preparation, recursive geometry, finalization, and paint replay.
use html_engine_model::{BoxGeometry, LayoutConstraints, LayoutState};
use super::FragmentWriter;
use super::engine::{LayoutEngine, LayoutInputs, LayoutOutputs, LayoutPassKind, LayoutReadContext};
use super::s03_finalize::FinalizationScratch;
use super::read_context::{PassContext, LayoutReader};

pub fn compute_layout(
    inputs: LayoutInputs<'_>,
    outputs: LayoutOutputs<'_>,
    constraints: LayoutConstraints,
) {
    let pass_context = PassContext::new(&inputs);
    let paint = inputs.paint;
    let read = LayoutReadContext::new(inputs, constraints, &pass_context);
    let mut engine = LayoutEngine::new(read, outputs, LayoutPassKind::Final);
    engine.prepare_layout_pass();
    engine.layout_root();
    engine.finalize();
    let reader = engine.reader.with_paint(paint);
    super::s04_paint::replay(
        &reader,
        engine.text.content(),
        engine.geometry,
        &mut engine.fragments,
        &mut engine.scratch.finalization,
    );
    engine.finish_pass();
}

/// Paint-only updates retain all box and line geometry.
pub fn repaint(inputs: LayoutInputs<'_>, geometry: &BoxGeometry, state: &mut LayoutState) {
    let pass_context = PassContext::for_paint(&inputs);
    let reader = LayoutReader::new(&inputs, &pass_context).with_paint(inputs.paint);
    let mut fragments = FragmentWriter::new(state);
    super::s04_paint::replay(
        &reader,
        inputs.inline_content,
        geometry,
        &mut fragments,
        &mut FinalizationScratch::default(),
    );
}
