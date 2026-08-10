use crate::layout_model::{GlyphAdvanceRun, GlyphOffsetRun, Line, OverflowClip};
use kurbo::Rect;
use std::ops::Range;
use std::time::Instant;

use super::engine::LayoutEngine;
use super::fragment_writer::FragmentWriter;
use super::geometry_writer::GeometryWriter;
use super::read_context::LayoutReader;

mod geometry;
mod ordering;
mod overflow;

use geometry::publish_inline_box_geometry;
use ordering::{build_block_decoration_traversal, rebuild_decoration_fragments_by_line, rebuild_glyph_line_indices, rebuild_image_fragments_by_line, sort_lines_and_remap_images};
use overflow::rebuild_overflow_clips;

#[derive(Default)]
pub(super) struct FinalizationScratch {
    line_pairs: Vec<(Line, Vec<GlyphOffsetRun>, Vec<GlyphAdvanceRun>, usize)>,
    index_map: Vec<usize>,
    sorted_line_owners: Vec<u32>,
    block_groups: Vec<(u32, Range<u32>)>,
    content_clips: Vec<Option<OverflowClip>>,
    inline_bounds: Vec<Option<Rect>>,
}
impl FinalizationScratch {
    #[cfg(test)]
    pub(super) fn allocation_capacities(&self) -> (usize, usize, usize, usize, usize, usize) {
        (self.line_pairs.capacity(), self.index_map.capacity(), self.sorted_line_owners.capacity(), self.block_groups.capacity(), self.content_clips.capacity(), self.inline_bounds.capacity())
    }
}

impl LayoutEngine<'_, '_> {
    pub(super) fn finalize(&mut self, track_overflow_clips: bool) {
        let finalization_started = Instant::now();

        let start = Instant::now();
        sort_lines_and_remap_images(&self.reader, &mut self.fragments, &mut self.finalization);
        self.record_timing(|timings| timings.sort_lines_and_remap_images += start.elapsed());

        rebuild_glyph_line_indices(self.text.glyphs().len(), &mut self.fragments);
        build_block_decoration_traversal(&self.reader, &mut self.fragments, &mut self.finalization);

        let start = Instant::now();
        super::decorations::collect_inline_decorations(self);
        self.record_timing(|timings| timings.collect_inline_decorations += start.elapsed());

        publish_inline_box_geometry(&self.reader, &mut self.geometry, &self.fragments, &mut self.finalization);

        rebuild_decoration_fragments_by_line(&mut self.fragments);

        let start = Instant::now();
        rebuild_image_fragments_by_line(&mut self.fragments);
        self.record_timing(|timings| timings.rebuild_image_fragments_by_line += start.elapsed());

        rebuild_overflow_clips(&self.reader, &self.geometry, &mut self.fragments, &mut self.finalization, track_overflow_clips);
        self.record_timing(|timings| timings.finalize_layout += finalization_started.elapsed());
    }
}
