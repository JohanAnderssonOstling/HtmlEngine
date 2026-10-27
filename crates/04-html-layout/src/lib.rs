//! Backend-neutral geometry execution over immutable prepared/shaped inputs.
//! Document ownership and stage orchestration live in html-pipeline.
#![cfg_attr(not(test), forbid(unsafe_code))]
macro_rules! profile_scope {
    ($name:literal) => {
        #[cfg(feature = "profiling")]
        let _profile_scope = crate::profiling::scope($name);
    };
}

use html_engine_model as layout_model;
#[cfg(feature = "profiling")]
pub use html_engine_model::profiling;
pub use html_engine_model::*;
mod layout;
pub use layout::text_placement::{LineCharacterPlacement, LineTextPlacement};
mod flex_grid;
mod table;
#[cfg(test)]
pub use html_pipeline::engine::{
    LaidOutDocument, PreparedDocument, ReadingFragment, RenderDecorationPattern, RenderView,
    ShapedDocument,
};
pub use layout::engine::{
    LayoutInputs, LayoutOutputs, LayoutScratch, LayoutTimings, layout_with_timings, repaint,
};
pub use layout::{InlinePlanGeneration, PlacementState, prepare_text_paint};
#[cfg(test)]
mod parser {
    pub(crate) use crate::test_support::DocumentFactory;
}
#[cfg(test)]
include!("../../../test-support/stage-fixtures.rs");

/// Diagnostics for tests of pipeline-driven layout execution.
#[cfg(feature = "testing")]
pub mod testing {
    pub use crate::layout::{observed_worker_count, reset_observed_workers};
}
