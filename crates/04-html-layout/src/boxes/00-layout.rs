//! Resolve, execute, and finish one box using the existing request/result contracts.
use super::model::BoxLayoutResult;
use super::s02_contents::layout_box_contents;
use super::s03_finish::finish_resolved_box;
use crate::{BoxLayoutRequest, LayoutEngine, ResolvedBoxSizing};

impl LayoutEngine<'_, '_> {
    pub fn layout_box(&mut self, request: BoxLayoutRequest) -> BoxLayoutResult {
        let resolved = self.resolve_box_sizing(request);
        self.layout_resolved_box(resolved)
    }

    pub fn layout_resolved_box(&mut self, resolved: ResolvedBoxSizing) -> BoxLayoutResult {
        let cursor = self.fragments.output_cursor();
        let placement = self.placement.begin_box(
            resolved.request.box_idx,
            resolved.request.splits_content_placement(),
        );
        let content_size = layout_box_contents(self, &resolved);
        finish_resolved_box(self, &resolved, content_size, cursor, placement)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
