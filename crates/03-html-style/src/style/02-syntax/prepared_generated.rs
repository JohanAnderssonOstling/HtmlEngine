use crate::style::syntax::generated_content::{counter_arguments, for_each_counter_directive};
use crate::style::syntax::value::{significant_tokens, token_ident};
use cssparser::match_ignore_ascii_case;
use html_style_model::CounterStyle;
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};

pub(crate) struct PreparedCounterDirective {
    pub(crate) name: String,
    pub(crate) value: i32,
}
pub(crate) enum PreparedQuoteStyle {
    Auto,
    None,
    Pairs(String),
}
pub(crate) struct PreparedContent {
    pub(crate) items: Vec<PreparedContentItem>,
}
pub(crate) enum PreparedContentItem {
    Text(String),
    Attribute(String),
    Counter {
        name: String,
        style: CounterStyle,
    },
    Counters {
        name: String,
        separator: String,
        style: CounterStyle,
    },
    OpenQuote,
    CloseQuote,
    NoOpenQuote,
    NoCloseQuote,
}
#[derive(Clone, Copy)]
pub(crate) enum QuoteKind {
    Open,
    Close,
    NoOpen,
    NoClose,
}

pub(crate) trait ContentBuilder {
    type Item;
    type Content;
    type Directive;
    type Quotes;
    fn text(&mut self, value: &str) -> Self::Item;
    fn attribute(&mut self, name: &str) -> Self::Item;
    fn counter(&mut self, name: &str, separator: Option<&str>, style: CounterStyle) -> Self::Item;
    fn quote(&mut self, kind: QuoteKind) -> Self::Item;
    fn content(&mut self, items: Vec<Self::Item>) -> Self::Content;
    fn directive(&mut self, name: &str, value: i32) -> Self::Directive;
    fn quotes(&mut self, value: PreparedQuoteStyle) -> Self::Quotes;
}

pub(crate) struct PreparedContentBuilder;
impl ContentBuilder for PreparedContentBuilder {
    type Item = PreparedContentItem;
    type Content = PreparedContent;
    type Directive = PreparedCounterDirective;
    type Quotes = PreparedQuoteStyle;
    fn text(&mut self, value: &str) -> Self::Item {
        PreparedContentItem::Text(value.to_owned())
    }
    fn attribute(&mut self, name: &str) -> Self::Item {
        PreparedContentItem::Attribute(name.to_owned())
    }
    fn counter(&mut self, name: &str, separator: Option<&str>, style: CounterStyle) -> Self::Item {
        match separator {
            Some(separator) => PreparedContentItem::Counters {
                name: name.to_owned(),
                separator: separator.to_owned(),
                style,
            },
            None => PreparedContentItem::Counter {
                name: name.to_owned(),
                style,
            },
        }
    }
    fn quote(&mut self, kind: QuoteKind) -> Self::Item {
        match kind {
            QuoteKind::Open => PreparedContentItem::OpenQuote,
            QuoteKind::Close => PreparedContentItem::CloseQuote,
            QuoteKind::NoOpen => PreparedContentItem::NoOpenQuote,
            QuoteKind::NoClose => PreparedContentItem::NoCloseQuote,
        }
    }
    fn content(&mut self, items: Vec<Self::Item>) -> Self::Content {
        PreparedContent { items }
    }
    fn directive(&mut self, name: &str, value: i32) -> Self::Directive {
        PreparedCounterDirective {
            name: name.to_owned(),
            value,
        }
    }
    fn quotes(&mut self, value: PreparedQuoteStyle) -> Self::Quotes {
        value
    }
}

/// Convert native generated-content tokens into renderer-owned items.
/// The outer `Option` distinguishes an invalid/unsupported declaration from
/// a valid suppressing value (`normal` or `none`).
pub(crate) fn parse_generated_content<B: ContentBuilder>(
    tokens: &TokenList<'_>,
    builder: &mut B,
) -> Option<Option<B::Content>> {
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
            TokenOrValue::Token(Token::String(value)) => builder.text(value.as_ref()),
            TokenOrValue::Token(Token::Ident(value)) => {
                match_ignore_ascii_case! { value.as_ref(),
                    "open-quote" => builder.quote(QuoteKind::Open),
                    "close-quote" => builder.quote(QuoteKind::Close),
                    "no-open-quote" => builder.quote(QuoteKind::NoOpen),
                    "no-close-quote" => builder.quote(QuoteKind::NoClose),
                    _ => return None,
                }
            }
            TokenOrValue::Function(function)
                if function.name.as_ref().eq_ignore_ascii_case("attr") =>
            {
                let name = function.arguments.0.iter().find_map(token_ident)?;
                builder.attribute(name)
            }
            TokenOrValue::Function(function)
                if function.name.as_ref().eq_ignore_ascii_case("counter")
                    || function.name.as_ref().eq_ignore_ascii_case("counters") =>
            {
                let arguments = counter_arguments(function)?;
                let style = match arguments.style {
                    None => CounterStyle::Decimal,
                    Some(token) => parse_counter_style(token_ident(token)?)?,
                };
                builder.counter(arguments.name, arguments.separator, style)
            }
            // Generated replaced images require their own layout path and are
            // deliberately not misrepresented as text here.
            _ => return None,
        };
        items.push(item);
    }
    Some(Some(builder.content(items)))
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

pub(crate) fn parse_counter_directives<B: ContentBuilder>(
    tokens: &TokenList<'_>,
    default_value: i32,
    builder: &mut B,
) -> Option<Vec<B::Directive>> {
    let mut directives = Vec::new();
    for_each_counter_directive(tokens, default_value, |name, value| {
        directives.push(builder.directive(name, value));
    })?;
    Some(directives)
}

pub(crate) fn parse_quotes<B: ContentBuilder>(
    tokens: &TokenList<'_>,
    builder: &mut B,
) -> Option<B::Quotes> {
    let mut tokens = significant_tokens(tokens);
    match tokens.clone().count() {
        1 => {
            return match_ignore_ascii_case! { token_ident(tokens.next()?)?,
                "auto" => Some(builder.quotes(PreparedQuoteStyle::Auto)),
                "none" => Some(builder.quotes(PreparedQuoteStyle::None)),
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
    Some(builder.quotes(PreparedQuoteStyle::Pairs(encoded)))
}
