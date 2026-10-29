use super::super::context::ResolutionContext;
use super::checked_line_height_components;
use super::lengths::{calc_box_length_percentage_components, length_percentage_to_px};
use html_style_model::{Font, OpenTypeFeature};
use lightningcss::properties::font::{AbsoluteFontWeight, Font as CssFont, FontSize, FontWeight};
use lightningcss::values::length::LengthValue;

pub(in crate::style::cascade::resolver) fn font_size_to_px(
    size: &FontSize,
    parent_font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    match size {
        FontSize::Length(lp) => {
            length_percentage_to_px(lp, parent_font_size, root_font_size, resolution)
        }
        FontSize::Absolute(abs) => Some({
            use lightningcss::properties::font::AbsoluteFontSize;
            match abs {
                AbsoluteFontSize::XXSmall => 9.0,
                AbsoluteFontSize::XSmall => 10.0,
                AbsoluteFontSize::Small => 13.0,
                AbsoluteFontSize::Medium => 16.0,
                AbsoluteFontSize::Large => 18.0,
                AbsoluteFontSize::XLarge => 24.0,
                AbsoluteFontSize::XXLarge => 32.0,
                AbsoluteFontSize::XXXLarge => 48.0,
            }
        }),
        FontSize::Relative(rel) => Some({
            use lightningcss::properties::font::RelativeFontSize;
            match rel {
                RelativeFontSize::Smaller => parent_font_size * 0.833,
                RelativeFontSize::Larger => parent_font_size * 1.2,
            }
        }),
    }
}

pub(in crate::style::cascade::resolver) fn checked_font_size(
    size: &FontSize,
    parent_font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    let scale = resolution.font_size_scale;
    let value = resolution.with_canonical_font_sizes(|canonical| {
        font_size_to_px(
            size,
            parent_font_size / scale,
            root_font_size / scale,
            canonical,
        )
    })? * scale;
    value.is_finite().then_some(value.max(0.0))
}

/// Coefficients for px, ex, ch, cap, rch, rcap, and rlh, in that order.
pub(in crate::style::cascade::resolver) fn checked_font_size_components(
    size: &FontSize,
    inherited_font: &Font,
    parent_font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<[f32; 7]> {
    use FontSize::Length;
    use LengthValue::{Cap, Rcap, Rch, Rlh};
    use lightningcss::values::percentage::DimensionPercentage::{Calc, Dimension};

    let scale = resolution.font_size_scale;
    let mut result = resolution.with_canonical_font_sizes(|resolution| {
        let inherited_font_size = inherited_font.font_size / scale;
        let parent_font_size = parent_font_size / scale;
        let root_font_size = root_font_size / scale;
        match size {
            Length(Dimension(Cap(value))) if value.is_finite() && *value >= 0.0 => {
                Some([0.0, 0.0, 0.0, *value * inherited_font_size, 0.0, 0.0, 0.0])
            }
            Length(Dimension(Rch(value))) if value.is_finite() && *value >= 0.0 => {
                Some([0.0, 0.0, 0.0, 0.0, *value, 0.0, 0.0])
            }
            Length(Dimension(Rcap(value))) if value.is_finite() && *value >= 0.0 => {
                Some([0.0, 0.0, 0.0, 0.0, 0.0, *value, 0.0])
            }
            Length(Dimension(Rlh(value))) if value.is_finite() && *value >= 0.0 => {
                Some([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, *value])
            }
            Length(Calc(calc)) => {
                let [
                    absolute_px,
                    percentage,
                    x_height_px,
                    ch_advance_px,
                    cap_height_px,
                ] = calc_box_length_percentage_components(
                    calc,
                    inherited_font_size,
                    root_font_size,
                    resolution,
                )?;
                let absolute_px = absolute_px + percentage * parent_font_size;
                let absolute_px =
                    if x_height_px == 0.0 && ch_advance_px == 0.0 && cap_height_px == 0.0 {
                        absolute_px.max(0.0)
                    } else {
                        absolute_px
                    };
                (absolute_px.is_finite()
                    && absolute_px >= 0.0
                    && x_height_px.is_finite()
                    && ch_advance_px.is_finite()
                    && cap_height_px.is_finite())
                .then_some([
                    absolute_px,
                    x_height_px,
                    ch_advance_px,
                    cap_height_px,
                    0.0,
                    0.0,
                    0.0,
                ])
            }
            _ => checked_font_size(size, parent_font_size, root_font_size, resolution)
                .map(|value| [value, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        }
    })?;
    // Root metric coefficients already consume the scaled root font at layout.
    for value in &mut result[..4] {
        *value *= scale;
    }
    result[..4]
        .iter()
        .all(|value| value.is_finite())
        .then_some(result)
}

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

pub(in crate::style::cascade::resolver) fn checked_font_shorthand(
    font: &CssFont<'_>,
    parent_font_size: f32,
    parent_font_weight: u16,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<()> {
    let font_size = checked_font_size(&font.size, parent_font_size, root_font_size, resolution)?;
    checked_font_weight(&font.weight, parent_font_weight)?;
    checked_line_height_components(&font.line_height, font_size, root_font_size, resolution)?;
    Some(())
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
        FontVariantCaps::SmallCaps => vec![OpenTypeFeature::new(*b"smcp", 1)],
        FontVariantCaps::AllSmallCaps => vec![
            OpenTypeFeature::new(*b"smcp", 1),
            OpenTypeFeature::new(*b"c2sc", 1),
        ],
        FontVariantCaps::PetiteCaps => vec![OpenTypeFeature::new(*b"pcap", 1)],
        FontVariantCaps::AllPetiteCaps => vec![
            OpenTypeFeature::new(*b"pcap", 1),
            OpenTypeFeature::new(*b"c2pc", 1),
        ],
        FontVariantCaps::Unicase => vec![OpenTypeFeature::new(*b"unic", 1)],
        FontVariantCaps::TitlingCaps => vec![OpenTypeFeature::new(*b"titl", 1)],
    }
}
