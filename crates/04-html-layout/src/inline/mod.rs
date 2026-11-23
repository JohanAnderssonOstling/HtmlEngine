//! Stage 00 coordinates preparation (01), paragraphs (02), composition (03), lines (04), and emission (05).
//! Stage 00 owns token and paragraph cache decisions and the shared publication path.
//! Composition reads prepared tokens and updates separate derived state.
//! Publication reads line geometry and owns retained fragments.
//! Float-sensitive feedback stays inside composition.
//! Each numbered folder owns its stage entry points in mod.rs and its private helpers.
//! Layout owns its caches and arenas; preparation owns normalization and eager hyphenation.
//! Composition owns wrapping, tracking, deferred hyphenation, and derived state.
//! Geometry records, token storage, metrics, and cross-stage policies remain shared.

mod context;
#[path = "00-layout/mod.rs"]
mod s00_layout;
#[path = "01-prepare/mod.rs"]
mod s01_prepare;
#[path = "02-paragraphs/mod.rs"]
mod s02_paragraphs;
#[path = "03-compose/mod.rs"]
mod s03_compose;
#[path = "04-lines/mod.rs"]
mod s04_lines;
#[path = "05-emit/mod.rs"]
mod s05_emit;

use crate::boxes::replaced::{
    ReplacedSizeInput, preferred_aspect_ratio, resolve_replaced_content_size,
};
use crate::engine::LayoutReader;
use html_engine_model::{
    EllipsisFragment, GlyphAdvanceRun, GlyphOffsetRun, HyphenFragment, ImageFragment, InlineItem,
    InlineItemKind, Line,
};
use html_style_model::{
    BoxSizing, HangingPunctuation, Hyphens, LineBreak, OverflowWrap, TabSizeKind, TextAlign,
    TextBoxOverEdge, TextBoxTrim, TextBoxUnderEdge, TextDirection, TextOverflow, TextWrapStyle,
    UnicodeBidi, UsedPreferredSize as PreferredSize, VerticalAlignValue, WhiteSpace, WordBreak,
    resolve_used_preferred_size,
};
use kurbo::{Point, Size, Vec2};
use std::ops::Range;

pub(super) use s03_compose::ParagraphScratch;

#[cfg(test)]
mod bidi_tests;
mod boundaries;
#[cfg(test)]
mod cjk_tests;
mod fragments;
mod intrinsic;
pub(crate) use s00_layout::measure_inline_intrinsic;
mod hyphenation;
mod justification;
mod metrics;
mod token_span;
pub(crate) use hyphenation::HyphenationDictionaryCache;
use hyphenation::{automatic_hyphen_fragments_are_long_enough, hyphenation_language};
use metrics::MetricsView;
use token_span::TokenSpan;
pub(crate) mod reader;
use crate::engine::LayoutReadContext;
mod storage;
pub(crate) use s00_layout::InlinePlanCache;
pub use s00_layout::InlinePlanGeneration;
mod paragraph;
use paragraph::{LineAdjustment, PreparedLine};
pub(crate) use s00_layout::ParagraphLayouts;
pub(crate) mod tokens;

use s03_compose::wrapping::{
    BrokenLine, KpPlan, LineBreakResult, LineConstraints, break_lines_knuth, break_next_line,
    knuth_plass,
};
#[cfg(any(test, feature = "testing"))]
use tokens::vertical_align_offset;
use tokens::{
    HYPHEN_PENALTY, INLINE_SUMMARY_BLOCK_TOKENS, InlineBidiSummary, InlineSummaryBlock,
    InlineToken, InlineTokenKind, InlineTokenMetrics, InlineTokens, InlineVerticalAlign,
    KP_TOLERANCE, PreparedTokenFeatures, ReplacedToken, TokenMetadata, break_kind,
    canonical_text_unit_from_facts, compute_baseline, has_aligned_inline_ancestor, indent_for_line,
    may_be_bidi_separator, text_vertical_align, token_is_collapsible_line_edge_space,
    vertical_align_metrics_offset,
};

pub(crate) use s00_layout::{layout_inline_box, layout_inline_content};
pub(crate) struct InlineFormattingInput {
    pub(crate) container_box_idx: usize,
    pub(crate) starts_block: bool,
    pub(crate) run_range: Range<u32>,
    pub(crate) area: InlineLayoutArea,
    pub(crate) paragraph: ParagraphLayout,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct InlineLayoutArea {
    pub(crate) origin: Point,
    pub(crate) width: f64,
    pub(crate) percentage_height_basis: Option<f64>,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ParagraphLayout {
    pub(crate) direction: TextDirection,
    pub(crate) indent: ResolvedTextIndent,
    pub(crate) text_align: TextAlign,
    pub(crate) text_align_last: TextAlign,
    pub(crate) wrap_style: TextWrapStyle,
    pub(crate) hanging_punctuation: HangingPunctuation,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ResolvedTextIndent {
    pub(crate) amount: f64,
    pub(crate) hanging: bool,
    pub(crate) each_line: bool,
    pub(crate) first_line: bool,
}

pub(crate) use tokens::{BreakKind, preserved_tab_metrics};

#[cfg(test)]
mod tests;
