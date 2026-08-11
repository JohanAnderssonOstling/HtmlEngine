//! Plain-data import discovery for the orchestration boundary.
//!
//! Resource access belongs to `html-pipeline`, while CSS parsing belongs here.
//! This adapter exposes import URLs and serialized wrappers without exposing
//! Lightning CSS AST nodes to the pipeline crate.

use super::declarations;
use lightningcss::printer::PrinterOptions;
use lightningcss::rules::CssRule;
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use lightningcss::traits::ToCss;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportLayer {
    Anonymous,
    Named(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StylesheetImport {
    pub url: String,
    pub layer: Option<ImportLayer>,
    pub supports: Option<String>,
    pub media: Option<String>,
}

impl StylesheetImport {
    /// Recreate the import's conditional and layer context around resolved CSS.
    pub fn wrap_resolved_css(&self, css: &str) -> String {
        let mut wrapped = css.to_owned();
        if let Some(media) = &self.media {
            wrapped = format!("@media {media} {{ {wrapped} }}");
        }
        if let Some(supports) = &self.supports {
            wrapped = format!("@supports {supports} {{ {wrapped} }}");
        }
        if let Some(layer) = &self.layer {
            wrapped = match layer {
                ImportLayer::Anonymous => format!("@layer {{ {wrapped} }}"),
                ImportLayer::Named(name) => format!("@layer {name} {{ {wrapped} }}"),
            };
        }
        wrapped
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StylesheetEntry {
    Css(String),
    Import(StylesheetImport),
}

/// Split a stylesheet at top-level imports while preserving rule order.
/// Stylesheets with no import use a no-reserialization fast path.
pub fn stylesheet_entries(css: &str) -> Vec<StylesheetEntry> {
    if !contains_import_at_rule(css) {
        return vec![StylesheetEntry::Css(css.to_owned())];
    }

    let normalized = declarations::normalize(css);
    let options = ParserOptions { error_recovery: true, ..ParserOptions::default() };
    let Ok(stylesheet) = StyleSheet::parse(&normalized, options) else {
        return vec![StylesheetEntry::Css(css.to_owned())];
    };
    if !stylesheet.rules.0.iter().any(|rule| matches!(rule, CssRule::Import(_))) {
        return vec![StylesheetEntry::Css(css.to_owned())];
    }

    let mut entries = Vec::new();
    for rule in &stylesheet.rules.0 {
        match rule {
            CssRule::Import(import) => {
                let layer = match &import.layer {
                    None => None,
                    Some(None) => Some(ImportLayer::Anonymous),
                    Some(Some(name)) => Some(ImportLayer::Named(name.to_css_string(PrinterOptions::default()).unwrap_or_default())),
                };
                let supports = import.supports.as_ref().and_then(|condition| condition.to_css_string(PrinterOptions::default()).ok());
                let media = (!import.media.media_queries.is_empty()).then(|| import.media.to_css_string(PrinterOptions::default()).ok()).flatten();
                entries.push(StylesheetEntry::Import(StylesheetImport { url: import.url.as_ref().to_owned(), layer, supports, media }));
            }
            _ => {
                let Ok(serialized) = rule.to_css_string(PrinterOptions::default()) else {
                    return vec![StylesheetEntry::Css(css.to_owned())];
                };
                push_css(&mut entries, serialized);
            }
        }
    }
    entries
}

fn push_css(entries: &mut Vec<StylesheetEntry>, css: String) {
    if let Some(StylesheetEntry::Css(previous)) = entries.last_mut() {
        previous.push(' ');
        previous.push_str(&css);
    } else {
        entries.push(StylesheetEntry::Css(css));
    }
}

fn contains_import_at_rule(css: &str) -> bool {
    css.as_bytes().windows(7).any(|window| window.eq_ignore_ascii_case(b"@import"))
}

#[cfg(test)]
mod tests {
    use super::{ImportLayer, StylesheetEntry, stylesheet_entries};

    #[test]
    fn discovers_imports_without_leaking_parser_types() {
        let entries = stylesheet_entries("@import 'base.css' layer(theme) supports(width: 1px) screen; p { color: red }");
        let StylesheetEntry::Import(import) = &entries[0] else { panic!("first entry should be an import") };
        assert_eq!(import.url, "base.css");
        assert_eq!(import.layer, Some(ImportLayer::Named("theme".to_owned())));
        assert!(import.supports.as_deref().is_some_and(|condition| condition.contains("width")));
        assert_eq!(import.media.as_deref(), Some("screen"));
        assert!(matches!(&entries[1], StylesheetEntry::Css(css) if css.contains("color: red")));
    }

    #[test]
    fn resolved_css_is_wrapped_in_import_context() {
        let StylesheetEntry::Import(import) = stylesheet_entries("@import 'base.css' layer supports(width: 1px) screen;").remove(0) else { panic!("import") };
        let wrapped = import.wrap_resolved_css("p { color: red }");
        assert!(wrapped.starts_with("@layer"));
        assert!(wrapped.contains("@supports"));
        assert!(wrapped.contains("@media screen"));
    }

    #[test]
    fn ordinary_stylesheets_take_the_exact_text_fast_path() {
        let css = "p { color: red; }";
        assert_eq!(stylesheet_entries(css), vec![StylesheetEntry::Css(css.to_owned())]);
    }
}
