//! Renderer-facing text, glyph, and line-layout queries.

use html_engine_model::layout_state;
use super::*;
use crate::layout_model::paint_program::PaintLayers;
use crate::layout_model::{
    LineCharacterPlacement, LineId, LineOutput, LineTextFragments, LineTextPlacement,
};
use crate::{GlyphHandle, GlyphMetric, PreparedTextRunFragment};
use html_engine_model::paint_color_at as resolved_paint_color_at;
use html_style_model::TextDirection;
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

/// Smallest logical interval containing both ranges, including any gap.
pub(super) fn enclosing_text_range(a: Range<u32>, b: Range<u32>) -> Range<u32> {
    a.start.min(b.start)..a.end.max(b.end)
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
    id: LineId,
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
    pub fn glyphs(&self) -> Range<u32> {
        self.glyphs.clone()
    }

    pub fn visible(&self) -> bool {
        self.visible
    }
}

pub struct RenderLineTextFragments<'a> {
    fragments: LineTextFragments<'a>,
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

impl Iterator for RenderLineTextFragments<'_> {
    type Item = RenderLineTextFragment;

    fn next(&mut self) -> Option<Self::Item> {
        let fragment = self.fragments.next()?;
        Some(RenderLineTextFragment {
            glyphs: fragment.glyphs,
            offset_x: fragment.offset_x,
            rtl: fragment.rtl,
            paint_order: fragment.paint_order,
            visible: fragment.visible,
        })
    }
}

impl RenderLine {
    /// Stable storage identity, independent of the spatial iteration position.
    pub fn id(&self) -> LineId {
        self.id
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

    pub fn height(&self) -> f64 {
        self.height
    }
}

/// Spatially ordered line view. `get` takes a spatial rank; each returned
/// line's `id` identifies storage and is used by the other line queries.
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

    pub fn first(&self) -> Option<RenderLine> {
        render_lines_get(&self, 0)
    }
    pub fn last(&self) -> Option<RenderLine> {
        render_lines_get(&self, self.len().checked_sub(1)?)
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderLine> + 'a + use<'a> {
        let output = self.output;
        output
            .spatial_order
            .iter()
            .map(move |id| render_line(output, *id).expect("valid line identity"))
    }
}

fn render_line(output: &LineOutput, id: LineId) -> Option<RenderLine> {
    let index = layout_state::line_id_index(id);
    let line = output.lines.get(index)?;
    Some(RenderLine {
        id,
        owner_box_idx: line.owner_box_idx as usize,
        glyphs: line.glyphs.clone(),
        point: line.point,
        height: line.height,
        baseline: line.baseline,
        word_spacing: line.word_spacing,
        letter_spacing: line.letter_spacing,
        optical_offset_x: line.optical_offset_x(&output.line_details),
        paint_color: line.paint_color(&output.line_details),
        paint_layers: output.paint_layers.get(index).copied().unwrap_or_default(),
    })
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

    pub(super) fn dom_text_node(&self) -> Option<u32> {
        self.dom_text_node
    }

    pub fn glyphs(&self) -> Range<u32> {
        self.glyphs.clone()
    }
}

pub struct RenderTextRuns<'a> {
    runs: std::slice::Iter<'a, InlineItem>,
    kind_filter: TextRunKind,
    only_box: Option<usize>,
}

impl<'a> RenderTextRuns<'a> {
    fn new(runs: &'a [InlineItem], kind_filter: TextRunKind) -> Self {
        Self {
            runs: runs.iter(),
            kind_filter,
            only_box: None,
        }
    }

    pub fn only_box(mut self, box_id: BoxId) -> Self {
        let box_idx = layout_state::box_id_index(box_id);
        self.only_box = Some(box_idx);
        self
    }
}

pub(super) fn text_runs(runs: &[InlineItem]) -> RenderTextRuns<'_> {
    RenderTextRuns::new(runs, TextRunKind::Text)
}

impl<'a> Iterator for RenderTextRuns<'a> {
    type Item = RenderTextRun;

    fn next(&mut self) -> Option<Self::Item> {
        for item in self.runs.by_ref() {
            let Some(run) = item.text_run() else { continue };
            if self
                .only_box
                .is_some_and(|box_idx| box_idx != run.box_idx as usize)
                || self.kind_filter != run.kind
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

/// Borrowed placement cursor constructed from a published render view.
/// The engine retains its shaping buffers and lookup hints.
pub struct RenderLinePlacement<'a> {
    cursor: LineTextPlacement<'a>,
}

impl RenderLinePlacement<'_> {
    pub fn at(&mut self, index: u32) -> Option<LineCharacterPlacement> {
        self.cursor.at(index)
    }
}

impl<'a> RenderTextView<'a> {


