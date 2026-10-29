use crate::style::syntax::value::{significant_tokens, token_ident};
use cssparser::match_ignore_ascii_case;
use html_style_model::{
    ComputedStylesBuilder, CounterDirective, CounterStyle, GeneratedContent, GeneratedContentItem,
    QuoteStyle,
};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};

/// Convert native generated-content tokens into renderer-owned items.
/// The outer `Option` distinguishes an invalid/unsupported declaration from
/// a valid suppressing value (`normal` or `none`).
pub(super) fn parse_generated_content(
    styles: &mut ComputedStylesBuilder,
    tokens: &TokenList<'_>,
) -> Option<Option<GeneratedContent>> {
    let significant = significant_tokens(tokens);
    let first = significant.clone().next()?;
    let count = significant.clone().count();
    if count == 1
        && let TokenOrValue::Token(Token::Ident(keyword)) = first
    {
        match_ignore_ascii_case! { keyword.as_ref(),
            // `content` is not inherited; the originating element normally
            // computes to `normal` when a pseudo-element explicitly inherits.
            "normal" | "none" | "initial" | "unset" | "inherit" => return Some(None),
            "revert" | "revert-layer" => return None,
            _ => {},
        }
    }
    let mut items = Vec::with_capacity(count);
    for token in significant {
        let item = match token {
            TokenOrValue::Token(Token::String(value)) => {
                GeneratedContentItem::Text(styles.intern_string(value.as_ref()))
            }
            TokenOrValue::Token(Token::Ident(value)) => {
                match_ignore_ascii_case! { value.as_ref(),
                    "open-quote" => GeneratedContentItem::OpenQuote,
                    "close-quote" => GeneratedContentItem::CloseQuote,
                    "no-open-quote" => GeneratedContentItem::NoOpenQuote,
                    "no-close-quote" => GeneratedContentItem::NoCloseQuote,
                    _ => return None,
                }
            }
            TokenOrValue::Function(function)
                if function.name.as_ref().eq_ignore_ascii_case("attr") =>
            {
                let name = function.arguments.0.iter().find_map(token_ident)?;
                GeneratedContentItem::Attribute(styles.intern_string(name))
            }
            TokenOrValue::Function(function)
                if function.name.as_ref().eq_ignore_ascii_case("counter")
                    || function.name.as_ref().eq_ignore_ascii_case("counters") =>
            {
                let mut arguments = significant_tokens(&function.arguments);
                let name = token_ident(arguments.next()?)?;
                let separator = if function.name.as_ref().eq_ignore_ascii_case("counters") {
                    let (
                        Some(TokenOrValue::Token(Token::Comma)),
                        Some(TokenOrValue::Token(Token::String(separator))),
                    ) = (arguments.next(), arguments.next())
                    else {
                        return None;
                    };
                    Some(separator.as_ref())
                } else {
                    None
                };
                let style = match arguments.next() {
                    None => CounterStyle::Decimal,
                    Some(TokenOrValue::Token(Token::Comma)) => {
                        let style = parse_counter_style(token_ident(arguments.next()?)?)?;
                        if arguments.next().is_some() {
                            return None;
                        }
                        style
                    }
                    _ => return None,
                };
                let name = styles.intern_string(name);
                match separator {
                    Some(separator) => GeneratedContentItem::Counters {
                        name,
                        separator: styles.intern_string(separator),
                        style,
                    },
                    None => GeneratedContentItem::Counter { name, style },
                }
            }
            // Generated replaced images require their own layout path and are
            // deliberately not misrepresented as text here.
            _ => return None,
        };
        items.push(item);
    }
    Some(Some(GeneratedContent { items }))
}

