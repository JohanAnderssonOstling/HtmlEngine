use super::glyph_line_indices::GlyphLineIndices;
use super::grouped_arena::GroupedArena;
use html_dom::MemoryUsageReport;
use html_style_model::UsedBorderRadii;
use kurbo::{Point, Rect, Size};
use rustc_data_structures::fx::FxHashMap;
use std::num::NonZeroU32;
use std::ops::Range;

/// Output of the layout phase: line boxes, glyph offsets, and paint fragments.
/// Produced by the layout phase and consumed read-only by the renderer.
#[derive(Default, Clone)]
pub struct LayoutState {
    pub paint_program: crate::paint_program::PaintProgram,
    pub line_output: LineOutput,
    pub fragment_output: FragmentOutput,
    pub semantic_indexes: SemanticIndexes,
}

#[derive(Default, Clone)]
pub struct LineOutput {
    pub lines: Vec<Line>,
    /// Uncommon paint/inline data is retained only for lines that need it.
    pub line_details: Vec<LineDetails>,
    pub text_fragments: Vec<LineTextFragment>,
    /// Line indexes in CSS paint order. Physical line storage remains sorted
    /// by document geometry so pagination and viewport lookup can use it.
    pub paint_order_indices: Vec<u32>,
    /// Inverse of `paint_order_indices`: CSS paint rank for each spatial line
    /// index. Page composition uses this to order only its visible lines
    /// instead of rescanning the document-wide paint traversal.
    pub paint_order_ranks: Vec<u32>,
    /// Contiguous non-text inline fragment arena. Each line owns a range into
    /// this buffer, avoiding one heap allocation per non-trivial line.
    pub inline_box_fragments: Vec<LineInlineBoxFragment>,
    pub positioned_layers: Vec<bool>,
    pub negative_positioned_layers: Vec<bool>,
    pub independent_positioned_layers: Vec<bool>,
    /// Sparse resolved overflow clips, indexed by line. Empty for the common
    /// case where every box uses `overflow: visible`.
    pub line_clips: Vec<Option<OverflowClip>>,
    /// Canonical owning line for each source glyph. Aggregate line ranges may
    /// overlap when an atomic inline box contains its own laid-out text, so
    /// renderer interaction must not infer ownership from `Line::glyphs`.
    pub glyph_line_indices: GlyphLineIndices,
    pub line_glyph_offsets: GroupedArena<GlyphOffsetRun>,
    pub line_glyph_advances: GroupedArena<GlyphAdvanceRun>,
    /// Contiguous arena of authoritative shaped-run slices prepared for
    /// painting. Lines retain compact ranges into this buffer.
    pub prepared_text_run_fragments: Vec<PreparedTextRunFragment>,
    pub ellipsis_fragments: Vec<EllipsisFragment>,
    pub hyphen_fragments: Vec<HyphenFragment>,
}

#[derive(Default, Clone)]
pub struct FragmentOutput {
    /// Combined CSS traversal for consumers that paint a complete scene.
    pub scene_paint_order: Vec<ScenePaintItem>,
    pub decorations: DecorationStore,
    /// Owning line for inline decorations, or `u32::MAX` for block and
    /// line-independent fragments. Renderers must not infer this relationship
    /// from decoration geometry.
    pub decoration_line_indices: Vec<u32>,
    /// Sparse source-token order for line-owned inline decorations. Block and
    /// line-independent decorations use `u32::MAX`.
    pub decoration_paint_orders: Vec<u32>,
    /// Decoration indexes in final paint traversal order, grouped by their
    /// owning line. Line-owned backgrounds and borders replay with the line
    /// instead of in the document-wide block-decoration phase.
    pub decoration_fragments_by_line: GroupedArena<usize>,
    pub decoration_positioned_layers: Vec<bool>,
    pub decoration_negative_positioned_layers: Vec<bool>,
    pub decoration_independent_positioned_layers: Vec<bool>,
    /// Semantic block-paint traversal. Recursive formatting emits block
    /// fragments postorder; these ranges provide preorder traversal without
    /// flattening or physically reordering fragment storage.
    pub block_paint_ranges: Vec<Range<u32>>,
    pub block_decoration_count: u32,
    /// Block decoration indexes sorted by vertical start, plus prefix maxima
    /// and inverse CSS ranks for bounded viewport interval queries.
    pub block_decoration_indices_by_y: Vec<u32>,
    pub block_decoration_prefix_max_y1: Vec<f64>,
    pub block_decoration_paint_ranks: Vec<u32>,
    /// Sparse resolved ancestor-overflow clips, indexed by decoration. Empty
    /// when the document does not establish an overflow clip.
    pub decoration_clips: Vec<Option<OverflowClip>>,
    pub image_fragments: Vec<ImageFragment>,
    pub image_fragments_by_line: GroupedArena<usize>,
}

