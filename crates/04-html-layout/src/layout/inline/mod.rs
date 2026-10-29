//! Token preparation flows into composition and fragment publication.
//! Retained caches can skip work; float-sensitive line feedback stays inside composition.

use crate::layout::read_context::LayoutReader;
use crate::layout::replaced::{
    ReplacedSizeInput, preferred_aspect_ratio, resolve_replaced_content_size,
};
use crate::layout_model::{
    EllipsisFragment, GlyphAdvanceRun, GlyphOffsetRun, HyphenFragment, ImageFragment, InlineItem,
    InlineItemKind, Line,
};
use html_style_model::{
    BoxSizing, Hyphens, LineBreak, OverflowWrap, TabSizeKind, TextAlign, TextBoxOverEdge,
    TextBoxTrim, TextBoxUnderEdge, TextDirection, TextOverflow, UnicodeBidi,
    UsedPreferredSize as PreferredSize, VerticalAlignValue, WhiteSpace, WordBreak,
    resolve_used_preferred_size,
};
use kurbo::{Point, Size, Vec2};
use std::ops::Range;

#[derive(Default)]
pub(super) struct ParagraphScratch {
    lines: Vec<wrapping::BrokenLine>,
    solver: wrapping::knuth_plass::CompactScratch,
}

impl ParagraphScratch {
    #[cfg(any(test, feature = "testing"))]
    pub(super) fn capacities(&self) -> (usize, (usize, usize, usize)) {
        (self.lines.capacity(), self.solver.capacities())
    }
}

#[path = "00-layout.rs"]
mod algorithm;
#[cfg(test)]
mod bidi_tests;
#[cfg(test)]
mod cjk_tests;
#[path = "02-compose.rs"]
mod composition;
mod decorations;
mod fragments;
mod hyphenation;
mod justification;
#[path = "03-emit.rs"]
mod publication;
#[path = "01-tokenize.rs"]
mod token_preparation;
use hyphenation::*;
mod plans;
mod storage;
pub(in crate::layout) use plans::InlinePlanCache;
pub use plans::InlinePlanGeneration;
mod paragraph;
mod retained;
use paragraph::{LineAdjustment, PreparedLine};
pub(in crate::layout) use retained::ParagraphLayouts;
mod text_tokens;
pub(in crate::layout) mod tokens;
mod wrapping;

use text_tokens::*;
use tokens::*;
use wrapping::*;

pub(in crate::layout) use algorithm::layout_inline_content;
pub(in crate::layout) struct InlineFormattingInput {
    pub(in crate::layout) container_box_idx: usize,
    pub(in crate::layout) starts_block: bool,
    pub(in crate::layout) run_range: Range<u32>,
    pub(in crate::layout) area: InlineLayoutArea,
    pub(in crate::layout) paragraph: ParagraphLayout,
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::layout) struct InlineLayoutArea {
    pub(in crate::layout) origin: Point,
    pub(in crate::layout) width: f64,
    pub(in crate::layout) percentage_height_basis: Option<f64>,
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::layout) struct ParagraphLayout {
    pub(in crate::layout) direction: TextDirection,
    pub(in crate::layout) indent: ResolvedTextIndent,
    pub(in crate::layout) text_align: TextAlign,
    pub(in crate::layout) text_align_last: TextAlign,
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::layout) struct ResolvedTextIndent {
    pub(in crate::layout) amount: f64,
    pub(in crate::layout) hanging: bool,
    pub(in crate::layout) each_line: bool,
    pub(in crate::layout) first_line: bool,
}

pub(in crate::layout) use tokens::{
    BreakKind, canonical_text_unit, preserved_tab_metrics,
    tab_reference_style,
};

include!("tests.rs");
