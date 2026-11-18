//! Authored punctuation hanging shares the breaker's and painter's geometry.
use crate::LayoutEngine;
use crate::inline::tokens::{BreakKind, InlineTokenKind};
use crate::inline::{ParagraphLayout, TokenSpan};
use unicode_categories::UnicodeCategories;

pub(super) fn first(
    engine: &LayoutEngine<'_, '_>,
    tokens: TokenSpan<'_>,
    paragraph: ParagraphLayout,
) -> f64 {
    if !paragraph.hanging_punctuation.first || !paragraph.indent.first_line {
        return 0.0;
    }
    edge(engine, tokens, 0..tokens.len(), |character| {
        character.is_punctuation_open() || character.is_punctuation_initial_quote()
    })
    .map_or(0.0, |(_, width)| width)
}

pub(super) fn last(
    engine: &LayoutEngine<'_, '_>,
    tokens: TokenSpan<'_>,
    paragraph: ParagraphLayout,
) -> Option<(usize, f64)> {
    if !paragraph.hanging_punctuation.last {
        return None;
    }
    edge(engine, tokens, (0..tokens.len()).rev(), |character| {
        character.is_punctuation_close() || character.is_punctuation_final_quote()
    })
}

fn edge(
    engine: &LayoutEngine<'_, '_>,
    tokens: TokenSpan<'_>,
    indices: impl Iterator<Item = usize>,
    punctuation: impl Fn(char) -> bool,
) -> Option<(usize, f64)> {
    for index in indices {
        let token = tokens.metadata_at(index);
        let width = token.width(tokens.stored_width_at(index));
        match token.kind() {
            InlineTokenKind::Glyph { glyph_idx } => {
                if width == 0.0 || token.break_kind() == BreakKind::Soft {
                    continue;
                }
                let character = engine.read.text.character(glyph_idx);
                return (punctuation(character) || matches!(character, '\'' | '"'))
                    .then_some((index, width));
            }
            InlineTokenKind::InlineBoundary { .. }
            | InlineTokenKind::ZeroWidthBreak
            | InlineTokenKind::FloatAnchor { .. }
            | InlineTokenKind::AbsoluteAnchor { .. }
                if width == 0.0 => {}
            _ => break,
        }
    }
    None
}
