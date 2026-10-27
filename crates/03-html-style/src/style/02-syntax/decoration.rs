//! Outline and emphasis grammars beyond the native parser's coverage.

use super::{box_syntax, color_syntax, sign};
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::properties::border::{BorderSideWidth, LineStyle};
use lightningcss::properties::text::TextEmphasisPosition;
use lightningcss::traits::Parse;

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    if !matches!(
        id,
        PropertyId::Outline | PropertyId::OutlineColor | PropertyId::TextEmphasisPosition(_)
    ) {
        return None;
    }
    let Ok(value) = sign::lower(value, |_| Some(1.0)) else {
        return Some(false);
    };
    let mut input = ParserInput::new(&value);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| match id {
                PropertyId::Outline => outline(parser),
                PropertyId::OutlineColor => {
                    if parser
                        .try_parse(|parser| parser.expect_ident_matching("auto"))
                        .is_ok()
                    {
                        Ok(())
                    } else {
                        color_syntax::color(parser, 0)
                    }
                }
                PropertyId::TextEmphasisPosition(_) => {
                    // CSSWG resolved to add auto in September 2024; the
                    // published grammar has not yet incorporated that change.
                    // https://www.w3.org/2024/09/18-css-minutes.html#t04
                    if parser
                        .try_parse(|parser| parser.expect_ident_matching("auto"))
                        .is_ok()
                    {
                        Ok(())
                    } else {
                        TextEmphasisPosition::parse(parser)
                            .map(|_| ())
                            .map_err(|_| parser.new_custom_error(()))
                    }
                }
                _ => unreachable!("guarded decoration property"),
            })
            .is_ok(),
    )
}

fn outline<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let (mut width, mut style, mut color, mut autos) = (false, false, false, 0);
    while !parser.is_exhausted() {
        if parser
            .try_parse(|parser| parser.expect_ident_matching("auto"))
            .is_ok()
        {
            // Either color or style can be auto, in either order. Delay
            // assigning that slot until all explicit components are known.
            autos += 1;
            if autos > 2 {
                return Err(parser.new_custom_error(()));
            }
        } else if !width && parser.try_parse(outline_width).is_ok() {
            width = true;
        } else if !style && parser.try_parse(outline_style).is_ok() {
            style = true;
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
    if (width || style || color || autos != 0) && autos + u8::from(style) + u8::from(color) <= 2 {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn outline_style<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    match LineStyle::parse(parser) {
        Ok(value) if !matches!(value, LineStyle::Hidden) => Ok(()),
        _ => Err(parser.new_custom_error(())),
    }
}

fn outline_width<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let start = parser.position();
    BorderSideWidth::parse(parser).map_err(|_| parser.new_custom_error(()))?;
    if box_syntax::property_value_is_valid(&PropertyId::OutlineWidth, parser.slice_from(start))
        == Some(true)
    {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}
