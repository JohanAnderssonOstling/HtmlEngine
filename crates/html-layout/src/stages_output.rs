use super::{AncestorIter, LaidOutDocument};
use crate::layout_model::{AnchorPosition, DecorationFragment, EllipsisFragment, GlyphAdvanceRun, GlyphId, GlyphOffsetRun, HyphenFragment, ImageFragment, InlineItem, InlineItemKind, LayoutMode, Line, ListItemMarker, RoundedDecoration};
use html_dom::{Document, ImageResource, NodeRef};
use html_style_model::{ListStylePosition, TextDecorationLines, UsedBorderRadii};
use kurbo::{Point, Size};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RenderForcedBreak {
    Column,
    Page,
}

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

#[derive(Clone, Debug, Default)]
pub struct ImageMetrics {
    sizes: Vec<(u32, u32)>,
}

impl ImageMetrics {
    pub(super) fn from_document(document: &Document) -> Self {
        let mut sizes = Vec::with_capacity(document.images().len());
        for image in document.images() {
            sizes.push((image.width, image.height));
        }
        Self { sizes }
    }

    pub fn set(&mut self, image_idx: u32, width: u32, height: u32) {
        let idx = image_idx as usize;
        if self.sizes.len() <= idx {
            self.sizes.resize(idx + 1, (0, 0));
        }
        self.sizes[idx] = (width, height);
    }

    pub fn get(&self, image_idx: u32) -> Option<(u32, u32)> {
        self.sizes.get(image_idx as usize).copied()
    }

    pub(crate) fn display_size(&self, image_idx: u32, resource: &ImageResource) -> (f64, f64) {
        let (width, height) = self.get(image_idx).unwrap_or((resource.width, resource.height));
        // HTML dimension attributes are CSS presentational hints, not natural
        // image dimensions. They are resolved by the style/layout sizing path;
        // folding them into the decoded metrics here would also change the
        // intrinsic aspect ratio and apply the same author input twice.
        (width.max(1) as f64, height.max(1) as f64)
    }

    pub(super) fn memory_usage_bytes(&self) -> usize {
        self.sizes.capacity() * std::mem::size_of::<(u32, u32)>()
    }

    pub(super) fn len(&self) -> usize {
        self.sizes.len()
    }
}

#[cfg(test)]
mod image_metrics_tests {
    use super::ImageMetrics;
    use html_dom::{ImageResource, ImageSource};

    #[test]
    fn html_dimensions_do_not_replace_decoded_intrinsic_metrics() {
        let resource = ImageResource { source: ImageSource::Uri("image.png".to_owned()), width: 320, height: 180, width_attr: Some(10), height_attr: Some(20) };
        let mut metrics = ImageMetrics::default();

        assert_eq!(metrics.display_size(0, &resource), (320.0, 180.0));
        metrics.set(0, 640, 360);
        assert_eq!(metrics.display_size(0, &resource), (640.0, 360.0));
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
pub struct RenderDecoration {
    rect: kurbo::Rect,
    color: u32,
    is_inline: bool,
    radii: Option<UsedBorderRadii>,
    border_width: Option<f32>,
    is_foreground: bool,
    is_background: bool,
    is_border: bool,
    pattern: RenderDecorationPattern,
    clip: Option<RenderOverflowClip>,
    positioned_layer: bool,
    negative_positioned_layer: bool,
    independent_positioned_layer: bool,
    line_idx: Option<usize>,
    paint_order: u32,
}

/// Backend-neutral execution style for a resolved decoration span. Pattern
/// tessellation is deliberately deferred to `html-render-core`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RenderDecorationPattern {
    #[default]
    Solid,
    DoubleHorizontal,
    DottedHorizontal,
    DashedHorizontal,
    DottedVertical,
    DashedVertical,
}

/// Renderer-facing overflow clip with independent physical axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOverflowClip {
    rect: kurbo::Rect,
    clip_x: bool,
    clip_y: bool,
}

impl RenderOverflowClip {
    fn from_clip(clip: crate::layout_model::OverflowClip) -> Self {
        Self { rect: clip.rect, clip_x: clip.x, clip_y: clip.y }
    }

    pub fn rect(self) -> kurbo::Rect {
        self.rect
    }

    pub fn clips_x(self) -> bool {
        self.clip_x
    }

    pub fn clips_y(self) -> bool {
        self.clip_y
    }
}

impl RenderDecoration {
    fn from_fragment(
        fragment: &DecorationFragment, rounded: &[RoundedDecoration], clip: Option<crate::layout_model::OverflowClip>, positioned_layer: bool, negative_positioned_layer: bool, independent_positioned_layer: bool, line_idx: Option<u32>,
        paint_order: u32,
    ) -> Self {
        let rounded = fragment.rounded_index().and_then(|index| rounded.get(index));
        Self {
            rect: fragment.rect,
            color: fragment.color,
            is_inline: fragment.is_inline(),
            radii: rounded.map(|value| value.radii),
            border_width: rounded.and_then(|value| value.border_width),
            is_foreground: fragment.is_foreground(),
            is_background: fragment.is_background(),
            is_border: fragment.is_border(),
            pattern: match fragment.pattern() {
                crate::layout_model::DecorationPattern::Solid => RenderDecorationPattern::Solid,
                crate::layout_model::DecorationPattern::DoubleHorizontal => RenderDecorationPattern::DoubleHorizontal,
                crate::layout_model::DecorationPattern::DottedHorizontal => RenderDecorationPattern::DottedHorizontal,
                crate::layout_model::DecorationPattern::DashedHorizontal => RenderDecorationPattern::DashedHorizontal,
                crate::layout_model::DecorationPattern::DottedVertical => RenderDecorationPattern::DottedVertical,
                crate::layout_model::DecorationPattern::DashedVertical => RenderDecorationPattern::DashedVertical,
            },
            clip: clip.map(RenderOverflowClip::from_clip),
            positioned_layer,
            negative_positioned_layer,
            independent_positioned_layer,
            line_idx: line_idx.filter(|idx| *idx != u32::MAX).map(|idx| idx as usize),
            paint_order,
        }
    }

