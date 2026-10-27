//! Timing-function syntax, independent of animation playback support.

use super::value::{
    IntegerValue, NumberValue, parse_integer_value, parse_number_value, parse_percentage_value,
};
use super::{numeric_functions, sign};
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::traits::Parse;
use lightningcss::values::easing::{EasingFunction, StepPosition};

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    if !matches!(
        id,
        PropertyId::AnimationTimingFunction(_) | PropertyId::TransitionTimingFunction(_)
    ) {
        return None;
    }
    let Ok(value) = numeric_functions::nonzero_divisors(value) else {
        return Some(false);
    };
    // Validate contextual signs while retaining their number type. Neither
    // zero-divisor stand-ins nor sign stand-ins are used for computation.
    let Ok(value) = sign::lower(&value, |_| Some(1.0)) else {
        return Some(false);
    };
    let Ok(value) = numeric_functions::nonzero_divisors(&value) else {
        return Some(false);
    };
    let mut input = ParserInput::new(&value);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| parser.parse_comma_separated(function))
            .is_ok(),
    )
}

pub(super) fn function<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
        return EasingFunction::parse_string(&name)
            .map(|_| ())
            .map_err(|_| parser.new_custom_error(()));
    }
    let name = parser.expect_function()?.clone();
    parser.parse_nested_block(|parser| {
        if name.eq_ignore_ascii_case("linear") {
            parser.parse_comma_separated(linear_stop)?;
            Ok(())
        } else if name.eq_ignore_ascii_case("cubic-bezier") {
            for index in 0..4 {
                if index != 0 {
                    parser.expect_comma()?;
                }
                let value = parse_number_value(parser)?;
                // Calculations defer range clamping until computation.
                if index % 2 == 0
                    && matches!(value, NumberValue::Number(value) if !(0.0..=1.0).contains(&value))
                {
                    return Err(parser.new_custom_error(()));
                }
            }
            Ok(())
        } else if name.eq_ignore_ascii_case("steps") {
            let count = parse_integer_value(parser, 1)?;
            let position = if parser.is_exhausted() {
                StepPosition::End
            } else {
                parser.expect_comma()?;
                StepPosition::parse(parser).map_err(|_| parser.new_custom_error(()))?
            };
            if position == StepPosition::JumpNone && matches!(count, IntegerValue::Integer(1)) {
                return Err(parser.new_custom_error(()));
            }
            Ok(())
        } else {
            Err(parser.new_custom_error(()))
        }
    })
}

fn linear_stop<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser.try_parse(parse_number_value).is_ok() {
        for _ in 0..2 {
            if parser.try_parse(percentage).is_err() {
                break;
            }
        }
    } else {
        // The && grammar permits the contiguous percentage group before
        // the output number, as well as after it.
        percentage(parser)?;
        let _ = parser.try_parse(percentage);
        parse_number_value(parser)?;
    }
    Ok(())
}

fn percentage<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    parse_percentage_value(parser).map(|_| ())
}
