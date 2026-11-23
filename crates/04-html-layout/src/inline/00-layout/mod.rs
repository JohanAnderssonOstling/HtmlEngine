//! Coordinate token preparation, paragraph caching, composition, and publication.
mod arenas;
mod plans;
mod retained;
use super::intrinsic::measure_tokens;
use super::s01_prepare::{prepare_box_input, prepare_inline_tokens};
use super::s02_paragraphs::{prepare_paragraph, prose_separators};
use super::s03_compose::CompositionState;
use super::s03_compose::compose_paragraph;
use super::s05_emit::emit_prepared_paragraph;
use super::tokens::PreparedInlinePlanKey;
use super::{InlineFormattingInput, InlineTokens, ResolvedTextIndent, Size};
use crate::{LayoutEngine, ResolvedBoxSizing};
use html_engine_model::InlineItemKind;
use kurbo::Point;
pub(crate) use plans::InlinePlanCache;
pub use plans::InlinePlanGeneration;
pub(crate) use retained::ParagraphLayouts;
use retained::{ParagraphDependencies, ParagraphSlot};
use std::mem::take;
use std::ops::Range;

/// Lays out one inline-item range from start to finish.
///
/// This is the entry point used by box layout. It tokenizes the runs,
/// breaks them into logical lines, and then emits fragments from those
/// lines in a separate pass.
pub(crate) fn layout_inline_content(
    engine: &mut LayoutEngine<'_, '_>,
    input: InlineFormattingInput,
) -> Size {
    if input.run_range.start >= input.run_range.end {
        return Size::ZERO;
    }
    let cache_key = text_plan_key(engine, input.run_range.clone(), input.container_box_idx);
    if let Some(key) = cache_key
        && engine.scratch.inline_plans.contains(key)
    {
        // Keep the token owner outside the mutable executor while its slices
        // are borrowed. Text-only replay cannot enter recursive box layout.
        let mut plans = take(&mut engine.scratch.inline_plans);
        let size = plans
            .with_plan(key, |tokens, composition| {
                layout_paragraphs(engine, input, tokens, composition, true)
            })
            .expect("matching retained token plan");
        debug_assert!(
            engine.scratch.inline_plans.is_empty(),
            "text-plan replay must not build nested token plans"
        );
        engine.scratch.inline_plans = plans;
        return size;
    }
    let mut tokens = prepare_inline_tokens(
        engine,
        input.run_range.clone(),
        input.container_box_idx,
        Some(&input),
    );
    let mut composition = CompositionState::default();
    let size = layout_paragraphs(
        engine,
        input,
        &mut tokens,
        &mut composition,
        cache_key.is_some(),
    );
    if let Some(key) = cache_key {
        engine
            .scratch
            .inline_plans
            .insert(key, &tokens, composition);
    }
    engine.scratch.inline_plans.recycle(tokens);
    size
}

pub(crate) fn layout_inline_box(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    run_range: Range<u32>,
    content_pos: Point,
    indent: ResolvedTextIndent,
) -> Size {
    let input = prepare_box_input(engine, resolved, run_range, content_pos, indent);
    layout_inline_content(engine, input)
}

/// Intrinsic queries can create the text plan before its first layout.
pub(crate) fn measure_inline_intrinsic(
    engine: &mut LayoutEngine<'_, '_>,
    run_range: Range<u32>,
    container_box_idx: usize,
) -> (f64, f64) {
    let key = text_plan_key(engine, run_range.clone(), container_box_idx);
    if let Some(key) = key.filter(|&key| engine.scratch.inline_plans.contains(key)) {
        let mut plans = take(&mut engine.scratch.inline_plans);
        let widths = plans
            .with_plan(key, |tokens, _| {
                measure_tokens(engine, container_box_idx, tokens)
            })
            .expect("matching intrinsic text plan");
        engine.scratch.inline_plans = plans;
        return widths;
    }
    let tokens = prepare_inline_tokens(engine, run_range, container_box_idx, None);
    let widths = measure_tokens(engine, container_box_idx, &tokens);
    if let Some(key) = key {
        engine
            .scratch
            .inline_plans
            .insert(key, &tokens, CompositionState::default());
    }
    engine.scratch.inline_plans.recycle(tokens);
    widths
}

