use crate::style::syntax::tree_counting;
use crate::style::syntax::value::{
    length_percentage_is_valid, literal_numeric_tokens_are_valid, split_important,
};
use cssparser::{Parser, ParserInput};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::values::length::LengthPercentage;

pub const LETTER_SPACING_MARKER: &str = "--html-renderer-letter-spacing";
pub const WORD_SPACING_MARKER: &str = "--html-renderer-word-spacing";

#[derive(Clone, Debug)]
pub enum ParsedSpacing {
    Normal,
    Value(LengthPercentage),
}

pub fn parse(value: &str) -> Option<ParsedSpacing> {
    let value = value.trim();
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    if parser.expect_ident_matching("normal").is_ok() && parser.is_exhausted() {
        return Some(ParsedSpacing::Normal);
    }
    if !literal_numeric_tokens_are_valid(value, false, false) {
        return None;
    }
    match Property::parse_string(PropertyId::TextIndent, value, ParserOptions::default()).ok()? {
        Property::TextIndent(indent)
            if !indent.hanging
                && !indent.each_line
                && length_percentage_is_valid(&indent.value) =>
        {
            Some(ParsedSpacing::Value(indent.value))
        }
        _ => None,
    }
}

pub fn normalized_declaration(id: &PropertyId<'_>, raw: &str) -> Option<String> {
    let marker = match id {
        PropertyId::LetterSpacing => LETTER_SPACING_MARKER,
        PropertyId::WordSpacing => WORD_SPACING_MARKER,
        _ => return None,
    };
    let property_name = id.name();
    let trimmed = raw.trim();
    let (value, important) = split_important(trimmed);
    let value = value.trim();
    let important_suffix = if important { " !important" } else { "" };

    let css_wide = value.to_ascii_lowercase();
    if matches!(css_wide.as_str(), "inherit" | "unset") {
        return Some(format!(
            "{property_name}: {css_wide}{important_suffix}; {marker}: {css_wide}{important_suffix}"
        ));
    }
    if matches!(css_wide.as_str(), "initial" | "revert" | "revert-layer") {
        return Some(format!(
            "{property_name}: {css_wide}{important_suffix}; {marker}: initial{important_suffix}"
        ));
    }

    let tree_value = tree_counting::lower(value, |_| 1).ok()?;
    let random_value = crate::style::syntax::random_math::for_syntax(&tree_value).ok()?;
    parse(&random_value)?;
    let lightning_accepts = matches!(tree_value, std::borrow::Cow::Borrowed(_))
        && matches!(random_value, std::borrow::Cow::Borrowed(_))
        && Property::parse_string(id.clone(), value, ParserOptions::default())
            .is_ok_and(|property| !matches!(property, Property::Unparsed(_)));
    if lightning_accepts {
        // Native values must clear an inherited/internal percentage marker.
        return Some(format!(
            "{property_name}: {value}{important_suffix}; {marker}: initial{important_suffix}"
        ));
    }

    // Lightning CSS does not yet accept the percentage extension. Preserve it
    // in an internal cascaded value while giving Lightning a harmless fallback.
    Some(format!(
        "{property_name}: 0px{important_suffix}; {marker}: {value}{important_suffix}"
    ))
}

#[cfg(test)]
mod tests {
    use super::{ParsedSpacing, normalized_declaration, parse};
    use lightningcss::properties::PropertyId;

    #[test]
    fn accepts_css_text_four_length_percentages_but_rejects_bare_numbers() {
        assert!(matches!(parse("120%"), Some(ParsedSpacing::Value(_))));
        assert!(matches!(
            parse("calc(2ch - 30%)"),
            Some(ParsedSpacing::Value(_))
        ));
        assert!(parse("20").is_none());
        assert!(parse("10% 10px").is_none());
    }

    #[test]
    fn only_dependency_gap_values_keep_an_internal_marker() {
        assert_eq!(
            normalized_declaration(&PropertyId::LetterSpacing, "120% !important").as_deref(),
            Some("letter-spacing: 0px !important; --html-renderer-letter-spacing: 120% !important")
        );
        assert_eq!(
            normalized_declaration(&PropertyId::WordSpacing, "2em").as_deref(),
            Some("word-spacing: 2em; --html-renderer-word-spacing: initial")
        );
    }
}
