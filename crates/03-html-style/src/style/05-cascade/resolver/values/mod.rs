//! Conversion from parser-owned CSS values to renderer-owned computed values.

use super::*;

mod colors;
mod font_features;
mod fonts;
mod grid;
mod lengths;
mod lists;

pub(super) use colors::*;
pub(crate) use font_features::parse_font_kerning;
pub(super) use font_features::{
    font_variant_caps_features, parse_font_feature_settings, parse_font_variant_ligatures,
    parse_font_variant_numeric,
};
pub(super) use fonts::*;
pub(super) use grid::*;
pub(super) use lengths::*;
pub(super) use lists::*;

pub(super) fn parse_line_height(
    lh: &lightningcss::properties::font::LineHeight,
    font_size: f32,
    root_font_size: f32,
) -> Option<f32> {
    use lightningcss::properties::font::LineHeight;
    match lh {
        LineHeight::Normal => Some(0.0),
        LineHeight::Number(n) => Some(font_size * n),
        LineHeight::Length(lp) => length_percentage_to_px(lp, font_size, root_font_size),
    }
}

pub(super) fn checked_line_height(
    lh: &lightningcss::properties::font::LineHeight,
    font_size: f32,
    root_font_size: f32,
) -> Option<f32> {
    checked_line_height_components(lh, font_size, root_font_size).map(|components| components.0)
}

pub(super) fn checked_line_height_components(
    lh: &lightningcss::properties::font::LineHeight,
    font_size: f32,
    root_font_size: f32,
) -> Option<(f32, f32)> {
    use lightningcss::properties::font::LineHeight;
    let components = match lh {
        LineHeight::Length(LengthPercentage::Dimension(LengthValue::Ex(value))) => {
            (0.0, value * font_size)
        }
        LineHeight::Length(LengthPercentage::Calc(calc)) => {
            let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) =
                calc_box_length_percentage_components(calc, font_size, root_font_size)?;
            if ch_advance_px != 0.0 || cap_height_px != 0.0 {
                return None;
            }
            (absolute_px + percentage * font_size, x_height_px)
        }
        _ => (parse_line_height(lh, font_size, root_font_size)?, 0.0),
    };
    (components.0.is_finite()
        && components.0 >= 0.0
        && components.1.is_finite()
        && components.1 >= 0.0)
        .then_some(components)
}

pub(super) fn overflow_mode(value: OverflowKeyword) -> OverflowMode {
    match value {
        OverflowKeyword::Visible => OverflowMode::Visible,
        OverflowKeyword::Hidden => OverflowMode::Hidden,
        OverflowKeyword::Clip => OverflowMode::Clip,
        OverflowKeyword::Scroll => OverflowMode::Scroll,
        OverflowKeyword::Auto => OverflowMode::Auto,
    }
}

pub(super) fn flex_direction(
    value: &lightningcss::properties::flex::FlexDirection,
) -> FlexDirection {
    use lightningcss::properties::flex::FlexDirection as Lc;
    match value {
        Lc::Row => FlexDirection::Row,
        Lc::RowReverse => FlexDirection::RowReverse,
        Lc::Column => FlexDirection::Column,
        Lc::ColumnReverse => FlexDirection::ColumnReverse,
    }
}

pub(super) fn flex_wrap(value: &lightningcss::properties::flex::FlexWrap) -> FlexWrap {
    use lightningcss::properties::flex::FlexWrap as Lc;
    match value {
        Lc::NoWrap => FlexWrap::NoWrap,
        Lc::Wrap => FlexWrap::Wrap,
        Lc::WrapReverse => FlexWrap::WrapReverse,
    }
}

pub(super) fn flex_basis(
    value: &LengthPercentageOrAuto,
    font: &Font,
    root_font_size: f32,
    styles: &mut ComputedStylesBuilder,
) -> Option<PreferredSize> {
    let value = match value {
        LengthPercentageOrAuto::Auto => Some(PreferredSize::Auto),
        LengthPercentageOrAuto::LengthPercentage(value) => {
            length_percentage_to_preferred(value, font, root_font_size, styles)
        }
    }?;
    non_negative_preferred_size(value)
}

