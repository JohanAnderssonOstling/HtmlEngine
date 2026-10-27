pub use html_engine_model::paint_program;
mod absolute_positioning;
mod block;
mod border_geometry;

mod box_constraints;
mod box_layout;
mod box_model;
mod box_positioning;
mod box_sizing;
mod config;
pub mod decorations;
pub mod engine;
mod finalization;
mod formatting_context;
mod fragment_writer;
mod geometry_writer;
mod inline;
mod inline_reader;
mod intrinsic_sizing;
mod measurement;
mod parallel;
mod placement;
mod read_context;
mod replaced;
pub(crate) mod text_placement;

pub(super) use block::{FloatBand, FloatContext, FloatSide};
pub use border_geometry::{PhysicalBorderEdge, border_pattern, physical_borders};

pub use box_constraints::{
    BoxLayoutRequest, constrain_content_width, resolve_definite_content_size,
    resolve_definite_outer_inline_size, resolve_definite_size_value, resolve_vertical_size,
};
pub use box_layout::BoxLayoutResult;
pub use box_model::{ResolvedBoxModel, UsedBorderInsets};
pub use box_positioning::{translate_laid_out_content, translate_laid_out_output};
pub use box_sizing::{ResolvedBoxSizing, ResolvedTableBoxSizing};
pub use decorations::{
    emit_block_background, emit_block_border_and_outline, emit_block_decorations,
};
pub(super) use engine::LayoutEngine;
pub use engine::LayoutInputs;
pub use engine::{ParallelBoxMeasurement, ParallelMeasurementWorker};
pub use fragment_writer::{FragmentWriter, OutputRanges};
pub use inline::InlinePlanGeneration;
pub use intrinsic_sizing::{
    box_content_intrinsic_widths, box_intrinsic_widths, contains_full_width_percentage_table,
    percentage_height_image_width,
};
pub use measurement::{
    measure_box_and_baselines_isolated, measure_box_isolated,
    measure_box_width_and_baselines_isolated, measure_intrinsic_block_size_isolated,
    measure_resolved_box_isolated, with_isolated_measurement,
};
pub use parallel::install as install_parallel;
pub use placement::PlacementState;
pub use read_context::LayoutReader;
pub use replaced::{
    ReplacedFlexAutoMinInput, ReplacedMainAxis,
    clamp_replaced_definite_size_by_intrinsic_constraints, measure_replaced_content,
    preferred_aspect_ratio, resolve_replaced_flex_auto_min_main_size,
    resolve_replaced_intrinsic_constraint,
};

pub use finalization::text_paint::prepare_text_paint;

#[cfg(feature = "testing")]
pub use parallel::{observed_worker_count, reset_observed_workers};
