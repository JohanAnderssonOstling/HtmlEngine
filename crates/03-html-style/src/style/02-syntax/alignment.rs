//! Compatibility grammar for the standalone legacy alignment marker.

use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::PropertyId;
use lightningcss::properties::align::AlignItems;
use lightningcss::traits::Parse;

pub(crate) enum LegacyItems {
    Justify,
    Place(AlignItems),
}

pub(crate) fn legacy_items(id: &PropertyId<'_>, source: &str) -> Option<LegacyItems> {
    if !matches!(id, PropertyId::JustifyItems | PropertyId::PlaceItems) {
        return None;
    }
    let mut input = ParserInput::new(source);
    Parser::new(&mut input)
        .parse_entirely(|parser| -> Result<LegacyItems, ParseError<'_, ()>> {
            let value = match id {
                PropertyId::JustifyItems => LegacyItems::Justify,
                PropertyId::PlaceItems => LegacyItems::Place(
                    AlignItems::parse(parser).map_err(|_| parser.new_custom_error(()))?,
                ),
                _ => return Err(parser.new_custom_error(())),
            };
            parser.expect_ident_matching("legacy")?;
            Ok(value)
        })
        .ok()
}
