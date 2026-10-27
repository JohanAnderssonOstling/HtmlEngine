//! Background and border-image grammar, separate from painting support.

use super::geometry::{self, Numeric, numeric};
use super::{color_syntax, images};
use cssparser::{ParseError, Parser, ParserInput, match_ignore_ascii_case};
use lightningcss::properties::PropertyId;
use lightningcss::properties::background::{
    BackgroundAttachment, BackgroundOrigin, BackgroundRepeat,
};
use lightningcss::properties::border_image::BorderImageRepeat;
use lightningcss::properties::masking::{GeometryBox, MaskComposite, MaskMode};
use lightningcss::traits::Parse;

pub(crate) fn supports(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::Background
            | PropertyId::BackgroundClip(_)
            | PropertyId::Mask(_)
            | PropertyId::BorderImage(_)
            | PropertyId::BorderImageSlice
            | PropertyId::BorderImageWidth
            | PropertyId::BorderImageOutset
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
                    PropertyId::BackgroundClip(_) => {
                        parser.parse_comma_separated(background_clip)?;
                    }
                    PropertyId::Background => {
                        let layers = parser.parse_comma_separated(background_layer)?;
                        if layers[..layers.len() - 1].contains(&true) {
                            return Err(parser.new_custom_error(()));
                        }
                    }
                    PropertyId::Mask(_) => {
                        parser.parse_comma_separated(mask_layer)?;
                    }
                    PropertyId::BorderImage(_) => border_image(parser)?,
                    PropertyId::BorderImageSlice => slice(parser)?,
                    PropertyId::BorderImageWidth => border_sides(parser, BorderSide::Width)?,
                    PropertyId::BorderImageOutset => border_sides(parser, BorderSide::Outset)?,
                    _ => unreachable!(),
                }
                Ok(())
            })
            .is_ok(),
    )
}

fn background_clip<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let name = parser.expect_ident_cloned()?;
    match_ignore_ascii_case! {name.as_ref(),
        "border-box" | "padding-box" | "content-box" => Ok(()),
        "border-area" => { let _ = parser.try_parse(|parser| parser.expect_ident_matching("text")); Ok(()) },
        "text" => { let _ = parser.try_parse(|parser| parser.expect_ident_matching("border-area")); Ok(()) },
        _ => Err(parser.new_custom_error(())),
    }
}

/// Return whether this layer supplies a color, which is allowed only last.
fn background_layer<'i>(parser: &mut Parser<'i, '_>) -> Result<bool, ParseError<'i, ()>> {
    let (mut color, mut image, mut position, mut repeat, mut attachment, mut origin, mut clip) =
        (false, false, false, false, false, false, false);
    let mut any = false;
    while !parser.is_exhausted() {
        if !color
            && parser
                .try_parse(|parser| color_syntax::color(parser, 0))
                .is_ok()
        {
            color = true;
        } else if !position
            && parser
                .try_parse(|parser| geometry::position(parser, true))
                .is_ok()
        {
            position = true;
            if parser.try_parse(|parser| parser.expect_delim('/')).is_ok() {
                geometry::size(parser)?;
            }
        } else if !image && parser.try_parse(images::image_or_none).is_ok() {
            image = true;
        } else if !repeat && parser.try_parse(BackgroundRepeat::parse).is_ok() {
            repeat = true;
        } else if !attachment && parser.try_parse(BackgroundAttachment::parse).is_ok() {
            attachment = true;
        } else if !origin && parser.try_parse(BackgroundOrigin::parse).is_ok() {
            origin = true;
        } else if !clip && parser.try_parse(background_clip).is_ok() {
            clip = true;
        } else {
            return Err(parser.new_custom_error(()));
        }
        any = true;
    }
    if any {
        Ok(color)
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn mask_box<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    match GeometryBox::parse(parser).map_err(|_| parser.new_custom_error(()))? {
        GeometryBox::MarginBox => Err(parser.new_custom_error(())),
        _ => Ok(()),
    }
}

fn mask_clip<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("no-clip"))
        .is_ok()
    {
        Ok(())
    } else {
        mask_box(parser)
    }
}

