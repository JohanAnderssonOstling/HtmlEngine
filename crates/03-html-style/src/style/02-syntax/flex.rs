//! Shared flex grammar for native values and dependency recovery tokens.

use super::value::{
    length_percentage_is_valid, literal_numeric_tokens_are_valid, may_have_substitutions,
    normalize_math, numeric_component, without_priority,
};
use super::{box_syntax, sign};
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::flex::FlexDirection;
use lightningcss::properties::size::Size;
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::PrinterOptions;
use lightningcss::traits::{Parse, ToCss};
use lightningcss::values::length::LengthPercentage;
use lightningcss::values::percentage::Percentage;
use static_self::IntoOwned;
use std::borrow::Cow;

/// Preserve contextual factor roles before the native stylesheet parser can
/// mistake sign(length) for a flex basis. Pending substitutions stay intact.
pub(crate) fn normalized_declaration(id: &PropertyId<'_>, source: &str) -> Option<String> {
    let PropertyId::Flex(prefix) = id else {
        return None;
    };
    if may_have_substitutions(source)
        || !matches!(sign::lower(source, |_| Some(1.0)), Ok(Cow::Owned(_)))
    {
        return None;
    }
    let value = without_priority(source);
    let priority = &source[value.len()..];
    let mut input = ParserInput::new(value);
    let parts = Parser::new(&mut input)
        .parse_entirely(|parser| -> Result<_, ParseError<'_, ()>> {
            let (mut factors, mut basis) = (None, None);
            fn component<'i>(
                parser: &mut Parser<'i, '_>,
                id: PropertyId<'_>,
            ) -> Result<&'i str, ParseError<'i, ()>> {
                let start = parser.position();
                if parser
                    .try_parse(|parser| parser.expect_ident().map(|_| ()))
                    .is_err()
                {
                    numeric_component(parser)?;
                }
                let source = parser.slice_from(start);
                if box_syntax::property_value_is_valid(&id, source) != Some(true) {
                    return Err(parser.new_custom_error(()));
                }
                Ok(source)
            }
            loop {
                if factors.is_none() {
                    if let Ok(grow) =
                        parser.try_parse(|parser| component(parser, PropertyId::FlexGrow(*prefix)))
                    {
                        factors = Some((
                            grow,
                            parser
                                .try_parse(|parser| {
                                    component(parser, PropertyId::FlexGrow(*prefix))
                                })
                                .unwrap_or("1"),
                        ));
                        continue;
                    }
                }
                if basis.is_none() {
                    if let Ok(source) =
                        parser.try_parse(|parser| component(parser, PropertyId::FlexBasis(*prefix)))
                    {
                        basis = Some(source);
                        continue;
                    }
                }
                break;
            }
            if factors.is_none() && basis.is_none() {
                return Err(parser.new_custom_error(()));
            }
            let (grow, shrink) = factors.unwrap_or(("1", "1"));
            Ok([grow, shrink, basis.unwrap_or("0%")])
        })
        .ok()?;
    let ids = [
        PropertyId::FlexGrow(*prefix),
        PropertyId::FlexShrink(*prefix),
        PropertyId::FlexBasis(*prefix),
    ];
    ids.iter()
        .zip(parts)
        .map(|(id, value)| {
            id.to_css_string(PrinterOptions::default())
                .map(|name| format!("{name}:{value}{priority};"))
        })
        .collect::<Result<String, _>>()
        .ok()
}

pub(crate) fn is_wrap_property(id: &PropertyId<'_>) -> bool {
    matches!(id, PropertyId::FlexWrap(_) | PropertyId::FlexFlow(_))
}

pub(crate) fn wrap_value_is_valid(id: &PropertyId<'_>, source: &str) -> Option<bool> {
    if !is_wrap_property(id) {
        return None;
    }
    let mut input = ParserInput::new(source);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| {
                if matches!(id, PropertyId::FlexWrap(_)) {
                    return parse_wrap(parser);
                }
                let (mut direction, mut wrap) = (false, false);
                loop {
                    if !direction && parser.try_parse(FlexDirection::parse).is_ok() {
                        direction = true;
                    } else if !wrap && parser.try_parse(parse_wrap).is_ok() {
                        wrap = true;
                    } else {
                        break;
                    }
                }
                if direction || wrap {
                    Ok(())
                } else {
                    Err(parser.new_custom_error(()))
                }
            })
            .is_ok(),
    )
}

