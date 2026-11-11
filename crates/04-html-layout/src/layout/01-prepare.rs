//! Bind immutable inputs and manage one executor's borrowed output lifetime.
use super::engine::{LayoutEngine, LayoutOutputs, LayoutPassKind, LayoutReadContext};

pub(super) fn execute_layout<'input, 'out, T>(
    read: LayoutReadContext<'input>,
    outputs: LayoutOutputs<'out>,
    pass_kind: LayoutPassKind,
    operation: impl FnOnce(&mut LayoutEngine<'input, 'out>) -> T,
) -> T {
    let mut engine = LayoutEngine::new(read, outputs, pass_kind);
    engine.prepare_layout_pass();
    let result = operation(&mut engine);
    engine.finish_pass();
    result
}

impl LayoutEngine<'_, '_> {
    pub(super) fn prepare_layout_pass(&mut self) {
        self.clear_layout_output();
        self.scratch.floats.reset();
        self.scratch.margins.reset();
        self.scratch.absolute_positioning.clear();
    }
}
