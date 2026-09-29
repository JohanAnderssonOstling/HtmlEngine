use crate::layout_model::{
    GlyphId, GlyphMetric, GlyphMetricError, GlyphMetrics, InlineContent, InlineItemKind,
    LayoutMode, LayoutTree, WhitespaceWrapOverride,
};
use html_dom::Document;
use html_style_model::{
    BorderStyle, ComputedStyles, FontStyle, OpenTypeFeature, StyleIndices, StyleView, TextOverflow,
    TextTransform, UsedStyleView, VerticalAlignValue,
};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::fmt;
use std::ops::Range;
use std::sync::Arc;
use unicode_categories::UnicodeCategories;
use unicode_segmentation::UnicodeSegmentation;

mod contracts;
mod execution;
mod normalization;
mod pseudo_styles;
mod spans;

pub(crate) use contracts::{AuthoritativeShapedRun, ShapedFontMetrics, ShapedTextGeometry};
use contracts::{BoxFontMetrics, RequiredFontMetrics};
pub use contracts::{
    CharacterPlacement, FontMetricsRequest, FontRelativeMetrics, FontSlant,
    GlyphResourceGeneration, GlyphResourceStore,
    GlyphShaper, ShapeError, ShapedLine, ShapedTextRun, TextRunId, TextRunShapeRequest,
    TextShapeRequest, TextStyleSpan,
};
pub(crate) use execution::{reshape_range_with_style, shape_document};
#[cfg(test)]
use normalization::is_css_collapsible_space;
pub(crate) use normalization::whitespace_context_root;
use normalization::{collapse_whitespace, get_style, transform_text};
pub(crate) use pseudo_styles::{
    first_letter_style_for_inline_root, first_letter_style_overrides,
    first_line_style_for_inline_root,
};
use spans::{plan_shaping_spans, text_runs};

include!("../shaping_tests.rs");
