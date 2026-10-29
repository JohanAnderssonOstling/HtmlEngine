//! Opaque parser output carried through resource expansion into compilation.
use super::fonts::resolve_rule_urls;
use super::imports::{ImportLayer, StylesheetImport};
use html_dom::has_uri_scheme;
use lightningcss::printer::PrinterOptions;
use lightningcss::rules::import::ImportRule;
use lightningcss::rules::layer::LayerBlockRule;
use lightningcss::rules::media::MediaRule;
use lightningcss::rules::supports::SupportsRule;
use lightningcss::rules::{CssRule, CssRuleList};
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use lightningcss::traits::ToCss;
use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};

/// A stylesheet whose rules can be compiled without parsing CSS again.
#[derive(Clone)]
pub struct ResourceStylesheet {
    sheet: Arc<StyleSheet<'static>>,
    css: Arc<OnceLock<String>>,
    font_urls: Arc<[String]>,
}

#[derive(Clone)]
pub enum ResourceStylesheetEntry {
    Rules(ResourceStylesheet),
    Import(ResourceImport),
}

/// Native import conditions stay private to the style crate.
#[derive(Clone)]
pub struct ResourceImport(ImportRule<'static>);

impl ResourceStylesheet {
    /// Parse and rebase font URLs once, preserving rules and import conditions.
    pub fn parse(
        css: &str,
        mut resolve: impl FnMut(&str) -> String,
    ) -> Vec<ResourceStylesheetEntry> {
        Self::parse_unresolved(css)
            .into_iter()
            .map(|entry| match entry {
                ResourceStylesheetEntry::Rules(sheet) => {
                    ResourceStylesheetEntry::Rules(sheet.resolve_font_urls(&mut resolve))
                }
                entry => entry,
            })
            .collect()
    }

    fn parse_unresolved(css: &str) -> Vec<ResourceStylesheetEntry> {
        let Some(sheet) = super::parse_stylesheet(css) else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        let mut rules = Vec::new();
        for rule in sheet.rules.0 {
            if let CssRule::Import(import) = rule {
                Self::flush(&mut rules, &mut entries);
                entries.push(ResourceStylesheetEntry::Import(ResourceImport(import)));
            } else {
                rules.push(rule);
            }
        }
        Self::flush(&mut rules, &mut entries);
        entries
    }

    fn from_rules(rules: Vec<CssRule<'static>>) -> Self {
        let mut sheet = StyleSheet::new(Vec::new(), CssRuleList(rules), ParserOptions::default());
        let mut font_urls = Vec::new();
        resolve_rule_urls(&mut sheet.rules, &mut |url| {
            font_urls.push(url.to_owned());
            url.to_owned()
        });
        Self {
            sheet: Arc::new(sheet),
            css: Arc::default(),
            font_urls: font_urls.into(),
        }
    }

    /// Rebase a cached source for this resource context. Ordinary sheets share
    /// their rules and cache identity; only changed font URLs copy the AST.
    pub fn resolve_font_urls(self, mut resolve: impl FnMut(&str) -> String) -> Self {
        let urls = self
            .font_urls
            .iter()
            .map(|url| resolve(url))
            .collect::<Vec<_>>();
        if urls.iter().eq(self.font_urls.iter()) {
            return self;
        }
        let font_urls = urls
            .iter()
            .filter(|url| !has_uri_scheme(url))
            .cloned()
            .collect();
        let mut sheet = (*self.sheet).clone();
        let mut urls = urls.into_iter();
        resolve_rule_urls(&mut sheet.rules, &mut |_| {
            urls.next().expect("one resolution per font URL")
        });
        Self {
            sheet: Arc::new(sheet),
            css: Arc::default(),
            font_urls,
        }
    }

    fn flush(rules: &mut Vec<CssRule<'static>>, entries: &mut Vec<ResourceStylesheetEntry>) {
        if !rules.is_empty() {
            entries.push(ResourceStylesheetEntry::Rules(Self::from_rules(
                std::mem::take(rules),
            )));
        }
    }

    /// Stable textual cache identity; compilation consumes the retained rules.
    pub fn css(&self) -> &str {
        self.css.get_or_init(|| {
            self.sheet
                .to_css(PrinterOptions::default())
                .expect("parsed CSS can be serialized")
                .code
        })
    }

    pub(crate) fn stylesheet(&self) -> StyleSheet<'static> {
        (*self.sheet).clone()
    }
}