pub(super) fn non_negative_preferred_size(value: PreferredSize) -> Option<PreferredSize> {
    match value {
        PreferredSize::Auto
        | PreferredSize::MinContent
        | PreferredSize::MaxContent
        | PreferredSize::FitContent
        | PreferredSize::Stretch => Some(value),
        PreferredSize::Px(value) if value >= 0.0 => Some(PreferredSize::Px(value)),
        PreferredSize::Percent(value) if value >= 0.0 => Some(PreferredSize::Percent(value)),
        PreferredSize::Ex(value) if value >= 0.0 => Some(PreferredSize::Ex(value)),
        PreferredSize::Ch(value) if value >= 0.0 => Some(PreferredSize::Ch(value)),
        PreferredSize::Cap(value) if value >= 0.0 => Some(PreferredSize::Cap(value)),
        PreferredSize::Calc {
            absolute_px,
            percentage,
            x_height_px,
            ch_advance_px,
            cap_height_px,
            percentage_dependent,
        } if absolute_px.is_finite()
            && percentage.is_finite()
            && x_height_px.is_finite()
            && ch_advance_px.is_finite()
            && cap_height_px.is_finite() =>
        {
            Some(PreferredSize::Calc {
                absolute_px,
                percentage,
                x_height_px,
                ch_advance_px,
                cap_height_px,
                percentage_dependent,
            })
        }
        PreferredSize::Comparison(_) => Some(value),
        PreferredSize::Px(_)
        | PreferredSize::Percent(_)
        | PreferredSize::Ex(_)
        | PreferredSize::Ch(_)
        | PreferredSize::Cap(_)
        | PreferredSize::Calc { .. } => None,
    }
}

pub(super) fn length_percentage_to_preferred(
    value: &LengthPercentage,
    font: &Font,
    root_font_size: f32,
    styles: &mut ComputedStylesBuilder,
) -> Option<PreferredSize> {
    match value {
        LengthPercentage::Dimension(value) => preferred_from_length(value, font, root_font_size),
        LengthPercentage::Percentage(value) => Some(PreferredSize::Percent(value.0)),
        LengthPercentage::Calc(value) => {
            comparison_preferred_size(value, font.font_size, root_font_size, styles).or_else(|| {
                let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) =
                    calc_box_length_percentage_components(value, font.font_size, root_font_size)?;
                Some(PreferredSize::Calc {
                    absolute_px,
                    percentage,
                    x_height_px,
                    ch_advance_px,
                    cap_height_px,
                    percentage_dependent: calc_depends_on_percentage(value),
                })
            })
        }
    }
}

pub(super) fn comparison_preferred_size(
    calc: &Calc<LengthPercentage>,
    font_size: f32,
    root_font_size: f32,
    styles: &mut ComputedStylesBuilder,
) -> Option<PreferredSize> {
    let Calc::Function(function) = calc else {
        return None;
    };
    match function.as_ref() {
        MathFunction::Min(values) => comparison_from_operands(
            SizeComparison::Min,
            values.iter(),
            values.len(),
            font_size,
            root_font_size,
            styles,
        ),
        MathFunction::Max(values) => comparison_from_operands(
            SizeComparison::Max,
            values.iter(),
            values.len(),
            font_size,
            root_font_size,
            styles,
        ),
        MathFunction::Clamp(min, value, max) => comparison_from_operands(
            SizeComparison::Clamp,
            [min, value, max],
            3,
            font_size,
            root_font_size,
            styles,
        ),
        MathFunction::Calc(value) => {
            return comparison_preferred_size(value, font_size, root_font_size, styles);
        }
        _ => None,
    }
}

pub(super) fn comparison_from_operands<'a>(
    kind: SizeComparison,
    operands: impl IntoIterator<Item = &'a Calc<LengthPercentage>>,
    count: usize,
    font_size: f32,
    root_font_size: f32,
    styles: &mut ComputedStylesBuilder,
) -> Option<PreferredSize> {
    if count == 0 || count > 3 {
        return None;
    }
    let mut values = [ComputedSizeComponent::default(); 3];
    for (slot, operand) in values.iter_mut().zip(operands) {
        let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) =
            calc_box_length_percentage_components(operand, font_size, root_font_size)?;
        *slot = ComputedSizeComponent {
            absolute_px,
            percentage,
            x_height_px,
            ch_advance_px,
            cap_height_px,
            percentage_dependent: calc_depends_on_percentage(operand),
        };
    }
    styles.intern_size_comparison(kind, values, u8::try_from(count).ok()?)
}