    pub fn rect(&self) -> kurbo::Rect {
        self.rect
    }

    pub fn color(&self) -> u32 {
        self.color
    }

    pub fn is_inline(&self) -> bool {
        self.is_inline
    }

    pub fn radii(&self) -> Option<UsedBorderRadii> {
        self.radii
    }

    pub fn border_width(&self) -> Option<f32> {
        self.border_width
    }

    pub fn is_foreground(&self) -> bool {
        self.is_foreground
    }

    pub fn is_background(&self) -> bool {
        self.is_background
    }

    pub fn is_border(&self) -> bool {
        self.is_border
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

    pub fn line_idx(&self) -> Option<usize> {
        self.line_idx
    }

    pub fn paint_order(&self) -> u32 {
        self.paint_order
    }

    pub fn pattern(&self) -> RenderDecorationPattern {
        self.pattern
    }

    /// Resolved padding-box clip contributed by overflow ancestors.
    pub fn overflow_clip(&self) -> Option<RenderOverflowClip> {
        self.clip
    }
}

pub struct RenderDecorations<'a> {
    decorations: &'a [DecorationFragment],
    rounded: &'a [RoundedDecoration],
    clips: &'a [Option<crate::layout_model::OverflowClip>],
    block_paint_ranges: &'a [Range<u32>],
    block_decoration_count: usize,
    positioned_layers: &'a [bool],
    negative_positioned_layers: &'a [bool],
    independent_positioned_layers: &'a [bool],
    line_indices: &'a [u32],
    paint_orders: &'a [u32],
}

impl<'a> RenderDecorations<'a> {
    fn new(
        decorations: &'a [DecorationFragment], rounded: &'a [RoundedDecoration], clips: &'a [Option<crate::layout_model::OverflowClip>], block_paint_ranges: &'a [Range<u32>], block_decoration_count: usize, positioned_layers: &'a [bool],
        negative_positioned_layers: &'a [bool], independent_positioned_layers: &'a [bool], line_indices: &'a [u32], paint_orders: &'a [u32],
    ) -> Self {
        Self { decorations, rounded, clips, block_paint_ranges, block_decoration_count, positioned_layers, negative_positioned_layers, independent_positioned_layers, line_indices, paint_orders }
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderDecoration> + '_ {
        let render = |index: usize| {
            RenderDecoration::from_fragment(
                &self.decorations[index],
                self.rounded,
                self.clips.get(index).copied().flatten(),
                self.positioned_layers.get(index).copied().unwrap_or(false),
                self.negative_positioned_layers.get(index).copied().unwrap_or(false),
                self.independent_positioned_layers.get(index).copied().unwrap_or(false),
                self.line_indices.get(index).copied(),
                self.paint_orders.get(index).copied().unwrap_or(u32::MAX),
            )
        };
        self.block_paint_ranges.iter().flat_map(|range| range.clone().map(|index| index as usize)).map(render).chain((self.block_decoration_count..self.decorations.len()).map(render))
    }

    pub fn len(&self) -> usize {
        self.decorations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.decorations.is_empty()
    }
}

pub struct RenderLineDecorations<'a> {
    decorations: &'a [DecorationFragment],
    rounded: &'a [RoundedDecoration],
    clips: &'a [Option<crate::layout_model::OverflowClip>],
    positioned_layers: &'a [bool],
    negative_positioned_layers: &'a [bool],
    independent_positioned_layers: &'a [bool],
    line_indices: &'a [u32],
    paint_orders: &'a [u32],
    indexes: &'a [usize],
}

impl<'a> RenderLineDecorations<'a> {
    fn new(
        decorations: &'a [DecorationFragment], rounded: &'a [RoundedDecoration], clips: &'a [Option<crate::layout_model::OverflowClip>], positioned_layers: &'a [bool], negative_positioned_layers: &'a [bool],
        independent_positioned_layers: &'a [bool], line_indices: &'a [u32], paint_orders: &'a [u32], indexes: &'a [usize],
    ) -> Self {
        Self { decorations, rounded, clips, positioned_layers, negative_positioned_layers, independent_positioned_layers, line_indices, paint_orders, indexes }
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderDecoration> + '_ {
        self.indexes.iter().filter_map(move |&index| {
            self.decorations.get(index).map(|fragment| {
                RenderDecoration::from_fragment(
                    fragment,
                    self.rounded,
                    self.clips.get(index).copied().flatten(),
                    self.positioned_layers.get(index).copied().unwrap_or(false),
                    self.negative_positioned_layers.get(index).copied().unwrap_or(false),
                    self.independent_positioned_layers.get(index).copied().unwrap_or(false),
                    self.line_indices.get(index).copied(),
                    self.paint_orders.get(index).copied().unwrap_or(u32::MAX),
                )
            })
        })
    }