    pub fn glyph_count(self) -> usize {
        self.doc.shaped().glyphs.len()
    }



    pub(crate) fn glyph_slice(self, range: Range<u32>) -> Option<crate::shaping::GlyphSlice<'a>> {
        let start = usize::try_from(range.start).ok()?;
        let end = usize::try_from(range.end).ok()?;
        self.doc.shaped().glyphs.slice(start..end)
    }

    pub fn line_count(self) -> usize {
        self.doc.layout_state.line_output.lines.len()
    }

    pub fn lines(self) -> RenderLines<'a> {
        RenderLines::new(&self.doc.layout_state.line_output)
    }

    /// Look up a stable line identity, rather than a spatial traversal rank.
    pub fn line(self, id: LineId) -> Option<RenderLine> {
        render_line(&self.doc.layout_state.line_output, id)
    }






}

/// Native key consumed by a baseline-oriented painter.
pub fn paint_handle(view: RenderTextView<'_>, glyph: GlyphId) -> Option<GlyphHandle> {
    view.glyph_metrics.paint_handle(glyph)
}

/// Resolved advances, vertical offsets and paint glyphs for one line.
/// The borrowed cursor allocates nothing and supports source-order reads
/// across visually reordered fragments.
pub fn line_placement<'a>(
    view: RenderTextView<'a>,
    line: LineId,
) -> Option<RenderLinePlacement<'a>> {
    let line_idx = layout_state::line_id_index(line);
    let output = &view.doc.layout_state.line_output;
    let line = output.lines.get(line_idx)?;
    let shaped = view.doc.shaped();
    Some(RenderLinePlacement {
        cursor: LineTextPlacement::new(
            view.doc.inputs.text.content(),
            shaped.glyphs,
            view.glyph_metrics,
            Some(shaped.text_geometry),
            line,
            output.line_glyph_offsets.get(line_idx).unwrap_or(&[]),
            output.line_glyph_advances.get(line_idx).unwrap_or(&[]),
        ),
    })
}

/// Stable line IDs in CSS paint order.
pub fn paint_order<'a>(view: RenderTextView<'a>) -> &'a [LineId] {
    &view.doc.layout_state.line_output.paint_order_indices
}

pub fn glyph_at(view: RenderTextView<'_>, glyph_idx: usize) -> Option<GlyphId> {
    view.doc.shaped().glyphs.get(glyph_idx).copied()
}

/// Resolved source color, independent of the retained backend glyph/run.
pub fn paint_color_at(view: RenderTextView<'_>, character_index: u32) -> Option<u32> {
    resolved_paint_color_at(view.doc.paint_colors, character_index)
}

pub fn glyph_metric(view: RenderTextView<'_>, glyph: GlyphId) -> Option<GlyphMetric> {
    view.glyph_metrics.get_checked(glyph)
}

/// Paint identity, including bidi mirroring, without changing logical text.
/// Directional controls and discretionary hyphens have no ordinary ink.
pub fn paint_glyph_at(view: RenderTextView<'_>, index: u32) -> Option<GlyphId> {
    let glyph = glyph_at(view, index as usize)?;
    let character = render_text_view_character_at(view, index as usize)?;
    if suppresses_glyph(character) {
        return None;
    }
    Some(view.doc.text_geometry().paint_glyph(index, glyph))
}

pub fn character_direction(view: RenderTextView<'_>, index: u32) -> TextDirection {
    if view.doc.text_geometry().bidi().level(index) % 2 == 1 {
        TextDirection::Rtl
    } else {
        TextDirection::Ltr
    }
}

/// Natural shaped advance for one logical character.
pub fn character_advance(view: RenderTextView<'_>, character_index: u32) -> Option<f32> {
    view.doc.text_geometry().advance(character_index as usize)
}

pub fn is_character_cluster_boundary(
    view: RenderTextView<'_>,
    character_boundary: u32,
) -> bool {
    view.doc
        .text_geometry()
        .is_cluster_boundary(character_boundary as usize)
}

pub fn authoritative_runs<'a>(
    view: RenderTextView<'a>,
    requested: Range<u32>,
) -> impl Iterator<Item = RenderAuthoritativeTextRun> + 'a {
    view.doc
        .text_geometry()
        .authoritative_runs(requested)
        .filter_map(move |run| {
            let run_range = run.run_range();
            let natural_offset =
                run.natural_offset(character_direction(view, run.source_range.start))?;
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
pub fn prepared_text_runs<'a>(
    view: RenderTextView<'a>,
    line: LineId,
) -> Option<&'a [PreparedTextRunFragment]> {
    let output = &view.doc.layout_state.line_output;
    let line = output.lines.get(layout_state::line_id_index(line))?;
    if !line.native_text_runs_complete {
        return None;
    }
    output
        .prepared_text_run_fragments
        .get(line.prepared_text_runs.start as usize..line.prepared_text_runs.end as usize)
}

