use super::super::context::{ApplyResult, PropertyContext};
use super::super::values::lengths::line_style_to_border_style;
use super::super::values::{inline_sides, set_border_css_color, set_border_style};
use html_style_model::TextDirection;
use lightningcss::properties::{Property, PropertyId};

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    let direction = style.text(styles).direction;
    match property {
        Property::ZIndex(value) => {
            use lightningcss::properties::position::ZIndex;
            style.layout.z_index = match value {
                ZIndex::Auto => None,
                ZIndex::Integer(value) => Some(*value),
            };
        }
        Property::BorderTopColor(value)
        | Property::BorderRightColor(value)
        | Property::BorderBottomColor(value)
        | Property::BorderLeftColor(value)
        | Property::BorderBlockStartColor(value)
        | Property::BorderBlockEndColor(value)
        | Property::BorderInlineStartColor(value)
        | Property::BorderInlineEndColor(value) => {
            set_border_css_color(style, styles, physical_side(property, direction), value)
        }

        Property::BorderTopStyle(value)
        | Property::BorderRightStyle(value)
        | Property::BorderBottomStyle(value)
        | Property::BorderLeftStyle(value)
        | Property::BorderBlockStartStyle(value)
        | Property::BorderBlockEndStyle(value)
        | Property::BorderInlineStartStyle(value)
        | Property::BorderInlineEndStyle(value) => set_border_style(
            style,
            physical_side(property, direction),
            line_style_to_border_style(value),
        ),

        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}

pub(in crate::style::cascade::resolver) fn physical_side(
    property: &Property<'_>,
    direction: TextDirection,
) -> usize {
    match property.property_id() {
        PropertyId::Top
        | PropertyId::MarginTop
        | PropertyId::MarginBlockStart
        | PropertyId::PaddingTop
        | PropertyId::PaddingBlockStart
        | PropertyId::BorderTopWidth
        | PropertyId::BorderBlockStartWidth
        | PropertyId::BorderTopStyle
        | PropertyId::BorderBlockStartStyle
        | PropertyId::BorderTopColor
        | PropertyId::BorderBlockStartColor => 0,
        PropertyId::Right
        | PropertyId::MarginRight
        | PropertyId::PaddingRight
        | PropertyId::BorderRightWidth
        | PropertyId::BorderRightStyle
        | PropertyId::BorderRightColor => 1,
        PropertyId::Bottom
        | PropertyId::MarginBottom
        | PropertyId::MarginBlockEnd
        | PropertyId::PaddingBottom
        | PropertyId::PaddingBlockEnd
        | PropertyId::BorderBottomWidth
        | PropertyId::BorderBlockEndWidth
        | PropertyId::BorderBottomStyle
        | PropertyId::BorderBlockEndStyle
        | PropertyId::BorderBottomColor
        | PropertyId::BorderBlockEndColor => 2,
        PropertyId::Left
        | PropertyId::MarginLeft
        | PropertyId::PaddingLeft
        | PropertyId::BorderLeftWidth
        | PropertyId::BorderLeftStyle
        | PropertyId::BorderLeftColor => 3,
        PropertyId::MarginInlineStart
        | PropertyId::PaddingInlineStart
        | PropertyId::BorderInlineStartWidth
        | PropertyId::BorderInlineStartStyle
        | PropertyId::BorderInlineStartColor => inline_sides(direction).0,
        PropertyId::MarginInlineEnd
        | PropertyId::PaddingInlineEnd
        | PropertyId::BorderInlineEndWidth
        | PropertyId::BorderInlineEndStyle
        | PropertyId::BorderInlineEndColor => inline_sides(direction).1,
        _ => unreachable!("property has a physical or logical side"),
    }
}
