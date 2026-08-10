//! Renderer-facing text, glyph, and line-layout queries.

use super::*;

/// Stable renderer-facing line geometry.
///
/// Contract ranges can only be produced by a validated layout result:
///
/// ```compile_fail
/// let _ = html_layout::RenderLine {
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
    glyphs: Range<u32>,
    point: Point,
    height: f64,
    baseline: f64,
    word_spacing: f64,
    letter_spacing: f64,
    optical_offset_x: f64,
    paint_color: Option<u32>,
    positioned_layer: bool,
    negative_positioned_layer: bool,
    independent_positioned_layer: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderLineTextFragment {
    glyphs: Range<u32>,
    offset_x: f64,
    paint_order: u32,
}

impl RenderLineTextFragment {
    pub fn glyphs(&self) -> Range<u32> {
        self.glyphs.clone()
    }

    pub fn offset_x(&self) -> f64 {
        self.offset_x
    }

    pub fn paint_order(&self) -> u32 {
        self.paint_order
    }
}

pub struct RenderLineTextFragments<'a> {
    line: &'a Line,
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
        if let Some(fragments) = &self.line.text_fragments {
            let fragment = fragments.get(self.index)?;
            self.index += 1;
            return Some(RenderLineTextFragment { glyphs: fragment.glyphs.clone(), offset_x: fragment.offset_x, paint_order: fragment.paint_order });
        }
        if self.implicit_emitted || self.line.glyphs.is_empty() {
            return None;
        }
        self.implicit_emitted = true;
        Some(RenderLineTextFragment { glyphs: self.line.glyphs.clone(), offset_x: 0.0, paint_order: 0 })
    }
}

impl RenderLine {
    fn from_line(index: usize, line: &Line, positioned_layer: bool, negative_positioned_layer: bool, independent_positioned_layer: bool) -> Self {
        Self {
            index,
            glyphs: line.glyphs.clone(),
            point: line.point,
            height: line.height,
            baseline: line.baseline,
            word_spacing: line.word_spacing,
            letter_spacing: line.letter_spacing,
            optical_offset_x: line.optical_offset_x,
            paint_color: line.paint_color,
            positioned_layer,
            negative_positioned_layer,
            independent_positioned_layer,
        }
    }

    pub fn index(&self) -> usize {
        self.index
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
        self.positioned_layer
    }

    pub fn is_in_negative_positioned_layer(&self) -> bool {
        self.negative_positioned_layer
    }

    pub fn is_in_independent_positioned_layer(&self) -> bool {
        self.independent_positioned_layer
    }
}

pub struct RenderLines<'a> {
    lines: &'a [Line],
    positioned_layers: &'a [bool],
    negative_positioned_layers: &'a [bool],
    independent_positioned_layers: &'a [bool],
}