impl ResourceImport {
    pub fn url(&self) -> &str {
        self.0.url.as_ref()
    }

    pub(super) fn description(&self) -> StylesheetImport {
        let import = &self.0;
        let options = PrinterOptions::default;
        StylesheetImport {
            url: import.url.as_ref().to_owned(),
            layer: import.layer.as_ref().map(|name| match name {
                None => ImportLayer::Anonymous,
                Some(name) => ImportLayer::Named(name.to_css_string(options()).unwrap_or_default()),
            }),
            supports: import
                .supports
                .as_ref()
                .and_then(|condition| condition.to_css_string(options()).ok()),
            media: (!import.media.media_queries.is_empty())
                .then(|| import.media.to_css_string(options()).ok())
                .flatten(),
        }
    }

    /// Keep all imported chunks inside one conditional/layer boundary. In
    /// particular, an anonymous import layer must not become one layer per chunk.
    pub fn wrap(&self, sheets: Vec<ResourceStylesheet>) -> ResourceStylesheet {
        let mut rules = CssRuleList(
            sheets
                .into_iter()
                .flat_map(|sheet| Arc::unwrap_or_clone(sheet.sheet).rules.0)
                .collect(),
        );
        let import = &self.0;
        if !import.media.media_queries.is_empty() {
            rules = CssRuleList(vec![CssRule::Media(MediaRule {
                query: import.media.clone(),
                rules,
                loc: import.loc,
            })]);
        }
        if let Some(condition) = &import.supports {
            rules = CssRuleList(vec![CssRule::Supports(SupportsRule {
                condition: condition.clone(),
                rules,
                loc: import.loc,
            })]);
        }
        if let Some(name) = &import.layer {
            rules = CssRuleList(vec![CssRule::LayerBlock(LayerBlockRule {
                name: name.clone(),
                rules,
                loc: import.loc,
            })]);
        }
        ResourceStylesheet::from_rules(rules.0)
    }
}

/// Bounded source cache, independent of URL resolution and media environments.
#[derive(Default)]
pub(crate) struct ResourceStylesheetCache {
    entries: VecDeque<(Arc<str>, Vec<ResourceStylesheetEntry>)>,
}

impl ResourceStylesheetCache {
    pub(crate) fn entries(&mut self, css: &str) -> Vec<ResourceStylesheetEntry> {
        if let Some((_, entries)) = self
            .entries
            .iter()
            .find(|(source, _)| source.as_ref() == css)
        {
            return entries.clone();
        }
        let entries = ResourceStylesheet::parse_unresolved(css);
        if self.entries.len() == 64 {
            self.entries.pop_front();
        }
        self.entries.push_back((Arc::from(css), entries.clone()));
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::{ResourceStylesheetCache, ResourceStylesheetEntry};
    use std::sync::Arc;

    #[test]
    fn cached_sources_share_ordinary_rules_and_keep_font_bases_independent() {
        let mut cache = ResourceStylesheetCache::default();
        let css = "p {color:green}";
        let ResourceStylesheetEntry::Rules(first) = cache.entries(css).remove(0) else {
            panic!("rules")
        };
        let ResourceStylesheetEntry::Rules(second) = cache.entries(css).remove(0) else {
            panic!("rules")
        };
        assert!(Arc::ptr_eq(&first.sheet, &second.sheet));
        assert!(Arc::ptr_eq(&first.css, &second.css));
        assert_eq!(
            first
                .resolve_font_urls(|_| panic!("ordinary sheet has no font URLs"))
                .css(),
            second.css()
        );

        let css = "@font-face {font-family:Reader;src:url(reader.woff2)}";
        for base in ["one", "two", "one"] {
            let ResourceStylesheetEntry::Rules(sheet) = cache.entries(css).remove(0) else {
                panic!("rules")
            };
            let rebased =
                sheet.resolve_font_urls(|url| format!("https://example.test/{base}/{url}"));
            assert!(
                rebased
                    .css()
                    .contains(&format!("https://example.test/{base}/reader.woff2"))
            );
            let absolute =
                rebased.resolve_font_urls(|_| panic!("absolute URLs must remain unchanged"));
            assert!(absolute.css().contains(base));
        }
    }
}
