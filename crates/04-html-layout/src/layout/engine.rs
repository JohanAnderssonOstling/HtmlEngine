use html_engine_model::{
    BoxGeometry, GlyphId, GlyphMetrics, GlyphResourceGeneration, GlyphSlice, ImageMetrics,
    ImageSizingPolicy, InlineContent, LayoutConstraints, LayoutState, LayoutTree, PaintView,
    ShapedFontMetrics, TextGeometryView, TextPaintRange, WordIndex};
use html_dom::{Document, MemoryUsageReport};
use html_style_model::ComputedStyles;
use html_text::FirstLetterStyles;
use kurbo::{Point, Size};

use super::fragment_writer::FragmentWriter;
use super::inline::{
    HyphenationDictionaryCache, InlinePlanCache, InlinePlanGeneration, ParagraphLayouts,
    ParagraphScratch};
use super::inline_reader::InlineReader;
use super::placement::PlacementState;
use super::read_context::{PassContext, LayoutReader};
use super::{
    absolute_positioning::AbsolutePositioningState,
    block::{FloatState, MarginAnalysis},
    config::LayoutConfig,
    s03_finalize::FinalizationScratch,
};

pub use super::s00_layout::{compute_layout, repaint};

/// Allocation-owning state reused by successive layout passes. It is separate
/// from published geometry so render consumers cannot observe or depend on it.
#[derive(Default)]
pub struct LayoutScratch {
    pub(super) floats: FloatState,
    pub(super) margins: MarginAnalysis,
    pub(super) absolute_positioning: AbsolutePositioningState,
    pub(super) measurement: Option<Box<MeasurementState>>,
    pub(super) intrinsic_widths: Vec<super::intrinsic_sizing::IntrinsicWidths>,
    pub(crate) flex_grid: crate::flex_grid::FlexGridState,
    pub(super) finalization: FinalizationScratch,
    pub(super) paragraphs: Vec<ParagraphScratch>,
    pub(super) paragraph_layouts: ParagraphLayouts,
    pub(super) inline_plans: InlinePlanCache,
    pub(super) hyphenation_dictionary: HyphenationDictionaryCache,
}

impl Clone for LayoutScratch {
    fn clone(&self) -> Self {
        // An independent executor starts without copying retained execution
        // buffers or publication locators.
        Self::default()
    }
}

impl LayoutScratch {
    pub fn inline_plan_memory_report(&self) -> MemoryUsageReport { self.inline_plans.memory_usage_report() }
    #[cfg(any(test, feature = "testing"))]
    pub fn inline_plan_count(&self) -> usize { self.inline_plans.len() }
    #[cfg(any(test, feature = "testing"))]
    pub fn inline_plan_pointer(&self) -> Option<usize> { self.inline_plans.first_token_pointer() }
    #[cfg(any(test, feature = "testing"))]
    pub fn paragraph_layout_bytes(&self) -> usize { self.paragraph_layouts.memory_usage_bytes() }
    pub fn paragraph_layout_memory_report(&self) -> MemoryUsageReport { self.paragraph_layouts.memory_usage_report() }
    #[cfg(any(test, feature = "testing"))]
    pub fn paragraph_reuse_counts(&self) -> (usize, usize) { self.paragraph_layouts.reuse_counts() }
    #[cfg(any(test, feature = "testing"))]
    pub fn clear_paragraph_layouts(&mut self) { self.paragraph_layouts = Default::default(); }
    #[cfg(any(test, feature = "testing"))]
    pub fn paragraph_layout_hits(&self, box_idx: usize) -> usize {
        self.paragraph_layouts.layout_hits(box_idx)
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn paragraph_capacities(&self) -> Vec<(usize, (usize, usize, usize))> {
        self.paragraphs
            .iter()
            .map(ParagraphScratch::capacities)
            .collect()
    }
}

pub struct LayoutInputs<'a> {
    pub document: &'a Document,
    pub styles: &'a ComputedStyles,
    pub topology: &'a LayoutTree,
    pub inline_content: &'a InlineContent,
    pub first_letter_styles: &'a FirstLetterStyles,
    pub glyphs: GlyphSlice<'a>,
    pub glyph_metrics: &'a GlyphMetrics,
    pub font_metrics: &'a ShapedFontMetrics,
    pub text_geometry: Option<TextGeometryView<'a>>,
    pub word_index: Option<&'a WordIndex>,
    pub ellipsis_glyphs: &'a rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
    pub hyphen_glyphs: &'a rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
    pub inline_plan_generation: InlinePlanGeneration,
    pub image_metrics: &'a ImageMetrics,
    pub paint: PaintView<'a>,
    pub text_style_ranges: &'a [TextPaintRange],
    pub resource_generation: GlyphResourceGeneration,
}

pub struct LayoutOutputs<'out> {
    pub geometry: &'out mut BoxGeometry,
    pub state: &'out mut LayoutState,
    pub placement: &'out mut PlacementState,
    pub scratch: &'out mut LayoutScratch,
}

/// Selects publication behavior independently of temporary fragmentation suppression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LayoutPassKind {
    Final,
    Probe,
}

pub struct LayoutEngine<'a, 'out> {
    pub config: LayoutConfig,
    pub(super) inline_plan_generation: InlinePlanGeneration,
    pub(super) text_style_ranges: &'a [TextPaintRange],
    pub(super) resource_generation: GlyphResourceGeneration,
    pub(super) pass_kind: LayoutPassKind,
    pub reader: LayoutReader<'a>,
    pub text: InlineReader<'a>,
    pub geometry: &'out mut BoxGeometry,
    pub fragments: FragmentWriter<'out>,
    pub(crate) placement: &'out mut PlacementState,
    pub(crate) scratch: &'out mut LayoutScratch,
}

