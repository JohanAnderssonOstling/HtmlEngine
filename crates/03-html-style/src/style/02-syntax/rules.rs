//! Stylesheet syntax delegated to Lightning CSS.
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
pub(crate) fn stylesheet_is_valid(css: &str) -> bool {
    StyleSheet::parse(css, ParserOptions::default()).is_ok()
}
