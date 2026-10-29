//! Conversion from parser-owned CSS values to renderer-owned computed values.

use super::context::ResolutionContext;
use super::state::WorkingStyle;
use colors::css_color_to_u32;
use html_style_model::{
    BorderStyle, ComputedStylesBuilder, ContentAlignment, CornerRadius, DecorationColor,
    FlexDirection, FlexWrap, Font, FontRelativeLength, InheritedText, ItemAlignment, LengthPct,
    LogicalTextAlign, OverflowMode, PreferredSize, StyleStringId, TextAlign, TextDecorationLines,
    TextDecorationStyle, TextDecorationThickness, TextDirection,
};
use lengths::{
    calc_box_length_percentage_components, computed_length_pct, length_percentage_to_preferred,
    length_percentage_to_px,
};
use lightningcss::printer::{Printer, PrinterOptions};
use lightningcss::properties::border::LineStyle;
use lightningcss::properties::font::{FontFamily, LineHeight};
use lightningcss::properties::overflow::OverflowKeyword;
use lightningcss::traits::ToCss;
use lightningcss::values::color::CssColor;
use lightningcss::values::length::{LengthPercentage, LengthPercentageOrAuto, LengthValue};
use lightningcss::values::size::Size2D;

pub(in crate::style::cascade::resolver) mod colors;
pub(in crate::style::cascade::resolver) mod fonts;
pub(in crate::style::cascade::resolver) mod grid;
pub(in crate::style::cascade::resolver) mod lengths;
pub(in crate::style::cascade::resolver) mod lists;

