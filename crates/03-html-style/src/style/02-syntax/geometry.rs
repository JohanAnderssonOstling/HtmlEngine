//! Typed geometric value grammar, independent of painting and transforms.

use super::numeric_functions;
use super::value::{
    angle_calculation_is_valid, length_is_valid, length_percentage_is_valid,
    literal_numeric_tokens_are_valid, numeric_component, parse_number, parse_percentage_value,
};
use cssparser::{ParseError, Parser, ParserInput, match_ignore_ascii_case};
use lightningcss::properties::PropertyId;
use lightningcss::traits::Parse;
use lightningcss::values::angle::Angle;
use lightningcss::values::calc::Calc;
use lightningcss::values::length::{Length, LengthPercentage};
use lightningcss::vendor_prefix::VendorPrefix;

pub(crate) fn supports(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::BackgroundPosition
            | PropertyId::BackgroundPositionX
            | PropertyId::BackgroundPositionY
            | PropertyId::MaskPosition(_)
            | PropertyId::MaskPositionX
            | PropertyId::MaskPositionY
            | PropertyId::BackgroundSize
            | PropertyId::MaskSize(_)
            | PropertyId::Perspective(_)
            | PropertyId::PerspectiveOrigin(_)
            | PropertyId::TransformOrigin(_)
            | PropertyId::Transform(_)
            | PropertyId::Scale
    )
}

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    if !supports(id) {
        return None;
    }
    let mut input = ParserInput::new(value);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| {
                match id {
                    PropertyId::BackgroundPosition => {
                        parser.parse_comma_separated(|parser| position(parser, true))?;
                    }
                    PropertyId::MaskPosition(_) => {
                        parser.parse_comma_separated(|parser| position(parser, false))?;
                    }
                    PropertyId::BackgroundPositionX | PropertyId::MaskPositionX => {
                        parser.parse_comma_separated(|parser| axis(parser, true))?;
                    }
                    PropertyId::BackgroundPositionY | PropertyId::MaskPositionY => {
                        parser.parse_comma_separated(|parser| axis(parser, false))?;
                    }
                    PropertyId::BackgroundSize | PropertyId::MaskSize(_) => {
                        parser.parse_comma_separated(size)?;
                    }
                    PropertyId::PerspectiveOrigin(_) => position(parser, false)?,
                    PropertyId::TransformOrigin(_) => transform_origin(parser)?,
                    PropertyId::Perspective(prefix) => {
                        perspective(parser, prefix.contains(VendorPrefix::WebKit))?
                    }
                    PropertyId::Scale => {
                        if parser
                            .try_parse(|parser| parser.expect_ident_matching("none"))
                            .is_ok()
                        {
                            return Ok(());
                        }
                        numeric(parser, Numeric::NumberPercentage, false)?;
                        for _ in 1..3 {
                            if parser
                                .try_parse(|parser| {
                                    numeric(parser, Numeric::NumberPercentage, false)
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    PropertyId::Transform(prefix) => {
                        if parser
                            .try_parse(|parser| parser.expect_ident_matching("none"))
                            .is_ok()
                        {
                            return Ok(());
                        }
                        transform_function(parser, prefix.contains(VendorPrefix::WebKit))?;
                        while !parser.is_exhausted() {
                            transform_function(parser, prefix.contains(VendorPrefix::WebKit))?;
                        }
                    }
                    _ => unreachable!(),
                }
                Ok(())
            })
            .is_ok(),
    )
}

#[derive(Clone, Copy)]
pub(super) enum Numeric {
    Number,
    NumberPercentage,
    Length,
    LengthPercentage,
    Angle,
}

pub(super) fn numeric<'i>(
    parser: &mut Parser<'i, '_>,
    kind: Numeric,
    nonnegative: bool,
) -> Result<(), ParseError<'i, ()>> {
    let source = numeric_component(parser)?;
    let numbers_allowed = matches!(kind, Numeric::Number | Numeric::NumberPercentage);
    if !literal_numeric_tokens_are_valid(source, nonnegative, numbers_allowed) {
        return Err(parser.new_custom_error(()));
    }
    let valid = numeric_functions::syntax_value_is_valid(source, |source| match kind {
        Numeric::Number => parse_number(source).is_some(),
        Numeric::NumberPercentage => {
            let mut input = ParserInput::new(source);
            parse_number(source).is_some()
                || Parser::new(&mut input)
                    .parse_entirely(parse_percentage_value)
                    .is_ok()
        }
        Numeric::Length => Length::parse_string(source).is_ok_and(|value| length_is_valid(&value)),
        Numeric::LengthPercentage => LengthPercentage::parse_string(source)
            .is_ok_and(|value| length_percentage_is_valid(&value)),
        Numeric::Angle => {
            let mut input = ParserInput::new(source);
            Parser::new(&mut input)
                .parse_entirely(Angle::parse_with_unitless_zero)
                .is_ok()
                || Calc::<Angle>::parse_string(source)
                    .is_ok_and(|value| angle_calculation_is_valid(&value))
        }
    });
    if valid {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

pub(super) fn length_percentage<'i>(
    parser: &mut Parser<'i, '_>,
    nonnegative: bool,
) -> Result<(), ParseError<'i, ()>> {
    numeric(parser, Numeric::LengthPercentage, nonnegative)
}

pub(super) fn length<'i>(
    parser: &mut Parser<'i, '_>,
    nonnegative: bool,
) -> Result<(), ParseError<'i, ()>> {
    numeric(parser, Numeric::Length, nonnegative)
}

#[derive(Clone, Copy)]
enum Component {
    Horizontal,
    Vertical,
    Center,
    Offset,
}

