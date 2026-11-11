use super::super::context::{ApplyResult, PropertyContext};
use super::super::values::fonts::computed_flex_factor;
use super::super::values::grid::grid_auto_flow;
use super::super::values::{flex_direction, flex_wrap, justify_content};
use lightningcss::properties::Property;

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let style = &mut *context.style;
    match property {
        Property::FlexDirection(value, _) => style.layout.flex_direction = flex_direction(value),
        Property::FlexWrap(value, _) => style.layout.flex_wrap = flex_wrap(value),

        Property::FlexGrow(value, _) => {
            style.layout.flex_grow = computed_flex_factor(*value);
        }
        Property::FlexShrink(value, _) => {
            style.layout.flex_shrink = computed_flex_factor(*value);
        }
        Property::Order(value, _) => style.layout.order = *value,
        Property::JustifyContent(value, _) => style.layout.justify_content = justify_content(value),
        Property::GridAutoFlow(value) => style.layout.grid_auto_flow = grid_auto_flow(*value),
        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
