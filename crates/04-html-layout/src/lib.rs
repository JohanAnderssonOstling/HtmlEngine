//! Backend-neutral geometry execution over immutable prepared/shaped inputs.
//! Document ownership and stage orchestration live in html-pipeline.
#![cfg_attr(not(test), forbid(unsafe_code))]

pub use html_engine_model::*;
mod layout;
mod flex_grid;
mod table;
#[cfg(test)]
pub use html_pipeline::engine::{
    LaidOutDocument, PreparedDocument, ReadingFragment, RenderDecorationPattern, RenderView,
    ShapedDocument,
};
pub use layout::engine::{
    LayoutInputs, LayoutOutputs, LayoutScratch, compute_layout, repaint};
pub use layout::{InlinePlanGeneration, PlacementState, prepare_text_paint};
#[cfg(test)]
mod parser {
    pub(crate) use crate::test_support::DocumentFactory;
}
#[cfg(test)]
include!("../../../test-support/stage-fixtures.rs");