fn mask_layer<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let (mut image, mut position, mut repeat, mut origin, mut clip, mut composite, mut mode) =
        (false, false, false, false, false, false, false);
    let mut any = false;
    while !parser.is_exhausted() {
        if !image && parser.try_parse(images::image_or_none).is_ok() {
            image = true;
        } else if !position
            && parser
                .try_parse(|parser| geometry::position(parser, false))
                .is_ok()
        {
            position = true;
            if parser.try_parse(|parser| parser.expect_delim('/')).is_ok() {
                geometry::size(parser)?;
            }
        } else if !repeat && parser.try_parse(BackgroundRepeat::parse).is_ok() {
            repeat = true;
        } else if !origin && parser.try_parse(mask_box).is_ok() {
            origin = true;
        } else if !clip && parser.try_parse(mask_clip).is_ok() {
            clip = true;
        } else if !composite && parser.try_parse(MaskComposite::parse).is_ok() {
            composite = true;
        } else if !mode && parser.try_parse(MaskMode::parse).is_ok() {
            mode = true;
        } else {
            return Err(parser.new_custom_error(()));
        }
        any = true;
    }
    if any {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn slice<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let fill = parser
        .try_parse(|parser| parser.expect_ident_matching("fill"))
        .is_ok();
    numeric(parser, Numeric::NumberPercentage, true)?;
    for _ in 1..4 {
        if parser
            .try_parse(|parser| numeric(parser, Numeric::NumberPercentage, true))
            .is_err()
        {
            break;
        }
    }
    if !fill {
        let _ = parser.try_parse(|parser| parser.expect_ident_matching("fill"));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum BorderSide {
    Width,
    Outset,
}

fn border_side<'i>(
    parser: &mut Parser<'i, '_>,
    side: BorderSide,
) -> Result<(), ParseError<'i, ()>> {
    if matches!(side, BorderSide::Width)
        && parser
            .try_parse(|parser| parser.expect_ident_matching("auto"))
            .is_ok()
    {
        return Ok(());
    }
    if parser
        .try_parse(|parser| numeric(parser, Numeric::Number, true))
        .is_ok()
    {
        return Ok(());
    }
    numeric(
        parser,
        if matches!(side, BorderSide::Width) {
            Numeric::LengthPercentage
        } else {
            Numeric::Length
        },
        true,
    )
}

fn border_sides<'i>(
    parser: &mut Parser<'i, '_>,
    side: BorderSide,
) -> Result<(), ParseError<'i, ()>> {
    border_side(parser, side)?;
    for _ in 1..4 {
        if parser
            .try_parse(|parser| border_side(parser, side))
            .is_err()
        {
            break;
        }
    }
    Ok(())
}

fn border_image<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let (mut source, mut offsets, mut repeat) = (false, false, false);
    let mut any = false;
    while !parser.is_exhausted() {
        if !source && parser.try_parse(images::image_or_none).is_ok() {
            source = true;
        } else if !offsets && parser.try_parse(slice).is_ok() {
            offsets = true;
            if parser.try_parse(|parser| parser.expect_delim('/')).is_ok() {
                let width = parser
                    .try_parse(|parser| border_sides(parser, BorderSide::Width))
                    .is_ok();
                if parser.try_parse(|parser| parser.expect_delim('/')).is_ok() {
                    border_sides(parser, BorderSide::Outset)?;
                } else if !width {
                    return Err(parser.new_custom_error(()));
                }
            }
        } else if !repeat && parser.try_parse(BorderImageRepeat::parse).is_ok() {
            repeat = true;
        } else {
            return Err(parser.new_custom_error(()));
        }
        any = true;
    }
    if any {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}
