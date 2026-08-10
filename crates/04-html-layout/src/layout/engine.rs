use crate::layout_model::{BoxGeometry, GlyphId, GlyphMetrics, InlineContent, LayoutState, LayoutTree};
use html_dom::Document;
use html_style_model::ComputedStyles;
use kurbo::Point;
use std::cell::RefCell;
use std::time::{Duration, Instant};

use super::fragment_writer::FragmentWriter;
use super::geometry_writer::GeometryWriter;
use super::inline_reader::InlineReader;
use super::measurement::MeasurementScratch;
use super::read_context::LayoutReader;
use super::{
    absolute_positioning::AbsolutePositioningState,
    block::{FloatState, MarginAnalysis},
    config::LayoutConfig,
    finalization::FinalizationScratch,
};

pub(crate) fn layout_with_timings(inputs: LayoutInputs<'_>, outputs: LayoutOutputs<'_>, constraints: crate::LayoutConstraints, timings: Option<&mut LayoutTimings>) {
    // Keep telemetry owned by the layout session. `record_timing` is called
    // through shared references throughout recursive layout, so interior
    // mutability provides the narrow safe capability that the old raw pointer
    // was approximating.
    let collected_timings = timings.as_deref().cloned().map(RefCell::new);
    let LayoutOutputs { geometry, state, inline_token_cache, scratch } = outputs;
    let reader = LayoutReader::new(inputs.document, inputs.styles, inputs.topology, inputs.inline_content, inputs.glyph_metrics, inputs.font_metrics, inputs.image_metrics);
    let track_overflow_clips = (0..reader.box_count()).any(|idx| {
        let style = reader.style(idx);
        style.overflow_x().clips() || style.overflow_y().clips()
    });
    let text = InlineReader::new(inputs.inline_content, inputs.glyph_metrics, inputs.text_geometry, inputs.ellipsis_glyphs, inputs.hyphen_glyphs);
    let mut context = LayoutEngine {
        config: LayoutConfig::new(constraints),
        floats: std::mem::take(&mut scratch.floats),
        margins: std::mem::take(&mut scratch.margins),
        absolute_positioning: std::mem::take(&mut scratch.absolute_positioning),
        flex_grid: std::mem::take(&mut scratch.flex_grid),
        measurement: std::mem::take(&mut scratch.measurement),
        finalization: std::mem::take(&mut scratch.finalization),
        track_overflow_clips,
        reader,
        text,
        geometry: GeometryWriter::new(geometry),
        fragments: FragmentWriter::new(state, std::mem::take(&mut scratch.line_owners), std::mem::take(&mut scratch.block_decoration_owners)),
        inline_token_cache,
        fragmentation_suppression_depth: 0,
        timings: collected_timings,
    };
    context.run();
    if let (Some(destination), Some(collected)) = (timings, context.timings.take()) {
        *destination = collected.into_inner();
    }
    context.fragments.recycle_owner_storage(&mut scratch.line_owners, &mut scratch.block_decoration_owners);
    scratch.floats = std::mem::take(&mut context.floats);
    scratch.margins = std::mem::take(&mut context.margins);
    scratch.absolute_positioning = std::mem::take(&mut context.absolute_positioning);
    scratch.flex_grid = std::mem::take(&mut context.flex_grid);
    scratch.measurement = std::mem::take(&mut context.measurement);
    scratch.finalization = std::mem::take(&mut context.finalization);
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
    pub(crate) fn allocation_capacities(&self) -> ((usize, usize), (usize, usize, usize, usize, usize, usize)) {
        ((self.line_owners.capacity(), self.block_decoration_owners.capacity()), self.finalization.allocation_capacities())
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
    pub(crate) image_metrics: &'a crate::ImageMetrics,
}

pub(crate) struct LayoutOutputs<'out> {
    pub(crate) geometry: &'out mut BoxGeometry,
    pub(crate) state: &'out mut LayoutState,
    pub(crate) inline_token_cache: &'out mut crate::layout::InlineTokenCache,
    pub(crate) scratch: &'out mut LayoutScratch,
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

pub(crate) struct LayoutEngine<'a, 'out> {
    pub(crate) config: LayoutConfig,
    pub(super) floats: FloatState,
    pub(super) margins: MarginAnalysis,
    pub(super) absolute_positioning: AbsolutePositioningState,
    pub(super) inline_token_cache: &'out mut crate::layout::InlineTokenCache,
    pub(crate) flex_grid: crate::flex_grid::FlexGridState,
    pub(super) measurement: MeasurementScratch,
    pub(super) finalization: FinalizationScratch,
    pub(super) track_overflow_clips: bool,
    fragmentation_suppression_depth: usize,
    pub(crate) reader: LayoutReader<'a>,
    pub(crate) text: InlineReader<'a>,
    pub(crate) geometry: GeometryWriter<'out>,
    pub(crate) fragments: FragmentWriter<'out>,
    timings: Option<RefCell<LayoutTimings>>,
}

impl<'a, 'out> LayoutEngine<'a, 'out> {
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
        self.fragmentation_suppression_depth = self.fragmentation_suppression_depth.saturating_sub(1);
        result
    }

    pub(crate) fn fragment_height(&self) -> Option<f64> {
        (self.fragmentation_suppression_depth == 0).then(|| self.config.viewport_height()).flatten().filter(|height| *height > 0.0)
    }

    fn run(&mut self) {
        let start = Instant::now();
        self.clear_layout_output();
        self.record_timing(|t| t.clear_layout_output += start.elapsed());
        let start = Instant::now();
        self.floats.reset();
        self.margins.reset();
        self.absolute_positioning.clear();
        self.flex_grid.clear();
        self.record_timing(|t| t.layout_tree_traversal += start.elapsed());

        if let Some(root_box) = self.reader.root_box() {
            let start = Instant::now();
            // The root element establishes the initial block formatting
            // context, so its margins never collapse with its descendants.
            // Unlike ordinary boxes, it has no parent flow to consume those
            // margins; place its border box explicitly inside the canvas.
            let root_style = self.reader.style(root_box);
            let root_origin = Point::new(root_style.margin_left().resolve(self.config.viewport_width()), root_style.margin_top().resolve(self.config.viewport_width()));
            if root_style.position() == html_style_model::PositionMode::Absolute {
                super::absolute_positioning::defer(self, root_box, Point::ZERO);
            } else {
                self.geometry.set_point(root_box, root_origin);
                let _ = self.layout_box(super::BoxLayoutRequest::normal(root_box, self.config.viewport_width(), self.config.viewport_height()));
            }
            super::absolute_positioning::layout_initial_containing_block(self);
            let elapsed = start.elapsed();
            self.record_timing(|t| t.layout_tree_traversal += elapsed);
            self.record_timing(|t| t.root_box_layout += elapsed);
        }

        let start = Instant::now();
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
        self.geometry.reset(self.reader.box_count());
        self.fragments.reset();
    }
}
