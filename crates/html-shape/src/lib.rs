//! Font shaping of prepared logical text; no viewport layout or lifecycle ownership.
#![cfg_attr(not(test), forbid(unsafe_code))]

#[cfg(test)]
use html_engine_model as layout_model;
#[cfg(feature = "profiling")]
pub use html_engine_model::profiling;
pub use html_engine_model::*;
use html_style_model::{
    BorderStyle, ComputedStyles, LanguageId, StyleIndices, TextOverflow, VerticalAlignValue,
};
use html_text::{FirstLetterStyles, StyleOverrides, get_style};
use rustc_data_structures::fx::FxHashMap;
use std::ops::Range;
mod execution;
mod spans;
pub use execution::{reshape_range_with_style, shape_document};
#[cfg(test)]
pub use html_pipeline::engine::{LaidOutDocument, PreparedDocument, ShapedDocument};
#[cfg(test)]
use html_text::{
    collapse_whitespace, first_letter_overrides_with_characters, first_letter_style_overrides,
    is_css_collapsible_space, transform_text,
};
use spans::plan_shaping_spans;
#[cfg(test)]
mod parser {
    pub(crate) use crate::test_support::DocumentFactory;
}
#[cfg(test)]
include!("../../../test-support/stage-fixtures.rs");
include!("shaping_tests.rs");