    pub fn is_empty(&self) -> bool {
        self.indexes.is_empty()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RenderImageFragment {
    line_idx: usize,
    image_idx: u32,
    offset: Point,
    size: Size,
    paint_order: u32,
}

impl RenderImageFragment {
    fn from_fragment(fragment: &ImageFragment) -> Self {
        Self { line_idx: fragment.line_idx, image_idx: fragment.image_idx, offset: fragment.offset, size: fragment.size, paint_order: fragment.paint_order }
    }

    pub fn line_idx(&self) -> usize {
        self.line_idx
    }

    pub fn image_idx(&self) -> u32 {
        self.image_idx
    }

    pub fn offset(&self) -> Point {
        self.offset
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn paint_order(&self) -> u32 {
        self.paint_order
    }
}

pub struct RenderImageFragments<'a> {
    fragments: &'a [ImageFragment],
    indexes: &'a [usize],
}

impl<'a> RenderImageFragments<'a> {
    fn new(fragments: &'a [ImageFragment], indexes: &'a [usize]) -> Self {
        Self { fragments, indexes }
    }

    pub fn len(&self) -> usize {
        self.indexes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.indexes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderImageFragment> + '_ {
        self.indexes.iter().filter_map(move |&idx| self.fragments.get(idx).map(RenderImageFragment::from_fragment))
    }
}

pub struct RenderAllImageFragments<'a> {
    fragments: &'a [ImageFragment],
}

impl<'a> RenderAllImageFragments<'a> {
    fn new(fragments: &'a [ImageFragment]) -> Self {
        Self { fragments }
    }

    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderImageFragment> + '_ {
        self.fragments.iter().map(RenderImageFragment::from_fragment)
    }
}

/// Stable semantic text-run view. Its glyph range is layout-owned and cannot
/// be forged by renderer crates.
///
/// ```compile_fail
/// let _ = html_layout::RenderTextRun {
///     kind: (),
///     box_idx: 0,
///     dom_text_node: None,
///     glyphs: 5..1,
/// };
/// ```
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
                InlineItemKind::Text { glyphs } => return Some(RenderTextRun { kind: TextRunKind::Text, box_idx: run.box_idx as usize, dom_text_node: run.dom_text_node, glyphs: glyphs.clone() }),
                InlineItemKind::Marker { glyphs } => return Some(RenderTextRun { kind: TextRunKind::Marker, box_idx: run.box_idx as usize, dom_text_node: run.dom_text_node, glyphs: glyphs.clone() }),
                _ => continue,
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RenderListItemMarker {
    marker_box: u32,
    position: ListStylePosition,
}

impl RenderListItemMarker {
    pub(super) fn from_model(marker: ListItemMarker) -> Self {
        Self { marker_box: marker.marker_box, position: marker.position }
    }

    pub fn marker_box(self) -> usize {
        self.marker_box as usize
    }

    pub fn position(self) -> ListStylePosition {
        self.position
    }
}

pub struct RenderAnchorPosition {
    y: f64,
    order: u32,
}

impl RenderAnchorPosition {
    fn new(y: f64, order: u32) -> Self {
        Self { y, order }
    }

    pub fn y(&self) -> f64 {
        self.y
    }

    pub fn order(&self) -> u32 {
        self.order
    }
}

pub struct RenderAnchorPositions<'a> {
    anchors: &'a FxHashMap<u16, AnchorPosition>,
}

impl<'a> RenderAnchorPositions<'a> {
    fn new(anchors: &'a FxHashMap<u16, AnchorPosition>) -> Self {
        Self { anchors }
    }

    pub fn get(&self, id_idx: u16) -> Option<RenderAnchorPosition> {
        self.anchors.get(&id_idx).copied().map(|pos| RenderAnchorPosition::new(pos.y, pos.order))
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, RenderAnchorPosition)> + '_ {
        self.anchors.iter().map(|(&id_idx, &pos)| (id_idx, RenderAnchorPosition::new(pos.y, pos.order)))
    }
}

#[derive(Clone, Copy)]
pub struct RenderView<'a> {
    pub(super) doc: &'a LaidOutDocument,
}

/// Text and line-layout queries. This capability deliberately excludes box
/// topology, resolved decorations, and document source details.
#[derive(Clone, Copy)]
pub struct RenderTextView<'a> {
    doc: &'a LaidOutDocument,
}

/// Used box geometry and semantic box queries.
#[derive(Clone, Copy)]
pub struct RenderBoxView<'a> {
    doc: &'a LaidOutDocument,
}

/// Semantic and geometric data for one laid-out HTML table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderTable {
    box_idx: usize,
    rows: Vec<RenderTableRow>,
    column_count: usize,
    authored_html: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderTableRow {
    cells: Vec<RenderTableCell>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderTableCell {
    box_idx: usize,
    row: usize,
    column: usize,
    rowspan: usize,
    colspan: usize,
    header: bool,
    text: String,
}

impl RenderTable {
    pub fn box_idx(&self) -> usize {
        self.box_idx
    }

    pub fn rows(&self) -> &[RenderTableRow] {
        &self.rows
    }

    pub fn column_count(&self) -> usize {
        self.column_count
    }

    /// Sanitized authored markup for rich table export. This preserves
    /// classes, inline styles, and structural attributes while dropping event
    /// handlers; consumers that want portable structure without styling can
    /// generate it from `rows()` instead.
    pub fn authored_html(&self) -> &str {
        &self.authored_html
    }
}

impl RenderTableRow {
    pub fn cells(&self) -> &[RenderTableCell] {
        &self.cells
    }
}

impl RenderTableCell {
    pub fn box_idx(&self) -> usize {
        self.box_idx
    }

    pub fn row(&self) -> usize {
        self.row
    }

    pub fn column(&self) -> usize {
        self.column
    }

    pub fn rowspan(&self) -> usize {
        self.rowspan
    }

    pub fn colspan(&self) -> usize {
        self.colspan
    }

    pub fn is_header(&self) -> bool {
        self.header
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Backend-neutral resolved decoration, clip, and image fragments.
#[derive(Clone, Copy)]
pub struct RenderFragmentView<'a> {
    doc: &'a LaidOutDocument,
}

/// Stable link, anchor, and source-correlation queries.
#[derive(Clone, Copy)]
pub struct RenderAddressingView<'a> {
    doc: &'a LaidOutDocument,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceElementStep<'a> {
    step: usize,
    id: Option<&'a str>,
}

impl<'a> SourceElementStep<'a> {
    pub fn step(self) -> usize {
        self.step
    }

    pub fn id(self) -> Option<&'a str> {
        self.id
    }
}

/// Generic source position corresponding to a laid-out glyph boundary.
/// Publication-specific syntaxes such as EPUB CFI are built outside layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePosition<'a> {
    elements: Vec<SourceElementStep<'a>>,
    text_step: usize,
    utf16_offset: usize,
}

