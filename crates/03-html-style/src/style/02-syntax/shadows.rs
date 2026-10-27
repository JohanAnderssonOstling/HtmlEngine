//! Shadow syntax, independent of text/box shadow painting.

use super::value::{length_is_valid, literal_numeric_tokens_are_valid, numeric_component};
use super::{color_syntax, numeric_functions};
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::traits::Parse;
use lightningcss::values::length::Length;

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    let kind = match id {
        PropertyId::TextShadow => ShadowKind::Text,
        PropertyId::BoxShadow(_) => ShadowKind::Box,
        _ => return None,
    };
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
                parser.parse_comma_separated(|parser| shadow(parser, kind))?;
                Ok(())
            })
            .is_ok(),
    )
}

#[derive(Clone, Copy, PartialEq)]
enum ShadowKind {
    Text,
    Box,
    Drop,
}

pub(super) fn drop_shadow<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    shadow(parser, ShadowKind::Drop)
}

fn shadow<'i>(parser: &mut Parser<'i, '_>, kind: ShadowKind) -> Result<(), ParseError<'i, ()>> {
    let (mut lengths, mut color, mut inset) = (false, false, false);
    while !parser.is_exhausted() {
        if !inset
            && kind != ShadowKind::Drop
            && parser
                .try_parse(|parser| parser.expect_ident_matching("inset"))
                .is_ok()
        {
            inset = true;
        } else if !lengths
            && parser
                .try_parse(|parser| length_group(parser, kind))
                .is_ok()
        {
            lengths = true;
        } else if !color
            && parser
                .try_parse(|parser| color_syntax::color(parser, 0))
                .is_ok()
        {
            color = true;
        } else {
            return Err(parser.new_custom_error(()));
        }
    }
    if lengths {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn length_group<'i>(
    parser: &mut Parser<'i, '_>,
    kind: ShadowKind,
) -> Result<(), ParseError<'i, ()>> {
    length(parser, false)?;
    length(parser, false)?;
    if parser.try_parse(|parser| length(parser, true)).is_ok() && kind != ShadowKind::Drop {
        let _ = parser.try_parse(|parser| length(parser, kind == ShadowKind::Text));
    }
    Ok(())
}

pub(super) fn length<'i>(
    parser: &mut Parser<'i, '_>,
    non_negative: bool,
) -> Result<(), ParseError<'i, ()>> {
    let source = numeric_component(parser)?;
    // Range-check literals before native simplification erases their source.
    // Calculated negative blur/spread values clamp when computed.
    if literal_numeric_tokens_are_valid(source, non_negative, false)
        && numeric_functions::syntax_value_is_valid(source, |source| {
            Length::parse_string(source).is_ok_and(|value| length_is_valid(&value))
        })
    {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}
