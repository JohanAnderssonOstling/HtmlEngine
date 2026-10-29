use crate::layout_model::paint_program::PaintLayers;
use crate::layout_model::{DecorationFragment, DecorationStore, LayoutState, RoundedDecoration};
use kurbo::{Point, Rect, Vec2};
use std::ops::Range;

use super::placement::{BoxPlacement, PlacementState};

#[derive(Clone, Copy)]
pub struct OutputCursor {
    lines: usize,
    images: usize,
    paint_commands: usize,
}

#[derive(Clone)]
pub struct OutputRanges {
    pub placement: BoxPlacement,
    pub lines: Range<usize>,
    pub images: Range<usize>,
    pub paint_commands: Range<usize>,
}

impl OutputRanges {
    pub fn has_lines_or_images(&self) -> bool {
        !self.lines.is_empty() || !self.images.is_empty()
    }
}

/// Owns published line/fragment output and temporary decoration owners used
/// while paint output is assembled.
pub struct FragmentWriter<'out> {
    state: &'out mut LayoutState,
    block_decoration_owners: Vec<u32>,
    last_inline_fragments: Vec<u32>,
}

impl<'out> FragmentWriter<'out> {
    pub(super) fn new(state: &'out mut LayoutState, block_decoration_owners: Vec<u32>) -> Self {
        Self {
            state,
            block_decoration_owners,
            last_inline_fragments: Vec::new(),
        }
    }

    /// Resets all published fragment state while retaining reusable allocation.
    pub fn reset(&mut self) {
        self.block_decoration_owners.clear();
        self.last_inline_fragments.clear();

        let state = &mut self.state;
        state.paint_program.clear();
        state.line_output.lines.clear();
        state.line_output.spatial_order.clear();
        state.line_output.spatial_ranks.clear();
        state.line_output.line_details.clear();
        state.line_output.text_fragments.clear();
        state.line_output.paint_order_indices.clear();
        state.line_output.paint_order_ranks.clear();
        state.line_output.inline_box_fragments.clear();
        state.line_output.paint_layers.clear();
        state.line_output.line_clips.clear();
        state.line_output.glyph_line_indices.clear();
        state.line_output.line_glyph_offsets.clear();
        state.line_output.line_glyph_advances.clear();
        state.line_output.prepared_text_run_fragments.clear();
        state.line_output.ellipsis_fragments.clear();
        state.line_output.hyphen_fragments.clear();
        state.fragment_output.decorations.clear();
        state.fragment_output.scene_paint_order.clear();
        state.fragment_output.decoration_line_indices.clear();
        state.fragment_output.decoration_paint_orders.clear();
        state.fragment_output.decoration_fragments_by_line.clear();
        state.fragment_output.decoration_paint_layers.clear();
        state.fragment_output.block_paint_ranges.clear();
        state.fragment_output.block_decoration_indices_by_y.clear();
        state.fragment_output.block_decoration_prefix_max_y1.clear();
        state.fragment_output.block_decoration_paint_ranks.clear();
        state.fragment_output.block_decoration_count = 0;
        state.fragment_output.decoration_clips.clear();
        state.fragment_output.image_fragments.clear();
        state.fragment_output.image_fragments_by_line.clear();
        state.semantic_indexes.anchor_positions.clear();
    }

    pub fn state(&self) -> &LayoutState {
        self.state
    }

    pub fn state_mut(&mut self) -> &mut LayoutState {
        self.state
    }

    pub fn output_cursor(&self) -> OutputCursor {
        OutputCursor {
            lines: self.state.line_output.lines.len(),
            images: self.state.fragment_output.image_fragments.len(),
            paint_commands: self.state.paint_program.commands.len(),
        }
    }

    pub fn output_since(&self, cursor: OutputCursor, placement: BoxPlacement) -> OutputRanges {
        OutputRanges {
            placement,
            lines: cursor.lines..self.state.line_output.lines.len(),
            images: cursor.images..self.state.fragment_output.image_fragments.len(),
            paint_commands: cursor.paint_commands..self.state.paint_program.commands.len(),
        }
    }

    pub fn record_paint(
        &mut self,
        placement: &PlacementState,
        operation: super::paint_program::PaintOperation,
    ) {
        if placement.retains_output() {
            self.state
                .paint_program
                .record(operation, placement.current_group());
        }
    }

    pub fn apply_paint_layers_since(&mut self, start: usize, layers: PaintLayers) {
        let output = &mut self.state.fragment_output;
        let end = output.decorations.len();
        output
            .decoration_paint_layers
            .resize(end, PaintLayers::default());
        for index in start..end {
            output.decoration_paint_layers[index] =
                layers.for_decoration(output.decorations.fragments()[index].is_foreground());
        }
    }