impl<'a> SourcePosition<'a> {
    pub fn elements(&self) -> impl ExactSizeIterator<Item = SourceElementStep<'a>> + '_ {
        self.elements.iter().copied()
    }

    pub fn text_step(&self) -> usize {
        self.text_step
    }

    pub fn utf16_offset(&self) -> usize {
        self.utf16_offset
    }
}

impl<'a> RenderView<'a> {
    pub fn text(self) -> RenderTextView<'a> {
        RenderTextView { doc: self.doc }
    }

    pub fn boxes(self) -> RenderBoxView<'a> {
        RenderBoxView { doc: self.doc }
    }

    pub fn fragments(self) -> RenderFragmentView<'a> {
        RenderFragmentView { doc: self.doc }
    }

    pub fn addressing(self) -> RenderAddressingView<'a> {
        RenderAddressingView { doc: self.doc }
    }

    pub fn root_font_size(&self) -> f32 {
        self.doc.root_font_size()
    }

    /// The CSS canvas background color, after root/body propagation.
    ///
    /// A root background suppresses body propagation even when its only
    /// drawable layer is an image that this renderer cannot paint yet.
    pub fn canvas_background_color(&self) -> Option<u32> {
        let root_box = self.doc.inputs.layout_tree.root_box()?;
        let root_indices = self.doc.box_style_indices(root_box)?;
        let root = self.doc.inputs.styles.view(root_indices)?;
        if root.background_image_present() || root.background_color() & 0xFF != 0 {
            return (root.background_color() & 0xFF != 0).then_some(root.background_color());
        }

        let body_box = self.doc.inputs.layout_tree.body_box()?;
        let body_indices = self.doc.box_style_indices(body_box)?;
        let body = self.doc.inputs.styles.view(body_indices)?;
        (body.background_color() & 0xFF != 0).then_some(body.background_color())
    }

    pub fn title(&self) -> Option<&'a str> {
        self.doc.title()
    }

    pub fn images(&self) -> &'a [html_dom::ImageResource] {
        self.doc.images()
    }

    pub fn image_uri(&self, image_idx: u32) -> Option<&'a str> {
        match &self.doc.images().get(image_idx as usize)?.source {
            html_dom::ImageSource::Uri(uri) => Some(uri.as_str()),
            html_dom::ImageSource::Inline(_) => None,
        }
    }

    pub fn document_toc_entries(&self) -> &'a [html_dom::DocumentTocNode] {
        self.doc.document_toc_entries()
    }

    pub fn string(&self, index: u16) -> &'a str {
        self.doc.string(index)
    }

    pub fn style_string(&self, index: html_style_model::StyleStringId) -> Option<&'a str> {
        self.doc.inputs.styles.string(index)
    }

    pub fn lookup_string(&self, value: &str) -> Option<u16> {
        self.doc.lookup_string(value)
    }
}

impl<'a> RenderTextView<'a> {
    /// Spatial line indexes in CSS paint order.
    pub fn paint_order_indices(self) -> &'a [u32] {
        &self.doc.layout_state.line_output.paint_order_indices
    }

    pub fn glyph_count(self) -> usize {
        self.doc.glyphs().len()
    }

    pub fn glyph_at(self, glyph_idx: usize) -> Option<GlyphId> {
        self.doc.glyph_at(glyph_idx)
    }

    pub fn glyph_slice(self, range: Range<u32>) -> Option<&'a [GlyphId]> {
        let start = usize::try_from(range.start).ok()?;
        let end = usize::try_from(range.end).ok()?;
        self.doc.glyphs().get(start..end)
    }

    pub fn glyph_metric(self, glyph: GlyphId) -> Option<crate::GlyphMetric> {
        self.doc.glyph_metrics().get_checked(glyph)
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
        self.doc.line_count()
    }

    pub fn lines(self) -> RenderLines<'a> {
        RenderLines::new(self.doc.lines(), &self.doc.layout_state.line_output.positioned_layers, &self.doc.layout_state.line_output.negative_positioned_layers, &self.doc.layout_state.line_output.independent_positioned_layers)
    }

    pub fn line(self, idx: usize) -> Option<RenderLine> {
        self.doc.line(idx).map(|line| {
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
        self.doc.line_glyph_offsets().get(line_idx).map(|runs| RenderGlyphOffsetRuns::new(runs))
    }

    pub fn line_glyph_advances(self, line_idx: usize) -> Option<RenderGlyphAdvanceRuns<'a>> {
        self.doc.line_glyph_advances().get(line_idx).map(|runs| RenderGlyphAdvanceRuns::new(runs))
    }

    pub fn line_text_fragments(self, line_idx: usize) -> Option<RenderLineTextFragments<'a>> {
        let line = self.doc.lines().get(line_idx)?;
        Some(RenderLineTextFragments { line, index: 0, implicit_emitted: false })
    }

    pub fn ellipsis_for_line(self, line_idx: usize) -> Option<RenderEllipsisFragment> {
        let fragments = self.doc.ellipsis_fragments();
        let idx = fragments.binary_search_by_key(&line_idx, |fragment| fragment.line_idx).ok()?;
        fragments.get(idx).map(RenderEllipsisFragment::from_fragment)
    }

    pub fn hyphen_for_line(self, line_idx: usize) -> Option<RenderHyphenFragment> {
        let fragments = self.doc.hyphen_fragments();
        let idx = fragments.binary_search_by_key(&line_idx, |fragment| fragment.line_idx).ok()?;
        fragments.get(idx).map(RenderHyphenFragment::from_fragment)
    }

    pub fn text_runs(self) -> RenderTextRuns<'a> {
        RenderTextRuns::new(self.doc.inline_items(), Some(TextRunKind::Text))
    }

    pub fn marker_runs(self) -> RenderTextRuns<'a> {
        RenderTextRuns::new(self.doc.inline_items(), Some(TextRunKind::Marker))
    }
}

