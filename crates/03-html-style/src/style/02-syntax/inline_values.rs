//! Inline alignment and overflow grammar beyond the dependency's value model.

use super::value::{
    length_percentage_is_valid, literal_numeric_tokens_are_valid, numeric_component,
    without_priority,
};
use super::{numeric_functions, random_math, tree_counting};
use cssparser::{ParseError, Parser, ParserInput, match_ignore_ascii_case};
use lightningcss::properties::PropertyId;
use lightningcss::traits::{Parse, Zero};
use lightningcss::values::length::LengthPercentage;
use std::borrow::Cow;

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    match id {
        PropertyId::VerticalAlign => Some(vertical_align(value).is_some()),
        PropertyId::TextOverflow(_) => Some(text_overflow(value).is_some()),
        _ => None,
    }
}

pub(crate) fn value_is_supported(id: &PropertyId<'_>, value: &str) -> bool {
    match id {
        PropertyId::VerticalAlign => {
            vertical_align(value).is_some_and(|value| value.legacy().is_some())
        }
        PropertyId::TextOverflow(_) => text_overflow(value) == Some(true),
        _ => false,
    }
}

/// Project equivalent shorthand combinations using their original components.
/// Numeric grammar stand-ins never escape into the declaration or cascade.
pub(crate) fn normalize_vertical_align(value: &str) -> Cow<'_, str> {
    let component = without_priority(value);
    let Some(legacy) = vertical_align(component).and_then(|value| value.legacy()) else {
        return Cow::Borrowed(value);
    };
    if legacy == component {
        Cow::Borrowed(value)
    } else {
        Cow::Owned(format!("{legacy}{}", &value[component.len()..]))
    }
}

#[derive(Clone, Copy)]
enum BaselineSource {
    First,
    Last,
}

#[derive(Clone, Copy, Default)]
enum AlignmentBaseline {
    #[default]
    Baseline,
    TextTop,
    TextBottom,
    Middle,
    Alphabetic,
    Ideographic,
    Central,
    Mathematical,
    Hanging,
}

impl AlignmentBaseline {
    fn legacy(self) -> Option<&'static str> {
        match self {
            Self::Baseline => Some("baseline"),
            Self::TextTop => Some("text-top"),
            Self::TextBottom => Some("text-bottom"),
            Self::Middle => Some("middle"),
            _ => None,
        }
    }
}

enum BaselineShift<'a> {
    Sub,
    Super,
    Top,
    Center,
    Bottom,
    Length(&'a str),
}

#[derive(Default)]
struct VerticalAlignment<'a> {
    source: Option<BaselineSource>,
    baseline: Option<AlignmentBaseline>,
    shift: Option<BaselineShift<'a>>,
}

impl<'a> VerticalAlignment<'a> {
    fn legacy(self) -> Option<&'a str> {
        if self.source.is_some() {
            return None;
        }
        let baseline = self.baseline.unwrap_or_default();
        if matches!(baseline, AlignmentBaseline::Baseline) {
            return match self.shift {
                None => Some("baseline"),
                Some(BaselineShift::Sub) => Some("sub"),
                Some(BaselineShift::Super) => Some("super"),
                Some(BaselineShift::Top) => Some("top"),
                Some(BaselineShift::Bottom) => Some("bottom"),
                Some(BaselineShift::Length(value)) => Some(value),
                Some(BaselineShift::Center) => None,
            };
        }
        // A zero shift preserves a supported alignment baseline. Check the
        // actual expression here, independently of syntax-only stand-ins.
        match self.shift {
            None => baseline.legacy(),
            Some(BaselineShift::Length(value))
                if LengthPercentage::parse_string(value).is_ok_and(|value| value.is_zero()) =>
            {
                baseline.legacy()
            }
            _ => None,
        }
    }
}

