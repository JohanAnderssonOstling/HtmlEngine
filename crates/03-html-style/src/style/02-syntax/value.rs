//! Native declaration and numeric parsing shared by style conversion.
use super::property_id::canonical_property_id;
use cssparser::{CowRcStr, Parser, ParserInput};
use lightningcss::printer::PrinterOptions;
use lightningcss::properties::custom::{CustomProperty, Token, TokenList, TokenOrValue};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::Parse;

pub(crate) fn parsed_property<'i>(id: PropertyId<'i>, value: &'i str) -> Option<Property<'i>> {
    let id = canonical_property_id(id);
    let mut input = ParserInput::new(value);
    // Property::parse_string can leave trailing tokens after the `all` value.
    Parser::new(&mut input)
        .parse_entirely(|parser| Property::parse(id, parser, &ParserOptions::default()))
        .ok()
}

pub(crate) fn parse_integer(source: &str, minimum: u32) -> Option<u32> {
    let value = u32::try_from(i32::parse_string(source).ok()?).ok()?;
    (value >= minimum).then_some(value)
}

pub(crate) fn token_list_to_css_string(tokens: &TokenList<'_>) -> Option<String> {
    // A custom property preserves units on zero lengths during serialization.
    Property::Custom(CustomProperty {
        name: CowRcStr::from("--html-value").into(),
        value: tokens.clone(),
    })
    .value_to_css_string(PrinterOptions::default())
    .ok()
}

pub(crate) fn single_ident_keyword<'tokens, 'css>(
    tokens: &'tokens TokenList<'css>,
) -> Option<&'tokens str> {
    let mut tokens = significant_tokens(tokens);
    let first = token_ident(tokens.next()?)?;
    tokens.next().is_none().then_some(first)
}

pub(crate) fn significant_tokens<'a, 'css>(
    tokens: &'a TokenList<'css>,
) -> impl Iterator<Item = &'a TokenOrValue<'css>> + Clone {
    tokens.0.iter().filter(|token| {
        !matches!(
            token,
            TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_))
        )
    })
}

pub(crate) fn token_ident<'a, 'css>(token: &'a TokenOrValue<'css>) -> Option<&'a str> {
    match token {
        TokenOrValue::Token(Token::Ident(value)) => Some(value.as_ref()),
        _ => None,
    }
}
