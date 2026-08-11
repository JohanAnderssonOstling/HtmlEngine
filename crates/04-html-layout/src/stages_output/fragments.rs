//! Renderer-facing decoration and image fragment queries.

use super::*;

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
    pub(super) fn from_clip(clip: crate::layout_model::OverflowClip) -> Self {
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
        fragment: &DecorationFragment,
        decorations: &DecorationStore,
        clip: Option<crate::layout_model::OverflowClip>,
        positioned_layer: bool,
        negative_positioned_layer: bool,
        independent_positioned_layer: bool,
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
    decorations: &'a DecorationStore,
    clips: &'a [Option<crate::layout_model::OverflowClip>],
    positioned_layers: &'a [bool],
    negative_positioned_layers: &'a [bool],
    independent_positioned_layers: &'a [bool],
    line_indices: &'a [u32],
    paint_orders: &'a [u32],
    selection: DecorationSelection<'a>,
}

enum DecorationSelection<'a> {
    All { block_paint_ranges: &'a [Range<u32>], block_decoration_count: usize },
    Indexes(&'a [usize]),
}

enum DecorationIndexes<'a> {
    All { ranges: std::slice::Iter<'a, Range<u32>>, current: Range<u32>, trailing: Range<usize> },
    Selected(std::slice::Iter<'a, usize>),
}

impl Iterator for DecorationIndexes<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Selected(indexes) => indexes.next().copied(),
            Self::All { ranges, current, trailing } => loop {
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
            Self::All { block_paint_ranges, block_decoration_count } => DecorationIndexes::All { ranges: block_paint_ranges.iter(), current: 0..0, trailing: *block_decoration_count..decoration_count },
            Self::Indexes(indexes) => DecorationIndexes::Selected(indexes.iter()),
        }
    }

    fn len(&self, decoration_count: usize) -> usize {
        match self {
            Self::All { .. } => decoration_count,
            Self::Indexes(indexes) => indexes.len(),
        }
    }
}

impl<'a> RenderDecorations<'a> {
    fn all(
        decorations: &'a DecorationStore,
        clips: &'a [Option<crate::layout_model::OverflowClip>],
        block_paint_ranges: &'a [Range<u32>],
        block_decoration_count: usize,
        positioned_layers: &'a [bool],
        negative_positioned_layers: &'a [bool],
        independent_positioned_layers: &'a [bool],
        line_indices: &'a [u32],
        paint_orders: &'a [u32],
    ) -> Self {
        Self { decorations, clips, positioned_layers, negative_positioned_layers, independent_positioned_layers, line_indices, paint_orders, selection: DecorationSelection::All { block_paint_ranges, block_decoration_count } }
    }

    fn selected(
        decorations: &'a DecorationStore,
        clips: &'a [Option<crate::layout_model::OverflowClip>],
        positioned_layers: &'a [bool],
        negative_positioned_layers: &'a [bool],
        independent_positioned_layers: &'a [bool],
        line_indices: &'a [u32],
        paint_orders: &'a [u32],
        indexes: &'a [usize],
    ) -> Self {
        Self { decorations, clips, positioned_layers, negative_positioned_layers, independent_positioned_layers, line_indices, paint_orders, selection: DecorationSelection::Indexes(indexes) }
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderDecoration> + '_ {
        let render = |index: usize| {
            RenderDecoration::from_fragment(
                &self.decorations.fragments()[index],
                self.decorations,
                self.clips.get(index).copied().flatten(),
                self.positioned_layers.get(index).copied().unwrap_or(false),
                self.negative_positioned_layers.get(index).copied().unwrap_or(false),
                self.independent_positioned_layers.get(index).copied().unwrap_or(false),
                self.line_indices.get(index).copied(),
                self.paint_orders.get(index).copied().unwrap_or(u32::MAX),
            )
        };
        self.selection.indexes(self.decorations.len()).map(render)
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
    line_idx: usize,
    image_idx: u32,
    offset: Point,
    size: Size,
    clip: kurbo::Rect,
    paint_order: u32,
}

impl RenderImageFragment {
    fn from_fragment(fragment: &ImageFragment) -> Self {
        Self { line_idx: fragment.line_idx, image_idx: fragment.image_idx, offset: fragment.offset, size: fragment.size, clip: fragment.clip, paint_order: fragment.paint_order }
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

    pub fn clip(&self) -> kurbo::Rect {
        self.clip
    }

    pub fn paint_order(&self) -> u32 {
        self.paint_order
    }
}

pub struct RenderImageFragments<'a> {
    fragments: &'a [ImageFragment],
    indexes: Option<&'a [usize]>,
}

impl<'a> RenderImageFragments<'a> {
    fn all(fragments: &'a [ImageFragment]) -> Self {
        Self { fragments, indexes: None }
    }

    fn selected(fragments: &'a [ImageFragment], indexes: &'a [usize]) -> Self {
        Self { fragments, indexes: Some(indexes) }
    }

    pub fn len(&self) -> usize {
        self.indexes.map_or(self.fragments.len(), <[usize]>::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = RenderImageFragment> + '_ {
        (0..self.len()).filter_map(move |position| {
            let index = self.indexes.map_or(position, |indexes| indexes[position]);
            self.fragments.get(index).map(RenderImageFragment::from_fragment)
        })
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
#[derive(Clone, Copy)]
pub struct RenderFragmentView<'a> {
    pub(super) doc: &'a LaidOutDocument,
}

impl<'a> RenderFragmentView<'a> {
    pub fn decorations(self) -> RenderDecorations<'a> {
        RenderDecorations::all(
            &self.doc.layout_state.fragment_output.decorations,
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
        let indexes = self.doc.layout_state.fragment_output.image_fragments_by_line.get(line_idx).map_or(&[] as &'a [usize], |indexes| indexes.as_slice());
        RenderImageFragments::selected(&self.doc.layout_state.fragment_output.image_fragments, indexes)
    }

    pub fn decorations_for_line(self, line_idx: usize) -> RenderDecorations<'a> {
        let indexes = self.doc.layout_state.fragment_output.decoration_fragments_by_line.get(line_idx).map_or(&[] as &'a [usize], |indexes| indexes.as_slice());
        RenderDecorations::selected(
            &self.doc.layout_state.fragment_output.decorations,
            &self.doc.layout_state.fragment_output.decoration_clips,
            &self.doc.layout_state.fragment_output.decoration_positioned_layers,
            &self.doc.layout_state.fragment_output.decoration_negative_positioned_layers,
            &self.doc.layout_state.fragment_output.decoration_independent_positioned_layers,
            &self.doc.layout_state.fragment_output.decoration_line_indices,
            &self.doc.layout_state.fragment_output.decoration_paint_orders,
            indexes,
        )
    }

    pub fn images(self) -> RenderImageFragments<'a> {
        RenderImageFragments::all(&self.doc.layout_state.fragment_output.image_fragments)
    }
}
