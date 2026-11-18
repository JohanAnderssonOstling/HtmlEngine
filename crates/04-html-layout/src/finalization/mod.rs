//! Publish final geometry, indexes, pagination metadata, ownership, and clips.
use html_engine_model::GroupedArena;
use kurbo::Rect;
use std::ops::Range;

use super::engine::LayoutEngine;
use super::engine::LayoutReader;
use crate::output::FragmentWriter;

mod geometry;
mod ordering;
mod overflow;
mod ownership;
mod pagination;

use geometry::publish_inline_box_geometry;
use ordering::{build_line_traversals, prepare_line_metadata, rebuild_glyph_line_indices};
use overflow::rebuild_overflow_clips;

#[derive(Default)]
pub(super) struct FinalizationScratch {
    inline_bounds: Vec<Option<Rect>>,
    line_bounds: Vec<Range<f64>>,
    carriers: GroupedArena<usize>,
    ownership: ownership::OwnershipScratch,
}

impl LayoutEngine<'_, '_> {
    pub(super) fn finalize(&mut self) {
        // Inherited facts and reverse range propagation require parents first.
        // Children and subtrees do not need contiguous box storage.
        debug_assert!(
            (0..self.read.reader.box_count()).all(|owner| {
                self.read
                    .reader
                    .get_parent(owner)
                    .is_none_or(|parent| parent < owner)
            }),
            "finalization requires parent-before-child box storage"
        );
        // Recursive layout retains subtree placement transforms so moving a
        // container is independent of descendant count. Public layout output
        // deliberately remains absolute; flatten it once at this boundary.
        self.placement
            .publish_absolute(self.geometry, &mut self.fragments);

        // Published source layout is independent of pagination height.
        self.fragments
            .state_mut()
            .line_output
            .pagination_fragment_height = None;
        self.scratch.paint.clear();
        build_line_traversals(
            &self.read.reader,
            &mut self.fragments,
            &mut self.scratch.paint,
        );

        rebuild_glyph_line_indices(self.read.text.glyphs().len(), &mut self.fragments);

        publish_inline_box_geometry(
            &self.read.reader,
            self.geometry,
            &self.fragments,
            &mut self.scratch.finalization,
        );

        prepare_line_metadata(
            &mut self.fragments,
            &mut self.scratch.finalization.line_bounds,
            &mut self.scratch.finalization.carriers,
        );
        pagination::build_pagination_groups(
            &self.read.reader,
            self.geometry,
            &mut self.fragments,
            &self.scratch.finalization.line_bounds,
            &self.scratch.finalization.carriers,
        );
        pagination::build_float_keeps(
            &self.read.reader,
            self.geometry,
            &mut self.fragments,
            &self.scratch.finalization.line_bounds,
        );

        ownership::prepare_ownership(
            &self.read.reader,
            self.geometry,
            &mut self.fragments,
            &self.scratch.finalization.carriers,
            &mut self.scratch.finalization.ownership,
        );
        rebuild_overflow_clips(&self.read.reader, self.geometry, &mut self.fragments);
    }
}
