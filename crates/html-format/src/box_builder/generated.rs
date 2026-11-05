use html_dom::{Document, DomNodeId};
use html_style_model::{
    ComputedStyles, CounterDirective, CounterDirectives, Display, GeneratedContent,
    GeneratedContentItem, QuoteStyle, StyleIndices, StyleStringId,
};

use crate::list_marker::append_counter;

use super::classification::display_for_style;

pub(super) struct ResolvedGeneratedContent {
    pub(super) style: StyleIndices,
    pub(super) display: Display,
    pub(super) text: String,
}

struct CounterScope {
    frames: Vec<Vec<(StyleStringId, i64)>>,
}

impl CounterScope {
    fn new() -> Self {
        Self {
            frames: vec![Vec::new()],
        }
    }

    fn current_frame(&mut self) -> &mut Vec<(StyleStringId, i64)> {
        self.frames
            .last_mut()
            .expect("counter scope always has a frame")
    }

    fn enter(&mut self) {
        self.frames.push(Vec::new());
    }

    fn leave(&mut self) {
        debug_assert!(self.frames.len() > 1);
        self.frames.pop();
    }

    fn apply(&mut self, directives: &CounterDirectives) {
        for directive in &directives.resets {
            self.reset(*directive);
        }
        for directive in &directives.increments {
            self.increment(*directive);
        }
    }

    fn reset(&mut self, directive: CounterDirective) {
        let frame = self.current_frame();
        if let Some((_, value)) = frame
            .iter_mut()
            .rev()
            .find(|(name, _)| *name == directive.name)
        {
            *value = i64::from(directive.value);
        } else {
            frame.push((directive.name, i64::from(directive.value)));
        }
    }

    fn increment(&mut self, directive: CounterDirective) {
        for frame in self.frames.iter_mut().rev() {
            if let Some((_, value)) = frame
                .iter_mut()
                .rev()
                .find(|(name, _)| *name == directive.name)
            {
                *value = value.saturating_add(i64::from(directive.value));
                return;
            }
        }
        self.current_frame()
            .push((directive.name, i64::from(directive.value)));
    }

    fn value(&self, name: StyleStringId) -> i64 {
        self.values(name).next_back().unwrap_or(0)
    }

    fn values(&self, name: StyleStringId) -> impl DoubleEndedIterator<Item = i64> + '_ {
        self.frames.iter().filter_map(move |frame| {
            frame
                .iter()
                .rev()
                .find_map(|(candidate, value)| (*candidate == name).then_some(*value))
        })
    }
}

/// Owns the stateful part of CSS generated-content resolution. It produces
/// text and style data; layout-tree allocation remains the builder's job.
pub(super) struct GeneratedContentResolver<'a> {
    document: &'a Document,
    styles: &'a ComputedStyles,
    quote_depth: usize,
    counters: CounterScope,
}

impl<'a> GeneratedContentResolver<'a> {
    pub(super) fn new(document: &'a Document, styles: &'a ComputedStyles) -> Self {
        Self {
            document,
            styles,
            quote_depth: 0,
            counters: CounterScope::new(),
        }
    }

    pub(super) fn enter_sibling_scope(&mut self) {
        self.counters.enter();
    }

    pub(super) fn leave_sibling_scope(&mut self) {
        self.counters.leave();
    }

    pub(super) fn apply_for_node(&mut self, node: DomNodeId) {
        if let Some(directives) = self.styles.counter_directives_for_node(node) {
            self.counters.apply(directives);
        }
    }

    pub(super) fn resolve_pseudo(
        &mut self,
        origin: DomNodeId,
        before: bool,
    ) -> Option<ResolvedGeneratedContent> {
        let (style, content, directives) = self.styles.generated_pseudo_for_node(origin, before)?;
        let display = display_for_style(self.styles, style);
        self.counters.apply(directives);
        let text = self.resolve_text(origin, content, style);
        Some(ResolvedGeneratedContent {
            style,
            display,
            text,
        })
    }

    fn resolve_text(
        &mut self,
        origin: DomNodeId,
        content: &GeneratedContent,
        style: StyleIndices,
    ) -> String {
        let mut text = String::new();
        let quotes = self
            .styles
            .view(style)
            .map_or(QuoteStyle::Auto, |view| view.quotes());
        for item in &content.items {
            match *item {
                GeneratedContentItem::Text(id) => {
                    text.push_str(self.styles.string(id).unwrap_or_default())
                }
                GeneratedContentItem::Attribute(id) => {
                    if let Some(name) = self.styles.string(id)
                        && let Some(value) = self.document.get_dom_attr(origin, name)
                    {
                        text.push_str(value);
                    }
                }
                GeneratedContentItem::Counter { name, style } => {
                    append_counter(style, self.counters.value(name), &mut text)
                }
                GeneratedContentItem::Counters {
                    name,
                    separator,
                    style,
                } => {
                    let separator = self.styles.string(separator).unwrap_or_default();
                    for (index, value) in self.counters.values(name).enumerate() {
                        if index > 0 {
                            text.push_str(separator);
                        }
                        append_counter(style, value, &mut text);
                    }
                }
                GeneratedContentItem::OpenQuote => {
                    let (open, _) = self.quote_pair(quotes, self.quote_depth);
                    text.push_str(open);
                    self.quote_depth += 1;
                }
                GeneratedContentItem::CloseQuote => {
                    if self.quote_depth > 0 {
                        self.quote_depth -= 1;
                        let (_, close) = self.quote_pair(quotes, self.quote_depth);
                        text.push_str(close);
                    }
                }
                GeneratedContentItem::NoOpenQuote => self.quote_depth += 1,
                GeneratedContentItem::NoCloseQuote => {
                    self.quote_depth = self.quote_depth.saturating_sub(1)
                }
            }
        }
        text
    }

    fn quote_pair(&self, quotes: QuoteStyle, depth: usize) -> (&str, &str) {
        match quotes {
            QuoteStyle::Auto => {
                if depth == 0 {
                    ("\u{201c}", "\u{201d}")
                } else {
                    ("\u{2018}", "\u{2019}")
                }
            }
            QuoteStyle::None => ("", ""),
            QuoteStyle::Pairs(id) => {
                let Some(encoded) = self.styles.string(id) else {
                    return ("", "");
                };
                let mut values = encoded.split('\0');
                let mut selected = ("", "");
                for _ in 0..=depth {
                    let (Some(open), Some(close)) = (values.next(), values.next()) else {
                        break;
                    };
                    selected = (open, close);
                }
                selected
            }
        }
    }
}