pub(super) fn checked_line_height_components(
    lh: &LineHeight,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<(f32, f32)> {
    let scale = resolution.font_size_scale;
    let (px, ex) = resolution.with_canonical_font_sizes(|resolution| {
        let font_size = font_size / scale;
        let root_font_size = root_font_size / scale;
        let mut components = match lh {
            LineHeight::Length(LengthPercentage::Dimension(LengthValue::Ex(value))) => {
                (0.0, value * font_size)
            }
            LineHeight::Length(LengthPercentage::Calc(calc)) => {
                let [
                    absolute_px,
                    percentage,
                    x_height_px,
                    ch_advance_px,
                    cap_height_px,
                ] = calc_box_length_percentage_components(
                    calc,
                    font_size,
                    root_font_size,
                    resolution,
                )?;
                if ch_advance_px != 0.0 || cap_height_px != 0.0 {
                    return None;
                }
                (absolute_px + percentage * font_size, x_height_px)
            }
            LineHeight::Normal => (0.0, 0.0),
            LineHeight::Number(value) => (font_size * value, 0.0),
            LineHeight::Length(value) => (
                length_percentage_to_px(value, font_size, root_font_size, resolution)?,
                0.0,
            ),
        };
        if components.1 == 0.0 {
            components.0 = components.0.max(0.0);
        }
        (components.0.is_finite()
            && components.0 >= 0.0
            && components.1.is_finite()
            && components.1 >= 0.0)
            .then_some(components)
    })?;
    let result = (px * scale, ex * scale);
    (result.0.is_finite() && result.1.is_finite()).then_some(result)
}

/// Rebind authored and inherited unitless line heights after a font-size change.
pub(super) fn rebind_line_height(
    style: &mut WorkingStyle,
    root_font_size: impl FnOnce() -> f32,
    resolution: &ResolutionContext,
) {
    if let Some(spec) = style.line_height_spec.as_ref() {
        if let Some((px, ex)) =
            checked_line_height_components(spec, style.font.font_size, root_font_size(), resolution)
        {
            style.text.line_height = px;
            style.text.line_height_x_height_px = ex;
        }
    } else if style.text.line_height_number != 0.0 {
        style.text.line_height = style.font.font_size * style.text.line_height_number;
        style.text.line_height_x_height_px = 0.0;
    }
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
    resolution: &ResolutionContext,
) -> Option<PreferredSize> {
    let value = match value {
        LengthPercentageOrAuto::Auto => Some(PreferredSize::Auto),
        LengthPercentageOrAuto::LengthPercentage(value) => {
            length_percentage_to_preferred(value, font, root_font_size, styles, resolution)
        }
    }?;
    non_negative_preferred_size(value)
}

pub(super) fn non_negative_preferred_size(mut value: PreferredSize) -> Option<PreferredSize> {
    let valid = match &mut value {
        PreferredSize::Auto
        | PreferredSize::Content
        | PreferredSize::MinContent
        | PreferredSize::MaxContent
        | PreferredSize::FitContent
        | PreferredSize::Stretch
        | PreferredSize::Comparison(_) => true,
        // Negative literals are rejected before cascade selection. The parser
        // can fold a negative calculation to a plain dimension, whose computed
        // value must be clamped rather than exposing a losing declaration.
        PreferredSize::Px(value)
        | PreferredSize::Percent(value)
        | PreferredSize::Ex(value)
        | PreferredSize::Ch(value)
        | PreferredSize::Cap(value) => {
            if !value.is_finite() {
                return None;
            }
            *value = value.max(0.0);
            true
        }
        PreferredSize::Calc {
            absolute_px,
            percentage,
            x_height_px,
            ch_advance_px,
            cap_height_px,
            ..
        } => {
            absolute_px.is_finite()
                && percentage.is_finite()
                && x_height_px.is_finite()
                && ch_advance_px.is_finite()
                && cap_height_px.is_finite()
        }
    };
    valid.then_some(value)
}

pub(super) fn non_negative_length_pct(value: LengthPct) -> Option<LengthPct> {
    let valid = match value {
        LengthPct::Px(value)
        | LengthPct::Pct(value)
        | LengthPct::Ex(value)
        | LengthPct::Ch(value)
        | LengthPct::Cap(value) => value >= 0.0,
        LengthPct::Calc {
            absolute_px,
            percentage,
            x_height_px,
            ch_advance_px,
            cap_height_px,
            ..
        } => {
            absolute_px.is_finite()
                && percentage.is_finite()
                && x_height_px.is_finite()
                && ch_advance_px.is_finite()
                && cap_height_px.is_finite()
        }
    };
    valid.then_some(value)
}

pub(super) fn corner_radius(
    value: &Size2D<LengthPercentage>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<CornerRadius> {
    Some(CornerRadius {
        x: computed_length_pct(&value.0, font_size, root_font_size, resolution)
            .and_then(non_negative_length_pct)?,
        y: computed_length_pct(&value.1, font_size, root_font_size, resolution)
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

pub(super) fn set_padding_side(style: &mut WorkingStyle, side: usize, value: LengthPct) {
    match side {
        0 => style.box_model.padding_top = value,
        1 => style.box_model.padding_right = value,
        2 => style.box_model.padding_bottom = value,
        3 => style.box_model.padding_left = value,
        _ => unreachable!("physical side index"),
    }
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

pub(super) fn gap_value(
    value: &lightningcss::properties::align::GapValue,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<LengthPct> {
    use lightningcss::properties::align::GapValue;
    let value = match value {
        GapValue::Normal => Some(LengthPct::Px(0.0)),
        GapValue::LengthPercentage(value) => {
            computed_length_pct(value, font_size, root_font_size, resolution)
        }
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
        JustifyContent::Left { .. } => ContentAlignment::Left,
        JustifyContent::Right { .. } => ContentAlignment::Right,
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

pub(super) fn default_justify_items(parent: ItemAlignment) -> ItemAlignment {
    match parent {
        value @ (ItemAlignment::LegacyLeft
        | ItemAlignment::LegacyRight
        | ItemAlignment::LegacyCenter) => value,
        _ => ItemAlignment::Normal,
    }
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
        JustifyItems::Legacy(LegacyJustify::Left) => ItemAlignment::LegacyLeft,
        JustifyItems::Legacy(LegacyJustify::Right) => ItemAlignment::LegacyRight,
        JustifyItems::Legacy(LegacyJustify::Center) => ItemAlignment::LegacyCenter,
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
    resolution: &ResolutionContext,
) -> Option<TextDecorationThickness> {
    use lightningcss::properties::text::TextDecorationThickness as CssThickness;
    match value {
        CssThickness::Auto => Some(TextDecorationThickness::Auto),
        CssThickness::FromFont => Some(TextDecorationThickness::FromFont),
        CssThickness::LengthPercentage(value) => TextDecorationThickness::length(
            computed_length_pct(value, font_size, root_font_size, resolution)?,
        ),
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
