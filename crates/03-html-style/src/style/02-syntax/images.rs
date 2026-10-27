//! Image and cursor grammar, independent of image generation and fetching.

use super::color_syntax::{color, mix_percentage};
use super::value::{numeric_component, parse_number_value};
use super::{sign, urls};
use cssparser::{ParseError, Parser, ParserInput, Token};
use lightningcss::properties::PropertyId;
use lightningcss::properties::ui::CursorKeyword;
use lightningcss::traits::Parse;
use lightningcss::values::image::Image;
use lightningcss::values::resolution::Resolution;

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    if !matches!(
        id,
        PropertyId::ImageRendering
            | PropertyId::BackgroundImage
            | PropertyId::ListStyleImage
            | PropertyId::BorderImageSource
            | PropertyId::MaskImage(_)
            | PropertyId::Cursor
    ) {
        return None;
    }
    let mut input = ParserInput::new(value);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| match id {
                PropertyId::ImageRendering => image_rendering(parser),
                PropertyId::BackgroundImage | PropertyId::MaskImage(_) => {
                    parser.parse_comma_separated(image_or_none)?;
                    Ok(())
                }
                PropertyId::Cursor => cursor(parser),
                _ => image_or_none(parser),
            })
            .is_ok(),
    )
}

fn image_rendering<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser.expect_ident().is_ok_and(|name| {
        [
            "auto",
            "smooth",
            "high-quality",
            "pixelated",
            "crisp-edges",
            "optimizeSpeed",
            "optimizeQuality",
        ]
        .iter()
        .any(|keyword| name.eq_ignore_ascii_case(keyword))
    }) {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

pub(super) fn image_or_none<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    image_or_none_at(parser, 0, false)
}

fn image_or_none_at<'i>(
    parser: &mut Parser<'i, '_>,
    depth: usize,
    inside_set: bool,
) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("none"))
        .is_ok()
    {
        Ok(())
    } else {
        image(parser, depth, inside_set)
    }
}

fn image<'i>(
    parser: &mut Parser<'i, '_>,
    depth: usize,
    inside_set: bool,
) -> Result<(), ParseError<'i, ()>> {
    if depth >= 128 {
        return Err(parser.new_custom_error(()));
    }
    if parser.try_parse(urls::parse).is_ok() {
        return Ok(());
    }
    let state = parser.state();
    let name = match parser.next()?.clone() {
        Token::Function(name) => name,
        _ => return Err(parser.new_custom_error(())),
    };
    if name.eq_ignore_ascii_case("image") {
        parser.parse_nested_block(|parser| color(parser, depth + 1))
    } else if name.eq_ignore_ascii_case("cross-fade") {
        parser.parse_nested_block(|parser| {
            parser.parse_comma_separated(|parser| {
                let percentage_first = parser.try_parse(mix_percentage).is_ok();
                if parser
                    .try_parse(|parser| image(parser, depth + 1, inside_set))
                    .is_err()
                {
                    color(parser, depth + 1)?;
                }
                if !percentage_first && !parser.is_exhausted() {
                    mix_percentage(parser)?;
                }
                Ok(())
            })?;
            Ok(())
        })
    } else if name.eq_ignore_ascii_case("light-dark") {
        parser.parse_nested_block(|parser| {
            image_or_none_at(parser, depth + 1, inside_set)?;
            parser.expect_comma()?;
            image_or_none_at(parser, depth + 1, inside_set)
        })
    } else if name.eq_ignore_ascii_case("image-set")
        || name.eq_ignore_ascii_case("-webkit-image-set")
    {
        if inside_set {
            return Err(parser.new_custom_error(()));
        }
        parser.parse_nested_block(|parser| {
            parser.parse_comma_separated(|parser| image_set_option(parser, depth + 1))?;
            Ok(())
        })
    } else if name.eq_ignore_ascii_case("paint") {
        parser.parse_nested_block(|parser| {
            parser.expect_ident()?;
            if !parser.is_exhausted() {
                parser.expect_comma()?;
                if urls::consume(parser, 0, true)? == 0 {
                    return Err(parser.new_custom_error(()));
                }
            }
            Ok(())
        })
    } else if name.eq_ignore_ascii_case("element") || name.eq_ignore_ascii_case("-moz-element") {
        parser.parse_nested_block(|parser| match parser.next()? {
            Token::IDHash(_) => Ok(()),
            _ => Err(parser.new_custom_error(())),
        })
    } else {
        // Delegate gradients and existing native extensions to their own
        // typed grammar. Unknown functions cannot become valid images.
        parser.reset(&state);
        match Image::parse(parser).map_err(|_| parser.new_custom_error(()))? {
            Image::None => Err(parser.new_custom_error(())),
            _ => Ok(()),
        }
    }
}

fn image_set_option<'i>(
    parser: &mut Parser<'i, '_>,
    depth: usize,
) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_string().map(|_| ()))
        .is_err()
    {
        image(parser, depth, true)?;
    }
    let mut resolution = false;
    let mut file_type = false;
    while !parser.is_exhausted() {
        if !resolution {
            if let Ok(value) = parser.try_parse(Resolution::parse) {
                let (Resolution::Dpi(value) | Resolution::Dpcm(value) | Resolution::Dppx(value)) =
                    value;
                if value < 0.0 {
                    return Err(parser.new_custom_error(()));
                }
                resolution = true;
                continue;
            }
        }
        if file_type {
            return Err(parser.new_custom_error(()));
        }
        parser.expect_function_matching("type")?;
        parser.parse_nested_block(|parser| {
            parser.expect_string()?;
            Ok(())
        })?;
        file_type = true;
    }
    Ok(())
}

fn cursor<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    while parser.try_parse(|parser| image(parser, 0, false)).is_ok() {
        if parser.try_parse(|parser| parser.expect_comma()).is_err() {
            hotspot_number(parser)?;
            hotspot_number(parser)?;
            parser.expect_comma()?;
        }
    }
    CursorKeyword::parse(parser)
        .map(|_| ())
        .map_err(|_| parser.new_custom_error(()))
}

fn hotspot_number<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let source = numeric_component(parser)?;
    let value = sign::lower(source, |_| Some(1.0)).map_err(|_| parser.new_custom_error(()))?;
    let mut input = ParserInput::new(&value);
    Parser::new(&mut input)
        .parse_entirely(parse_number_value)
        .map(|_| ())
        .map_err(|_| parser.new_custom_error(()))
}