    pub fn mark_positioned_layer(
        &mut self,
        output: &OutputRanges,
        negative: bool,
        independent: bool,
    ) {
        let layers = PaintLayers::positioned(negative, independent);
        for command in &mut self.state.paint_program.commands[output.paint_commands.clone()] {
            command.layers.include(layers);
        }
        let lines = &mut self.state.line_output;
        lines
            .paint_layers
            .resize(lines.lines.len(), PaintLayers::default());
        // Ancestors add their layer without erasing a nested descendant's
        // negative or independent positioned layer.
        for line in &mut lines.paint_layers[output.lines.clone()] {
            line.include(layers);
        }
    }

    pub fn baseline_offsets(
        &self,
        lines: Range<usize>,
        origin_y: f64,
    ) -> (Option<f64>, Option<f64>) {
        let mut first: Option<(Point, f64)> = None;
        let mut last: Option<(Point, f64)> = None;
        for line in &self.state.line_output.lines[lines] {
            let point = line.point;
            if point.y >= origin_y - 0.01 {
                if first.is_none_or(|(current, _)| point.y < current.y) {
                    first = Some((point, line.baseline));
                }
                if last.is_none_or(|(current, _)| point.y > current.y) {
                    last = Some((point, line.baseline));
                }
            }
        }
        let offset = |(point, baseline): (Point, f64)| point.y + baseline - origin_y;
        (first.map(offset), last.map(offset))
    }

    pub fn line_point(&self, line_idx: usize) -> Point {
        self.state.line_output.lines[line_idx].point
    }

    pub fn lines_before_y(&self, lines: Range<usize>, y: f64) -> usize {
        lines
            .filter(|&line_idx| self.line_point(line_idx).y < y)
            .count()
    }

    pub(super) fn line_len(&self) -> usize {
        self.state.line_output.lines.len()
    }

    pub(super) fn materialize_line_positions(
        &mut self,
        line_groups: &[u32],
        resolved_offsets: &[Vec2],
    ) {
        for (index, line) in self.state.line_output.lines.iter_mut().enumerate() {
            let offset = resolved_offsets[line_groups[index] as usize];
            if offset != Vec2::ZERO {
                line.point += offset;
            }
        }
    }

    /// Publishes one line's inline fragments and links continuations without
    /// adding per-line allocations.
    pub fn push_inline_box_fragments(
        &mut self,
        fragments: Vec<crate::layout_model::LineInlineBoxFragment>,
        box_count: usize,
    ) -> Range<u32> {
        const NONE: u32 = u32::MAX;
        self.last_inline_fragments.resize(box_count, NONE);
        let start = u32::try_from(self.state.line_output.inline_box_fragments.len())
            .expect("inline fragment arena exhausted");
        for mut fragment in fragments {
            let previous = self.last_inline_fragments[fragment.box_idx as usize];
            if previous != NONE
                && (previous as usize) < self.state.line_output.inline_box_fragments.len()
            {
                self.state.line_output.inline_box_fragments[previous as usize].flags &=
                    !crate::layout_model::LineInlineBoxFragment::INLINE_END;
                fragment.flags &= !crate::layout_model::LineInlineBoxFragment::INLINE_START;
            }
            let current = u32::try_from(self.state.line_output.inline_box_fragments.len())
                .expect("inline fragment arena exhausted");
            self.last_inline_fragments[fragment.box_idx as usize] = current;
            self.state.line_output.inline_box_fragments.push(fragment);
        }
        start
            ..u32::try_from(self.state.line_output.inline_box_fragments.len())
                .expect("inline fragment arena exhausted")
    }

    pub(super) fn push_block_decoration(&mut self, owner: u32, decoration: DecorationFragment) {
        self.state.fragment_output.decorations.push(decoration);
        self.block_decoration_owners.push(owner);
    }

    pub fn decoration_len(&self) -> usize {
        self.state.fragment_output.decorations.len()
    }

    pub fn decoration_store_mut(&mut self) -> &mut DecorationStore {
        &mut self.state.fragment_output.decorations
    }

    pub(super) fn push_decoration(&mut self, decoration: DecorationFragment) {
        self.state.fragment_output.decorations.push(decoration);
    }

    pub(super) fn push_rounded_decoration(
        &mut self,
        rect: Rect,
        color: u32,
        is_inline: bool,
        rounded: RoundedDecoration,
    ) {
        self.state
            .fragment_output
            .decorations
            .push_rounded_border(rect, color, is_inline, rounded);
    }

    pub(super) fn push_rounded_background(
        &mut self,
        rect: Rect,
        color: u32,
        is_inline: bool,
        rounded: RoundedDecoration,
    ) {
        self.state
            .fragment_output
            .decorations
            .push_rounded_background(rect, color, is_inline, rounded);
    }

    pub(super) fn record_decoration_owner_since(&mut self, owner: u32, start: usize) {
        let added = self
            .decoration_len()
            .checked_sub(start)
            .expect("decoration cursor must precede emitted output");
        self.block_decoration_owners
            .extend(std::iter::repeat_n(owner, added));
    }

