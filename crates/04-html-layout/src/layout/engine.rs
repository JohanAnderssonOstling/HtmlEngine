use crate::layout_model::{
    BoxGeometry, GlyphId, GlyphMetrics, GlyphSlice, InlineContent, LayoutState, LayoutTree,
    ShapedFontMetrics, TextGeometryView,
};
use html_dom::{Document, MemoryUsageReport};
use html_text::FirstLetterStyles;
use html_style_model::ComputedStyles;
use kurbo::Point;
use std::cell::RefCell;
use std::time::Duration;
use web_time::Instant;

use super::fragment_writer::FragmentWriter;
use super::inline::{InlinePlanCache, InlinePlanGeneration};
use super::inline_reader::InlineReader;
use super::placement::PlacementState;
use super::read_context::{BoxStyleSnapshot, LayoutReader};
use super::{
    absolute_positioning::AbsolutePositioningState,
    block::{FloatState, MarginAnalysis},
    config::LayoutConfig,
    finalization::FinalizationScratch,
};

pub use super::execution::{layout_with_timings, repaint};

/// Allocation-owning state reused by successive layout passes. It is separate
/// from published geometry so render consumers cannot observe or depend on it.
#[derive(Default)]
pub struct LayoutScratch {
    floats: FloatState,
    margins: MarginAnalysis,
    absolute_positioning: AbsolutePositioningState,
    flex_grid: crate::flex_grid::FlexGridState,
    measurement: Option<Box<MeasurementWorker>>,
    finalization: FinalizationScratch,
    paragraphs: Vec<super::inline::ParagraphScratch>,
    paragraph_layouts: super::inline::ParagraphLayouts,
    pub(super) inline_plans: InlinePlanCache,
    block_decoration_owners: Vec<u32>,
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
        self.paragraphs.iter().map(super::inline::ParagraphScratch::capacities).collect()
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
    pub ellipsis_glyphs: &'a rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
    pub hyphen_glyphs: &'a rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
    pub inline_plan_generation: InlinePlanGeneration,
    pub image_metrics: &'a crate::ImageMetrics,
    pub paint: html_engine_model::PaintView<'a>,
    pub text_style_ranges: &'a [html_engine_model::TextPaintRange],
    pub resource_generation: crate::GlyphResourceGeneration,
}

pub struct LayoutOutputs<'out, 'scratch> {
    pub geometry: &'out mut BoxGeometry,
    pub state: &'out mut LayoutState,
    pub placement: &'scratch mut PlacementState,
    pub scratch: &'scratch mut LayoutScratch,
}

#[derive(Default, Debug, Clone)]
pub struct LayoutTimings {
    pub clear_layout_output: Duration,
    pub layout_tree_traversal: Duration,
    /// Inclusive wall time for the root box and every descendant it lays out.
    pub root_box_layout: Duration,
    /// Inclusive wall time for line sorting, decoration collection, and image indexing.
    pub finalize_layout: Duration,
    pub build_line_traversals: Duration,
    pub collect_inline_decorations: Duration,
    pub rebuild_image_fragments_by_line: Duration,
    pub layout_box_total: Duration,
    pub layout_block_children: Duration,
    pub layout_table: Duration,
    pub layout_table_row: Duration,
    pub layout_runs: Duration,
    pub layout_runs_around_float_exclusions: Duration,
    pub build_inline_tokens: Duration,
    pub build_inline_tokens_from_runs: Duration,
    pub break_lines: Duration,
    pub break_lines_knuth: Duration,
    pub emit_lines: Duration,
    pub measure_line: Duration,
    pub write_line_fragments: Duration,
    /// Lines measured during this pass, including intrinsic probes.
    pub measured_lines: usize,
    /// Paragraph-local line layouts reused without measurement.
    pub reused_lines: usize,
    pub place_float_anchor: Duration,
    pub layout_flex_grid: Duration,
    pub measure_flex_grid_item: Duration,
}

/// Start time for optional layout telemetry.
///
/// Normal layout does not request [`LayoutTimings`], so keeping the absence in
/// the token prevents hot layout paths from reading the system clock merely to
/// discard the result.
#[derive(Clone, Copy)]
pub struct LayoutTimingStart(Option<Instant>);

