//! Anchor-dependent value syntax, independent of anchor layout resolution.

use super::value::{
    length_percentage_is_valid, literal_numeric_tokens_are_valid, numeric_component,
    parse_percentage_value,
};
use super::{intrinsic_sizing, numeric_functions, random_math, sign, tree_counting, urls};
use crate::{PropertyValueSyntax, property_value_syntax};
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::properties::align::{AlignSelf, JustifySelf};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::traits::Parse;
use lightningcss::values::ident::DashedIdent;
use lightningcss::values::length::LengthPercentage;

fn is_inset(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::Top
            | PropertyId::Right
            | PropertyId::Bottom
            | PropertyId::Left
            | PropertyId::Inset
            | PropertyId::InsetBlock
            | PropertyId::InsetInline
            | PropertyId::InsetBlockStart
            | PropertyId::InsetBlockEnd
            | PropertyId::InsetInlineStart
            | PropertyId::InsetInlineEnd
    )
}

fn is_margin(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::Margin
            | PropertyId::MarginTop
            | PropertyId::MarginRight
            | PropertyId::MarginBottom
            | PropertyId::MarginLeft
            | PropertyId::MarginBlock
            | PropertyId::MarginInline
            | PropertyId::MarginBlockStart
            | PropertyId::MarginBlockEnd
            | PropertyId::MarginInlineStart
            | PropertyId::MarginInlineEnd
    )
}

fn is_alignment(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::AlignSelf(_) | PropertyId::JustifySelf | PropertyId::PlaceSelf
    )
}

fn is_context(id: &PropertyId<'_>) -> bool {
    is_inset(id)
        || is_margin(id)
        || is_alignment(id)
        || (intrinsic_sizing::is_size_property(id) && !matches!(id, PropertyId::FlexBasis(_)))
}

fn is_anchor_function(name: &str) -> bool {
    name.eq_ignore_ascii_case("anchor") || name.eq_ignore_ascii_case("anchor-size")
}

fn is_anchor_center(name: &str) -> bool {
    name.eq_ignore_ascii_case("anchor-center")
}

pub(crate) fn tokens_use_anchor(id: &PropertyId<'_>, tokens: &TokenList<'_>) -> bool {
    is_context(id) && (super::substitution::contains_function(tokens, is_anchor_function)
        || (is_alignment(id) && tokens.0.iter().any(|token| matches!(token, TokenOrValue::Token(Token::Ident(name)) if is_anchor_center(name)))))
}

pub(crate) fn source_uses_anchor(id: &PropertyId<'_>, source: &str) -> bool {
    if !is_context(id)
        || (!source.contains('\\')
            && !source
                .as_bytes()
                .windows(6)
                .any(|part| part.eq_ignore_ascii_case(b"anchor")))
    {
        return false;
    }
    let mut found = false;
    let _ = numeric_functions::replace(source, |name, _| {
        found |= is_anchor_function(name);
        Ok(None)
    });
    if is_alignment(id) {
        let _ = numeric_functions::replace_identifiers(source, |name| {
            found |= is_anchor_center(name);
            Ok(None)
        });
    }
    found
}

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, source: &str) -> Option<bool> {
    if !source_uses_anchor(id, source) {
        return None;
    }
    if is_alignment(id) {
        let mut input = ParserInput::new(source);
        return Some(
            Parser::new(&mut input)
                .parse_entirely(|parser| alignment(parser, id))
                .is_ok(),
        );
    }
    let lowered = numeric_functions::replace(source, |name, arguments| {
        if !is_anchor_function(name) {
            return Ok(None);
        }
        let size = name.eq_ignore_ascii_case("anchor-size");
        if !size && !is_inset(id) {
            return Err(());
        }
        let mut input = ParserInput::new(arguments);
        Parser::new(&mut input)
            .parse_entirely(|parser| function_arguments(parser, size))
            .map_err(|_| ())?;
        // Validate as a length atom without resolving a target anchor. This
        // temporary value is never passed into the cascade or computed style.
        Ok(Some("calc(1em)".to_owned()))
    });
    let Ok(lowered) = lowered else {
        return Some(false);
    };
    let Ok(lowered) = numeric_source(&lowered) else {
        return Some(false);
    };
    Some(property_value_syntax(id.name(), &lowered) == PropertyValueSyntax::Valid)
}

fn numeric_source(source: &str) -> Result<String, ()> {
    let source = tree_counting::lower(source, |_| 1)?;
    let source = random_math::for_syntax(&source)?;
    Ok(sign::lower(&source, |_| Some(1.0))?.into_owned())
}

fn function_arguments<'i>(
    parser: &mut Parser<'i, '_>,
    size: bool,
) -> Result<(), ParseError<'i, ()>> {
    let name_first = parser.try_parse(DashedIdent::parse).is_ok();
    let component = if size {
        parser.try_parse(size_keyword).is_ok()
    } else {
        side(parser)?;
        true
    };
    let name = name_first || (component && parser.try_parse(DashedIdent::parse).is_ok());
    if parser.is_exhausted() {
        return Ok(());
    }
    if name || component {
        parser.expect_comma()?;
    }
    fallback(parser)
}

fn size_keyword<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let name = parser.expect_ident()?;
    if [
        "width",
        "height",
        "block",
        "inline",
        "self-block",
        "self-inline",
    ]
    .iter()
    .any(|keyword| name.eq_ignore_ascii_case(keyword))
    {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn side<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| {
            let name = parser.expect_ident()?;
            if [
                "inside",
                "outside",
                "top",
                "right",
                "bottom",
                "left",
                "start",
                "end",
                "self-start",
                "self-end",
                "center",
            ]
            .iter()
            .any(|keyword| name.eq_ignore_ascii_case(keyword))
            {
                Ok(())
            } else {
                Err(parser.new_custom_error::<(), ()>(()))
            }
        })
        .is_ok()
    {
        return Ok(());
    }
    let source = numeric_component(parser)?;
    let source = numeric_source(source).map_err(|_| parser.new_custom_error(()))?;
    let mut input = ParserInput::new(&source);
    Parser::new(&mut input)
        .parse_entirely(parse_percentage_value)
        .map(|_| ())
        .map_err(|_| parser.new_custom_error(()))
}

fn fallback<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let start = parser.position();
    urls::consume(parser, 0, false)?;
    let source =
        numeric_source(parser.slice_from(start)).map_err(|_| parser.new_custom_error(()))?;
    if LengthPercentage::parse_string(&source).is_ok_and(|value| length_percentage_is_valid(&value))
        && literal_numeric_tokens_are_valid(&source, false, false)
    {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn alignment<'i>(
    parser: &mut Parser<'i, '_>,
    id: &PropertyId<'_>,
) -> Result<(), ParseError<'i, ()>> {
    if matches!(id, PropertyId::JustifySelf) {
        return self_alignment(parser, false);
    }
    self_alignment(parser, true)?;
    if matches!(id, PropertyId::PlaceSelf) && !parser.is_exhausted() {
        self_alignment(parser, false)?;
    }
    Ok(())
}

fn self_alignment<'i>(parser: &mut Parser<'i, '_>, align: bool) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("anchor-center"))
        .is_ok()
    {
        return Ok(());
    }
    if align {
        AlignSelf::parse(parser)
            .map(|_| ())
            .map_err(|_| parser.new_custom_error(()))
    } else {
        JustifySelf::parse(parser)
            .map(|_| ())
            .map_err(|_| parser.new_custom_error(()))
    }
}
