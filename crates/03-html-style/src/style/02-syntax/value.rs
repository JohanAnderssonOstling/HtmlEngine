//! Native declaration and numeric parsing shared by style conversion.
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::Parse;

pub(crate) fn parsed_property<'i>(id: PropertyId<'i>, value: &'i str) -> Option<Property<'i>> {
    Property::parse_string(id, value, ParserOptions::default()).ok()
}

pub(crate) fn parse_integer(source: &str, minimum: u32) -> Option<u32> {
    let value = u32::try_from(i32::parse_string(source).ok()?).ok()?;
    (value >= minimum).then_some(value)
}

pub(crate) fn css_wide_keyword(value: &str) -> bool {
    lightningcss::properties::CSSWideKeyword::parse_string(value).is_ok()
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
