//! Backend-neutral geometry execution over immutable prepared/shaped inputs.
//! Document ownership and stage orchestration live in html-pipeline.
//! The layout module coordinates the document pass. Numbered stages describe
//! formatting-context pipelines; child layout and measurement can recurse.
//! Formatting contexts own their local pipelines; shared queries and storage stay in dedicated modules.

#![cfg_attr(not(test), forbid(unsafe_code))]

pub use html_engine_model::*;
mod paint;

pub use html_engine_model::paint_program;
mod block;
mod boxes;

mod decorations;
mod engine;
mod finalization;
mod flex_grid;
mod inline;
mod layout;
mod measurement;
mod output;
mod table;

pub(crate) use boxes::ResolvedBoxSizing;
pub(crate) use boxes::constraints::{
    constrain_content_width, constrain_size, resolve_definite_content_size,
    resolve_definite_outer_inline_size, resolve_vertical_size,
};
pub(crate) use boxes::model::BoxLayoutResult;
pub(crate) use boxes::model::ContentLayoutResult;
pub(crate) use boxes::model::{ResolvedBoxModel, UsedBorderInsets};
pub(crate) use boxes::replaced::{
    ReplacedMainAxis, ReplacedSizeInput, clamp_replaced_definite_size_by_intrinsic_constraints,
    preferred_aspect_ratio, resolve_replaced_flex_auto_min_main_size,
    resolve_replaced_intrinsic_constraint,
};
pub(crate) use boxes::request::BoxLayoutRequest;

pub(crate) use engine::LayoutEngine;
pub(crate) use engine::LayoutReader;
pub use engine::{LayoutInputs, LayoutOutputs, LayoutScratch};
pub use inline::InlinePlanGeneration;
pub use layout::compute_layout;

pub(crate) use output::PlacementId;
pub use output::PlacementState;
pub(crate) use output::{FragmentWriter, OutputRanges};

pub use paint::{prepare_text_paint, repaint};

#[cfg(test)]
pub use html_pipeline::engine::{
    LaidOutDocument, PreparedDocument, ReadingFragment, RenderDecorationPattern, RenderView,
    ShapedDocument,
};
pub use table::{TableCellPlacement, place_table_cells};
#[cfg(test)]
mod parser {
    pub(crate) use crate::test_support::DocumentFactory;
}
#[cfg(test)]
include!("../../../test-support/stage-fixtures.rs");
