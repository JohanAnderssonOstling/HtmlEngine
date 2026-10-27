//! Filter value grammar, separate from offscreen painting and compositing.

use super::value::{
    angle_calculation_is_valid, literal_numeric_tokens_are_valid, numeric_component, parse_number,
    parse_percentage_value,
};
use super::{numeric_functions, shadows, urls};
use cssparser::{ParseError, Parser, ParserInput, match_ignore_ascii_case};
use lightningcss::properties::PropertyId;
use lightningcss::traits::Parse;
use lightningcss::values::angle::Angle;
use lightningcss::values::calc::Calc;

pub(crate) fn supports(id: &PropertyId<'_>) -> bool {
    matches!(id, PropertyId::Filter(_) | PropertyId::BackdropFilter(_))
}

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    if !supports(id) {
        return None;
    }
    let mut input = ParserInput::new(value);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| {
                if parser
                    .try_parse(|parser| parser.expect_ident_matching("none"))
                    .is_ok()
                {
                    return Ok(());
                }
                function(parser)?;
                while !parser.is_exhausted() {
                    function(parser)?;
                }
                Ok(())
            })
            .is_ok(),
    )
}

fn function<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser.try_parse(urls::parse).is_ok() {
        return Ok(());
    }
    let name = parser.expect_function()?.clone();
    parser.parse_nested_block(|parser| {
        match_ignore_ascii_case! {name.as_ref(),
            "drop-shadow" => shadows::drop_shadow(parser),
            "blur" => {
                if parser.is_exhausted() { Ok(()) } else { shadows::length(parser, true) }
            },
            "hue-rotate" => {
                if parser.is_exhausted() { return Ok(()) }
                let source = numeric_component(parser)?;
                let valid = numeric_functions::syntax_value_is_valid(source, |source| {
                    let mut input = ParserInput::new(source);
                    Parser::new(&mut input).parse_entirely(Angle::parse_with_unitless_zero).is_ok()
                    || Calc::<Angle>::parse_string(source).is_ok_and(|value| angle_calculation_is_valid(&value))
                });
                if valid { Ok(()) } else { Err(parser.new_custom_error(())) }
            },
            "brightness" | "contrast" | "grayscale" | "invert" | "opacity" | "saturate" | "sepia" => {
                if parser.is_exhausted() { return Ok(()) }
                let source = numeric_component(parser)?;
                if !literal_numeric_tokens_are_valid(source, true, true) { return Err(parser.new_custom_error(())) }
                let valid = numeric_functions::syntax_value_is_valid(source, |source| {
                    let mut input = ParserInput::new(source);
                    parse_number(source).is_some() || Parser::new(&mut input).parse_entirely(parse_percentage_value).is_ok()
                });
                if valid { Ok(()) } else { Err(parser.new_custom_error(())) }
            },
            _ => Err(parser.new_custom_error(()))
        }
    })
}
