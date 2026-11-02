//! Renderer-facing decoration and image fragment queries.

use super::*;
pub use crate::layout_model::DecorationPattern as RenderDecorationPattern;
use crate::layout_model::paint_program::PaintLayers;
use crate::layout_model::{FragmentOutput, OverflowClip, ScenePaintItem};
use kurbo::Rect;

/// A decoration or text/image carrier line in the document's CSS paint order.
pub enum RenderPaintItem {
    Decoration(RenderDecoration),
    Line(usize),
}

impl RenderPaintItem {
    fn from_scene(item: ScenePaintItem, decorations: &RenderDecorations<'_>) -> Self {
        match item {
            ScenePaintItem::Decoration(index) => Self::Decoration(decorations.at(index as usize)),
            ScenePaintItem::Line(index) => Self::Line(index as usize),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenderDecoration {
    rect: Rect,
    color: u32,
    is_inline: bool,
    radii: Option<CornerRadii>,
    border_width: Option<f32>,
    is_foreground: bool,
    is_background: bool,
    is_border: bool,
    pattern: RenderDecorationPattern,
    clip: Option<RenderOverflowClip>,
    paint_layers: PaintLayers,
    line_idx: Option<usize>,
    owner_box_idx: Option<usize>,
    paint_order: u32,
}

/// Renderer-facing overflow clip with independent physical axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOverflowClip {
    clip: OverflowClip,
}

impl RenderOverflowClip {
    pub(super) fn from_clip(clip: OverflowClip) -> Self {
        Self { clip }
    }

    pub fn rect(self) -> Rect {
        self.clip.rect
    }

    pub fn clips_x(self) -> bool {
        self.clip.x
    }

    pub fn clips_y(self) -> bool {
        self.clip.y
    }

    /// Restrict a rectangle on the axes selected by this overflow clip.
    pub fn apply_to(self, rect: Rect) -> Rect {
        self.clip.apply_to(rect)
    }
}

impl RenderDecoration {
    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn color(&self) -> u32 {
        self.color
    }

    pub fn is_inline(&self) -> bool {
        self.is_inline
    }

    pub fn radii(&self) -> Option<CornerRadii> {
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
        self.paint_layers.is_positioned()
    }

    pub fn is_in_negative_positioned_layer(&self) -> bool {
        self.paint_layers.is_negative()
    }

    pub fn is_in_independent_positioned_layer(&self) -> bool {
        self.paint_layers.is_independent()
    }

    pub fn line_idx(&self) -> Option<usize> {
        self.line_idx
    }

    pub fn owner_box_idx(&self) -> Option<usize> {
        self.owner_box_idx
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
    output: &'a FragmentOutput,
    selection: DecorationSelection<'a>,
}

enum DecorationSelection<'a> {
    All,
    Blocks,
    Indexes(&'a [usize]),
}

enum DecorationIndexes<'a> {
    All {
        ranges: std::slice::Iter<'a, Range<u32>>,
        current: Range<u32>,
        trailing: Range<usize>,
    },
    Selected(std::slice::Iter<'a, usize>),
}

impl Iterator for DecorationIndexes<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Selected(indexes) => indexes.next().copied(),
            Self::All {
                ranges,
                current,
                trailing,
            } => loop {
                if let Some(index) = current.next() {
                    return Some(index as usize);
                }
                if let Some(range) = ranges.next() {
                    *current = range.clone();
                } else {
                    return trailing.next();
                }
            },
        }
    }
}

impl<'a> RenderDecorations<'a> {
    fn new(document: DocumentRead<'a>, selection: DecorationSelection<'a>) -> Self {
        Self {
            output: &document.layout_state.fragment_output,
            selection,
        }
    }

    fn indexes(&self) -> DecorationIndexes<'_> {
        match &self.selection {
            DecorationSelection::All | DecorationSelection::Blocks => DecorationIndexes::All {
                ranges: self.output.block_paint_ranges.iter(),
                current: 0..0,
                trailing: self.output.block_decoration_count as usize..self.len(),
            },
            DecorationSelection::Indexes(indexes) => DecorationIndexes::Selected(indexes.iter()),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderDecoration> + '_ {
        self.indexes().map(|index| self.at(index))
    }

