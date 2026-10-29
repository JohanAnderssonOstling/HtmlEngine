//! Execute the root and its recursive formatting contexts.
use super::absolute_positioning;
use super::{BoxLayoutRequest, LayoutEngine};
use html_style_model::PositionMode;
use kurbo::Point;

impl LayoutEngine<'_, '_> {
    pub(super) fn layout_root(&mut self) {
        if let Some(root_box) = self.reader.root_box() {
            let start = self.start_timing();
            // The root element establishes the initial block formatting
            // context, so its margins never collapse with its descendants.
            // Unlike ordinary boxes, it has no parent flow to consume those
            // margins; place its border box explicitly inside the canvas.
            let root_style = self.reader.style(root_box);
            let root_origin = Point::new(
                root_style
                    .margin_left()
                    .resolve(self.config.viewport_width()),
                root_style
                    .margin_top()
                    .resolve(self.config.viewport_width()),
            );
            if root_style.position() == PositionMode::Absolute {
                absolute_positioning::defer(self, root_box, Point::ZERO);
            } else {
                self.geometry.set_point(root_box, root_origin);
                let _ = self.layout_box(BoxLayoutRequest::normal(
                    root_box,
                    self.config.viewport_width(),
                    self.config.viewport_height(),
                ));
            }
            absolute_positioning::layout_pending(self, None);
            let elapsed = start.elapsed();
            self.record_timing(|t| t.layout_tree_traversal += elapsed);
            self.record_timing(|t| t.root_box_layout += elapsed);
        }

        let start = self.start_timing();
        self.floats.clear();
        self.record_timing(|t| t.layout_tree_traversal += start.elapsed());
    }
}