impl<'a> RenderBoxView<'a> {
    pub fn len(self) -> usize {
        self.doc.box_count()
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn tag(self, box_idx: usize) -> Option<&'a str> {
        self.doc.inputs.layout_tree.box_at(box_idx).and_then(|layout_box| layout_box.get_element(self.doc.document()).map(|element| element.tag()))
    }

    pub fn attribute(self, box_idx: usize, name: &str) -> Option<&'a str> {
        self.doc.inputs.layout_tree.box_at(box_idx).and_then(|layout_box| layout_box.get_element(self.doc.document()).and_then(|element| element.attr(name)))
    }

    pub fn attribute_expanded(self, box_idx: usize, namespace: Option<&str>, local_name: &str) -> Option<&'a str> {
        self.doc.inputs.layout_tree.box_at(box_idx).and_then(|layout_box| layout_box.get_element(self.doc.document()).and_then(|element| element.attr_expanded(namespace, local_name)))
    }

    pub fn id(self, box_idx: usize) -> Option<u16> {
        self.doc.inputs.layout_tree.box_at(box_idx)?.get_element(self.doc.document()).and_then(|element| element.id_idx())
    }

    pub fn dom_node_index(self, box_idx: usize) -> Option<usize> {
        self.doc.box_dom_element_idx(box_idx).map(|node_idx| node_idx as usize)
    }

    pub fn href(self, box_idx: usize) -> Option<u16> {
        let href_name = self.doc.document().lookup_string("href")?;
        self.doc.inputs.layout_tree.box_at(box_idx).and_then(|layout_box| layout_box.get_element(self.doc.document()).and_then(|element| element.attr_value_idx_no_namespace(href_name)))
    }

    pub fn point(self, box_idx: usize) -> Option<Point> {
        (box_idx < self.doc.inputs.layout_tree.box_count()).then(|| self.doc.geometry.point(box_idx))
    }

    pub fn size(self, box_idx: usize) -> Option<Size> {
        (box_idx < self.doc.inputs.layout_tree.box_count()).then(|| self.doc.geometry.size(box_idx))
    }

    pub fn is_table(self, box_idx: usize) -> bool {
        matches!(self.doc.box_layout_mode(box_idx), Some(LayoutMode::Table(_)))
    }

    /// Returns the source table's logical grid, retaining the laid-out box for
    /// every origin cell so renderers can hit-test and paint selections.
    pub fn table(self, box_idx: usize) -> Option<RenderTable> {
        if !self.is_table(box_idx) {
            return None;
        }
        let document = self.doc.document();
        let table_element = self.doc.inputs.layout_tree.box_at(box_idx)?.get_element(document)?;
        if !table_element.tag().eq_ignore_ascii_case("table") {
            return None;
        }

        fn escape_html(value: &str, attribute: bool) -> String {
            let mut escaped = String::with_capacity(value.len());
            for character in value.chars() {
                match character {
                    '&' => escaped.push_str("&amp;"),
                    '<' => escaped.push_str("&lt;"),
                    '>' => escaped.push_str("&gt;"),
                    '"' if attribute => escaped.push_str("&quot;"),
                    '\'' if attribute => escaped.push_str("&#39;"),
                    _ => escaped.push(character),
                }
            }
            escaped
        }

        fn serialize_authored_html(document: &Document, node: html_dom::DomNodeId, output: &mut String) {
            match document.node_ref(node) {
                Some(NodeRef::Text(text)) => output.push_str(&escape_html(text.text(), false)),
                Some(NodeRef::Element(element)) => {
                    let tag = element.tag();
                    output.push('<');
                    output.push_str(tag);
                    for attribute in element.attributes() {
                        let name = attribute.name();
                        if name.get(..2).is_some_and(|prefix| prefix.eq_ignore_ascii_case("on")) {
                            continue;
                        }
                        output.push(' ');
                        output.push_str(name);
                        output.push_str("=\"");
                        output.push_str(&escape_html(attribute.value(), true));
                        output.push('"');
                    }
                    output.push('>');
                    for child in element.children() {
                        serialize_authored_html(document, child, output);
                    }
                    output.push_str("</");
                    output.push_str(tag);
                    output.push('>');
                }
                None => {}
            }
        }

        fn collect_rows(document: &Document, node: html_dom::DomNodeId, root_table: html_dom::DomNodeId, rows: &mut Vec<html_dom::DomNodeId>) {
            let Some(element) = document.element_ref(node) else { return };
            for child in element.children() {
                let Some(child_element) = document.element_ref(child) else { continue };
                if child_element.tag().eq_ignore_ascii_case("table") && child != root_table {
                    continue;
                }
                if child_element.tag().eq_ignore_ascii_case("tr") {
                    rows.push(child);
                } else {
                    collect_rows(document, child, root_table, rows);
                }
            }
        }

        fn collect_text(document: &Document, node: html_dom::DomNodeId, text: &mut String) {
            match document.node_ref(node) {
                Some(NodeRef::Text(value)) => {
                    text.push_str(value.text());
                    text.push(' ');
                }
                Some(NodeRef::Element(element)) => {
                    if element.tag().eq_ignore_ascii_case("table") {
                        return;
                    }
                    if element.tag().eq_ignore_ascii_case("br") {
                        text.push(' ');
                    }
                    for child in element.children() {
                        collect_text(document, child, text);
                    }
                }
                None => {}
            }
        }

        let mut box_for_node = FxHashMap::default();
        for candidate in 0..self.len() {
            if let Some(node_idx) = self.dom_node_index(candidate) {
                box_for_node.entry(node_idx).or_insert(candidate);
            }
        }

        let table_node = table_element.node_id();
        let mut authored_html = String::new();
        serialize_authored_html(document, table_node, &mut authored_html);
        let mut row_nodes = Vec::new();
        collect_rows(document, table_node, table_node, &mut row_nodes);
        let mut occupied_until = Vec::<usize>::new();
        let mut rows = Vec::with_capacity(row_nodes.len());
        let mut column_count = 0;
        for (row, row_node) in row_nodes.into_iter().enumerate() {
            let row_element = document.element_ref(row_node)?;
            let mut column = 0;
            let mut cells = Vec::new();
            for cell_node in row_element.children() {
                let Some(cell_element) = document.element_ref(cell_node) else { continue };
                let header = cell_element.tag().eq_ignore_ascii_case("th");
                if !header && !cell_element.tag().eq_ignore_ascii_case("td") {
                    continue;
                }
                while occupied_until.get(column).is_some_and(|&until| until > row) {
                    column += 1;
                }
                let rowspan = cell_element.attr("rowspan").and_then(|value| value.parse::<usize>().ok()).unwrap_or(1).max(1);
                let colspan = cell_element.attr("colspan").and_then(|value| value.parse::<usize>().ok()).unwrap_or(1).max(1);
                if occupied_until.len() < column + colspan {
                    occupied_until.resize(column + colspan, 0);
                }
                for occupied in &mut occupied_until[column..column + colspan] {
                    *occupied = row.saturating_add(rowspan);
                }
                let Some(&cell_box) = box_for_node.get(&cell_element.node_idx()) else {
                    column += colspan;
                    continue;
                };
                let mut raw_text = String::new();
                for child in cell_element.children() {
                    collect_text(document, child, &mut raw_text);
                }
                let text = raw_text.split_whitespace().collect::<Vec<_>>().join(" ");
                cells.push(RenderTableCell { box_idx: cell_box, row, column, rowspan, colspan, header, text });
                column += colspan;
            }
            column_count = column_count.max(occupied_until.len()).max(column);
            rows.push(RenderTableRow { cells });
        }
        (!rows.is_empty() && column_count > 0).then_some(RenderTable { box_idx, rows, column_count, authored_html })
    }

    pub fn is_table_row(self, box_idx: usize) -> bool {
        matches!(self.doc.box_layout_mode(box_idx), Some(LayoutMode::TableRow(_)))
    }

    pub fn is_table_cell(self, box_idx: usize) -> bool {
        matches!(self.doc.box_layout_mode(box_idx), Some(LayoutMode::TableCell(_)))
    }

    pub fn is_table_header_group(self, box_idx: usize) -> bool {
        self.used_style(box_idx).is_some_and(|style| style.display() == html_style_model::Display::TableHeaderGroup)
    }

    pub fn table_cell_rowspan(self, box_idx: usize) -> usize {
        match self.doc.box_layout_mode(box_idx) {
            Some(LayoutMode::TableCell(cell)) => cell.rowspan.max(1),
            _ => 1,
        }
    }

    pub fn forces_break_before(self, box_idx: usize) -> bool {
        self.used_style(box_idx).is_some_and(|style| style.break_before().is_forced())
    }

    pub fn forces_break_after(self, box_idx: usize) -> bool {
        self.used_style(box_idx).is_some_and(|style| style.break_after().is_forced())
    }

    pub fn avoids_break_before(self, box_idx: usize) -> bool {
        self.used_style(box_idx).is_some_and(|style| style.break_before() == html_style_model::BreakBetween::Avoid)
    }

    pub fn avoids_break_after(self, box_idx: usize) -> bool {
        self.used_style(box_idx).is_some_and(|style| style.break_after() == html_style_model::BreakBetween::Avoid)
    }

    pub fn break_before(self, box_idx: usize) -> html_style_model::BreakBetween {
        self.used_style(box_idx).map_or(html_style_model::BreakBetween::Auto, |style| style.break_before())
    }

    pub fn forced_break_before(self, box_idx: usize) -> Option<RenderForcedBreak> {
        render_forced_break(self.break_before(box_idx))
    }

    pub fn forced_break_after(self, box_idx: usize) -> Option<RenderForcedBreak> {
        render_forced_break(self.used_style(box_idx).map_or(html_style_model::BreakBetween::Auto, |style| style.break_after()))
    }

    pub fn avoids_break_inside(self, box_idx: usize) -> bool {
        self.used_style(box_idx).is_some_and(|style| style.break_inside() == html_style_model::BreakInside::Avoid)
    }

    pub fn widows(self, box_idx: usize) -> usize {
        self.used_style(box_idx).map_or(2, |style| style.widows() as usize)
    }

    pub fn orphans(self, box_idx: usize) -> usize {
        self.used_style(box_idx).map_or(2, |style| style.orphans() as usize)
    }

    pub fn parent(self, box_idx: usize) -> Option<usize> {
        self.doc.inputs.layout_tree.box_at(box_idx).and_then(|layout_box| layout_box.parent().map(|parent| parent as usize))
    }

    pub fn text_format(self, box_idx: usize) -> BoxTextFormat {
        self.doc.box_text_format(box_idx)
    }

    pub fn list_marker(self, box_idx: usize) -> Option<RenderListItemMarker> {
        self.doc.list_marker(box_idx)
    }

    pub fn is_block_container(self, box_idx: usize) -> bool {
        self.doc.is_block_container_box(box_idx)
    }

    pub fn ancestors(self, box_idx: usize) -> impl Iterator<Item = usize> + 'a {
        AncestorIter { doc: self.doc, current: self.parent(box_idx) }
    }

    fn used_style(self, box_idx: usize) -> Option<html_style_model::UsedStyleView<'a>> {
        self.doc.box_used_style(box_idx)
    }
}

