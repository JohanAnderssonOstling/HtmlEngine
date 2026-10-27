//! Detect color values that cannot be represented by the renderer.

use super::substitution::{contains_function, is_random_math};
use cssparser::{ParseError, Parser, ParserInput, Token};
use lightningcss::properties::PropertyId;
use lightningcss::properties::custom::TokenList;

pub(crate) fn color_component_count(id: &PropertyId<'_>) -> Option<usize> {
    match id {
        PropertyId::Color
        | PropertyId::BackgroundColor
        | PropertyId::BorderTopColor
        | PropertyId::BorderBottomColor
        | PropertyId::BorderLeftColor
        | PropertyId::BorderRightColor
        | PropertyId::BorderBlockStartColor
        | PropertyId::BorderBlockEndColor
        | PropertyId::BorderInlineStartColor
        | PropertyId::BorderInlineEndColor
        | PropertyId::TextDecorationColor(_)
        | PropertyId::TextEmphasisColor(_) => Some(1),
        PropertyId::BorderColor => Some(4),
        PropertyId::BorderBlockColor | PropertyId::BorderInlineColor => Some(2),
        _ => None,
    }
}

pub(crate) fn raw_color_is_unsupported(id: &PropertyId<'_>, tokens: &TokenList<'_>) -> bool {
    color_component_count(id).is_some()
        && (contains_function(tokens, |name| {
            name.eq_ignore_ascii_case("color-mix")
                || name.eq_ignore_ascii_case("color-layers")
                || name.eq_ignore_ascii_case("contrast-color")
        }) || !contains_function(tokens, |name| {
            is_random_math(name)
                || name.eq_ignore_ascii_case("sibling-index")
                || name.eq_ignore_ascii_case("sibling-count")
        }))
}

/// Avoid the native color converter's non-finite hue panic before parsing.
/// The renderer cannot compute these values; no substitute enters the cascade.
pub(crate) fn declaration_is_unsupported(id: &PropertyId<'_>, value: &str) -> bool {
    if color_component_count(id).is_none() {
        return false;
    }
    fn non_finite(parser: &mut Parser<'_, '_>, depth: usize) -> bool {
        if depth >= 128 {
            return true;
        }
        let mut found = false;
        while let Ok(token) = parser.next() {
            match token {
                Token::Ident(name) => {
                    found |= name
                        .trim_start_matches('-')
                        .eq_ignore_ascii_case("infinity")
                        || name.eq_ignore_ascii_case("NaN")
                }
                Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock => {
                    let mut nested_found = false;
                    let _ = parser.parse_nested_block(|nested| {
                        nested_found = non_finite(nested, depth + 1);
                        Ok::<_, ParseError<'_, ()>>(())
                    });
                    found |= nested_found;
                }
                _ => {}
            }
        }
        found
    }
    let mut input = ParserInput::new(value);
    non_finite(&mut Parser::new(&mut input), 0)
}