#[derive(Clone, Copy)]
pub enum ScenePaintItem {
    Decoration(u32),
    Line(u32),
}

#[derive(Default, Clone)]
pub struct SemanticIndexes {
    pub anchor_positions: FxHashMap<u16, AnchorPosition>,
}

impl LayoutState {
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.extend_prefixed(
            "LayoutState.paint_program",
            self.paint_program.memory_usage_report(),
        );
        report.add_slice_storage::<Line>(
            "LayoutState.line_output.lines.storage",
            self.line_output.lines.capacity(),
            self.line_output.lines.len(),
        );
        report.add_slice_storage::<LineDetails>(
            "LayoutState.line_output.line_details.storage",
            self.line_output.line_details.capacity(),
            self.line_output.line_details.len(),
        );
        report.add_slice_storage::<LineTextFragment>(
            "LayoutState.line_output.text_fragments.storage",
            self.line_output.text_fragments.capacity(),
            self.line_output.text_fragments.len(),
        );
        report.add_slice_storage::<u32>(
            "LayoutState.line_output.paint_order_indices.storage",
            self.line_output.paint_order_indices.capacity(),
            self.line_output.paint_order_indices.len(),
        );
        report.add_slice_storage::<u32>(
            "LayoutState.line_output.paint_order_ranks.storage",
            self.line_output.paint_order_ranks.capacity(),
            self.line_output.paint_order_ranks.len(),
        );
        report.add_slice_storage::<LineInlineBoxFragment>(
            "LayoutState.line_output.inline_box_fragments.storage",
            self.line_output.inline_box_fragments.capacity(),
            self.line_output.inline_box_fragments.len(),
        );
        report.add_slice_storage::<bool>(
            "LayoutState.line_output.positioned_layers.storage",
            self.line_output.positioned_layers.capacity(),
            self.line_output.positioned_layers.len(),
        );
        report.add_slice_storage::<bool>(
            "LayoutState.line_output.negative_positioned_layers.storage",
            self.line_output.negative_positioned_layers.capacity(),
            self.line_output.negative_positioned_layers.len(),
        );
        report.add_slice_storage::<bool>(
            "LayoutState.line_output.independent_positioned_layers.storage",
            self.line_output.independent_positioned_layers.capacity(),
            self.line_output.independent_positioned_layers.len(),
        );
        report.add_slice_storage::<Option<OverflowClip>>(
            "LayoutState.line_output.line_clips.storage",
            self.line_output.line_clips.capacity(),
            self.line_output.line_clips.len(),
        );
        report.extend_prefixed(
            "LayoutState.line_output.glyph_line_indices",
            self.line_output.glyph_line_indices.memory_usage_report(),
        );
        report.extend_prefixed(
            "LayoutState.line_output.line_glyph_offsets",
            self.line_output.line_glyph_offsets.memory_usage_report(),
        );
        report.extend_prefixed(
            "LayoutState.line_output.line_glyph_advances",
            self.line_output.line_glyph_advances.memory_usage_report(),
        );
        report.add_slice_storage::<PreparedTextRunFragment>(
            "LayoutState.line_output.prepared_text_run_fragments.storage",
            self.line_output.prepared_text_run_fragments.capacity(),
            self.line_output.prepared_text_run_fragments.len(),
        );
        report.add_slice_storage::<EllipsisFragment>(
            "LayoutState.line_output.ellipsis_fragments.storage",
            self.line_output.ellipsis_fragments.capacity(),
            self.line_output.ellipsis_fragments.len(),
        );
        report.add_slice_storage::<HyphenFragment>(
            "LayoutState.line_output.hyphen_fragments.storage",
            self.line_output.hyphen_fragments.capacity(),
            self.line_output.hyphen_fragments.len(),
        );
        report.add_slice_storage::<ScenePaintItem>(
            "LayoutState.fragment_output.scene_paint_order.storage",
            self.fragment_output.scene_paint_order.capacity(),
            self.fragment_output.scene_paint_order.len(),
        );
        report.add_slice_storage::<DecorationFragment>(
            "LayoutState.fragment_output.decorations.storage",
            self.fragment_output.decorations.fragment_capacity(),
            self.fragment_output.decorations.len(),
        );
        report.add_slice_storage::<u32>(
            "LayoutState.fragment_output.block_decoration_indices_by_y.storage",
            self.fragment_output
                .block_decoration_indices_by_y
                .capacity(),
            self.fragment_output.block_decoration_indices_by_y.len(),
        );
        report.add_slice_storage::<f64>(
            "LayoutState.fragment_output.block_decoration_prefix_max_y1.storage",
            self.fragment_output
                .block_decoration_prefix_max_y1
                .capacity(),
            self.fragment_output.block_decoration_prefix_max_y1.len(),
        );
        report.add_slice_storage::<u32>(
            "LayoutState.fragment_output.block_decoration_paint_ranks.storage",
            self.fragment_output.block_decoration_paint_ranks.capacity(),
            self.fragment_output.block_decoration_paint_ranks.len(),
        );
        report.add_slice_storage::<u32>(
            "LayoutState.fragment_output.decoration_line_indices.storage",
            self.fragment_output.decoration_line_indices.capacity(),
            self.fragment_output.decoration_line_indices.len(),
        );
        report.add_slice_storage::<u32>(
            "LayoutState.fragment_output.decoration_paint_orders.storage",
            self.fragment_output.decoration_paint_orders.capacity(),
            self.fragment_output.decoration_paint_orders.len(),
        );
        report.extend_prefixed(
            "LayoutState.fragment_output.decoration_fragments_by_line",
            self.fragment_output
                .decoration_fragments_by_line
                .memory_usage_report(),
        );
        report.add_slice_storage::<bool>(
            "LayoutState.fragment_output.decoration_positioned_layers.storage",
            self.fragment_output.decoration_positioned_layers.capacity(),
            self.fragment_output.decoration_positioned_layers.len(),
        );
        report.add_slice_storage::<bool>(
            "LayoutState.fragment_output.decoration_negative_positioned_layers.storage",
            self.fragment_output
                .decoration_negative_positioned_layers
                .capacity(),
            self.fragment_output
                .decoration_negative_positioned_layers
                .len(),
        );
        report.add_slice_storage::<bool>(
            "LayoutState.fragment_output.decoration_independent_positioned_layers.storage",
            self.fragment_output
                .decoration_independent_positioned_layers
                .capacity(),
            self.fragment_output
                .decoration_independent_positioned_layers
                .len(),
        );
        report.add_slice_storage::<Range<u32>>(
            "LayoutState.fragment_output.block_paint_ranges.storage",
            self.fragment_output.block_paint_ranges.capacity(),
            self.fragment_output.block_paint_ranges.len(),
        );
        report.add_slice_storage::<Option<OverflowClip>>(
            "LayoutState.fragment_output.decoration_clips.storage",
            self.fragment_output.decoration_clips.capacity(),
            self.fragment_output.decoration_clips.len(),
        );
        report.add_slice_storage::<RoundedDecoration>(
            "LayoutState.fragment_output.decorations.rounded_storage",
            self.fragment_output.decorations.rounded_capacity(),
            self.fragment_output.decorations.rounded_len(),
        );
        report.add_slice_storage::<ImageFragment>(
            "LayoutState.fragment_output.image_fragments.storage",
            self.fragment_output.image_fragments.capacity(),
            self.fragment_output.image_fragments.len(),
        );
        report.extend_prefixed(
            "LayoutState.fragment_output.image_fragments_by_line",
            self.fragment_output
                .image_fragments_by_line
                .memory_usage_report(),
        );
        report.add_slice_storage::<(u16, AnchorPosition)>(
            "LayoutState.semantic_indexes.anchor_positions.storage",
            self.semantic_indexes.anchor_positions.capacity(),
            self.semantic_indexes.anchor_positions.len(),
        );
        report
    }
}

