use crate::layout_model::{
    BoxGeometry, GlyphId, GlyphMetrics, GlyphSlice, InlineContent, LayoutState, LayoutTree,
    LineOutput, ShapedFontMetrics, TextGeometryView,
};
use html_dom::{Document, MemoryUsageReport};
use html_style_model::ComputedStyles;
use kurbo::Point;
use std::cell::RefCell;
use std::time::Duration;
use web_time::Instant;

use super::fragment_writer::FragmentWriter;
use super::geometry_writer::GeometryWriter;
use super::inline_reader::InlineReader;
use super::inline::{InlinePlanCache, InlinePlanGeneration};
use super::measurement::MeasurementScratch;
use super::placement::PlacementState;
use super::read_context::{BoxStyleSnapshot, LayoutReader};
use super::{
    absolute_positioning::AbsolutePositioningState,
    block::{FloatState, MarginAnalysis},
    config::LayoutConfig,
    finalization::FinalizationScratch,
};

pub fn layout_with_timings(
    inputs: LayoutInputs<'_>,
    outputs: LayoutOutputs<'_, '_>,
    constraints: crate::LayoutConstraints,
    timings: Option<&mut LayoutTimings>,
) {
    profile_scope!("layout.engine");
    // Keep telemetry owned by the layout session. `record_timing` is called
    // through shared references throughout recursive layout, so interior
    // mutability provides the narrow safe capability that the old raw pointer
    // was approximating.
    let collected_timings = timings.as_deref().cloned().map(RefCell::new);
    let LayoutOutputs {
        geometry,
        state,
        placement,
        scratch,
    } = outputs;
    let box_styles = BoxStyleSnapshot::new(inputs.document, inputs.inline_content, inputs.styles, inputs.topology, inputs.font_metrics);
    let reader = LayoutReader::new(
        inputs.document,
        inputs.styles,
        inputs.topology,
        inputs.inline_content,
        inputs.font_metrics,
        inputs.image_metrics,
        &box_styles,
    );
    let track_overflow_clips = reader.tracks_overflow_clips();
    let text = InlineReader::new(
        inputs.inline_content,
        inputs.glyphs,
        inputs.glyph_metrics,
        inputs.text_geometry,
        inputs.ellipsis_glyphs,
        inputs.hyphen_glyphs,
    );
    let read = LayoutReadContext {
        config: LayoutConfig::new(constraints),
        track_overflow_clips,
        reader,
        text,
        inline_plan_generation: inputs.inline_plan_generation,
        text_style_ranges: inputs.text_style_ranges,
        resource_generation: inputs.resource_generation,
    };
    let mut context = LayoutEngine::new(
        read,
        LayoutOutputs {
            geometry,
            state,
            placement: &mut *placement,
            scratch: &mut *scratch,
        },
        collected_timings,
        0,
    );
    context.run();
    context.reader = context.reader.with_paint(inputs.paint);
    let paint_started = context.start_timing();
    context.repaint();
    context.record_timing(|timings| timings.finalize_layout += paint_started.elapsed());
    if let (Some(destination), Some(collected)) = (timings, context.timings.take()) {
        *destination = collected.into_inner();
    }
    context.recycle_into(placement, scratch);
}

