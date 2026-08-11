use super::*;

#[allow(unused_variables)]
pub(super) fn apply(context: &mut PropertyContext<'_, '_>, property: &Property<'_>) -> bool {
    let doc = context.doc;
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    let resolved_root_font_size = context.resolved_root_font_size;
    let parent_font_size = context.parent_font_size;
    let parent_font_weight = context.parent_font_weight;
    let parent_color = context.parent_color;
    let environment = context.environment;
    match property {
        // Color
        Property::Color(c) => {
            // On the `color` property itself, currentColor computes as the
            // inherited color. It must not observe an earlier declaration in
            // the same cascade phase.
            style.text.color = css_color_to_u32(c, parent_color);
        }
        Property::BackgroundColor(c) => {
            style.background.background_color = css_color_to_u32(c, style.text.color);
            style.background.background_color_current_color = matches!(c, CssColor::CurrentColor);
        }
        Property::Background(bg) => {
            style.background.background_image_present =
                crate::style::syntax::capabilities::property_uses_background_image(property);
            // The color may only appear in the shorthand's last layer; earlier
            // layers carry the transparent default.
            if let Some(layer) = bg.last() {
                style.background.background_color =
                    css_color_to_u32(&layer.color, style.text.color);
                style.background.background_color_current_color =
                    matches!(layer.color, CssColor::CurrentColor);
            }
        }
        Property::BackgroundImage(_) => {
            style.background.background_image_present = false;
        }
        _ => return false,
    }
    true
}
