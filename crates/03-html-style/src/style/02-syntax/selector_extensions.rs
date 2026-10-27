//! Syntax-only recovery for pseudo-element chains the native parser predates.
//! The projection is never used to parse style rules or to match elements.

use super::urls;
use crate::style::matching::selectors::{search_text_state_is_valid, selector_list_is_web_valid};
use cssparser::{ParseError, Parser, ParserInput, Token};
use lightningcss::selector::{Component, PseudoClass, SelectorList};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::ParseWithOptions;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Before,
    After,
    Marker,
    Slotted,
    Picker,
    Tree,
    Part,
    Column,
    ScrollMarker,
    SearchText,
    Other,
}

struct Pseudo {
    kind: Kind,
    head: Range<usize>,
}

struct Edit {
    range: Range<usize>,
    replacement: String,
}

pub(crate) fn is_valid(source: &str) -> bool {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    let mut pseudos = Vec::new();
    let mut edits = Vec::new();
    loop {
        let start = parser.position().byte_index();
        let token = match parser.next_including_whitespace_and_comments() {
            Ok(token) => token.clone(),
            Err(_) => break,
        };
        match token {
            Token::WhiteSpace(_) | Token::Comma | Token::Delim('>' | '+' | '~') => {
                if !compound(source, start, &pseudos, &mut edits) {
                    return false;
                }
                pseudos.clear();
            }
            Token::Colon => match pseudo(&mut parser, start) {
                Ok(Some(pseudo)) => pseudos.push(pseudo),
                Ok(None) => {}
                Err(_) => return false,
            },
            Token::Function(_)
            | Token::ParenthesisBlock
            | Token::SquareBracketBlock
            | Token::CurlyBracketBlock => {
                // Selector-valued function arguments retain the dependency's
                // context checks. Never repair pseudo-elements inside :not,
                // :has, ::slotted arguments, attribute strings, or comments.
                if parser
                    .parse_nested_block(|parser| urls::consume(parser, 0, false).map(|_| ()))
                    .is_err()
                {
                    return false;
                }
            }
            _ => {}
        }
    }
    if !compound(source, source.len(), &pseudos, &mut edits) || edits.is_empty() {
        return false;
    }
    let mut projection = String::with_capacity(source.len());
    let mut copied = 0;
    for edit in edits {
        projection.push_str(&source[copied..edit.range.start]);
        projection.push_str(&edit.replacement);
        copied = edit.range.end;
    }
    projection.push_str(&source[copied..]);
    native_is_valid(&projection)
}

fn pseudo<'i>(
    parser: &mut Parser<'i, '_>,
    start: usize,
) -> Result<Option<Pseudo>, ParseError<'i, ()>> {
    let double = parser.try_parse(|parser| parser.expect_colon()).is_ok();
    let (name, function) = match parser.next()?.clone() {
        Token::Ident(name) => (name, false),
        Token::Function(name) => (name, true),
        _ => return Err(parser.new_custom_error(())),
    };
    if function {
        parser.parse_nested_block(|parser| urls::consume(parser, 0, false).map(|_| ()))?;
    }
    let legacy = !function
        && ["before", "after", "first-line", "first-letter"]
            .iter()
            .any(|keyword| name.eq_ignore_ascii_case(keyword));
    if !double && !legacy {
        return Ok(None);
    }
    let kind = if function {
        if name.eq_ignore_ascii_case("slotted") {
            Kind::Slotted
        } else if name.eq_ignore_ascii_case("picker") {
            Kind::Picker
        } else if name.eq_ignore_ascii_case("part") {
            Kind::Part
        } else {
            Kind::Other
        }
    } else if name.eq_ignore_ascii_case("before") {
        Kind::Before
    } else if name.eq_ignore_ascii_case("after") {
        Kind::After
    } else if name.eq_ignore_ascii_case("marker") {
        Kind::Marker
    } else if name.eq_ignore_ascii_case("column") {
        Kind::Column
    } else if name.eq_ignore_ascii_case("scroll-marker") {
        Kind::ScrollMarker
    } else if name.eq_ignore_ascii_case("search-text") {
        Kind::SearchText
    } else if [
        "details-content",
        "placeholder",
        "file-selector-button",
        "picker-icon",
        "checkmark",
    ]
    .iter()
    .any(|keyword| name.eq_ignore_ascii_case(keyword))
    {
        Kind::Tree
    } else {
        Kind::Other
    };
    Ok(Some(Pseudo {
        kind,
        head: start..parser.position().byte_index(),
    }))
}

