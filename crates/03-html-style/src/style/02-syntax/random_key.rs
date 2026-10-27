//! Shared grammar for random() and random-item() cache keys.

use super::value::{NumberValue, parse_number};
use cssparser::{Parser, ParserInput};

pub(crate) enum PropertyScope {
    None,
    Property,
    Index,
    User(String),
}

pub(crate) enum Key {
    Fixed(NumberValue),
    Cache {
        name: Option<String>,
        element: bool,
        property: PropertyScope,
    },
}

pub(crate) fn parse_key(source: &str) -> Option<Key> {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    if parser
        .try_parse(|parser| parser.expect_ident_matching("fixed"))
        .is_ok()
    {
        let value = parse_number(&source[parser.position().byte_index()..])?;
        if matches!(value, NumberValue::Number(number) if !(0.0..=1.0).contains(&number)) {
            return None;
        }
        return Some(Key::Fixed(value));
    }
    if parser
        .try_parse(|parser| {
            parser.expect_ident_matching("auto")?;
            parser.expect_exhausted()
        })
        .is_ok()
    {
        return Some(Key::Cache {
            name: None,
            element: true,
            property: PropertyScope::Index,
        });
    }
    let mut name = None;
    let mut element = false;
    let mut property = PropertyScope::None;
    let mut present = false;
    while !parser.is_exhausted() {
        let ident = parser.expect_ident().ok()?;
        if ident.starts_with("--") && name.is_none() {
            name = Some(ident.to_string());
        } else if ident.eq_ignore_ascii_case("element-scoped") && !element {
            element = true;
        } else if matches!(property, PropertyScope::None) {
            property = if ident.eq_ignore_ascii_case("property-scoped") {
                PropertyScope::Property
            } else if ident.eq_ignore_ascii_case("property-index-scoped") {
                PropertyScope::Index
            } else if ident.starts_with("ua-") {
                PropertyScope::User(ident.to_string())
            } else {
                return None;
            };
        } else {
            return None;
        }
        present = true;
    }
    present.then_some(Key::Cache {
        name,
        element,
        property,
    })
}
