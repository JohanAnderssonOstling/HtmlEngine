//! Engine partitioning of preferences for preparation reuse and paint updates.
//! Application callers submit the complete preference record to the document.

use crate::ReaderStyleOverrides;

/// Preferences that can affect text metrics, box generation, or geometry.
pub fn layout_overrides(preferences: &ReaderStyleOverrides) -> ReaderStyleOverrides {
    ReaderStyleOverrides {
        foreground: None,
        link_foreground: None,
        background: None,
        theme: None,
        ..preferences.clone()
    }
}

/// Color preferences applied to the retained publication styles.
pub fn paint_overrides(preferences: &ReaderStyleOverrides) -> ReaderStyleOverrides {
    ReaderStyleOverrides {
        foreground: preferences.foreground,
        link_foreground: preferences.link_foreground,
        background: preferences.background,
        theme: preferences.theme,
        ..ReaderStyleOverrides::default()
    }
}

pub fn same_paint_preferences(
    preferences: &ReaderStyleOverrides,
    other: &ReaderStyleOverrides,
) -> bool {
    preferences.foreground == other.foreground
        && preferences.link_foreground == other.link_foreground
        && preferences.background == other.background
        && preferences.theme == other.theme
}

pub fn same_layout_preferences(
    preferences: &ReaderStyleOverrides,
    other: &ReaderStyleOverrides,
) -> bool {
    preferences.hyphens == other.hyphens
        && preferences.font_weight_adjustment == other.font_weight_adjustment
        && preferences.font_family == other.font_family
        && preferences.cjk_font_family == other.cjk_font_family
        && preferences.cjk_font_fallback == other.cjk_font_fallback
        && preferences.serif_font_family == other.serif_font_family
        && preferences.sans_serif_font_family == other.sans_serif_font_family
        && preferences.monospace_font_family == other.monospace_font_family
        && preferences.text_align == other.text_align
        && preferences.word_spacing_factor.map(f32::to_bits)
            == other.word_spacing_factor.map(f32::to_bits)
        && preferences.line_height.map(f32::to_bits) == other.line_height.map(f32::to_bits)
        && preferences.paragraph_spacing_em.map(f32::to_bits)
            == other.paragraph_spacing_em.map(f32::to_bits)
        && preferences.first_line_indent_em.map(f32::to_bits)
            == other.first_line_indent_em.map(f32::to_bits)
        && preferences.canonical_font_size.map(f32::to_bits)
            == other.canonical_font_size.map(f32::to_bits)
        && preferences.minimum_font_size.map(f32::to_bits)
            == other.minimum_font_size.map(f32::to_bits)
        && preferences.strip_root_spacing == other.strip_root_spacing
}
