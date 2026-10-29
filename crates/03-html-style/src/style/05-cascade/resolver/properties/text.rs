use super::super::context::{ApplyResult, PropertyContext};
use super::super::values::fonts::font_variant_caps_features;
use super::super::values::lengths::{
    border_width, computed_length_pct, length_to_px_from_length, spacing_to_text_spacing,
    vertical_align_value,
};
use super::super::values::lists::{
    list_style_image_to_interned, map_list_style_position, map_list_style_type,
};
use super::super::values::{
    checked_line_height_components, decoration_color, outline_style, overflow_mode,
    text_decoration_lines, text_decoration_style, text_decoration_thickness, text_end_alignment,
    text_start_alignment,
};
use html_style_model::{
    LogicalTextAlign, TabSize, TextAlign, TextOverflow, TextTransform, VerticalAlignValue,
    Visibility, WhiteSpace,
};
use lightningcss::properties::Property;
use lightningcss::properties::font::{LineHeight, VerticalAlign, VerticalAlignKeyword};
use lightningcss::properties::overflow::TextOverflow as CssTextOverflow;

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let root_font_size = context.root_font_size;
    let resolution = context.resolution;
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    match property {
        Property::LineBreak(value) => {
            use html_style_model::LineBreak;
            use lightningcss::properties::text::LineBreak as CssLineBreak;
            style.text.line_break = match value {
                CssLineBreak::Auto => LineBreak::Auto,
                CssLineBreak::Loose => LineBreak::Loose,
                CssLineBreak::Normal => LineBreak::Normal,
                CssLineBreak::Strict => LineBreak::Strict,
                CssLineBreak::Anywhere => LineBreak::Anywhere,
            };
        }
        Property::UnicodeBidi(value) => {
            use html_style_model::UnicodeBidi;
            use lightningcss::properties::text::UnicodeBidi as CssUnicodeBidi;
            style.box_model.unicode_bidi = match value {
                CssUnicodeBidi::Normal => UnicodeBidi::Normal,
                CssUnicodeBidi::Embed => UnicodeBidi::Embed,
                CssUnicodeBidi::Isolate => UnicodeBidi::Isolate,
                CssUnicodeBidi::BidiOverride => UnicodeBidi::BidiOverride,
                CssUnicodeBidi::IsolateOverride => UnicodeBidi::IsolateOverride,
                CssUnicodeBidi::Plaintext => UnicodeBidi::Plaintext,
            };
        }
        // Text properties
        Property::LineHeight(lh) => {
            let Some((line_height, x_height_px)) = checked_line_height_components(
                lh,
                style.font.font_size,
                root_font_size,
                resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.line_height_spec = Some(lh.clone());
            style.text.line_height_number = match lh {
                LineHeight::Number(value) => value.max(0.0),
                _ => 0.0,
            };
            style.text.line_height = line_height;
            style.text.line_height_x_height_px = x_height_px;
            style.text.line_height_normal = matches!(lh, LineHeight::Normal);
        }
        Property::TabSize(value, _) => {
            use lightningcss::values::length::LengthOrNumber;
            let value = match value {
                LengthOrNumber::Number(value) => TabSize::spaces(value.max(0.0)),
                LengthOrNumber::Length(value) => length_to_px_from_length(
                    value,
                    style.font.font_size,
                    root_font_size,
                    resolution,
                )
                .and_then(|value| TabSize::length_px(value.max(0.0))),
            };
            let Some(value) = value else {
                return ApplyResult::Invalid;
            };
            style.text.tab_size = value;
        }
        Property::LetterSpacing(spacing) | Property::WordSpacing(spacing) => {
            let Some(value) =
                spacing_to_text_spacing(spacing, style.font.font_size, root_font_size, resolution)
            else {
                return ApplyResult::Invalid;
            };
            *match property {
                Property::LetterSpacing(_) => &mut style.text.letter_spacing,
                _ => &mut style.text.word_spacing,
            } = value;
        }
        Property::TextAlign(ta) => {
            use lightningcss::properties::text::TextAlign as LcTextAlign;
            style.text.text_align_logical = match ta {
                LcTextAlign::Start => LogicalTextAlign::Start,
                LcTextAlign::End => LogicalTextAlign::End,
                LcTextAlign::MatchParent => style.text.text_align_logical,
                _ => LogicalTextAlign::Physical,
            };
            style.text.text_align = match ta {
                LcTextAlign::Left => TextAlign::Left,
                LcTextAlign::Right => TextAlign::Right,
                LcTextAlign::Center => TextAlign::Center,
                LcTextAlign::Justify => TextAlign::Justify,
                LcTextAlign::Start => text_start_alignment(style.text.direction),
                LcTextAlign::End => text_end_alignment(style.text.direction),
                LcTextAlign::JustifyAll => TextAlign::Justify,
                // For horizontal LTR, `match-parent` computes to the parent's
                // value — which is what this element inherited.
                LcTextAlign::MatchParent => style.text.text_align,
            };
            // `text-align-last: auto` (the default) follows `text-align`.
            if !style.text.text_align_last_explicit {
                style.text.text_align_last = style.text.text_align;
                style.text.text_align_last_logical = style.text.text_align_logical;
            }
        }
        Property::TextAlignLast(ta, _) => {
            use lightningcss::properties::text::TextAlignLast as LcTextAlignLast;
            let explicit = match ta {
                LcTextAlignLast::Auto => None,
                LcTextAlignLast::Left => Some(TextAlign::Left),
                LcTextAlignLast::Right => Some(TextAlign::Right),
                LcTextAlignLast::Center => Some(TextAlign::Center),
                LcTextAlignLast::Justify => Some(TextAlign::Justify),
                LcTextAlignLast::Start => Some(text_start_alignment(style.text.direction)),
                LcTextAlignLast::End => Some(text_end_alignment(style.text.direction)),
                _ => None,
            };
            style.text.text_align_last_explicit = explicit.is_some();
            style.text.text_align_last = explicit.unwrap_or(style.text.text_align);
            style.text.text_align_last_logical = match ta {
                LcTextAlignLast::Start => LogicalTextAlign::Start,
                LcTextAlignLast::End => LogicalTextAlign::End,
                LcTextAlignLast::Auto => style.text.text_align_logical,
                _ => LogicalTextAlign::Physical,
            };
        }
        Property::TextIndent(ti) => {
            let Some(value) =
                computed_length_pct(&ti.value, style.font.font_size, root_font_size, resolution)
            else {
                return ApplyResult::Invalid;
            };
            style.text.text_indent = value;
            style.text.text_indent_hanging = ti.hanging;
            style.text.text_indent_each_line = ti.each_line;
        }
        Property::TextTransform(tt) => {
            use lightningcss::properties::text::TextTransformCase;
            if !tt.other.is_empty() {
                return ApplyResult::Invalid;
            }
            style.text.text_transform = match tt.case {
                TextTransformCase::Uppercase => TextTransform::Uppercase,
                TextTransformCase::Lowercase => TextTransform::Lowercase,
                TextTransformCase::Capitalize => TextTransform::Capitalize,
                _ => TextTransform::None,
            };
        }
        Property::FontVariantCaps(caps) => {
            style.font.font_variant_caps_features = font_variant_caps_features(caps);
        }
        Property::ListStyleType(lst) => {
            let Some(value) = map_list_style_type(lst) else {
                return ApplyResult::Invalid;
            };
            style.text.list_style_type = value;
        }
        Property::ListStylePosition(pos) => {
            style.text.list_style_position = map_list_style_position(pos);
        }
        Property::ListStyleImage(image) => {
            style.text.list_style_image = list_style_image_to_interned(styles, image);
        }

        Property::TextDecorationLine(line, _) => {
            style.background.text_decoration.lines = text_decoration_lines(line);
        }

        Property::TextDecorationColor(color, _) => {
            style.background.text_decoration.color = decoration_color(color, style.text.color);
        }
        Property::TextDecorationStyle(decoration_style, _) => {
            let Some(decoration_style) = text_decoration_style(decoration_style) else {
                return ApplyResult::Invalid;
            };
            style.background.text_decoration.style = decoration_style;
        }
        Property::TextDecorationThickness(thickness) => {
            let Some(thickness) = text_decoration_thickness(
                thickness,
                style.font.font_size,
                root_font_size,
                resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.background.text_decoration.thickness = thickness;
        }
        Property::Visibility(value) => {
            use lightningcss::properties::display::Visibility as CssVisibility;
            style.text.visibility = match value {
                CssVisibility::Visible => Visibility::Visible,
                CssVisibility::Hidden => Visibility::Hidden,
                CssVisibility::Collapse => return ApplyResult::Invalid,
            };
        }

        Property::OutlineWidth(width) => {
            let Some(width) = border_width(width, style.font.font_size, root_font_size, resolution)
            else {
                return ApplyResult::Invalid;
            };
            style.background.outline.set_width(width);
        }
        Property::OutlineStyle(value) => {
            let Some(value) = outline_style(value) else {
                return ApplyResult::Invalid;
            };
            style.background.outline.style = value;
        }
        Property::OutlineColor(color) => {
            style.background.outline.color = decoration_color(color, style.text.color);
        }
        Property::VerticalAlign(va) => {
            let vertical_align = match va {
                VerticalAlign::Keyword(keyword) => match keyword {
                    VerticalAlignKeyword::Baseline => VerticalAlignValue::Baseline,
                    VerticalAlignKeyword::Sub => VerticalAlignValue::Sub,
                    VerticalAlignKeyword::Super => VerticalAlignValue::Super,
                    VerticalAlignKeyword::Top => VerticalAlignValue::Top,
                    VerticalAlignKeyword::TextTop => VerticalAlignValue::TextTop,
                    VerticalAlignKeyword::Middle => VerticalAlignValue::Middle,
                    VerticalAlignKeyword::Bottom => VerticalAlignValue::Bottom,
                    VerticalAlignKeyword::TextBottom => VerticalAlignValue::TextBottom,
                },
                VerticalAlign::Length(lp) => {
                    match vertical_align_value(lp, style.font.font_size, root_font_size, resolution)
                    {
                        Some(value) => value,
                        None => return ApplyResult::Invalid,
                    }
                }
            };
            style.box_model.vertical_align = vertical_align;
        }

        Property::OverflowX(overflow) => {
            style.box_model.overflow_x = overflow_mode(*overflow);
        }
        Property::OverflowY(overflow) => {
            style.box_model.overflow_y = overflow_mode(*overflow);
        }
        Property::TextOverflow(value, _) => {
            style.box_model.text_overflow = match value {
                CssTextOverflow::Clip => TextOverflow::Clip,
                CssTextOverflow::Ellipsis => TextOverflow::Ellipsis,
            };
        }
        Property::WhiteSpace(ws) => {
            use lightningcss::properties::text::WhiteSpace as LcWhiteSpace;
            style.text.white_space = match ws {
                LcWhiteSpace::Normal => WhiteSpace::Normal,
                LcWhiteSpace::Pre => WhiteSpace::Pre,
                LcWhiteSpace::NoWrap => WhiteSpace::NoWrap,
                LcWhiteSpace::PreWrap => WhiteSpace::PreWrap,
                LcWhiteSpace::PreLine => WhiteSpace::PreLine,
                LcWhiteSpace::BreakSpaces => WhiteSpace::BreakSpaces,
            };
        }
        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
