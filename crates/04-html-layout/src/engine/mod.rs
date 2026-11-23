//! Shared execution state, immutable input access, and pass configuration.
//! The crate-root layout module coordinates the document pass.

mod children;
mod paragraph_spacing;
mod preparation;
mod reader;

use html_dom::{Document, MemoryUsageReport};
use html_engine_model::{
    BoxGeometry, GlyphId, GlyphMetrics, GlyphResourceGeneration, GlyphSlice, ImageMetrics,
    InlineContent, LayoutConstraints, LayoutState, LayoutTree, PaintView, ShapedFontMetrics,
    TextGeometryView, TextPaintRange, WordIndex,
};
use html_style_model::ComputedStyles;
use html_text::FirstLetterStyles;
use std::mem::{swap, take};
use rustc_data_structures::fx::FxHashMap;

use super::inline::{
    HyphenationDictionaryCache, InlinePlanCache, InlinePlanGeneration, ParagraphLayouts,
    ParagraphScratch,
};
use super::{
    block::{FloatState, MarginAnalysis},
    boxes::absolute_positioning::AbsolutePositioningState,
    finalization::FinalizationScratch,
    paint::PaintOrderCache,
};
use crate::inline::reader::InlineReader;
use crate::output::FragmentWriter;
use crate::output::PlacementState;
pub(crate) use preparation::{BoxSizeSnapshot, PassContext};
pub(crate) use reader::{LayoutReader, StyleReader};

pub(super) fn execute_layout<'input, 'out, T>(
    read: LayoutReadContext<'input>,
    outputs: LayoutOutputs<'out>,
    pass_kind: LayoutPassKind,
    operation: impl FnOnce(&mut LayoutEngine<'input, 'out>) -> T,
) -> T {
    let mut engine = LayoutEngine::new(read, outputs, pass_kind);
    let result = operation(&mut engine);
    // Measurement plans return to the parent executor. Compact shared
    // arenas once after the complete layout, rather than after each probe.
    if engine.placement.retains_output() {
        engine.scratch.inline_plans.trim();
    }
    result
}

/// Allocation-owning state reused by successive layout passes. It is separate
/// from published geometry so render consumers cannot observe or depend on it.
#[derive(Default)]
pub struct LayoutScratch {
    pub(super) floats: FloatState,
    pub(super) margins: MarginAnalysis,
    pub(super) absolute_positioning: AbsolutePositioningState,
    pub(super) measurement: Option<Box<MeasurementState>>,
    pub(super) intrinsic_widths: Vec<crate::measurement::IntrinsicWidths>,
    // Retain only the latest request/result per box within this pass.
    pub(super) box_sizing: Vec<Option<crate::ResolvedBoxSizing>>,
    pub(super) cell_height_dependencies: FxHashMap<usize, crate::table::CellHeightDependency>,
    pub(crate) flex_grid: crate::flex_grid::FlexGridState,
    pub(super) finalization: FinalizationScratch,
    pub(crate) paint: PaintOrderCache,
    pub(super) paragraphs: Vec<ParagraphScratch>,
    pub(super) paragraph_layouts: ParagraphLayouts,
    pub(super) inline_plans: InlinePlanCache,
    pub(super) hyphenation_dictionary: HyphenationDictionaryCache,
}

impl LayoutScratch {
    fn begin_pass(&mut self, read: LayoutReadContext<'_>, pass_kind: LayoutPassKind) {
        // These results depend on immutable inputs for this pass. Nested
        // measurements borrow their owner's cache instead of resetting it.
        if pass_kind == LayoutPassKind::Final {
            self.intrinsic_widths.clear();
            self.intrinsic_widths
                .resize(read.reader.box_count(), Default::default());
            self.cell_height_dependencies.clear();
            self.box_sizing.clear();
            self.box_sizing.resize(read.reader.box_count(), None);
            self.flex_grid.clear();
        }
        self.inline_plans
            .begin_generation(read.inline_plan_generation);
        self.inline_plans
            .set_token_capacity_hint(read.text.glyphs().len());
        self.floats.reset();
        self.margins.reset();
        self.absolute_positioning.clear();
    }

    /// Transfer caches used by nested measurement, leaving all layout state in place.
    /// Inline plans move to the recipient; the other caches exchange reusable storage.
    pub(super) fn transfer_measurement_caches(&mut self, target: &mut Self) {
        target.inline_plans = take(&mut self.inline_plans);
        swap(&mut self.intrinsic_widths, &mut target.intrinsic_widths);
        swap(&mut self.box_sizing, &mut target.box_sizing);
        swap(
            &mut self.cell_height_dependencies,
            &mut target.cell_height_dependencies,
        );
        swap(&mut self.flex_grid, &mut target.flex_grid);
    }

    pub fn inline_plan_memory_report(&self) -> MemoryUsageReport {
        self.inline_plans.memory_usage_report()
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn inline_plan_count(&self) -> usize {
        self.inline_plans.len()
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn inline_plan_pointer(&self) -> Option<usize> {
        self.inline_plans.first_token_pointer()
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn paragraph_layout_bytes(&self) -> usize {
        self.paragraph_layouts.memory_usage_bytes()
    }
    pub fn paragraph_layout_memory_report(&self) -> MemoryUsageReport {
        self.paragraph_layouts.memory_usage_report()
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn paragraph_reuse_counts(&self) -> (usize, usize) {
        self.paragraph_layouts.reuse_counts()
    }
    #[cfg(any(test, feature = "testing"))]
    pub fn clear_paragraph_layouts(&mut self) {
        self.paragraph_layouts = Default::default();
    }
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
    pub ellipsis_glyphs: &'a FxHashMap<u32, GlyphId>,
    pub hyphen_glyphs: &'a FxHashMap<u32, GlyphId>,
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
    Measurement,
}

pub struct LayoutEngine<'a, 'out> {
    pub read: LayoutReadContext<'a>,
    pub geometry: &'out mut BoxGeometry,
    pub fragments: FragmentWriter<'out>,
    pub(crate) placement: &'out mut PlacementState,
    pub(crate) scratch: &'out mut LayoutScratch,
}

impl<'a, 'out> LayoutEngine<'a, 'out> {
    fn new(
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
        scratch.begin_pass(read, pass_kind);
        let mut fragments = FragmentWriter::new(state);
        geometry.reset(read.reader.box_count());
        fragments.reset();
        match pass_kind {
            LayoutPassKind::Final => placement.reset(read.reader.box_count()),
            LayoutPassKind::Measurement => placement.reset_for_measurement(),
        }
        Self {
            read,
            geometry,
            fragments,
            placement,
            scratch,
        }
    }
}

/// Immutable inputs borrowed by normal layout and isolated measurements.
#[derive(Clone, Copy)]
pub struct LayoutReadContext<'a> {
    pub(crate) constraints: LayoutConstraints,
    pub(crate) reader: LayoutReader<'a>,
    pub(crate) text: InlineReader<'a>,
    pub(crate) inline_plan_generation: InlinePlanGeneration,
    pub(crate) text_style_ranges: &'a [TextPaintRange],
    pub(crate) resource_generation: GlyphResourceGeneration,
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
            constraints,
            reader,
            text: InlineReader::from_inputs(&inputs),
            inline_plan_generation: inputs.inline_plan_generation,
            text_style_ranges: inputs.text_style_ranges,
            resource_generation: inputs.resource_generation,
        }
    }
}