fn parse_counter_style(value: &str) -> Option<CounterStyle> {
    match_ignore_ascii_case! { value,
        "decimal" => Some(CounterStyle::Decimal),
        "decimal-leading-zero" => Some(CounterStyle::DecimalLeadingZero),
        "lower-roman" => Some(CounterStyle::LowerRoman),
        "upper-roman" => Some(CounterStyle::UpperRoman),
        "lower-greek" => Some(CounterStyle::LowerGreek),
        "lower-alpha" | "lower-latin" => Some(CounterStyle::LowerAlpha),
        "upper-alpha" | "upper-latin" => Some(CounterStyle::UpperAlpha),
        "armenian" => Some(CounterStyle::Armenian),
        "georgian" => Some(CounterStyle::Georgian),
        "disc" => Some(CounterStyle::Disc),
        "circle" => Some(CounterStyle::Circle),
        "square" => Some(CounterStyle::Square),
        _ => None,
    }
}

pub(super) fn parse_counter_directives(
    styles: &mut ComputedStylesBuilder,
    tokens: &TokenList<'_>,
    default_value: i32,
) -> Option<Vec<CounterDirective>> {
    let mut tokens = significant_tokens(tokens);
    let mut directives = Vec::new();
    while let Some(token) = tokens.next() {
        let name = token_ident(token)?;
        if name.eq_ignore_ascii_case("none") {
            return (directives.is_empty() && tokens.next().is_none()).then_some(directives);
        }
        if name.eq_ignore_ascii_case("inherit")
            || name.eq_ignore_ascii_case("initial")
            || name.eq_ignore_ascii_case("unset")
        {
            return None;
        }
        let value = match tokens.clone().next() {
            Some(TokenOrValue::Token(Token::Number {
                int_value: Some(value),
                ..
            })) => {
                tokens.next();
                *value
            }
            _ => default_value,
        };
        directives.push(CounterDirective {
            name: styles.intern_string(name),
            value,
        });
    }
    (!directives.is_empty()).then_some(directives)
}

pub(super) fn parse_quotes(
    styles: &mut ComputedStylesBuilder,
    tokens: &TokenList<'_>,
) -> Option<QuoteStyle> {
    let mut tokens = significant_tokens(tokens);
    match tokens.clone().count() {
        1 => {
            return match_ignore_ascii_case! { token_ident(tokens.next()?)?,
                "auto" => Some(QuoteStyle::Auto),
                "none" => Some(QuoteStyle::None),
                _ => None,
            };
        }
        count if count == 0 || count % 2 != 0 => return None,
        _ => {}
    }
    let mut encoded = String::new();
    for (index, token) in tokens.enumerate() {
        let TokenOrValue::Token(Token::String(value)) = token else {
            return None;
        };
        if index > 0 {
            encoded.push('\0');
        }
        encoded.push_str(value.as_ref());
    }
    Some(QuoteStyle::Pairs(styles.intern_string(&encoded)))
}

#[cfg(test)]
mod tests {
    use super::{parse_counter_directives, parse_generated_content, parse_quotes};
    use crate::allocation_test_support::count_allocations;
    use html_parse::parse_dom_document;
    use html_style_model::{
        ComputedStylesBuilder, CounterDirective, CounterStyle, GeneratedContentItem, QuoteStyle,
    };
    use lightningcss::properties::custom::TokenList;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::traits::ParseWithOptions;

