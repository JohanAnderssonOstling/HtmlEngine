//! Renderer-facing text, glyph, and line-layout queries.

use super::*;
use crate::layout_model::paint_program::PaintLayers;
use crate::layout_model::{LineDetails, LineId, LineOutput, LineTextFragment, LineTextPlacement};
use html_style_model::{TextDirection, Visibility};
use html_text::{InlineTextKind as TextRunKind, suppresses_glyph};

/// Merge overlapping and adjacent logical text intervals in place, removing
/// empty and reversed ranges. Callers validate document bounds separately.
pub fn merge_text_ranges(ranges: &mut Vec<Range<u32>>) {
    ranges.retain(|range| !range.is_empty());
    ranges.sort_unstable_by_key(|range| range.start);
    ranges.dedup_by(|next, previous| {
        if next.start <= previous.end {
            previous.end = previous.end.max(next.end);
            true
        } else {
            false
        }
    });
}

/// Stable renderer-facing line geometry.
///
/// Contract ranges can only be produced by a validated layout result:
///
/// ```compile_fail
/// let _ = html_pipeline::engine::RenderLine {
///     glyphs: 9..2,
///     point: kurbo::Point::ZERO,
///     height: 0.0,
///     baseline: 0.0,
///     word_spacing: 0.0,
///     letter_spacing: 0.0,
/// };
/// ```
#[derive(Clone, Debug)]
pub struct RenderLine {
    index: usize,
    owner_box_idx: usize,
    glyphs: Range<u32>,
    point: Point,
    height: f64,
    baseline: f64,
    word_spacing: f64,
    letter_spacing: f64,
    optical_offset_x: f64,
    paint_color: Option<u32>,
    paint_layers: PaintLayers,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderLineTextFragment {
    glyphs: Range<u32>,
    offset_x: f64,
    rtl: bool,
    paint_order: u32,
    visible: bool,
}

impl RenderLineTextFragment {
    pub fn is_rtl(&self) -> bool {
        self.rtl
    }
    pub fn glyphs(&self) -> Range<u32> {
        self.glyphs.clone()
    }

    pub fn offset_x(&self) -> f64 {
        self.offset_x
    }

    pub fn paint_order(&self) -> u32 {
        self.paint_order
    }

    pub fn visible(&self) -> bool {
        self.visible
    }
}

pub struct RenderLineTextFragments<'a> {
    line: &'a Line,
    fragments: Option<&'a [LineTextFragment]>,
    index: usize,
    implicit_emitted: bool,
}

#[derive(Clone, Debug)]
pub struct RenderAuthoritativeTextRun {
    source_range: Range<u32>,
    run_range: Range<u32>,
    run: crate::TextRunId,
    natural_offset: f32,
    ascent: f32,
    placement_required: bool,
}

impl RenderAuthoritativeTextRun {
    pub fn source_range(&self) -> Range<u32> {
        self.source_range.clone()
    }
    pub fn run_range(&self) -> Range<u32> {
        self.run_range.clone()
    }
    pub fn run(&self) -> crate::TextRunId {
        self.run
    }
    pub fn natural_offset(&self) -> f32 {
        self.natural_offset
    }
    pub fn ascent(&self) -> f32 {
        self.ascent
    }
    pub fn placement_required(&self) -> bool {
        self.placement_required
    }
}

impl<'a> Iterator for RenderLineTextFragments<'a> {
    type Item = RenderLineTextFragment;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(fragments) = self.fragments {
            let fragment = fragments.get(self.index)?;
            self.index += 1;
            return Some(RenderLineTextFragment {
                glyphs: fragment.glyphs.clone(),
                offset_x: fragment.offset_x,
                rtl: fragment.rtl,
                paint_order: fragment.paint_order,
                visible: fragment.visible,
            });
        }
        if self.implicit_emitted || self.line.glyphs.is_empty() {
            return None;
        }
        self.implicit_emitted = true;
        Some(RenderLineTextFragment {
            glyphs: self.line.glyphs.clone(),
            offset_x: 0.0,
            rtl: false,
            paint_order: 0,
            visible: true,
        })
    }
}

