//! Owned `@font-face` declarations shared by style preparation and text backends.

use std::ops::RangeInclusive;

/// A usable `@font-face` declaration. Hosts load and register these faces under
/// `family`, which may differ from the font file's internal family name.
/// Faces retain stylesheet order, including multiple faces for the same family.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    /// Decoded family name, without CSS quotes or escapes.
    pub family: String,
    /// Alternatives in authored order. Try the next source if one is unavailable
    /// or its format/technology is unsupported by the host.
    pub sources: Vec<FontFaceSource>,
    /// Inclusive CSS weight range; defaults to 400..=400.
    pub weight: RangeInclusive<f32>,
    pub style: FontFaceStyle,
    /// Inclusive percentages, where 100 is normal width.
    pub stretch: RangeInclusive<f32>,
    /// Inclusive Unicode code point ranges. Defaults to U+0-10FFFF.
    /// The host intersects this coverage with the font's actual character map.
    pub unicode_ranges: Vec<RangeInclusive<u32>>,
    /// Other descriptors, preserved as CSS for host policy (e.g. font-display).
    /// The engine does not apply these descriptors.
    pub additional_descriptors: Vec<FontFaceDescriptor>,
    /// Explicit descriptors constrain variable axes; omission uses font capabilities.
    pub style_explicit: bool,
    pub stretch_explicit: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FontFaceStyle {
    Normal,
    Italic,
    /// Inclusive angles in degrees; plain `oblique` is 14..=14.
    Oblique(RangeInclusive<f32>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FontFaceSource {
    /// A decoded local face name, without CSS quotes or escapes.
    Local(String),
    Url {
        /// Decoded URL. Pipeline APIs resolve this using ResourceProvider.
        url: String,
        /// Decoded format hint, e.g. "woff2". Unknown hints are preserved.
        format: Option<String>,
        /// Required technology keywords, e.g. "variations".
        technologies: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontFaceDescriptor {
    pub name: String,
    pub value: String,
}
