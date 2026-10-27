//! Intrinsic-size calculation syntax. Layout does not yet resolve calc-size().

use super::value::{length_percentage_is_valid, literal_numeric_tokens_are_valid};
use super::{numeric_functions, random_math, sign, tree_counting, urls};
use cssparser::{Delimiter, ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::properties::size::Size;
use lightningcss::traits::Parse;
use lightningcss::values::length::LengthPercentage;

pub(crate) fn is_calc_size(name: &str) -> bool {
    name.eq_ignore_ascii_case("calc-size")
}

pub(crate) fn is_size_property(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::Width
            | PropertyId::Height
            | PropertyId::BlockSize
            | PropertyId::InlineSize
            | PropertyId::MinWidth
            | PropertyId::MinHeight
            | PropertyId::MinBlockSize
            | PropertyId::MinInlineSize
            | PropertyId::MaxWidth
            | PropertyId::MaxHeight
            | PropertyId::MaxBlockSize
            | PropertyId::MaxInlineSize
            | PropertyId::FlexBasis(_)
    )
}

pub(crate) fn contains_calc_size(source: &str) -> bool {
    if !source.contains('\\')
        && !source
            .as_bytes()
            .windows(9)
            .any(|part| part.eq_ignore_ascii_case(b"calc-size"))
    {
        return false;
    }
    let mut found = false;
    let _ = numeric_functions::replace(source, |name, _| {
        found |= is_calc_size(name);
        Ok(None)
    });
    found
}

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    if !is_size_property(id) {
        return None;
    }
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    // calc-size() is a distinct sizing value, never a length-valued math
    // function. Leave other sizes to their ordinary grammar paths.
    if parser
        .try_parse(|parser| parser.expect_function_matching("calc-size"))
        .is_err()
    {
        return None;
    }
    let mut input = ParserInput::new(value);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| calc_size(parser, id, 0))
            .is_ok(),
    )
}

fn calc_size<'i>(
    parser: &mut Parser<'i, '_>,
    id: &PropertyId<'_>,
    depth: usize,
) -> Result<(), ParseError<'i, ()>> {
    if depth >= 64 {
        return Err(parser.new_custom_error(()));
    }
    parser.expect_function_matching("calc-size")?;
    parser.parse_nested_block(|parser| {
        let basis = parser.parse_until_before(Delimiter::Comma, component_values)?;
        parser.expect_comma()?;
        let calculation = component_values(parser)?;
        let mut input = ParserInput::new(basis);
        let mut basis_parser = Parser::new(&mut input);
        let any = basis_parser.expect_ident_matching("any").is_ok() && basis_parser.is_exhausted();
        if !any && !valid_basis(id, basis, depth + 1) {
            return Err(parser.new_custom_error(()));
        }
        if valid_calculation(calculation, !any) {
            Ok(())
        } else {
            Err(parser.new_custom_error(()))
        }
    })
}

fn component_values<'i>(parser: &mut Parser<'i, '_>) -> Result<&'i str, ParseError<'i, ()>> {
    let start = parser.position();
    urls::consume(parser, 0, false)?;
    Ok(parser.slice_from(start))
}

fn valid_basis(id: &PropertyId<'_>, source: &str, depth: usize) -> bool {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    if parser
        .try_parse(|parser| parser.expect_function_matching("calc-size"))
        .is_ok()
    {
        let mut input = ParserInput::new(source);
        return Parser::new(&mut input)
            .parse_entirely(|parser| calc_size(parser, id, depth))
            .is_ok();
    }
    if parser
        .try_parse(|parser| parser.expect_function_matching("fit-content"))
        .is_ok()
    {
        let valid = parser
            .parse_nested_block(|parser| {
                let source = component_values(parser)?;
                let source = lower_math(source, false).map_err(|_| parser.new_custom_error(()))?;
                if LengthPercentage::parse_string(&source)
                    .is_ok_and(|value| length_percentage_is_valid(&value))
                    && literal_numeric_tokens_are_valid(&source, true, false)
                {
                    Ok(())
                } else {
                    Err(parser.new_custom_error::<(), ()>(()))
                }
            })
            .is_ok();
        return valid && parser.is_exhausted();
    }
    let maximum = matches!(
        id,
        PropertyId::MaxWidth
            | PropertyId::MaxHeight
            | PropertyId::MaxBlockSize
            | PropertyId::MaxInlineSize
    );
    match Size::parse_string(source) {
        Ok(Size::Auto) => return !maximum,
        Ok(
            Size::MinContent(_)
            | Size::MaxContent(_)
            | Size::FitContent(_)
            | Size::Stretch(_)
            | Size::Contain,
        ) => return true,
        _ => {}
    }
    if matches!(id, PropertyId::FlexBasis(_)) {
        if parser.expect_ident_matching("content").is_ok() && parser.is_exhausted() {
            return true;
        }
    }
    valid_calculation(source, false)
}

fn lower_math(source: &str, allow_size: bool) -> Result<String, ()> {
    let source = numeric_functions::replace_identifiers(source, |name| {
        if name.eq_ignore_ascii_case("size") {
            if allow_size {
                // A relative length preserves dimensional type without
                // resolving an intrinsic size or committing to a used value.
                Ok(Some("1em".to_owned()))
            } else {
                Err(())
            }
        } else {
            Ok(None)
        }
    })?;
    let source = tree_counting::lower(&source, |_| 1)?;
    let source = random_math::for_syntax(&source)?;
    Ok(sign::lower(&source, |_| Some(1.0))?.into_owned())
}

fn valid_calculation(source: &str, allow_size: bool) -> bool {
    let Ok(source) = lower_math(source, allow_size) else {
        return false;
    };
    // Both arguments contain a calc-sum, rather than an ordinary length:
    // unitless zero and bare numbers therefore keep their number type.
    LengthPercentage::parse_string(&format!("calc({source})"))
        .is_ok_and(|value| length_percentage_is_valid(&value))
}