impl RenderLine {
    fn from_line(
        index: usize,
        line: &Line,
        details: &[LineDetails],
        paint_layers: PaintLayers,
    ) -> Self {
        Self {
            index,
            owner_box_idx: line.owner_box_idx as usize,
            glyphs: line.glyphs.clone(),
            point: line.point,
            height: line.height,
            baseline: line.baseline,
            word_spacing: line.word_spacing,
            letter_spacing: line.letter_spacing,
            optical_offset_x: line.optical_offset_x(details),
            paint_color: line.paint_color(details),
            paint_layers,
        }
    }

    /// Stable storage identity, independent of the spatial iteration position.
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn id(&self) -> LineId {
        LineId::new(self.index)
    }

    /// Block box that emitted this line.
    pub fn owner_box_idx(&self) -> usize {
        self.owner_box_idx
    }

    pub fn glyphs(&self) -> Range<u32> {
        self.glyphs.clone()
    }

    pub fn start(&self) -> u32 {
        self.glyphs.start
    }

    pub fn end(&self) -> u32 {
        self.glyphs.end
    }

    pub fn point(&self) -> Point {
        self.point
    }

    pub fn point_x(&self) -> f64 {
        self.point.x
    }

    pub fn point_y(&self) -> f64 {
        self.point.y
    }

    pub fn height(&self) -> f64 {
        self.height
    }

    pub fn baseline(&self) -> f64 {
        self.baseline
    }

    pub fn word_spacing(&self) -> f64 {
        self.word_spacing
    }

    pub fn letter_spacing(&self) -> f64 {
        self.letter_spacing
    }

    /// Horizontal paint offset for optical margin alignment. Logical line
    /// geometry remains anchored at [`Self::point`].
    pub fn optical_offset_x(&self) -> f64 {
        self.optical_offset_x
    }

    /// A pseudo-element paint color for this line, when one was resolved
    /// after line breaking (currently `::first-line`).
    pub fn paint_color(&self) -> Option<u32> {
        self.paint_color
    }

    pub fn is_in_positioned_layer(&self) -> bool {
        self.paint_layers.is_positioned()
    }

    pub fn is_in_negative_positioned_layer(&self) -> bool {
        self.paint_layers.is_negative()
    }

    pub fn is_in_independent_positioned_layer(&self) -> bool {
        self.paint_layers.is_independent()
    }
}

/// Spatially ordered line view. `get` takes a spatial rank; each returned
/// line's `index`/`id` identifies storage and is used by the other line queries.
pub struct RenderLines<'a> {
    output: &'a LineOutput,
}

impl<'a> RenderLines<'a> {
    fn new(output: &'a LineOutput) -> Self {
        Self { output }
    }

    pub fn len(&self) -> usize {
        self.output.lines.len()
    }
    pub fn is_empty(&self) -> bool {
        self.output.lines.is_empty()
    }

    pub fn get(&self, rank: usize) -> Option<RenderLine> {
        let id = *self.output.spatial_order.get(rank)?;
        render_line(self.output, id.index())
    }

    pub fn first(&self) -> Option<RenderLine> {
        self.get(0)
    }
    pub fn last(&self) -> Option<RenderLine> {
        self.get(self.len().checked_sub(1)?)
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderLine> + '_ {
        self.output
            .spatial_order
            .iter()
            .map(|id| render_line(self.output, id.index()).expect("valid line identity"))
    }
}

fn render_line(output: &LineOutput, index: usize) -> Option<RenderLine> {
    let line = output.lines.get(index)?;
    Some(RenderLine::from_line(
        index,
        line,
        &output.line_details,
        output.paint_layers.get(index).copied().unwrap_or_default(),
    ))
}

#[derive(Clone, Debug)]
pub struct RenderGlyphOffsetRun {
    range: Range<u32>,
    offset: f32,
}

