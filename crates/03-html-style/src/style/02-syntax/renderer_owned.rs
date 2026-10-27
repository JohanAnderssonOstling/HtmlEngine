//! Syntax for properties Lightning CSS preserves as raw declarations.
//!
//! These parsers are shared by capability reporting and computed-value
//! conversion so the renderer cannot claim a wider grammar than it applies.

use super::font_features;
use super::property_id::RendererPropertyId;
use super::value::parse_integer;
use cssparser::match_ignore_ascii_case;
use html_style_model::{
    BorderCollapseMode, BreakBetween, BreakInside, CaptionSide, EmptyCellsMode, ObjectFit,
    TableLayoutMode, TextBoxEdge, TextBoxOverEdge, TextBoxTrim, TextBoxUnderEdge,
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

pub(crate) fn object_fit(tokens: &TokenList<'_>) -> Option<ObjectFit> {
    let words = words(tokens)?;
    match words.as_slice() {
        [word] => match_ignore_ascii_case! { word,
            "fill" => Some(ObjectFit::Fill),
            "contain" => Some(ObjectFit::Contain),
            "cover" => Some(ObjectFit::Cover),
            "none" => Some(ObjectFit::None),
            "scale-down" => Some(ObjectFit::ScaleDown),
            _ => None,
        },
        [a, b] => {
            let other = if a.eq_ignore_ascii_case("scale-down") {
                b
            } else if b.eq_ignore_ascii_case("scale-down") {
                a
            } else {
                return None;
            };
            match_ignore_ascii_case! { other,
                "contain" => Some(ObjectFit::ScaleDown),
                "cover" => Some(ObjectFit::CoverScaleDown),
                _ => None,
            }
        }
        _ => None,
    }
}

pub(crate) fn break_between(
    id: RendererPropertyId,
    tokens: &TokenList<'_>,
) -> Option<BreakBetween> {
    let value = one_word(tokens)?;
    if matches!(
        id,
        RendererPropertyId::PageBreakBefore | RendererPropertyId::PageBreakAfter
    ) && !["auto", "avoid", "always", "left", "right", "recto", "verso"]
        .iter()
        .any(|word| keyword(value, word))
    {
        return None;
    }
    match_ignore_ascii_case! { value,
        "auto" => Some(BreakBetween::Auto),
        "avoid" | "avoid-page" | "avoid-column" => Some(BreakBetween::Avoid),
        "column" => Some(BreakBetween::Column),
        // Facing-page parity is not modeled; retain the forced page break.
        "always" | "all" | "page" | "left" | "right" | "recto" | "verso" => Some(BreakBetween::Page),
        // Region controls have no effect in page/column fragmentation contexts.
        "region" | "avoid-region" => Some(BreakBetween::Auto),
        _ => None,
    }
}

pub(crate) fn break_inside(id: RendererPropertyId, tokens: &TokenList<'_>) -> Option<BreakInside> {
    let value = one_word(tokens)?;
    if id == RendererPropertyId::PageBreakInside
        && !["auto", "avoid"].iter().any(|word| keyword(value, word))
    {
        return None;
    }
    match_ignore_ascii_case! { value,
        "auto" | "avoid-region" => Some(BreakInside::Auto),
        "avoid" | "avoid-page" | "avoid-column" => Some(BreakInside::Avoid),
        _ => None,
    }
}

pub(crate) fn property_value_is_valid(id: RendererPropertyId, value: &str) -> Option<bool> {
    if id == RendererPropertyId::FontFeatureSettings {
        return Some(font_features::parse_settings(value).is_some());
    }
    if matches!(id, RendererPropertyId::Widows | RendererPropertyId::Orphans) {
        return Some(parse_integer(value, 1).is_some());
    }
    let tokens = parsed_tokens(value)?;
    Some(match id {
        RendererPropertyId::BreakBefore
        | RendererPropertyId::BreakAfter
        | RendererPropertyId::PageBreakBefore
        | RendererPropertyId::PageBreakAfter => break_between(id, &tokens).is_some(),
        RendererPropertyId::BreakInside | RendererPropertyId::PageBreakInside => {
            break_inside(id, &tokens).is_some()
        }
        RendererPropertyId::FontKerning
        | RendererPropertyId::FontVariantNumeric
        | RendererPropertyId::FontVariantLigatures => {
            font_features::parse_tokens(id, &tokens).is_some()
        }
        RendererPropertyId::Float => one_word(&tokens).is_some_and(|word| {
            ["left", "right", "none", "inline-start", "inline-end"]
                .iter()
                .any(|value| keyword(word, value))
        }),
        RendererPropertyId::Clear => one_word(&tokens).is_some_and(|word| {
            [
                "left",
                "right",
                "both",
                "none",
                "inline-start",
                "inline-end",
            ]
            .iter()
            .any(|value| keyword(word, value))
        }),
        RendererPropertyId::ObjectFit => object_fit(&tokens).is_some(),
        RendererPropertyId::TableLayout => table_layout(&tokens).is_some(),
        RendererPropertyId::BorderCollapse => border_collapse(&tokens).is_some(),
        RendererPropertyId::CaptionSide => caption_side(&tokens).is_some(),
        RendererPropertyId::EmptyCells => empty_cells(&tokens).is_some(),
        RendererPropertyId::TextBoxTrim => text_box_trim(&tokens).is_some(),
        RendererPropertyId::TextBoxEdge => text_box_edge(&tokens).is_some(),
        RendererPropertyId::TextBox => text_box(&tokens).is_some(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::syntax::value::css_wide_keyword;

    fn valid(name: &str, value: &str) -> bool {
        property_value_is_valid(RendererPropertyId::from_name(name).unwrap(), value).unwrap()
    }

    #[test]
    fn validates_table_keywords() {
        assert!(valid("table-layout", "fixed"));
        assert!(valid("border-collapse", "collapse"));
        assert!(valid("caption-side", "bottom"));
        assert!(valid("empty-cells", "hide"));
        assert!(!valid("table-layout", "auto fixed"));
        assert!(!valid("caption-side", "left"));
        assert!(css_wide_keyword("\\69nitial"));
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
