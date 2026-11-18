//! Coordinate preparation, recursive geometry, finalization, and paint replay.
use super::engine::PassContext;
use super::engine::{
    LayoutEngine, LayoutInputs, LayoutOutputs, LayoutPassKind, LayoutReadContext, execute_layout,
};
use crate::boxes::absolute_positioning;
use crate::{BoxLayoutRequest, paint};
use html_engine_model::LayoutConstraints;
use html_style_model::PositionMode;
use kurbo::Point;

pub fn compute_layout(
    inputs: LayoutInputs<'_>,
    outputs: LayoutOutputs<'_>,
    constraints: LayoutConstraints,
) {
    let pass_context = PassContext::new(&inputs);
    let paint = inputs.paint;
    let read = LayoutReadContext::new(inputs, constraints, &pass_context);
    execute_layout(read, outputs, LayoutPassKind::Final, |engine| {
        engine.layout_root();
        engine.finalize();
        let reader = paint::PaintReader {
            shared: engine.read.reader.shared,
            paint,
        };
        paint::replay(
            &reader,
            engine.read.text.content(),
            &mut engine.fragments,
            &mut engine.scratch.paint,
        );
    });
}

impl LayoutEngine<'_, '_> {
    fn layout_root(&mut self) {
        if let Some(root_box) = self.read.reader.root_box() {
            // The root element establishes the initial block formatting
            // context, so its margins never collapse with its descendants.
            // Unlike ordinary boxes, it has no parent flow to consume those
            // margins; place its border box explicitly inside the canvas.
            let root_style = self.read.reader.style(root_box);
            let root_origin = Point::new(
                root_style
                    .margin_left()
                    .resolve(self.read.constraints.viewport_width()),
                root_style
                    .margin_top()
                    .resolve(self.read.constraints.viewport_width()),
            );
            if root_style.position() == PositionMode::Absolute {
                absolute_positioning::defer(self, root_box, Point::ZERO);
            } else {
                self.geometry.set_point(root_box, root_origin);
                let _ = self.layout_box(BoxLayoutRequest::normal(
                    root_box,
                    self.read.constraints.viewport_width(),
                    self.read.constraints.viewport_height(),
                ));
            }
            absolute_positioning::layout_pending(self, None);
        }

        self.scratch.floats.clear();
    }
}
