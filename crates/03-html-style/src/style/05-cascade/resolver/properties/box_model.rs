use super::super::values::inline_sides;
use html_style_model::TextDirection;
use crate::style::property_catalogue::PropertyKey;

pub(in crate::style::cascade::resolver) fn physical_side(
    key: PropertyKey,
    direction: TextDirection,
) -> usize {
    match key {
        PropertyKey::Top
        | PropertyKey::MarginTop
        | PropertyKey::MarginBlockStart
        | PropertyKey::PaddingTop
        | PropertyKey::PaddingBlockStart
        | PropertyKey::BorderTopWidth
        | PropertyKey::BorderBlockStartWidth
        | PropertyKey::BorderTopStyle
        | PropertyKey::BorderBlockStartStyle
        | PropertyKey::BorderTopColor
        | PropertyKey::BorderBlockStartColor => 0,
        PropertyKey::Right
        | PropertyKey::MarginRight
        | PropertyKey::PaddingRight
        | PropertyKey::BorderRightWidth
        | PropertyKey::BorderRightStyle
        | PropertyKey::BorderRightColor => 1,
        PropertyKey::Bottom
        | PropertyKey::MarginBottom
        | PropertyKey::MarginBlockEnd
        | PropertyKey::PaddingBottom
        | PropertyKey::PaddingBlockEnd
        | PropertyKey::BorderBottomWidth
        | PropertyKey::BorderBlockEndWidth
        | PropertyKey::BorderBottomStyle
        | PropertyKey::BorderBlockEndStyle
        | PropertyKey::BorderBottomColor
        | PropertyKey::BorderBlockEndColor => 2,
        PropertyKey::Left
        | PropertyKey::MarginLeft
        | PropertyKey::PaddingLeft
        | PropertyKey::BorderLeftWidth
        | PropertyKey::BorderLeftStyle
        | PropertyKey::BorderLeftColor => 3,
        PropertyKey::MarginInlineStart
        | PropertyKey::PaddingInlineStart
        | PropertyKey::BorderInlineStartWidth
        | PropertyKey::BorderInlineStartStyle
        | PropertyKey::BorderInlineStartColor => inline_sides(direction).0,
        PropertyKey::MarginInlineEnd
        | PropertyKey::PaddingInlineEnd
        | PropertyKey::BorderInlineEndWidth
        | PropertyKey::BorderInlineEndStyle
        | PropertyKey::BorderInlineEndColor => inline_sides(direction).1,
        _ => unreachable!("property has a physical or logical side"),
    }
}
