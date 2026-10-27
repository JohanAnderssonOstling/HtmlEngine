//! Animation and transition grammar, independent of playback support.

use super::value::{
    NumberValue, PercentageValue, length_percentage_is_valid, literal_numeric_tokens_are_valid,
    numeric_component, parse_number_value, parse_percentage_value, time_calculation_is_valid,
};
use super::{easing, numeric_functions};
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::properties::animation::{
    AnimationAttachmentRange, AnimationDirection, AnimationFillMode, AnimationName,
    AnimationPlayState, AnimationRange, AnimationRangeEnd, AnimationRangeStart, AnimationTimeline,
    TimelineRangeName,
};
use lightningcss::rules::keyframes::KeyframesName;
use lightningcss::traits::Parse;
use lightningcss::values::calc::Calc;
use lightningcss::values::ident::CustomIdent;
use lightningcss::values::time::Time;

#[derive(Clone, Copy)]
enum MotionProperty {
    Transition,
    TransitionProperty,
    TransitionBehavior,
    TransitionDuration,
    Delay,
    Animation,
    AnimationDuration,
    IterationCount,
    AnimationName,
    Range,
    RangeStart,
    RangeEnd,
}

impl MotionProperty {
    fn from_id(id: &PropertyId<'_>) -> Option<Self> {
        Some(match id {
            PropertyId::Transition(_) => Self::Transition,
            PropertyId::TransitionProperty(_) => Self::TransitionProperty,
            PropertyId::TransitionDuration(_) => Self::TransitionDuration,
            PropertyId::TransitionDelay(_) | PropertyId::AnimationDelay(_) => Self::Delay,
            PropertyId::Animation(_) => Self::Animation,
            PropertyId::AnimationDuration(_) => Self::AnimationDuration,
            PropertyId::AnimationIterationCount(_) => Self::IterationCount,
            PropertyId::AnimationName(_) => Self::AnimationName,
            PropertyId::AnimationRange => Self::Range,
            PropertyId::AnimationRangeStart => Self::RangeStart,
            PropertyId::AnimationRangeEnd => Self::RangeEnd,
            // The dependency has no typed ID for this Level 2 longhand.
            PropertyId::Custom(name)
                if name.as_ref().eq_ignore_ascii_case("transition-behavior") =>
            {
                Self::TransitionBehavior
            }
            _ => return None,
        })
    }
}

pub(crate) fn supports(id: &PropertyId<'_>) -> bool {
    MotionProperty::from_id(id).is_some()
}

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, source: &str) -> Option<bool> {
    let property = MotionProperty::from_id(id)?;
    Some(numeric_functions::syntax_value_is_valid(source, |source| {
        let mut input = ParserInput::new(source);
        Parser::new(&mut input)
            .parse_entirely(|parser| {
                let items = parser.parse_comma_separated(|parser| {
                    match property {
                        MotionProperty::Transition => return transition(parser),
                        MotionProperty::TransitionProperty => return transition_property(parser),
                        MotionProperty::TransitionBehavior => behavior(parser)?,
                        MotionProperty::TransitionDuration => duration(parser, false)?,
                        MotionProperty::Delay => {
                            time(parser)?;
                        }
                        MotionProperty::Animation => animation(parser)?,
                        MotionProperty::AnimationDuration => duration(parser, true)?,
                        MotionProperty::IterationCount => iteration_count(parser)?,
                        MotionProperty::AnimationName => animation_name(parser)?,
                        MotionProperty::Range
                        | MotionProperty::RangeStart
                        | MotionProperty::RangeEnd => {
                            let start = parser.position();
                            let valid = match property {
                                MotionProperty::Range => {
                                    let range = AnimationRange::parse(parser)
                                        .map_err(|_| parser.new_custom_error(()))?;
                                    attachment_range_is_valid(&range.start.0)
                                        && attachment_range_is_valid(&range.end.0)
                                }
                                MotionProperty::RangeStart => attachment_range_is_valid(
                                    &AnimationRangeStart::parse(parser)
                                        .map_err(|_| parser.new_custom_error(()))?
                                        .0,
                                ),
                                MotionProperty::RangeEnd => attachment_range_is_valid(
                                    &AnimationRangeEnd::parse(parser)
                                        .map_err(|_| parser.new_custom_error(()))?
                                        .0,
                                ),
                                _ => unreachable!(),
                            };
                            if !valid
                                || !literal_numeric_tokens_are_valid(
                                    parser.slice_from(start),
                                    false,
                                    false,
                                )
                            {
                                return Err(parser.new_custom_error(()));
                            }
                        }
                    }
                    Ok(false)
                })?;
                // `none` cannot be one property among multiple transitions.
                if items.len() > 1 && items.contains(&true) {
                    Err(parser.new_custom_error(()))
                } else {
                    Ok(())
                }
            })
            .is_ok()
    }))
}

fn transition_property<'i>(parser: &mut Parser<'i, '_>) -> Result<bool, ParseError<'i, ()>> {
    let name = CustomIdent::parse(parser).map_err(|_| parser.new_custom_error(()))?;
    Ok(name.0.eq_ignore_ascii_case("none"))
}

