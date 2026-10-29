//! Bind immutable inputs and manage one executor's borrowed output lifetime.
use super::engine::{LayoutEngine, LayoutInputs, LayoutOutputs, LayoutReadContext, LayoutTimings};
use super::read_context::BoxStyleSnapshot;
use html_engine_model::PaintView;
use std::cell::RefCell;

pub(super) fn with_engine(
    inputs: LayoutInputs<'_>,
    outputs: LayoutOutputs<'_, '_>,
    constraints: crate::LayoutConstraints,
    timings: Option<&mut LayoutTimings>,
    run: impl for<'input, 'out> FnOnce(&mut LayoutEngine<'input, 'out>, PaintView<'input>),
) {
    let box_styles = BoxStyleSnapshot::new(
        inputs.document,
        inputs.inline_content,
        inputs.styles,
        inputs.topology,
        inputs.font_metrics,
    );
    let paint = inputs.paint;
    let read = LayoutReadContext::new(inputs, constraints, &box_styles);
    let mut collected = timings.as_deref().cloned().map(RefCell::new);
    execute_layout(read, outputs, &mut collected, 0, |engine| {
        run(engine, paint)
    });
    if let (Some(destination), Some(collected)) = (timings, collected) {
        *destination = collected.into_inner();
    }
}

pub(super) fn execute_layout<'input, 'out, T>(
    read: LayoutReadContext<'input>,
    outputs: LayoutOutputs<'out, '_>,
    timings: &mut Option<RefCell<LayoutTimings>>,
    fragmentation_suppression_depth: usize,
    operation: impl FnOnce(&mut LayoutEngine<'input, 'out>) -> T,
) -> T {
    let LayoutOutputs {
        geometry,
        state,
        placement,
        scratch,
    } = outputs;
    let mut engine = LayoutEngine::new(
        read,
        LayoutOutputs {
            geometry,
            state,
            placement: &mut *placement,
            scratch: &mut *scratch,
        },
        timings.take(),
        fragmentation_suppression_depth,
    );
    engine.prepare_layout_pass();
    let result = operation(&mut engine);
    *timings = engine.timings.take();
    engine.recycle_into(placement, scratch);
    result
}

impl LayoutEngine<'_, '_> {
    pub(super) fn prepare_layout_pass(&mut self) {
        let start = self.start_timing();
        self.clear_layout_output();
        self.record_timing(|t| t.clear_layout_output += start.elapsed());
        let start = self.start_timing();
        self.floats.reset();
        self.margins.reset();
        self.absolute_positioning.clear();
        self.flex_grid.clear();
        self.record_timing(|t| t.layout_tree_traversal += start.elapsed());
    }
}
