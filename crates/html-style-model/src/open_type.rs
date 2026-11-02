//! Named OpenType features and validated custom feature settings.

/// Font features interpreted by the engine. Custom font-defined tags are
/// accepted separately by [`OpenTypeFeature::from_tag_bytes`].
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OpenTypeFeatureKind {
    GlyphCompositionDecomposition = u32::from_be_bytes(*b"ccmp"),
    RequiredLigatures = u32::from_be_bytes(*b"rlig"),
    RequiredContextualAlternates = u32::from_be_bytes(*b"rclt"),
    MarkPositioning = u32::from_be_bytes(*b"mark"),
    MarkToMarkPositioning = u32::from_be_bytes(*b"mkmk"),
    Kerning = u32::from_be_bytes(*b"kern"),
    StandardLigatures = u32::from_be_bytes(*b"liga"),
    ContextualLigatures = u32::from_be_bytes(*b"clig"),
    DiscretionaryLigatures = u32::from_be_bytes(*b"dlig"),
    HistoricalLigatures = u32::from_be_bytes(*b"hlig"),
    ContextualAlternates = u32::from_be_bytes(*b"calt"),
    SmallCaps = u32::from_be_bytes(*b"smcp"),
    CapitalsToSmallCaps = u32::from_be_bytes(*b"c2sc"),
    PetiteCaps = u32::from_be_bytes(*b"pcap"),
    CapitalsToPetiteCaps = u32::from_be_bytes(*b"c2pc"),
    Unicase = u32::from_be_bytes(*b"unic"),
    TitlingCapitals = u32::from_be_bytes(*b"titl"),
    LiningFigures = u32::from_be_bytes(*b"lnum"),
    OldstyleFigures = u32::from_be_bytes(*b"onum"),
    ProportionalFigures = u32::from_be_bytes(*b"pnum"),
    TabularFigures = u32::from_be_bytes(*b"tnum"),
    DiagonalFractions = u32::from_be_bytes(*b"frac"),
    StackedFractions = u32::from_be_bytes(*b"afrc"),
    Ordinals = u32::from_be_bytes(*b"ordn"),
    SlashedZero = u32::from_be_bytes(*b"zero"),
}

impl OpenTypeFeatureKind {
    /// Encode a named feature for a native shaping API.
    pub const fn tag_bytes(self) -> [u8; 4] {
        (self as u32).to_be_bytes()
    }

    /// Recognize an engine-defined feature. Custom tags return `None`.
    pub const fn from_tag_bytes(tag: [u8; 4]) -> Option<Self> {
        match u32::from_be_bytes(tag) {
            value if value == Self::GlyphCompositionDecomposition as u32 => {
                Some(Self::GlyphCompositionDecomposition)
            }
            value if value == Self::RequiredLigatures as u32 => Some(Self::RequiredLigatures),
            value if value == Self::RequiredContextualAlternates as u32 => {
                Some(Self::RequiredContextualAlternates)
            }
            value if value == Self::MarkPositioning as u32 => Some(Self::MarkPositioning),
            value if value == Self::MarkToMarkPositioning as u32 => {
                Some(Self::MarkToMarkPositioning)
            }
            value if value == Self::Kerning as u32 => Some(Self::Kerning),
            value if value == Self::StandardLigatures as u32 => Some(Self::StandardLigatures),
            value if value == Self::ContextualLigatures as u32 => Some(Self::ContextualLigatures),
            value if value == Self::DiscretionaryLigatures as u32 => {
                Some(Self::DiscretionaryLigatures)
            }
            value if value == Self::HistoricalLigatures as u32 => Some(Self::HistoricalLigatures),
            value if value == Self::ContextualAlternates as u32 => Some(Self::ContextualAlternates),
            value if value == Self::SmallCaps as u32 => Some(Self::SmallCaps),
            value if value == Self::CapitalsToSmallCaps as u32 => Some(Self::CapitalsToSmallCaps),
            value if value == Self::PetiteCaps as u32 => Some(Self::PetiteCaps),
            value if value == Self::CapitalsToPetiteCaps as u32 => Some(Self::CapitalsToPetiteCaps),
            value if value == Self::Unicase as u32 => Some(Self::Unicase),
            value if value == Self::TitlingCapitals as u32 => Some(Self::TitlingCapitals),
            value if value == Self::LiningFigures as u32 => Some(Self::LiningFigures),
            value if value == Self::OldstyleFigures as u32 => Some(Self::OldstyleFigures),
            value if value == Self::ProportionalFigures as u32 => Some(Self::ProportionalFigures),
            value if value == Self::TabularFigures as u32 => Some(Self::TabularFigures),
            value if value == Self::DiagonalFractions as u32 => Some(Self::DiagonalFractions),
            value if value == Self::StackedFractions as u32 => Some(Self::StackedFractions),
            value if value == Self::Ordinals as u32 => Some(Self::Ordinals),
            value if value == Self::SlashedZero as u32 => Some(Self::SlashedZero),
            _ => None,
        }
    }
}

/// One OpenType feature setting. Zero disables the feature; larger values are
/// interpreted by the font and can select alternate glyphs, rather than toggles.
/// The compact tag encoding also retains custom features without allocating.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OpenTypeFeature {
    tag: [u8; 4],
    value: u32,
}

impl OpenTypeFeature {
    pub const fn new(kind: OpenTypeFeatureKind, value: u32) -> Self {
        Self {
            tag: kind.tag_bytes(),
            value,
        }
    }

    /// Accept a custom CSS/native feature only if its tag is printable ASCII.
    pub const fn from_tag_bytes(tag: [u8; 4], value: u32) -> Option<Self> {
        let mut index = 0;
        while index < tag.len() {
            if tag[index] < 0x20 || tag[index] > 0x7e {
                return None;
            }
            index += 1;
        }
        Some(Self { tag, value })
    }

    pub const fn kind(self) -> Option<OpenTypeFeatureKind> {
        OpenTypeFeatureKind::from_tag_bytes(self.tag)
    }

    /// Encode this feature for a native shaping API, including custom tags.
    pub const fn tag_bytes(self) -> [u8; 4] {
        self.tag
    }

    pub const fn value(self) -> u32 {
        self.value
    }
}