impl<'a> RenderLines<'a> {
    fn new(lines: &'a [Line], positioned_layers: &'a [bool], negative_positioned_layers: &'a [bool], independent_positioned_layers: &'a [bool]) -> Self {
        Self { lines, positioned_layers, negative_positioned_layers, independent_positioned_layers }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn get(&self, idx: usize) -> Option<RenderLine> {
        self.lines.get(idx).map(|line| {
            RenderLine::from_line(
                idx,
                line,
                self.positioned_layers.get(idx).copied().unwrap_or(false),
                self.negative_positioned_layers.get(idx).copied().unwrap_or(false),
                self.independent_positioned_layers.get(idx).copied().unwrap_or(false),
            )
        })
    }

    pub fn first(&self) -> Option<RenderLine> {
        self.lines.first().map(|line| {
            RenderLine::from_line(0, line, self.positioned_layers.first().copied().unwrap_or(false), self.negative_positioned_layers.first().copied().unwrap_or(false), self.independent_positioned_layers.first().copied().unwrap_or(false))
        })
    }

    pub fn last(&self) -> Option<RenderLine> {
        let index = self.lines.len().checked_sub(1)?;
        self.lines.last().map(|line| {
            RenderLine::from_line(
                index,
                line,
                self.positioned_layers.get(index).copied().unwrap_or(false),
                self.negative_positioned_layers.get(index).copied().unwrap_or(false),
                self.independent_positioned_layers.get(index).copied().unwrap_or(false),
            )
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderLine> + '_ {
        self.lines.iter().enumerate().map(|(index, line)| {
            RenderLine::from_line(
                index,
                line,
                self.positioned_layers.get(index).copied().unwrap_or(false),
                self.negative_positioned_layers.get(index).copied().unwrap_or(false),
                self.independent_positioned_layers.get(index).copied().unwrap_or(false),
            )
        })
    }
}

#[derive(Clone, Debug)]
pub struct RenderGlyphOffsetRun {
    range: Range<u32>,
    offset: f32,
}

impl RenderGlyphOffsetRun {
    fn from_run(run: &GlyphOffsetRun) -> Self {
        Self { range: run.range.clone(), offset: run.offset }
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
        Self { range: run.range.clone(), advance: run.advance }
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
}

#[derive(Clone, Copy, Debug)]
pub struct RenderHyphenFragment {
    glyph: GlyphId,
    offset: Point,
}

impl RenderHyphenFragment {
    fn from_fragment(fragment: &HyphenFragment) -> Self {
        Self { glyph: fragment.glyph, offset: fragment.offset }
    }

    pub fn glyph(&self) -> GlyphId {
        self.glyph
    }

    pub fn offset(&self) -> Point {
        self.offset
    }
}

impl RenderEllipsisFragment {
    fn from_fragment(fragment: &EllipsisFragment) -> Self {
        Self { glyph: fragment.glyph, offset: fragment.offset }
    }

    pub fn glyph(&self) -> GlyphId {
        self.glyph
    }

    pub fn offset(&self) -> Point {
        self.offset
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

#[derive(Clone, Copy, Debug)]
enum TextRunKind {
    Text,
    Marker,
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
    runs: &'a [InlineItem],
    idx: usize,
    kind_filter: Option<TextRunKind>,
    only_box: Option<usize>,
}

impl<'a> RenderTextRuns<'a> {
    fn new(runs: &'a [InlineItem], kind_filter: Option<TextRunKind>) -> Self {
        Self { runs, idx: 0, kind_filter, only_box: None }
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
        loop {
            let idx = self.idx;
            self.idx += 1;

            let run = self.runs.get(idx)?;
            if self.only_box.is_some_and(|box_idx| box_idx != run.box_idx as usize) {
                continue;
            }

            if let Some(kind_filter) = self.kind_filter {
                match (&run.kind, kind_filter) {
                    (InlineItemKind::Text { glyphs }, TextRunKind::Text) => {
                        return Some(RenderTextRun { kind: TextRunKind::Text, box_idx: run.box_idx as usize, dom_text_node: run.dom_text_node, glyphs: glyphs.clone() });
                    }
                    (InlineItemKind::Marker { glyphs }, TextRunKind::Marker) => {
                        return Some(RenderTextRun { kind: TextRunKind::Marker, box_idx: run.box_idx as usize, dom_text_node: run.dom_text_node, glyphs: glyphs.clone() });
                    }
                    _ => continue,
                }
            }

            match &run.kind {
                InlineItemKind::Text { glyphs } => {
                    return Some(RenderTextRun { kind: TextRunKind::Text, box_idx: run.box_idx as usize, dom_text_node: run.dom_text_node, glyphs: glyphs.clone() });
                }
                InlineItemKind::Marker { glyphs } => {
                    return Some(RenderTextRun { kind: TextRunKind::Marker, box_idx: run.box_idx as usize, dom_text_node: run.dom_text_node, glyphs: glyphs.clone() });
                }
                _ => continue,
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct RenderTextView<'a> {
    pub(super) doc: &'a LaidOutDocument,
}

impl<'a> RenderTextView<'a> {
    /// Spatial line indexes in CSS paint order.
    pub fn paint_order_indices(self) -> &'a [u32] {
        &self.doc.layout_state.line_output.paint_order_indices
    }

    pub fn glyph_count(self) -> usize {
        self.doc.shaped.inline_content.glyphs().len()
    }

    pub fn glyph_at(self, glyph_idx: usize) -> Option<GlyphId> {
        self.doc.shaped.inline_content.glyph_at(glyph_idx)
    }

    pub fn glyph_slice(self, range: Range<u32>) -> Option<&'a [GlyphId]> {
        let start = usize::try_from(range.start).ok()?;
        let end = usize::try_from(range.end).ok()?;
        self.doc.shaped.inline_content.glyphs().get(start..end)
    }

    pub fn glyph_metric(self, glyph: GlyphId) -> Option<crate::GlyphMetric> {
        self.doc.shaped.glyph_metrics.get_checked(glyph)
    }

    /// Natural shaped advance for one logical character.
    pub fn character_advance(self, character_index: u32) -> Option<f32> {
        self.doc.text_geometry().advance(character_index as usize)
    }

    pub fn is_character_cluster_boundary(self, character_boundary: u32) -> bool {
        self.doc.text_geometry().is_cluster_boundary(character_boundary as usize)
    }

    pub fn authoritative_runs(self, requested: Range<u32>) -> impl Iterator<Item = RenderAuthoritativeTextRun> + 'a {
        self.doc.text_geometry().authoritative_runs().iter().filter_map(move |run| {
            let start = requested.start.max(run.source_range.start);
            let end = requested.end.min(run.source_range.end);
            if start >= end {
                return None;
            }
            let local_start = start - run.source_range.start;
            let local_end = end - run.source_range.start;
            Some(RenderAuthoritativeTextRun {
                source_range: start..end,
                run_range: local_start..local_end,
                run: run.backend_run,
                natural_offset: *run.caret_stops.get(local_start as usize)?,
                ascent: run.ascent,
                placement_required: run.placement_required,
            })
        })
    }

    pub fn line_count(self) -> usize {
        self.doc.layout_state.line_output.lines.len()
    }

    pub fn lines(self) -> RenderLines<'a> {
        RenderLines::new(&self.doc.layout_state.line_output.lines, &self.doc.layout_state.line_output.positioned_layers, &self.doc.layout_state.line_output.negative_positioned_layers, &self.doc.layout_state.line_output.independent_positioned_layers)
    }

    pub fn line(self, idx: usize) -> Option<RenderLine> {
        self.doc.layout_state.line_output.lines.get(idx).map(|line| {
            RenderLine::from_line(
                idx,
                line,
                self.doc.layout_state.line_output.positioned_layers.get(idx).copied().unwrap_or(false),
                self.doc.layout_state.line_output.negative_positioned_layers.get(idx).copied().unwrap_or(false),
                self.doc.layout_state.line_output.independent_positioned_layers.get(idx).copied().unwrap_or(false),
            )
        })
    }

    /// Resolved overflow clip for the line's formatting context.
    pub fn line_overflow_clip(self, idx: usize) -> Option<RenderOverflowClip> {
        self.doc.layout_state.line_output.line_clips.get(idx).copied().flatten().map(RenderOverflowClip::from_clip)
    }

    /// Returns the line that owns a source glyph.
    pub fn line_index_for_glyph(self, glyph_idx: u32) -> Option<usize> {
        let encoded = *self.doc.layout_state.line_output.glyph_line_indices.get(glyph_idx as usize)?;
        (encoded != u32::MAX).then_some(encoded as usize)
    }

    pub fn line_glyph_offsets(self, line_idx: usize) -> Option<RenderGlyphOffsetRuns<'a>> {
        self.doc.layout_state.line_output.line_glyph_offsets.get(line_idx).map(|runs| RenderGlyphOffsetRuns::new(runs))
    }

    pub fn line_glyph_advances(self, line_idx: usize) -> Option<RenderGlyphAdvanceRuns<'a>> {
        self.doc.layout_state.line_output.line_glyph_advances.get(line_idx).map(|runs| RenderGlyphAdvanceRuns::new(runs))
    }

    pub fn line_text_fragments(self, line_idx: usize) -> Option<RenderLineTextFragments<'a>> {
        let line = self.doc.layout_state.line_output.lines.get(line_idx)?;
        Some(RenderLineTextFragments { line, index: 0, implicit_emitted: false })
    }

    pub fn ellipsis_for_line(self, line_idx: usize) -> Option<RenderEllipsisFragment> {
        let fragments = &self.doc.layout_state.line_output.ellipsis_fragments;
        let idx = fragments.binary_search_by_key(&line_idx, |fragment| fragment.line_idx).ok()?;
        fragments.get(idx).map(RenderEllipsisFragment::from_fragment)
    }

    pub fn hyphen_for_line(self, line_idx: usize) -> Option<RenderHyphenFragment> {
        let fragments = &self.doc.layout_state.line_output.hyphen_fragments;
        let idx = fragments.binary_search_by_key(&line_idx, |fragment| fragment.line_idx).ok()?;
        fragments.get(idx).map(RenderHyphenFragment::from_fragment)
    }

    pub fn text_runs(self) -> RenderTextRuns<'a> {
        RenderTextRuns::new(self.doc.shaped.inline_content.inline_items(), Some(TextRunKind::Text))
    }

    pub fn marker_runs(self) -> RenderTextRuns<'a> {
        RenderTextRuns::new(self.doc.shaped.inline_content.inline_items(), Some(TextRunKind::Marker))
    }
}
