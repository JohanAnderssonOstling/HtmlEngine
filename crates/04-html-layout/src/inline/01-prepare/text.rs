use super::{
    BreakKind, Hyphens, InlineToken, InlineTokenKind, InlineTokenMetrics, InlineTokens,
    InlineVerticalAlign, LayoutReadContext, LineBreak, OverflowWrap, Range, TokenMetadata,
    VerticalAlignValue, WhiteSpace, WordBreak, canonical_text_unit_from_facts,
    has_aligned_inline_ancestor, may_be_bidi_separator,
};
use crate::inline::reader::TextCursor;
use html_engine_model::{FontRelativeMetrics, GlyphId};
use html_style_model::{UsedStyleView, Visibility};
use html_text::{is_bidi_control, is_mandatory_line_break};
use unicode_bidi::{BidiClass, bidi_class};
use unicode_categories::UnicodeCategories;

pub(super) fn append_text_tokens(
    inputs: &LayoutReadContext<'_>,
    tokens: &mut InlineTokens,
    text_cursor: &mut TextCursor<'_>,
    glyphs: Range<u32>,
    box_idx: usize,
    style: UsedStyleView,
    white_space: WhiteSpace,
    vertical_align: VerticalAlignValue,
    hyphen_breaks: &[u32],
    prepare_bidi_clusters: bool,
) {
    // Resolve backing arrays once per run; split patches retain their lookup
    // path without flattening or copying glyph storage.
    let glyph_range = glyphs.clone();
    let range = glyphs.start as usize..glyphs.end as usize;
    let contiguous = inputs
        .text
        .glyphs()
        .slice(range.clone())
        .and_then(|glyphs| glyphs.as_contiguous())
        .zip(inputs.text.content().characters().get(range));
    if let Some((ids, characters)) = contiguous {
        let input = glyphs
            .zip(ids.iter().zip(characters))
            .map(|(index, (&glyph, &character))| (index, glyph, character));
        append_text_tokens_from_iter(
            inputs,
            tokens,
            text_cursor,
            input,
            glyph_range,
            box_idx,
            style,
            white_space,
            vertical_align,
            hyphen_breaks,
            prepare_bidi_clusters,
        )
    } else {
        let input = glyphs.map(|index| {
            (
                index,
                inputs.text.glyph_at(index as usize).unwrap_or_default(),
                inputs.text.character(index),
            )
        });
        append_text_tokens_from_iter(
            inputs,
            tokens,
            text_cursor,
            input,
            glyph_range,
            box_idx,
            style,
            white_space,
            vertical_align,
            hyphen_breaks,
            prepare_bidi_clusters,
        )
    };
}

