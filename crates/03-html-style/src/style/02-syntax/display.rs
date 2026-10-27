//! Display grammar extensions independent of formatting-context support.

use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::properties::display::DisplayOutside;
use lightningcss::traits::Parse;

fn is_grid_lanes_keyword(name: &str) -> bool {
    name.eq_ignore_ascii_case("grid-lanes") || name.eq_ignore_ascii_case("inline-grid-lanes")
}

pub(crate) fn tokens_use_grid_lanes(tokens: &TokenList<'_>) -> bool {
    tokens.0.iter().any(|token| {
        matches!(token,
        TokenOrValue::Token(Token::Ident(name)) if is_grid_lanes_keyword(name))
    })
}

pub(crate) fn grid_lanes_is_valid(source: &str) -> bool {
    if !source.contains('\\')
        && !source
            .as_bytes()
            .windows(10)
            .any(|part| part.eq_ignore_ascii_case(b"grid-lanes"))
    {
        return false;
    }
    let mut input = ParserInput::new(source);
    Parser::new(&mut input)
        .parse_entirely(|parser| -> Result<(), ParseError<'_, ()>> {
            // The legacy inline spelling is a complete value. The inside
            // keyword can pair with one outside role in either order.
            if parser
                .try_parse(|parser| parser.expect_ident_matching("inline-grid-lanes"))
                .is_ok()
            {
                return Ok(());
            }
            let outside_first = parser.try_parse(DisplayOutside::parse).is_ok();
            parser.expect_ident_matching("grid-lanes")?;
            if !outside_first && !parser.is_exhausted() {
                DisplayOutside::parse(parser).map_err(|_| parser.new_custom_error(()))?;
            }
            Ok(())
        })
        .is_ok()
}