/// Resolved overflow clip with independent physical-axis constraints. Keeping
/// the axes explicit avoids accidentally clipping the visible axis for mixed
/// declarations such as `overflow-x: clip; overflow-y: visible`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverflowClip {
    pub rect: Rect,
    pub x: bool,
    pub y: bool,
}

#[derive(Clone, Copy)]
pub struct AnchorPosition {
    pub y: f64,
    pub order: u32,
}

#[derive(Clone, Default)]
pub struct Line {
    /// Block formatting context that emitted this line. Retained with the
    /// line so consumers can inspect semantics for a visible range without
    /// walking every box or text run in the document.
    pub owner_box_idx: u32,
    pub glyphs: Range<u32>,
    pub point: Point,
    pub height: f64,
    pub baseline: f64,
    pub word_spacing: f64, // extra space to add after each space (for justification)
    pub letter_spacing: f64, // micro-tracking after eligible shaped-cluster boundaries
    /// One-based index into the sparse line-detail arena. Plain lines need no entry.
    pub details: Option<NonZeroU32>,
    /// Range into `LineOutput::prepared_text_run_fragments`.
    pub prepared_text_runs: Range<u32>,
    /// False when authoritative native-run coverage was incomplete.
    pub native_text_runs_complete: bool,
}

#[derive(Clone, Default)]
pub struct LineDetails {
    /// Paint-only optical margin shift, excluded from logical line breaking.
    pub optical_offset_x: f64,
    /// First-line pseudo color and refinement marker. Ordinary lines use the
    /// retained backend colors or the document's updated source paint colors.
    pub paint_color: Option<u32>,
    /// Range into LineOutput::text_fragments. None implicitly paints the
    /// complete line glyph range; Some can represent gaps or hidden text.
    pub text_fragments: Option<Range<u32>>,
    /// Range into LineOutput::inline_box_fragments, retaining occupied spans
    /// for inline backgrounds, borders, and propagated text decorations.
    pub inline_box_fragments: Range<u32>,
}

