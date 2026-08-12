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

mod algorithm;
mod decorations;
mod fragments;
mod justification;
mod paragraph;
mod text_tokens;
pub(in crate::layout) mod tokens;
mod wrapping;

use fragments::*;
use paragraph::{layout_inline_content_around_float_exclusions, prepare_inline_tokens};
use text_tokens::*;
use tokens::*;
use wrapping::*;

pub(in crate::layout) use algorithm::layout_inline_content;
pub(super) use paragraph::{
    InlineFormattingInput, InlineLayoutArea, ParagraphLayout, ResolvedTextIndent,
};
pub(crate) use tokens::PreparedInlinePlans;
pub(in crate::layout) use tokens::{
    BreakKind, ParallelInlinePlanTask, canonical_text_unit, collect_parallel_inline_plan_tasks,
    preserved_tab_metrics, run_belongs_to_inline_context, tab_reference_style,
};

pub(in crate::layout) fn prepare_cached_inline_plan(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    task: &ParallelInlinePlanTask,
) {
    let tokens = tokens::build_inline_tokens(
        engine,
        task.run_range.clone(),
        task.container_box_idx,
        0.0,
        None,
    );
    // Construct the width-independent breakpoint stream as part of the same
    // worker job. The dynamic-programming line choice remains width-dependent
    // and is therefore deferred until the paragraph is laid out.
    let _ = tokens.kp_plan(engine.config.book_optimized_text());
}

include!("tests.rs");
