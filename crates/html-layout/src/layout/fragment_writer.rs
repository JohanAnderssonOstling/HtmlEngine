use crate::layout_model::{DecorationFragment, LayoutState, RoundedDecoration};
use kurbo::Rect;
use std::ops::Range;

#[derive(Clone, Copy)]
pub(crate) struct OutputCursor {
    lines: usize,
    decorations: usize,
    images: usize,
}

#[derive(Clone)]
pub(crate) struct OutputRanges {
    pub lines: Range<usize>,
    pub decorations: Range<usize>,
    pub images: Range<usize>,
}

impl OutputRanges {
    pub(crate) fn has_lines_or_images(&self) -> bool {
        !self.lines.is_empty() || !self.images.is_empty()
    }
}

/// Owns published line/fragment output and the parallel owner indexes required
/// while that output is being assembled.
pub(crate) struct FragmentWriter<'out> {
    state: &'out mut LayoutState,
    line_owners: Vec<u32>,
    block_decoration_owners: Vec<u32>,
    last_inline_fragments: Vec<u32>,
}

impl<'out> FragmentWriter<'out> {
    pub(crate) fn new(state: &'out mut LayoutState, line_owners: Vec<u32>, block_decoration_owners: Vec<u32>) -> Self {
        Self { state, line_owners, block_decoration_owners, last_inline_fragments: Vec::new() }
    }

    /// Resets all published fragment state while retaining reusable allocation.
    pub(crate) fn reset(&mut self) {
        self.line_owners.clear();
        self.block_decoration_owners.clear();
        self.last_inline_fragments.clear();

        let state = &mut self.state;
        state.line_output.lines.clear();
        state.line_output.paint_order_indices.clear();
        state.line_output.inline_box_fragments.clear();
        state.line_output.positioned_layers.clear();
        state.line_output.negative_positioned_layers.clear();
        state.line_output.independent_positioned_layers.clear();
        state.line_output.line_clips.clear();
        state.line_output.glyph_line_indices.clear();
        state.line_output.line_glyph_offsets.clear();
        state.line_output.line_glyph_advances.clear();
        state.line_output.ellipsis_fragments.clear();
        state.line_output.hyphen_fragments.clear();
        state.fragment_output.decorations.clear();
        state.fragment_output.decoration_line_indices.clear();
        state.fragment_output.decoration_paint_orders.clear();
        for fragments in &mut state.fragment_output.decoration_fragments_by_line {
            fragments.clear();
        }
        state.fragment_output.decoration_positioned_layers.clear();
        state.fragment_output.decoration_negative_positioned_layers.clear();
        state.fragment_output.decoration_independent_positioned_layers.clear();
        state.fragment_output.block_paint_ranges.clear();
        state.fragment_output.block_decoration_count = 0;
        state.fragment_output.decoration_clips.clear();
        state.fragment_output.image_fragments.clear();
        for fragments in &mut state.fragment_output.image_fragments_by_line {
            fragments.clear();
        }
        state.semantic_indexes.anchor_positions.clear();
    }

    pub(crate) fn state(&self) -> &LayoutState {
        self.state
    }

    pub(crate) fn state_mut(&mut self) -> &mut LayoutState {
        self.state
    }

    pub(crate) fn output_cursor(&self) -> OutputCursor {
        OutputCursor { lines: self.state.line_output.lines.len(), decorations: self.state.fragment_output.decorations.len(), images: self.state.fragment_output.image_fragments.len() }
    }

    pub(crate) fn output_since(&self, cursor: OutputCursor) -> OutputRanges {
        OutputRanges { lines: cursor.lines..self.state.line_output.lines.len(), decorations: cursor.decorations..self.state.fragment_output.decorations.len(), images: cursor.images..self.state.fragment_output.image_fragments.len() }
    }

