//! GPUI rendering for the HTML engine, independent of reader pagination.
//!
//! [`HtmlFragmentElement`] lays out a continuous block at its available width
//! and natural height. Its parent owns scrolling. HTML and supplied CSS own
//! typography and colors; `.justify()` selects the engine's Knuth–Plass line
//! breaking for paragraphs.
//! `.text_color()` explicitly overrides text paint while preserving CSS
//! typography and non-text colors.
//!
//! Applications select their GPUI platform backend and features. This crate
//! depends on GPUI with default features disabled and needs no reader session.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

mod fonts;
mod fragment;
mod fragment_selection;
mod math_svg;
mod paint;
mod raster_theme;
mod svg_theme;
mod text;

pub use fragment::HtmlFragmentElement;
pub use text::TextShapeCacheStats;

/// Shaping and painting shared with adapters that own document navigation.
pub mod backend {
    pub use crate::fonts::{PublicationFonts, SharedPublicationFonts, publication_faces_for_overrides};
    pub use html_fonts::{LoadedFontFace, face_key, is_private_font_family, load_font_face, load_remote_font};
    #[cfg(not(target_arch = "wasm32"))]
    pub use html_fonts::configure_cache_directory;
    pub use crate::paint::{GpuiCommandPainter, ImageCache, PaintCommand, paint_commands};
    pub use crate::text::{GlyphStorage, GlyphStore, GpuiTextCache};
}
