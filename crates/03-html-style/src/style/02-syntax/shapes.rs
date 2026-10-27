//! Basic-shape syntax for clipping, independent of geometry and painting.

mod svg_path;

use super::{geometry, numeric_functions, urls};
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::properties::masking::GeometryBox;
use lightningcss::traits::Parse;
use lightningcss::values::angle::Angle;
use lightningcss::values::shape::FillRule;

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    if !matches!(id, PropertyId::ClipPath(_)) {
        return None;
    }
    let mut input = ParserInput::new(value);
    Some(Parser::new(&mut input).parse_entirely(clip_path).is_ok())
}

fn clip_path<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("none"))
        .is_ok()
        || parser.try_parse(urls::parse).is_ok()
    {
        return Ok(());
    }
    let reference_box = parser.try_parse(GeometryBox::parse).is_ok();
    if reference_box && parser.is_exhausted() {
        return Ok(());
    }
    basic_shape(parser)?;
    if !reference_box && !parser.is_exhausted() {
        GeometryBox::parse(parser).map_err(|_| parser.new_custom_error(()))?;
    }
    Ok(())
}

fn basic_shape<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let name = parser.expect_function()?.clone();
    let arguments = parser.parse_nested_block(|parser| {
        let start = parser.position();
        urls::consume(parser, 0, false)?;
        Ok(parser.slice_from(start))
    })?;
    if name.eq_ignore_ascii_case("path") {
        let mut input = ParserInput::new(arguments);
        return Parser::new(&mut input)
            .parse_entirely(path)
            .map_err(|_| parser.new_custom_error(()));
    }
    if [
        "rect", "xywh", "shape", "inset", "circle", "ellipse", "polygon",
    ]
    .iter()
    .any(|keyword| name.eq_ignore_ascii_case(keyword))
    {
        // These functions contain only typed shape arguments, never opaque
        // resource modifiers. Quoted SVG path data takes the separate path above.
        let valid = numeric_functions::syntax_value_is_valid(arguments, |arguments| {
            let mut input = ParserInput::new(arguments);
            Parser::new(&mut input)
                .parse_entirely(|parser| {
                    if name.eq_ignore_ascii_case("shape") {
                        shape(parser)
                    } else if name.eq_ignore_ascii_case("inset") {
                        inset(parser)
                    } else if name.eq_ignore_ascii_case("circle")
                        || name.eq_ignore_ascii_case("ellipse")
                    {
                        radial_shape(parser, name.eq_ignore_ascii_case("ellipse"))
                    } else if name.eq_ignore_ascii_case("polygon") {
                        polygon(parser)
                    } else {
                        rectangle(parser, name.eq_ignore_ascii_case("xywh"))
                    }
                })
                .is_ok()
        });
        return if valid {
            Ok(())
        } else {
            Err(parser.new_custom_error(()))
        };
    }
    Err(parser.new_custom_error(()))
}

fn length_percentage<'i>(
    parser: &mut Parser<'i, '_>,
    non_negative: bool,
) -> Result<(), ParseError<'i, ()>> {
    geometry::length_percentage(parser, non_negative)
}

fn inset<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    length_percentage(parser, false)?;
    for _ in 1..4 {
        if parser
            .try_parse(|parser| length_percentage(parser, false))
            .is_err()
        {
            break;
        }
    }
    rounded_corners(parser)
}

fn radial_size<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| {
            let name = parser.expect_ident()?;
            if name.eq_ignore_ascii_case("closest-side")
                || name.eq_ignore_ascii_case("farthest-side")
            {
                Ok(())
            } else {
                Err(parser.new_custom_error::<(), ()>(()))
            }
        })
        .is_ok()
    {
        Ok(())
    } else {
        length_percentage(parser, true)
    }
}

fn radial_shape<'i>(parser: &mut Parser<'i, '_>, ellipse: bool) -> Result<(), ParseError<'i, ()>> {
    if parser.try_parse(radial_size).is_ok() && ellipse {
        radial_size(parser)?;
    }
    if !parser.is_exhausted() {
        parser.expect_ident_matching("at")?;
        position(parser)?;
    }
    Ok(())
}

fn polygon<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let fill_rule = parser.try_parse(FillRule::parse).is_ok();
    let rounded = parser
        .try_parse(|parser| parser.expect_ident_matching("round"))
        .is_ok();
    if rounded {
        geometry::length(parser, true)?;
    }
    if fill_rule || rounded {
        parser.expect_comma()?;
    }
    parser.parse_comma_separated(coordinate_pair)?;
    Ok(())
}

fn rounded_corners<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if !parser.is_exhausted() {
        parser.expect_ident_matching("round")?;
        radius_list(parser)?;
        if parser.try_parse(|parser| parser.expect_delim('/')).is_ok() {
            radius_list(parser)?;
        }
    }
    Ok(())
}

