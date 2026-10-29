use super::super::context::{ApplyResult, PropertyContext};
use super::super::values::colors::css_color_to_u32;
use lightningcss::properties::Property;
use lightningcss::values::color::CssColor;

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let style = &mut *context.style;
    let parent_color = context.parent.text.color;
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

        Property::BackgroundImage(_) => {
            style.background.background_image_present = false;
        }
        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