fn component<'i>(parser: &mut Parser<'i, '_>) -> Result<Component, ParseError<'i, ()>> {
    if let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
        return match_ignore_ascii_case! {name.as_ref(),
            "left" | "right" => Ok(Component::Horizontal),
            "top" | "bottom" => Ok(Component::Vertical),
            "center" => Ok(Component::Center),
            _ => Err(parser.new_custom_error(())),
        };
    }
    length_percentage(parser, false)?;
    Ok(Component::Offset)
}

fn valid_pair(a: Component, b: Component) -> bool {
    use Component::*;
    matches!(
        (a, b),
        (Horizontal | Center | Offset, Vertical | Center | Offset)
            | (Vertical, Horizontal | Center)
            | (Center, Horizontal)
    )
}

/// Values 4 permits one, two or four components. Backgrounds additionally
/// retain their legacy three-component form, which masks and origins exclude.
pub(super) fn position<'i>(
    parser: &mut Parser<'i, '_>,
    allow_three: bool,
) -> Result<(), ParseError<'i, ()>> {
    use Component::*;
    let mut components = [Center; 4];
    components[0] = component(parser)?;
    let mut count = 1;
    for slot in &mut components[1..] {
        let Ok(value) = parser.try_parse(component) else {
            break;
        };
        *slot = value;
        count += 1;
    }
    let valid = match &components[..count] {
        [_] => true,
        [a, b] => valid_pair(*a, *b),
        [Horizontal, Offset, Vertical | Center]
        | [Vertical, Offset, Horizontal | Center]
        | [Horizontal | Center, Vertical, Offset]
        | [Vertical | Center, Horizontal, Offset] => allow_three,
        [Horizontal, Offset, Vertical, Offset] | [Vertical, Offset, Horizontal, Offset] => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn axis<'i>(parser: &mut Parser<'i, '_>, horizontal: bool) -> Result<(), ParseError<'i, ()>> {
    if let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
        if name.eq_ignore_ascii_case("center") {
            return Ok(());
        }
        let valid = if horizontal {
            ["left", "right", "x-start", "x-end"]
        } else {
            ["top", "bottom", "y-start", "y-end"]
        };
        if !valid
            .iter()
            .any(|keyword| name.eq_ignore_ascii_case(keyword))
        {
            return Err(parser.new_custom_error(()));
        }
        let _ = parser.try_parse(|parser| length_percentage(parser, false));
        Ok(())
    } else {
        length_percentage(parser, false)
    }
}

pub(super) fn size<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| {
            let name = parser.expect_ident()?;
            if name.eq_ignore_ascii_case("cover") || name.eq_ignore_ascii_case("contain") {
                Ok(())
            } else {
                Err(parser.new_custom_error::<(), ()>(()))
            }
        })
        .is_ok()
    {
        return Ok(());
    }
    size_component(parser)?;
    let _ = parser.try_parse(size_component);
    Ok(())
}

fn size_component<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("auto"))
        .is_ok()
    {
        Ok(())
    } else {
        length_percentage(parser, true)
    }
}

fn transform_origin<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let first = component(parser)?;
    let Ok(second) = parser.try_parse(component) else {
        return Ok(());
    };
    if !valid_pair(first, second) {
        return Err(parser.new_custom_error(()));
    }
    if !parser.is_exhausted() {
        length(parser, false)?;
    }
    Ok(())
}

fn perspective<'i>(parser: &mut Parser<'i, '_>, webkit: bool) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("none"))
        .is_ok()
    {
        return Ok(());
    }
    if webkit && let Ok(number) = parser.try_parse(|parser| parser.expect_number()) {
        return if number >= 0.0 {
            Ok(())
        } else {
            Err(parser.new_custom_error(()))
        };
    }
    length(parser, true)
}

fn arguments<'i>(parser: &mut Parser<'i, '_>, kinds: &[Numeric]) -> Result<(), ParseError<'i, ()>> {
    for (index, kind) in kinds.iter().enumerate() {
        if index != 0 {
            parser.expect_comma()?;
        }
        numeric(parser, *kind, false)?;
    }
    Ok(())
}

fn optional_pair<'i>(parser: &mut Parser<'i, '_>, kind: Numeric) -> Result<(), ParseError<'i, ()>> {
    numeric(parser, kind, false)?;
    if !parser.is_exhausted() {
        parser.expect_comma()?;
        numeric(parser, kind, false)?;
    }
    Ok(())
}

fn transform_function<'i>(
    parser: &mut Parser<'i, '_>,
    webkit: bool,
) -> Result<(), ParseError<'i, ()>> {
    use Numeric::*;
    let name = parser.expect_function()?.clone();
    parser.parse_nested_block(|parser| {
        match_ignore_ascii_case! {name.as_ref(),
            "matrix" => arguments(parser, &[Number;6]),
            "matrix3d" => arguments(parser, &[Number;16]),
            "translate" => optional_pair(parser, LengthPercentage),
            "translatex" | "translatey" => numeric(parser, LengthPercentage, false),
            "translatez" => numeric(parser, Length, false),
            "translate3d" => arguments(parser, &[LengthPercentage,LengthPercentage,Length]),
            "scale" => optional_pair(parser, NumberPercentage),
            "scalex" | "scaley" | "scalez" => numeric(parser, NumberPercentage, false),
            "scale3d" => arguments(parser, &[NumberPercentage;3]),
            "rotate" | "rotatex" | "rotatey" | "rotatez" | "skewx" | "skewy" => numeric(parser, Angle, false),
            "rotate3d" => arguments(parser, &[Number,Number,Number,Angle]),
            "skew" => optional_pair(parser, Angle),
            "perspective" => perspective(parser, webkit),
            _ => Err(parser.new_custom_error(())),
        }
    })
}
