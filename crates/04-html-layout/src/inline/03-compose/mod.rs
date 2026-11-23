//! Select and measure lines; float-sensitive feedback stays within composition.
mod floats;
mod hanging;
pub(super) mod hyphenation;
mod state;
pub(super) use state::CompositionState;
pub(super) mod wrapping;
use super::paragraph::ParagraphLayoutResult;
use super::s02_paragraphs::CompositionOptions;
use super::s04_lines::{LinePreparationContext, prepare_selected_paragraph};
use super::tokens::{INLINE_KP_CACHE_MIN_TOKENS, InlineTokens};
use super::{InlineFormattingInput, TextAlign, TextDirection, TextWrapStyle};
use crate::LayoutEngine;
use crate::measurement::box_intrinsic_widths;
use hyphenation::DeferredHyphenation;
use std::mem::{swap, take};
use wrapping::break_lines_knuth;

/// Compose paragraph-local line geometry, reusing storage from an earlier result.
/// The caller owns paragraph caching and publication.
pub(super) fn compose_paragraph(
    engine: &mut LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
    tokens: &InlineTokens,
    composition: &mut CompositionState,
    options: &CompositionOptions,
    mut paragraph: ParagraphLayoutResult,
) -> ParagraphLayoutResult {
    let context = LinePreparationContext::new(&engine.read.reader, tokens, input, &options.policy);
    let mut deferred_cache = composition.deferred_hyphens.take();
    let mut deferred = context.tokens.features.deferred_hyphenation().then(|| {
        DeferredHyphenation::new(
            engine,
            context.input.run_range.clone(),
            deferred_cache.get_or_insert_with(Default::default),
        )
    });
    if options.has_float_exclusions {
        floats::compose_float_paragraph(engine, &context, deferred.as_mut(), &mut paragraph);
    } else {
        compose_ordinary_paragraph(
            engine,
            &context,
            composition,
            options.cache_break_plan,
            deferred.as_mut(),
            &mut paragraph,
        );
    }
    composition.deferred_hyphens = deferred_cache;
    paragraph
}

fn compose_ordinary_paragraph<'input>(
    engine: &mut LayoutEngine<'input, '_>,
    context: &LinePreparationContext<'_>,
    composition: &mut CompositionState,
    cache_break_plan: bool,
    mut deferred: Option<&mut DeferredHyphenation<'input, '_>>,
    paragraph: &mut ParagraphLayoutResult,
) {
    let mut scratch = engine.scratch.paragraphs.pop().unwrap_or_default();
    let breaker = select_breaker(engine, context);
    let break_storage = take(&mut scratch.lines);
    if matches!(breaker, Breaker::Knuth { .. })
        && cache_break_plan
        && context.tokens.len() >= INLINE_KP_CACHE_MIN_TOKENS
    {
        composition.prepare_kp_plan(
            context.tokens,
            engine.read.constraints.book_optimized_text(),
        );
    }
    let mut lines = match breaker {
        Breaker::Knuth { ragged } => break_lines_knuth(
            engine,
            context.tokens,
            context.input,
            composition.kp_plan.as_ref(),
            ragged,
            break_storage,
            &mut scratch.solver,
        ),
        Breaker::Greedy => wrapping::greedy::break_lines_into(
            engine,
            context.tokens.span(),
            context.tokens.metrics_view(),
            context.input.paragraph,
            context.input.area.width,
            break_storage,
            deferred.as_deref_mut(),
        ),
    };
    balance_lines(
        engine,
        context.input,
        context.tokens,
        &mut lines,
        deferred.as_deref_mut(),
    );
    prepare_selected_paragraph(engine, context, &lines, paragraph);
    scratch.lines = lines;
    scratch.lines.clear();
    // Publication can recurse into atomic content; line breaking is done,
    // so nested paragraphs can reuse its scratch immediately.
    engine.scratch.paragraphs.push(scratch);
}

fn balance_lines<'input>(
    engine: &mut LayoutEngine<'input, '_>,
    input: &InlineFormattingInput,
    tokens: &InlineTokens,
    lines: &mut Vec<wrapping::BrokenLine>,
    mut deferred: Option<&mut DeferredHyphenation<'input, '_>>,
) {
    if input.paragraph.wrap_style == TextWrapStyle::Balance
        && !tokens.features.has_tabs
        && lines.len() > 1
        && engine
            .read
            .reader
            .style(input.container_box_idx)
            .white_space()
            .allows_wrap()
    {
        let count = lines.len();
        let (minimum, _) = box_intrinsic_widths(engine, input.container_box_idx);
        let mut low = (minimum + input.paragraph.indent.amount.abs()).min(input.area.width);
        let mut high = input.area.width;
        let mut candidate = Vec::new();
        for _ in 0..12 {
            if high - low <= 1.0 / 64.0 {
                break;
            }
            let width = (low + high) * 0.5;
            candidate = wrapping::greedy::break_lines_into(
                engine,
                tokens.span(),
                tokens.metrics_view(),
                input.paragraph,
                width,
                candidate,
                deferred.as_deref_mut(),
            );
            if candidate.len() > count {
                low = width;
            } else {
                high = width;
                swap(lines, &mut candidate);
            }
        }
        // The narrower width selects breaks; alignment still uses the
        // original content box, including authored punctuation hanging.
        let hanging_end = hanging::last(engine, tokens.span(), input.paragraph);
        let hanging_start = hanging::first(engine, tokens.span(), input.paragraph);
        for (index, line) in lines.iter_mut().enumerate() {
            let first_hang = if index == 0 { hanging_start } else { 0.0 };
            let last_hang = hanging_end
                .filter(|(index, _)| *index < line.end && *index >= line.start)
                .map_or(0.0, |(_, width)| width);
            let original_indent = line.indent
                + if input.paragraph.direction == TextDirection::Ltr {
                    first_hang
                } else {
                    last_hang
                };
            line.width_limit =
                (input.area.width - original_indent).max(0.0) + first_hang + last_hang;
        }
    }
}

#[derive(Clone, Copy)]
enum Breaker {
    Greedy,
    Knuth { ragged: bool },
}

fn select_breaker(engine: &LayoutEngine<'_, '_>, context: &LinePreparationContext<'_>) -> Breaker {
    if context.tokens.features.has_tabs
        || context.input.paragraph.wrap_style == TextWrapStyle::Stable
    {
        return Breaker::Greedy;
    }
    let ragged = (context.input.paragraph.wrap_style == TextWrapStyle::Pretty
        || (engine.read.constraints.book_optimized_text()
            && context.input.paragraph.wrap_style == TextWrapStyle::Auto
            && context.input.paragraph.text_align
                == (if context.input.paragraph.direction == TextDirection::Rtl {
                    TextAlign::Right
                } else {
                    TextAlign::Left
                })))
        && !context.tokens.features.preserves_spaces;
    if (engine.read.constraints.book_optimized_text()
        && context.input.paragraph.text_align == TextAlign::Justify)
        || ragged
    {
        Breaker::Knuth { ragged }
    } else {
        Breaker::Greedy
    }
}

#[derive(Default)]
pub(crate) struct ParagraphScratch {
    lines: Vec<wrapping::BrokenLine>,
    solver: wrapping::knuth_plass::CompactScratch,
}

impl ParagraphScratch {
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn capacities(&self) -> (usize, (usize, usize, usize)) {
        (self.lines.capacity(), self.solver.capacities())
    }
}
