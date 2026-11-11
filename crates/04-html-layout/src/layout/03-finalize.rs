use kurbo::Rect;
use std::ops::Range;

use super::engine::LayoutEngine;
use super::fragment_writer::FragmentWriter;
use super::read_context::LayoutReader;

#[path = "finalization/geometry.rs"]
mod geometry;
#[path = "finalization/ordering.rs"]
pub(super) mod ordering;
#[path = "finalization/overflow.rs"]
mod overflow;
#[path = "finalization/ownership.rs"]
mod ownership;
#[path = "finalization/pagination.rs"]
mod pagination;
#[path = "finalization/text_paint.rs"]
pub mod text_paint;

use geometry::publish_inline_box_geometry;
use ordering::{build_line_traversals, rebuild_glyph_line_indices, rebuild_image_fragments_by_line};
use overflow::rebuild_overflow_clips;

#[derive(Default)]
pub(super) struct FinalizationScratch {
    block_groups: Vec<(u32, Range<u32>)>,
    inline_bounds: Vec<Option<Rect>>,
}

impl LayoutEngine<'_, '_> {
    pub(super) fn finalize(&mut self) {

        // Recursive layout retains subtree placement transforms so moving a
        // container is independent of descendant count. Public layout output
        // deliberately remains absolute; flatten it once at this boundary.
        self.placement
            .publish_absolute(self.geometry, &mut self.fragments);

        // Published source layout is independent of pagination height.
        self.fragments.state_mut().line_output.pagination_fragment_height = None;
        self.fragments.state_mut().line_output.pagination_header_rows.clear();
        build_line_traversals(&self.reader, &mut self.fragments);

        rebuild_glyph_line_indices(self.text.glyphs().len(), &mut self.fragments);

        publish_inline_box_geometry(
            &self.reader,
            self.geometry,
            &self.fragments,
            &mut self.scratch.finalization,
        );

        rebuild_image_fragments_by_line(&mut self.fragments);
        pagination::build_pagination_groups(&self.reader, self.geometry, &mut self.fragments);
        pagination::build_float_keeps(&self.reader, self.geometry, &mut self.fragments);

        ownership::prepare_ownership(&self.reader, self.geometry, &mut self.fragments);
        rebuild_overflow_clips(&self.reader, self.geometry, &mut self.fragments);
    }
}
