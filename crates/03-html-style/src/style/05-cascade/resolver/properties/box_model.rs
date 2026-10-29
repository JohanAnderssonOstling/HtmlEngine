use super::super::context::{ApplyResult, PropertyContext};
use super::super::values::lengths::{
    border_width, inset_value, length_or_auto_to_lengthpct, length_to_px_from_length,
    line_style_to_border_style, margin_value, max_size_to_preferred, size_to_preferred,
};
use super::super::values::{
    corner_radius, inline_sides, non_negative_length_pct, non_negative_preferred_size,
    set_border_css_color, set_border_style, set_border_width, set_corner_radius, set_margin_side,
    set_padding_side,
};
use html_style_model::{AspectRatio as ComputedAspectRatio, PositionMode, TextDirection};
use lightningcss::properties::{Property, PropertyId};

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    let font_size = style.font.font_size;
    let root_font_size = context.root_font_size;
    let resolution = context.resolution;
    match property {
        Property::Width(value)
        | Property::Height(value)
        | Property::InlineSize(value)
        | Property::BlockSize(value)
        | Property::MinWidth(value)
        | Property::MinHeight(value)
        | Property::MinInlineSize(value)
        | Property::MinBlockSize(value) => {
            let Some(value) =
                size_to_preferred(value, &style.font, root_font_size, styles, resolution)
                    .and_then(non_negative_preferred_size)
            else {
                return ApplyResult::Invalid;
            };
            *match property {
                Property::Width(_) | Property::InlineSize(_) => &mut style.box_model.width,
                Property::Height(_) | Property::BlockSize(_) => &mut style.box_model.height,
                Property::MinWidth(_) | Property::MinInlineSize(_) => {
                    &mut style.box_model.min_width
                }
                Property::MinHeight(_) | Property::MinBlockSize(_) => {
                    &mut style.box_model.min_height
                }
                _ => unreachable!(),
            } = value;
        }
        Property::MaxWidth(value)
        | Property::MaxHeight(value)
        | Property::MaxInlineSize(value)
        | Property::MaxBlockSize(value) => {
            let Some(value) =
                max_size_to_preferred(value, &style.font, root_font_size, styles, resolution)
                    .and_then(non_negative_preferred_size)
            else {
                return ApplyResult::Invalid;
            };
            *match property {
                Property::MaxWidth(_) | Property::MaxInlineSize(_) => {
                    &mut style.box_model.max_width
                }
                Property::MaxHeight(_) | Property::MaxBlockSize(_) => {
                    &mut style.box_model.max_height
                }
                _ => unreachable!(),
            } = value;
        }
        Property::AspectRatio(value) => {
            let components = value
                .ratio
                .as_ref()
                .map(|ratio| (ratio.0.max(0.0), ratio.1.max(0.0)));
            let Some(value) = ComputedAspectRatio::new(value.auto, components) else {
                return ApplyResult::Invalid;
            };
            style.box_model.aspect_ratio = value;
        }
        Property::BorderSpacing(spacing) => {
            let Some(horizontal) = length_to_px_from_length(
                &spacing.0,
                style.font.font_size,
                root_font_size,
                resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            let Some(vertical) = length_to_px_from_length(
                &spacing.1,
                style.font.font_size,
                root_font_size,
                resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            if !horizontal.is_finite()
                || !vertical.is_finite()
                || horizontal < 0.0
                || vertical < 0.0
            {
                return ApplyResult::Invalid;
            }
            style.box_model.border_spacing_horizontal = horizontal;
            style.box_model.border_spacing_vertical = vertical;
        }
        Property::Position(position) => {
            use lightningcss::properties::position::Position;
            style.layout.position = match position {
                Position::Static => PositionMode::Static,
                Position::Relative => PositionMode::Relative,
                Position::Absolute => PositionMode::Absolute,
                Position::Fixed | Position::Sticky(_) => return ApplyResult::Invalid,
            };
        }
        Property::ZIndex(value) => {
            use lightningcss::properties::position::ZIndex;
            style.layout.z_index = match value {
                ZIndex::Auto => None,
                ZIndex::Integer(value) => Some(*value),
            };
        }
        Property::Top(value)
        | Property::Right(value)
        | Property::Bottom(value)
        | Property::Left(value) => {
            let Some(value) = inset_value(value, font_size, root_font_size, resolution) else {
                return ApplyResult::Invalid;
            };
            *match physical_side(property, style.text.direction) {
                0 => &mut style.layout.inset_top,
                1 => &mut style.layout.inset_right,
                2 => &mut style.layout.inset_bottom,
                3 => &mut style.layout.inset_left,
                _ => unreachable!(),
            } = value;
        }

        Property::MarginTop(value)
        | Property::MarginRight(value)
        | Property::MarginBottom(value)
        | Property::MarginLeft(value)
        | Property::MarginBlockStart(value)
        | Property::MarginBlockEnd(value)
        | Property::MarginInlineStart(value)
        | Property::MarginInlineEnd(value) => {
            let Some(value) = margin_value(value, font_size, root_font_size, resolution) else {
                return ApplyResult::Invalid;
            };
            set_margin_side(style, physical_side(property, style.text.direction), value);
        }

        Property::PaddingTop(value)
        | Property::PaddingRight(value)
        | Property::PaddingBottom(value)
        | Property::PaddingLeft(value)
        | Property::PaddingBlockStart(value)
        | Property::PaddingBlockEnd(value)
        | Property::PaddingInlineStart(value)
        | Property::PaddingInlineEnd(value) => {
            let Some(value) =
                length_or_auto_to_lengthpct(value, font_size, root_font_size, resolution)
                    .and_then(non_negative_length_pct)
            else {
                return ApplyResult::Invalid;
            };
            set_padding_side(style, physical_side(property, style.text.direction), value);
        }

        Property::BorderTopWidth(value)
        | Property::BorderRightWidth(value)
        | Property::BorderBottomWidth(value)
        | Property::BorderLeftWidth(value)
        | Property::BorderBlockStartWidth(value)
        | Property::BorderBlockEndWidth(value)
        | Property::BorderInlineStartWidth(value)
        | Property::BorderInlineEndWidth(value) => {
            let Some(value) = border_width(value, font_size, root_font_size, resolution) else {
                return ApplyResult::Invalid;
            };
            set_border_width(style, physical_side(property, style.text.direction), value);
        }

        Property::BorderTopColor(value)
        | Property::BorderRightColor(value)
        | Property::BorderBottomColor(value)
        | Property::BorderLeftColor(value)
        | Property::BorderBlockStartColor(value)
        | Property::BorderBlockEndColor(value)
        | Property::BorderInlineStartColor(value)
        | Property::BorderInlineEndColor(value) => {
            set_border_css_color(style, physical_side(property, style.text.direction), value)
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
            physical_side(property, style.text.direction),
            line_style_to_border_style(value),
        ),

        Property::BorderTopLeftRadius(value, _)
        | Property::BorderTopRightRadius(value, _)
        | Property::BorderBottomRightRadius(value, _)
        | Property::BorderBottomLeftRadius(value, _)
        | Property::BorderStartStartRadius(value)
        | Property::BorderStartEndRadius(value)
        | Property::BorderEndStartRadius(value)
        | Property::BorderEndEndRadius(value) => {
            let Some(value) = corner_radius(value, font_size, root_font_size, resolution) else {
                return ApplyResult::Invalid;
            };
            use TextDirection::{Ltr, Rtl};
            let corner = match (property, style.text.direction) {
                (Property::BorderTopLeftRadius(_, _), _)
                | (Property::BorderStartStartRadius(_), Ltr)
                | (Property::BorderStartEndRadius(_), Rtl) => 0,
                (Property::BorderTopRightRadius(_, _), _)
                | (Property::BorderStartEndRadius(_), Ltr)
                | (Property::BorderStartStartRadius(_), Rtl) => 1,
                (Property::BorderBottomRightRadius(_, _), _)
                | (Property::BorderEndEndRadius(_), Ltr)
                | (Property::BorderEndStartRadius(_), Rtl) => 2,
                (Property::BorderBottomLeftRadius(_, _), _)
                | (Property::BorderEndStartRadius(_), Ltr)
                | (Property::BorderEndEndRadius(_), Rtl) => 3,
                _ => unreachable!(),
            };
            set_corner_radius(style, corner, value);
        }

        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}

fn physical_side(property: &Property<'_>, direction: TextDirection) -> usize {
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