/// Allocation-owning state reused by successive layout passes. It is separate
/// from published geometry so render consumers cannot observe or depend on it.
#[derive(Default)]
pub struct LayoutScratch {
    floats: FloatState,
    margins: MarginAnalysis,
    absolute_positioning: AbsolutePositioningState,
    flex_grid: crate::flex_grid::FlexGridState,
    measurement: MeasurementScratch,
    finalization: FinalizationScratch,
    paragraphs: Vec<super::inline::ParagraphScratch>,
    paragraph_layouts: super::inline::ParagraphLayouts,
    publication_epoch: u64,
    previous_config: Option<LayoutConfig>,
    inline_plans: InlinePlanCache,
    line_owners: Vec<u32>,
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
    pub fn paragraph_published_hits(&self, box_idx: usize) -> usize { self.paragraph_layouts.published_hits(box_idx) }
    #[cfg(any(test, feature = "testing"))]
    pub fn paragraph_capacities(&self) -> Vec<(usize, (usize, usize, usize))> {
        self.paragraphs.iter().map(super::inline::ParagraphScratch::capacities).collect()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn allocation_capacities(
        &self,
    ) -> ((usize, usize), (usize, usize, usize, usize, usize, usize)) {
        (
            (
                self.line_owners.capacity(),
                self.block_decoration_owners.capacity(),
            ),
            self.finalization.allocation_capacities(),
        )
    }
}

pub struct LayoutInputs<'a> {
    pub document: &'a Document,
    pub styles: &'a ComputedStyles,
    pub topology: &'a LayoutTree,
    pub inline_content: &'a InlineContent,
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
    pub sort_lines_and_remap_images: Duration,
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
    /// Published line records moved from the previous output without measurement.
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
    pub(super) measurement: MeasurementScratch,
    pub(super) finalization: FinalizationScratch,
    pub(super) paragraphs: Vec<super::inline::ParagraphScratch>,
    pub(super) paragraph_layouts: super::inline::ParagraphLayouts,
    pub(super) publication_epoch: u64,
    // Only lives during the pass. No old publication is retained in scratch.
    pub(super) previous_lines: Option<LineOutput>,
    pub(super) text_style_ranges: &'a [html_engine_model::TextPaintRange],
    pub(super) resource_generation: crate::GlyphResourceGeneration,
    pub(super) track_overflow_clips: bool,
    fragmentation_suppression_depth: usize,
    pub reader: LayoutReader<'a>,
    pub text: InlineReader<'a>,
    pub geometry: GeometryWriter<'out>,
    pub fragments: FragmentWriter<'out>,
    pub(super) placement: PlacementState,
    timings: Option<RefCell<LayoutTimings>>,
}

impl<'a, 'out> LayoutEngine<'a, 'out> {
    fn new(
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
        scratch.inline_plans.begin_generation(read.inline_plan_generation);
        let previous_lines = (scratch.previous_config == Some(read.config)
            && !geometry.uses_lazy_reset()
            && scratch.paragraph_layouts.has_publication()
            && !state.line_output.lines.is_empty())
            .then(|| std::mem::take(&mut state.line_output));
        Self {
            config: read.config,
            previous_lines,
            publication_epoch: scratch.publication_epoch.wrapping_add(1),
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
            track_overflow_clips: read.track_overflow_clips,
            reader: read.reader,
            text: read.text,
            geometry: GeometryWriter::new(geometry),
            fragments: FragmentWriter::new(
                state,
                std::mem::take(&mut scratch.line_owners),
                std::mem::take(&mut scratch.block_decoration_owners),
            ),
            placement: std::mem::take(placement),
            inline_plans: std::mem::take(&mut scratch.inline_plans),
            inline_plan_generation: read.inline_plan_generation,
            fragmentation_suppression_depth,
            timings,
        }
    }

    fn recycle_into(&mut self, placement: &mut PlacementState, scratch: &mut LayoutScratch) {
        self.fragments.recycle_owner_storage(
            &mut scratch.line_owners,
            &mut scratch.block_decoration_owners,
        );
        *placement = std::mem::take(&mut self.placement);
        scratch.floats = std::mem::take(&mut self.floats);
        scratch.margins = std::mem::take(&mut self.margins);
        scratch.absolute_positioning = std::mem::take(&mut self.absolute_positioning);
        scratch.flex_grid = std::mem::take(&mut self.flex_grid);
        scratch.measurement = std::mem::take(&mut self.measurement);
        scratch.finalization = std::mem::take(&mut self.finalization);
        scratch.paragraphs = std::mem::take(&mut self.paragraphs);
        scratch.paragraph_layouts = std::mem::take(&mut self.paragraph_layouts);
        scratch.publication_epoch = self.publication_epoch;
        scratch.previous_config = Some(self.config);
        self.inline_plans.trim();
        scratch.inline_plans = std::mem::take(&mut self.inline_plans);
    }

