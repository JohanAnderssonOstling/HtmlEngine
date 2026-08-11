//! Syntax for properties Lightning CSS preserves as raw declarations.
//!
//! These parsers are shared by capability reporting and computed-value
//! conversion so the renderer cannot claim a wider grammar than it applies.

use html_style_model::{
    BorderCollapseMode, CaptionSide, EmptyCellsMode, TableLayoutMode, TextBoxEdge, TextBoxOverEdge,
    TextBoxTrim, TextBoxUnderEdge,
};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;

fn parsed_tokens(value: &str) -> Option<TokenList<'_>> {
    match Property::parse_string(
        PropertyId::from("--html-style-value"),
        value,
        ParserOptions::default(),
    )
    .ok()?
    {
        Property::Custom(value) => Some(value.value),
        _ => None,
    }
}

fn words<'a, 'i>(tokens: &'a TokenList<'i>) -> Option<Vec<&'a str>> {
    tokens
        .0
        .iter()
        .filter(|token| {
            !matches!(
                token,
                TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_))
            )
        })
        .map(|token| match token {
            TokenOrValue::Token(Token::Ident(value)) => Some(value.as_ref()),
            _ => None,
        })
        .collect()
}

fn one_word<'a>(tokens: &'a TokenList<'_>) -> Option<&'a str> {
    let mut tokens = tokens.0.iter().filter(|token| {
        !matches!(
            token,
            TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_))
        )
    });
    let TokenOrValue::Token(Token::Ident(word)) = tokens.next()? else {
        return None;
    };
    tokens.next().is_none().then_some(word.as_ref())
}

fn keyword(value: &str, expected: &str) -> bool {
    value.eq_ignore_ascii_case(expected)
}

pub(crate) fn value_is_css_wide_keyword(value: &str) -> bool {
    let Some(tokens) = parsed_tokens(value) else {
        return false;
    };
    let Some(value) = one_word(&tokens) else {
        return false;
    };
    ["inherit", "initial", "unset", "revert", "revert-layer"]
        .iter()
        .any(|keyword| value.eq_ignore_ascii_case(keyword))
}

pub(crate) fn table_layout(tokens: &TokenList<'_>) -> Option<TableLayoutMode> {
    let value = one_word(tokens)?;
    if keyword(value, "auto") {
        Some(TableLayoutMode::Auto)
    } else if keyword(value, "fixed") {
        Some(TableLayoutMode::Fixed)
    } else {
        None
    }
}

pub(crate) fn border_collapse(tokens: &TokenList<'_>) -> Option<BorderCollapseMode> {
    let value = one_word(tokens)?;
    if keyword(value, "collapse") {
        Some(BorderCollapseMode::Collapse)
    } else if keyword(value, "separate") {
        Some(BorderCollapseMode::Separate)
    } else {
        None
    }
}

pub(crate) fn caption_side(tokens: &TokenList<'_>) -> Option<CaptionSide> {
    let value = one_word(tokens)?;
    if keyword(value, "top") {
        Some(CaptionSide::Top)
    } else if keyword(value, "bottom") {
        Some(CaptionSide::Bottom)
    } else {
        None
    }
}

pub(crate) fn empty_cells(tokens: &TokenList<'_>) -> Option<EmptyCellsMode> {
    let value = one_word(tokens)?;
    if keyword(value, "show") {
        Some(EmptyCellsMode::Show)
    } else if keyword(value, "hide") {
        Some(EmptyCellsMode::Hide)
    } else {
        None
    }
}

pub(crate) fn text_box_trim(tokens: &TokenList<'_>) -> Option<TextBoxTrim> {
    parse_text_box_trim(one_word(tokens)?)
}

fn parse_text_box_trim(value: &str) -> Option<TextBoxTrim> {
    if keyword(value, "none") {
        Some(TextBoxTrim::None)
    } else if keyword(value, "trim-start") {
        Some(TextBoxTrim::Start)
    } else if keyword(value, "trim-end") {
        Some(TextBoxTrim::End)
    } else if keyword(value, "trim-both") {
        Some(TextBoxTrim::Both)
    } else {
        None
    }
}

pub(crate) fn text_box_edge(tokens: &TokenList<'_>) -> Option<TextBoxEdge> {
    parse_text_box_edge(&words(tokens)?)
}

