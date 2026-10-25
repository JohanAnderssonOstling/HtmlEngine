use crate::layout::FloatSide;
use crate::layout::replaced::{
    ReplacedSizeInput, preferred_aspect_ratio, resolve_replaced_content_size,
};
use crate::layout_model::{
    EllipsisFragment, GlyphAdvanceRun, GlyphOffsetRun, HyphenFragment, ImageFragment, InlineItem,
    InlineItemKind, Line,
};
use html_style_model::{
    BoxSizing, Float, OverflowWrap, TabSizeKind, TextAlign, TextBoxOverEdge, TextBoxTrim,
    TextBoxUnderEdge, TextOverflow, UsedPreferredSize as PreferredSize, VerticalAlignValue,
    WhiteSpace, WordBreak, resolve_used_preferred_size,
};
use kurbo::{Point, Size, Vec2};
use std::ops::Range;

#[derive(Default)]
pub(super) struct ParagraphScratch {
    lines: Vec<wrapping::BrokenLine>,
    solver: wrapping::knuth_plass::CompactScratch,
}

impl ParagraphScratch {
    #[cfg(test)]
    pub(super) fn capacities(&self) -> (usize, (usize, usize, usize)) {
        (self.lines.capacity(), self.solver.capacities())
    }
}

mod algorithm;
mod decorations;
mod fragments;
mod justification;
mod paragraph;
mod retained;
pub(in crate::layout) use retained::ParagraphLayouts;
use retained::{LineAdjustment, ParagraphDependencies, ParagraphLookup, ParagraphSlot, PreparedLine, PreparedParagraph};
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
pub(crate) use tokens::PreparedInlinePlans;
pub(in crate::layout) use tokens::{
    BreakKind, canonical_text_unit, preserved_tab_metrics, run_belongs_to_inline_context,
    tab_reference_style,
};

include!("tests.rs");
