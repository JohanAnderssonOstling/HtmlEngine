use crate::layout::FloatSide;
use crate::layout::read_context::LayoutReader;
use crate::layout::replaced::{
    ReplacedSizeInput, preferred_aspect_ratio, resolve_replaced_content_size,
};
use crate::layout_model::{
    EllipsisFragment, GlyphAdvanceRun, GlyphOffsetRun, HyphenFragment, ImageFragment, InlineItem,
    InlineItemKind, Line,
};
use html_style_model::{
    BoxSizing, Float, Hyphens, LineBreak, OverflowWrap, TabSizeKind, TextAlign, TextBoxOverEdge, TextBoxTrim,
    TextBoxUnderEdge, TextOverflow, UsedPreferredSize as PreferredSize, VerticalAlignValue,
    WhiteSpace, WordBreak, TextDirection, UnicodeBidi, resolve_used_preferred_size,
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

mod algorithm;
#[cfg(test)]
mod bidi_tests;
#[cfg(test)]
mod cjk_tests;
mod decorations;
mod fragments;
mod justification;
mod hyphenation;
use hyphenation::*;
mod paragraph;
mod plans;
mod storage;
pub use plans::InlinePlanGeneration;
pub(in crate::layout) use plans::InlinePlanCache;
mod retained;
pub(in crate::layout) use retained::ParagraphLayouts;
use retained::{CachedBreak, CachedParagraph, LineAdjustment, ParagraphDependencies, ParagraphLookup, ParagraphSlot, PreparedLine, PreparedParagraph};
mod text_tokens;
pub(in crate::layout) mod tokens;
mod wrapping;

use fragments::*;
use paragraph::{prepare_lines_around_float_exclusions, prepare_inline_tokens};
use text_tokens::*;
use tokens::*;
use wrapping::*;

pub(in crate::layout) use algorithm::layout_inline_content;
pub(super) use paragraph::{
    InlineFormattingInput, InlineLayoutArea, ParagraphLayout, ResolvedTextIndent,
};
pub(in crate::layout) use tokens::{
    BreakKind, canonical_text_unit, preserved_tab_metrics, run_belongs_to_inline_context,
    tab_reference_style,
};

include!("tests.rs");