fn render_forced_break(value: html_style_model::BreakBetween) -> Option<RenderForcedBreak> {
    match value {
        html_style_model::BreakBetween::Column => Some(RenderForcedBreak::Column),
        html_style_model::BreakBetween::Page => Some(RenderForcedBreak::Page),
        html_style_model::BreakBetween::Auto | html_style_model::BreakBetween::Avoid => None,
    }
}

impl<'a> RenderFragmentView<'a> {
    pub fn decorations(self) -> RenderDecorations<'a> {
        RenderDecorations::new(
            self.doc.decorations(),
            self.doc.rounded_decorations(),
            &self.doc.layout_state.fragment_output.decoration_clips,
            &self.doc.layout_state.fragment_output.block_paint_ranges,
            self.doc.layout_state.fragment_output.block_decoration_count as usize,
            &self.doc.layout_state.fragment_output.decoration_positioned_layers,
            &self.doc.layout_state.fragment_output.decoration_negative_positioned_layers,
            &self.doc.layout_state.fragment_output.decoration_independent_positioned_layers,
            &self.doc.layout_state.fragment_output.decoration_line_indices,
            &self.doc.layout_state.fragment_output.decoration_paint_orders,
        )
    }

    pub fn images_for_line(self, line_idx: usize) -> RenderImageFragments<'a> {
        let indexes = self.doc.image_fragments_by_line().get(line_idx).map_or(&[] as &'a [usize], |indexes| indexes.as_slice());
        RenderImageFragments::new(self.doc.image_fragments(), indexes)
    }

    pub fn decorations_for_line(self, line_idx: usize) -> RenderLineDecorations<'a> {
        let indexes = self.doc.layout_state.fragment_output.decoration_fragments_by_line.get(line_idx).map_or(&[] as &'a [usize], |indexes| indexes.as_slice());
        RenderLineDecorations::new(
            self.doc.decorations(),
            self.doc.rounded_decorations(),
            &self.doc.layout_state.fragment_output.decoration_clips,
            &self.doc.layout_state.fragment_output.decoration_positioned_layers,
            &self.doc.layout_state.fragment_output.decoration_negative_positioned_layers,
            &self.doc.layout_state.fragment_output.decoration_independent_positioned_layers,
            &self.doc.layout_state.fragment_output.decoration_line_indices,
            &self.doc.layout_state.fragment_output.decoration_paint_orders,
            indexes,
        )
    }

    pub fn images(self) -> RenderAllImageFragments<'a> {
        RenderAllImageFragments::new(self.doc.image_fragments())
    }
}