fn native_is_valid(source: &str) -> bool {
    SelectorList::parse_string_with_options(source, ParserOptions::default())
        .is_ok_and(|selectors| selector_list_is_web_valid(&selectors))
}

fn follows(parent: Kind, child: Kind) -> bool {
    match parent {
        Kind::Before | Kind::After => matches!(child, Kind::Marker | Kind::Column),
        Kind::Slotted => matches!(
            child,
            Kind::Before | Kind::After | Kind::Marker | Kind::Tree | Kind::Picker
        ),
        Kind::Picker => matches!(
            child,
            Kind::Before | Kind::After | Kind::Marker | Kind::Tree | Kind::Picker
        ),
        Kind::Part => !matches!(child, Kind::Slotted | Kind::Part | Kind::Column),
        Kind::Column => child == Kind::ScrollMarker,
        _ => false,
    }
}

fn compound(source: &str, end: usize, pseudos: &[Pseudo], edits: &mut Vec<Edit>) -> bool {
    let Some(last) = pseudos.last() else {
        return true;
    };
    for (index, pseudo) in pseudos.iter().enumerate() {
        let next = pseudos.get(index + 1);
        let tail_end = next.map_or(end, |pseudo| pseudo.head.start);
        let tail = &source[pseudo.head.end..tail_end];
        let valid = match pseudo.kind {
            Kind::Column => trivia_only(tail),
            Kind::ScrollMarker => native_is_valid(&format!("::marker{tail}")),
            Kind::SearchText => search_states_are_valid(tail),
            Kind::Part => {
                native_is_valid(&source[pseudo.head.clone()]) && part_states_are_valid(tail)
            }
            _ => native_is_valid(&source[pseudo.head.start..tail_end]),
        };
        if !valid || next.is_some_and(|child| !follows(pseudo.kind, child.kind)) {
            return false;
        }
    }
    let tail = &source[last.head.end..end];
    let replacement = match last.kind {
        Kind::Column => "::marker".to_owned(),
        Kind::ScrollMarker => format!("::marker{tail}"),
        Kind::SearchText => source[last.head.clone()].to_owned(),
        Kind::Part if !trivia_only(tail) => "::marker".to_owned(),
        _ if pseudos.len() > 1 => source[last.head.start..end].to_owned(),
        _ => return true,
    };
    // Replacing a validated chain with its final pseudo-element preserves
    // the outer selector grammar and pseudo-element placement constraints.
    // Column, scroll-marker and part-state proxies retain that subject restriction.
    edits.push(Edit {
        range: pseudos[0].head.start..end,
        replacement,
    });
    true
}

fn trivia_only(source: &str) -> bool {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    while let Ok(token) = parser.next_including_whitespace_and_comments() {
        if !matches!(token, Token::WhiteSpace(_) | Token::Comment(_)) {
            return false;
        }
    }
    true
}

fn search_states_are_valid(source: &str) -> bool {
    SelectorList::parse_string_with_options(&format!("*{source}"), ParserOptions::default())
        .is_ok_and(|selectors| {
            selectors.0.len() == 1
                && selectors.0[0]
                    .iter_raw_match_order()
                    .all(|component| match component {
                        Component::ExplicitUniversalType => true,
                        _ => search_text_state_is_valid(component, false),
                    })
        })
}

fn part_states_are_valid(source: &str) -> bool {
    SelectorList::parse_string_with_options(&format!("*{source}"), ParserOptions::default())
        .is_ok_and(|selectors| {
            selectors.0.len() == 1
                && selector_list_is_web_valid(&selectors)
                && selectors.0[0]
                    .iter_raw_match_order()
                    .all(|component| match component {
                        Component::ExplicitUniversalType => true,
                        _ => part_state_is_valid(component),
                    })
        })
}

fn part_state_is_valid(component: &Component<'_>) -> bool {
    match component {
        Component::NonTSPseudoClass(PseudoClass::CustomFunction { .. }) => false,
        Component::NonTSPseudoClass(PseudoClass::Custom { name }) => {
            name.eq_ignore_ascii_case("xr-overlay")
        }
        Component::NonTSPseudoClass(_) => true,
        Component::Negation(selectors) => {
            !selectors.is_empty()
                && selectors
                    .iter()
                    .all(|selector| selector.iter_raw_match_order().all(part_state_is_valid))
        }
        // Structural arguments are discarded by these forgiving lists, just
        // as they are in the native ::part() grammar. Negation is unforgiving.
        Component::Is(_) | Component::Where(_) => true,
        _ => false,
    }
}