    pub fn record_decoration_line_since(&mut self, line_idx: usize, start: usize) {
        const NO_LINE: u32 = u32::MAX;
        let end = self.decoration_len();
        self.state
            .fragment_output
            .decoration_line_indices
            .resize(end, NO_LINE);
        self.state.fragment_output.decoration_line_indices[start..end].fill(
            u32::try_from(line_idx).expect("line index exceeds decoration traversal capacity"),
        );
    }

    /// Inline decorations are materialized after positioned subtree ranges
    /// have already been marked. Copy the carrier line's paint-layer flags to
    /// those late decorations so they can continue to be replayed with that
    /// line (especially after relative positioning moves them away from its
    /// unshifted flow geometry).
    pub fn inherit_line_layer_for_decorations_since(&mut self, line_idx: usize, start: usize) {
        let end = self.state.fragment_output.decorations.len();
        if start >= end {
            return;
        }
        let layers = self
            .state
            .line_output
            .paint_layers
            .get(line_idx)
            .copied()
            .unwrap_or_default();
        let decorations = &mut self.state.fragment_output.decoration_paint_layers;
        decorations.resize(end, PaintLayers::default());
        decorations[start..end].fill(layers);
    }

    pub fn record_decoration_paint_order_since(&mut self, paint_order: u32, start: usize) {
        let end = self.decoration_len();
        self.state
            .fragment_output
            .decoration_paint_orders
            .resize(end, u32::MAX);
        self.state.fragment_output.decoration_paint_orders[start..end].fill(paint_order);
    }

    pub fn decoration_parts(&mut self) -> (&mut LayoutState, &mut Vec<u32>) {
        (self.state, &mut self.block_decoration_owners)
    }

    pub fn take_decoration_owners(&mut self) -> Vec<u32> {
        self.block_decoration_owners.clear();
        std::mem::take(&mut self.block_decoration_owners)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_model::Line;
    use crate::layout_model::paint_program::PaintOperation;

    #[test]
    fn nested_layers_survive_command_replay_and_late_inline_decorations() {
        let mut state = LayoutState::default();
        let mut fragments = FragmentWriter::new(&mut state, Vec::new());
        let mut placement = PlacementState::default();
        placement.reset(1);
        let owner = placement.begin_box(0, false);
        let cursor = fragments.output_cursor();
        fragments
            .state_mut()
            .line_output
            .lines
            .push(Line::default());
        fragments.record_paint(
            &placement,
            PaintOperation::Background {
                owner: 0,
                rect: Rect::ZERO,
            },
        );
        let output = fragments.output_since(cursor, owner);
        fragments.mark_positioned_layer(&output, true, true);
        fragments.mark_positioned_layer(&output, false, false);
        let layers = fragments.state().line_output.paint_layers[0];
        assert!(layers.is_positioned() && layers.is_negative() && layers.is_independent());
        assert_eq!(layers, fragments.state().paint_program.commands[0].layers);
        assert_eq!(size_of::<PaintLayers>(), 1);

        fragments.push_decoration(DecorationFragment::background_rect(Rect::ZERO, 0, false));
        fragments.push_decoration(DecorationFragment::foreground_rect(Rect::ZERO, 0, false));
        fragments.apply_paint_layers_since(0, layers);
        let decorations = &fragments.state().fragment_output.decoration_paint_layers;
        assert_eq!(decorations[0], layers);
        assert!(decorations[1].is_positioned() && decorations[1].is_negative());
        assert!(!decorations[1].is_independent());

        fragments.push_decoration(DecorationFragment::background_rect(Rect::ZERO, 0, true));
        fragments.inherit_line_layer_for_decorations_since(0, 2);
        assert_eq!(
            fragments.state().fragment_output.decoration_paint_layers[2],
            layers
        );
    }

    #[test]
    fn baseline_endpoints_keep_first_ties_and_ignore_lines_above_the_origin() {
        let mut state = LayoutState::default();
        for (y, baseline) in [
            (-1.0, 9.0),
            (0.0, 3.0),
            (0.0, 8.0),
            (10.0, 2.0),
            (f64::NAN, 100.0),
        ] {
            state.line_output.lines.push(Line {
                point: Point::new(0.0, y),
                baseline,
                ..Line::default()
            });
        }
        let fragments = FragmentWriter::new(&mut state, Vec::new());
        assert_eq!(
            fragments.baseline_offsets(0..5, 0.0),
            (Some(3.0), Some(12.0))
        );
        assert_eq!(
            fragments.baseline_offsets(2..4, 0.0),
            (Some(8.0), Some(12.0))
        );
        assert_eq!(
            fragments.baseline_offsets(0..5, 5.0),
            (Some(7.0), Some(7.0))
        );
        assert_eq!(fragments.baseline_offsets(0..5, 100.0), (None, None));
    }
}
