/// A renderer glyph-cache index in a derived shaping buffer.
///
/// Renderer IDs are distinct from the canonical Unicode source in html-text.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GlyphId(pub u32);