fn rectangle<'i>(parser: &mut Parser<'i, '_>, xywh: bool) -> Result<(), ParseError<'i, ()>> {
    for index in 0..4 {
        if !xywh
            && parser
                .try_parse(|parser| parser.expect_ident_matching("auto"))
                .is_ok()
        {
            continue;
        }
        length_percentage(parser, xywh && index >= 2)?;
    }
    rounded_corners(parser)
}

fn radius_list<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    length_percentage(parser, true)?;
    for _ in 1..4 {
        if parser
            .try_parse(|parser| length_percentage(parser, true))
            .is_err()
        {
            break;
        }
    }
    Ok(())
}

fn path<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    if parser.try_parse(FillRule::parse).is_ok() {
        parser.expect_comma()?;
    }
    let source = parser.expect_string()?;
    if svg_path::is_valid(source) {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn shape<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let _ = parser.try_parse(FillRule::parse);
    parser.expect_ident_matching("from")?;
    position(parser)?;
    parser.expect_comma()?;
    parser.parse_comma_separated(command)?;
    Ok(())
}

fn position<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    geometry::position(parser, false)
}

fn coordinate_pair<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    length_percentage(parser, false)?;
    length_percentage(parser, false)
}

/// Return whether this command uses relative coordinates.
fn end_point<'i>(parser: &mut Parser<'i, '_>) -> Result<bool, ParseError<'i, ()>> {
    if parser
        .try_parse(|parser| parser.expect_ident_matching("by"))
        .is_ok()
    {
        coordinate_pair(parser)?;
        Ok(true)
    } else {
        parser.expect_ident_matching("to")?;
        position(parser)?;
        Ok(false)
    }
}

fn control_point<'i>(
    parser: &mut Parser<'i, '_>,
    relative: bool,
) -> Result<(), ParseError<'i, ()>> {
    // A coordinate pair may specify its reference point explicitly even in
    // an absolute curve. Position keywords cannot take a reference point.
    if parser.try_parse(coordinate_pair).is_ok() {
        if parser
            .try_parse(|parser| parser.expect_ident_matching("from"))
            .is_ok()
        {
            let reference = parser.expect_ident()?;
            if !["start", "end", "origin"]
                .iter()
                .any(|name| reference.eq_ignore_ascii_case(name))
            {
                return Err(parser.new_custom_error(()));
            }
        }
        Ok(())
    } else if !relative {
        position(parser)
    } else {
        Err(parser.new_custom_error(()))
    }
}

fn command<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let name = parser.expect_ident()?.clone();
    if name.eq_ignore_ascii_case("close") {
        return Ok(());
    }
    if name.eq_ignore_ascii_case("hline") || name.eq_ignore_ascii_case("vline") {
        if parser
            .try_parse(|parser| parser.expect_ident_matching("by"))
            .is_ok()
        {
            return length_percentage(parser, false);
        }
        parser.expect_ident_matching("to")?;
        let keywords = if name.eq_ignore_ascii_case("hline") {
            ["left", "center", "right", "x-start", "x-end"]
        } else {
            ["top", "center", "bottom", "y-start", "y-end"]
        };
        if parser
            .try_parse(|parser| {
                let name = parser.expect_ident()?;
                if keywords
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
        return length_percentage(parser, false);
    }
    if !["move", "line", "curve", "smooth", "arc"]
        .iter()
        .any(|keyword| name.eq_ignore_ascii_case(keyword))
    {
        return Err(parser.new_custom_error(()));
    }
    let relative = end_point(parser)?;
    if name.eq_ignore_ascii_case("curve") || name.eq_ignore_ascii_case("smooth") {
        if parser
            .try_parse(|parser| parser.expect_ident_matching("with"))
            .is_ok()
        {
            control_point(parser, relative)?;
            if name.eq_ignore_ascii_case("curve")
                && parser.try_parse(|parser| parser.expect_delim('/')).is_ok()
            {
                control_point(parser, relative)?;
            }
        } else if name.eq_ignore_ascii_case("curve") {
            return Err(parser.new_custom_error(()));
        }
    } else if name.eq_ignore_ascii_case("arc") {
        arc_options(parser)?;
    }
    Ok(())
}

fn arc_options<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    let (mut radii, mut sweep, mut size, mut rotation) = (false, false, false, false);
    while !parser.is_exhausted() {
        let keyword = parser.expect_ident()?.clone();
        if keyword.eq_ignore_ascii_case("of") && !radii {
            length_percentage(parser, false)?;
            let _ = parser.try_parse(|parser| length_percentage(parser, false));
            radii = true;
        } else if ["cw", "ccw"]
            .iter()
            .any(|name| keyword.eq_ignore_ascii_case(name))
            && !sweep
        {
            sweep = true;
        } else if ["large", "small"]
            .iter()
            .any(|name| keyword.eq_ignore_ascii_case(name))
            && !size
        {
            size = true;
        } else if keyword.eq_ignore_ascii_case("rotate") && !rotation {
            Angle::parse(parser).map_err(|_| parser.new_custom_error(()))?;
            rotation = true;
        } else {
            return Err(parser.new_custom_error(()));
        }
    }
    if radii {
        Ok(())
    } else {
        Err(parser.new_custom_error(()))
    }
}