/// Resolved overflow clip for the line's formatting context.
pub fn line_overflow_clip(
    view: RenderTextView<'_>,
    line: LineId,
) -> Option<RenderOverflowClip> {
    view.doc
        .layout_state
        .line_output
        .line_clips
        .get(layout_state::line_id_index(line))
        .copied()
        .flatten()
        .map(RenderOverflowClip::from_clip)
}

pub fn line_glyph_offsets<'a>(
    view: RenderTextView<'a>,
    line: LineId,
) -> Option<RenderGlyphOffsetRuns<'a>> {
    view.doc
        .layout_state
        .line_output
        .line_glyph_offsets
        .get(layout_state::line_id_index(line))
        .map(RenderGlyphOffsetRuns::new)
}

pub fn line_glyph_advances<'a>(
    view: RenderTextView<'a>,
    line: LineId,
) -> Option<RenderGlyphAdvanceRuns<'a>> {
    view.doc
        .layout_state
        .line_output
        .line_glyph_advances
        .get(layout_state::line_id_index(line))
        .map(RenderGlyphAdvanceRuns::new)
}

pub fn ellipsis_for_line(
    view: RenderTextView<'_>,
    line: LineId,
) -> Option<RenderEllipsisFragment> {
    let fragments = &view.doc.layout_state.line_output.ellipsis_fragments;
    let idx = fragments
        .binary_search_by_key(&layout_state::line_id_index(line), |fragment| fragment.line_idx)
        .ok()?;
    fragments
        .get(idx)
        .map(RenderEllipsisFragment::from_fragment)
}

pub fn hyphen_for_line(view: RenderTextView<'_>, line: LineId) -> Option<RenderHyphenFragment> {
    let fragments = &view.doc.layout_state.line_output.hyphen_fragments;
    let idx = fragments
        .binary_search_by_key(&layout_state::line_id_index(line), |fragment| fragment.line_idx)
        .ok()?;
    fragments.get(idx).map(RenderHyphenFragment::from_fragment)
}

pub fn marker_runs<'a>(view: RenderTextView<'a>) -> RenderTextRuns<'a> {
    RenderTextRuns::new(
        view.doc.inputs.text.content().inline_items(),
        TextRunKind::Marker,
    )
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

pub fn line_baseline(item: &RenderLine) -> f64 {
    item.baseline
}

pub fn line_word_spacing(item: &RenderLine) -> f64 {
    item.word_spacing
}

pub fn line_letter_spacing(item: &RenderLine) -> f64 {
    item.letter_spacing
}

/// Horizontal paint offset for optical margin alignment. Logical line
/// geometry remains anchored at [`RenderLine::point`].
pub fn line_optical_offset_x(item: &RenderLine) -> f64 {
    item.optical_offset_x
}

/// A pseudo-element paint color for this line, when one was resolved
/// after line breaking (currently `::first-line`).
pub fn line_paint_color(item: &RenderLine) -> Option<u32> {
    item.paint_color
}

pub fn line_is_in_positioned_layer(item: &RenderLine) -> bool {
    item.paint_layers.is_positioned()
}

pub fn line_is_in_negative_positioned_layer(item: &RenderLine) -> bool {
    item.paint_layers.is_negative()
}

pub fn line_is_in_independent_positioned_layer(item: &RenderLine) -> bool {
    item.paint_layers.is_independent()
}

pub fn fragment_is_rtl(item: &RenderLineTextFragment) -> bool {
    item.rtl
}

pub fn fragment_offset_x(item: &RenderLineTextFragment) -> f64 {
    item.offset_x
}

pub fn fragment_paint_order(item: &RenderLineTextFragment) -> u32 {
    item.paint_order
}

pub fn paint_resources_valid(view: RenderView<'_>) -> bool {
    view.resources
        .is_none_or(|resources| resources.paint_resources_valid())
}

pub fn retain_paint_resources(view: RenderView<'_>) -> Option<RenderResourceLease> {
    Some(RenderResourceLease {
        native: view.resources?.paint_lease(),
        _scope: view.scope_lifetime.cloned(),
    })
}

pub fn render_text_view_spatial_rank<'a>(value: RenderTextView<'a>, id: LineId) -> Option<usize> {
    value
        .doc
        .layout_state
        .line_output
        .spatial_order
        .ranks()
        .get(layout_state::line_id_index(id))
        .map(|&rank| rank as usize)
}

/// Stable line IDs in spatial order. Geometry range queries return ranks
/// into this traversal, never ranges of line identities.
pub fn render_text_view_spatial_order<'a>(value: RenderTextView<'a>) -> &'a [LineId] {
    &value.doc.layout_state.line_output.spatial_order
}