impl<'a, 'out> LayoutEngine<'a, 'out> {
    pub(super) fn new(
        read: LayoutReadContext<'a>,
        outputs: LayoutOutputs<'out>,
        pass_kind: LayoutPassKind,
    ) -> Self {
        let LayoutOutputs {
            geometry,
            state,
            placement,
            scratch,
        } = outputs;
        // These results depend on immutable inputs for this pass. Nested
        // measurements borrow their owner's cache instead of resetting it.
        if pass_kind == LayoutPassKind::Final {
            scratch.intrinsic_widths.resize(read.reader.box_count(), Default::default());
            scratch.intrinsic_widths.fill(Default::default());
            scratch.flex_grid.clear();
        }
        scratch
            .inline_plans
            .begin_generation(read.inline_plan_generation);
        scratch
            .inline_plans
            .set_compact_token_capacity_hint(read.text.glyphs().len());
        let fragments = FragmentWriter::new(state);
        Self {
            config: read.config,
            text_style_ranges: read.text_style_ranges,
            resource_generation: read.resource_generation,
            reader: read.reader,
            text: read.text,
            geometry,
            fragments,
            placement,
            scratch,
            inline_plan_generation: read.inline_plan_generation,
            pass_kind,
        }
    }

    pub(super) fn finish_pass(self) {
        // Measurement plans return to the parent executor. Compact shared
        // arenas once after the complete layout, rather than after each probe.
        if self.pass_kind == LayoutPassKind::Final {
            self.scratch.inline_plans.trim();
        }
    }

    pub fn measurement_context(&self) -> LayoutReadContext<'a> {
        LayoutReadContext {
            config: self.config,
            reader: self.reader,
            text: self.text,
            inline_plan_generation: self.inline_plan_generation,
            text_style_ranges: self.text_style_ranges,
            resource_generation: self.resource_generation,
        }
    }

    /// Resolve reader image fitting before line breaking and float placement.
    /// Images already inside the limits retain their exact resolved geometry.
    pub(super) fn fit_image(
        &self,
        box_idx: usize,
        size: Size,
        available_width: f64,
        outer: Size,
    ) -> Size {
        if self.config.image_sizing_policy() != ImageSizingPolicy::SmartStandalone
            || size.width <= 0.0
            || size.height <= 0.0
        {
            return size;
        }
        let width = (available_width.min(self.config.viewport_width()) - outer.width).max(0.0);
        let height = self
            .config
            .viewport_height()
            .filter(|height| *height > 0.0)
            .map_or(f64::INFINITY, |height| (height - outer.height).max(0.0));
        if (size.width <= width && size.height <= height)
            || self
                .reader
                .image_idx(box_idx)
                .and_then(|index| self.reader.document().image(index))
                .is_some_and(|image| matches!(image.source, html_dom::ImageSource::MathMl { .. }))
        {
            return size;
        }
        let scale = (width / size.width).min(height / size.height).min(1.0);
        Size::new(size.width * scale, size.height * scale)
    }

    pub(super) fn clear_layout_output(&mut self) {
        self.geometry.reset(self.reader.box_count());
        self.fragments.reset();
        match self.pass_kind {
            LayoutPassKind::Final => self.placement.reset(self.reader.box_count()),
            LayoutPassKind::Probe => self.placement.reset_for_measurement(),
        }
    }
}

/// Immutable inputs borrowed by normal layout and isolated measurements.
#[derive(Clone, Copy)]
pub struct LayoutReadContext<'a> {
    config: LayoutConfig,
    reader: LayoutReader<'a>,
    text: InlineReader<'a>,
    inline_plan_generation: InlinePlanGeneration,
    text_style_ranges: &'a [TextPaintRange],
    resource_generation: GlyphResourceGeneration,
}

pub(super) struct MeasurementState {
    geometry: BoxGeometry,
    state: LayoutState,
    placement: PlacementState,
    pub(super) scratch: LayoutScratch,
}

impl Default for MeasurementState {
    fn default() -> Self {
        Self {
            geometry: BoxGeometry::lazy(),
            state: LayoutState::default(),
            placement: PlacementState::default(),
            scratch: LayoutScratch::default(),
        }
    }
}

impl MeasurementState {
    pub(super) fn outputs(&mut self) -> LayoutOutputs<'_> {
        LayoutOutputs {
            geometry: &mut self.geometry,
            state: &mut self.state,
            placement: &mut self.placement,
            scratch: &mut self.scratch,
        }
    }
}

impl<'a> LayoutReadContext<'a> {
    pub(super) fn new(
        inputs: LayoutInputs<'a>,
        constraints: LayoutConstraints,
        pass_context: &'a PassContext<'a>,
    ) -> Self {
        let reader = LayoutReader::new(&inputs, pass_context);
        Self {
            config: LayoutConfig::new(constraints),
            reader,
            text: InlineReader::new(
                inputs.inline_content,
                inputs.first_letter_styles,
                inputs.glyphs,
                inputs.glyph_metrics,
                inputs.text_geometry,
                inputs.ellipsis_glyphs,
                inputs.hyphen_glyphs,
            )
            .with_word_index(inputs.word_index),
            inline_plan_generation: inputs.inline_plan_generation,
            text_style_ranges: inputs.text_style_ranges,
            resource_generation: inputs.resource_generation,
        }
    }
}
