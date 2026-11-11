//! Materialize the shared generated-content grammar directly into document strings.
use crate::style::syntax::prepared_generated::{
    self, ContentBuilder, PreparedQuoteStyle, QuoteKind};
use html_style_model::{
    ComputedStylesBuilder, CounterDirective, CounterStyle, GeneratedContent, GeneratedContentItem,
    QuoteStyle,
};
use lightningcss::properties::custom::TokenList;

struct ComputedContentBuilder<'a>(&'a mut ComputedStylesBuilder);
impl ContentBuilder for ComputedContentBuilder<'_> {
    type Item = GeneratedContentItem;
    type Content = GeneratedContent;
    type Directive = CounterDirective;
    type Quotes = QuoteStyle;
    fn text(&mut self, value: &str) -> Self::Item {
        GeneratedContentItem::Text(self.0.intern_string(value))
    }
    fn attribute(&mut self, name: &str) -> Self::Item {
        GeneratedContentItem::Attribute(self.0.intern_string(name))
    }
    fn counter(&mut self, name: &str, separator: Option<&str>, style: CounterStyle) -> Self::Item {
        let name = self.0.intern_string(name);
        match separator {
            Some(separator) => GeneratedContentItem::Counters {
                name,
                separator: self.0.intern_string(separator),
                style,
            },
            None => GeneratedContentItem::Counter { name, style },
        }
    }
    fn quote(&mut self, kind: QuoteKind) -> Self::Item {
        match kind {
            QuoteKind::Open => GeneratedContentItem::OpenQuote,
            QuoteKind::Close => GeneratedContentItem::CloseQuote,
            QuoteKind::NoOpen => GeneratedContentItem::NoOpenQuote,
            QuoteKind::NoClose => GeneratedContentItem::NoCloseQuote,
        }
    }
    fn content(&mut self, items: Vec<Self::Item>) -> Self::Content {
        GeneratedContent { items }
    }
    fn directive(&mut self, name: &str, value: i32) -> Self::Directive {
        CounterDirective {
            name: self.0.intern_string(name),
            value,
        }
    }
    fn quotes(&mut self, value: PreparedQuoteStyle) -> Self::Quotes {
        match value {
            PreparedQuoteStyle::Auto => QuoteStyle::Auto,
            PreparedQuoteStyle::None => QuoteStyle::None,
            PreparedQuoteStyle::Pairs(value) => QuoteStyle::Pairs(self.0.intern_string(&value)),
        }
    }
}

pub(super) fn parse_generated_content(
    styles: &mut ComputedStylesBuilder,
    tokens: &TokenList<'_>,
) -> Option<Option<GeneratedContent>> {
    prepared_generated::parse_generated_content(tokens, &mut ComputedContentBuilder(styles))
}
pub(super) fn parse_counter_directives(
    styles: &mut ComputedStylesBuilder,
    tokens: &TokenList<'_>,
    default_value: i32,
) -> Option<Vec<CounterDirective>> {
    prepared_generated::parse_counter_directives(
        tokens,
        default_value,
        &mut ComputedContentBuilder(styles),
    )
}
pub(super) fn parse_quotes(
    styles: &mut ComputedStylesBuilder,
    tokens: &TokenList<'_>,
) -> Option<QuoteStyle> {
    prepared_generated::parse_quotes(tokens, &mut ComputedContentBuilder(styles))
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
