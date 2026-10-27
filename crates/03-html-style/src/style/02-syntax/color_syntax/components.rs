//! Validate channel types without resolving colors, missing values or references.

use super::super::numeric_functions;
use super::super::sign;
use super::super::unit_algebra::{Calculation, Dimension};
use super::super::value::{
    angle_calculation_is_valid, numeric_component, parse_number, parse_percentage_value,
};
use super::color;
use cssparser::{ParseError, Parser, ParserInput, match_ignore_ascii_case, serialize_identifier};
use lightningcss::traits::Parse;
use lightningcss::values::angle::Angle;
use lightningcss::values::calc::Calc;
use lightningcss::values::ident::CustomIdent;
use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Function {
    Rgb,
    Hsl,
    Hwb,
    Lab,
    Lch,
    Oklab,
    Oklch,
    Color,
    Alpha,
}

impl Function {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        Some(match_ignore_ascii_case! {name,
            "rgb" | "rgba" => Self::Rgb,
            "hsl" | "hsla" => Self::Hsl,
            "hwb" => Self::Hwb,
            "lab" => Self::Lab,
            "lch" => Self::Lch,
            "oklab" => Self::Oklab,
            "oklch" => Self::Oklch,
            "color" => Self::Color,
            "alpha" => Self::Alpha,
            _ => return None,
        })
    }

    fn hue_index(self) -> Option<usize> {
        match self {
            Self::Hsl | Self::Hwb => Some(0),
            Self::Lch | Self::Oklch => Some(2),
            _ => None,
        }
    }

    fn references(self) -> References {
        References::Named(match self {
            Self::Rgb | Self::Color => ["r", "g", "b"],
            Self::Hsl => ["h", "s", "l"],
            Self::Hwb => ["h", "w", "b"],
            Self::Lab | Self::Oklab => ["l", "a", "b"],
            Self::Lch | Self::Oklch => ["l", "c", "h"],
            Self::Alpha => return References::Alpha,
        })
    }
}

