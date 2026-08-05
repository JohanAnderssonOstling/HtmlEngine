use html_dom::{Document, DomNodeId};
use html_style_model::{ComputedStyles, CounterDirective, CounterDirectives, Display, GeneratedContent, GeneratedContentItem, QuoteStyle, StyleIndices, StyleStringId};

use super::classification::is_block_level_pseudo_display;

pub(super) struct ResolvedGeneratedContent {
    pub(super) style: StyleIndices,
    pub(super) display: Display,
    pub(super) text: String,
}

#[derive(Default)]
struct CounterScope {
    frames: Vec<Vec<(StyleStringId, i64)>>,
}

impl CounterScope {
    fn new() -> Self {
        Self { frames: vec![Vec::new()] }
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
        let frame = self.frames.last_mut().expect("counter scope always has a frame");
        if let Some((_, value)) = frame.iter_mut().rev().find(|(name, _)| *name == directive.name) {
            *value = i64::from(directive.value);
        } else {
            frame.push((directive.name, i64::from(directive.value)));
        }
    }

    fn increment(&mut self, directive: CounterDirective) {
        for frame in self.frames.iter_mut().rev() {
            if let Some((_, value)) = frame.iter_mut().rev().find(|(name, _)| *name == directive.name) {
                *value = value.saturating_add(i64::from(directive.value));
                return;
            }
        }
        self.frames.last_mut().expect("counter scope always has a frame").push((directive.name, i64::from(directive.value)));
    }

    fn value(&self, name: StyleStringId) -> i64 {
        self.frames.iter().rev().find_map(|frame| frame.iter().rev().find_map(|(candidate, value)| (*candidate == name).then_some(*value))).unwrap_or(0)
    }

    fn values(&self, name: StyleStringId) -> Vec<i64> {
        self.frames.iter().filter_map(|frame| frame.iter().rev().find_map(|(candidate, value)| (*candidate == name).then_some(*value))).collect()
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
        Self { document, styles, quote_depth: 0, counters: CounterScope::new() }
    }

    pub(super) fn enter_sibling_scope(&mut self) {
        self.counters.enter();
    }

    pub(super) fn leave_sibling_scope(&mut self) {
        self.counters.leave();
    }

    pub(super) fn apply(&mut self, directives: &CounterDirectives) {
        self.counters.apply(directives);
    }

    pub(super) fn pseudo_display(&self, origin: DomNodeId, before: bool) -> Option<Display> {
        self.pseudo(origin, before).map(|(style, _, _)| self.styles.box_model_style(style).expect("validated generated pseudo style").display)
    }

    pub(super) fn pseudo_is_block_level(&self, origin: DomNodeId, before: bool) -> bool {
        self.pseudo_display(origin, before).is_some_and(is_block_level_pseudo_display)
    }

    pub(super) fn resolve_pseudo(&mut self, origin: DomNodeId, before: bool) -> Option<ResolvedGeneratedContent> {
        let (style, content, directives) = self.pseudo(origin, before)?;
        let content = content.clone();
        let directives = directives.clone();
        let display = self.styles.box_model_style(style).expect("validated generated pseudo style").display;
        self.counters.apply(&directives);
        let text = self.resolve_text(origin, &content, style);
        Some(ResolvedGeneratedContent { style, display, text })
    }

    fn pseudo(&self, origin: DomNodeId, before: bool) -> Option<(StyleIndices, &GeneratedContent, &CounterDirectives)> {
        if before { self.styles.before_style_for_node(origin) } else { self.styles.after_style_for_node(origin) }
    }

    fn resolve_text(&mut self, origin: DomNodeId, content: &GeneratedContent, style: StyleIndices) -> String {
        let mut text = String::new();
        let quotes = self.styles.view(style).map(|view| view.quotes()).unwrap_or(QuoteStyle::Auto);
        for item in &content.items {
            match *item {
                GeneratedContentItem::Text(id) => text.push_str(self.styles.string(id).unwrap_or_default()),
                GeneratedContentItem::Attribute(id) => {
                    if let Some(name) = self.styles.string(id)
                        && let Some(value) = self.document.get_dom_attr(origin, name)
                    {
                        text.push_str(value);
                    }
                }
                GeneratedContentItem::Counter { name, style } => text.push_str(&super::super::list_marker::counter_text(style, self.counters.value(name))),
                GeneratedContentItem::Counters { name, separator, style } => {
                    let separator = self.styles.string(separator).unwrap_or_default();
                    for (index, value) in self.counters.values(name).into_iter().enumerate() {
                        if index > 0 {
                            text.push_str(separator);
                        }
                        text.push_str(&super::super::list_marker::counter_text(style, value));
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
                GeneratedContentItem::NoCloseQuote => self.quote_depth = self.quote_depth.saturating_sub(1),
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
                let Some(encoded) = self.styles.string(id) else { return ("", "") };
                let mut values = encoded.split('\0');
                let mut selected = ("", "");
                for _ in 0..=depth {
                    let (Some(open), Some(close)) = (values.next(), values.next()) else { break };
                    selected = (open, close);
                }
                selected
            }
        }
    }
}
