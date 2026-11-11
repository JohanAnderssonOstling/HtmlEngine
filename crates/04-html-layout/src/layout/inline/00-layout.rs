use super::s02_compose::layout_prepared_tokens;
use super::s01_tokenize::prepare_inline_tokens;
use super::tokens::{BreakKind, TextPreparationRequest};
use super::{
    InlineFormattingInput, InlineLayoutArea, InlineTokenKind, InlineTokens, ParagraphLayout,
    ResolvedTextIndent, Size, TokenMetadata};
use crate::layout::{LayoutEngine, ResolvedBoxSizing};
use kurbo::Point;
use std::ops::Range;
use std::mem::take;

/// Lays out one inline-item range from start to finish.
///
/// This is the entry point used by box layout. It tokenizes the runs,
/// breaks them into logical lines, and then emits fragments from those
/// lines in a separate pass.
pub(in crate::layout) fn layout_inline_content(
    engine: &mut LayoutEngine<'_, '_>,
    input: InlineFormattingInput,
) -> Size {
    let run_range = &input.run_range;
    if run_range.start >= run_range.end {
        return Size::ZERO;
    }
    let preparation = TextPreparationRequest::new(
        engine.text_preparation_inputs(),
        run_range,
        input.container_box_idx,
    );
    let cache_key = preparation.as_ref().map(TextPreparationRequest::cache_key);
    if let Some(key) = cache_key
        && engine.scratch.inline_plans.contains(key)
    {
        // Keep the token owner outside the mutable executor while its slices
        // are borrowed. Text-only replay cannot enter recursive box layout.
        let mut plans = take(&mut engine.scratch.inline_plans);
        let has_float_exclusions = engine
            .scratch
            .floats
            .current_float_context()
            .is_some_and(|context| !context.is_empty());
        let size = plans
            .with_plan(key, |tokens| {
                execute_inline_tokens(engine, input, tokens, has_float_exclusions, true)
            })
            .expect("matching retained token plan");
        debug_assert!(
            engine.scratch.inline_plans.is_empty(),
            "text-plan replay must not build nested token plans"
        );
        engine.scratch.inline_plans = plans;
        return size;
    }
    let (mut tokens, has_float_exclusions) = prepare_inline_tokens(engine, &input, preparation);
    let size = execute_inline_tokens(
        engine,
        input,
        &mut tokens,
        has_float_exclusions,
        cache_key.is_some(),
    );
    engine.scratch.inline_plans.restore(cache_key, tokens);
    size
}

fn execute_inline_tokens(
    engine: &mut LayoutEngine<'_, '_>,
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
        || tokens
            .metrics_view()
            .iter(tokens.span())
            .any(|(token, metrics)| {
                let ws = metrics.white_space;
                // Anchors are positioned before line composition; keep their
                // source flow intact instead of relocating already placed floats.
                ws.preserves_spaces()
                    || ws.preserves_newlines()
                    || matches!(
                        token.kind(),
                        InlineTokenKind::FloatAnchor { .. }
                            | InlineTokenKind::AbsoluteAnchor { .. }
                    )
            })
    {
        return Vec::new();
    }
    let authored_break = |token: &TokenMetadata| {
        token.is_authored_break()
            && matches!(
                token.kind(),
                InlineTokenKind::Break {
                    clear: html_style_model::Clear::None
                }
            )
    };
    let prose = |token: &TokenMetadata| {
        let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
            return false;
        };
        engine.text.character(glyph_idx).is_alphanumeric()
    };
    let mut separators = Vec::new();
    let mut index = 0;
    let mut passage_start = 0;
    while index < tokens.len() {
        if !authored_break(&tokens.span().metadata_at(index)) {
            index += 1;
            continue;
        }
        let first = index;
        let mut last = index;
        let mut count = 0;
        while index < tokens.len() {
            let token = tokens.span().metadata_at(index);
            if authored_break(&token) {
                count += 1;
                last = index;
            } else if !(token.break_kind() == BreakKind::Soft
                && !tokens
                    .metrics_view()
                    .at(tokens.span(), index)
                    .white_space
                    .preserves_spaces())
                && !matches!(token.kind(), InlineTokenKind::InlineBoundary { .. })
            {
                break;
            }
            index += 1;
        }
        if count == 2
            && tokens
                .span()
                .slice(passage_start..first)
                .metadata_iter()
                .any(|token| prose(&token))
            && tokens
                .span()
                .slice(index..)
                .metadata_iter()
                .take_while(|token| !matches!(token.kind(), InlineTokenKind::Break { .. }))
                .any(|token| prose(&token))
        {
            let gap = engine
                .reader
                .reader_paragraph_gap(input.container_box_idx)
                .unwrap_or_else(|| {
                    f64::from(tokens.metrics_view().at(tokens.span(), last).line_height)
                });
            separators.push((first, last, gap));
        }
        passage_start = last + 1;
    }
    separators
}

fn layout_prose_paragraphs(
    engine: &mut LayoutEngine<'_, '_>,
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

pub(in crate::layout) fn layout_inline_box(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    run_range: Range<u32>,
    content_pos: Point,
    indent: ResolvedTextIndent,
) -> Size {
    let box_idx = resolved.box_idx();
    let percentage_height_basis =
        if engine.reader.is_inline_box(box_idx) || engine.reader.is_anonymous_box(box_idx) {
            resolved
                .replaced_size
                .map(|size| size.height)
                .or(resolved.vertical.descendant_basis)
        } else {
            resolved.vertical.descendant_basis
        };
    let style = engine.reader.style(box_idx);
    let indent = engine
        .reader
        .reader_text_indent(box_idx)
        .map_or(indent, |amount| ResolvedTextIndent {
            amount,
            hanging: false,
            each_line: false,
            first_line: true,
        });
    let paragraph = ParagraphLayout {
        indent,
        direction: style.direction(),
        text_align: style.text_align(),
        text_align_last: style.text_align_last(),
        wrap_style: style.text_wrap_style(),
        hanging_punctuation: style.hanging_punctuation(),
    };
    layout_inline_content(
        engine,
        InlineFormattingInput {
            container_box_idx: box_idx,
            starts_block: true,
            run_range,
            area: InlineLayoutArea {
                origin: content_pos,
                width: resolved.horizontal.content,
                percentage_height_basis,
            },
            paragraph,
        },
    )
}
