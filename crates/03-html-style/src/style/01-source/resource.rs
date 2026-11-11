//! Opaque parser output carried through resource expansion into compilation.
use super::fonts::{resolve_rule_urls, visit_relative_font_urls};
use super::imports::{ImportLayer, StylesheetImport};
use crate::style::rules::prepared::PreparedStylesheetFragment;
use lightningcss::printer::PrinterOptions;
use lightningcss::rules::import::ImportRule;
use lightningcss::rules::layer::LayerBlockRule;
use lightningcss::rules::media::MediaRule;
use lightningcss::rules::supports::SupportsRule;
use lightningcss::rules::{CssRule, CssRuleList};
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use lightningcss::traits::ToCss;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// A stylesheet whose rules can be compiled without parsing CSS again.
#[derive(Clone)]
pub struct ResourceStylesheet(Arc<ResourceRules>);

static NEXT_STYLESHEET_ID: AtomicU64 = AtomicU64::new(1);

struct ResourceRules {
    id: u64,
    prepared: OnceLock<Arc<PreparedStylesheetFragment<'static>>>,
    font_variants: Mutex<VecDeque<(Vec<String>, ResourceStylesheet)>>,
    sheet: Mutex<Option<StyleSheet<'static>>>,
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
pub struct ResourceImport(Arc<ResourceImportData>);

struct ResourceImportData {
    rule: ImportRule<'static>,
    variants: Mutex<VecDeque<(Vec<u64>, ResourceStylesheet)>>,
}

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
                entries.push(ResourceStylesheetEntry::Import(ResourceImport(Arc::new(
                    ResourceImportData {
                        rule: import,
                        variants: Mutex::new(VecDeque::new()),
                    },
                ))));
            } else {
                rules.push(rule);
            }
        }
        Self::flush(&mut rules, &mut entries);
        entries
    }

    fn from_rules(rules: Vec<CssRule<'static>>) -> Self {
        let sheet = StyleSheet::new(Vec::new(), CssRuleList(rules), ParserOptions::default());
        let mut has_relative_fonts = false;
        visit_relative_font_urls(&sheet.rules, &mut |_| {
            has_relative_fonts = true;
        });
        Self(Arc::new(ResourceRules {
            id: NEXT_STYLESHEET_ID.fetch_add(1, Ordering::Relaxed),
            prepared: OnceLock::new(),
            font_variants: Mutex::new(VecDeque::new()),
            sheet: Mutex::new(Some(sheet)),
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
        let mut resolved_urls = Vec::new();
        // Resolve against the caller's resource context before consulting the
        // cache: the same source can be used with different URL mappings.
        let mut changed = false;
        self.with_stylesheet(|sheet| {
            visit_relative_font_urls(&sheet.rules, &mut |url| {
                let resolved = resolve(url);
                changed |= resolved != url;
                resolved_urls.push(resolved);
            })
        });
        if !changed {
            return self;
        }
        let mut variants = self.0.font_variants.lock().expect("font URL variant cache");
        if let Some((_, variant)) =
            super::inline::cached_entry(&mut variants, |(urls, _)| *urls == resolved_urls)
        {
            return variant.clone();
        }
        let mut sheet = self.stylesheet();
        let mut replacements = resolved_urls.iter();
        resolve_rule_urls(&mut sheet.rules, &mut |_| replacements.next().cloned());
        let variant = Self::from_rules(sheet.rules.0);
        if variants.len() == 8 {
            variants.pop_front();
        }
        variants.push_back((resolved_urls, variant.clone()));
        variant
    }

    pub(crate) fn id(&self) -> u64 {
        self.0.id
    }

    pub(crate) fn prepared(&self) -> Arc<PreparedStylesheetFragment<'static>> {
        Arc::clone(self.0.prepared.get_or_init(|| {
            let sheet = self
                .0
                .sheet
                .lock()
                .expect("stylesheet source")
                .take()
                .expect("source is consumed once during preparation");
            Arc::new(PreparedStylesheetFragment::compile(sheet))
        }))
    }

    pub(crate) fn parse_complete(css: &str) -> Option<Self> {
        super::parse_stylesheet(css).map(|sheet| Self::from_rules(sheet.rules.0))
    }

    fn flush(rules: &mut Vec<CssRule<'static>>, entries: &mut Vec<ResourceStylesheetEntry>) {
        if !rules.is_empty() {
            entries.push(ResourceStylesheetEntry::Rules(Self::from_rules(
                std::mem::take(rules),
            )));
        }
    }

    /// Serialize on explicit request; program caches use the resource identity.
    pub fn css(&self) -> &str {
        self.0.css.get_or_init(|| {
            self.with_stylesheet(|sheet| {
                sheet
                    .to_css(PrinterOptions::default())
                    .expect("parsed CSS can be serialized")
                    .code
            })
        })
    }

    pub(crate) fn stylesheet(&self) -> StyleSheet<'static> {
        self.with_stylesheet(Clone::clone)
    }

    fn with_stylesheet<R>(&self, read: impl FnOnce(&StyleSheet<'static>) -> R) -> R {
        let source = self.0.sheet.lock().expect("stylesheet source");
        if let Some(sheet) = source.as_ref() {
            return read(sheet);
        }
        drop(source);
        // Only explicit source operations reconstruct the AST after compilation.
        read(&self.0.prepared.wait().stylesheet())
    }

    fn into_rules(self) -> Vec<CssRule<'static>> {
        match Arc::try_unwrap(self.0) {
            Ok(rules) => match rules.sheet.into_inner().expect("stylesheet source") {
                Some(sheet) => sheet.rules.0,
                None => {
                    rules
                        .prepared
                        .into_inner()
                        .expect("prepared source")
                        .stylesheet()
                        .rules
                        .0
                }
            },
            Err(rules) => Self(rules).stylesheet().rules.0,
        }
    }
}

impl ResourceImport {
    pub fn url(&self) -> &str {
        self.0.rule.url.as_ref()
    }

    pub(super) fn description(&self) -> StylesheetImport {
        let import = &self.0.rule;
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
        let ids: Vec<_> = sheets.iter().map(ResourceStylesheet::id).collect();
        let mut variants = self.0.variants.lock().expect("import variant cache");
        if let Some((_, variant)) =
            super::inline::cached_entry(&mut variants, |(inputs, _)| *inputs == ids)
        {
            return variant.clone();
        }
        let mut rules = CssRuleList(
            sheets
                .into_iter()
                .flat_map(ResourceStylesheet::into_rules)
                .collect(),
        );
        let import = &self.0.rule;
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
        let variant = ResourceStylesheet::from_rules(rules.0);
        if variants.len() == 8 {
            variants.pop_front();
        }
        variants.push_back((ids, variant.clone()));
        variant
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
            let pointer = sheet
                .0
                .sheet
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .rules
                .0
                .as_ptr();
            let expected = sheet.css().to_owned();
            let (mut rules, allocations) = count_allocations(|| sheet.into_rules());
            if let Some(retained) = retained {
                assert_ne!(rules.as_ptr(), pointer);
                rules.clear();
                assert_eq!(retained.css(), expected);
                assert!(!retained.stylesheet().rules.0.is_empty());
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