impl RenderGlyphOffsetRun {
    fn from_run(run: &GlyphOffsetRun) -> Self {
        Self {
            range: run.range.clone(),
            offset: run.offset,
        }
    }

    pub fn range(&self) -> Range<u32> {
        self.range.clone()
    }

    pub fn offset(&self) -> f64 {
        self.offset as f64
    }
}

pub struct RenderGlyphOffsetRuns<'a> {
    runs: &'a [GlyphOffsetRun],
}

#[derive(Clone, Debug)]
pub struct RenderGlyphAdvanceRun {
    range: Range<u32>,
    advance: f32,
}

impl RenderGlyphAdvanceRun {
    fn from_run(run: &GlyphAdvanceRun) -> Self {
        Self {
            range: run.range.clone(),
            advance: run.advance,
        }
    }

    pub fn range(&self) -> Range<u32> {
        self.range.clone()
    }

    pub fn advance(&self) -> f64 {
        self.advance as f64
    }
}

pub struct RenderGlyphAdvanceRuns<'a> {
    runs: &'a [GlyphAdvanceRun],
}

#[derive(Clone, Copy, Debug)]
pub struct RenderEllipsisFragment {
    glyph: GlyphId,
    offset: Point,
    visible: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderHyphenFragment {
    glyph: GlyphId,
    offset: Point,
    visible: bool,
}

impl RenderHyphenFragment {
    fn from_fragment(fragment: &HyphenFragment) -> Self {
        Self {
            glyph: fragment.glyph,
            offset: fragment.offset,
            visible: fragment.visible,
        }
    }

    pub fn glyph(&self) -> GlyphId {
        self.glyph
    }

    pub fn offset(&self) -> Point {
        self.offset
    }

    pub fn visible(&self) -> bool {
        self.visible
    }
}

impl RenderEllipsisFragment {
    fn from_fragment(fragment: &EllipsisFragment) -> Self {
        Self {
            glyph: fragment.glyph,
            offset: fragment.offset,
            visible: fragment.visible,
        }
    }

    pub fn glyph(&self) -> GlyphId {
        self.glyph
    }

    pub fn offset(&self) -> Point {
        self.offset
    }

    pub fn visible(&self) -> bool {
        self.visible
    }
}

impl<'a> RenderGlyphAdvanceRuns<'a> {
    fn new(runs: &'a [GlyphAdvanceRun]) -> Self {
        Self { runs }
    }

    pub fn len(&self) -> usize {
        self.runs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
    pub fn get(&self, idx: usize) -> Option<RenderGlyphAdvanceRun> {
        self.runs.get(idx).map(RenderGlyphAdvanceRun::from_run)
    }
    pub fn iter(&self) -> impl Iterator<Item = RenderGlyphAdvanceRun> + '_ {
        self.runs.iter().map(RenderGlyphAdvanceRun::from_run)
    }
}

impl<'a> RenderGlyphOffsetRuns<'a> {
    fn new(runs: &'a [GlyphOffsetRun]) -> Self {
        Self { runs }
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderGlyphOffsetRun> + '_ {
        self.runs.iter().map(RenderGlyphOffsetRun::from_run)
    }

    pub fn len(&self) -> usize {
        self.runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    pub fn get(&self, idx: usize) -> Option<RenderGlyphOffsetRun> {
        self.runs.get(idx).map(RenderGlyphOffsetRun::from_run)
    }
}

#[derive(Clone, Debug)]

pub struct RenderTextRun {
    kind: TextRunKind,
    box_idx: usize,
    dom_text_node: Option<u32>,
    glyphs: Range<u32>,
}

impl RenderTextRun {
    pub fn is_marker(&self) -> bool {
        matches!(self.kind, TextRunKind::Marker)
    }

    pub fn box_idx(&self) -> usize {
        self.box_idx
    }

    pub fn dom_text_node(&self) -> Option<u32> {
        self.dom_text_node
    }

