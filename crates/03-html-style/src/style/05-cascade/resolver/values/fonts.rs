use html_style_model::{OpenTypeFeature, OpenTypeFeatureKind, open_type};
use lightningcss::properties::font::{AbsoluteFontWeight, FontWeight};

pub(in crate::style::cascade::resolver) fn checked_font_weight(
    weight: &FontWeight,
    parent_weight: u16,
) -> Option<u16> {
    let value = match weight {
        FontWeight::Absolute(abs) => match abs {
            AbsoluteFontWeight::Normal => 400,
            AbsoluteFontWeight::Bold => 700,
            AbsoluteFontWeight::Weight(weight)
                if weight.is_finite() && *weight >= 1.0 && *weight <= 1000.0 =>
            {
                *weight as u16
            }
            AbsoluteFontWeight::Weight(_) => return None,
        },
        FontWeight::Bolder => match parent_weight {
            0..=349 => 400,
            350..=549 => 700,
            _ => 900,
        },
        FontWeight::Lighter => match parent_weight {
            0..=549 => 100,
            550..=749 => 400,
            _ => 700,
        },
    };
    Some(value)
}

// CSS math may produce positive infinity for a non-negative numeric property.
// Keep the computed-style/layout boundary finite while preserving the fact
// that such a factor dominates ordinary authored values. The cap also avoids
// overflow when the layout backend sums several large flex factors.
pub(in crate::style::cascade::resolver) const MAX_COMPUTED_FLEX_FACTOR: f32 = 1_000_000.0;

pub(in crate::style::cascade::resolver) fn computed_flex_factor(value: f32) -> f32 {
    // Source validation rejects negative literals. Calculated results clamp
    // at computed-value time; NaN is censored to zero.
    value.max(0.0).min(MAX_COMPUTED_FLEX_FACTOR)
}

pub(in crate::style::cascade::resolver) fn font_variant_caps_features(
    value: &lightningcss::properties::font::FontVariantCaps,
) -> Vec<OpenTypeFeature> {
    use lightningcss::properties::font::FontVariantCaps;
    match value {
        FontVariantCaps::Normal => Vec::new(),
        FontVariantCaps::SmallCaps => vec![open_type::open_type_feature_new(
            OpenTypeFeatureKind::SmallCaps,
            1,
        )],
        FontVariantCaps::AllSmallCaps => vec![
            open_type::open_type_feature_new(OpenTypeFeatureKind::SmallCaps, 1),
            open_type::open_type_feature_new(OpenTypeFeatureKind::CapitalsToSmallCaps, 1),
        ],
        FontVariantCaps::PetiteCaps => {
            vec![open_type::open_type_feature_new(
                OpenTypeFeatureKind::PetiteCaps,
                1,
            )]
        }
        FontVariantCaps::AllPetiteCaps => vec![
            open_type::open_type_feature_new(OpenTypeFeatureKind::PetiteCaps, 1),
            open_type::open_type_feature_new(OpenTypeFeatureKind::CapitalsToPetiteCaps, 1),
        ],
        FontVariantCaps::Unicase => vec![open_type::open_type_feature_new(
            OpenTypeFeatureKind::Unicase,
            1,
        )],
        FontVariantCaps::TitlingCaps => vec![open_type::open_type_feature_new(
            OpenTypeFeatureKind::TitlingCapitals,
            1,
        )],
    }
}
