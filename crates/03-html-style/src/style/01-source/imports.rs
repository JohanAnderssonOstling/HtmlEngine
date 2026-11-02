//! Plain-data import discovery for the orchestration boundary.
//!
//! Resource access belongs to `html-pipeline`, while CSS parsing belongs here.
//! This adapter exposes import URLs and serialized wrappers without exposing
//! Lightning CSS AST nodes to the pipeline crate.

use super::resource::{ResourceStylesheet, ResourceStylesheetEntry};

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
        let contexts = [
            self.layer.as_ref().map(|layer| match layer {
                ImportLayer::Anonymous => ("@layer", ""),
                ImportLayer::Named(name) => ("@layer ", name.as_str()),
            }),
            self.supports.as_deref().map(|value| ("@supports ", value)),
            self.media.as_deref().map(|value| ("@media ", value)),
        ];
        let capacity = css.len()
            + contexts
                .iter()
                .flatten()
                .map(|(prefix, value)| prefix.len() + value.len() + " {  }".len())
                .sum::<usize>();
        let mut wrapped = String::with_capacity(capacity);
        for (prefix, value) in contexts.iter().flatten() {
            wrapped.push_str(prefix);
            wrapped.push_str(value);
            wrapped.push_str(" { ");
        }
        wrapped.push_str(css);
        for _ in contexts.iter().flatten() {
            wrapped.push_str(" }");
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

    let entries = ResourceStylesheet::parse(css);
    if !entries.iter().any(|entry| matches!(entry, ResourceStylesheetEntry::Import(_))) {
        return vec![StylesheetEntry::Css(css.to_owned())];
    }
    entries.into_iter().map(|entry| match entry {
        ResourceStylesheetEntry::Rules(sheet) => StylesheetEntry::Css(sheet.css().to_owned()),
        ResourceStylesheetEntry::Import(import) => StylesheetEntry::Import(import.description()),
    }).collect()
}

fn contains_import_at_rule(css: &str) -> bool {
    css.as_bytes().windows(7).any(|window| window.eq_ignore_ascii_case(b"@import"))
        || css.contains('\\')
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