impl LayoutTimingStart {
    pub fn elapsed(self) -> Duration {
        self.0.map_or(Duration::ZERO, |started| started.elapsed())
    }
}

pub struct LayoutEngine<'a, 'out> {
    pub config: LayoutConfig,
    pub(super) floats: FloatState,
    pub(super) margins: MarginAnalysis,
    pub(super) absolute_positioning: AbsolutePositioningState,
    pub(super) inline_plans: InlinePlanCache,
    pub(super) inline_plan_generation: InlinePlanGeneration,
    pub flex_grid: crate::flex_grid::FlexGridState,
    pub(super) measurement: Option<Box<MeasurementWorker>>,
    pub(super) finalization: FinalizationScratch,
    pub(super) paragraphs: Vec<super::inline::ParagraphScratch>,
    pub(super) paragraph_layouts: super::inline::ParagraphLayouts,
    pub(super) text_style_ranges: &'a [html_engine_model::TextPaintRange],
    pub(super) resource_generation: crate::GlyphResourceGeneration,
    pub(super) fragmentation_suppression_depth: usize,
    pub reader: LayoutReader<'a>,
    pub text: InlineReader<'a>,
    pub geometry: &'out mut BoxGeometry,
    pub fragments: FragmentWriter<'out>,
    pub(crate) placement: PlacementState,
    pub(super) timings: Option<RefCell<LayoutTimings>>,
}

impl<'a, 'out> LayoutEngine<'a, 'out> {
    pub(super) fn new(
        read: LayoutReadContext<'a>,
        outputs: LayoutOutputs<'out, '_>,
        timings: Option<RefCell<LayoutTimings>>,
        fragmentation_suppression_depth: usize,
    ) -> Self {
        let LayoutOutputs {
            geometry,
            state,
            placement,
            scratch,
        } = outputs;
        scratch
            .inline_plans
            .begin_generation(read.inline_plan_generation);
        Self {
            config: read.config,
            floats: std::mem::take(&mut scratch.floats),
            margins: std::mem::take(&mut scratch.margins),
            absolute_positioning: std::mem::take(&mut scratch.absolute_positioning),
            flex_grid: std::mem::take(&mut scratch.flex_grid),
            measurement: std::mem::take(&mut scratch.measurement),
            finalization: std::mem::take(&mut scratch.finalization),
            paragraphs: std::mem::take(&mut scratch.paragraphs),
            paragraph_layouts: std::mem::take(&mut scratch.paragraph_layouts),
            text_style_ranges: read.text_style_ranges,
            resource_generation: read.resource_generation,
            reader: read.reader,
            text: read.text,
            geometry,
            fragments: FragmentWriter::new(
                state,
                std::mem::take(&mut scratch.block_decoration_owners),
            ),
            placement: std::mem::take(placement),
            inline_plans: std::mem::take(&mut scratch.inline_plans),
            inline_plan_generation: read.inline_plan_generation,
            fragmentation_suppression_depth,
            timings,
        }
    }

    pub(super) fn recycle_into(
        &mut self,
        placement: &mut PlacementState,
        scratch: &mut LayoutScratch,
    ) {
        scratch.block_decoration_owners = self.fragments.take_decoration_owners();
        *placement = std::mem::take(&mut self.placement);
        scratch.floats = std::mem::take(&mut self.floats);
        scratch.margins = std::mem::take(&mut self.margins);
        scratch.absolute_positioning = std::mem::take(&mut self.absolute_positioning);
        scratch.flex_grid = std::mem::take(&mut self.flex_grid);
        scratch.measurement = std::mem::take(&mut self.measurement);
        scratch.finalization = std::mem::take(&mut self.finalization);
        scratch.paragraphs = std::mem::take(&mut self.paragraphs);
        scratch.paragraph_layouts = std::mem::take(&mut self.paragraph_layouts);
        self.inline_plans.trim();
        scratch.inline_plans = std::mem::take(&mut self.inline_plans);
    }

