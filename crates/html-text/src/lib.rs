//! CSS logical-text preparation after styling and formatting-structure construction.
//!
//! PreparedText owns canonical characters, source offsets, wrap flags, and
//! pseudo-style runs. Font shaping and geometry layout borrow this immutable
//! result; this crate does not own font resources or computed box geometry.
#![forbid(unsafe_code)]

mod content;
mod first_letter;
mod line_breaks;
mod normalization;
mod prepared_text;
mod pseudo_styles;
mod source_offsets;
mod spacing;

pub use content::{
    InlineContent, InlineItem, InlineItemKind, InlineTextKind, InlineTextRun, WhitespaceWrapOverride,
};
pub use first_letter::{FirstLetterPart, FirstLetterSelection};
use html_style_model::StyleIndices;
pub use normalization::{get_style, whitespace_context_root};
pub use prepared_text::{FirstLetterStyles, PreparedText, StyleOverrides};
pub use pseudo_styles::PseudoStyles;
pub use spacing::{
    spaced_advance, suppresses_glyph, tracking_boundary_after, tracking_boundary_allowed,
};

#[cfg(feature = "testing")]
pub use normalization::{collapse_whitespace, is_css_collapsible_space, transform_text};
#[cfg(feature = "testing")]
pub use pseudo_styles::{first_letter_overrides_with_characters, first_letter_style_overrides};

/// Read-only formatting ancestry required by CSS text policies.
///
/// Implementations expose stable context and style handles, never used sizes,
/// positions, font resources, or a reference to a layout executor.
pub trait TextStructure {
    fn get_box_parent(&self, context: usize) -> Option<usize>;
    fn get_box_style_indices(&self, context: usize) -> Option<StyleIndices>;
    fn get_box_dom_element(&self, context: usize) -> Option<u32>;
    fn is_inline(&self, context: usize) -> bool;
    fn is_flex_or_grid(&self, context: usize) -> bool;
}

/// Unicode directional controls retained in logical source, without visible advance.
pub fn is_bidi_control(character: char) -> bool {
    matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}
