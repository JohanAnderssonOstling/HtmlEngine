//! Opaque parser output carried through resource expansion into compilation.
use super::fonts::resolve_rule_urls;
use super::imports::{ImportLayer, StylesheetImport};
use lightningcss::printer::PrinterOptions;
use lightningcss::rules::import::ImportRule;
use lightningcss::rules::layer::LayerBlockRule;
use lightningcss::rules::media::MediaRule;
use lightningcss::rules::supports::SupportsRule;
use lightningcss::rules::{CssRule, CssRuleList};
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use lightningcss::traits::ToCss;
use std::sync::{Arc, OnceLock};

/// A stylesheet whose rules can be compiled without parsing CSS again.
#[derive(Clone)]
pub struct ResourceStylesheet(Arc<ResourceRules>);

struct ResourceRules {
    sheet: StyleSheet<'static>,
    css: OnceLock<String>,
    has_relative_fonts: bool,
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
    /// Parse once, preserving rules and native import conditions.
    pub fn parse(css: &str) -> Vec<ResourceStylesheetEntry> {
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
        let mut has_relative_fonts = false;
        resolve_rule_urls(&mut sheet.rules, &mut |_| {
            has_relative_fonts = true;
            None
        });
        Self(Arc::new(ResourceRules {
            sheet,
            css: OnceLock::new(),
            has_relative_fonts,
        }))
    }

    /// Ordinary sheets retain their shared rules and identity. URL changes copy
    /// the source so another resource context can still resolve the original.
    pub fn resolve_font_urls(self, mut resolve: impl FnMut(&str) -> String) -> Self {
        if !self.0.has_relative_fonts {
            return self;
        }
        let mut sheet = self.stylesheet();
        if resolve_rule_urls(&mut sheet.rules, &mut |url| Some(resolve(url))) {
            Self::from_rules(sheet.rules.0)
        } else {
            self
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
        self.0.css.get_or_init(|| {
            self.0
                .sheet
                .to_css(PrinterOptions::default())
                .expect("parsed CSS can be serialized")
                .code
        })
    }

    pub(crate) fn stylesheet(&self) -> StyleSheet<'static> {
        self.0.sheet.clone()
    }

    fn into_rules(self) -> Vec<CssRule<'static>> {
        match Arc::try_unwrap(self.0) {
            Ok(rules) => rules.sheet.rules.0,
            Err(rules) => rules.sheet.rules.0.clone(),
        }
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
                .flat_map(ResourceStylesheet::into_rules)
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

#[cfg(test)]
mod tests {
    use super::{ResourceStylesheet, ResourceStylesheetEntry};
    use crate::StyleProgramCache;
    use crate::allocation_test_support::count_allocations;
    use std::sync::Arc;

    #[test]
    fn font_url_detection_does_not_allocate_per_relative_url() {
        let (_, baseline_allocations) =
            count_allocations(|| ResourceStylesheet::from_rules(Vec::new()));
        for count in [1, 256] {
            let sources = (0..count)
                .map(|index| format!("url(font-{index}.woff2)"))
                .collect::<Vec<_>>()
                .join(",");
            let css =
                format!("@media screen {{ @font-face {{font-family:Reader;src:{sources}}} }}");
            let rules = super::super::parse_stylesheet(&css).unwrap().rules.0;
            let (sheet, allocations) = count_allocations(|| ResourceStylesheet::from_rules(rules));
            assert!(sheet.0.has_relative_fonts);
            assert_eq!(allocations, baseline_allocations);
            let mut visited = 0;
            sheet.resolve_font_urls(|url| {
                visited += 1;
                url.to_owned()
            });
            assert_eq!(visited, count);
        }
    }

    #[test]
    fn consuming_owned_rules_reuses_storage_and_keeps_shared_sources_intact() {
        for shared in [false, true] {
            let ResourceStylesheetEntry::Rules(sheet) =
                ResourceStylesheet::parse("@media screen { p { color: green } }").remove(0)
            else {
                panic!("rules")
            };
            let retained = shared.then(|| sheet.clone());
            let pointer = sheet.0.sheet.rules.0.as_ptr();
            let expected = sheet.css().to_owned();
            let (mut rules, allocations) = count_allocations(|| sheet.into_rules());
            if let Some(retained) = retained {
                assert_ne!(rules.as_ptr(), pointer);
                rules.clear();
                assert_eq!(retained.css(), expected);
                assert!(!retained.0.sheet.rules.0.is_empty());
            } else {
                assert_eq!(rules.as_ptr(), pointer);
                assert_eq!(allocations, 0);
            }
        }
    }

    #[test]
    fn cached_sources_share_ordinary_rules_and_keep_font_bases_independent() {
        let mut cache = StyleProgramCache::default();
        let css = "p {color:green}";
        let ResourceStylesheetEntry::Rules(first) = cache.resource_entries(css).remove(0) else {
            panic!("rules")
        };
        let ResourceStylesheetEntry::Rules(second) = cache.resource_entries(css).remove(0) else {
            panic!("rules")
        };
        assert!(Arc::ptr_eq(&first.0, &second.0));
        assert_eq!(
            first
                .resolve_font_urls(|_| panic!("ordinary sheet has no font URLs"))
                .css(),
            second.css()
        );

        let css = "@font-face {font-family:Reader;src:url(reader.woff2)}";
        for base in ["one", "two", "one"] {
            let ResourceStylesheetEntry::Rules(sheet) = cache.resource_entries(css).remove(0)
            else {
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