fn append_text_tokens_from_iter(
    inputs: &LayoutReadContext<'_>,
    tokens: &mut InlineTokens,
    text_cursor: &mut TextCursor<'_>,
    glyphs: impl Iterator<Item = (u32, GlyphId, char)>,
    run_range: Range<u32>,
    box_idx: usize,
    style: UsedStyleView,
    white_space: WhiteSpace,
    vertical_align: VerticalAlignValue,
    hyphen_breaks: &[u32],
    prepare_bidi_clusters: bool,
) {
    let run_end = run_range.end;
    let letter_spacing = style.letter_spacing() as f64;
    let word_spacing = style.word_spacing() as f64;
    let word_spacing_factor = style.word_spacing_factor() as f64;
    let font_size = style.font_size();
    let fixed_line_height = style.resolved_line_height();
    // An authored line-height uses the first available font's layout bounds;
    // fallback glyphs can paint outside them without changing the baseline.
    // Backends without selected-face extents retain their glyph metrics.
    let selected_extents = (!style.line_height_is_normal())
        .then(|| inputs.reader.font_metrics(box_idx))
        .filter(|metrics| metrics.line_metrics.is_some())
        .map(|metrics| {
            let (ascent, descent) = metrics.line_box_extents(f64::from(font_size));
            (ascent as f32, descent as f32)
        });
    let visible = style.visibility() == Visibility::Visible;
    let placement_required = letter_spacing != 0.0
        || word_spacing != 0.0
        || has_aligned_inline_ancestor(&inputs.reader, box_idx);
    let allows_wrap = white_space.allows_wrap();
    let preserves_spaces = white_space.preserves_spaces();
    let emergency_wrap = allows_emergency_wrap(style, white_space);
    let hyphens =
        if style.word_break() == WordBreak::BreakAll || style.line_break() == LineBreak::Anywhere {
            Hyphens::None
        } else {
            style.hyphens()
        };
    let hyphen_glyph = (hyphens != Hyphens::None)
        .then(|| inputs.text.hyphen_glyph(box_idx))
        .flatten();
    let discretionary_hyphen =
        hyphen_glyph.map(|glyph| (glyph, inputs.text.glyph_metric(glyph).advance() as f64));
    let mut no_bidi_glyph_separators =
        tokens.features.no_bidi_glyph_separators() && prepare_bidi_clusters;
    let (metadata, widths, runs) = (
        tokens.data.metadata.make_mut(),
        tokens.data.widths.make_mut(),
        &mut tokens.runs,
    );
    let mut previous_run_idx = metadata
        .last()
        .map(|&header| TokenMetadata::from_header(header).run_idx());
    let mut previous_dimensions = None;
    let mut selected_style = None;
    let mut selected_style_end = 0;
    let mut effective_extents = selected_extents;
    let primary_height = |metrics: FontRelativeMetrics, size: f32| {
        let (ascent, descent) = metrics.line_box_extents(f64::from(size));
        ascent + descent
    };
    let selected_primary_height = primary_height(inputs.reader.font_metrics(box_idx), font_size);
    let mut effective_primary_height = selected_primary_height;
    // Each source run starts its own cursor, including when ranges overlap.
    let first_hyphen = hyphen_breaks.partition_point(|&glyph| glyph < run_range.start);
    let mut hyphen_offsets = hyphen_breaks[first_hyphen..].iter().copied();
    // The exclusive Range<u32> iterator cannot yield this sentinel.
    let mut next_hyphen = hyphen_offsets.next().unwrap_or(u32::MAX);

    for (glyph_idx, glyph, source_character) in glyphs {
        let (fallback_advance, metric) = text_cursor
            .glyph_dimensions(glyph)
            .expect("registered text metric");
        let (advance, cluster_boundary) =
            text_cursor.character_geometry(glyph_idx as usize, fallback_advance);
        if no_bidi_glyph_separators
            && may_be_bidi_separator(source_character)
            && matches!(bidi_class(source_character), BidiClass::B | BidiClass::S)
        {
            no_bidi_glyph_separators = false;
        }
        let unit = canonical_text_unit_from_facts(
            inputs,
            glyph_idx,
            box_idx,
            run_end,
            source_character,
            advance,
            font_size,
            letter_spacing,
            word_spacing,
            word_spacing_factor,
            white_space,
        );
        let character = unit.character;
        let width = unit.natural_advance;
        // ZERO WIDTH SPACE is a source-authored wrap opportunity, not a
        // paintable glyph. Retaining it as a source control also prevents a
        // fallback font's nonzero missing-glyph advance from leaking into
        // line and intrinsic widths.
        let token_kind = if character == '\u{200b}' && allows_wrap {
            InlineTokenKind::ZeroWidthBreak
        } else {
            InlineTokenKind::Glyph { glyph_idx }
        };
        // Reuse the policy already read for the canonical unit instead of
        // scanning completed tokens to discover Unicode opportunities.
        if unit.unicode_break_before && matches!(token_kind, InlineTokenKind::Glyph { .. }) {
            tokens.features.record_unicode_break();
        }
        let mut token = InlineToken::new(
            token_kind,
            width,
            if matches!(character, '\u{000b}' | '\u{000c}' | '\u{0085}') {
                BreakKind::None
            } else {
                unit.break_kind
            },
            // Pre-wrap space sequences hang together rather than acquiring
            // emergency breaks. Break-spaces still permits those breaks.
            if white_space == WhiteSpace::PreWrap && matches!(character, ' ' | '\t') {
                false
            } else {
                emergency_wrap
            },
            cluster_boundary,
        );
        if character == ' ' {
            token.set_is_space();
        }
        if character == '\t' && preserves_spaces {
            token.set_is_tab();
        }
        if prepare_bidi_clusters && cluster_boundary && !character.is_ascii() && character.is_mark()
        {
            token.set_bidi_continuation();
        }
        // All other metrics are invariant for this source run. Reuse the
        // interned ID while glyph extents and preserved-tab metrics match.
        let placement_required = placement_required
            || (word_spacing_factor != 1.0 && matches!(character, ' ' | '\u{00a0}'));
        // First-line and first-letter refinements can select another face or
        // size. Their selected metrics must follow the style used to shape
        // these glyphs, rather than the originating box's font.
        // Glyphs arrive in logical order. A selection (including no override)
        // remains valid until the next first-line or first-letter boundary.
        if glyph_idx >= selected_style_end {
            let ranges = inputs.text_style_ranges;
            let range = ranges.get(ranges.partition_point(|range| range.glyphs.end <= glyph_idx));
            let (refined_style, end) = match range {
                Some(range) if range.glyphs.start <= glyph_idx => {
                    (Some(range.style), range.glyphs.end)
                }
                next => inputs.text.first_letter_style_segment(
                    glyph_idx,
                    next.map_or(u32::MAX, |range| range.glyphs.start),
                ),
            };
            selected_style_end = end;
            if refined_style != selected_style {
                selected_style = refined_style;
                effective_primary_height =
                    refined_style.map_or(selected_primary_height, |indices| {
                        let style = inputs.reader.used_style(indices);
                        primary_height(inputs.reader.style_font_metrics(indices), style.font_size())
                    });
                effective_extents = refined_style.map_or(selected_extents, |indices| {
                    let style = inputs.reader.used_style(indices);
                    (!style.line_height_is_normal())
                        .then(|| inputs.reader.style_font_metrics(indices))
                        .filter(|metrics| metrics.line_metrics.is_some())
                        .map(|metrics| {
                            let (ascent, descent) =
                                metrics.line_box_extents(f64::from(style.font_size()));
                            (ascent as f32, descent as f32)
                        })
                });
            }
        }
        let (ascent, descent) = effective_extents.unwrap_or((metric.ascent, metric.descent));
        let contributes_line_metrics = !is_bidi_control(character);
        let dimensions = (
            ascent,
            descent,
            placement_required,
            contributes_line_metrics,
        );
        if previous_dimensions == Some(dimensions) {
            token.set_run_idx(previous_run_idx.expect("previous text metrics"));
        } else {
            let glyph_height = f64::from(metric.ascent + metric.descent);
            let line_height = {
                // Ratios and native glyph metrics can differ by a few f32
                // rounding operations. Only a genuinely taller fallback face
                // expands normal leading; keep the primary face's rounded
                // normal height, including first-line/first-letter selections.
                let taller_fallback =
                    glyph_height > effective_primary_height * (1.0 + 4.0 * f64::from(f32::EPSILON));
                if style.line_height_is_normal() && taller_fallback {
                    // Normal leading must accommodate the fonts actually used.
                    // Reusing only the primary face's smaller normal height
                    // gives fallback runs negative leading and lets an inkless
                    // space or empty inline increase the line's total height.
                    fixed_line_height.max(glyph_height)
                } else {
                    fixed_line_height
                }
            };
            let metrics = InlineTokenMetrics {
                owner_box_idx: box_idx as u32,
                ascent,
                descent,
                line_height: line_height as f32,
                font_size,
                vertical_align: InlineVerticalAlign::resolve(
                    vertical_align,
                    line_height,
                    font_size,
                ),
                white_space,
                contributes_line_metrics,
                visible,
                placement_required,
            };
            let index = InlineTokens::bind_metrics_in(runs, previous_run_idx, &mut token, metrics);
            previous_run_idx = Some(index);
            previous_dimensions = Some(dimensions);
        }
        metadata.push(token.header);
        widths.push(token.width);
        // Cc controls still need visible ink. Keep their glyph on this line,
        // then emit the mandatory break separately. LS/PS have no control ink.
        if is_mandatory_line_break(character) && character <= '\u{0085}' {
            let mut forced = InlineToken::new(
                InlineTokenKind::Break {
                    clear: html_style_model::Clear::None,
                },
                0.0,
                BreakKind::Hard,
                false,
                true,
            );
            forced.set_run_idx(token.run_idx());
            metadata.push(forced.header);
            widths.push(0.0);
        }
        if let Some((hyphen_glyph, hyphen_width)) = discretionary_hyphen
            && allows_wrap
        {
            let automatic_hyphen = next_hyphen == glyph_idx;
            if automatic_hyphen {
                next_hyphen = hyphen_offsets.next().unwrap_or(u32::MAX);
            }
            if automatic_hyphen || character == '\u{00ad}' {
                let mut discretionary = InlineToken::new(
                    InlineTokenKind::Discretionary {
                        glyph: hyphen_glyph,
                    },
                    hyphen_width,
                    BreakKind::Discretionary,
                    false,
                    true,
                );
                discretionary.set_run_idx(token.run_idx());
                metadata.push(discretionary.header);
                widths.push(discretionary.width);
            }
        }
    }
    tokens
        .features
        .set_no_bidi_glyph_separators(no_bidi_glyph_separators);
}

/// Emergency word splitting is independent of prepared normal opportunities.
/// `break-all` is resolved during preparation so punctuation rules still apply.
pub(super) fn allows_emergency_wrap(style: UsedStyleView, white_space: WhiteSpace) -> bool {
    white_space.allows_wrap()
        && (style.word_break() == WordBreak::BreakWord
            || matches!(
                style.overflow_wrap(),
                OverflowWrap::BreakWord | OverflowWrap::Anywhere
            ))
}