fn parse_text_box_edge(words: &[&str]) -> Option<TextBoxEdge> {
    if matches!(words, [value] if keyword(value, "auto")) {
        return Some(TextBoxEdge::default());
    }
    if matches!(words, [value] if keyword(value, "alphabetic")) {
        return Some(TextBoxEdge {
            over: TextBoxOverEdge::Text,
            under: TextBoxUnderEdge::Alphabetic,
        });
    }
    let over = parse_over_edge(words.first()?)?;
    let under = match words.get(1) {
        Some(value) => parse_under_edge(value)?,
        None => TextBoxUnderEdge::Text,
    };
    (words.len() <= 2).then_some(TextBoxEdge { over, under })
}

fn parse_over_edge(value: &str) -> Option<TextBoxOverEdge> {
    if ["text", "ideographic", "ideographic-ink"]
        .iter()
        .any(|keyword| value.eq_ignore_ascii_case(keyword))
    {
        Some(TextBoxOverEdge::Text)
    } else if keyword(value, "cap") {
        Some(TextBoxOverEdge::Cap)
    } else if keyword(value, "ex") {
        Some(TextBoxOverEdge::Ex)
    } else {
        None
    }
}

fn parse_under_edge(value: &str) -> Option<TextBoxUnderEdge> {
    if ["text", "ideographic", "ideographic-ink"]
        .iter()
        .any(|keyword| value.eq_ignore_ascii_case(keyword))
    {
        Some(TextBoxUnderEdge::Text)
    } else if keyword(value, "alphabetic") {
        Some(TextBoxUnderEdge::Alphabetic)
    } else {
        None
    }
}

pub(crate) fn text_box(tokens: &TokenList<'_>) -> Option<(TextBoxTrim, TextBoxEdge)> {
    let words = words(tokens)?;
    if matches!(words.as_slice(), [value] if keyword(value, "normal")) {
        return Some((TextBoxTrim::None, TextBoxEdge::default()));
    }

    if let Some(first) = words.first().and_then(|value| parse_text_box_trim(value)) {
        let edge = if words.len() == 1 {
            TextBoxEdge::default()
        } else {
            parse_text_box_edge(&words[1..])?
        };
        return Some((first, edge));
    }
    if let Some(last) = words.last().and_then(|value| parse_text_box_trim(value)) {
        let edge = parse_text_box_edge(&words[..words.len() - 1])?;
        return Some((last, edge));
    }

    parse_text_box_edge(&words).map(|edge| (TextBoxTrim::Both, edge))
}

pub(crate) fn property_value_is_valid(name: &str, value: &str) -> Option<bool> {
    let tokens = parsed_tokens(value)?;
    Some(match name {
        "table-layout" => table_layout(&tokens).is_some(),
        "border-collapse" => border_collapse(&tokens).is_some(),
        "caption-side" => caption_side(&tokens).is_some(),
        "empty-cells" => empty_cells(&tokens).is_some(),
        "text-box-trim" => text_box_trim(&tokens).is_some(),
        "text-box-edge" => text_box_edge(&tokens).is_some(),
        "text-box" => text_box(&tokens).is_some(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(name: &str, value: &str) -> bool {
        property_value_is_valid(name, value).unwrap()
    }

    #[test]
    fn validates_table_keywords() {
        assert!(valid("table-layout", "fixed"));
        assert!(valid("border-collapse", "collapse"));
        assert!(valid("caption-side", "bottom"));
        assert!(valid("empty-cells", "hide"));
        assert!(!valid("table-layout", "auto fixed"));
        assert!(!valid("caption-side", "left"));
        assert!(value_is_css_wide_keyword("\\69nitial"));
    }

    #[test]
    fn validates_text_box_longhands() {
        for value in [
            "auto",
            "alphabetic",
            "cap ideographic",
            "ideographic-ink alphabetic",
        ] {
            assert!(valid("text-box-edge", value), "{value}");
        }
        for value in ["none", "trim-start", "trim-end", "trim-both"] {
            assert!(valid("text-box-trim", value), "{value}");
        }
        assert!(!valid("text-box-edge", "text cap"));
        assert!(!valid("text-box-trim", "trim-start trim-end"));
    }

    #[test]
    fn text_box_preserves_edge_token_adjacency() {
        for value in [
            "normal",
            "none auto",
            "auto none",
            "trim-start cap alphabetic",
            "cap alphabetic trim-start",
            "text text none",
        ] {
            assert!(valid("text-box", value), "{value}");
        }
        for value in ["normal none", "cap none alphabetic", "text trim-both text"] {
            assert!(!valid("text-box", value), "{value}");
        }
    }
}
