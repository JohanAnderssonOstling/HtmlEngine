//! Canonical text advances, whitespace breaks and preserved tab metrics.
use super::TokenMetadata;
use crate::inline::metrics::MetricsCursor;
use crate::LayoutReader;
use crate::engine::LayoutReadContext;
use html_engine_model::WhitespaceWrapOverride;
use html_style_model::{TabSizeKind, WhiteSpace};
use html_text::{
    is_mandatory_line_break, is_word_separator, suppresses_glyph, tracking_boundary_allowed,
};

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BreakKind {
    None,
    Soft,
    Hard,
    Discretionary,
}

pub(in crate::inline) fn token_is_collapsible_line_edge_space(
    token: TokenMetadata,
    metrics: &mut MetricsCursor<'_>,
) -> bool {
    token.break_kind() == BreakKind::Soft && !metrics.at(token).white_space.preserves_spaces()
}

/// Box supplying the space/ch metric used by number-valued `tab-size`.
/// CSS defines it on the nearest block container rather than the inline
/// span that happens to contain the tab.
fn tab_reference_box(reader: &LayoutReader<'_>, box_idx: usize) -> usize {
    let mut current = Some(box_idx);
    while let Some(idx) = current {
        let style = reader.style(idx);
        if style.display() != html_style_model::Display::Inline {
            return idx;
        }
        current = reader.get_parent(idx);
    }
    box_idx
}

/// Canonical metrics for a preserved tab. Both intrinsic measurement and
/// final line placement use this helper so tab stops cannot diverge between
/// sizing and layout.
pub(crate) fn preserved_tab_metrics(reader: &LayoutReader<'_>, box_idx: usize) -> (f64, f64) {
    let reference_box = tab_reference_box(reader, box_idx);
    let style = reader.style(box_idx);
    let reference_style = reader.style(reference_box);
    let space_advance = reference_style.font_size() as f64
        * f64::from(reader.font_metrics(reference_box).space_advance_ratio);
    let ch_advance = reference_style.font_size() as f64
        * f64::from(reader.font_metrics(reference_box).ch_advance_ratio);
    let interval = match style.tab_size().kind() {
        TabSizeKind::Spaces => {
            style.tab_size().value() as f64
                * (space_advance
                    + reference_style.letter_spacing() as f64
                    + reference_style.word_spacing() as f64)
                    .max(0.0)
        }
        TabSizeKind::LengthPx => style.tab_size().value() as f64,
    };
    (
        interval.min(f32::MAX as f64),
        (ch_advance * 0.5).min(f32::MAX as f64),
    )
}

/// The text facts shared by intrinsic sizing and final line construction.
/// This is computed on demand rather than stored beside each glyph, keeping
/// token storage independent of these derived text facts.
#[derive(Clone, Copy)]
pub(in crate::inline) struct CanonicalTextUnit {
    pub(crate) character: char,
    pub(crate) break_kind: BreakKind,
    pub(in crate::inline) unicode_break_before: bool,
    pub(crate) natural_advance: f64,
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(in crate::inline) fn canonical_text_unit_from_facts(
    inputs: &LayoutReadContext<'_>,
    glyph_idx: u32,
    box_idx: usize,
    run_end: u32,
    character: char,
    glyph_advance: f32,
    font_size: f32,
    letter_spacing: f64,
    word_spacing: f64,
    word_spacing_factor: f64,
    white_space: WhiteSpace,
) -> CanonicalTextUnit {
    let wrap_override = inputs.text.whitespace_wrap_before(glyph_idx as usize);
    let break_kind = if is_mandatory_line_break(character) {
        BreakKind::Hard
    } else {
        match wrap_override {
            WhitespaceWrapOverride::Style => break_kind(character, white_space),
            WhitespaceWrapOverride::Allow => BreakKind::Soft,
            WhitespaceWrapOverride::Suppress => BreakKind::None,
            WhitespaceWrapOverride::UnicodeAllow => BreakKind::Discretionary,
            WhitespaceWrapOverride::UnicodeSuppress => BreakKind::None,
        }
    };
    let natural_advance = if suppresses_glyph(character)
        || character == '\u{200b}'
        || font_size <= 0.0
    {
        0.0
    } else {
        let advance = glyph_advance as f64
            + if glyph_advance > 0.0 && is_word_separator(character) {
                word_spacing
            } else {
                0.0
            };
        let advance = if matches!(character, ' ' | '\u{00a0}') {
            advance * word_spacing_factor
        } else {
            advance
        };
        let mut resolved_tracking = 0.0;
        let tracking = (letter_spacing != 0.0
            || (inputs.reader.has_tracking_spacing()
                && !inputs.constraints.book_optimized_text()
                && (glyph_idx + 1 >= run_end
                    || inputs.text.bidi_level(glyph_idx)
                        != inputs.text.bidi_level(glyph_idx + 1))))
            && {
                // Format controls keep source positions and shaping effects, but
                // do not create extra typographic units. Test the boundary before
                // the next visible unit, including after a ZWJ attached to this
                // character; an emoji joining sequence still has no such boundary.
                let mut next = glyph_idx as usize + 1;
                while inputs
                    .text
                    .content()
                    .character_at(next)
                    .is_some_and(|character| suppresses_glyph(character) && character != '\u{00ad}')
                {
                    next += 1;
                }
                resolved_tracking =
                    if letter_spacing == 0.0 && inputs.constraints.book_optimized_text() {
                        0.0
                    } else if next < run_end as usize
                        && inputs.text.bidi_level(glyph_idx) == inputs.text.bidi_level(next as u32)
                    {
                        letter_spacing
                    } else {
                        inputs.reader.tracking_spacing_after(glyph_idx, box_idx)
                    };
                resolved_tracking != 0.0
                    && inputs.text.is_cluster_boundary(next)
                    && tracking_boundary_allowed(
                        character,
                        inputs.text.content().character_at(next).unwrap_or('\0'),
                    )
            };
        advance + if tracking { resolved_tracking } else { 0.0 }
    };
    CanonicalTextUnit {
        character,
        break_kind,
        unicode_break_before: wrap_override == WhitespaceWrapOverride::UnicodeAllow,
        // Token storage uses f32 widths. Intrinsic sizing must accumulate
        // those same values, otherwise a shrink-to-fit line can overflow its
        // own max-content width solely through per-token rounding.
        natural_advance: f64::from(natural_advance as f32),
    }
}

/// Classifies a glyph as a hard break, soft break, or non-break.
///
/// This replaces the old pair of `breakable` and `hard_break` booleans.
pub(crate) fn break_kind(character: char, white_space: WhiteSpace) -> BreakKind {
    if is_mandatory_line_break(character) || (character == '\n' && white_space.preserves_newlines())
    {
        return BreakKind::Hard;
    }
    if character == '\u{00A0}' || !white_space.allows_wrap() {
        return BreakKind::None;
    }
    if white_space.preserves_spaces() && matches!(character, ' ' | '\t') {
        return BreakKind::None;
    }
    match character {
        ' ' | '\t' | '\n' => BreakKind::Soft,
        '\u{200b}' => BreakKind::Discretionary,
        _ => BreakKind::None,
    }
}