    pub fn glyphs(&self) -> Range<u32> {
        self.glyphs.clone()
    }
}

pub struct RenderTextRuns<'a> {
    runs: std::slice::Iter<'a, InlineItem>,
    kind_filter: Option<TextRunKind>,
    only_box: Option<usize>,
}

impl<'a> RenderTextRuns<'a> {
    fn new(runs: &'a [InlineItem], kind_filter: Option<TextRunKind>) -> Self {
        Self {
            runs: runs.iter(),
            kind_filter,
            only_box: None,
        }
    }

    pub fn only_box(mut self, box_idx: usize) -> Self {
        self.only_box = Some(box_idx);
        self
    }
}

pub(super) fn text_runs(runs: &[InlineItem]) -> RenderTextRuns<'_> {
    RenderTextRuns::new(runs, Some(TextRunKind::Text))
}

impl<'a> Iterator for RenderTextRuns<'a> {
    type Item = RenderTextRun;

    fn next(&mut self) -> Option<Self::Item> {
        for item in self.runs.by_ref() {
            let Some(run) = item.text_run() else { continue };
            if self
                .only_box
                .is_some_and(|box_idx| box_idx != run.box_idx as usize)
                || self.kind_filter.is_some_and(|kind| kind != run.kind)
            {
                continue;
            }
            return Some(RenderTextRun {
                kind: run.kind,
                box_idx: run.box_idx as usize,
                dom_text_node: run.dom_text_node,
                glyphs: run.glyphs.clone(),
            });
        }
        None
    }
}

#[derive(Clone, Copy)]
pub struct RenderTextView<'a> {
    pub(super) glyph_metrics: &'a crate::layout_model::GlyphMetrics,
    pub(super) doc: DocumentRead<'a>,
}

impl<'a> RenderTextView<'a> {
    /// Native key consumed by a baseline-oriented painter.
    pub fn paint_handle(self, glyph: GlyphId) -> Option<crate::GlyphHandle> {
        self.glyph_metrics.paint_handle(glyph)
    }