impl Line {
    pub fn details<'a>(&self, details: &'a [LineDetails]) -> Option<&'a LineDetails> {
        self.details
            .and_then(|index| details.get(index.get() as usize - 1))
    }
    pub fn optical_offset_x(&self, details: &[LineDetails]) -> f64 {
        self.details(details)
            .map_or(0.0, |details| details.optical_offset_x)
    }
    pub fn paint_color(&self, details: &[LineDetails]) -> Option<u32> {
        self.details(details)
            .and_then(|details| details.paint_color)
    }
    pub fn text_fragment_range(&self, details: &[LineDetails]) -> Option<Range<u32>> {
        self.details(details)
            .and_then(|details| details.text_fragments.clone())
    }
    pub fn inline_box_fragments(&self, details: &[LineDetails]) -> Range<u32> {
        self.details(details)
            .map_or(0..0, |details| details.inline_box_fragments.clone())
    }
}

impl LineOutput {
    pub fn push_details(
        &mut self,
        optical_offset_x: f64,
        paint_color: Option<u32>,
        text_fragments: Option<Box<[LineTextFragment]>>,
        inline_box_fragments: Range<u32>,
    ) -> Option<NonZeroU32> {
        if optical_offset_x == 0.0
            && paint_color.is_none()
            && text_fragments.is_none()
            && inline_box_fragments.is_empty()
        {
            return None;
        }
        let text_fragments = text_fragments.map(|fragments| {
            let start = u32::try_from(self.text_fragments.len())
                .expect("line text-fragment capacity exhausted");
            self.text_fragments.extend(Vec::from(fragments));
            let end = u32::try_from(self.text_fragments.len())
                .expect("line text-fragment capacity exhausted");
            start..end
        });
        let index = self
            .line_details
            .len()
            .checked_add(1)
            .and_then(|len| u32::try_from(len).ok())
            .and_then(NonZeroU32::new)
            .expect("line detail capacity exhausted");
        self.line_details.push(LineDetails {
            optical_offset_x,
            paint_color,
            text_fragments,
            inline_box_fragments,
        });
        Some(index)
    }
}

