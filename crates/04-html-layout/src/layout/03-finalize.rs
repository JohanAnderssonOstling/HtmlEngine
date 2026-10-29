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
#[path = "finalization/text_paint.rs"]
pub mod text_paint;

use geometry::publish_inline_box_geometry;
use ordering::{
    build_line_traversals, rebuild_glyph_line_indices, rebuild_image_fragments_by_line,
};
use overflow::rebuild_overflow_clips;

#[derive(Default)]
pub(super) struct FinalizationScratch {
    block_groups: Vec<(u32, Range<u32>)>,
    inline_bounds: Vec<Option<Rect>>,
}

impl LayoutEngine<'_, '_> {
    pub(super) fn finalize(&mut self) {
        profile_scope!("layout.finalize");
        let finalization_started = self.start_timing();

        // Recursive layout retains subtree placement transforms so moving a
        // container is independent of descendant count. Public layout output
        // deliberately remains absolute; flatten it once at this boundary.
        self.placement
            .publish_absolute(self.geometry, &mut self.fragments);

        let start = self.start_timing();
        build_line_traversals(&self.reader, &mut self.fragments);
        self.record_timing(|timings| timings.build_line_traversals += start.elapsed());

        rebuild_glyph_line_indices(self.text.glyphs().len(), &mut self.fragments);

        publish_inline_box_geometry(
            &self.reader,
            self.geometry,
            &self.fragments,
            &mut self.finalization,
        );

        let start = self.start_timing();
        rebuild_image_fragments_by_line(&mut self.fragments);
        self.record_timing(|timings| timings.rebuild_image_fragments_by_line += start.elapsed());

        rebuild_overflow_clips(&self.reader, self.geometry, &mut self.fragments);
        self.record_timing(|timings| timings.finalize_layout += finalization_started.elapsed());
    }
}