impl<'a> RenderAddressingView<'a> {
    pub fn link_for_glyph(self, glyph_idx: u32) -> Option<u16> {
        self.doc.link_href_for_glyph(glyph_idx)
    }

    pub fn is_note_reference(self, glyph_idx: u32) -> bool {
        self.doc.glyph_is_note_reference(glyph_idx)
    }

    pub fn is_note_target(self, id: &str) -> bool {
        self.doc.target_is_note(id)
    }

    pub fn anchor_glyph(self, id_idx: u16) -> Option<u32> {
        self.doc.anchor_glyph(id_idx)
    }

    pub fn anchor_glyphs(self) -> impl Iterator<Item = (u16, u32)> + 'a {
        self.doc.anchor_glyphs().iter().map(|(&id_idx, &glyph_idx)| (id_idx, glyph_idx))
    }

    pub fn glyph_range_for_anchor(self, id: &str) -> Option<Range<u32>> {
        let document = self.doc.document();
        let target = document.node_ids().find(|node| document.get_dom_id(*node) == Some(id))?;
        let mut start = u32::MAX;
        let mut end = 0u32;
        for run in RenderTextRuns::new(self.doc.inline_items(), Some(TextRunKind::Text)) {
            let Some(mut node) = run.dom_text_node().and_then(|raw| document.node_id_from_raw(raw)) else { continue };
            let mut belongs = node == target;
            while !belongs {
                let Some(parent) = document.get_dom_parent(node) else { break };
                node = parent;
                belongs = node == target;
            }
            if belongs {
                let glyphs = run.glyphs();
                start = start.min(glyphs.start);
                end = end.max(glyphs.end);
            }
        }
        (start < end).then_some(start..end)
    }

    pub fn anchor_positions(self) -> RenderAnchorPositions<'a> {
        RenderAnchorPositions::new(&self.doc.layout_state.semantic_indexes.anchor_positions)
    }

    pub fn anchor_position(self, id_idx: u16) -> Option<RenderAnchorPosition> {
        self.anchor_positions().get(id_idx)
    }

    pub fn source_position_for_glyph(self, glyph_idx: u32, after_glyph: bool) -> Option<SourcePosition<'a>> {
        let (text_node_idx, local_offset) = self.doc.get_dom_node_for_glyph(glyph_idx)?;
        let document = self.doc.document();
        let text_node = document.node_id_from_raw(text_node_idx)?;
        let parent = document.node_ref(text_node)?.parent()?;
        let text_step = document.get_text_node_step(text_node)?;
        let mut chunk_offset = 0usize;
        for node in document.find_text_nodes_by_step(parent, text_step) {
            if node == text_node {
                break;
            }
            chunk_offset += document.text_ref(node)?.text().encode_utf16().count();
        }
        let local_offset = if after_glyph { utf16_offset_after_character(document.text_ref(text_node)?.text(), local_offset)? } else { local_offset };
        let path = document.get_dom_path_to_node(parent);
        if path.is_empty() {
            return None;
        }
        let elements = path.into_iter().skip(1).map(|node| Some(SourceElementStep { step: document.get_element_step(node)?, id: document.get_dom_id(node) })).collect::<Option<Vec<_>>>()?;
        Some(SourcePosition { elements, text_step, utf16_offset: chunk_offset + local_offset })
    }

    /// Resolves a generic element/text path to a source glyph boundary.
    pub fn resolve_source_position(self, element_steps: &[usize], text_step: Option<usize>, utf16_offset: Option<usize>, allow_end: bool) -> Option<u32> {
        let document = self.doc.document();
        let parent = if element_steps.is_empty() { document.dom_root()? } else { document.find_dom_node_by_steps(element_steps)? };
        match text_step {
            Some(step) => find_glyph_in_text_chunk(self.doc, parent, step, utf16_offset, allow_end),
            None => find_glyph_in_node(self.doc, parent, utf16_offset, allow_end),
        }
    }
}