/// One authoritative shaped-run slice positioned relative to its line.
///
/// Pagination supplies the line's screen origin; painters only translate
/// this retained geometry and never need to reconstruct glyph placement.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedTextRunFragment {
    pub run: crate::TextRunId,
    pub range: Range<u32>,
    pub offset: Point,
    pub color: Option<u32>,
}

impl PreparedTextRunFragment {
    pub fn run(&self) -> crate::TextRunId {
        self.run
    }

    pub fn range(&self) -> Range<u32> {
        self.range.clone()
    }

    /// Updated paint color, independent of the retained shaping resource.
    pub fn color(&self) -> Option<u32> {
        self.color
    }

    pub fn offset(&self) -> Point {
        self.offset
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LineTextFragment {
    pub glyphs: Range<u32>,
    pub offset_x: f64,
    pub rtl: bool,
    /// Source-token order within the owning line. Sparse text fragments are
    /// already split at every inline boundary and replaced item, making this
    /// sufficient to merge paint without retaining the dense token stream.
    pub paint_order: u32,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineInlineBoxFragment {
    pub box_idx: u32,
    /// Earliest source-token order covered by this fragment on the line.
    pub paint_order: u32,
    /// Offsets from the line's physical inline start, including the item's
    /// margins because those consume space inside its inline ancestor.
    pub start_x: f32,
    pub end_x: f32,
    /// Vertical geometry relative to the owning line origin.
    pub top: f32,
    pub bottom: f32,
    pub baseline: f32,
    pub flags: u8,
}

impl LineInlineBoxFragment {
    pub const INLINE_START: u8 = 1 << 0;
    pub const INLINE_END: u8 = 1 << 1;
    /// The stored bounds already describe the border box. Ordinary inline
    /// fragments store their content bounds and expand them during decoration
    /// emission; replaced inline content has an independently sized border
    /// box, so expanding it a second time would duplicate its edges.
    pub const BORDER_BOX_BOUNDS: u8 = 1 << 2;
}

// Per-line inline fragments are intentionally fixed-width and cache compact:
// two indexes, five relative coordinates, and edge flags.
const _: () = assert!(std::mem::size_of::<LineInlineBoxFragment>() <= 32);

#[derive(Clone, Default)]
pub struct GlyphOffsetRun {
    pub range: Range<u32>,
    pub offset: f32,
}

/// Sparse horizontal advance overrides produced for position-dependent glyphs
/// such as preserved tabs. Ordinary glyphs remain on the metric fast path.
#[derive(Clone, Default)]
pub struct GlyphAdvanceRun {
    pub range: Range<u32>,
    pub advance: f32,
}

/// Synthetic ellipsis painted at the end of a clipped source-glyph prefix.
/// It intentionally has no DOM glyph index and therefore cannot leak into
/// selection, search, or CFI addressing.
#[derive(Clone, Copy)]
pub struct EllipsisFragment {
    pub line_idx: usize,
    pub glyph: crate::GlyphId,
    pub offset: Point,
    pub visible: bool,
}

/// Synthetic hyphen painted only when an automatic discretionary break wins.
/// It has no source glyph index and therefore stays outside selection/CFI data.
#[derive(Clone, Copy)]
pub struct HyphenFragment {
    pub line_idx: usize,
    pub glyph: crate::GlyphId,
    pub offset: Point,
    pub visible: bool,
}

#[derive(Clone)]
pub struct DecorationFragment {
    pub rect: Rect,
    pub color: u32,
    metadata: u32,
}

/// Compact decoration storage. Rounded geometry is a sparse implementation
/// detail rather than a second collection that callers must keep synchronized.
#[derive(Clone, Default)]
pub struct DecorationStore {
    fragments: Vec<DecorationFragment>,
    rounded: Vec<RoundedDecoration>,
}

impl DecorationStore {
    pub fn len(&self) -> usize {
        self.fragments.len()
    }

    pub fn clear(&mut self) {
        self.fragments.clear();
        self.rounded.clear();
    }

    pub fn fragments(&self) -> &[DecorationFragment] {
        &self.fragments
    }

    pub fn fragments_mut(&mut self) -> &mut [DecorationFragment] {
        &mut self.fragments
    }

    pub fn push(&mut self, fragment: DecorationFragment) {
        self.fragments.push(fragment);
    }

    pub fn push_rounded_border(
        &mut self,
        rect: Rect,
        color: u32,
        is_inline: bool,
        rounded: RoundedDecoration,
    ) {
        let index = self.push_rounded(rounded);
        self.fragments.push(DecorationFragment::rounded_border(
            rect, color, is_inline, index,
        ));
    }

    pub fn push_rounded_background(
        &mut self,
        rect: Rect,
        color: u32,
        is_inline: bool,
        rounded: RoundedDecoration,
    ) {
        let index = self.push_rounded(rounded);
        self.fragments.push(DecorationFragment::rounded_background(
            rect, color, is_inline, index,
        ));
    }

    pub fn rounded_for(&self, fragment: &DecorationFragment) -> Option<&RoundedDecoration> {
        fragment
            .rounded_index()
            .and_then(|index| self.rounded.get(index))
    }

    fn push_rounded(&mut self, rounded: RoundedDecoration) -> usize {
        let index = self.rounded.len();
        self.rounded.push(rounded);
        index
    }

    fn fragment_capacity(&self) -> usize {
        self.fragments.capacity()
    }

    fn rounded_capacity(&self) -> usize {
        self.rounded.capacity()
    }

    fn rounded_len(&self) -> usize {
        self.rounded.len()
    }
}

/// Semantic decoration execution retained across the layout/render boundary.
/// Layout resolves the occupied geometry; render core expands patterned
/// strokes into painter primitives.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum DecorationPattern {
    #[default]
    Solid = 0,
    DoubleHorizontal = 1,
    DottedHorizontal = 2,
    DashedHorizontal = 3,
    DottedVertical = 4,
    DashedVertical = 5,
}

impl DecorationFragment {
    const INLINE_BIT: u32 = 1 << 31;
    const FOREGROUND_BIT: u32 = 1 << 30;
    const BACKGROUND_BIT: u32 = 1 << 29;
    const BORDER_BIT: u32 = 1 << 25;
    const PATTERN_SHIFT: u32 = 26;
    const PATTERN_MASK: u32 = 0b111 << Self::PATTERN_SHIFT;
    const FLAG_BITS: u32 = Self::INLINE_BIT
        | Self::FOREGROUND_BIT
        | Self::BACKGROUND_BIT
        | Self::BORDER_BIT
        | Self::PATTERN_MASK;

    #[cfg(any(test, feature = "testing"))]
    pub fn rect(rect: Rect, color: u32, is_inline: bool) -> Self {
        Self {
            rect,
            color,
            metadata: if is_inline { Self::INLINE_BIT } else { 0 },
        }
    }

    pub fn background_rect(rect: Rect, color: u32, is_inline: bool) -> Self {
        Self {
            rect,
            color,
            metadata: Self::BACKGROUND_BIT | if is_inline { Self::INLINE_BIT } else { 0 },
        }
    }

    pub fn border_rect(rect: Rect, color: u32, is_inline: bool, foreground: bool) -> Self {
        Self {
            rect,
            color,
            metadata: Self::BORDER_BIT
                | if is_inline { Self::INLINE_BIT } else { 0 }
                | if foreground { Self::FOREGROUND_BIT } else { 0 },
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn foreground_rect(rect: Rect, color: u32, is_inline: bool) -> Self {
        Self {
            rect,
            color,
            metadata: Self::FOREGROUND_BIT | if is_inline { Self::INLINE_BIT } else { 0 },
        }
    }

    pub fn patterned(
        rect: Rect,
        color: u32,
        is_inline: bool,
        foreground: bool,
        pattern: DecorationPattern,
    ) -> Self {
        Self {
            rect,
            color,
            metadata: ((pattern as u32) << Self::PATTERN_SHIFT)
                | if is_inline { Self::INLINE_BIT } else { 0 }
                | if foreground { Self::FOREGROUND_BIT } else { 0 },
        }
    }

    pub fn border_patterned(
        rect: Rect,
        color: u32,
        is_inline: bool,
        foreground: bool,
        pattern: DecorationPattern,
    ) -> Self {
        Self {
            rect,
            color,
            metadata: Self::BORDER_BIT
                | ((pattern as u32) << Self::PATTERN_SHIFT)
                | if is_inline { Self::INLINE_BIT } else { 0 }
                | if foreground { Self::FOREGROUND_BIT } else { 0 },
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn rounded(rect: Rect, color: u32, is_inline: bool, rounded_index: usize) -> Self {
        assert!(
            rounded_index < ((1 << 25) - 1) as usize,
            "too many rounded decorations"
        );
        Self {
            rect,
            color,
            metadata: (rounded_index as u32 + 1) | if is_inline { Self::INLINE_BIT } else { 0 },
        }
    }

    pub fn rounded_border(rect: Rect, color: u32, is_inline: bool, rounded_index: usize) -> Self {
        assert!(
            rounded_index < ((1 << 25) - 1) as usize,
            "too many rounded decorations"
        );
        Self {
            rect,
            color,
            metadata: Self::BORDER_BIT
                | (rounded_index as u32 + 1)
                | if is_inline { Self::INLINE_BIT } else { 0 },
        }
    }

    pub fn rounded_background(
        rect: Rect,
        color: u32,
        is_inline: bool,
        rounded_index: usize,
    ) -> Self {
        assert!(
            rounded_index < ((1 << 25) - 1) as usize,
            "too many rounded decorations"
        );
        Self {
            rect,
            color,
            metadata: Self::BACKGROUND_BIT
                | (rounded_index as u32 + 1)
                | if is_inline { Self::INLINE_BIT } else { 0 },
        }
    }

    pub fn is_inline(&self) -> bool {
        self.metadata & Self::INLINE_BIT != 0
    }

    pub fn rounded_index(&self) -> Option<usize> {
        let encoded = self.metadata & !Self::FLAG_BITS;
        (encoded != 0).then(|| (encoded - 1) as usize)
    }

    pub fn is_foreground(&self) -> bool {
        self.metadata & Self::FOREGROUND_BIT != 0
    }

    pub fn is_background(&self) -> bool {
        self.metadata & Self::BACKGROUND_BIT != 0
    }

    pub fn is_border(&self) -> bool {
        self.metadata & Self::BORDER_BIT != 0
    }

    pub fn pattern(&self) -> DecorationPattern {
        match (self.metadata & Self::PATTERN_MASK) >> Self::PATTERN_SHIFT {
            1 => DecorationPattern::DoubleHorizontal,
            2 => DecorationPattern::DottedHorizontal,
            3 => DecorationPattern::DashedHorizontal,
            4 => DecorationPattern::DottedVertical,
            5 => DecorationPattern::DashedVertical,
            _ => DecorationPattern::Solid,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RoundedDecoration {
    pub radii: UsedBorderRadii,
    /// `None` is a filled background; `Some` is a uniform solid border width.
    pub border_width: Option<f32>,
}

#[derive(Clone)]
pub struct ImageFragment {
    pub line_idx: usize,
    pub image_idx: u32,
    pub box_idx: u32,
    pub offset: Point,
    pub size: Size,
    pub clip: Rect,
    pub paint_order: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_radius_metadata_does_not_grow_common_decoration_fragments() {
        assert_eq!(std::mem::size_of::<DecorationFragment>(), 40);
        let plain = DecorationFragment::rect(Rect::ZERO, 0, true);
        assert!(plain.is_inline());
        assert_eq!(plain.rounded_index(), None);
        let rounded = DecorationFragment::rounded(Rect::ZERO, 0, false, 12);
        assert!(!rounded.is_inline());
        assert_eq!(rounded.rounded_index(), Some(12));
        let foreground = DecorationFragment::foreground_rect(Rect::ZERO, 0, true);
        assert!(foreground.is_inline());
        assert!(foreground.is_foreground());
        assert_eq!(foreground.rounded_index(), None);
        let background = DecorationFragment::background_rect(Rect::ZERO, 0, true);
        assert!(background.is_background());
        assert!(!background.is_foreground());
        assert_eq!(background.rounded_index(), None);
        let rounded_background = DecorationFragment::rounded_background(Rect::ZERO, 0, true, 7);
        assert!(rounded_background.is_background());
        assert_eq!(rounded_background.rounded_index(), Some(7));
        let border = DecorationFragment::border_rect(Rect::ZERO, 0, true, false);
        assert!(border.is_border());
        assert!(!border.is_background());
        let rounded_border = DecorationFragment::rounded_border(Rect::ZERO, 0, true, 9);
        assert!(rounded_border.is_border());
        assert_eq!(rounded_border.rounded_index(), Some(9));
        let patterned = DecorationFragment::patterned(
            Rect::ZERO,
            0,
            true,
            true,
            DecorationPattern::DashedHorizontal,
        );
        assert_eq!(patterned.pattern(), DecorationPattern::DashedHorizontal);
        assert!(patterned.is_inline());
        assert!(patterned.is_foreground());
    }

    #[test]
    fn decoration_store_owns_sparse_rounded_geometry() {
        let mut decorations = DecorationStore::default();
        decorations.push(DecorationFragment::background_rect(Rect::ZERO, 1, false));
        let radii = UsedBorderRadii {
            top_left: (4.0, 5.0),
            ..UsedBorderRadii::default()
        };
        decorations.push_rounded_background(
            Rect::ZERO,
            2,
            false,
            RoundedDecoration {
                radii,
                border_width: None,
            },
        );

        assert!(
            decorations
                .rounded_for(&decorations.fragments()[0])
                .is_none()
        );
        let rounded = decorations
            .rounded_for(&decorations.fragments()[1])
            .expect("rounded details");
        assert_eq!(rounded.radii, radii);
        assert_eq!(rounded.border_width, None);
    }

    #[test]
    fn durable_inline_fragment_record_stays_cache_compact() {
        assert_eq!(std::mem::size_of::<LineInlineBoxFragment>(), 32);
    }
}
