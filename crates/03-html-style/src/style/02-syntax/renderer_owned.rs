//! Conversion of Lightning CSS raw tokens into renderer-owned style values.

use super::value::{significant_tokens, single_ident_keyword, token_ident};
use crate::style::syntax::property_id::RendererPropertyId;
use cssparser::match_ignore_ascii_case;
use html_style_model::{
    BorderCollapseMode, BreakBetween, BreakInside, CaptionSide, Clear, EmptyCellsMode, Float,
    ObjectFit, TableLayoutMode, TextBoxEdge, TextBoxOverEdge, TextBoxTrim, TextBoxUnderEdge,
    TextDirection,
};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
#[cfg(test)]
use lightningcss::properties::{Property, PropertyId};
#[cfg(test)]
use lightningcss::stylesheet::ParserOptions;

// Fit and edge values have at most two keywords; text-box may add a trim keyword.
fn words<'a, 'i, const N: usize>(tokens: &'a TokenList<'i>) -> Option<([&'a str; N], usize)> {
    let mut words = [""; N];
    let mut len = 0;
    for token in significant_tokens(tokens) {
        *words.get_mut(len)? = token_ident(token)?;
        len += 1;
    }
    Some((words, len))
}

pub(crate) fn float(tokens: &TokenList<'_>, direction: TextDirection) -> Option<Float> {
    Some(match_ignore_ascii_case! { single_ident_keyword(tokens)?,
        "left" => Float::Left,
        "right" => Float::Right,
        "none" => Float::None,
        "inline-start" => if direction == TextDirection::Ltr { Float::Left } else { Float::Right },
        "inline-end" => if direction == TextDirection::Ltr { Float::Right } else { Float::Left },
        _ => return None,
    })
}

pub(crate) fn clear(tokens: &TokenList<'_>, direction: TextDirection) -> Option<Clear> {
    Some(match_ignore_ascii_case! { single_ident_keyword(tokens)?,
        "left" => Clear::Left,
        "right" => Clear::Right,
        "both" => Clear::Both,
        "none" => Clear::None,
        "inline-start" => if direction == TextDirection::Ltr { Clear::Left } else { Clear::Right },
        "inline-end" => if direction == TextDirection::Ltr { Clear::Right } else { Clear::Left },
        _ => return None,
    })
}

pub(crate) fn table_layout(tokens: &TokenList<'_>) -> Option<TableLayoutMode> {
    match single_ident_keyword(tokens)? {
        value if value.eq_ignore_ascii_case("auto") => Some(TableLayoutMode::Auto),
        value if value.eq_ignore_ascii_case("fixed") => Some(TableLayoutMode::Fixed),
        _ => None,
    }
}

pub(crate) fn border_collapse(tokens: &TokenList<'_>) -> Option<BorderCollapseMode> {
    match single_ident_keyword(tokens)? {
        value if value.eq_ignore_ascii_case("collapse") => Some(BorderCollapseMode::Collapse),
        value if value.eq_ignore_ascii_case("separate") => Some(BorderCollapseMode::Separate),
        _ => None,
    }
}

pub(crate) fn caption_side(tokens: &TokenList<'_>) -> Option<CaptionSide> {
    match single_ident_keyword(tokens)? {
        value if value.eq_ignore_ascii_case("top") => Some(CaptionSide::Top),
        value if value.eq_ignore_ascii_case("bottom") => Some(CaptionSide::Bottom),
        _ => None,
    }
}

pub(crate) fn empty_cells(tokens: &TokenList<'_>) -> Option<EmptyCellsMode> {
    match single_ident_keyword(tokens)? {
        value if value.eq_ignore_ascii_case("show") => Some(EmptyCellsMode::Show),
        value if value.eq_ignore_ascii_case("hide") => Some(EmptyCellsMode::Hide),
        _ => None,
    }
}

pub(crate) fn text_box_trim(tokens: &TokenList<'_>) -> Option<TextBoxTrim> {
    parse_text_box_trim(single_ident_keyword(tokens)?)
}

fn parse_text_box_trim(value: &str) -> Option<TextBoxTrim> {
    match value {
        value if value.eq_ignore_ascii_case("none") => Some(TextBoxTrim::None),
        value if value.eq_ignore_ascii_case("trim-start") => Some(TextBoxTrim::Start),
        value if value.eq_ignore_ascii_case("trim-end") => Some(TextBoxTrim::End),
        value if value.eq_ignore_ascii_case("trim-both") => Some(TextBoxTrim::Both),
        _ => None,
    }
}

pub(crate) fn text_box_edge(tokens: &TokenList<'_>) -> Option<TextBoxEdge> {
    let (words, len) = words::<2>(tokens)?;
    parse_text_box_edge(&words[..len])
}

fn parse_text_box_edge(words: &[&str]) -> Option<TextBoxEdge> {
    let (over, under) = match words {
        [value] if value.eq_ignore_ascii_case("auto") => return Some(TextBoxEdge::default()),
        [value] if value.eq_ignore_ascii_case("alphabetic") => {
            (TextBoxOverEdge::Text, TextBoxUnderEdge::Alphabetic)
        }
        [over] => (parse_over_edge(over)?, TextBoxUnderEdge::Text),
        [over, under] => (parse_over_edge(over)?, parse_under_edge(under)?),
        _ => return None,
    };
    Some(TextBoxEdge { over, under })
}

fn parse_over_edge(value: &str) -> Option<TextBoxOverEdge> {
    Some(match_ignore_ascii_case! { value,
        "text" | "ideographic" | "ideographic-ink" => TextBoxOverEdge::Text,
        "cap" => TextBoxOverEdge::Cap,
        "ex" => TextBoxOverEdge::Ex,
        _ => return None,
    })
}

fn parse_under_edge(value: &str) -> Option<TextBoxUnderEdge> {
    Some(match_ignore_ascii_case! { value,
        "text" | "ideographic" | "ideographic-ink" => TextBoxUnderEdge::Text,
        "alphabetic" => TextBoxUnderEdge::Alphabetic,
        _ => return None,
    })
}

pub(crate) fn text_box(tokens: &TokenList<'_>) -> Option<(TextBoxTrim, TextBoxEdge)> {
    let (words, len) = words::<3>(tokens)?;
    let words = &words[..len];
    if matches!(words, [value] if value.eq_ignore_ascii_case("normal")) {
        return Some((TextBoxTrim::None, TextBoxEdge::default()));
    }

    let (trim, edges) =
        if let Some(trim) = words.first().and_then(|value| parse_text_box_trim(value)) {
            if words.len() == 1 {
                return Some((trim, TextBoxEdge::default()));
            }
            (trim, &words[1..])
        } else if let Some(trim) = words.last().and_then(|value| parse_text_box_trim(value)) {
            (trim, &words[..words.len() - 1])
        } else {
            (TextBoxTrim::Both, words)
        };
    parse_text_box_edge(edges).map(|edge| (trim, edge))
}

pub(crate) fn object_fit(tokens: &TokenList<'_>) -> Option<ObjectFit> {
    let (words, len) = words::<2>(tokens)?;
    match &words[..len] {
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
    let value = single_ident_keyword(tokens)?;
    if matches!(
        id,
        RendererPropertyId::PageBreakBefore | RendererPropertyId::PageBreakAfter
    ) && !["auto", "avoid", "always", "left", "right", "recto", "verso"]
        .iter()
        .any(|word| value.eq_ignore_ascii_case(word))
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
    let value = single_ident_keyword(tokens)?;
    if id == RendererPropertyId::PageBreakInside
        && !["auto", "avoid"]
            .iter()
            .any(|word| value.eq_ignore_ascii_case(word))
    {
        return None;
    }
    match_ignore_ascii_case! { value,
        "auto" | "avoid-region" => Some(BreakInside::Auto),
        "avoid" | "avoid-page" | "avoid-column" => Some(BreakInside::Avoid),
        _ => None,
    }
}

/// Whether the existing raw-token conversion can represent this value.
pub(crate) fn tokens_are_supported(id: RendererPropertyId, tokens: &TokenList<'_>) -> bool {
    match id {
        RendererPropertyId::Float => float(tokens, TextDirection::Ltr).is_some(),
        RendererPropertyId::Clear => clear(tokens, TextDirection::Ltr).is_some(),
        RendererPropertyId::ObjectFit => object_fit(tokens).is_some(),
        RendererPropertyId::TableLayout => table_layout(tokens).is_some(),
        RendererPropertyId::BorderCollapse => border_collapse(tokens).is_some(),
        RendererPropertyId::CaptionSide => caption_side(tokens).is_some(),
        RendererPropertyId::EmptyCells => empty_cells(tokens).is_some(),
        RendererPropertyId::TextBox => text_box(tokens).is_some(),
        RendererPropertyId::TextBoxTrim => text_box_trim(tokens).is_some(),
        RendererPropertyId::TextBoxEdge => text_box_edge(tokens).is_some(),
        RendererPropertyId::FontFeatureSettings => {
            super::font_features::parse_settings(tokens).is_some()
        }
        RendererPropertyId::FontKerning
        | RendererPropertyId::FontVariantNumeric
        | RendererPropertyId::FontVariantLigatures => {
            super::font_features::parse_tokens(id, tokens).is_some()
        }
        RendererPropertyId::BreakBefore
        | RendererPropertyId::BreakAfter
        | RendererPropertyId::PageBreakBefore
        | RendererPropertyId::PageBreakAfter => break_between(id, tokens).is_some(),
        RendererPropertyId::BreakInside | RendererPropertyId::PageBreakInside => {
            break_inside(id, tokens).is_some()
        }
        RendererPropertyId::Widows | RendererPropertyId::Orphans => {
            let mut tokens = significant_tokens(tokens);
            matches!(tokens.next(), Some(TokenOrValue::Token(Token::Number { int_value: Some(value), .. })) if *value > 0)
                && tokens.next().is_none()
        }
        RendererPropertyId::Contain => super::contain::parse_tokens(tokens).is_some(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::syntax::value::css_wide_keyword;

    fn valid(name: &str, value: &str) -> bool {
        let tokens =
            match Property::parse_string(PropertyId::from(name), value, ParserOptions::default())
                .unwrap()
            {
                Property::Custom(value) => value.value,
                _ => panic!("raw renderer property"),
            };
        match name {
            "table-layout" => table_layout(&tokens).is_some(),
            "border-collapse" => border_collapse(&tokens).is_some(),
            "caption-side" => caption_side(&tokens).is_some(),
            "empty-cells" => empty_cells(&tokens).is_some(),
            "text-box" => text_box(&tokens).is_some(),
            "text-box-trim" => text_box_trim(&tokens).is_some(),
            "text-box-edge" => text_box_edge(&tokens).is_some(),
            _ => false,
        }
    }

    #[test]
    fn bounded_values_keep_native_keyword_and_integer_rules_without_scratch_allocations() {
        use crate::allocation_test_support::count_allocations;
        use RendererPropertyId::{ObjectFit, Orphans, TextBox, TextBoxEdge, Widows};
        use lightningcss::traits::ParseWithOptions;

        for (id, css, expected) in [
            (ObjectFit, "cover", true),
            (ObjectFit, "cover scale-down", true),
            (ObjectFit, "SCALE-DOWN /**/contain", true),
            (TextBoxEdge, "alphabetic", true),
            (TextBoxEdge, "\\63 ap ALPHABETIC", true),
            (TextBoxEdge, "ideographic-ink text", true),
            (TextBox, "normal", true),
            (TextBox, "none", true),
            (TextBox, "trim-start cap alphabetic", true),
            (TextBox, "cap alphabetic TrIm-EnD", true),
            (Widows, "+3/**/", true),
            (Orphans, "2147483647", true),
            (ObjectFit, "cover scale-down extra", false),
            (ObjectFit, "'cover'", false),
            (TextBoxEdge, "cap alphabetic extra", false),
            (TextBoxEdge, "", false),
            (TextBox, "trim-start cap alphabetic extra", false),
            (TextBox, "cap none alphabetic", false),
            (TextBox, "trim-start 2", false),
            (Widows, "1e0", false),
            (Widows, "0", false),
            (Orphans, "-1", false),
            (Orphans, "3 4", false),
        ] {
            let tokens =
                TokenList::parse_string_with_options(css, ParserOptions::default()).unwrap();
            let (supported, allocations) = count_allocations(|| tokens_are_supported(id, &tokens));
            assert_eq!(supported, expected, "{id:?}: {css}");
            assert_eq!(allocations, 0, "{id:?}: {css}");
        }
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
