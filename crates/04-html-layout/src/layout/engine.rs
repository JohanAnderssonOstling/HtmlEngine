use crate::layout_model::{
    BoxGeometry, GlyphId, GlyphMetrics, InlineContent, LayoutState, LayoutTree,
};
use html_dom::Document;
use html_style_model::ComputedStyles;
use kurbo::Point;
use std::cell::RefCell;
use std::time::Duration;
use web_time::Instant;

use super::fragment_writer::FragmentWriter;
use super::geometry_writer::GeometryWriter;
use super::inline_reader::InlineReader;
use super::measurement::MeasurementScratch;
use super::placement::PlacementState;
use super::read_context::LayoutReader;
use super::{
    absolute_positioning::AbsolutePositioningState,
    block::{FloatState, MarginAnalysis},
    config::LayoutConfig,
    finalization::FinalizationScratch,
};

pub(crate) fn layout_with_timings(
    inputs: LayoutInputs<'_>,
    outputs: LayoutOutputs<'_, '_>,
    constraints: crate::LayoutConstraints,
    timings: Option<&mut LayoutTimings>,
) {
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
    let reader = LayoutReader::new(
        inputs.document,
        inputs.styles,
        inputs.topology,
        inputs.inline_content,
        inputs.glyph_metrics,
        inputs.font_metrics,
        inputs.image_metrics,
    );
    let track_overflow_clips = reader.tracks_overflow_clips();
    let text = InlineReader::new(
        inputs.inline_content,
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
        inline_plans: inputs.inline_plans,
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
    if let (Some(destination), Some(collected)) = (timings, context.timings.take()) {
        *destination = collected.into_inner();
    }
    context.recycle_into(placement, scratch);
}

/// Allocation-owning state reused by successive layout passes. It is separate
/// from published geometry so render consumers cannot observe or depend on it.
#[derive(Default)]
pub(crate) struct LayoutScratch {
    floats: FloatState,
    margins: MarginAnalysis,
    absolute_positioning: AbsolutePositioningState,
    flex_grid: crate::flex_grid::FlexGridState,
    measurement: MeasurementScratch,
    finalization: FinalizationScratch,
    line_owners: Vec<u32>,
    block_decoration_owners: Vec<u32>,
}

impl Clone for LayoutScratch {
    fn clone(&self) -> Self {
        // Scratch has no semantic state. Cloning a laid-out document should not
        // duplicate potentially large temporary allocations.
        Self::default()
    }
}

impl LayoutScratch {
    #[cfg(test)]
    pub(crate) fn allocation_capacities(
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

pub(crate) struct LayoutInputs<'a> {
    pub(crate) document: &'a Document,
    pub(crate) styles: &'a ComputedStyles,
    pub(crate) topology: &'a LayoutTree,
    pub(crate) inline_content: &'a InlineContent,
    pub(crate) glyph_metrics: &'a GlyphMetrics,
    pub(crate) font_metrics: &'a crate::shaping::ShapedFontMetrics,
    pub(crate) text_geometry: Option<&'a crate::shaping::ShapedTextGeometry>,
    pub(crate) ellipsis_glyphs: &'a rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
    pub(crate) hyphen_glyphs: &'a rustc_data_structures::fx::FxHashMap<u32, GlyphId>,
    pub(crate) inline_plans: &'a crate::layout::PreparedInlinePlans,
    pub(crate) image_metrics: &'a crate::ImageMetrics,
}

pub(crate) struct LayoutOutputs<'out, 'scratch> {
    pub(crate) geometry: &'out mut BoxGeometry,
    pub(crate) state: &'out mut LayoutState,
    pub(crate) placement: &'scratch mut PlacementState,
    pub(crate) scratch: &'scratch mut LayoutScratch,
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
pub(crate) struct LayoutTimingStart(Option<Instant>);

impl LayoutTimingStart {
    pub(crate) fn elapsed(self) -> Duration {
        self.0.map_or(Duration::ZERO, |started| started.elapsed())
    }
}

pub(crate) struct LayoutEngine<'a, 'out> {
    pub(crate) config: LayoutConfig,
    pub(super) floats: FloatState,
    pub(super) margins: MarginAnalysis,
    pub(super) absolute_positioning: AbsolutePositioningState,
    pub(super) inline_plans: &'a crate::layout::PreparedInlinePlans,
    pub(crate) flex_grid: crate::flex_grid::FlexGridState,
    pub(super) measurement: MeasurementScratch,
    pub(super) finalization: FinalizationScratch,
    pub(super) track_overflow_clips: bool,
    fragmentation_suppression_depth: usize,
    pub(crate) reader: LayoutReader<'a>,
    pub(crate) text: InlineReader<'a>,
    pub(crate) geometry: GeometryWriter<'out>,
    pub(crate) fragments: FragmentWriter<'out>,
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
        Self {
            config: read.config,
            floats: std::mem::take(&mut scratch.floats),
            margins: std::mem::take(&mut scratch.margins),
            absolute_positioning: std::mem::take(&mut scratch.absolute_positioning),
            flex_grid: std::mem::take(&mut scratch.flex_grid),
            measurement: std::mem::take(&mut scratch.measurement),
            finalization: std::mem::take(&mut scratch.finalization),
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
            inline_plans: read.inline_plans,
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
    }

    pub(crate) fn parallel_measurement_context(&self) -> LayoutReadContext<'a> {
        LayoutReadContext {
            config: self.config,
            reader: self.reader.clone(),
            text: self.text,
            inline_plans: self.inline_plans,
            track_overflow_clips: self.track_overflow_clips,
        }
    }

    pub(crate) fn start_timing(&self) -> LayoutTimingStart {
        LayoutTimingStart(self.timings.as_ref().map(|_| Instant::now()))
    }

    pub(crate) fn select_placement_group(&mut self, group: super::placement::PlacementId) {
        self.placement.select(group);
    }

    pub(crate) fn push_line_owner(&mut self, owner: u32) {
        self.fragments.push_line_owner(&mut self.placement, owner);
    }

    pub(crate) fn push_block_decoration(
        &mut self,
        owner: u32,
        decoration: crate::layout_model::DecorationFragment,
    ) {
        self.fragments
            .push_block_decoration(&mut self.placement, owner, decoration);
    }

    pub(crate) fn natural_content_height(&self, box_idx: usize) -> f64 {
        self.margins.natural_content_height(box_idx)
    }

    /// Runs layout whose intrinsic geometry must not depend on page boundaries.
    ///
    /// Table cells are measured as continuous content. The paginator later
    /// decides which complete rows fit in each fragment; allowing ordinary
    /// block fragmentation here would insert page-sized gaps inside cells and
    /// make row heights change when the viewport is resized.
    pub(crate) fn without_fragmentation<R>(&mut self, layout: impl FnOnce(&mut Self) -> R) -> R {
        self.fragmentation_suppression_depth += 1;
        let result = layout(self);
        self.fragmentation_suppression_depth =
            self.fragmentation_suppression_depth.saturating_sub(1);
        result
    }

    pub(crate) fn fragment_height(&self) -> Option<f64> {
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

    pub(crate) fn record_timing(&self, f: impl FnOnce(&mut LayoutTimings)) {
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
#[derive(Clone)]
pub(crate) struct LayoutReadContext<'a> {
    config: LayoutConfig,
    reader: LayoutReader<'a>,
    text: InlineReader<'a>,
    inline_plans: &'a crate::layout::PreparedInlinePlans,
    track_overflow_clips: bool,
}

pub(crate) struct ParallelMeasurementWorker {
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

#[derive(Clone, Copy)]
pub(crate) struct ParallelBoxMeasurement {
    pub(crate) size: kurbo::Size,
    pub(crate) first_baseline: Option<f64>,
}

impl LayoutReadContext<'_> {
    pub(crate) fn measure_table_cell(
        &self,
        worker: &mut ParallelMeasurementWorker,
        point: Point,
        request: crate::layout::BoxLayoutRequest,
    ) -> ParallelBoxMeasurement {
        #[cfg(test)]
        super::parallel::record_worker();
        let mut engine = LayoutEngine::new(
            self.clone(),
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
        let result = ParallelBoxMeasurement {
            size: cell_layout.size,
            first_baseline,
        };

        engine.recycle_into(&mut worker.placement, &mut worker.scratch);
        result
    }
}
