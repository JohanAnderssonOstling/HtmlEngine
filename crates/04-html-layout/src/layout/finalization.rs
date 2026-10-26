use crate::layout_model::Line;
use kurbo::Rect;
use std::ops::Range;

use super::engine::LayoutEngine;
use super::fragment_writer::FragmentWriter;
use super::geometry_writer::GeometryWriter;
use super::read_context::LayoutReader;

mod geometry;
mod ordering;
mod overflow;
pub(crate) mod text_paint;

use geometry::publish_inline_box_geometry;
use ordering::{
    build_block_decoration_traversal, rebuild_decoration_fragments_by_line,
    rebuild_glyph_line_indices, rebuild_image_fragments_by_line, sort_lines_and_remap_images,
};
use overflow::rebuild_overflow_clips;
use text_paint::prepare_line_text_runs;

#[derive(Default)]
pub(super) struct FinalizationScratch {
    line_pairs: Vec<(Line, Range<u32>, Range<u32>, usize)>,
    index_map: Vec<usize>,
    sorted_line_owners: Vec<u32>,
    block_groups: Vec<(u32, Range<u32>)>,
    inline_bounds: Vec<Option<Rect>>,
}
impl FinalizationScratch {
    #[cfg(test)]
    pub(super) fn allocation_capacities(&self) -> (usize, usize, usize, usize, usize, usize) {
        (
            self.line_pairs.capacity(),
            self.index_map.capacity(),
            self.sorted_line_owners.capacity(),
            self.block_groups.capacity(),
            0,
            self.inline_bounds.capacity(),
        )
    }
}

impl LayoutEngine<'_, '_> {
    pub(super) fn repaint(&mut self) {
        profile_scope!("layout.paint_replay");
        let commands = std::mem::take(&mut self.fragments.state_mut().paint_program.commands);
        let output = &mut self.fragments.state_mut().fragment_output;
        output.decorations.clear();
        output.decoration_line_indices.clear();
        output.decoration_paint_orders.clear();
        output.decoration_positioned_layers.clear();
        output.decoration_negative_positioned_layers.clear();
        output.decoration_independent_positioned_layers.clear();
        output.decoration_clips.clear();
        output.decoration_fragments_by_line.clear();
        // Paint emits absolute rectangles. Keep the canonical placement tree
        // intact when the geometry pass reuses this engine and style snapshot.
        let retained_placement = std::mem::take(&mut self.placement);
        self.placement.reset_for_measurement();
        self.fragments.set_paint_replay();
        for &command in &commands { super::decorations::replay_command(self, command); }
        build_block_decoration_traversal(&self.reader, &mut self.fragments, &mut self.finalization);
        super::decorations::collect_inline_decorations(self);
        rebuild_decoration_fragments_by_line(&mut self.fragments);
        if self.track_overflow_clips {
            let (layout, _, owners) = self.fragments.finalization_parts();
            layout.fragment_output.decoration_clips.extend(owners.iter().map(|&owner| {
                self.reader.get_parent(owner as usize).and_then(|parent| layout.paint_program.content_clips.get(parent).copied().flatten())
            }));
        }
        self.fragments.state_mut().paint_program.commands = commands;
        self.placement = retained_placement;
    }

    pub(super) fn finalize(&mut self, track_overflow_clips: bool) {
        profile_scope!("layout.finalize");
        let finalization_started = self.start_timing();

        self.release_previous_lines();

        // Recursive layout retains subtree placement transforms so moving a
        // container is independent of descendant count. Public layout output
        // deliberately remains absolute; flatten it once at this boundary.
        self.placement
            .publish_absolute(&mut self.geometry, &mut self.fragments);

        let start = self.start_timing();
        sort_lines_and_remap_images(
            &self.reader,
            &mut self.fragments,
            &mut self.finalization,
        );
        self.record_timing(|timings| timings.sort_lines_and_remap_images += start.elapsed());

        prepare_line_text_runs(&self.text, &mut self.fragments, &[]);

        rebuild_glyph_line_indices(self.text.glyphs().len(), &mut self.fragments);

        publish_inline_box_geometry(
            &self.reader,
            &mut self.geometry,
            &self.fragments,
            &mut self.finalization,
        );

        let start = self.start_timing();
        rebuild_image_fragments_by_line(&mut self.fragments);
        self.record_timing(|timings| timings.rebuild_image_fragments_by_line += start.elapsed());

        rebuild_overflow_clips(
            &self.reader,
            &self.geometry,
            &mut self.fragments,
            &mut self.finalization,
            track_overflow_clips,
        );
        self.record_timing(|timings| timings.finalize_layout += finalization_started.elapsed());
    }
}