/// Only ordinary text has identical tokens for intrinsic measurement and layout.
fn text_plan_key(
    engine: &LayoutEngine<'_, '_>,
    run_range: Range<u32>,
    container_box_idx: usize,
) -> Option<PreparedInlinePlanKey> {
    let runs = &engine.read.text.inline_items()[run_range.start as usize..run_range.end as usize];
    if runs.is_empty()
        || !runs
            .iter()
            .all(|run| matches!(run.kind, InlineItemKind::Text { .. }))
    {
        return None;
    }
    Some(PreparedInlinePlanKey {
        run_start: run_range.start,
        run_end: run_range.end,
        container_box_idx: u32::try_from(container_box_idx)
            .expect("inline-plan box index exhausted"),
        composition: engine.read.constraints.text_composition_policy(),
    })
}

/// Execute ordinary content once or reader-prose segments in source order.
fn layout_paragraphs(
    engine: &mut LayoutEngine<'_, '_>,
    mut input: InlineFormattingInput,
    tokens: &mut InlineTokens<'_>,
    composition: &mut CompositionState,
    retain_token_plan: bool,
) -> Size {
    let has_float_exclusions = tokens.features.has_float_anchors
        || !engine.scratch.floats.current_float_context().is_empty();
    let separators = prose_separators(&engine.read.reader, &engine.read.text, &input, tokens);
    if separators.is_empty() {
        return layout_prepared_tokens(
            engine,
            &input,
            tokens,
            composition,
            has_float_exclusions,
            retain_token_plan,
        );
    }
    let custom_indent = engine
        .read
        .reader
        .reader_text_indent(input.container_box_idx)
        .is_some();
    let mut size = Size::ZERO;
    let mut start = 0;
    for boundary in 0..=separators.len() {
        let separator = separators.get(boundary).copied();
        let end = separator.map_or(tokens.len(), |(_, last, _)| last + 1);
        let mut paragraph = tokens.take_paragraph(start..end, separator.map(|(a, b, _)| (a, b)));
        let measured = layout_prepared_tokens(
            engine,
            &input,
            &mut paragraph,
            &mut CompositionState::default(),
            has_float_exclusions,
            false,
        );
        tokens.restore_paragraph(paragraph);
        size.width = size.width.max(measured.width);
        size.height += measured.height;
        if let Some((_, last, gap)) = separator {
            size.height += gap;
            input.area.origin.y += measured.height + gap;
            input.starts_block = false;
            input.paragraph.indent.first_line = custom_indent || input.paragraph.indent.each_line;
            start = last + 1;
        }
    }
    size
}

fn layout_prepared_tokens(
    engine: &mut LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
    tokens: &mut InlineTokens,
    composition: &mut CompositionState,
    has_float_exclusions: bool,
    retain_token_plan: bool,
) -> Size {
    if tokens.is_empty() {
        return Size::ZERO;
    }
    let options = prepare_paragraph(
        engine,
        input,
        tokens,
        has_float_exclusions,
        retain_token_plan,
    );
    let cacheable = retain_token_plan && !has_float_exclusions && tokens.replaced.is_empty();
    let dependencies = ParagraphDependencies::new(engine, input, options.policy);
    let slot = ParagraphSlot::new(engine, input);
    let mut checkout =
        engine
            .scratch
            .paragraph_layouts
            .checkout(slot, dependencies, tokens.len(), cacheable);
    if checkout.needs_composition {
        checkout.paragraph = compose_paragraph(
            engine,
            input,
            tokens,
            composition,
            &options,
            checkout.paragraph,
        );
    }
    emit_prepared_paragraph(engine, input, tokens, &mut checkout.paragraph, cacheable);
    engine.scratch.paragraph_layouts.commit(checkout)
}