pub(super) fn computed_length_pct(
    value: &LengthPercentage,
    font_size: f32,
    root_font_size: f32,
) -> Option<LengthPct> {
    match value {
        LengthPercentage::Dimension(value) => {
            length_pct_from_length(value, font_size, root_font_size)
        }
        LengthPercentage::Percentage(value) => Some(LengthPct::Pct(value.0)),
        LengthPercentage::Calc(value) => {
            let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) =
                calc_box_length_percentage_components(value, font_size, root_font_size)?;
            Some(LengthPct::Calc {
                absolute_px,
                percentage,
                x_height_px,
                ch_advance_px,
                cap_height_px,
                percentage_dependent: calc_depends_on_percentage(value),
            })
        }
    }
}

pub(super) fn non_negative_length_pct(value: LengthPct) -> Option<LengthPct> {
    match value {
        LengthPct::Px(value) if value >= 0.0 => Some(LengthPct::Px(value)),
        LengthPct::Pct(value) if value >= 0.0 => Some(LengthPct::Pct(value)),
        LengthPct::Ex(value) if value >= 0.0 => Some(LengthPct::Ex(value)),
        LengthPct::Ch(value) if value >= 0.0 => Some(LengthPct::Ch(value)),
        LengthPct::Cap(value) if value >= 0.0 => Some(LengthPct::Cap(value)),
        LengthPct::Calc {
            absolute_px,
            percentage,
            x_height_px,
            ch_advance_px,
            cap_height_px,
            percentage_dependent,
        } if absolute_px.is_finite()
            && percentage.is_finite()
            && x_height_px.is_finite()
            && ch_advance_px.is_finite()
            && cap_height_px.is_finite() =>
        {
            Some(LengthPct::Calc {
                absolute_px,
                percentage,
                x_height_px,
                ch_advance_px,
                cap_height_px,
                percentage_dependent,
            })
        }
        LengthPct::Px(_)
        | LengthPct::Pct(_)
        | LengthPct::Ex(_)
        | LengthPct::Ch(_)
        | LengthPct::Cap(_)
        | LengthPct::Calc { .. } => None,
    }
}

pub(super) fn corner_radius(
    value: &Size2D<LengthPercentage>,
    font_size: f32,
    root_font_size: f32,
) -> Option<CornerRadius> {
    Some(CornerRadius {
        x: length_percentage_to_lengthpct(&value.0, font_size, root_font_size)
            .and_then(non_negative_length_pct)?,
        y: length_percentage_to_lengthpct(&value.1, font_size, root_font_size)
            .and_then(non_negative_length_pct)?,
    })
}

pub(super) fn set_corner_radius(style: &mut WorkingStyle, corner: usize, value: CornerRadius) {
    match corner {
        0 => style.radii.top_left = value,
        1 => style.radii.top_right = value,
        2 => style.radii.bottom_right = value,
        3 => style.radii.bottom_left = value,
        _ => unreachable!("physical corner index"),
    }
}

pub(super) fn copy_corner_radius(style: &mut WorkingStyle, source: &ParentStyle, corner: usize) {
    let value = match corner {
        0 => source.radii.top_left,
        1 => source.radii.top_right,
        2 => source.radii.bottom_right,
        3 => source.radii.bottom_left,
        _ => unreachable!("physical corner index"),
    };
    set_corner_radius(style, corner, value);
}

/// Physical side indices use CSS clockwise order: top, right, bottom, left.
pub(super) fn inline_sides(direction: TextDirection) -> (usize, usize) {
    match direction {
        TextDirection::Ltr => (3, 1),
        TextDirection::Rtl => (1, 3),
    }
}

pub(super) fn text_start_alignment(direction: TextDirection) -> TextAlign {
    match direction {
        TextDirection::Ltr => TextAlign::Left,
        TextDirection::Rtl => TextAlign::Right,
    }
}

