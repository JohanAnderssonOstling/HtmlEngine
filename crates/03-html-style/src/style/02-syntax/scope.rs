//! Unforgiving, element-only scope boundaries before native selector recovery.

use crate::style::matching::selectors::selector_list_is_web_valid_for_scope;
use cssparser::{ParseError, Parser, ParserInput};
use lightningcss::selector::SelectorList;
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::ParseWithOptions;

pub(crate) fn prelude_is_valid(source: &str) -> bool {
    let mut input = ParserInput::new(source);
    Parser::new(&mut input)
        .parse_entirely(|parser| {
            if parser
                .try_parse(|parser| parser.expect_parenthesis_block())
                .is_ok()
            {
                parser.parse_nested_block(boundary)?;
            }
            if !parser.is_exhausted() {
                parser.expect_ident_matching("to")?;
                parser.expect_parenthesis_block()?;
                parser.parse_nested_block(boundary)?;
            }
            Ok::<_, ParseError<'_, ()>>(())
        })
        .is_ok()
}

fn boundary<'i>(parser: &mut Parser<'i, '_>) -> Result<(), ParseError<'i, ()>> {
    parser
        .parse_comma_separated(|parser| {
            if parser.is_exhausted() {
                return Err(parser.new_custom_error(()));
            }
            let start = parser.position();
            while parser.next().is_ok() {}
            // Native relative parsing is forgiving. A synthetic ancestor lets its
            // strict selector-list parser validate each original relative member.
            let source = format!(":scope {}", parser.slice_from(start));
            if SelectorList::parse_string_with_options(&source, ParserOptions::default())
                .is_ok_and(|list| selector_list_is_web_valid_for_scope(&list))
            {
                Ok(())
            } else {
                Err(parser.new_custom_error(()))
            }
        })
        .map(|_| ())
}

pub(crate) fn relative_list_is_valid(source: &str) -> bool {
    let mut input = ParserInput::new(source);
    Parser::new(&mut input).parse_entirely(boundary).is_ok()
}