    pub(crate) fn mark_positioned_layer(&mut self, output: &OutputRanges, negative: bool, independent: bool) {
        self.state.line_output.positioned_layers.resize(self.state.line_output.lines.len(), false);
        self.state.line_output.negative_positioned_layers.resize(self.state.line_output.lines.len(), false);
        self.state.line_output.independent_positioned_layers.resize(self.state.line_output.lines.len(), false);
        self.state.fragment_output.decoration_positioned_layers.resize(self.state.fragment_output.decorations.len(), false);
        self.state.fragment_output.decoration_negative_positioned_layers.resize(self.state.fragment_output.decorations.len(), false);
        self.state.fragment_output.decoration_independent_positioned_layers.resize(self.state.fragment_output.decorations.len(), false);
        self.state.line_output.positioned_layers[output.lines.clone()].fill(true);
        // A positioned ancestor owns the whole emitted subtree, but it must
        // not erase a negative stacking layer already established by a nested
        // positioned descendant. A negative ancestor, conversely, makes its
        // complete subtree part of the negative outer layer.
        if negative {
            self.state.line_output.negative_positioned_layers[output.lines.clone()].fill(true);
        }
        if independent {
            self.state.line_output.independent_positioned_layers[output.lines.clone()].fill(true);
        }
        self.state.fragment_output.decoration_positioned_layers[output.decorations.clone()].fill(true);
        if negative {
            self.state.fragment_output.decoration_negative_positioned_layers[output.decorations.clone()].fill(true);
        }
        if independent {
            for decoration_idx in output.decorations.clone() {
                // Foreground borders already paint after the owning layer's
                // lines. Keep them in that final traversal so overlapping
                // inline and absolute borders retain source order; only the
                // independently positioned background needs a later sublayer.
                if !self.state.fragment_output.decorations.fragments()[decoration_idx].is_foreground() {
                    self.state.fragment_output.decoration_independent_positioned_layers[decoration_idx] = true;
                }
            }
        }
    }

    pub(crate) fn first_baseline_offset(&self, lines: Range<usize>, origin_y: f64) -> Option<f64> {
        self.state.line_output.lines[lines].iter().filter(|line| line.point.y >= origin_y - 0.01).min_by(|left, right| left.point.y.total_cmp(&right.point.y)).map(|line| line.point.y + line.baseline - origin_y)
    }

    pub(crate) fn last_baseline_offset(&self, lines: Range<usize>, origin_y: f64) -> Option<f64> {
        self.state.line_output.lines[lines].iter().filter(|line| line.point.y >= origin_y - 0.01).max_by(|left, right| left.point.y.total_cmp(&right.point.y)).map(|line| line.point.y + line.baseline - origin_y)
    }

    pub(crate) fn push_line_owner(&mut self, owner: u32) {
        self.line_owners.push(owner);
    }

    /// Publishes one line's inline fragments and links continuations without
    /// adding per-line allocations.
    pub(crate) fn push_inline_box_fragments(&mut self, fragments: Vec<crate::layout_model::LineInlineBoxFragment>, box_count: usize) -> Range<u32> {
        const NONE: u32 = u32::MAX;
        self.last_inline_fragments.resize(box_count, NONE);
        let start = u32::try_from(self.state.line_output.inline_box_fragments.len()).expect("inline fragment arena exhausted");
        for mut fragment in fragments {
            let previous = self.last_inline_fragments[fragment.box_idx as usize];
            if previous != NONE && (previous as usize) < self.state.line_output.inline_box_fragments.len() {
                self.state.line_output.inline_box_fragments[previous as usize].flags &= !crate::layout_model::LineInlineBoxFragment::INLINE_END;
                fragment.flags &= !crate::layout_model::LineInlineBoxFragment::INLINE_START;
            }
            let current = u32::try_from(self.state.line_output.inline_box_fragments.len()).expect("inline fragment arena exhausted");
            self.last_inline_fragments[fragment.box_idx as usize] = current;
            self.state.line_output.inline_box_fragments.push(fragment);
        }
        start..u32::try_from(self.state.line_output.inline_box_fragments.len()).expect("inline fragment arena exhausted")
    }

    pub(crate) fn push_block_decoration(&mut self, owner: u32, decoration: DecorationFragment) {
        self.state.fragment_output.decorations.push(decoration);
        self.block_decoration_owners.push(owner);
    }

    pub(crate) fn decoration_len(&self) -> usize {
        self.state.fragment_output.decorations.len()
    }

    pub(crate) fn decorations_mut(&mut self) -> &mut Vec<DecorationFragment> {
        self.state.fragment_output.decorations.fragments_mut()
    }