pub fn render_view_image_uri<'a>(value: &RenderView<'a>, image: ImageId) -> Option<&'a str> {
    match &render_view_images(&value)
        .get(image.index())?
        .source
    {
        ImageSource::Uri(uri) => Some(uri.as_str()),
        ImageSource::InlineSvg { .. } | ImageSource::MathMl { .. } => None,
    }
}

pub fn render_view_images<'a>(value: &RenderView<'a>) -> &'a [ImageResource] {
    value.doc.inputs.document.images()
}

pub fn render_line_glyphs(value: &RenderLine) -> Range<u32> {
    value.glyphs.clone()
}

pub fn render_lines_get<'a>(value: &RenderLines<'a>, rank: usize) -> Option<RenderLine> {
    let id = *value.output.spatial_order.get(rank)?;
    render_line(value.output, id)
}

/// The CSS canvas background color, after root/body propagation.
///
/// A root background suppresses body propagation even when its only
/// drawable layer is an image that this renderer cannot paint yet.
pub fn render_view_canvas_background_color<'a>(value: &RenderView<'a>) -> Option<u32> {
    let background = |box_idx| {
        let indices = value
            .doc
            .inputs
            .layout_tree
            .get_box_style_indices(box_idx)?;
        let style = value.doc.inputs.styles.view(indices)?;
        let color = value
            .doc
            .inputs
            .paint
            .box_colors(box_idx)
            .map_or_else(|| style.background_color(), |paint| paint.background);
        Some((style, color))
    };
    let (root, root_color) = background(value.doc.inputs.layout_tree.root_box()?)?;
    if root.background_image_present() || root_color & 0xFF != 0 {
        return (root.visibility() == Visibility::Visible && root_color & 0xFF != 0)
            .then_some(root_color);
    }

    let (body, body_color) = background(value.doc.inputs.layout_tree.body_box()?)?;
    (body.visibility() == Visibility::Visible && body_color & 0xFF != 0).then_some(body_color)
}

/// Preserved source for equation-specific actions; ordinary images return None.
pub fn render_view_image_mathml<'a>(value: &RenderView<'a>, image: ImageId) -> Option<&'a str> {
    match &render_view_images(&value)
        .get(image.index())?
        .source
    {
        ImageSource::MathMl { markup, .. } => Some(markup),
        _ => None,
    }
}

/// Block box that emitted this line.
pub fn render_line_owner_box(value: &RenderLine) -> BoxId {
    layout_state::box_id_new(value.owner_box_idx)
}

pub fn render_text_run_is_marker(value: &RenderTextRun) -> bool {
    matches!(value.kind, TextRunKind::Marker)
}

pub fn render_text_run_box_id(value: &RenderTextRun) -> BoxId {
    layout_state::box_id_new(value.box_idx)
}

pub fn render_text_view_text_runs<'a>(value: RenderTextView<'a>) -> RenderTextRuns<'a> {
    text_runs(value.doc.inputs.text.content().inline_items())
}

/// Returns the line that owns a source glyph.
#[inline]
pub fn render_text_view_line_for_glyph<'a>(
    value: RenderTextView<'a>,
    glyph_idx: u32,
) -> Option<LineId> {
    value
        .doc
        .layout_state
        .line_output
        .glyph_line_indices
        .get(glyph_idx as usize)
        .map(LineId::new)
}

/// Source-backed text in logical order, excluding generated and hidden text.
/// Descendants with `visibility: visible` remain available under hidden parents.
pub fn render_text_view_visible_source_runs<'a>(
    value: RenderTextView<'a>,
) -> impl Iterator<Item = RenderTextRun> + 'a {
    render_text_view_text_runs(value).filter(move |run| {
        run.dom_text_node().is_some()
            && value
                .doc
                .box_is_visible(layout_state::box_id_index(render_text_run_box_id(&run)))
    })
}



/// Prepared logical character, including text transforms and generated
/// content. Bidi mirroring affects paint identity rather than this text.
pub fn render_text_view_character_at<'a>(value: RenderTextView<'a>, index: usize) -> Option<char> {
    value.doc.inputs.text.content().character_at(index)
}

pub fn render_text_view_line_text_fragments<'a>(
    value: RenderTextView<'a>,
    line: LineId,
) -> Option<RenderLineTextFragments<'a>> {
    let line = value
        .doc
        .layout_state
        .line_output
        .lines
        .get(layout_state::line_id_index(line))?;
    Some(RenderLineTextFragments {
        fragments: line.text_fragments(
            &value.doc.layout_state.line_output.line_details,
            &value.doc.layout_state.line_output.text_fragments,
        ),
    })
}
