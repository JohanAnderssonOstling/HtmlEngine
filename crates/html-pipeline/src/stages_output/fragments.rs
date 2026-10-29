//! Renderer-facing decoration and image fragment queries.

use super::*;
pub use crate::layout_model::DecorationPattern as RenderDecorationPattern;
use crate::layout_model::paint_program::PaintLayers;
use crate::layout_model::{OverflowClip, ScenePaintItem};
use kurbo::Rect;

/// A decoration or text/image carrier line in the document's CSS paint order.
pub enum RenderPaintItem {
    Decoration(RenderDecoration),
    Line(usize),
}

#[derive(Clone, Debug)]
pub struct RenderDecoration {
    rect: Rect,
    color: u32,
    is_inline: bool,
    radii: Option<UsedBorderRadii>,
    border_width: Option<f32>,
    is_foreground: bool,
    is_background: bool,
    is_border: bool,
    pattern: RenderDecorationPattern,
    clip: Option<RenderOverflowClip>,
    paint_layers: PaintLayers,
    line_idx: Option<usize>,
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
    fn from_fragment(
        fragment: &DecorationFragment,
        decorations: &DecorationStore,
        clip: Option<OverflowClip>,
        paint_layers: PaintLayers,
        line_idx: Option<u32>,
        paint_order: u32,
    ) -> Self {
        let rounded = decorations.rounded_for(fragment);
        Self {
            rect: fragment.rect,
            color: fragment.color,
            is_inline: fragment.is_inline(),
            radii: rounded.map(|value| value.radii),
            border_width: rounded.and_then(|value| value.border_width),
            is_foreground: fragment.is_foreground(),
            is_background: fragment.is_background(),
            is_border: fragment.is_border(),
            pattern: fragment.pattern(),
            clip: clip.map(RenderOverflowClip::from_clip),
            paint_layers,
            line_idx: line_idx
                .filter(|idx| *idx != u32::MAX)
                .map(|idx| idx as usize),
            paint_order,
        }
    }

    pub fn rect(&self) -> Rect {
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
    decorations: &'a DecorationStore,
    clips: &'a [Option<OverflowClip>],
    paint_layers: &'a [PaintLayers],
    line_indices: &'a [u32],
    paint_orders: &'a [u32],
    selection: DecorationSelection<'a>,
}

enum DecorationSelection<'a> {
    All {
        block_paint_ranges: &'a [Range<u32>],
        block_decoration_count: usize,
    },
    Blocks {
        block_paint_ranges: &'a [Range<u32>],
        block_decoration_count: usize,
    },
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

impl DecorationSelection<'_> {
    fn indexes(&self, decoration_count: usize) -> DecorationIndexes<'_> {
        match self {
            Self::All {
                block_paint_ranges,
                block_decoration_count,
            } => DecorationIndexes::All {
                ranges: block_paint_ranges.iter(),
                current: 0..0,
                trailing: *block_decoration_count..decoration_count,
            },
            Self::Blocks {
                block_paint_ranges,
                block_decoration_count,
            } => DecorationIndexes::All {
                ranges: block_paint_ranges.iter(),
                current: 0..0,
                trailing: *block_decoration_count..*block_decoration_count,
            },
            Self::Indexes(indexes) => DecorationIndexes::Selected(indexes.iter()),
        }
    }

    fn len(&self, decoration_count: usize) -> usize {
        match self {
            Self::All { .. } => decoration_count,
            Self::Blocks {
                block_decoration_count,
                ..
            } => *block_decoration_count,
            Self::Indexes(indexes) => indexes.len(),
        }
    }
}