fn parse_wrap<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("nowrap"))
        .is_ok()
    {
        return Ok(());
    }
    let (mut wrap, mut balance) = (false, false);
    loop {
        let state = parser.state();
        let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) else {
            break;
        };
        if !balance && name.eq_ignore_ascii_case("balance") {
            balance = true;
        } else if !wrap
            && (name.eq_ignore_ascii_case("wrap") || name.eq_ignore_ascii_case("wrap-reverse"))
        {
            wrap = true;
        } else {
            parser.reset(&state);
            break;
        }
    }
    if wrap || balance {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

pub(crate) enum Basis {
    Content,
    Size(Size),
}

pub(crate) struct Flex {
    pub grow: f32,
    pub shrink: f32,
    pub basis: Basis,
}

pub(crate) fn basis(source: &str) -> Option<Basis> {
    let source = normalize_math(source);
    let mut input = ParserInput::new(&source);
    Parser::new(&mut input).parse_entirely(parse_basis).ok()
}

fn parse_basis<'i>(parser: &mut Parser<'i, '_>) -> Result<Basis, ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("content"))
        .is_ok()
    {
        return Ok(Basis::Content);
    }
    let start = parser.position();
    let value = Size::parse(parser).map_err(|_| parser.new_custom_error(()))?;
    let valid = match &value {
        Size::Auto
        | Size::MinContent(_)
        | Size::MaxContent(_)
        | Size::FitContent(_)
        | Size::Stretch(_) => true,
        Size::LengthPercentage(length) => length_percentage_is_valid(length),
        // These sizing functions need additional intrinsic-size machinery.
        Size::FitContentFunction(_) | Size::Contain => false,
    };
    if !valid || !literal_numeric_tokens_are_valid(parser.slice_from(start), true, false) {
        return Err(parser.new_custom_error(()));
    }
    Ok(Basis::Size(value))
}

fn factor<'i>(parser: &mut Parser<'i, '_>) -> Result<f32, ParseError<'i, ()>> {
    let start = parser.position();
    let value = f32::parse(parser).map_err(|_| parser.new_custom_error(()))?;
    if !literal_numeric_tokens_are_valid(parser.slice_from(start), true, true) {
        return Err(parser.new_custom_error(()));
    }
    Ok(value)
}

pub(crate) fn factor_is_valid(source: &str) -> bool {
    let mut input = ParserInput::new(source);
    Parser::new(&mut input).parse_entirely(factor).is_ok()
}

pub(crate) fn parse(source: &str) -> Option<Flex> {
    let source = normalize_math(source);
    let mut input = ParserInput::new(&source);
    Parser::new(&mut input)
        .parse_entirely(|parser| -> Result<Flex, ParseError<'_, ()>> {
            if parser
                .try_parse(|parser| parser.expect_ident_matching("none"))
                .is_ok()
            {
                return Ok(Flex {
                    grow: 0.0,
                    shrink: 0.0,
                    basis: Basis::Size(Size::Auto),
                });
            }
            let mut factors = None;
            let mut basis = None;
            loop {
                // The two factors remain adjacent; a leading unitless zero is
                // a factor until both factor positions have been filled.
                if factors.is_none() {
                    if let Ok(grow) = parser.try_parse(factor) {
                        let shrink = parser.try_parse(factor).unwrap_or(1.0);
                        factors = Some((grow, shrink));
                        continue;
                    }
                }
                if basis.is_none() {
                    if let Ok(value) = parser.try_parse(parse_basis) {
                        basis = Some(value);
                        continue;
                    }
                }
                break;
            }
            if factors.is_none() && basis.is_none() {
                return Err(parser.new_custom_error(()));
            }
            let (grow, shrink) = factors.unwrap_or((1.0, 1.0));
            Ok(Flex {
                grow,
                shrink,
                basis: basis.unwrap_or(Basis::Size(Size::LengthPercentage(
                    LengthPercentage::Percentage(Percentage(0.0)),
                ))),
            })
        })
        .ok()
}

/// Materialize only a selected longhand of a recovery shorthand. Replaying
/// the entire shorthand would overwrite other longhand winners.
pub(crate) fn longhand<'a>(
    property: &Property<'_>,
    target: &PropertyId<'_>,
) -> Option<Property<'a>> {
    let Property::Unparsed(value) = property else {
        return None;
    };
    if !matches!(value.property_id, PropertyId::Flex(_)) {
        return None;
    }
    let css = property
        .value_to_css_string(PrinterOptions::default())
        .ok()?;
    let value = parse(&css)?;
    match target {
        PropertyId::FlexGrow(prefix) => Some(Property::FlexGrow(value.grow, *prefix)),
        PropertyId::FlexShrink(prefix) => Some(Property::FlexShrink(value.shrink, *prefix)),
        PropertyId::FlexBasis(_) => {
            let css = match value.basis {
                Basis::Content => "content".to_owned(),
                Basis::Size(size) => size.to_css_string(PrinterOptions::default()).ok()?,
            };
            super::value::parsed_property(target.clone().into_owned(), &css)
                .map(IntoOwned::into_owned)
        }
        _ => None,
    }
}
