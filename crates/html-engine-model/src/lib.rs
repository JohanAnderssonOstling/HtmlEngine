//! Shared owned stage data and borrowed contracts, below formatting, shaping,
//! layout and pipeline orchestration. No stage executor is stored here.
#![forbid(unsafe_code)]
/// Enter a named profiling scope when the calling crate enables `profiling`.
/// Callers forward that feature to `html-engine-model/profiling`.
#[macro_export]
macro_rules! profile_scope {
    ($name:literal) => {
        #[cfg(feature = "profiling")]
        let _profile_scope = $crate::profiling::scope($name);
    };
}

/// Evaluate an operation inside a profiling scope with the same feature gate.
#[macro_export]
macro_rules! profile {
    ($name:literal, $operation:expr) => {{
        $crate::profile_scope!($name);
        $operation
    }};
}

mod bidi;
mod constraints;
mod geometry;
mod glyph_adapter;
mod glyph_line_indices;
mod glyph_metrics;
mod glyph_resources;
mod grouped_arena;
mod image_metrics;
mod inline;
mod layout_state;
mod layout_tree;
mod paint;
pub mod paint_program;
#[cfg(feature = "profiling")]
pub mod profiling;
mod shaping_contracts;
mod shaping_views;
mod table_grid;
mod text_placement;

pub use bidi::{ResolvedBidi, grapheme_boundaries, paragraph_unicode_bidi};
pub use constraints::*;
pub use geometry::*;
pub use glyph_adapter::*;
pub use glyph_line_indices::*;
pub use glyph_metrics::*;
pub use glyph_resources::*;
pub use grouped_arena::*;
use html_style_model::{ComputedStyles, StyleIndices, StyleView, UsedStyleView};
pub use html_style_model::{
    ContentLanguage, FontStyle, LanguageTag, ListStylePosition, OpenTypeFeature,
    ReaderStyleOverrides, StyleStringId, TextAlign, TextDecorationLines, TextDirection,
    ThemeAdaptation, UnicodeBidi, UsedBorderRadii,
};
pub use html_text::{InlineContent, InlineItem, InlineItemKind, WhitespaceWrapOverride};
pub use image_metrics::*;
pub use inline::*;
pub use layout_state::*;
pub use layout_tree::*;
pub use paint::*;
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
pub use shaping_contracts::*;
pub use shaping_views::*;
pub use table_grid::{TableCellPlacement, TableGrid, build_table_grid, place_table_cells};
pub use text_placement::{LineCharacterPlacement, LineTextPlacement};
use std::{fmt, ops::Range, sync::Arc};

/// Positive HTML row/column span, with the default value for invalid attributes.
pub fn parse_positive_span(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(1)
}
