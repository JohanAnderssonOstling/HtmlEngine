//! Renderer-neutral color palettes resolved from native stylesheet rules.

/// Palette selection within a color font's CPAL table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontPaletteBase {
    #[default]
    Normal,
    Index(u16),
    Light,
    Dark,
}

/// An active named palette. Entries are RGBA colors and retain declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontPaletteDefinition {
    pub name: String,
    pub family: String,
    pub base: FontPaletteBase,
    pub colors: Vec<(u16, [u8; 4])>,
}