pub(super) fn text_end_alignment(direction: TextDirection) -> TextAlign {
    match direction {
        TextDirection::Ltr => TextAlign::Right,
        TextDirection::Rtl => TextAlign::Left,
    }
}

pub(super) fn resolve_logical_text_alignments(text: &mut InheritedText) {
    text.text_align = match text.text_align_logical {
        LogicalTextAlign::Physical => text.text_align,
        LogicalTextAlign::Start => text_start_alignment(text.direction),
        LogicalTextAlign::End => text_end_alignment(text.direction),
    };
    text.text_align_last = match text.text_align_last_logical {
        LogicalTextAlign::Physical => text.text_align_last,
        LogicalTextAlign::Start => text_start_alignment(text.direction),
        LogicalTextAlign::End => text_end_alignment(text.direction),
    };
}

pub(super) fn logical_float(value: &str, direction: TextDirection, fallback: Float) -> Float {
    match value {
        "left" => Float::Left,
        "right" => Float::Right,
        "inline-start" => match direction {
            TextDirection::Ltr => Float::Left,
            TextDirection::Rtl => Float::Right,
        },
        "inline-end" => match direction {
            TextDirection::Ltr => Float::Right,
            TextDirection::Rtl => Float::Left,
        },
        "none" => Float::None,
        _ => fallback,
    }
}

pub(super) fn logical_clear(value: &str, direction: TextDirection, fallback: Clear) -> Clear {
    match value {
        "left" => Clear::Left,
        "right" => Clear::Right,
        "inline-start" => match direction {
            TextDirection::Ltr => Clear::Left,
            TextDirection::Rtl => Clear::Right,
        },
        "inline-end" => match direction {
            TextDirection::Ltr => Clear::Right,
            TextDirection::Rtl => Clear::Left,
        },
        "both" => Clear::Both,
        "none" => Clear::None,
        _ => fallback,
    }
}

pub(super) fn set_margin_side(style: &mut WorkingStyle, side: usize, value: (LengthPct, bool)) {
    match side {
        0 => (style.box_model.margin_top, style.layout.margin_top_auto) = value,
        1 => (style.box_model.margin_right, style.layout.margin_right_auto) = value,
        2 => {
            (
                style.box_model.margin_bottom,
                style.layout.margin_bottom_auto,
            ) = value
        }
        3 => (style.box_model.margin_left, style.layout.margin_left_auto) = value,
        _ => unreachable!("physical side index"),
    }
}

pub(super) fn copy_margin_side(style: &mut WorkingStyle, source: &ParentStyle, side: usize) {
    let value = match side {
        0 => (source.box_model.margin_top, source.layout.margin_top_auto),
        1 => (
            source.box_model.margin_right,
            source.layout.margin_right_auto,
        ),
        2 => (
            source.box_model.margin_bottom,
            source.layout.margin_bottom_auto,
        ),
        3 => (source.box_model.margin_left, source.layout.margin_left_auto),
        _ => unreachable!("physical side index"),
    };
    set_margin_side(style, side, value);
}

pub(super) fn set_padding_side(style: &mut WorkingStyle, side: usize, value: LengthPct) {
    match side {
        0 => style.box_model.padding_top = value,
        1 => style.box_model.padding_right = value,
        2 => style.box_model.padding_bottom = value,
        3 => style.box_model.padding_left = value,
        _ => unreachable!("physical side index"),
    }
}

pub(super) fn copy_padding_side(style: &mut WorkingStyle, source: &ParentStyle, side: usize) {
    let value = match side {
        0 => source.box_model.padding_top,
        1 => source.box_model.padding_right,
        2 => source.box_model.padding_bottom,
        3 => source.box_model.padding_left,
        _ => unreachable!("physical side index"),
    };
    set_padding_side(style, side, value);
}

pub(super) fn set_border_width(style: &mut WorkingStyle, side: usize, value: FontRelativeLength) {
    match side {
        0 => style.border.border_top_width = value,
        1 => style.border.border_right_width = value,
        2 => style.border.border_bottom_width = value,
        3 => style.border.border_left_width = value,
        _ => unreachable!("physical side index"),
    }
}

