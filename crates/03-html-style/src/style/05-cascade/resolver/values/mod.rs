//! Conversion from parser-owned CSS values to renderer-owned computed values.

use super::context::ResolutionContext;
use super::state::WorkingStyle;
use html_style_model::computed_style;
use html_style_model::{
    BorderStyle, ComputedStylesBuilder, ContentAlignment, CornerRadius, FlexDirection, FlexWrap,
    FontRelativeLength, InheritedText, ItemAlignment, LengthPct, LogicalTextAlign, OverflowMode,
    TextAlign, TextDecorationLines, TextDirection,
};

use lightningcss::properties::align::{ContentDistribution, ContentPosition};
use lightningcss::properties::overflow::OverflowKeyword;

pub(in crate::style::cascade::resolver) mod colors;
pub(in crate::style::cascade::resolver) mod fonts;
pub(in crate::style::cascade::resolver) mod grid;
pub(in crate::style::cascade::resolver) mod lengths;
pub(in crate::style::cascade::resolver) mod lists;

/// Rebind authored and inherited unitless line heights after a font-size change.
pub(super) fn rebind_line_height(
    style: &mut WorkingStyle,
    styles: &ComputedStylesBuilder,
    root_font_size: impl FnOnce() -> f32,
    resolution: &mut ResolutionContext,
) {
    let rebound = if let Some(spec) = style.line_height_spec.as_ref() {
        Some(spec.evaluate(style.font(styles).font_size, root_font_size(), resolution))
    } else {
        let text = style.text(styles);
        (text.line_height_number != 0.0).then(|| {
            (
                crate::style::syntax::prepared_length::finite(
                    f64::from(style.font(styles).font_size) * f64::from(text.line_height_number),
                ),
                0.0,
            )
        })
    };
    if let Some((px, ex)) = rebound {
        let current = style.text(styles);
        if current.line_height.to_bits() != px.to_bits()
            || current.line_height_x_height_px.to_bits() != ex.to_bits()
        {
            let text = style.text_mut(styles);
            text.line_height = px;
            text.line_height_x_height_px = ex;
        }
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
    if !text.text_align_last_explicit {
        if text.text_align_justify_all {
            text.text_align_last = TextAlign::Justify;
            text.text_align_last_logical = LogicalTextAlign::Physical;
        } else if text.text_align == TextAlign::Justify {
            text.text_align_last = text_start_alignment(text.direction);
            text.text_align_last_logical = LogicalTextAlign::Start;
        } else {
            text.text_align_last = text.text_align;
            text.text_align_last_logical = text.text_align_logical;
        }
    }
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

fn content_distribution(value: &ContentDistribution) -> ContentAlignment {
    match value {
        ContentDistribution::SpaceBetween => ContentAlignment::SpaceBetween,
        ContentDistribution::SpaceAround => ContentAlignment::SpaceAround,
        ContentDistribution::SpaceEvenly => ContentAlignment::SpaceEvenly,
        ContentDistribution::Stretch => ContentAlignment::Stretch,
    }
}

fn content_position(value: &ContentPosition) -> ContentAlignment {
    match value {
        ContentPosition::Center => ContentAlignment::Center,
        ContentPosition::Start => ContentAlignment::Start,
        ContentPosition::End => ContentAlignment::End,
        ContentPosition::FlexStart => ContentAlignment::FlexStart,
        ContentPosition::FlexEnd => ContentAlignment::FlexEnd,
    }
}

pub(super) fn content_alignment(
    value: &lightningcss::properties::align::AlignContent,
) -> Option<ContentAlignment> {
    use lightningcss::properties::align::{AlignContent, BaselinePosition};
    Some(match value {
        AlignContent::Normal => ContentAlignment::Normal,
        AlignContent::BaselinePosition(BaselinePosition::First | BaselinePosition::Last) => {
            return None;
        }
        AlignContent::ContentDistribution(value) => content_distribution(value),
        AlignContent::ContentPosition { value, .. } => content_position(value),
    })
}

pub(super) fn justify_content(
    value: &lightningcss::properties::align::JustifyContent,
) -> ContentAlignment {
    use lightningcss::properties::align::JustifyContent;
    match value {
        JustifyContent::Normal => ContentAlignment::Normal,
        JustifyContent::ContentDistribution(value) => content_distribution(value),
        JustifyContent::ContentPosition { value, .. } => content_position(value),
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
    computed_style::text_decoration_lines_new(
        line.contains(CssLine::Underline),
        line.contains(CssLine::Overline),
        line.contains(CssLine::LineThrough),
    )
}