    fn at(&self, index: usize) -> RenderDecoration {
        let output = self.output;
        let fragment = &output.decorations.fragments()[index];
        let rounded = output.decorations.rounded_for(fragment);
        RenderDecoration {
            rect: fragment.rect,
            color: fragment.color,
            is_inline: fragment.is_inline(),
            radii: rounded.map(|value| value.radii),
            border_width: rounded.and_then(|value| value.border_width),
            is_foreground: fragment.is_foreground(),
            is_background: fragment.is_background(),
            is_border: fragment.is_border(),
            pattern: fragment.pattern(),
            clip: output
                .decoration_clips
                .get(index)
                .copied()
                .flatten()
                .map(RenderOverflowClip::from_clip),
            paint_layers: output
                .decoration_paint_layers
                .get(index)
                .copied()
                .unwrap_or_default(),
            line_idx: output
                .decoration_line_indices
                .get(index)
                .copied()
                .filter(|idx| *idx != u32::MAX)
                .map(|idx| idx as usize),
            owner_box_idx: output
                .block_decoration_owners
                .get(index)
                .map(|&owner| owner as usize),
            paint_order: output
                .decoration_paint_orders
                .get(index)
                .copied()
                .unwrap_or(u32::MAX),
        }
    }

    pub fn len(&self) -> usize {
        match &self.selection {
            DecorationSelection::All => self.output.decorations.len(),
            DecorationSelection::Blocks => self.output.block_decoration_count as usize,
            DecorationSelection::Indexes(indexes) => indexes.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RenderImageFragment {
    pub(super) box_idx: usize,
    line_idx: usize,
    image_idx: u32,
    offset: Point,
    size: Size,
    clip: Rect,
    paint_order: u32,
    text_color: u32,
}

impl RenderImageFragment {
    pub fn box_idx(&self) -> usize {
        self.box_idx
    }

    fn from_fragment(fragment: &ImageFragment, document: DocumentRead<'_>) -> Self {
        Self {
            box_idx: fragment.box_idx as usize,
            line_idx: fragment.line_idx,
            image_idx: fragment.image_idx,
            offset: fragment.offset,
            size: fragment.size,
            clip: fragment.clip,
            paint_order: fragment.paint_order,
            text_color: document.box_text_format(fragment.box_idx as usize).color,
        }
    }

    pub fn line_idx(&self) -> usize {
        self.line_idx
    }

    pub fn image_idx(&self) -> u32 {
        self.image_idx
    }

    pub fn text_color(&self) -> u32 {
        self.text_color
    }

    pub fn offset(&self) -> Point {
        self.offset
    }

    pub fn size(&self) -> Size {
        self.size
    }

    /// Fitted image bounds relative to the carrier line's origin.
    pub fn rect(&self) -> Rect {
        Rect::from_origin_size(self.offset, self.size)
    }

    /// Fitted bounds restricted by the image's object-fit clip, relative to
    /// the carrier line's origin. Ancestor overflow clips apply separately.
    pub fn visible_rect(&self) -> Rect {
        self.rect().intersect(self.clip)
    }

    pub fn clip(&self) -> Rect {
        self.clip
    }

    pub fn paint_order(&self) -> u32 {
        self.paint_order
    }
}

#[derive(Clone, Copy)]
pub struct RenderImageFragments<'a> {
    fragments: &'a [ImageFragment],
    indexes: Option<&'a [usize]>,
    document: DocumentRead<'a>,
}

impl<'a> RenderImageFragments<'a> {
    fn new(document: DocumentRead<'a>, indexes: Option<&'a [usize]>) -> Self {
        Self {
            fragments: &document.layout_state.fragment_output.image_fragments,
            indexes,
            document,
        }
    }

    pub fn len(&self) -> usize {
        self.indexes.map_or(self.fragments.len(), <[usize]>::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(self) -> impl Iterator<Item = RenderImageFragment> + 'a {
        (0..self.len()).filter_map(move |position| {
            let index = self.indexes.map_or(position, |indexes| indexes[position]);
            self.fragments
                .get(index)
                .map(|fragment| RenderImageFragment::from_fragment(fragment, self.document))
        })
    }
}

/// Stable semantic text-run view. Its glyph range is layout-owned and cannot
/// be forged by renderer crates.
///
/// ```compile_fail
/// let _ = html_pipeline::engine::RenderTextRun {
///     kind: (),
///     box_idx: 0,
///     dom_text_node: None,
///     glyphs: 5..1,
/// };
/// ```
#[derive(Clone, Copy)]
pub struct RenderFragmentView<'a> {
    pub(super) doc: DocumentRead<'a>,
}

impl<'a> RenderFragmentView<'a> {
    pub fn line_paint_rank(self, line: usize) -> Option<u32> {
        self.doc
            .layout_state
            .fragment_output
            .scene_paint_order
            .line_ranks()
            .get(line)
            .copied()
    }

    /// Interleaves positioned decorations and content without consulting
    /// execution scratch or inferring stacking order from screen geometry.
    pub fn paint_order(self) -> impl Iterator<Item = RenderPaintItem> + 'a {
        let decorations = self.decorations();
        self.doc
            .layout_state
            .fragment_output
            .scene_paint_order
            .iter()
            .map(move |&item| RenderPaintItem::from_scene(item, &decorations))
    }

    /// Replay caller-selected content in the document's canonical CSS order.
    /// Inline decorations are included with their owning lines. Duplicate
    /// candidates are harmless, including repeated source regions.
    pub fn paint_order_for(
        self,
        lines: &[usize],
        block_decorations: &[usize],
    ) -> Vec<RenderPaintItem> {
        self.paint_order_for_iter(lines, block_decorations)
            .collect()
    }

    /// Stream caller-selected content from sorted, deduplicated scene ranks.
    pub fn paint_order_for_iter(
        self,
        lines: &[usize],
        block_decorations: &[usize],
    ) -> impl Iterator<Item = RenderPaintItem> + 'a + use<'a> {
        let output = &self.doc.layout_state.fragment_output;
        let decoration_indices = lines
            .iter()
            .flat_map(|&line| output.decoration_fragments_by_line.get(line).unwrap_or(&[]))
            .chain(block_decorations);
        let mut ranks = lines
            .iter()
            .filter_map(|&line| output.scene_paint_order.line_ranks().get(line))
            .chain(
                decoration_indices
                    .filter_map(|&index| output.scene_paint_order.decoration_ranks().get(index)),
            )
            .copied()
            .filter(|rank| *rank != u32::MAX)
            .collect::<Vec<_>>();
        ranks.sort_unstable();
        ranks.dedup();
        let decorations = self.decorations();
        ranks
            .into_iter()
            .filter_map(move |rank| output.scene_paint_order.get(rank as usize))
            .map(move |&item| RenderPaintItem::from_scene(item, &decorations))
    }

    pub fn decorations(self) -> RenderDecorations<'a> {
        RenderDecorations::new(self.doc, DecorationSelection::All)
    }

