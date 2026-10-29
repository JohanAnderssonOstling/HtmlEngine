//! Resolve, execute, and finish one box using the existing request/result contracts.
use super::box_contents::layout_box_contents;
pub use super::box_model::BoxLayoutResult;
use super::box_positioning::finish_resolved_box;
use super::{BoxLayoutRequest, LayoutEngine, ResolvedBoxSizing};

impl LayoutEngine<'_, '_> {
    pub fn defer_absolute_box(&mut self, box_idx: usize, static_position: kurbo::Point) {
        super::absolute_positioning::defer(self, box_idx, static_position);
    }

    pub fn layout_box(&mut self, request: BoxLayoutRequest) -> BoxLayoutResult {
        let resolved = self.resolve_box_sizing(request);
        self.layout_resolved_box(resolved)
    }

    pub fn layout_resolved_box(&mut self, resolved: ResolvedBoxSizing) -> BoxLayoutResult {
        let started = self.start_timing();
        let cursor = self.fragments.output_cursor();
        let placement = self.placement.begin_box(
            resolved.request.box_idx,
            resolved.request.splits_content_placement(),
        );
        let content_size = layout_box_contents(self, &resolved);
        let result = finish_resolved_box(self, &resolved, content_size, cursor, placement);
        self.record_timing(|t| t.layout_box_total += started.elapsed());
        result
    }
}

#[cfg(test)]
#[path = "../box_layout_tests.rs"]
mod tests;