    /// Return buffers only used by finalization after the last borrowed read.
    /// All remaining previous output storage is released at this boundary.
    pub(super) fn release_previous_lines(&mut self) {
        if let Some(mut previous) = self.previous_lines.take() {
            let current = &mut self.fragments.state_mut().line_output;
            previous.lines.clear();
            previous.lines.append(&mut current.lines);
            current.lines = std::mem::take(&mut previous.lines);
            current.line_glyph_offsets.recycle_storage(std::mem::take(&mut previous.line_glyph_offsets));
            current.line_glyph_advances.recycle_storage(std::mem::take(&mut previous.line_glyph_advances));
            current.glyph_line_indices = std::mem::take(&mut previous.glyph_line_indices);
            previous.prepared_text_run_fragments.clear();
            current.prepared_text_run_fragments = std::mem::take(&mut previous.prepared_text_run_fragments);
        }
    }

    pub fn retain_measurement_plans(&mut self, measurement: ParallelBoxMeasurement) {
        self.inline_plans.merge(measurement.inline_plans);
    }

    pub fn parallel_measurement_context(&self) -> LayoutReadContext<'a> {
        LayoutReadContext {
            config: self.config,
            reader: self.reader,
            text: self.text,
            inline_plan_generation: self.inline_plan_generation,
            text_style_ranges: self.text_style_ranges,
            resource_generation: self.resource_generation,
            track_overflow_clips: self.track_overflow_clips,
        }
    }

    pub fn start_timing(&self) -> LayoutTimingStart {
        LayoutTimingStart(self.timings.as_ref().map(|_| Instant::now()))
    }

    pub fn select_placement_group(&mut self, group: super::placement::PlacementId) {
        self.placement.select(group);
    }

    pub fn push_line_owner(&mut self, owner: u32) {
        self.fragments.push_line_owner(&mut self.placement, owner);
    }

    pub fn natural_content_height(&self, box_idx: usize) -> f64 {
        self.margins.natural_content_height(box_idx)
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
        (self.fragmentation_suppression_depth == 0)
            .then(|| self.config.viewport_height())
            .flatten()
            .filter(|height| *height > 0.0)
    }

    fn run(&mut self) {
        let start = self.start_timing();
        self.clear_layout_output();
        self.record_timing(|t| t.clear_layout_output += start.elapsed());
        let start = self.start_timing();
        self.floats.reset();
        self.margins.reset();
        self.absolute_positioning.clear();
        self.flex_grid.clear();
        self.record_timing(|t| t.layout_tree_traversal += start.elapsed());

        if let Some(root_box) = self.reader.root_box() {
            let start = self.start_timing();
            // The root element establishes the initial block formatting
            // context, so its margins never collapse with its descendants.
            // Unlike ordinary boxes, it has no parent flow to consume those
            // margins; place its border box explicitly inside the canvas.
            let root_style = self.reader.style(root_box);
            let root_origin = Point::new(
                root_style
                    .margin_left()
                    .resolve(self.config.viewport_width()),
                root_style
                    .margin_top()
                    .resolve(self.config.viewport_width()),
            );
            if root_style.position() == html_style_model::PositionMode::Absolute {
                super::absolute_positioning::defer(self, root_box, Point::ZERO);
            } else {
                self.geometry.set_point(root_box, root_origin);
                let _ = self.layout_box(super::BoxLayoutRequest::normal(
                    root_box,
                    self.config.viewport_width(),
                    self.config.viewport_height(),
                ));
            }
            super::absolute_positioning::layout_initial_containing_block(self);
            let elapsed = start.elapsed();
            self.record_timing(|t| t.layout_tree_traversal += elapsed);
            self.record_timing(|t| t.root_box_layout += elapsed);
        }

        let start = self.start_timing();
        self.floats.clear();
        self.record_timing(|t| t.layout_tree_traversal += start.elapsed());
        self.finalize(self.track_overflow_clips);
    }

    pub fn record_timing(&self, f: impl FnOnce(&mut LayoutTimings)) {
        if let Some(timings) = &self.timings {
            f(&mut timings.borrow_mut());
        }
    }

    pub(super) fn clear_layout_output(&mut self) {
        let measurement_output = self.geometry.as_ref().uses_lazy_reset();
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
    track_overflow_clips: bool,
}