pub(super) fn set_border_style(style: &mut WorkingStyle, side: usize, value: BorderStyle) {
    match side {
        0 => style.border.border_top_style = value,
        1 => style.border.border_right_style = value,
        2 => style.border.border_bottom_style = value,
        3 => style.border.border_left_style = value,
        _ => unreachable!("physical side index"),
    }
}

pub(super) fn set_border_color(style: &mut WorkingStyle, side: usize, value: u32) {
    match side {
        0 => style.border.border_top_color = value,
        1 => style.border.border_right_color = value,
        2 => style.border.border_bottom_color = value,
        3 => style.border.border_left_color = value,
        _ => unreachable!("physical side index"),
    }
}

pub(super) fn set_border_css_color(style: &mut WorkingStyle, side: usize, value: &CssColor) {
    set_border_color(style, side, css_color_to_u32(value, style.text.color));
    let bit = 1 << side;
    if matches!(value, CssColor::CurrentColor) {
        style.border.current_color_sides |= bit;
    } else {
        style.border.current_color_sides &= !bit;
    }
}

pub(super) fn checked_border_width(
    value: &BorderSideWidth,
    font_size: f32,
    root_font_size: f32,
) -> Option<FontRelativeLength> {
    border_width(value, font_size, root_font_size)
}

pub(super) fn apply_logical_border<const P: u8>(
    style: &mut WorkingStyle,
    side: usize,
    value: &lightningcss::properties::border::GenericBorder<LineStyle, P>,
    root_font_size: f32,
) {
    let Some(width) = checked_border_width(&value.width, style.font.font_size, root_font_size)
    else {
        return;
    };
    set_border_width(style, side, width);
    set_border_style(style, side, line_style_to_border_style(&value.style));
    set_border_css_color(style, side, &value.color);
}

pub(super) fn gap_value(
    value: &lightningcss::properties::align::GapValue,
    font_size: f32,
    root_font_size: f32,
) -> Option<LengthPct> {
    use lightningcss::properties::align::GapValue;
    let value = match value {
        GapValue::Normal => Some(LengthPct::Px(0.0)),
        GapValue::LengthPercentage(value) => computed_length_pct(value, font_size, root_font_size),
    }?;

    // Lightning CSS currently represents negative gaps as typed values even
    // though CSS Box Alignment makes them invalid. Do not duplicate its CSS
    // parser here, but preserve the last valid cascaded value rather than
    // allowing an invalid typed value into the computed-style store.
    non_negative_length_pct(value)
}

pub(super) fn content_alignment(
    value: &lightningcss::properties::align::AlignContent,
) -> Option<ContentAlignment> {
    use lightningcss::properties::align::{
        AlignContent, BaselinePosition, ContentDistribution, ContentPosition,
    };
    Some(match value {
        AlignContent::Normal => ContentAlignment::Normal,
        AlignContent::BaselinePosition(BaselinePosition::First | BaselinePosition::Last) => {
            return None;
        }
        AlignContent::ContentDistribution(value) => match value {
            ContentDistribution::SpaceBetween => ContentAlignment::SpaceBetween,
            ContentDistribution::SpaceAround => ContentAlignment::SpaceAround,
            ContentDistribution::SpaceEvenly => ContentAlignment::SpaceEvenly,
            ContentDistribution::Stretch => ContentAlignment::Stretch,
        },
        AlignContent::ContentPosition { value, .. } => match value {
            ContentPosition::Center => ContentAlignment::Center,
            ContentPosition::Start => ContentAlignment::Start,
            ContentPosition::End => ContentAlignment::End,
            ContentPosition::FlexStart => ContentAlignment::FlexStart,
            ContentPosition::FlexEnd => ContentAlignment::FlexEnd,
        },
    })
}

