//! Stylesheet syntax as understood by the parser used for rendering.

use crate::style::source::{declarations, imports};
use lightningcss::stylesheet::{ParserOptions, StyleSheet};

pub(crate) fn stylesheet_is_valid(css: &str) -> bool {
    let Some(css) = declarations::normalize_rule_preludes(css) else {
        return false;
    };
    let normalized = imports::normalize_layers(&css);
    StyleSheet::parse(&normalized, ParserOptions::default()).is_ok()
}
