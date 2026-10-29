use super::super::context::{ApplyResult, PropertyContext};
use super::super::values::fonts::{checked_font_size_components, checked_font_weight};
use super::super::values::{intern_font_family, rebind_line_height};
use html_style_model::FontStyle;
use lightningcss::properties::Property;

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let resolution = context.resolution;
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    let root_font_size = context.root_font_size;
    let parent_font_size = context.parent.font.font_size;
    let parent_font_weight = context.parent.font.font_weight;
    match property {
        // Font properties
        Property::FontSize(size) => {
            let Some(components) = checked_font_size_components(
                size,
                &style.font,
                parent_font_size,
                root_font_size,
                resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            [
                style.font.font_size,
                style.font.font_size_x_height_px,
                style.font.font_size_ch_advance_px,
                style.font.font_size_cap_height_px,
                style.font.font_size_root_ch,
                style.font.font_size_root_cap_height,
                style.font.font_size_root_line_height,
            ] = components;
            rebind_line_height(style, || root_font_size, resolution);
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

        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
