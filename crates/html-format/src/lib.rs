//! Formatting structure construction from styled DOM, before text preprocessing.
#![cfg_attr(not(test), forbid(unsafe_code))]
use html_engine_model as layout_model;
#[cfg(feature = "profiling")]
pub use html_engine_model::profiling;
mod box_builder;
mod list_marker;
pub mod mathml;
pub use box_builder::{build_layout_inputs, build_layout_inputs_from};
pub use html_engine_model::{LayoutTree, NoteFlow, PrepareError};
#[cfg(test)]
pub use html_pipeline::engine::*;
#[cfg(test)]
mod parser {
    pub(crate) use crate::test_support::DocumentFactory;
}
#[cfg(test)]
include!("../../../test-support/stage-fixtures.rs");
