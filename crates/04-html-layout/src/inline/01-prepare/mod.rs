//! Resolve box inputs and build canonical tokens for inline layout.
use super::{
    BoxSizing, BreakKind, Hyphens, INLINE_SUMMARY_BLOCK_TOKENS, InlineItem, InlineItemKind,
    InlineSummaryBlock, InlineToken, InlineTokenKind, InlineTokenMetrics, InlineTokens,
    InlineVerticalAlign, LayoutReadContext, LineBreak, OverflowWrap, PreferredSize,
    PreparedTokenFeatures, ReplacedSizeInput, ReplacedToken, Size, TextDirection, TokenMetadata,
    TokenSpan, Vec2, VerticalAlignValue, WhiteSpace, WordBreak, break_kind,
    canonical_text_unit_from_facts, has_aligned_inline_ancestor, may_be_bidi_separator,
    preferred_aspect_ratio, resolve_replaced_content_size, resolve_used_preferred_size,
    text_vertical_align, token_is_collapsible_line_edge_space,
};
mod hyphenation;
use hyphenation::{automatic_hyphen_breaks, can_defer_hyphenation};
mod items;
mod normalization;
mod text;
use super::{InlineFormattingInput, InlineLayoutArea, ParagraphLayout, ResolvedTextIndent};
use crate::LayoutEngine;
use crate::ResolvedBoxSizing;
use items::{append_non_text_item, standalone_inline_image};
use kurbo::Point;
use std::ops::Range;
use text::{allows_emergency_wrap, append_text_tokens};

pub(super) fn prepare_box_input(
    engine: &LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    run_range: Range<u32>,
    content_pos: Point,
    indent: ResolvedTextIndent,
) -> InlineFormattingInput {
    let box_idx = resolved.box_idx();
    let percentage_height_basis = if engine.read.reader.is_inline_box(box_idx)
        || engine.read.reader.is_anonymous_box(box_idx)
    {
        resolved
            .replaced_size
            .map(|size| size.height)
            .or(resolved.vertical.descendant_basis)
    } else {
        resolved.vertical.descendant_basis
    };
    let style = engine.read.reader.style(box_idx);
    let indent = engine
        .read
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
    }
}

/// Prepare shared text and resolve non-text geometry only when layout supplies a containing area.
pub(super) fn prepare_inline_tokens(
    engine: &mut LayoutEngine<'_, '_>,
    run_range: Range<u32>,
    container_box_idx: usize,
    layout: Option<&InlineFormattingInput>,
) -> InlineTokens<'static> {
    let inputs = engine.read;
    let runs = &inputs.text.inline_items()[run_range.start as usize..run_range.end as usize];
    let standalone_image =
        layout.and_then(|_| standalone_inline_image(engine, runs, container_box_idx));
    let mut has_atomic = false;
    let mut tokens = engine.scratch.inline_plans.working_tokens(0);
    let mut glyph_capacity = 0;
    let mut has_automatic_hyphens = false;
    for run in runs {
        glyph_capacity += match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                glyphs.end.saturating_sub(glyphs.start) as usize
            }
            _ => 1,
        };
        has_automatic_hyphens = has_automatic_hyphens
            || inputs.reader.style(run.box_idx as usize).hyphens() == Hyphens::Auto;
    }
    let deferred = has_automatic_hyphens && can_defer_hyphenation(&inputs, runs, container_box_idx);
    let hyphen_breaks = if !has_automatic_hyphens || deferred {
        Default::default()
    } else {
        automatic_hyphen_breaks(
            &inputs,
            &mut engine.scratch.hyphenation_dictionary,
            runs,
            container_box_idx,
        )
    };
    let default_ltr = inputs.text.is_resolved_default_ltr();
    tokens.data.reserve(glyph_capacity);
    tokens
        .summary_blocks
        .reserve(glyph_capacity.div_ceil(INLINE_SUMMARY_BLOCK_TOKENS));
    tokens.features.set_deferred_hyphenation(deferred);
    // An RTL overflow ellipsis can introduce ordering even when source levels
    // are default LTR, so keep its source glyphs ready for bidi clustering.
    let prepare_bidi_clusters =
        !default_ltr || inputs.reader.style(container_box_idx).direction() == TextDirection::Rtl;
    tokens
        .features
        .set_no_bidi_glyph_separators(prepare_bidi_clusters);
    let mut text_cursor = inputs.text.cursor();
    for (offset, run) in runs.iter().enumerate() {
        if !inputs.inline_item_belongs_to_context(run, container_box_idx) {
            continue;
        }
        let style = inputs.reader.style(run.box_idx as usize);
        if let InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } = &run.kind {
            if layout.is_none() && matches!(run.kind, InlineItemKind::Marker { .. }) {
                continue;
            }
            append_text_tokens(
                &inputs,
                &mut tokens,
                &mut text_cursor,
                glyphs.clone(),
                run.box_idx as usize,
                style,
                style.white_space(),
                text_vertical_align(&inputs.reader, run.box_idx as usize),
                &hyphen_breaks,
                prepare_bidi_clusters,
            );
        } else if let Some(input) = layout {
            has_atomic |= append_non_text_item(
                engine,
                run,
                &mut tokens,
                container_box_idx,
                (input.area.width - input.paragraph.indent.amount.max(0.0)).max(0.0),
                input.area.percentage_height_basis,
                standalone_image,
            );
        } else {
            // Source references need no layout metrics or replaced payload.
            tokens.data.push(InlineToken::new(
                InlineTokenKind::SourceItem {
                    item_idx: run_range.start + offset as u32,
                },
                0.0,
                BreakKind::None,
                false,
                true,
            ));
        }
    }
    if has_atomic {
        tokens.resolve_atomic_break_opportunities(&inputs);
    }
    finish_text_tokens(
        &inputs,
        &mut tokens,
        layout.is_some()
            || runs
                .iter()
                .all(|run| matches!(run.kind, InlineItemKind::Text { .. })),
    );
    tokens
}

/// Both consumers use the same break facts; only layout-ready streams need summaries.
fn finish_text_tokens(
    inputs: &LayoutReadContext<'_>,
    tokens: &mut InlineTokens<'_>,
    resolved: bool,
) {
    if inputs.text.content().has_unicode_wraps() && tokens.features.has_unicode_breaks() {
        tokens.prepare_unicode_boundaries(inputs);
    }
    if tokens.runs.iter().any(|metrics| {
        matches!(
            metrics.white_space,
            WhiteSpace::PreWrap | WhiteSpace::BreakSpaces
        )
    }) {
        tokens.prepare_preserved_space_boundaries(inputs);
    }
    if resolved {
        tokens.classify_space_glue(inputs);
        tokens.rebuild_summary_blocks();
    }
}