    /// Document-level decorations only, in CSS paint order. Line-owned
    /// decorations are already indexed and replayed with their visible line.
    pub fn block_decorations(self) -> RenderDecorations<'a> {
        RenderDecorations::new(self.doc, DecorationSelection::Blocks)
    }

    /// Selects the document-level decorations intersecting a vertical layout
    /// interval and returns their storage indexes in CSS paint order.
    pub fn block_decoration_indices_intersecting(
        self,
        top: f64,
        bottom: f64,
        indexes: &mut Vec<usize>,
    ) {
        indexes.clear();
        if !top.is_finite() || !bottom.is_finite() || bottom <= top {
            return;
        }
        let output = &self.doc.layout_state.fragment_output;
        let spatial = &output.block_decoration_indices_by_y;
        let prefix_max = &output.block_decoration_prefix_max_y1;
        let fragments = output.decorations.fragments();
        let end = spatial.partition_point(|&index| fragments[index as usize].rect.y0 < bottom);
        let start = prefix_max[..end].partition_point(|&maximum_y1| maximum_y1 <= top);
        indexes.extend(
            spatial[start..end]
                .iter()
                .map(|&index| index as usize)
                .filter(|&index| fragments[index].rect.y1 > top),
        );
        indexes.sort_unstable_by_key(|&index| {
            output
                .block_decoration_paint_ranks
                .get(index)
                .copied()
                .unwrap_or(u32::MAX)
        });
    }

    pub fn decorations_by_indices(self, indexes: &'a [usize]) -> RenderDecorations<'a> {
        RenderDecorations::new(self.doc, DecorationSelection::Indexes(indexes))
    }

    pub fn images_for_line(self, line_idx: usize) -> RenderImageFragments<'a> {
        let indexes = self
            .doc
            .layout_state
            .fragment_output
            .image_fragments_by_line
            .get(line_idx)
            .unwrap_or(&[]);
        RenderImageFragments::new(self.doc, Some(indexes))
    }

    pub fn decorations_for_line(self, line_idx: usize) -> RenderDecorations<'a> {
        let indexes = self
            .doc
            .layout_state
            .fragment_output
            .decoration_fragments_by_line
            .get(line_idx)
            .unwrap_or(&[]);
        RenderDecorations::new(self.doc, DecorationSelection::Indexes(indexes))
    }

    pub fn images(self) -> RenderImageFragments<'a> {
        RenderImageFragments::new(self.doc, None)
    }
}
