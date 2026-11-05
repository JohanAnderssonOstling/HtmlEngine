//! Authored punctuation hanging shares the breaker's and painter's geometry.
use super::*;
use crate::layout::LayoutEngine;
use unicode_categories::UnicodeCategories;

pub(super) fn first<S: TokenRead>(
    engine: &LayoutEngine<'_, '_>,
    tokens: S,
    paragraph: ParagraphLayout,
) -> f64 {
    if !paragraph.hanging_punctuation.first || !paragraph.indent.first_line {
        return 0.0;
    }
    for index in 0..tokens.len() {
        let token = tokens.metadata_at(index);
        match token.kind() {
            InlineTokenKind::Glyph { glyph_idx } => {
                let character = engine.text.character(glyph_idx);
                let width = token.width(tokens.stored_width_at(index));
                if width == 0.0 || token.break_kind() == BreakKind::Soft {
                    continue;
                }
                return if character.is_punctuation_open()
                    || character.is_punctuation_initial_quote()
                    || matches!(character, '\'' | '"')
                {
                    width
                } else {
                    0.0
                };
            }
            InlineTokenKind::InlineBoundary { .. }
            | InlineTokenKind::Opportunity
            | InlineTokenKind::FloatAnchor { .. }
            | InlineTokenKind::AbsoluteAnchor { .. }
                if token.width(tokens.stored_width_at(index)) == 0.0 => {}
            _ => break,
        }
    }
    0.0
}

pub(super) fn last<S: TokenRead>(
    engine: &LayoutEngine<'_, '_>,
    tokens: S,
    paragraph: ParagraphLayout,
) -> Option<(usize, f64)> {
    if !paragraph.hanging_punctuation.last {
        return None;
    }
    for index in (0..tokens.len()).rev() {
        let token = tokens.metadata_at(index);
        match token.kind() {
            InlineTokenKind::Glyph { glyph_idx } => {
                let character = engine.text.character(glyph_idx);
                let width = token.width(tokens.stored_width_at(index));
                if width == 0.0 || token.break_kind() == BreakKind::Soft {
                    continue;
                }
                return (character.is_punctuation_close()
                    || character.is_punctuation_final_quote()
                    || matches!(character, '\'' | '"'))
                .then_some((index, width));
            }
            InlineTokenKind::InlineBoundary { .. }
            | InlineTokenKind::Opportunity
            | InlineTokenKind::FloatAnchor { .. }
            | InlineTokenKind::AbsoluteAnchor { .. }
                if token.width(tokens.stored_width_at(index)) == 0.0 => {}
            _ => break,
        }
    }
    None
}
