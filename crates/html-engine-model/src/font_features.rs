//! Engine-resolved font controls for renderer backends.

use html_style_model::{OpenTypeFeature, OpenTypeFeatureKind};

/// Concrete requests after CSS defaults, overrides and tracking are resolved.
/// False requests that a control be disabled. Font-native
/// numeral forms are preserved when both corresponding alternate controls are
/// false. Adapters apply supported controls without filling in CSS defaults.
/// Returned geometry must describe the actual shaping result, including any
/// substitutions or positioning, regardless of which controls were supported.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontFeatures<'a> {
    pub glyph_composition_decomposition: bool,
    pub required_ligatures: bool,
    pub required_contextual_alternates: bool,
    pub mark_positioning: bool,
    pub mark_to_mark_positioning: bool,
    pub kerning: bool,
    pub standard_ligatures: bool,
    pub contextual_ligatures: bool,
    pub discretionary_ligatures: bool,
    pub historical_ligatures: bool,
    pub contextual_alternates: bool,
    pub small_caps: bool,
    pub capitals_to_small_caps: bool,
    pub petite_caps: bool,
    pub capitals_to_petite_caps: bool,
    pub unicase: bool,
    pub titling_capitals: bool,
    pub lining_figures: bool,
    pub oldstyle_figures: bool,
    pub proportional_figures: bool,
    pub tabular_figures: bool,
    pub diagonal_fractions: bool,
    pub stacked_fractions: bool,
    pub ordinals: bool,
    pub slashed_zero: bool,
    /// Additional font-specific settings. Backends may ignore unsupported tags.
    /// All named features above have already been removed from this slice.
    pub custom_features: &'a [OpenTypeFeature],
}

impl<'a> FontFeatures<'a> {
    /// The engine's typography policy before CSS overrides.
    pub const DEFAULT: Self = Self {
        glyph_composition_decomposition: true,
        required_ligatures: true,
        required_contextual_alternates: true,
        mark_positioning: true,
        mark_to_mark_positioning: true,
        kerning: true,
        standard_ligatures: true,
        contextual_ligatures: true,
        discretionary_ligatures: false,
        historical_ligatures: false,
        contextual_alternates: true,
        small_caps: false,
        capitals_to_small_caps: false,
        petite_caps: false,
        capitals_to_petite_caps: false,
        unicase: false,
        titling_capitals: false,
        lining_figures: false,
        oldstyle_figures: false,
        proportional_figures: false,
        tabular_figures: false,
        diagonal_fractions: false,
        stacked_fractions: false,
        ordinals: false,
        slashed_zero: false,
        custom_features: &[],
    };
}

impl Default for FontFeatures<'_> {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Resolve named controls and retain custom settings in the existing allocation.
/// The returned record borrows that allocation for the lifetime of the request.
pub(crate) fn resolve_font_features<'a>(
    settings: &'a mut Vec<OpenTypeFeature>,
    explicit: &[OpenTypeFeature],
    has_tracking: bool,
) -> FontFeatures<'a> {
    let mut resolved = FontFeatures::DEFAULT;
    settings.retain(|feature| match feature.kind() {
        Some(OpenTypeFeatureKind::GlyphCompositionDecomposition) => {
            resolved.glyph_composition_decomposition = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::RequiredLigatures) => {
            resolved.required_ligatures = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::RequiredContextualAlternates) => {
            resolved.required_contextual_alternates = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::MarkPositioning) => {
            resolved.mark_positioning = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::MarkToMarkPositioning) => {
            resolved.mark_to_mark_positioning = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::Kerning) => {
            resolved.kerning = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::StandardLigatures) => {
            resolved.standard_ligatures = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::ContextualLigatures) => {
            resolved.contextual_ligatures = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::DiscretionaryLigatures) => {
            resolved.discretionary_ligatures = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::HistoricalLigatures) => {
            resolved.historical_ligatures = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::ContextualAlternates) => {
            resolved.contextual_alternates = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::SmallCaps) => {
            resolved.small_caps = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::CapitalsToSmallCaps) => {
            resolved.capitals_to_small_caps = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::PetiteCaps) => {
            resolved.petite_caps = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::CapitalsToPetiteCaps) => {
            resolved.capitals_to_petite_caps = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::Unicase) => {
            resolved.unicase = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::TitlingCapitals) => {
            resolved.titling_capitals = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::LiningFigures) => {
            resolved.lining_figures = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::OldstyleFigures) => {
            resolved.oldstyle_figures = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::ProportionalFigures) => {
            resolved.proportional_figures = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::TabularFigures) => {
            resolved.tabular_figures = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::DiagonalFractions) => {
            resolved.diagonal_fractions = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::StackedFractions) => {
            resolved.stacked_fractions = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::Ordinals) => {
            resolved.ordinals = feature.value() != 0;
            false
        }
        Some(OpenTypeFeatureKind::SlashedZero) => {
            resolved.slashed_zero = feature.value() != 0;
            false
        }
        None => true,
    });
    if has_tracking {
        if !explicit.iter().any(|feature| {
            feature.tag_bytes() == OpenTypeFeatureKind::StandardLigatures.tag_bytes()
        }) {
            resolved.standard_ligatures = false;
        }
        if !explicit.iter().any(|feature| {
            feature.tag_bytes() == OpenTypeFeatureKind::ContextualLigatures.tag_bytes()
        }) {
            resolved.contextual_ligatures = false;
        }
        if !explicit.iter().any(|feature| {
            feature.tag_bytes() == OpenTypeFeatureKind::DiscretionaryLigatures.tag_bytes()
        }) {
            resolved.discretionary_ligatures = false;
        }
        if !explicit.iter().any(|feature| {
            feature.tag_bytes() == OpenTypeFeatureKind::HistoricalLigatures.tag_bytes()
        }) {
            resolved.historical_ligatures = false;
        }
    }
    resolved.custom_features = settings.as_slice();
    resolved
}