pub(super) fn justify_content(
    value: &lightningcss::properties::align::JustifyContent,
) -> ContentAlignment {
    use lightningcss::properties::align::{ContentDistribution, ContentPosition, JustifyContent};
    match value {
        JustifyContent::Normal => ContentAlignment::Normal,
        JustifyContent::ContentDistribution(value) => match value {
            ContentDistribution::SpaceBetween => ContentAlignment::SpaceBetween,
            ContentDistribution::SpaceAround => ContentAlignment::SpaceAround,
            ContentDistribution::SpaceEvenly => ContentAlignment::SpaceEvenly,
            ContentDistribution::Stretch => ContentAlignment::Stretch,
        },
        JustifyContent::ContentPosition { value, .. } => match value {
            ContentPosition::Center => ContentAlignment::Center,
            ContentPosition::Start => ContentAlignment::Start,
            ContentPosition::End => ContentAlignment::End,
            ContentPosition::FlexStart => ContentAlignment::FlexStart,
            ContentPosition::FlexEnd => ContentAlignment::FlexEnd,
        },
        JustifyContent::Left { .. } => ContentAlignment::Start,
        JustifyContent::Right { .. } => ContentAlignment::End,
    }
}

pub(super) fn self_position(
    value: &lightningcss::properties::align::SelfPosition,
) -> ItemAlignment {
    use lightningcss::properties::align::SelfPosition;
    match value {
        SelfPosition::Center => ItemAlignment::Center,
        SelfPosition::Start => ItemAlignment::Start,
        SelfPosition::End => ItemAlignment::End,
        SelfPosition::SelfStart => ItemAlignment::SelfStart,
        SelfPosition::SelfEnd => ItemAlignment::SelfEnd,
        SelfPosition::FlexStart => ItemAlignment::FlexStart,
        SelfPosition::FlexEnd => ItemAlignment::FlexEnd,
    }
}

pub(super) fn align_items(
    value: &lightningcss::properties::align::AlignItems,
) -> Option<ItemAlignment> {
    use lightningcss::properties::align::{AlignItems, BaselinePosition};
    Some(match value {
        AlignItems::Normal => ItemAlignment::Normal,
        AlignItems::Stretch => ItemAlignment::Stretch,
        AlignItems::BaselinePosition(BaselinePosition::First) => ItemAlignment::Baseline,
        AlignItems::BaselinePosition(BaselinePosition::Last) => return None,
        AlignItems::SelfPosition { value, .. } => self_position(value),
    })
}

pub(super) fn align_self(
    value: &lightningcss::properties::align::AlignSelf,
) -> Option<ItemAlignment> {
    use lightningcss::properties::align::{AlignSelf, BaselinePosition};
    Some(match value {
        AlignSelf::Auto => ItemAlignment::Auto,
        AlignSelf::Normal => ItemAlignment::Normal,
        AlignSelf::Stretch => ItemAlignment::Stretch,
        AlignSelf::BaselinePosition(BaselinePosition::First) => ItemAlignment::Baseline,
        AlignSelf::BaselinePosition(BaselinePosition::Last) => return None,
        AlignSelf::SelfPosition { value, .. } => self_position(value),
    })
}

pub(super) fn justify_items(
    value: &lightningcss::properties::align::JustifyItems,
) -> Option<ItemAlignment> {
    use lightningcss::properties::align::{BaselinePosition, JustifyItems, LegacyJustify};
    Some(match value {
        JustifyItems::Normal => ItemAlignment::Normal,
        JustifyItems::Stretch => ItemAlignment::Stretch,
        JustifyItems::BaselinePosition(BaselinePosition::First) => ItemAlignment::Baseline,
        JustifyItems::BaselinePosition(BaselinePosition::Last) => return None,
        JustifyItems::SelfPosition { value, .. } => self_position(value),
        JustifyItems::Left { .. } => ItemAlignment::Left,
        JustifyItems::Right { .. } => ItemAlignment::Right,
        JustifyItems::Legacy(LegacyJustify::Left) => ItemAlignment::Left,
        JustifyItems::Legacy(LegacyJustify::Right) => ItemAlignment::Right,
        JustifyItems::Legacy(LegacyJustify::Center) => ItemAlignment::Center,
    })
}