    /// Resolved advances, vertical offsets and paint glyphs for one line.
    /// The borrowed cursor allocates nothing and supports source-order reads
    /// across visually reordered fragments.
    pub fn line_placement(self, line_idx: usize) -> Option<LineTextPlacement<'a>> {
        let output = &self.doc.layout_state.line_output;
        let line = output.lines.get(line_idx)?;
        let shaped = self.doc.shaped();
        Some(LineTextPlacement::new(
            self.doc.inputs.text.content(),
            shaped.glyphs,
            self.glyph_metrics,
            Some(shaped.text_geometry),
            line,
            output.line_glyph_offsets.get(line_idx).unwrap_or(&[]),
            output.line_glyph_advances.get(line_idx).unwrap_or(&[]),
        ))
    }

    /// Stable line IDs in CSS paint order.
    pub fn paint_order_indices(self) -> &'a [LineId] {
        &self.doc.layout_state.line_output.paint_order_indices
    }

    /// Returns the CSS paint rank for a stable line ID.
    pub fn paint_order_rank(self, line_idx: usize) -> Option<u32> {
        self.doc
            .layout_state
            .line_output
            .paint_order_ranks
            .get(line_idx)
            .copied()
    }

    pub fn glyph_count(self) -> usize {
        self.doc.shaped().glyphs.len()
    }

    pub fn glyph_at(self, glyph_idx: usize) -> Option<GlyphId> {
        self.doc.shaped().glyphs.get(glyph_idx).copied()
    }

    /// Prepared logical character, including text transforms and generated
    /// content. Bidi mirroring affects paint identity rather than this text.
    pub fn character_at(self, index: usize) -> Option<char> {
        self.doc.inputs.text.content().character_at(index)
    }

    pub fn glyph_slice(self, range: Range<u32>) -> Option<crate::shaping::GlyphSlice<'a>> {
        let start = usize::try_from(range.start).ok()?;
        let end = usize::try_from(range.end).ok()?;
        self.doc.shaped().glyphs.slice(start..end)
    }

    /// Resolved source color, independent of the retained backend glyph/run.
    pub fn paint_color_at(self, character_index: u32) -> Option<u32> {
        let colors = &self.doc.paint_colors;
        let index = colors.partition_point(|run| run.glyphs.end <= character_index);
        colors
            .get(index)
            .filter(|run| run.glyphs.contains(&character_index))
            .map(|run| run.color)
    }

    pub fn glyph_metric(self, glyph: GlyphId) -> Option<crate::GlyphMetric> {
        self.glyph_metrics.get_checked(glyph)
    }

    /// Paint identity, including bidi mirroring, without changing logical text.
    /// Directional controls and discretionary hyphens have no ordinary ink.
    pub fn paint_glyph_at(self, index: u32) -> Option<GlyphId> {
        let glyph = self.glyph_at(index as usize)?;
        let character = self.character_at(index as usize)?;
        if suppresses_glyph(character) {
            return None;
        }
        Some(self.doc.text_geometry().paint_glyph(index, glyph))
    }

    pub fn character_direction(self, index: u32) -> TextDirection {
        if self.doc.text_geometry().bidi().level(index) % 2 == 1 {
            TextDirection::Rtl
        } else {
            TextDirection::Ltr
        }
    }

    /// Natural shaped advance for one logical character.
    pub fn character_advance(self, character_index: u32) -> Option<f32> {
        self.doc.text_geometry().advance(character_index as usize)
    }

    pub fn is_character_cluster_boundary(self, character_boundary: u32) -> bool {
        self.doc
            .text_geometry()
            .is_cluster_boundary(character_boundary as usize)
    }

    pub fn authoritative_runs(
        self,
        requested: Range<u32>,
    ) -> impl Iterator<Item = RenderAuthoritativeTextRun> + 'a {
        self.doc
            .text_geometry()
            .authoritative_runs(requested)
            .filter_map(move |run| {
                let run_range = run.run_range();
                let natural_offset =
                    run.natural_offset(self.character_direction(run.source_range.start))?;
                Some(RenderAuthoritativeTextRun {
                    source_range: run.source_range,
                    run_range,
                    run: run.backend_run,
                    natural_offset,
                    ascent: run.ascent,
                    placement_required: run.placement_required,
                })
            })
    }

    /// Retained, line-relative authoritative run slices prepared by layout.
    /// `None` means native-run coverage was incomplete for this line.
    pub fn prepared_text_runs(
        self,
        line_idx: usize,
    ) -> Option<&'a [crate::PreparedTextRunFragment]> {
        let output = &self.doc.layout_state.line_output;
        let line = output.lines.get(line_idx)?;
        if !line.native_text_runs_complete {
            return None;
        }
        output
            .prepared_text_run_fragments
            .get(line.prepared_text_runs.start as usize..line.prepared_text_runs.end as usize)
    }

    pub fn line_count(self) -> usize {
        self.doc.layout_state.line_output.lines.len()
    }

    pub fn lines(self) -> RenderLines<'a> {
        RenderLines::new(&self.doc.layout_state.line_output)
    }

    /// Stable line IDs in spatial order. Geometry range queries return ranks
    /// into this traversal, never ranges of line identities.
    pub fn spatial_order(self) -> &'a [LineId] {
        &self.doc.layout_state.line_output.spatial_order
    }

    pub fn spatial_rank(self, id: LineId) -> Option<usize> {
        self.doc
            .layout_state
            .line_output
            .spatial_ranks
            .get(id.index())
            .map(|&rank| rank as usize)
    }

    /// Look up a stable line identity, rather than a spatial traversal rank.
    pub fn line(self, id: usize) -> Option<RenderLine> {
        render_line(&self.doc.layout_state.line_output, id)
    }

    /// Resolved overflow clip for the line's formatting context.
    pub fn line_overflow_clip(self, idx: usize) -> Option<RenderOverflowClip> {
        self.doc
            .layout_state
            .line_output
            .line_clips
            .get(idx)
            .copied()
            .flatten()
            .map(RenderOverflowClip::from_clip)
    }

    /// Returns the line that owns a source glyph.
    #[inline]
    pub fn line_index_for_glyph(self, glyph_idx: u32) -> Option<usize> {
        self.doc
            .layout_state
            .line_output
            .glyph_line_indices
            .get(glyph_idx as usize)
    }

    pub fn line_glyph_offsets(self, line_idx: usize) -> Option<RenderGlyphOffsetRuns<'a>> {
        self.doc
            .layout_state
            .line_output
            .line_glyph_offsets
            .get(line_idx)
            .map(|runs| RenderGlyphOffsetRuns::new(runs))
    }

    pub fn line_glyph_advances(self, line_idx: usize) -> Option<RenderGlyphAdvanceRuns<'a>> {
        self.doc
            .layout_state
            .line_output
            .line_glyph_advances
            .get(line_idx)
            .map(|runs| RenderGlyphAdvanceRuns::new(runs))
    }

    pub fn line_text_fragments(self, line_idx: usize) -> Option<RenderLineTextFragments<'a>> {
        let line = self.doc.layout_state.line_output.lines.get(line_idx)?;
        Some(RenderLineTextFragments {
            line,
            fragments: line
                .text_fragment_range(&self.doc.layout_state.line_output.line_details)
                .map(|range| {
                    &self.doc.layout_state.line_output.text_fragments
                        [range.start as usize..range.end as usize]
                }),
            index: 0,
            implicit_emitted: false,
        })
    }

    pub fn ellipsis_for_line(self, line_idx: usize) -> Option<RenderEllipsisFragment> {
        let fragments = &self.doc.layout_state.line_output.ellipsis_fragments;
        let idx = fragments
            .binary_search_by_key(&line_idx, |fragment| fragment.line_idx)
            .ok()?;
        fragments
            .get(idx)
            .map(RenderEllipsisFragment::from_fragment)
    }

    pub fn hyphen_for_line(self, line_idx: usize) -> Option<RenderHyphenFragment> {
        let fragments = &self.doc.layout_state.line_output.hyphen_fragments;
        let idx = fragments
            .binary_search_by_key(&line_idx, |fragment| fragment.line_idx)
            .ok()?;
        fragments.get(idx).map(RenderHyphenFragment::from_fragment)
    }

    pub fn text_runs(self) -> RenderTextRuns<'a> {
        RenderTextRuns::new(
            self.doc.inputs.text.content().inline_items(),
            Some(TextRunKind::Text),
        )
    }

    /// Source-backed text in logical order, excluding generated and hidden text.
    /// Descendants with `visibility: visible` remain available under hidden parents.
    pub fn visible_source_runs(self) -> impl Iterator<Item = RenderTextRun> + 'a {
        self.text_runs().filter(move |run| {
            run.dom_text_node().is_some()
                && self
                    .doc
                    .box_used_style(run.box_idx())
                    .is_some_and(|style| style.visibility() == Visibility::Visible)
        })
    }

    pub fn marker_runs(self) -> RenderTextRuns<'a> {
        RenderTextRuns::new(
            self.doc.inputs.text.content().inline_items(),
            Some(TextRunKind::Marker),
        )
    }
}

#[cfg(test)]
mod range_tests {
    use super::merge_text_ranges;
    use crate::allocation_test_support::count_allocations;
    use std::ops::Range;

    #[test]
    fn range_merging_reuses_input_storage_at_u32_boundaries() {
        let mut ranges = vec![
            u32::MAX - 2..u32::MAX,
            5..9,
            2..6,
            9..9,
            0..1,
            1..2,
            8..10,
            u32::MAX - 3..u32::MAX - 1,
            Range { start: 12, end: 11 },
        ];
        let pointer = ranges.as_ptr();
        let capacity = ranges.capacity();
        let ((), allocations) = count_allocations(|| merge_text_ranges(&mut ranges));
        assert_eq!(ranges, [0..10, u32::MAX - 3..u32::MAX]);
        assert_eq!(allocations, 0);
        assert_eq!(ranges.as_ptr(), pointer);
        assert_eq!(ranges.capacity(), capacity);
    }
}
