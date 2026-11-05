//! Renderer-facing decoration and image fragment queries.

use html_engine_model::layout_state;
use html_dom as dom_values;
use super::*;
pub use crate::layout_model::DecorationPattern as RenderDecorationPattern;
use crate::layout_model::paint_program::PaintLayers;
use crate::layout_model::{FragmentOutput, OverflowClip, RoundedDecoration, ScenePaintItem};
use kurbo::Rect;

/// A decoration or text/image carrier line in the document's CSS paint order.
pub enum RenderPaintItem {
    Decoration(RenderDecoration),
    Line(LineId),
}

impl RenderPaintItem {
    fn from_scene(item: ScenePaintItem, decorations: &RenderDecorations<'_>) -> Self {
        match item {
            ScenePaintItem::Decoration(index) => Self::Decoration(decorations.at(index as usize)),
            ScenePaintItem::Line(index) => Self::Line(LineId::new(index as usize)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenderDecoration {
    rect: Rect,
    color: u32,
    is_inline: bool,
    rounded: Option<RoundedDecoration>,
    is_foreground: bool,
    is_background: bool,
    is_border: bool,
    pattern: RenderDecorationPattern,
    clip: Option<RenderOverflowClip>,
    paint_layers: PaintLayers,
    line: Option<LineId>,
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

    pub fn is_inline(&self) -> bool {
        self.is_inline
    }

    pub fn border_width(&self) -> Option<f32> {
        self.rounded
            .as_ref()
            .and_then(|rounded| rounded.border_width)
    }

    pub fn owner_box(&self) -> Option<BoxId> {
        self.owner_box_idx.map(layout_state::box_id_new)
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

impl<'a> RenderDecorations<'a> {
    fn new(document: DocumentRead<'a>, selection: DecorationSelection<'a>) -> Self {
        Self {
            output: &document.layout_state.fragment_output,
            selection,
        }
    }

    fn indexes(&self) -> impl Iterator<Item = usize> + '_ {
        let mut ordered = self.output
            .block_paint_ranges
            .iter()
            .flat_map(|range| range.clone().map(|index| index as usize))
            .chain(self.output.block_decoration_count as usize..render_decorations_len(&self));
        (0..render_decorations_len(&self)).filter_map(move |position| match &self.selection {
            DecorationSelection::Indexes(indexes) => indexes.get(position).copied(),
            DecorationSelection::All | DecorationSelection::Blocks => ordered.next(),
        })
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
            rounded: rounded.copied(),
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
            line: output
                .decoration_line_indices
                .get(index)
                .copied()
                .filter(|idx| *idx != u32::MAX)
                .map(|idx| LineId::new(idx as usize)),
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

    pub(crate) fn is_empty(&self) -> bool {
        render_decorations_len(&self) == 0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RenderImageFragment {
    pub(super) box_idx: usize,
    line: LineId,
    image: ImageId,
    offset: Point,
    size: Size,
    clip: Rect,
    text_color: u32,
}

impl RenderImageFragment {

    fn from_fragment(fragment: &ImageFragment, document: DocumentRead<'_>) -> Self {
        Self {
            box_idx: fragment.box_idx as usize,
            line: LineId::new(fragment.line_idx),
            image: dom_values::image_id_new(fragment.image_idx),
            offset: fragment.offset,
            size: fragment.size,
            clip: fragment.clip,
            text_color: document.box_text_format(fragment.box_idx as usize).color,
        }
    }

    pub fn image(&self) -> ImageId {
        self.image
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

    pub(crate) fn len(&self) -> usize {
        self.indexes.map_or(self.fragments.len(), <[usize]>::len)
    }

    pub(crate) fn is_empty(&self) -> bool {
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







}

pub fn decoration_color(item: &RenderDecoration) -> u32 {
    item.color
}

pub fn decoration_radii(item: &RenderDecoration) -> Option<CornerRadii> {
    item.rounded.as_ref().map(|rounded| rounded.radii)
}

pub fn decoration_is_foreground(item: &RenderDecoration) -> bool {
    item.is_foreground
}

pub fn decoration_is_background(item: &RenderDecoration) -> bool {
    item.is_background
}

pub fn decoration_is_border(item: &RenderDecoration) -> bool {
    item.is_border
}

pub fn decoration_is_in_positioned_layer(item: &RenderDecoration) -> bool {
    item.paint_layers.is_positioned()
}

pub fn decoration_is_in_negative_positioned_layer(item: &RenderDecoration) -> bool {
    item.paint_layers.is_negative()
}

pub fn decoration_is_in_independent_positioned_layer(item: &RenderDecoration) -> bool {
    item.paint_layers.is_independent()
}

pub fn decoration_paint_order(item: &RenderDecoration) -> u32 {
    item.paint_order
}

pub fn decoration_pattern(item: &RenderDecoration) -> RenderDecorationPattern {
    item.pattern
}

/// Resolved padding-box clip contributed by overflow ancestors.
pub fn decoration_overflow_clip(item: &RenderDecoration) -> Option<RenderOverflowClip> {
    item.clip
}

pub fn image_text_color(item: &RenderImageFragment) -> u32 {
    item.text_color
}

pub fn line_paint_rank(view: RenderFragmentView<'_>, line: LineId) -> Option<u32> {
    view.doc
        .layout_state
        .fragment_output
        .scene_paint_order
        .line_ranks()
        .get(layout_state::line_id_index(line))
        .copied()
}

/// Interleaves positioned decorations and content without consulting
/// execution scratch or inferring stacking order from screen geometry.
pub fn paint_order<'a>(view: RenderFragmentView<'a>) -> impl Iterator<Item = RenderPaintItem> + 'a {
    let decorations = decorations(view);
    view.doc
        .layout_state
        .fragment_output
        .scene_paint_order
        .iter()
        .map(move |&item| RenderPaintItem::from_scene(item, &decorations))
}

/// Replay selected content in canonical CSS order, including decorations
/// owned by the selected lines. Candidates are sorted and deduplicated;
/// the returned iterator does not borrow the candidate slices.
pub fn paint_order_for<'a>(
    view: RenderFragmentView<'a>,
    lines: &[LineId],
    block_decorations: &[usize],
) -> impl Iterator<Item = RenderPaintItem> + 'a + use<'a> {
    let output = &view.doc.layout_state.fragment_output;
    let decoration_indices = lines
        .iter()
        .flat_map(|line| {
            output
                .decoration_fragments_by_line
                .get(layout_state::line_id_index(*(line)))
                .unwrap_or(&[])
        })
        .chain(block_decorations);
    let mut ranks = lines
        .iter()
        .filter_map(|line| output.scene_paint_order.line_ranks().get(layout_state::line_id_index(*(line))))
        .chain(
            decoration_indices
                .filter_map(|&index| output.scene_paint_order.decoration_ranks().get(index)),
        )
        .copied()
        .filter(|rank| *rank != u32::MAX)
        .collect::<Vec<_>>();
    ranks.sort_unstable();
    ranks.dedup();
    let decorations = decorations(view);
    ranks
        .into_iter()
        .filter_map(move |rank| output.scene_paint_order.get(rank as usize))
        .map(move |&item| RenderPaintItem::from_scene(item, &decorations))
}

pub fn decorations<'a>(view: RenderFragmentView<'a>) -> RenderDecorations<'a> {
    RenderDecorations::new(view.doc, DecorationSelection::All)
}

/// Document-level decorations only, in CSS paint order. Line-owned
/// decorations are already indexed and replayed with their visible line.
pub fn block_decorations<'a>(view: RenderFragmentView<'a>) -> RenderDecorations<'a> {
    RenderDecorations::new(view.doc, DecorationSelection::Blocks)
}

pub fn render_image_fragment_clip(value: &RenderImageFragment) -> Rect {
    value.clip
}

pub fn render_image_fragment_box_id(value: &RenderImageFragment) -> BoxId {
    layout_state::box_id_new(value.box_idx)
}

pub fn render_decorations_len<'a>(value: &RenderDecorations<'a>) -> usize {
    match &value.selection {
        DecorationSelection::All => value.output.decorations.len(),
        DecorationSelection::Blocks => value.output.block_decoration_count as usize,
        DecorationSelection::Indexes(indexes) => indexes.len(),
    }
}

pub fn render_image_fragment_line(value: &RenderImageFragment) -> LineId {
    value.line
}

pub fn render_decoration_line(value: &RenderDecoration) -> Option<LineId> {
    value.line
}

/// Fitted image bounds relative to the carrier line's origin.
pub fn render_image_fragment_rect(value: &RenderImageFragment) -> Rect {
    Rect::from_origin_size(value.offset, value.size)
}

pub fn render_image_fragment_offset(value: &RenderImageFragment) -> Point {
    value.offset
}

/// Fitted bounds restricted by the image's object-fit clip, relative to
/// the carrier line's origin. Ancestor overflow clips apply separately.
pub fn render_image_fragment_visible_rect(value: &RenderImageFragment) -> Rect {
    render_image_fragment_rect(&value).intersect(value.clip)
}

pub fn render_image_fragment_size(value: &RenderImageFragment) -> Size {
    value.size
}

pub fn render_fragment_view_images<'a>(value: RenderFragmentView<'a>) -> RenderImageFragments<'a> {
    RenderImageFragments::new(value.doc, None)
}