pub(super) fn justify_self(
    value: &lightningcss::properties::align::JustifySelf,
) -> Option<ItemAlignment> {
    use lightningcss::properties::align::{BaselinePosition, JustifySelf};
    Some(match value {
        JustifySelf::Auto => ItemAlignment::Auto,
        JustifySelf::Normal => ItemAlignment::Normal,
        JustifySelf::Stretch => ItemAlignment::Stretch,
        JustifySelf::BaselinePosition(BaselinePosition::First) => ItemAlignment::Baseline,
        JustifySelf::BaselinePosition(BaselinePosition::Last) => return None,
        JustifySelf::SelfPosition { value, .. } => self_position(value),
        JustifySelf::Left { .. } => ItemAlignment::Left,
        JustifySelf::Right { .. } => ItemAlignment::Right,
    })
}

pub(super) fn line_height_number(lh: &lightningcss::properties::font::LineHeight) -> f32 {
    use lightningcss::properties::font::LineHeight;
    match lh {
        LineHeight::Number(n) => *n,
        _ => 0.0,
    }
}

pub(super) fn line_height_is_normal(lh: &lightningcss::properties::font::LineHeight) -> bool {
    matches!(lh, lightningcss::properties::font::LineHeight::Normal)
}

pub(super) fn text_decoration_lines(
    line: &lightningcss::properties::text::TextDecorationLine,
) -> TextDecorationLines {
    use lightningcss::properties::text::TextDecorationLine as CssLine;
    TextDecorationLines::new(
        line.contains(CssLine::Underline),
        line.contains(CssLine::Overline),
        line.contains(CssLine::LineThrough),
    )
}

pub(super) fn text_decoration_style(
    style: &lightningcss::properties::text::TextDecorationStyle,
) -> Option<TextDecorationStyle> {
    use lightningcss::properties::text::TextDecorationStyle as CssStyle;
    match style {
        CssStyle::Solid => Some(TextDecorationStyle::Solid),
        CssStyle::Double => Some(TextDecorationStyle::Double),
        CssStyle::Dotted => Some(TextDecorationStyle::Dotted),
        CssStyle::Dashed => Some(TextDecorationStyle::Dashed),
        CssStyle::Wavy => None,
    }
}

pub(super) fn text_decoration_thickness(
    value: &lightningcss::properties::text::TextDecorationThickness,
    font_size: f32,
    root_font_size: f32,
) -> Option<TextDecorationThickness> {
    use lightningcss::properties::text::TextDecorationThickness as CssThickness;
    match value {
        CssThickness::Auto => Some(TextDecorationThickness::Auto),
        CssThickness::FromFont => Some(TextDecorationThickness::FromFont),
        CssThickness::LengthPercentage(value) => {
            TextDecorationThickness::length(computed_length_pct(value, font_size, root_font_size)?)
        }
    }
}

pub(super) fn decoration_color(color: &CssColor, current_color: u32) -> DecorationColor {
    if matches!(color, CssColor::CurrentColor) {
        DecorationColor::CurrentColor
    } else {
        DecorationColor::Rgba(css_color_to_u32(color, current_color))
    }
}

pub(super) fn outline_style(
    style: &lightningcss::properties::outline::OutlineStyle,
) -> Option<BorderStyle> {
    use lightningcss::properties::outline::OutlineStyle;
    match style {
        OutlineStyle::Auto => Some(BorderStyle::Solid),
        OutlineStyle::LineStyle(LineStyle::None | LineStyle::Hidden) => Some(BorderStyle::None),
        OutlineStyle::LineStyle(LineStyle::Solid) => Some(BorderStyle::Solid),
        OutlineStyle::LineStyle(LineStyle::Dashed) => Some(BorderStyle::Dashed),
        OutlineStyle::LineStyle(LineStyle::Dotted) => Some(BorderStyle::Dotted),
        OutlineStyle::LineStyle(
            LineStyle::Double
            | LineStyle::Groove
            | LineStyle::Ridge
            | LineStyle::Inset
            | LineStyle::Outset,
        ) => None,
    }
}

// ============================================================================
// Conversion helpers
// ============================================================================
pub(super) fn intern_font_family(
    styles: &mut ComputedStylesBuilder,
    families: &[FontFamily],
) -> StyleStringId {
    let mut out = String::new();
    for (i, family) in families.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = family.to_css(&mut Printer::new(&mut out, PrinterOptions::default()));
    }
    styles.intern_string(&out)
}