fn behavior<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let name = parser.expect_ident()?;
    if name.eq_ignore_ascii_case("normal") || name.eq_ignore_ascii_case("allow-discrete") {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

/// Return whether the literal satisfies the duration range. A negative first
/// time must fail as a duration rather than being reinterpreted as a delay.
fn time<'i>(parser: &mut Parser<'i, '_>) -> Result<bool, ParseError<'i, ()>> {
    let source = numeric_component(parser)?;
    if Time::parse_string(source).is_err()
        && !Calc::<Time>::parse_string(source).is_ok_and(|value| time_calculation_is_valid(&value))
    {
        return Err(parser.new_custom_error(()));
    }
    Ok(literal_numeric_tokens_are_valid(source, true, false))
}

fn duration<'i>(parser: &mut Parser<'i, '_>, auto: bool) -> Result<(), ParseError<'i, ()>> {
    if auto
        && parser
            .try_parse(|parser| parser.expect_ident_matching("auto"))
            .is_ok()
    {
        return Ok(());
    }
    if time(parser)? {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn transition<'i>(parser: &mut Parser<'i, '_>) -> Result<bool, ParseError<'i, ()>> {
    let (mut duration, mut delay, mut easing, mut behavior, mut property) =
        (false, false, false, false, None);
    let mut any = false;
    loop {
        if !duration {
            if let Ok(nonnegative) = parser.try_parse(time) {
                if !nonnegative {
                    return Err(parser.new_custom_error(()));
                }
                duration = true;
                any = true;
                continue;
            }
        }
        if !delay && duration && parser.try_parse(time).is_ok() {
            delay = true;
        } else if !easing && parser.try_parse(easing::function).is_ok() {
            easing = true;
        } else if !behavior && parser.try_parse(self::behavior).is_ok() {
            behavior = true;
        } else if property.is_none() {
            if let Ok(none) = parser.try_parse(transition_property) {
                property = Some(none);
            } else {
                break;
            }
        } else {
            break;
        }
        any = true;
    }
    if any {
        Ok(property == Some(true))
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn iteration_count<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("infinite"))
        .is_ok()
    {
        return Ok(());
    }
    match parse_number_value(parser)? {
        NumberValue::Number(value) if value < 0.0 => Err(parser.new_custom_error(())),
        _ => Ok(()),
    }
}

fn animation_name<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    match AnimationName::parse(parser).map_err(|_| parser.new_custom_error(()))? {
        AnimationName::String(name) if name.is_empty() => Err(parser.new_custom_error(())),
        _ => Ok(()),
    }
}

fn animation<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let (mut duration, mut delay, mut easing, mut iterations) = (false, false, false, false);
    let (mut direction, mut fill, mut play, mut name, mut timeline) =
        (false, false, false, false, false);
    let mut any = false;
    loop {
        if !duration {
            if parser
                .try_parse(|parser| parser.expect_ident_matching("auto"))
                .is_ok()
            {
                duration = true;
                any = true;
                continue;
            }
            if let Ok(nonnegative) = parser.try_parse(time) {
                if !nonnegative {
                    return Err(parser.new_custom_error(()));
                }
                duration = true;
                any = true;
                continue;
            }
        }
        if !delay && duration && parser.try_parse(time).is_ok() {
            delay = true;
        } else if !easing && parser.try_parse(easing::function).is_ok() {
            easing = true;
        } else if !iterations && parser.try_parse(iteration_count).is_ok() {
            iterations = true;
        } else if !direction && parser.try_parse(AnimationDirection::parse).is_ok() {
            direction = true;
        } else if !fill && parser.try_parse(AnimationFillMode::parse).is_ok() {
            fill = true;
        } else if !play && parser.try_parse(AnimationPlayState::parse).is_ok() {
            play = true;
        } else if !name && parser.try_parse(animation_name).is_ok() {
            name = true;
        } else if !timeline && parser.try_parse(AnimationTimeline::parse).is_ok() {
            timeline = true;
        } else {
            break;
        }
        any = true;
    }
    if any {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn attachment_range_is_valid(range: &AnimationAttachmentRange) -> bool {
    match range {
        AnimationAttachmentRange::Normal => true,
        AnimationAttachmentRange::LengthPercentage(offset)
        | AnimationAttachmentRange::TimelineRange { offset, .. } => {
            length_percentage_is_valid(offset)
        }
    }
}

pub(crate) fn keyframes_name_is_valid(source: &str) -> bool {
    KeyframesName::parse_string(source)
        .is_ok_and(|name| !matches!(name, KeyframesName::Custom(name) if name.is_empty()))
}

pub(crate) fn keyframe_selectors_are_valid(source: &str) -> bool {
    let Ok(source) = super::tree_counting::lower(source, |_| 1) else {
        return false;
    };
    let Ok(source) = super::random_math::for_syntax(&source) else {
        return false;
    };
    numeric_functions::syntax_value_is_valid(&source, |source| {
        let mut input = ParserInput::new(source);
        Parser::new(&mut input)
            .parse_entirely(|parser| {
                parser.parse_comma_separated(|parser| {
                    if parser
                        .try_parse(|parser| parser.expect_ident_matching("from"))
                        .is_ok()
                        || parser
                            .try_parse(|parser| parser.expect_ident_matching("to"))
                            .is_ok()
                    {
                        return Ok(());
                    }
                    let named_range = parser.try_parse(TimelineRangeName::parse).is_ok();
                    match parse_percentage_value(parser)? {
                        PercentageValue::Percentage(value)
                            if !named_range && !(0.0..=1.0).contains(&value) =>
                        {
                            Err(parser.new_custom_error(()))
                        }
                        _ => Ok(()),
                    }
                })
            })
            .is_ok()
    })
}
