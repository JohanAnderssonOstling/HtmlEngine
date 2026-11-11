//! Numbered stages show the document pass and each box pipeline.
//! Stage 00 coordinates later stages; child layout and measurement can recurse.
//! Formatting contexts own their local pipelines; shared queries and storage stay in dedicated modules.

#[path = "00-layout.rs"]
mod s00_layout;
#[path = "01-prepare.rs"]
mod s01_prepare;
#[path = "02-root_layout.rs"]
mod s02_root_layout;
#[path = "03-finalize.rs"]
mod s03_finalize;
#[path = "04-paint.rs"]
mod s04_paint;
#[path = "box/00-layout.rs"]
mod box_s00_layout;
#[path = "box/01-sizing.rs"]
mod box_s01_sizing;
#[path = "box/02-contents.rs"]
mod box_s02_contents;
#[path = "box/03-finish.rs"]
mod box_s03_finish;

pub use html_engine_model::paint_program;
mod absolute_positioning;
mod block;
mod border_geometry;

mod box_constraints;
mod box_model;
mod config;
pub mod decorations;
pub mod engine;
mod fragment_writer;
mod inline;
mod inline_reader;
mod intrinsic_sizing;
mod measurement;
mod placement;
mod read_context;
mod replaced;

pub use border_geometry::{PhysicalBorderEdge, border_pattern, physical_borders};

pub use box_constraints::{
    BoxLayoutRequest, constrain_content_width, constrain_size, resolve_definite_content_size,
    resolve_definite_outer_inline_size, resolve_definite_size_value, resolve_vertical_size,
};
pub use box_s00_layout::BoxLayoutResult;
pub(crate) use box_model::ContentLayoutResult;
pub use box_model::{ResolvedBoxModel, UsedBorderInsets};
pub use box_s03_finish::{translate_laid_out_content, translate_laid_out_output};
pub use box_s01_sizing::{ResolvedBoxSizing, ResolvedTableBoxSizing};
pub use decorations::{
    record_block_background, record_block_border_and_outline, record_block_decorations,
};
pub(super) use engine::LayoutEngine;
pub use engine::LayoutInputs;
pub use fragment_writer::{FragmentWriter, OutputRanges};
pub(crate) use s03_finalize::ordering::collapsed_border_paint_key;
pub use inline::InlinePlanGeneration;
pub use intrinsic_sizing::{
    box_content_intrinsic_widths, box_intrinsic_widths, contains_full_width_percentage_table,
    percentage_height_image_width,
};
pub use measurement::{measure_box_isolated, measure_resolved_box_isolated, with_isolated_measurement};
pub use placement::{PlacementId, PlacementState};
pub use read_context::LayoutReader;
pub use replaced::{
    ReplacedMainAxis, ReplacedSizeInput, clamp_replaced_definite_size_by_intrinsic_constraints,
    preferred_aspect_ratio, resolve_replaced_flex_auto_min_main_size,
    resolve_replaced_intrinsic_constraint,
};

pub use s03_finalize::text_paint::prepare_text_paint;