pub struct ParallelMeasurementWorker {
    geometry: BoxGeometry,
    state: LayoutState,
    placement: PlacementState,
    scratch: LayoutScratch,
}

impl Default for ParallelMeasurementWorker {
    fn default() -> Self {
        Self {
            geometry: BoxGeometry::lazy(),
            state: LayoutState::default(),
            placement: PlacementState::default(),
            scratch: LayoutScratch::default(),
        }
    }
}

pub struct ParallelBoxMeasurement {
    pub size: kurbo::Size,
    pub first_baseline: Option<f64>,
    pub(super) inline_plans: InlinePlanCache,
}

impl LayoutReadContext<'_> {
    pub fn measure_table_cell(
        &self,
        worker: &mut ParallelMeasurementWorker,
        point: Point,
        request: crate::layout::BoxLayoutRequest,
    ) -> ParallelBoxMeasurement {
        #[cfg(any(test, feature = "testing"))]
        super::parallel::record_worker();
        let mut engine = LayoutEngine::new(
            *self,
            LayoutOutputs {
                geometry: &mut worker.geometry,
                state: &mut worker.state,
                placement: &mut worker.placement,
                scratch: &mut worker.scratch,
            },
            None,
            1,
        );
        engine.clear_layout_output();
        engine.floats.reset();
        engine.margins.reset();
        engine.absolute_positioning.clear();
        engine.flex_grid.clear();
        engine.geometry.set_point(request.box_idx, point);
        let cell_layout = engine.layout_box(request);
        let first_baseline = engine.fragments.first_baseline_offset(
            cell_layout.output.lines,
            engine.geometry.point(request.box_idx).y,
        );
        engine.recycle_into(&mut worker.placement, &mut worker.scratch);
        ParallelBoxMeasurement {
            size: cell_layout.size,
            first_baseline,
            inline_plans: std::mem::take(&mut worker.scratch.inline_plans),
        }
    }
}

/// Rebuild paint fragments over retained line and box geometry without
/// running geometry algorithms or modifying their retained placement state.
pub fn repaint(inputs: LayoutInputs<'_>, geometry: &mut BoxGeometry, state: &mut LayoutState, constraints: crate::LayoutConstraints) {
    let mut placement = PlacementState::default();
    let mut scratch = LayoutScratch::default();
    let box_styles = BoxStyleSnapshot::new(inputs.document, inputs.inline_content, inputs.styles, inputs.topology, inputs.font_metrics);
    let reader = LayoutReader::new(inputs.document, inputs.styles, inputs.topology, inputs.inline_content, inputs.font_metrics, inputs.image_metrics, &box_styles).with_paint(inputs.paint);
    let text = InlineReader::new(inputs.inline_content, inputs.glyphs, inputs.glyph_metrics, inputs.text_geometry, inputs.ellipsis_glyphs, inputs.hyphen_glyphs);
    let read = LayoutReadContext { config: LayoutConfig::new(constraints), track_overflow_clips: reader.tracks_overflow_clips(), reader, text, inline_plan_generation: inputs.inline_plan_generation, text_style_ranges: inputs.text_style_ranges, resource_generation: inputs.resource_generation };
    let mut engine = LayoutEngine::new(read, LayoutOutputs { geometry, state, placement: &mut placement, scratch: &mut scratch }, None, 0);
    engine.repaint();
}