    fn tokens(source: &str) -> TokenList<'_> {
        TokenList::parse_string_with_options(source, ParserOptions::default()).unwrap()
    }

    #[test]
    fn generated_items_preserve_values_and_allocate_only_the_exact_result_vector() {
        let doc = parse_dom_document("<p></p>").unwrap();
        let mut styles = ComputedStylesBuilder::new(&doc);
        let chapter = styles.intern_string("chapter");
        let capital = styles.intern_string("Chapter");
        let separator = styles.intern_string(".");
        let title = styles.intern_string("title");
        let text = styles.intern_string("X");
        let empty = styles.intern_string("");
        for (css, expected) in [
            (
                "counter(chapter)",
                GeneratedContentItem::Counter {
                    name: chapter,
                    style: CounterStyle::Decimal,
                },
            ),
            (
                "COUNTER(Chapter, UpPeR-RoMaN)",
                GeneratedContentItem::Counter {
                    name: capital,
                    style: CounterStyle::UpperRoman,
                },
            ),
            (
                "counters(chapter, '.')",
                GeneratedContentItem::Counters {
                    name: chapter,
                    separator,
                    style: CounterStyle::Decimal,
                },
            ),
            (
                "CoUnTeRs(chapter /**/, '.' /**/, LoWeR-LaTiN)",
                GeneratedContentItem::Counters {
                    name: chapter,
                    separator,
                    style: CounterStyle::LowerAlpha,
                },
            ),
            ("OpEn-QuOtE", GeneratedContentItem::OpenQuote),
            ("close-quote", GeneratedContentItem::CloseQuote),
            ("no-open-quote", GeneratedContentItem::NoOpenQuote),
            ("no-close-quote", GeneratedContentItem::NoCloseQuote),
            ("'X'", GeneratedContentItem::Text(text)),
            ("''", GeneratedContentItem::Text(empty)),
            ("attr(title)", GeneratedContentItem::Attribute(title)),
        ] {
            let input = tokens(css);
            let (content, allocations) =
                count_allocations(|| parse_generated_content(&mut styles, &input));
            let content = content.unwrap().unwrap();
            assert_eq!(content.items, [expected], "{css}");
            assert_eq!(content.items.capacity(), 1, "{css}");
            assert_eq!(allocations, 1, "{css}");
        }
        let input = tokens(
            "'X' open-quote attr(title) counter(chapter) counters(chapter, '.') close-quote",
        );
        let (content, allocations) =
            count_allocations(|| parse_generated_content(&mut styles, &input));
        let content = content.unwrap().unwrap();
        assert_eq!(
            content.items,
            [
                GeneratedContentItem::Text(text),
                GeneratedContentItem::OpenQuote,
                GeneratedContentItem::Attribute(title),
                GeneratedContentItem::Counter {
                    name: chapter,
                    style: CounterStyle::Decimal
                },
                GeneratedContentItem::Counters {
                    name: chapter,
                    separator,
                    style: CounterStyle::Decimal
                },
                GeneratedContentItem::CloseQuote,
            ]
        );
        assert_eq!(content.items.capacity(), content.items.len());
        assert_eq!(allocations, 1);
        for css in ["normal", "NoNe", "initial", "unset", "inherit"] {
            let input = tokens(css);
            let (content, allocations) =
                count_allocations(|| parse_generated_content(&mut styles, &input));
            assert_eq!(content, Some(None), "{css}");
            assert_eq!(allocations, 0, "{css}");
        }
        for css in ["", "revert", "revert-layer"] {
            let input = tokens(css);
            let (content, allocations) =
                count_allocations(|| parse_generated_content(&mut styles, &input));
            assert_eq!(content, None, "{css}");
            assert_eq!(allocations, 0, "{css}");
        }
    }

    #[test]
    fn counter_functions_reject_missing_misplaced_and_extra_arguments() {
        let doc = parse_dom_document("<p></p>").unwrap();
        let mut styles = ComputedStylesBuilder::new(&doc);
        styles.intern_string("chapter");
        for css in [
            "counter()",
            "counter(1)",
            "counter(chapter,)",
            "counter(chapter, 'decimal')",
            "counter(chapter, decimal, extra)",
            "counter(chapter decimal)",
            "counters(chapter)",
            "counters(chapter, decimal)",
            "counters(chapter '.')",
            "counters(chapter, '.',)",
            "counters(chapter, '.', unknown)",
            "counters(chapter, '.', decimal, extra)",
            "counters(chapter, '.', decimal extra)",
            "counter(chapter) counters(chapter, unknown)",
        ] {
            let input = tokens(css);
            let (content, allocations) =
                count_allocations(|| parse_generated_content(&mut styles, &input));
            assert_eq!(content, None, "{css}");
            assert!(allocations <= 1, "{css}: {allocations} allocations");
        }
    }
    #[test]
    fn counter_directives_keep_native_integers_defaults_case_and_order_without_scratch() {
        let doc = parse_dom_document("<p></p>").unwrap();
        let mut styles = ComputedStylesBuilder::new(&doc);
        let chapter = styles.intern_string("chapter");
        let section = styles.intern_string("Section");
        let sub = styles.intern_string("sub");
        for default in [0, 1] {
            for (css, expected) in [
                (
                    "chapter",
                    vec![CounterDirective {
                        name: chapter,
                        value: default,
                    }],
                ),
                (
                    "chapter 3 Section -2 sub",
                    vec![
                        CounterDirective {
                            name: chapter,
                            value: 3,
                        },
                        CounterDirective {
                            name: section,
                            value: -2,
                        },
                        CounterDirective {
                            name: sub,
                            value: default,
                        },
                    ],
                ),
                (
                    "chapter/**/2 chapter -3",
                    vec![
                        CounterDirective {
                            name: chapter,
                            value: 2,
                        },
                        CounterDirective {
                            name: chapter,
                            value: -3,
                        },
                    ],
                ),
                (
                    "chapter 2147483647",
                    vec![CounterDirective {
                        name: chapter,
                        value: i32::MAX,
                    }],
                ),
                (
                    "chapter -2147483648",
                    vec![CounterDirective {
                        name: chapter,
                        value: i32::MIN,
                    }],
                ),
            ] {
                let input = tokens(css);
                let (actual, allocations) =
                    count_allocations(|| parse_counter_directives(&mut styles, &input, default));
                assert_eq!(actual.as_deref(), Some(expected.as_slice()), "{css}");
                assert_eq!(allocations, 1, "{css}");
            }
            for css in ["none", "NoNe /**/"] {
                let input = tokens(css);
                let (actual, allocations) =
                    count_allocations(|| parse_counter_directives(&mut styles, &input, default));
                assert_eq!(actual, Some(Vec::new()), "{css}");
                assert_eq!(allocations, 0, "{css}");
            }
            for css in [
                "",
                "none chapter",
                "chapter none",
                "inherit",
                "chapter UnSeT",
                "initial",
                "chapter 1.5",
                "chapter 2 3",
                "'chapter'",
                "chapter calc(2)",
            ] {
                let input = tokens(css);
                let (actual, allocations) =
                    count_allocations(|| parse_counter_directives(&mut styles, &input, default));
                assert_eq!(actual, None, "{css}");
                assert!(allocations <= 1, "{css}: {allocations} allocations");
            }
        }
    }

    #[test]
    fn quotes_keep_pair_encoding_and_keyword_semantics_without_token_scratch() {
        let doc = parse_dom_document("<p></p>").unwrap();
        let mut styles = ComputedStylesBuilder::new(&doc);
        for (css, encoded) in [
            ("'a' 'b'", "a\0b"),
            ("'a'/**/'b'", "a\0b"),
            ("'«' '»' '‹' '›'", "«\0»\0‹\0›"),
            ("'\\00ab' '\\00bb'", "«\0»"),
            ("'' ''", "\0"),
            ("'a' 'b' 'c' 'd' 'e' 'f'", "a\0b\0c\0d\0e\0f"),
        ] {
            let expected = styles.intern_string(encoded);
            let input = tokens(css);
            let (actual, allocations) = count_allocations(|| parse_quotes(&mut styles, &input));
            assert_eq!(actual, Some(QuoteStyle::Pairs(expected)), "{css}");
            assert!(allocations <= 2, "{css}: {allocations} allocations");
        }
        for (css, expected) in [("AuTo", QuoteStyle::Auto), ("NoNe", QuoteStyle::None)] {
            let input = tokens(css);
            let (actual, allocations) = count_allocations(|| parse_quotes(&mut styles, &input));
            assert_eq!(actual, Some(expected), "{css}");
            assert_eq!(allocations, 0, "{css}");
        }
        for css in [
            "",
            "''",
            "'a' 'b' 'c'",
            "auto none",
            "'a' auto",
            "initial",
            "var(--q)",
        ] {
            let input = tokens(css);
            let (actual, allocations) = count_allocations(|| parse_quotes(&mut styles, &input));
            assert_eq!(actual, None, "{css}");
            assert!(allocations <= 1, "{css}: {allocations} allocations");
        }
    }
}
