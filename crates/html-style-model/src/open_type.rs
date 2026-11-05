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

/// One OpenType feature setting. Zero disables the feature; larger values are
/// interpreted by the font and can select alternate glyphs, rather than toggles.
/// The compact tag encoding also retains custom features without allocating.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OpenTypeFeature {
    tag: [u8; 4],
    value: u32,
}

/// Recognize an engine-defined feature. Custom tags return `None`.
pub const fn open_type_feature_kind_from_tag_bytes(tag: [u8; 4]) -> Option<OpenTypeFeatureKind> {
    match u32::from_be_bytes(tag) {
        value if value == OpenTypeFeatureKind::GlyphCompositionDecomposition as u32 => {
            Some(OpenTypeFeatureKind::GlyphCompositionDecomposition)
        }
        value if value == OpenTypeFeatureKind::RequiredLigatures as u32 => {
            Some(OpenTypeFeatureKind::RequiredLigatures)
        }
        value if value == OpenTypeFeatureKind::RequiredContextualAlternates as u32 => {
            Some(OpenTypeFeatureKind::RequiredContextualAlternates)
        }
        value if value == OpenTypeFeatureKind::MarkPositioning as u32 => {
            Some(OpenTypeFeatureKind::MarkPositioning)
        }
        value if value == OpenTypeFeatureKind::MarkToMarkPositioning as u32 => {
            Some(OpenTypeFeatureKind::MarkToMarkPositioning)
        }
        value if value == OpenTypeFeatureKind::Kerning as u32 => Some(OpenTypeFeatureKind::Kerning),
        value if value == OpenTypeFeatureKind::StandardLigatures as u32 => {
            Some(OpenTypeFeatureKind::StandardLigatures)
        }
        value if value == OpenTypeFeatureKind::ContextualLigatures as u32 => {
            Some(OpenTypeFeatureKind::ContextualLigatures)
        }
        value if value == OpenTypeFeatureKind::DiscretionaryLigatures as u32 => {
            Some(OpenTypeFeatureKind::DiscretionaryLigatures)
        }
        value if value == OpenTypeFeatureKind::HistoricalLigatures as u32 => {
            Some(OpenTypeFeatureKind::HistoricalLigatures)
        }
        value if value == OpenTypeFeatureKind::ContextualAlternates as u32 => {
            Some(OpenTypeFeatureKind::ContextualAlternates)
        }
        value if value == OpenTypeFeatureKind::SmallCaps as u32 => {
            Some(OpenTypeFeatureKind::SmallCaps)
        }
        value if value == OpenTypeFeatureKind::CapitalsToSmallCaps as u32 => {
            Some(OpenTypeFeatureKind::CapitalsToSmallCaps)
        }
        value if value == OpenTypeFeatureKind::PetiteCaps as u32 => {
            Some(OpenTypeFeatureKind::PetiteCaps)
        }
        value if value == OpenTypeFeatureKind::CapitalsToPetiteCaps as u32 => {
            Some(OpenTypeFeatureKind::CapitalsToPetiteCaps)
        }
        value if value == OpenTypeFeatureKind::Unicase as u32 => Some(OpenTypeFeatureKind::Unicase),
        value if value == OpenTypeFeatureKind::TitlingCapitals as u32 => {
            Some(OpenTypeFeatureKind::TitlingCapitals)
        }
        value if value == OpenTypeFeatureKind::LiningFigures as u32 => {
            Some(OpenTypeFeatureKind::LiningFigures)
        }
        value if value == OpenTypeFeatureKind::OldstyleFigures as u32 => {
            Some(OpenTypeFeatureKind::OldstyleFigures)
        }
        value if value == OpenTypeFeatureKind::ProportionalFigures as u32 => {
            Some(OpenTypeFeatureKind::ProportionalFigures)
        }
        value if value == OpenTypeFeatureKind::TabularFigures as u32 => {
            Some(OpenTypeFeatureKind::TabularFigures)
        }
        value if value == OpenTypeFeatureKind::DiagonalFractions as u32 => {
            Some(OpenTypeFeatureKind::DiagonalFractions)
        }
        value if value == OpenTypeFeatureKind::StackedFractions as u32 => {
            Some(OpenTypeFeatureKind::StackedFractions)
        }
        value if value == OpenTypeFeatureKind::Ordinals as u32 => {
            Some(OpenTypeFeatureKind::Ordinals)
        }
        value if value == OpenTypeFeatureKind::SlashedZero as u32 => {
            Some(OpenTypeFeatureKind::SlashedZero)
        }
        _ => None,
    }
}

/// Encode this feature for a native shaping API, including custom tags.
pub const fn open_type_feature_tag_bytes(value: OpenTypeFeature) -> [u8; 4] {
    value.tag
}

/// Accept a custom CSS/native feature only if its tag is printable ASCII.
pub const fn open_type_feature_from_tag_bytes(tag: [u8; 4], value: u32) -> Option<OpenTypeFeature> {
    let mut index = 0;
    while index < tag.len() {
        if tag[index] < 0x20 || tag[index] > 0x7e {
            return None;
        }
        index += 1;
    }
    Some(OpenTypeFeature { tag, value })
}

pub const fn open_type_feature_kind(value: OpenTypeFeature) -> Option<OpenTypeFeatureKind> {
    open_type_feature_kind_from_tag_bytes(value.tag)
}

pub const fn open_type_feature_value(value: OpenTypeFeature) -> u32 {
    value.value
}

pub const fn open_type_feature_new(kind: OpenTypeFeatureKind, value: u32) -> OpenTypeFeature {
    OpenTypeFeature {
        tag: open_type_feature_kind_tag_bytes(kind),
        value,
    }
}

/// Encode a named feature for a native shaping API.
pub const fn open_type_feature_kind_tag_bytes(value: OpenTypeFeatureKind) -> [u8; 4] {
    (value as u32).to_be_bytes()
}
