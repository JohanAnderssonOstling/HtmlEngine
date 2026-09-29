use super::*;

#[allow(unused_variables)]
pub(super) fn apply(context: &mut PropertyContext<'_, '_>, property: &Property<'_>) -> ApplyResult {
    let doc = context.doc;
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    let resolved_root_font_size = context.resolved_root_font_size;
    let parent_font_size = context.parent_font_size;
    let parent_font_weight = context.parent_font_weight;
    let parent_color = context.parent_color;
    match property {
        // Text properties
        Property::LineHeight(lh) => {
            let Some((line_height, x_height_px)) = checked_line_height_components(
                lh,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.line_height_spec = Some(lh.clone());
            style.text.line_height_number = line_height_number(lh);
            style.text.line_height = line_height;
            style.text.line_height_x_height_px = x_height_px;
            style.text.line_height_normal = line_height_is_normal(lh);
        }
        Property::LetterSpacing(spacing) => {
            let Some(value) = spacing_to_text_spacing(
                spacing,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.text.letter_spacing = value;
        }
        Property::WordSpacing(spacing) => {
            let Some(value) = spacing_to_text_spacing(
                spacing,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.text.word_spacing = value;
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
            let Some(value) = length_percentage_to_lengthpct(
                &ti.value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
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
        Property::ListStyle(ls) => {
            let Some(list_style_type) = map_list_style_type(&ls.list_style_type) else {
                return ApplyResult::Invalid;
            };
            style.text.list_style_type = list_style_type;
            style.text.list_style_position = map_list_style_position(&ls.position);
            style.text.list_style_image = list_style_image_to_interned(styles, &ls.image);
        }
        Property::TextDecorationLine(line, _) => {
            style.background.text_decoration.lines = text_decoration_lines(line);
        }
        Property::TextDecoration(td, _) => {
            let Some(decoration_style) = text_decoration_style(&td.style) else {
                return ApplyResult::Invalid;
            };
            let Some(thickness) = text_decoration_thickness(
                &td.thickness,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.background.text_decoration.lines = text_decoration_lines(&td.line);
            style.background.text_decoration.style = decoration_style;
            style.background.text_decoration.color = decoration_color(&td.color, style.text.color);
            style.background.text_decoration.thickness = thickness;
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
                doc.root_font_size(),
                doc.resolution,
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
        Property::Outline(outline) => {
            let Some(outline_style) = outline_style(&outline.style) else {
                return ApplyResult::Invalid;
            };
            let Some(width) = border_width(
                &outline.width,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.background.outline.set_width(width);
            style.background.outline.style = outline_style;
            style.background.outline.color = decoration_color(&outline.color, style.text.color);
        }
        Property::OutlineWidth(width) => {
            let Some(width) = border_width(
                width,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
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
                    match vertical_align_value(
                        lp,
                        style.font.font_size,
                        doc.root_font_size(),
                        doc.resolution,
                    ) {
                        Some(value) => value,
                        None => return ApplyResult::Invalid,
                    }
                }
            };
            style.box_model.vertical_align = vertical_align;
        }
        Property::Overflow(overflow) => {
            style.box_model.overflow_x = overflow_mode(overflow.x);
            style.box_model.overflow_y = overflow_mode(overflow.y);
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