impl<'a> RenderDecorations<'a> {
    fn new(document: DocumentRead<'a>, selection: DecorationSelection<'a>) -> Self {
        let output = &document.layout_state.fragment_output;
        Self {
            decorations: &output.decorations,
            clips: &output.decoration_clips,
            paint_layers: &output.decoration_paint_layers,
            line_indices: &output.decoration_line_indices,
            paint_orders: &output.decoration_paint_orders,
            selection,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderDecoration> + '_ {
        self.selection
            .indexes(self.decorations.len())
            .map(|index| self.at(index))
    }

    fn at(&self, index: usize) -> RenderDecoration {
        RenderDecoration::from_fragment(
            &self.decorations.fragments()[index],
            self.decorations,
            self.clips.get(index).copied().flatten(),
            self.paint_layers.get(index).copied().unwrap_or_default(),
            self.line_indices.get(index).copied(),
            self.paint_orders.get(index).copied().unwrap_or(u32::MAX),
        )
    }

    pub fn len(&self) -> usize {
        self.selection.len(self.decorations.len())
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
    fn all(fragments: &'a [ImageFragment], document: DocumentRead<'a>) -> Self {
        Self {
            fragments,
            indexes: None,
            document,
        }
    }

    fn selected(
        fragments: &'a [ImageFragment],
        indexes: &'a [usize],
        document: DocumentRead<'a>,
    ) -> Self {
        Self {
            fragments,
            indexes: Some(indexes),
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
    /// Interleaves positioned decorations and content without consulting
    /// execution scratch or inferring stacking order from screen geometry.
    pub fn paint_order(self) -> impl Iterator<Item = RenderPaintItem> + 'a {
        let decorations = self.decorations();
        self.doc
            .layout_state
            .fragment_output
            .scene_paint_order
            .iter()
            .map(move |item| match *item {
                ScenePaintItem::Decoration(index) => {
                    RenderPaintItem::Decoration(decorations.at(index as usize))
                }
                ScenePaintItem::Line(index) => RenderPaintItem::Line(index as usize),
            })
    }

    /// Replay caller-selected content in the document's canonical CSS order.
    /// Inline decorations are included with their owning lines. Duplicate
    /// candidates are harmless, including repeated source regions.
    pub fn paint_order_for(
        self,
        lines: &[usize],
        block_decorations: &[usize],
    ) -> Vec<RenderPaintItem> {
        let output = &self.doc.layout_state.fragment_output;
        let mut ranks = Vec::new();
        for &line in lines {
            ranks.extend(
                output
                    .scene_line_ranks
                    .get(line)
                    .copied()
                    .filter(|rank| *rank != u32::MAX),
            );
            for &index in output.decoration_fragments_by_line.get(line).unwrap_or(&[]) {
                ranks.extend(
                    output
                        .scene_decoration_ranks
                        .get(index)
                        .copied()
                        .filter(|rank| *rank != u32::MAX),
                );
            }
        }
        for &index in block_decorations {
            ranks.extend(
                output
                    .scene_decoration_ranks
                    .get(index)
                    .copied()
                    .filter(|rank| *rank != u32::MAX),
            );
        }
        ranks.sort_unstable();
        ranks.dedup();
        let decorations = self.decorations();
        ranks
            .into_iter()
            .filter_map(|rank| match *output.scene_paint_order.get(rank as usize)? {
                ScenePaintItem::Line(line) => Some(RenderPaintItem::Line(line as usize)),
                ScenePaintItem::Decoration(index) => {
                    Some(RenderPaintItem::Decoration(decorations.at(index as usize)))
                }
            })
            .collect()
    }

    pub fn decorations(self) -> RenderDecorations<'a> {
        let output = &self.doc.layout_state.fragment_output;
        RenderDecorations::new(
            self.doc,
            DecorationSelection::All {
                block_paint_ranges: &output.block_paint_ranges,
                block_decoration_count: output.block_decoration_count as usize,
            },
        )
    }

    /// Document-level decorations only, in CSS paint order. Line-owned
    /// decorations are already indexed and replayed with their visible line.
    pub fn block_decorations(self) -> RenderDecorations<'a> {
        let output = &self.doc.layout_state.fragment_output;
        RenderDecorations::new(
            self.doc,
            DecorationSelection::Blocks {
                block_paint_ranges: &output.block_paint_ranges,
                block_decoration_count: output.block_decoration_count as usize,
            },
        )
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
        RenderImageFragments::selected(
            &self.doc.layout_state.fragment_output.image_fragments,
            indexes,
            self.doc,
        )
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
        RenderImageFragments::all(
            &self.doc.layout_state.fragment_output.image_fragments,
            self.doc,
        )
    }
}