fn utf16_offset_after_character(text: &str, wanted: usize) -> Option<usize> {
    let mut offset = 0usize;
    for character in text.chars() {
        if offset == wanted {
            return Some(offset + character.len_utf16());
        }
        if offset > wanted {
            return None;
        }
        offset += character.len_utf16();
    }
    None
}

fn glyph_range_for_text_node(doc: &LaidOutDocument, node: html_dom::DomNodeId) -> Option<Range<u32>> {
    let mut runs = doc.render_view().text().text_runs().filter(|run| run.dom_text_node() == Some(node.raw())).map(|run| run.glyphs());
    let mut range = runs.next()?;
    for run in runs {
        range.start = range.start.min(run.start);
        range.end = range.end.max(run.end);
    }
    Some(range)
}

fn glyph_for_source_offset(doc: &LaidOutDocument, range: Range<u32>, source_offset: Option<usize>, allow_end: bool) -> u32 {
    let Some(source_offset) = source_offset else { return range.start };
    for glyph_idx in range.clone() {
        if doc.glyph_source_offset(glyph_idx).is_some_and(|offset| offset as usize >= source_offset) {
            return glyph_idx;
        }
    }
    if allow_end { range.end } else { range.end.saturating_sub(1) }
}

fn find_glyph_in_text_chunk(doc: &LaidOutDocument, parent: html_dom::DomNodeId, text_step: usize, source_offset: Option<usize>, allow_end: bool) -> Option<u32> {
    let mut nodes = doc.document().find_text_nodes_by_step(parent, text_step).into_iter().peekable();
    let mut remaining = source_offset.unwrap_or(0);
    let mut last_end = None;
    while let Some(node) = nodes.next() {
        let text = doc.document().text_ref(node)?.text();
        let utf16_len = text.encode_utf16().count();
        let Some(range) = glyph_range_for_text_node(doc, node) else {
            remaining = remaining.saturating_sub(utf16_len);
            continue;
        };
        last_end = Some(range.end);
        if remaining < utf16_len || (remaining == utf16_len && (allow_end || nodes.peek().is_none())) {
            return Some(glyph_for_source_offset(doc, range, Some(remaining), allow_end));
        }
        remaining -= utf16_len;
    }
    last_end.map(|end| if allow_end { end } else { end.saturating_sub(1) })
}

fn find_glyph_in_node(doc: &LaidOutDocument, node: html_dom::DomNodeId, source_offset: Option<usize>, allow_end: bool) -> Option<u32> {
    match doc.document().node_ref(node)? {
        NodeRef::Text(_) => glyph_range_for_text_node(doc, node).map(|range| apply_source_offset(range, source_offset, allow_end)),
        NodeRef::Element(element) => {
            if let Some(glyph) = doc.render_view().text().text_runs().find_map(|run| {
                let box_node = doc.box_dom_element_idx(run.box_idx()).and_then(|raw| doc.document().node_id_from_raw(raw))?;
                (box_node == node).then(|| apply_source_offset(run.glyphs(), source_offset, allow_end))
            }) {
                return Some(glyph);
            }
            element.children().find_map(|child| find_glyph_in_node(doc, child, None, allow_end))
        }
    }
}

fn apply_source_offset(range: Range<u32>, source_offset: Option<usize>, allow_end: bool) -> u32 {
    let maximum = if allow_end { range.end } else { range.end.saturating_sub(1) };
    source_offset.map(|offset| (range.start + offset as u32).min(maximum)).unwrap_or(range.start)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxTextFormat {
    pub font_size: f32,
    pub font_weight: u16,
    pub font_style: html_style_model::FontStyle,
    pub color: u32,
    pub font_family: Option<html_style_model::StyleStringId>,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub text_decoration: TextDecorationLines,
}