fn vertical_align(value: &str) -> Option<VerticalAlignment<'_>> {
    let mut input = ParserInput::new(value);
    Parser::new(&mut input).parse_entirely(|parser| {
        let mut result = VerticalAlignment::default();
        let mut any = false;
        while !parser.is_exhausted() {
            if let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
                match_ignore_ascii_case! { name.as_ref(),
                    "first" | "last" if result.source.is_none() => {
                        result.source = Some(if name.eq_ignore_ascii_case("first") { BaselineSource::First } else { BaselineSource::Last });
                    },
                    "baseline" | "text-top" | "text-bottom" | "middle" | "alphabetic" | "ideographic" | "central" | "mathematical" | "hanging" if result.baseline.is_none() => {
                        result.baseline = Some(match_ignore_ascii_case! { name.as_ref(),
                            "baseline" => AlignmentBaseline::Baseline,
                            "text-top" => AlignmentBaseline::TextTop,
                            "text-bottom" => AlignmentBaseline::TextBottom,
                            "middle" => AlignmentBaseline::Middle,
                            "alphabetic" => AlignmentBaseline::Alphabetic,
                            "ideographic" => AlignmentBaseline::Ideographic,
                            "central" => AlignmentBaseline::Central,
                            "mathematical" => AlignmentBaseline::Mathematical,
                            _ => AlignmentBaseline::Hanging,
                        });
                    },
                    "sub" | "super" | "top" | "center" | "bottom" if result.shift.is_none() => {
                        result.shift = Some(match_ignore_ascii_case! { name.as_ref(),
                            "sub" => BaselineShift::Sub,
                            "super" => BaselineShift::Super,
                            "top" => BaselineShift::Top,
                            "bottom" => BaselineShift::Bottom,
                            _ => BaselineShift::Center,
                        });
                    },
                    _ => return Err(parser.new_custom_error(())),
                }
            } else if result.shift.is_none() {
                result.shift = Some(BaselineShift::Length(length_percentage(parser)?));
            } else { return Err(parser.new_custom_error(())); }
            any = true;
        }
        if any { Ok(result) } else { Err(parser.new_custom_error(())) }
    }).ok()
}

fn length_percentage<'i>(parser: &mut Parser<'i, '_>) -> Result<&'i str, ParseError<'i, ()>> {
    let source = numeric_component(parser)?;
    let tree = tree_counting::lower(source, |_| 1).map_err(|_| parser.new_custom_error(()))?;
    let random = random_math::for_syntax(&tree).map_err(|_| parser.new_custom_error(()))?;
    let valid = literal_numeric_tokens_are_valid(source, false, false)
        && numeric_functions::syntax_value_is_valid(&random, |source| {
            LengthPercentage::parse_string(source)
                .is_ok_and(|value| length_percentage_is_valid(&value))
        });
    if valid {
        Ok(source)
    } else {
        Err(parser.new_custom_error(()))
    }
}

/// Return whether the value is one of the renderer's single-edge keywords.
fn text_overflow(value: &str) -> Option<bool> {
    let mut input = ParserInput::new(value);
    Parser::new(&mut input)
        .parse_entirely(|parser| {
            let first = overflow_edge(parser)?;
            if parser.is_exhausted() {
                return Ok(first);
            }
            overflow_edge(parser)?;
            Ok(false)
        })
        .ok()
}

fn overflow_edge<'i>(parser: &mut Parser<'i, '_>) -> Result<bool, ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_string_cloned())
        .is_ok()
    {
        return Ok(false);
    }
    if let Ok(name) = parser.try_parse(|parser| parser.expect_ident_cloned()) {
        return match_ignore_ascii_case! {name.as_ref(),
            "clip" | "ellipsis" => Ok(true),
            "fade" => Ok(false),
            _ => Err(parser.new_custom_error(())),
        };
    }
    parser.expect_function_matching("fade")?;
    parser.parse_nested_block(|parser| length_percentage(parser).map(|_| ()))?;
    Ok(false)
}
