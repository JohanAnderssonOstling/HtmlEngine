use super::composition::layout_prepared_tokens;
use super::token_preparation::prepare_inline_tokens;
use super::tokens::{cached_inline_plan_key, token_is_collapsible_line_edge_space};
use super::{InlineFormattingInput, InlineToken, InlineTokenKind, InlineTokens, Size};
use std::mem::take;

/// Lays out one inline-item range from start to finish.
///
/// This is the entry point used by box layout. It tokenizes the runs,
/// breaks them into logical lines, and then emits fragments from those
/// lines in a separate pass.
pub(in crate::layout) fn layout_inline_content(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    input: InlineFormattingInput,
) -> Size {
    profile_scope!("layout.inline_context");
    let timing_started = engine.start_timing();
    let run_range = &input.run_range;
    if run_range.start >= run_range.end {
        engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
        return Size::ZERO;
    }
    let cache_key = cached_inline_plan_key(engine, run_range, input.container_box_idx);
    if let Some(key) = cache_key
        && engine.inline_plans.contains(key)
    {
        // Keep the token owner outside the mutable executor while its slices
        // are borrowed. Text-only replay cannot enter recursive box layout.
        let mut plans = take(&mut engine.inline_plans);
        let has_float_exclusions = engine
            .floats
            .current_float_context()
            .is_some_and(|context| !context.is_empty());
        let token_started = engine.start_timing();
        let size = plans
            .with_plan(key, |tokens| {
                engine.record_timing(|t| t.build_inline_tokens += token_started.elapsed());
                execute_inline_tokens(engine, input, tokens, has_float_exclusions, true)
            })
            .expect("matching retained token plan");
        debug_assert!(
            engine.inline_plans.is_empty(),
            "text-plan replay must not build nested token plans"
        );
        engine.inline_plans = plans;
        engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
        return size;
    }
    let (mut tokens, has_float_exclusions) = prepare_inline_tokens(engine, &input);
    let size = execute_inline_tokens(
        engine,
        input,
        &mut tokens,
        has_float_exclusions,
        cache_key.is_some(),
    );
    engine.inline_plans.restore(cache_key, tokens);
    engine.record_timing(|t| t.layout_runs += timing_started.elapsed());
    size
}

fn execute_inline_tokens(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    input: InlineFormattingInput,
    tokens: &mut InlineTokens<'_>,
    has_float_exclusions: bool,
    immutable_text: bool,
) -> Size {
    let separators = prose_separators(engine, &input, tokens);
    if separators.is_empty() {
        layout_prepared_tokens(engine, &input, tokens, has_float_exclusions, immutable_text)
    } else {
        layout_prose_paragraphs(engine, input, tokens, &separators, has_float_exclusions)
    }
}

/// Only exactly two authored BRs between prose passages form a paragraph
/// boundary. Preserve single breaks, larger runs, preformatted text and verse.
fn prose_separators(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
    tokens: &InlineTokens,
) -> Vec<(usize, usize, f64)> {
    if !engine.reader.is_reader_prose(input.container_box_idx)
        || tokens.iter().any(|token| {
            let ws = token.run_metrics(&tokens.runs).white_space;
            // Anchors are positioned before line composition; keep their
            // source flow intact instead of relocating already placed floats.
            ws.preserves_spaces()
                || ws.preserves_newlines()
                || matches!(
                    token.kind(),
                    InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. }
                )
        })
    {
        return Vec::new();
    }
    let authored_break = |token: &InlineToken| {
        token.is_authored_break()
            && matches!(
                token.kind(),
                InlineTokenKind::Break {
                    clear: html_style_model::Clear::None
                }
            )
    };
    let prose = |token: &InlineToken| {
        let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
            return false;
        };
        engine.text.character(glyph_idx).is_alphanumeric()
    };
    let mut separators = Vec::new();
    let mut index = 0;
    let mut passage_start = 0;
    while index < tokens.len() {
        if !authored_break(&tokens[index]) {
            index += 1;
            continue;
        }
        let first = index;
        let mut last = index;
        let mut count = 0;
        while index < tokens.len() {
            let token = &tokens[index];
            if authored_break(token) {
                count += 1;
                last = index;
            } else if !token_is_collapsible_line_edge_space(token, &tokens.runs)
                && !matches!(token.kind(), InlineTokenKind::InlineBoundary { .. })
            {
                break;
            }
            index += 1;
        }
        if count == 2
            && tokens[passage_start..first].iter().any(&prose)
            && tokens[index..]
                .iter()
                .take_while(|token| !matches!(token.kind(), InlineTokenKind::Break { .. }))
                .any(&prose)
        {
            let gap = engine
                .reader
                .reader_paragraph_gap(input.container_box_idx)
                .unwrap_or_else(|| tokens[last].line_height(&tokens.runs));
            separators.push((first, last, gap));
        }
        passage_start = last + 1;
    }
    separators
}

fn layout_prose_paragraphs(
    engine: &mut crate::layout::LayoutEngine<'_, '_>,
    mut input: InlineFormattingInput,
    tokens: &mut InlineTokens,
    separators: &[(usize, usize, f64)],
    has_float_exclusions: bool,
) -> Size {
    let custom_indent = engine
        .reader
        .reader_text_indent(input.container_box_idx)
        .is_some();
    let mut size = Size::ZERO;
    let mut start = 0;
    for boundary in 0..=separators.len() {
        let separator = separators.get(boundary).copied();
        let end = separator.map_or(tokens.len(), |(_, last, _)| last + 1);
        let mut paragraph = tokens.take_paragraph(start..end, separator.map(|(a, b, _)| (a, b)));
        let measured =
            layout_prepared_tokens(engine, &input, &mut paragraph, has_float_exclusions, false);
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