pub fn render_fragment_view_decorations_for_line<'a>(
    value: RenderFragmentView<'a>,
    line: LineId,
) -> RenderDecorations<'a> {
    let indexes = value
        .doc
        .layout_state
        .fragment_output
        .decoration_fragments_by_line
        .get(layout_state::line_id_index(line))
        .unwrap_or(&[]);
    RenderDecorations::new(value.doc, DecorationSelection::Indexes(indexes))
}

pub fn render_fragment_view_images_for_line<'a>(
    value: RenderFragmentView<'a>,
    line: LineId,
) -> RenderImageFragments<'a> {
    let indexes = value
        .doc
        .layout_state
        .fragment_output
        .image_fragments_by_line
        .get(layout_state::line_id_index(line))
        .unwrap_or(&[]);
    RenderImageFragments::new(value.doc, Some(indexes))
}

/// Whether a source-space cut crosses a continuous block border.
pub fn has_unbroken_border(view: RenderFragmentView<'_>, line: LineId, y: f64) -> bool {
    render_fragment_view_decorations_for_line(view, line)
        .iter()
        .any(|decoration| {
            !decoration.is_inline()
                && decoration.border_width().is_some()
                && y > decoration.rect().y0 + 0.01
                && y < decoration.rect().y1 - 0.01
        })
}

pub fn block_decoration_indices_intersecting(
    value: RenderFragmentView<'_>,
    top: f64,
    bottom: f64,
    indexes: &mut Vec<usize>,
) {
    indexes.clear();
    if !top.is_finite() || !bottom.is_finite() || bottom <= top {
        return;
    }
    let output = &value.doc.layout_state.fragment_output;
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