    pub(crate) fn push_decoration(&mut self, decoration: DecorationFragment) {
        self.state.fragment_output.decorations.push(decoration);
    }

    pub(crate) fn push_rounded_decoration(&mut self, rect: Rect, color: u32, is_inline: bool, rounded: RoundedDecoration) {
        self.state.fragment_output.decorations.push_rounded_border(rect, color, is_inline, rounded);
    }

    pub(crate) fn push_rounded_background(&mut self, rect: Rect, color: u32, is_inline: bool, rounded: RoundedDecoration) {
        self.state.fragment_output.decorations.push_rounded_background(rect, color, is_inline, rounded);
    }

    pub(crate) fn record_decoration_owner_since(&mut self, owner: u32, start: usize) {
        let added = self.decoration_len().checked_sub(start).expect("decoration cursor must precede emitted output");
        self.block_decoration_owners.extend(std::iter::repeat_n(owner, added));
    }

    pub(crate) fn record_decoration_line_since(&mut self, line_idx: usize, start: usize) {
        const NO_LINE: u32 = u32::MAX;
        let end = self.decoration_len();
        self.state.fragment_output.decoration_line_indices.resize(end, NO_LINE);
        self.state.fragment_output.decoration_line_indices[start..end].fill(u32::try_from(line_idx).expect("line index exceeds decoration traversal capacity"));
    }

    /// Inline decorations are materialized after positioned subtree ranges
    /// have already been marked. Copy the carrier line's paint-layer flags to
    /// those late decorations so they can continue to be replayed with that
    /// line (especially after relative positioning moves them away from its
    /// unshifted flow geometry).
    pub(crate) fn inherit_line_layer_for_decorations_since(&mut self, line_idx: usize, start: usize) {
        let end = self.state.fragment_output.decorations.len();
        if start >= end {
            return;
        }
        let positioned = self.state.line_output.positioned_layers.get(line_idx).copied().unwrap_or(false);
        let negative = self.state.line_output.negative_positioned_layers.get(line_idx).copied().unwrap_or(false);
        let independent = self.state.line_output.independent_positioned_layers.get(line_idx).copied().unwrap_or(false);
        self.state.fragment_output.decoration_positioned_layers.resize(end, false);
        self.state.fragment_output.decoration_negative_positioned_layers.resize(end, false);
        self.state.fragment_output.decoration_independent_positioned_layers.resize(end, false);
        self.state.fragment_output.decoration_positioned_layers[start..end].fill(positioned);
        self.state.fragment_output.decoration_negative_positioned_layers[start..end].fill(negative);
        self.state.fragment_output.decoration_independent_positioned_layers[start..end].fill(independent);
    }

    pub(crate) fn record_decoration_paint_order_since(&mut self, paint_order: u32, start: usize) {
        let end = self.decoration_len();
        self.state.fragment_output.decoration_paint_orders.resize(end, u32::MAX);
        self.state.fragment_output.decoration_paint_orders[start..end].fill(paint_order);
    }

    pub(crate) fn finalization_parts(&mut self) -> (&mut LayoutState, &mut Vec<u32>, &mut Vec<u32>) {
        (self.state, &mut self.line_owners, &mut self.block_decoration_owners)
    }

    pub(crate) fn swap_scratch(&mut self, state: &mut LayoutState, line_owners: &mut Vec<u32>, block_decoration_owners: &mut Vec<u32>, last_inline_fragments: &mut Vec<u32>) {
        std::mem::swap(self.state, state);
        std::mem::swap(&mut self.line_owners, line_owners);
        std::mem::swap(&mut self.block_decoration_owners, block_decoration_owners);
        std::mem::swap(&mut self.last_inline_fragments, last_inline_fragments);
    }

    pub(crate) fn recycle_owner_storage(&mut self, line_owners: &mut Vec<u32>, block_decoration_owners: &mut Vec<u32>) {
        self.line_owners.clear();
        self.block_decoration_owners.clear();
        std::mem::swap(&mut self.line_owners, line_owners);
        std::mem::swap(&mut self.block_decoration_owners, block_decoration_owners);
    }
}
