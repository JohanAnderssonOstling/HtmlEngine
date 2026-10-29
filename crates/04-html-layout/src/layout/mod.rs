//! Numbered entry stages coordinate the document pass and each box.
//! Formatting contexts own their local pipelines; shared queries and storage stay unnumbered.

pub use html_engine_model::paint_program;
mod absolute_positioning;
mod block;
mod border_geometry;

mod box_constraints;
#[path = "box/02-contents.rs"]
mod box_contents;
#[path = "box/00-layout.rs"]
mod box_layout;
mod box_model;
#[path = "box/03-finish.rs"]
mod box_positioning;
#[path = "box/01-sizing.rs"]
mod box_sizing;
mod config;
pub mod decorations;
pub mod engine;
#[path = "00-layout.rs"]
mod execution;
#[path = "03-finalize.rs"]
mod finalization;
mod formatting_context;
mod fragment_writer;
mod inline;
mod inline_reader;
mod intrinsic_sizing;
mod measurement;
#[path = "04-paint.rs"]
mod paint;
mod parallel;
mod placement;
#[path = "01-prepare.rs"]
mod preparation;
mod read_context;
mod replaced;
#[path = "02-root_layout.rs"]
mod root_layout;

pub(super) use block::{FloatBand, FloatSide};
pub use border_geometry::{PhysicalBorderEdge, border_pattern, physical_borders};

pub use box_constraints::{
    BoxLayoutRequest, constrain_content_width, constrain_size, resolve_definite_content_size,
    resolve_definite_outer_inline_size, resolve_definite_size_value, resolve_vertical_size,
};
pub use box_layout::BoxLayoutResult;
pub(crate) use box_model::ContentLayoutResult;
pub use box_model::{ResolvedBoxModel, UsedBorderInsets};
pub use box_positioning::{translate_laid_out_content, translate_laid_out_output};
pub use box_sizing::{ResolvedBoxSizing, ResolvedTableBoxSizing};
pub use decorations::{
    record_block_background, record_block_border_and_outline, record_block_decorations,
};
pub(super) use engine::LayoutEngine;
pub use engine::LayoutInputs;
pub use engine::{MeasurementWorker, ParallelBoxMeasurement};
pub use fragment_writer::{FragmentWriter, OutputRanges};
pub use inline::InlinePlanGeneration;
pub use intrinsic_sizing::{
    box_content_intrinsic_widths, box_intrinsic_widths, contains_full_width_percentage_table,
    percentage_height_image_width,
};
pub use measurement::{
    measure_box_isolated, measure_resolved_box_isolated, with_isolated_measurement,
};
pub use parallel::install as install_parallel;
pub use placement::{PlacementId, PlacementState};
pub use read_context::LayoutReader;
pub use replaced::{
    ReplacedMainAxis, ReplacedSizeInput, clamp_replaced_definite_size_by_intrinsic_constraints,
    preferred_aspect_ratio, resolve_replaced_flex_auto_min_main_size,
    resolve_replaced_intrinsic_constraint,
};

pub use finalization::text_paint::prepare_text_paint;

#[cfg(feature = "testing")]
pub use parallel::{observed_worker_count, reset_observed_workers};
