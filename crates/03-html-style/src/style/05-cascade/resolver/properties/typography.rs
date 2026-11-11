use super::super::context::{ApplyResult, PropertyContext};
use super::super::values::intern_font_family;
use lightningcss::properties::Property;

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    match property {
        Property::FontFamily(families) => {
            style.font_mut(styles).font_family = Some(intern_font_family(styles, families));
        }

        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
