use super::*;

pub(in crate::style::cascade::resolver) fn viewport_length_to_px(
    length: &LengthValue,
    resolution: &ResolutionContext,
) -> Option<f32> {
    if matches!(
        length,
        LengthValue::Vw(_) | LengthValue::Vh(_) | LengthValue::Vmin(_) | LengthValue::Vmax(_)
    ) {
        resolution.uses_viewport_units.set(true);
    }
    let environment = resolution.environment;
    let width = environment.viewport_width() as f32;
    let value = match length {
        LengthValue::Vw(value) => value * width / 100.0,
        LengthValue::Vh(value) => value * environment.viewport_height()? as f32 / 100.0,
        LengthValue::Vmin(value) => {
            value * width.min(environment.viewport_height()? as f32) / 100.0
        }
        LengthValue::Vmax(value) => {
            value * width.max(environment.viewport_height()? as f32) / 100.0
        }
        _ => return None,
    };
    value.is_finite().then_some(value)
}

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
    let value = font_size_to_px(size, parent_font_size, root_font_size, resolution)?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

pub(in crate::style::cascade::resolver) fn checked_font_size_components(
    size: &FontSize,
    inherited_font: &Font,
    parent_font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<(f32, f32, f32, f32, f32, f32, f32)> {
    if let FontSize::Length(LengthPercentage::Dimension(length)) = size {
        return match length {
            LengthValue::Cap(value) if value.is_finite() && *value >= 0.0 => Some((
                0.0,
                0.0,
                0.0,
                *value * inherited_font.font_size,
                0.0,
                0.0,
                0.0,
            )),
            LengthValue::Rch(value) if value.is_finite() && *value >= 0.0 => {
                Some((0.0, 0.0, 0.0, 0.0, *value, 0.0, 0.0))
            }
            LengthValue::Rcap(value) if value.is_finite() && *value >= 0.0 => {
                Some((0.0, 0.0, 0.0, 0.0, 0.0, *value, 0.0))
            }
            LengthValue::Rlh(value) if value.is_finite() && *value >= 0.0 => {
                Some((0.0, 0.0, 0.0, 0.0, 0.0, 0.0, *value))
            }
            _ => checked_font_size(size, parent_font_size, root_font_size, resolution)
                .map(|value| (value, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)),
        };
    }
    if let FontSize::Length(LengthPercentage::Calc(calc)) = size {
        let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) =
            calc_box_length_percentage_components(
                calc,
                inherited_font.font_size,
                root_font_size,
                resolution,
            )?;
        let absolute_px = absolute_px + percentage * parent_font_size;
        return (absolute_px.is_finite()
            && absolute_px >= 0.0
            && x_height_px.is_finite()
            && ch_advance_px.is_finite()
            && cap_height_px.is_finite())
        .then_some((
            absolute_px,
            x_height_px,
            ch_advance_px,
            cap_height_px,
            0.0,
            0.0,
            0.0,
        ));
    }
    checked_font_size(size, parent_font_size, root_font_size, resolution)
        .map(|value| (value, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
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

pub(in crate::style::cascade::resolver) struct CheckedFontShorthand {
    pub(in crate::style::cascade::resolver) font_size: f32,
    pub(in crate::style::cascade::resolver) font_weight: u16,
    pub(in crate::style::cascade::resolver) line_height: f32,
    pub(in crate::style::cascade::resolver) line_height_x_height_px: f32,
    pub(in crate::style::cascade::resolver) line_height_number: f32,
    pub(in crate::style::cascade::resolver) line_height_normal: bool,
}

pub(in crate::style::cascade::resolver) fn checked_font_shorthand(
    font: &CssFont<'_>,
    parent_font_size: f32,
    parent_font_weight: u16,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<CheckedFontShorthand> {
    let font_size = checked_font_size(&font.size, parent_font_size, root_font_size, resolution)?;
    let font_weight = checked_font_weight(&font.weight, parent_font_weight)?;
    let (line_height, line_height_x_height_px) =
        checked_line_height_components(&font.line_height, font_size, root_font_size, resolution)?;
    let line_height_number = line_height_number(&font.line_height);
    let line_height_normal = line_height_is_normal(&font.line_height);
    Some(CheckedFontShorthand {
        font_size,
        font_weight,
        line_height,
        line_height_x_height_px,
        line_height_number,
        line_height_normal,
    })
}

pub(in crate::style::cascade::resolver) fn computed_line_height(
    font: &Font,
    text: &InheritedText,
) -> Option<f32> {
    // Keep `lh` aligned with the renderer's established used-value behavior
    // for `normal` (the same 1.2 multiplier used by shaping and inline layout).
    let value = if text.line_height_normal {
        font.font_size * 1.2
    } else {
        text.line_height
    };
    value.is_finite().then_some(value)
}

/// CSS Values makes `lh` in font-size and line-height refer to the parent's
/// computed line height, avoiding a dependency cycle. Other properties use
/// the element's own (already prerequisite-resolved) computed line height.
pub(in crate::style::cascade::resolver) fn set_line_height_resolution_bases(
    doc: &Document,
    styles: &ComputedStylesBuilder,
    property: &Property<'_>,
    style: &WorkingStyle,
    parent: &ParentStyle,
    resolution: &ResolutionContext,
) {
    let (font, text) = if matches!(
        property,
        Property::FontSize(_) | Property::LineHeight(_) | Property::Font(_)
    ) {
        (&parent.font, &parent.text)
    } else {
        (&style.font, &style.text)
    };
    resolution.line_height.set(computed_line_height(font, text));

    let root_line_height = stored_root_line_height(doc, styles).or_else(|| {
        // While resolving the root itself, font-size and line-height must
        // use their initial basis to avoid a cycle. Other root properties
        // can use the prerequisite-resolved root line height.
        if matches!(
            property,
            Property::FontSize(_) | Property::LineHeight(_) | Property::Font(_)
        ) {
            let initial = ParentStyle::initial(doc.root_font_size());
            computed_line_height(&initial.font, &initial.text)
        } else {
            computed_line_height(&style.font, &style.text)
        }
    });
    resolution.root_line_height.set(root_line_height);
}

fn stored_root_line_height(doc: &Document, styles: &ComputedStylesBuilder) -> Option<f32> {
    let indices = styles.style_for_node(doc.dom_root()?)?;
    computed_line_height(styles.font_style(indices)?, styles.text_style(indices)?)
}

pub(in crate::style::cascade::resolver) fn set_marker_resolution_bases(
    doc: &Document,
    styles: &ComputedStylesBuilder,
    style: &WorkingStyle,
    resolution: &ResolutionContext,
) {
    resolution
        .line_height
        .set(computed_line_height(&style.font, &style.text));
    resolution.root_line_height.set(
        stored_root_line_height(doc, styles)
            .or_else(|| computed_line_height(&style.font, &style.text)),
    );
}

// CSS math may produce positive infinity for a non-negative numeric property.
// Keep the computed-style/layout boundary finite while preserving the fact
// that such a factor dominates ordinary authored values. The cap also avoids
// overflow when the layout backend sums several large flex factors.
pub(in crate::style::cascade::resolver) const MAX_COMPUTED_FLEX_FACTOR: f32 = 1_000_000.0;

pub(in crate::style::cascade::resolver) fn checked_flex_factor(value: f32) -> Option<f32> {
    if value.is_nan() || value < 0.0 {
        None
    } else if value.is_infinite() {
        value.is_sign_positive().then_some(MAX_COMPUTED_FLEX_FACTOR)
    } else {
        Some(value.min(MAX_COMPUTED_FLEX_FACTOR))
    }
}
