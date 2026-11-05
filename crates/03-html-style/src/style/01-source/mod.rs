//! Stage 1: stylesheet sources and native parser adapters.

use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use static_self::IntoOwned;

pub mod fonts;
pub mod imports;
pub mod resource;

pub const DEFAULT_CSS: &str = include_str!("default.css");
pub(crate) mod inline;

/// Canonical author-sheet parser shared by direct and resource-backed inputs.
pub(crate) fn parse_stylesheet(css: &str) -> Option<StyleSheet<'static>> {
    StyleSheet::parse(
        css,
        ParserOptions {
            error_recovery: true,
            ..ParserOptions::default()
        },
    )
    .map(IntoOwned::into_owned)
    .ok()
}