#[derive(Clone, Copy)]
enum References {
    Alpha,
    Named([&'static str; 3]),
    Custom,
}

impl References {
    fn accepts(self, name: &str) -> bool {
        if name.eq_ignore_ascii_case("alpha") {
            return true;
        }
        match self {
            Self::Alpha => false,
            Self::Named(names) => names.iter().any(|item| name.eq_ignore_ascii_case(item)),
            // Custom profile channel names depend on its components descriptor.
            // They remain symbolic until a profile can be resolved at runtime.
            Self::Custom => {
                let mut ident = String::new();
                let _ = serialize_identifier(name, &mut ident);
                CustomIdent::parse_string(&ident).is_ok()
                    && !["none", "e", "pi", "infinity", "-infinity", "NaN"]
                        .iter()
                        .any(|item| name.eq_ignore_ascii_case(item))
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ChannelType {
    NumberPercentage,
    Percentage,
    Hue,
}

#[derive(Clone, Copy, PartialEq)]
enum Channel {
    Number,
    Percentage,
    Angle,
    Missing,
}

pub(super) fn parse<'i>(
    parser: &mut Parser<'i, '_>,
    function: Function,
    depth: usize,
) -> Result<(), ParseError<'i, ()>> {
    let relative = parser
        .try_parse(|parser| parser.expect_ident_matching("from"))
        .is_ok();
    if relative {
        color(parser, depth + 1)?;
    }
    if function == Function::Alpha {
        if !relative {
            return Err(parser.new_custom_error(()));
        }
        parser.expect_delim('/')?;
        channel(
            parser,
            ChannelType::NumberPercentage,
            true,
            Some(References::Alpha),
        )?;
        parser.expect_exhausted()?;
        return Ok(());
    }
    let mut references = function.references();
    let mut custom = false;
    if function == Function::Color {
        let name = parser.expect_ident_cloned()?;
        if name.starts_with("--") {
            custom = true;
            references = References::Custom;
        } else if ["xyz", "xyz-d50", "xyz-d65"]
            .iter()
            .any(|space| name.eq_ignore_ascii_case(space))
        {
            references = References::Named(["x", "y", "z"]);
        } else if ![
            "srgb",
            "srgb-linear",
            "display-p3",
            "display-p3-linear",
            "a98-rgb",
            "prophoto-rgb",
            "rec2020",
        ]
        .iter()
        .any(|space| name.eq_ignore_ascii_case(space))
        {
            return Err(parser.new_custom_error(()));
        }
    }
    let references = relative.then_some(references);
    let mode = |index| {
        if function.hue_index() == Some(index) {
            ChannelType::Hue
        } else {
            ChannelType::NumberPercentage
        }
    };
    let first = channel(parser, mode(0), true, references)?;
    if parser.try_parse(Parser::expect_comma).is_ok() {
        if relative
            || !matches!(function, Function::Rgb | Function::Hsl)
            || first == Channel::Missing
        {
            return Err(parser.new_custom_error(()));
        }
        let mode = if function == Function::Hsl {
            ChannelType::Percentage
        } else {
            ChannelType::NumberPercentage
        };
        let second = channel(parser, mode, false, None)?;
        parser.expect_comma()?;
        let third = channel(parser, mode, false, None)?;
        if function == Function::Rgb && (first != second || first != third) {
            return Err(parser.new_custom_error(()));
        }
        if parser.try_parse(Parser::expect_comma).is_ok() {
            channel(parser, ChannelType::NumberPercentage, false, None)?;
        }
    } else {
        if custom {
            // Custom profiles can define any nonempty number of channels.
            while parser
                .try_parse(|parser| {
                    channel(parser, ChannelType::NumberPercentage, true, references)
                })
                .is_ok()
            {}
        } else {
            channel(parser, mode(1), true, references)?;
            channel(parser, mode(2), true, references)?;
        }
        if parser.try_parse(|parser| parser.expect_delim('/')).is_ok() {
            channel(parser, ChannelType::NumberPercentage, true, references)?;
        }
    }
    parser.expect_exhausted()?;
    Ok(())
}

fn channel<'i>(
    parser: &mut Parser<'i, '_>,
    mode: ChannelType,
    missing: bool,
    references: Option<References>,
) -> Result<Channel, ParseError<'i, ()>> {
    if let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
        if missing && name.eq_ignore_ascii_case("none") {
            return Ok(Channel::Missing);
        }
        return if references.is_some_and(|references| references.accepts(&name)) {
            Ok(Channel::Number)
        } else {
            Err(parser.new_custom_error(()))
        };
    }
    let source = numeric_component(parser)?;
    let source = if let Some(references) = references {
        // Color references have number type in every argument position. These
        // stand-ins validate math only and never reach source or computed styles.
        numeric_functions::replace_identifiers(source, |name| {
            Ok(references.accepts(name).then(|| "1".to_owned()))
        })
        .map_err(|_| parser.new_custom_error(()))?
    } else {
        Cow::Borrowed(source)
    };
    let source =
        numeric_functions::nonzero_divisors(&source).map_err(|_| parser.new_custom_error(()))?;
    if let Some(value) = Calculation::parse(&source, false).filter(|value| value.extended) {
        return match value.dimension {
            Dimension::NUMBER if !matches!(mode, ChannelType::Percentage) => Ok(Channel::Number),
            Dimension::ANGLE if matches!(mode, ChannelType::Hue) => Ok(Channel::Angle),
            Dimension::PERCENTAGE if !matches!(mode, ChannelType::Hue) => Ok(Channel::Percentage),
            _ => Err(parser.new_custom_error(())),
        };
    }
    let source = sign::lower(&source, |_| Some(1.0)).map_err(|_| parser.new_custom_error(()))?;
    // A contextual sign stand-in can make a previously unresolved divisor zero.
    let source =
        numeric_functions::nonzero_divisors(&source).map_err(|_| parser.new_custom_error(()))?;
    if !matches!(mode, ChannelType::Percentage) && parse_number(&source).is_some() {
        return Ok(Channel::Number);
    }
    if matches!(mode, ChannelType::Hue) {
        if Angle::parse_string(&source).is_ok()
            || Calc::<Angle>::parse_string(&source)
                .is_ok_and(|value| angle_calculation_is_valid(&value))
        {
            return Ok(Channel::Angle);
        }
    } else {
        let mut input = ParserInput::new(&source);
        if Parser::new(&mut input)
            .parse_entirely(parse_percentage_value)
            .is_ok()
        {
            return Ok(Channel::Percentage);
        }
    }
    Err(parser.new_custom_error(()))
}
