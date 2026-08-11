use super::*;

#[allow(unused_variables)]
pub(super) fn apply(context: &mut PropertyContext<'_, '_>, property: &Property<'_>) -> ApplyResult {
    let doc = context.doc;
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    let resolved_root_font_size = context.resolved_root_font_size;
    let parent_font_size = context.parent_font_size;
    let parent_font_weight = context.parent_font_weight;
    let parent_color = context.parent_color;
    let environment = context.environment;
    match property {
        // Font properties
        Property::FontSize(size) => {
            let Some((
                font_size,
                x_height_px,
                ch_advance_px,
                cap_height_px,
                root_ch,
                root_cap_height,
                root_line_height,
            )) = checked_font_size_components(
                size,
                &style.font,
                parent_font_size,
                resolved_root_font_size,
                environment,
            )
            else {
                return ApplyResult::Invalid;
            };
            style.font.font_size = font_size;
            style.font.font_size_x_height_px = x_height_px;
            style.font.font_size_ch_advance_px = ch_advance_px;
            style.font.font_size_cap_height_px = cap_height_px;
            style.font.font_size_root_ch = root_ch;
            style.font.font_size_root_cap_height = root_cap_height;
            style.font.font_size_root_line_height = root_line_height;
            if let Some(spec) = style.line_height_spec.as_ref() {
                if let Some((line_height, x_height_px)) =
                    checked_line_height_components(spec, style.font.font_size, doc.root_font_size())
                {
                    style.text.line_height = line_height;
                    style.text.line_height_x_height_px = x_height_px;
                }
            } else if style.text.line_height_number != 0.0 {
                // An inherited unitless line-height re-resolves against this
                // element's own font size.
                style.text.line_height = style.font.font_size * style.text.line_height_number;
                style.text.line_height_x_height_px = 0.0;
            }
        }
        Property::FontWeight(weight) => {
            let Some(weight) = checked_font_weight(weight, parent_font_weight) else {
                return ApplyResult::Invalid;
            };
            style.font.font_weight = weight;
        }
        Property::FontStyle(fs) => {
            use lightningcss::properties::font::FontStyle as LcFontStyle;
            style.font.font_style = match fs {
                LcFontStyle::Normal => FontStyle::Normal,
                LcFontStyle::Italic => FontStyle::Italic,
                LcFontStyle::Oblique(_) => FontStyle::Oblique,
            };
        }
        Property::FontFamily(families) => {
            style.font.font_family = Some(intern_font_family(styles, families));
        }
        Property::Font(font) => {
            let Some(values) = checked_font_shorthand(
                font,
                parent_font_size,
                parent_font_weight,
                doc.root_font_size(),
                environment,
            ) else {
                return ApplyResult::Invalid;
            };
            style.font.font_size = values.font_size;
            style.font.font_size_x_height_px = 0.0;
            style.font.font_size_ch_advance_px = 0.0;
            style.font.font_size_cap_height_px = 0.0;
            style.font.font_size_root_ch = 0.0;
            style.font.font_size_root_cap_height = 0.0;
            style.font.font_size_root_line_height = 0.0;
            style.font.font_weight = values.font_weight;
            use lightningcss::properties::font::FontStyle as LcFontStyle;
            style.font.font_style = match font.style {
                LcFontStyle::Normal => FontStyle::Normal,
                LcFontStyle::Italic => FontStyle::Italic,
                LcFontStyle::Oblique(_) => FontStyle::Oblique,
            };
            style.line_height_spec = Some(font.line_height.clone());
            style.text.line_height_number = values.line_height_number;
            style.text.line_height = values.line_height;
            style.text.line_height_x_height_px = values.line_height_x_height_px;
            style.text.line_height_normal = values.line_height_normal;
            style.font.font_family = Some(intern_font_family(styles, &font.family));
            style.font.font_variant_caps_features = font_variant_caps_features(&font.variant_caps);
            style.font.font_variant_numeric_features.clear();
            style.font.font_variant_ligature_features.clear();
            style.font.font_kerning_features.clear();
            style.font.font_feature_settings.clear();
        }
        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
