//! Line measurement, token placement, and fragment publication.
//!
//! This module consumes already-selected line ranges and never decides where
//! the paragraph should wrap.

use super::*;
use crate::layout_model::{LayoutMode, LineInlineBoxFragment, LineTextFragment};
use unicode_categories::UnicodeCategories;

#[derive(Clone, Copy)]
pub(super) struct TokenPlacement {
    pub(super) x: f64,
    pub(super) advance: f64,
    pub(super) y_offset: f32,
    pub(super) relative_offset: Vec2,
}

mod alignment;
mod emission;
mod measurement;
mod preparation;

use alignment::{adjusted_line_metrics, build_token_placements};
use emission::{inline_relative_position_offset, write_line_fragments};
use measurement::{measure_line, summarize_token_span};
pub(super) use preparation::pseudo_line_heights;
use preparation::{
    apply_first_line_pseudo_styles, apply_inline_overflow, first_line_color,
    include_container_strut, inline_box_struts, parent_font_content_extents, strut_for_inline_box,
};

pub(super) struct LineLayout {
    pub(super) glyph_range: Range<u32>,
    pub(super) width: f64,
    pub(super) x_offset: f64,
    pub(super) line_height: f64,
    pub(super) baseline: f64,
    pub(super) word_spacing: f64,
    pub(super) letter_spacing: f64,
    /// Exact advances for justified spaces whose punctuation context gives
    /// them a different share of the line's bounded glue adjustment.
    pub(super) punctuation_space_advances: Vec<(u32, f32)>,
    pub(super) optical_offset_x: f64,
    pub(super) hyphen: Option<(crate::GlyphId, f64)>,
    pub(super) publish_empty: bool,
    /// Plain, baseline-aligned text needs no per-glyph placement storage. Its
    /// glyph range is contiguous and every offset is zero.
    pub(super) placements: Option<Vec<TokenPlacement>>,
}

pub(super) struct TokenSpanSummary {
    pub(super) glyph_range: Range<u32>,
    pub(super) has_text: bool,
    pub(super) has_images: bool,
    pub(super) has_baseline_relative_image: bool,
    pub(super) plain_text: bool,
    pub(super) preserves_newlines: bool,
    pub(super) width: f64,
    pub(super) line_height: f64,
    pub(super) baseline: f64,
}

/// The vertical CSS contribution of one non-replaced inline box. These are
/// derived once for a selected line from retained box ownership; they do not
/// enlarge the dense token stream walked by line breaking.
#[derive(Clone, Copy)]
struct InlineBoxStrut {
    box_idx: usize,
    ascent: f64,
    descent: f64,
    line_height: f64,
    font_size: f64,
}

/// Paragraph-wide inputs shared by every line emitted from one token plan.
pub(super) struct LineEmissionContext<'a> {
    tokens: &'a InlineTokens,
    input: &'a InlineFormattingInput,
    overflow: InlineOverflow,
    first_line_height: Option<f64>,
    first_letter_height: Option<f64>,
}

impl<'a> LineEmissionContext<'a> {
    pub(super) fn new(
        tokens: &'a InlineTokens,
        input: &'a InlineFormattingInput,
        overflow: InlineOverflow,
        first_line_height: Option<f64>,
        first_letter_height: Option<f64>,
    ) -> Self {
        Self {
            tokens,
            input,
            overflow,
            first_line_height,
            first_letter_height,
        }
    }
}

/// Measures and emits broken lines, returning the final laid-out size.
///
/// This is the write-side pass of the pipeline. It turns `BrokenLine`
/// records into measured line layouts and then writes document fragments.
pub(super) fn emit_lines(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    tokens: &InlineTokens,
    input: &InlineFormattingInput,
    overflow: InlineOverflow,
    first_line_height: Option<f64>,
    first_letter_height: Option<f64>,
    lines: &[BrokenLine],
) -> Size {
    let timing_started = engine.start_timing();
    let context = LineEmissionContext::new(
        tokens,
        input,
        overflow,
        first_line_height,
        first_letter_height,
    );
    let mut y = 0.0;
    let mut max_width = 0.0f64;
    for (line_index, line) in lines.iter().enumerate() {
        let origin = Point::new(input.area.origin.x, input.area.origin.y + y);
        let (width, height) = emit_line(engine, &context, line, line_index == 0, origin);
        max_width = max_width.max(width);
        y += height;
    }
    let size = Size::new(max_width, y);
    engine.record_timing(|t| t.emit_lines += timing_started.elapsed());
    size
}

/// Measures and publishes one selected line. Ordinary and float-constrained
/// paragraphs share this write path; only their selected line geometry differs.
pub(super) fn emit_line(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    context: &LineEmissionContext<'_>,
    line: &BrokenLine,
    is_first_line: bool,
    origin: Point,
) -> (f64, f64) {
    let tokens = context.tokens;
    let original_span = &tokens[line.start..line.end];
    let mut overflow_adjusted = Vec::new();
    let span = apply_inline_overflow(
        original_span,
        &tokens.runs,
        line.tab_origin,
        line.width_limit,
        context.overflow,
        &mut overflow_adjusted,
    );
    let mut pseudo_adjusted = Vec::new();
    let mut pseudo_runs = Vec::new();
    let (span, line_runs) = if is_first_line {
        apply_first_line_pseudo_styles(
            engine,
            span,
            &tokens.runs,
            context.first_line_height,
            context.first_letter_height,
            &mut pseudo_adjusted,
            &mut pseudo_runs,
        )
    } else {
        (span, tokens.runs.as_ref())
    };
    let summary_offset = (span.as_ptr() == original_span.as_ptr()
        && line_runs.as_ptr() == tokens.runs.as_ptr())
    .then_some(line.start);
    let inline_struts = inline_box_struts(engine, context.input.container_box_idx, span, line_runs);
    let mut summary = summarize_token_span(
        span,
        line_runs,
        &tokens.summary_blocks,
        summary_offset,
        line.tab_origin,
        &inline_struts,
    );
    include_container_strut(
        engine,
        context.input.container_box_idx,
        is_first_line.then_some(context.first_line_height).flatten(),
        &mut summary,
    );
    let empty_break_metrics = line
        .empty_segment_break
        .and_then(|index| tokens.get(index))
        .map(|token| *token.run_metrics(&tokens.runs));
    let layout = measure_line(
        engine,
        span,
        line_runs,
        summary,
        line,
        empty_break_metrics,
        &inline_struts,
        context.input.container_box_idx,
        context.input.area.width,
    );
    let fragment_origin = Point::new(origin.x + line.indent, origin.y);
    let paint_color = is_first_line
        .then(|| first_line_color(engine, context.input.container_box_idx))
        .flatten();
    let height = write_line_fragments(
        engine,
        context.input.container_box_idx,
        span,
        line_runs,
        &tokens.replaced,
        fragment_origin,
        &layout,
        paint_color,
        context.input.area.width,
    );
    (layout.width + line.indent, height)
}