    pub fn retain_measurement_plans(&mut self, measurement: ParallelBoxMeasurement) {
        self.inline_plans.merge(measurement.inline_plans);
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

    pub fn start_timing(&self) -> LayoutTimingStart {
        LayoutTimingStart(self.timings.as_ref().map(|_| Instant::now()))
    }

    /// Runs layout whose intrinsic geometry must not depend on page boundaries.
    ///
    /// Table cells are measured as continuous content. The paginator later
    /// decides which complete rows fit in each fragment; allowing ordinary
    /// block fragmentation here would insert page-sized gaps inside cells and
    /// make row heights change when the viewport is resized.
    pub fn without_fragmentation<R>(&mut self, layout: impl FnOnce(&mut Self) -> R) -> R {
        self.fragmentation_suppression_depth += 1;
        let result = layout(self);
        self.fragmentation_suppression_depth =
            self.fragmentation_suppression_depth.saturating_sub(1);
        result
    }

    pub fn fragment_height(&self) -> Option<f64> {
        (self.fragmentation_suppression_depth == 0 && !self.config.book_optimized_text())
            .then(|| self.config.viewport_height())
            .flatten()
            .filter(|height| *height > 0.0)
    }

    pub fn record_timing(&self, f: impl FnOnce(&mut LayoutTimings)) {
        if let Some(timings) = &self.timings {
            f(&mut timings.borrow_mut());
        }
    }

    pub(super) fn clear_layout_output(&mut self) {
        let measurement_output = self.geometry.uses_lazy_reset();
        self.geometry.reset(self.reader.box_count());
        self.fragments.reset();
        if measurement_output {
            self.placement.reset_for_measurement();
        } else {
            self.placement.reset(self.reader.box_count());
        }
    }
}

/// Immutable inputs shared by normal layout and table-cell measurement workers.
#[derive(Clone, Copy)]
pub struct LayoutReadContext<'a> {
    config: LayoutConfig,
    reader: LayoutReader<'a>,
    text: InlineReader<'a>,
    inline_plan_generation: InlinePlanGeneration,
    text_style_ranges: &'a [html_engine_model::TextPaintRange],
    resource_generation: crate::GlyphResourceGeneration,
}

pub struct MeasurementWorker {
    geometry: BoxGeometry,
    state: LayoutState,
    placement: PlacementState,
    pub(super) scratch: LayoutScratch,
}

impl Default for MeasurementWorker {
    fn default() -> Self {
        Self {
            geometry: BoxGeometry::lazy(),
            state: LayoutState::default(),
            placement: PlacementState::default(),
            scratch: LayoutScratch::default(),
        }
    }
}

impl MeasurementWorker {
    pub(super) fn outputs(&mut self) -> LayoutOutputs<'_, '_> {
        LayoutOutputs {
            geometry: &mut self.geometry,
            state: &mut self.state,
            placement: &mut self.placement,
            scratch: &mut self.scratch,
        }
    }
}

pub struct ParallelBoxMeasurement {
    pub layout: super::BoxLayoutResult<()>,
    pub(super) inline_plans: InlinePlanCache,
}

impl<'a> LayoutReadContext<'a> {
    pub(super) fn new(
        inputs: LayoutInputs<'a>,
        constraints: crate::LayoutConstraints,
        box_styles: &'a BoxStyleSnapshot<'a>,
    ) -> Self {
        let reader = LayoutReader::new(
            inputs.document,
            inputs.styles,
            inputs.topology,
            inputs.inline_content,
            inputs.font_metrics,
            inputs.image_metrics,
            box_styles,
        );
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
            ),
            inline_plan_generation: inputs.inline_plan_generation,
            text_style_ranges: inputs.text_style_ranges,
            resource_generation: inputs.resource_generation,
        }
    }
}

impl LayoutReadContext<'_> {
    pub fn measure_table_cell(
        &self,
        worker: &mut MeasurementWorker,
        point: Point,
        request: crate::layout::BoxLayoutRequest,
    ) -> ParallelBoxMeasurement {
        #[cfg(any(test, feature = "testing"))]
        super::parallel::record_worker();
        let layout =
            super::preparation::execute_layout(*self, worker.outputs(), &mut None, 1, |engine| {
                engine.geometry.set_point(request.box_idx, point);
                engine.layout_box(request)
            });
        ParallelBoxMeasurement {
            layout: layout.without_output(),
            inline_plans: std::mem::take(&mut worker.scratch.inline_plans),
        }
    }
}
