//! Coordinate preparation, recursive geometry, finalization, and paint replay.
use super::FragmentWriter;
use super::engine::{LayoutInputs, LayoutOutputs, LayoutTimings};
use super::finalization::FinalizationScratch;
use super::preparation::with_engine;
use super::read_context::{BoxStyleSnapshot, LayoutReader};
use crate::layout_model::{BoxGeometry, LayoutState};

pub fn layout_with_timings(
    inputs: LayoutInputs<'_>,
    outputs: LayoutOutputs<'_, '_>,
    constraints: crate::LayoutConstraints,
    timings: Option<&mut LayoutTimings>,
) {
    profile_scope!("layout.engine");
    with_engine(inputs, outputs, constraints, timings, |engine, paint| {
        engine.layout_root();
        engine.finalize();
        let reader = engine.reader.with_paint(paint);
        let started = engine.start_timing();
        super::paint::replay(
            &reader,
            engine.text.content(),
            engine.geometry,
            &mut engine.fragments,
            &mut engine.finalization,
        );
        engine.record_timing(|t| t.finalize_layout += started.elapsed());
    });
}

/// Paint-only updates retain all box and line geometry.
pub fn repaint(
    inputs: LayoutInputs<'_>,
    geometry: &BoxGeometry,
    state: &mut LayoutState,
    _constraints: crate::LayoutConstraints,
) {
    let box_styles = BoxStyleSnapshot::new(
        inputs.document,
        inputs.inline_content,
        inputs.styles,
        inputs.topology,
        inputs.font_metrics,
    );
    let reader = LayoutReader::new(
        inputs.document,
        inputs.styles,
        inputs.topology,
        inputs.inline_content,
        inputs.font_metrics,
        inputs.image_metrics,
        &box_styles,
    )
    .with_paint(inputs.paint);
    let mut fragments = FragmentWriter::new(state, Vec::new());
    super::paint::replay(
        &reader,
        inputs.inline_content,
        geometry,
        &mut fragments,
        &mut FinalizationScratch::default(),
    );
}
