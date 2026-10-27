use super::*;
use crate::style::syntax::font_features::FontFeatureSetting;

pub(in crate::style::cascade::resolver) fn feature(tag: &[u8; 4], value: u32) -> OpenTypeFeature {
    OpenTypeFeature::new(*tag, value)
}

pub(in crate::style::cascade::resolver) fn font_variant_caps_features(
    value: &lightningcss::properties::font::FontVariantCaps,
) -> Vec<OpenTypeFeature> {
    use lightningcss::properties::font::FontVariantCaps;
    match value {
        FontVariantCaps::Normal => Vec::new(),
        FontVariantCaps::SmallCaps => vec![feature(b"smcp", 1)],
        FontVariantCaps::AllSmallCaps => vec![feature(b"smcp", 1), feature(b"c2sc", 1)],
        FontVariantCaps::PetiteCaps => vec![feature(b"pcap", 1)],
        FontVariantCaps::AllPetiteCaps => vec![feature(b"pcap", 1), feature(b"c2pc", 1)],
        FontVariantCaps::Unicase => vec![feature(b"unic", 1)],
        FontVariantCaps::TitlingCaps => vec![feature(b"titl", 1)],
    }
}

pub(in crate::style::cascade::resolver) fn resolve_font_feature_settings(
    settings: &[FontFeatureSetting],
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<Vec<OpenTypeFeature>> {
    let mut features: Vec<OpenTypeFeature> = Vec::with_capacity(settings.len());
    for setting in settings.iter().rev() {
        if features.iter().any(|feature| feature.tag() == setting.tag) {
            continue;
        }
        let value = resolve_integer(
            &setting.value,
            0..=u32::MAX,
            font_size,
            root_font_size,
            resolution,
        )?;
        features.push(OpenTypeFeature::new(setting.tag, value));
    }
    features.sort_unstable_by_key(|feature| feature.tag());
    Some(features)
}
