//! Scroll-area offset grammars, independent of scrolling behavior.

use super::sign;
use super::value::{length_is_valid, length_percentage_is_valid, literal_numeric_tokens_are_valid};
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::traits::Parse;
use lightningcss::values::length::{Length, LengthPercentage};

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    let (padding, maximum) = match id {
        PropertyId::ScrollMargin => (false, 4),
        PropertyId::ScrollMarginBlock | PropertyId::ScrollMarginInline => (false, 2),
        PropertyId::ScrollMarginTop
        | PropertyId::ScrollMarginRight
        | PropertyId::ScrollMarginBottom
        | PropertyId::ScrollMarginLeft
        | PropertyId::ScrollMarginBlockStart
        | PropertyId::ScrollMarginBlockEnd
        | PropertyId::ScrollMarginInlineStart
        | PropertyId::ScrollMarginInlineEnd => (false, 1),
        PropertyId::ScrollPadding => (true, 4),
        PropertyId::ScrollPaddingBlock | PropertyId::ScrollPaddingInline => (true, 2),
        PropertyId::ScrollPaddingTop
        | PropertyId::ScrollPaddingRight
        | PropertyId::ScrollPaddingBottom
        | PropertyId::ScrollPaddingLeft
        | PropertyId::ScrollPaddingBlockStart
        | PropertyId::ScrollPaddingBlockEnd
        | PropertyId::ScrollPaddingInlineStart
        | PropertyId::ScrollPaddingInlineEnd => (true, 1),
        _ => return None,
    };
    let Ok(value) = sign::lower(value, |_| Some(1.0)) else {
        return Some(false);
    };
    let mut input = ParserInput::new(&value);
    Some(
        Parser::new(&mut input)
            .parse_entirely(|parser| values(parser, padding, maximum))
            .is_ok(),
    )
}

fn values<'i>(
    parser: &mut Parser<'i, '_>,
    padding: bool,
    maximum: usize,
) -> Result<(), ParseError<'i, ()>> {
    let mut count = 0;
    while !parser.is_exhausted() {
        if count == maximum {
            return Err(parser.new_custom_error(()));
        }
        count += 1;
        if padding
            && parser
                .try_parse(|parser| parser.expect_ident_matching("auto"))
                .is_ok()
        {
            continue;
        }
        let start = parser.position();
        let valid = if padding {
            let value = LengthPercentage::parse(parser).map_err(|_| parser.new_custom_error(()))?;
            length_percentage_is_valid(&value)
        } else {
            let value = Length::parse(parser).map_err(|_| parser.new_custom_error(()))?;
            length_is_valid(&value)
        };
        // Calculated negative values are clamped at computation. Literal
        // negative padding and nonzero unitless lengths are parse errors.
        if !valid || !literal_numeric_tokens_are_valid(parser.slice_from(start), padding, false) {
            return Err(parser.new_custom_error(()));
        }
    }
    if count == 0 {
        Err(parser.new_custom_error(()))
    } else {
        Ok(())
    }
}
