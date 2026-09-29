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
        // Size
        Property::Width(size) => {
            let Some(value) = size_to_preferred(
                size,
                &style.font,
                resolved_root_font_size,
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.width = value;
        }
        Property::Height(size) => {
            let Some(value) = size_to_preferred(
                size,
                &style.font,
                resolved_root_font_size,
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.height = value;
        }
        Property::AspectRatio(value) => {
            let components = value.ratio.as_ref().map(|ratio| (ratio.0, ratio.1));
            let Some(value) = ComputedAspectRatio::new(value.auto, components) else {
                return ApplyResult::Invalid;
            };
            style.box_model.aspect_ratio = value;
        }
        Property::BorderSpacing(spacing) => {
            let Some(horizontal) = length_to_px_from_length(
                &spacing.0,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            let Some(vertical) = length_to_px_from_length(
                &spacing.1,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
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
        Property::MinWidth(size) => {
            let Some(value) = size_to_preferred(
                size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.min_width = value;
        }
        Property::MinHeight(size) => {
            let Some(value) = size_to_preferred(
                size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.min_height = value;
        }
        Property::MaxWidth(max_size) => {
            let Some(value) = max_size_to_preferred(
                max_size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.max_width = value;
        }
        Property::MaxHeight(max_size) => {
            let Some(value) = max_size_to_preferred(
                max_size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.max_height = value;
        }
        Property::InlineSize(size) => {
            let Some(value) = size_to_preferred(
                size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.width = value;
        }
        Property::BlockSize(size) => {
            let Some(value) = size_to_preferred(
                size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.height = value;
        }
        Property::MinInlineSize(size) => {
            let Some(value) = size_to_preferred(
                size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.min_width = value;
        }
        Property::MinBlockSize(size) => {
            let Some(value) = size_to_preferred(
                size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.min_height = value;
        }
        Property::MaxInlineSize(size) => {
            let Some(value) = max_size_to_preferred(
                size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.max_width = value;
        }
        Property::MaxBlockSize(size) => {
            let Some(value) = max_size_to_preferred(
                size,
                &style.font,
                doc.root_font_size(),
                styles,
                doc.resolution,
            )
            .and_then(non_negative_preferred_size) else {
                return ApplyResult::Invalid;
            };
            style.box_model.max_height = value;
        }

        // Absolute boxes cross the typed style boundary and are removed from
        // normal flow by layout. Fixed and sticky still remain unsupported.
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
        Property::Top(value) => {
            let Some(value) = inset_value(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.inset_top = value;
        }
        Property::Right(value) => {
            let Some(value) = inset_value(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.inset_right = value;
        }
        Property::Bottom(value) => {
            let Some(value) = inset_value(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.inset_bottom = value;
        }
        Property::Left(value) => {
            let Some(value) = inset_value(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.inset_left = value;
        }
        Property::Inset(value) => {
            let (Some(top), Some(right), Some(bottom), Some(left)) = (
                inset_value(
                    &value.top,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                inset_value(
                    &value.right,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                inset_value(
                    &value.bottom,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                inset_value(
                    &value.left,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.inset_top = top;
            style.layout.inset_right = right;
            style.layout.inset_bottom = bottom;
            style.layout.inset_left = left;
        }

        // Margin
        Property::MarginTop(m) => {
            let Some(value) = margin_value(
                m,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            (style.box_model.margin_top, style.layout.margin_top_auto) = value;
        }
        Property::MarginBottom(m) => {
            let Some(value) = margin_value(
                m,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            (
                style.box_model.margin_bottom,
                style.layout.margin_bottom_auto,
            ) = value;
        }
        Property::MarginLeft(m) => {
            let Some(value) = margin_value(
                m,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            (style.box_model.margin_left, style.layout.margin_left_auto) = value;
        }
        Property::MarginRight(m) => {
            let Some(value) = margin_value(
                m,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            (style.box_model.margin_right, style.layout.margin_right_auto) = value;
        }
        Property::Margin(m) => {
            let (Some(top), Some(bottom), Some(left), Some(right)) = (
                margin_value(
                    &m.top,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                margin_value(
                    &m.bottom,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                margin_value(
                    &m.left,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                margin_value(
                    &m.right,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
            ) else {
                return ApplyResult::Invalid;
            };
            (style.box_model.margin_top, style.layout.margin_top_auto) = top;
            (
                style.box_model.margin_bottom,
                style.layout.margin_bottom_auto,
            ) = bottom;
            (style.box_model.margin_left, style.layout.margin_left_auto) = left;
            (style.box_model.margin_right, style.layout.margin_right_auto) = right;
        }
        Property::MarginBlock(m) => {
            let (Some(start), Some(end)) = (
                margin_value(
                    &m.block_start,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                margin_value(
                    &m.block_end,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
            ) else {
                return ApplyResult::Invalid;
            };
            set_margin_side(style, 0, start);
            set_margin_side(style, 2, end);
        }
        Property::MarginBlockStart(m) => {
            let Some(value) = margin_value(
                m,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_margin_side(style, 0, value);
        }
        Property::MarginBlockEnd(m) => {
            let Some(value) = margin_value(
                m,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_margin_side(style, 2, value);
        }
        Property::MarginInline(m) => {
            let (start, end) = inline_sides(style.text.direction);
            let (Some(start_value), Some(end_value)) = (
                margin_value(
                    &m.inline_start,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                margin_value(
                    &m.inline_end,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
            ) else {
                return ApplyResult::Invalid;
            };
            set_margin_side(style, start, start_value);
            set_margin_side(style, end, end_value);
        }
        Property::MarginInlineStart(m) => {
            let (start, _) = inline_sides(style.text.direction);
            let Some(value) = margin_value(
                m,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_margin_side(style, start, value);
        }
        Property::MarginInlineEnd(m) => {
            let (_, end) = inline_sides(style.text.direction);
            let Some(value) = margin_value(
                m,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_margin_side(style, end, value);
        }

        // Padding - uses LengthPercentageOrAuto in lightningcss
        Property::PaddingTop(p) => {
            let Some(value) = length_or_auto_to_lengthpct(
                p,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .and_then(non_negative_length_pct) else {
                return ApplyResult::Invalid;
            };
            style.box_model.padding_top = value;
        }
        Property::PaddingBottom(p) => {
            let Some(value) = length_or_auto_to_lengthpct(
                p,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .and_then(non_negative_length_pct) else {
                return ApplyResult::Invalid;
            };
            style.box_model.padding_bottom = value;
        }
        Property::PaddingLeft(p) => {
            let Some(value) = length_or_auto_to_lengthpct(
                p,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .and_then(non_negative_length_pct) else {
                return ApplyResult::Invalid;
            };
            style.box_model.padding_left = value;
        }
        Property::PaddingRight(p) => {
            let Some(value) = length_or_auto_to_lengthpct(
                p,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .and_then(non_negative_length_pct) else {
                return ApplyResult::Invalid;
            };
            style.box_model.padding_right = value;
        }
        Property::Padding(p) => {
            let (Some(top), Some(right), Some(bottom), Some(left)) = (
                length_or_auto_to_lengthpct(
                    &p.top,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                )
                .and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(
                    &p.right,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                )
                .and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(
                    &p.bottom,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                )
                .and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(
                    &p.left,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                )
                .and_then(non_negative_length_pct),
            ) else {
                return ApplyResult::Invalid;
            };
            style.box_model.padding_top = top;
            style.box_model.padding_right = right;
            style.box_model.padding_bottom = bottom;
            style.box_model.padding_left = left;
        }
        Property::PaddingBlock(p) => {
            let (Some(start), Some(end)) = (
                length_or_auto_to_lengthpct(
                    &p.block_start,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                )
                .and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(
                    &p.block_end,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                )
                .and_then(non_negative_length_pct),
            ) else {
                return ApplyResult::Invalid;
            };
            set_padding_side(style, 0, start);
            set_padding_side(style, 2, end);
        }
        Property::PaddingBlockStart(p) => {
            let Some(value) = length_or_auto_to_lengthpct(
                p,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .and_then(non_negative_length_pct) else {
                return ApplyResult::Invalid;
            };
            set_padding_side(style, 0, value);
        }
        Property::PaddingBlockEnd(p) => {
            let Some(value) = length_or_auto_to_lengthpct(
                p,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .and_then(non_negative_length_pct) else {
                return ApplyResult::Invalid;
            };
            set_padding_side(style, 2, value);
        }
        Property::PaddingInline(p) => {
            let (Some(start_value), Some(end_value)) = (
                length_or_auto_to_lengthpct(
                    &p.inline_start,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                )
                .and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(
                    &p.inline_end,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                )
                .and_then(non_negative_length_pct),
            ) else {
                return ApplyResult::Invalid;
            };
            let (start, end) = inline_sides(style.text.direction);
            set_padding_side(style, start, start_value);
            set_padding_side(style, end, end_value);
        }
        Property::PaddingInlineStart(p) => {
            let Some(value) = length_or_auto_to_lengthpct(
                p,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .and_then(non_negative_length_pct) else {
                return ApplyResult::Invalid;
            };
            let (start, _) = inline_sides(style.text.direction);
            set_padding_side(style, start, value);
        }
        Property::PaddingInlineEnd(p) => {
            let Some(value) = length_or_auto_to_lengthpct(
                p,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .and_then(non_negative_length_pct) else {
                return ApplyResult::Invalid;
            };
            let (_, end) = inline_sides(style.text.direction);
            set_padding_side(style, end, value);
        }

        // Border (per-side)
        Property::BorderWidth(bw) => {
            let (Some(top), Some(right), Some(bottom), Some(left)) = (
                checked_border_width(
                    &bw.top,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                checked_border_width(
                    &bw.right,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                checked_border_width(
                    &bw.bottom,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                checked_border_width(
                    &bw.left,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
            ) else {
                return ApplyResult::Invalid;
            };
            style.border.border_top_width = top;
            style.border.border_right_width = right;
            style.border.border_bottom_width = bottom;
            style.border.border_left_width = left;
        }
        Property::BorderTopWidth(w) => {
            let Some(width) = checked_border_width(
                w,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.border.border_top_width = width;
        }
        Property::BorderRightWidth(w) => {
            let Some(width) = checked_border_width(
                w,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.border.border_right_width = width;
        }
        Property::BorderBottomWidth(w) => {
            let Some(width) = checked_border_width(
                w,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.border.border_bottom_width = width;
        }
        Property::BorderLeftWidth(w) => {
            let Some(width) = checked_border_width(
                w,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.border.border_left_width = width;
        }
        Property::BorderColor(bc) => {
            set_border_css_color(style, 0, &bc.top);
            set_border_css_color(style, 1, &bc.right);
            set_border_css_color(style, 2, &bc.bottom);
            set_border_css_color(style, 3, &bc.left);
        }
        Property::BorderTopColor(c) => {
            set_border_css_color(style, 0, c);
        }
        Property::BorderRightColor(c) => {
            set_border_css_color(style, 1, c);
        }
        Property::BorderBottomColor(c) => {
            set_border_css_color(style, 2, c);
        }
        Property::BorderLeftColor(c) => {
            set_border_css_color(style, 3, c);
        }
        Property::BorderStyle(bs) => {
            style.border.border_top_style = line_style_to_border_style(&bs.top);
            style.border.border_right_style = line_style_to_border_style(&bs.right);
            style.border.border_bottom_style = line_style_to_border_style(&bs.bottom);
            style.border.border_left_style = line_style_to_border_style(&bs.left);
        }
        Property::BorderTopStyle(s) => {
            style.border.border_top_style = line_style_to_border_style(s);
        }
        Property::BorderRightStyle(s) => {
            style.border.border_right_style = line_style_to_border_style(s);
        }
        Property::BorderBottomStyle(s) => {
            style.border.border_bottom_style = line_style_to_border_style(s);
        }
        Property::BorderLeftStyle(s) => {
            style.border.border_left_style = line_style_to_border_style(s);
        }
        Property::Border(b) => {
            let Some(width) = checked_border_width(
                &b.width,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            let border_style = line_style_to_border_style(&b.style);
            style.border.border_top_width = width;
            style.border.border_right_width = width;
            style.border.border_bottom_width = width;
            style.border.border_left_width = width;
            set_border_css_color(style, 0, &b.color);
            set_border_css_color(style, 1, &b.color);
            set_border_css_color(style, 2, &b.color);
            set_border_css_color(style, 3, &b.color);
            style.border.border_top_style = border_style;
            style.border.border_right_style = border_style;
            style.border.border_bottom_style = border_style;
            style.border.border_left_style = border_style;
        }
        Property::BorderTop(b) => {
            let Some(width) = checked_border_width(
                &b.width,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.border.border_top_width = width;
            set_border_css_color(style, 0, &b.color);
            style.border.border_top_style = line_style_to_border_style(&b.style);
        }
        Property::BorderRight(b) => {
            let Some(width) = checked_border_width(
                &b.width,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.border.border_right_width = width;
            set_border_css_color(style, 1, &b.color);
            style.border.border_right_style = line_style_to_border_style(&b.style);
        }
        Property::BorderBottom(b) => {
            let Some(width) = checked_border_width(
                &b.width,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.border.border_bottom_width = width;
            set_border_css_color(style, 2, &b.color);
            style.border.border_bottom_style = line_style_to_border_style(&b.style);
        }
        Property::BorderLeft(b) => {
            let Some(width) = checked_border_width(
                &b.width,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.border.border_left_width = width;
            set_border_css_color(style, 3, &b.color);
            style.border.border_left_style = line_style_to_border_style(&b.style);
        }
        Property::BorderRadius(value, _) => {
            let (Some(top_left), Some(top_right), Some(bottom_right), Some(bottom_left)) = (
                corner_radius(
                    &value.top_left,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                corner_radius(
                    &value.top_right,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                corner_radius(
                    &value.bottom_right,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                corner_radius(
                    &value.bottom_left,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
            ) else {
                return ApplyResult::Invalid;
            };
            style.radii.top_left = top_left;
            style.radii.top_right = top_right;
            style.radii.bottom_right = bottom_right;
            style.radii.bottom_left = bottom_left;
        }
        Property::BorderTopLeftRadius(value, _) => {
            let Some(value) = corner_radius(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.radii.top_left = value;
        }
        Property::BorderTopRightRadius(value, _) => {
            let Some(value) = corner_radius(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.radii.top_right = value;
        }
        Property::BorderBottomRightRadius(value, _) => {
            let Some(value) = corner_radius(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.radii.bottom_right = value;
        }
        Property::BorderBottomLeftRadius(value, _) => {
            let Some(value) = corner_radius(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            style.radii.bottom_left = value;
        }
        Property::BorderStartStartRadius(value) => {
            let Some(value) = corner_radius(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_corner_radius(
                style,
                if style.text.direction == TextDirection::Ltr {
                    0
                } else {
                    1
                },
                value,
            );
        }
        Property::BorderStartEndRadius(value) => {
            let Some(value) = corner_radius(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_corner_radius(
                style,
                if style.text.direction == TextDirection::Ltr {
                    1
                } else {
                    0
                },
                value,
            );
        }
        Property::BorderEndStartRadius(value) => {
            let Some(value) = corner_radius(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_corner_radius(
                style,
                if style.text.direction == TextDirection::Ltr {
                    3
                } else {
                    2
                },
                value,
            );
        }
        Property::BorderEndEndRadius(value) => {
            let Some(value) = corner_radius(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_corner_radius(
                style,
                if style.text.direction == TextDirection::Ltr {
                    2
                } else {
                    3
                },
                value,
            );
        }

        // Logical borders are resolved here, while `direction` is known, and
        // only renderer-owned physical sides cross into layout.
        Property::BorderBlockStartWidth(value) => {
            let Some(value) = checked_border_width(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_border_width(style, 0, value);
        }
        Property::BorderBlockEndWidth(value) => {
            let Some(value) = checked_border_width(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_border_width(style, 2, value);
        }
        Property::BorderInlineStartWidth(value) => {
            let Some(value) = checked_border_width(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_border_width(style, inline_sides(style.text.direction).0, value);
        }
        Property::BorderInlineEndWidth(value) => {
            let Some(value) = checked_border_width(
                value,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            set_border_width(style, inline_sides(style.text.direction).1, value);
        }
        Property::BorderBlockWidth(value) => {
            let (Some(start), Some(end)) = (
                checked_border_width(
                    &value.start,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                checked_border_width(
                    &value.end,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
            ) else {
                return ApplyResult::Invalid;
            };
            set_border_width(style, 0, start);
            set_border_width(style, 2, end);
        }
        Property::BorderInlineWidth(value) => {
            let (Some(start), Some(end)) = (
                checked_border_width(
                    &value.start,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
                checked_border_width(
                    &value.end,
                    style.font.font_size,
                    doc.root_font_size(),
                    doc.resolution,
                ),
            ) else {
                return ApplyResult::Invalid;
            };
            let (start_side, end_side) = inline_sides(style.text.direction);
            set_border_width(style, start_side, start);
            set_border_width(style, end_side, end);
        }
        Property::BorderBlockStartStyle(value) => {
            set_border_style(style, 0, line_style_to_border_style(value))
        }
        Property::BorderBlockEndStyle(value) => {
            set_border_style(style, 2, line_style_to_border_style(value))
        }
        Property::BorderInlineStartStyle(value) => set_border_style(
            style,
            inline_sides(style.text.direction).0,
            line_style_to_border_style(value),
        ),
        Property::BorderInlineEndStyle(value) => set_border_style(
            style,
            inline_sides(style.text.direction).1,
            line_style_to_border_style(value),
        ),
        Property::BorderBlockStyle(value) => {
            set_border_style(style, 0, line_style_to_border_style(&value.start));
            set_border_style(style, 2, line_style_to_border_style(&value.end));
        }
        Property::BorderInlineStyle(value) => {
            let (start, end) = inline_sides(style.text.direction);
            set_border_style(style, start, line_style_to_border_style(&value.start));
            set_border_style(style, end, line_style_to_border_style(&value.end));
        }
        Property::BorderBlockStartColor(value) => set_border_css_color(style, 0, value),
        Property::BorderBlockEndColor(value) => set_border_css_color(style, 2, value),
        Property::BorderInlineStartColor(value) => {
            set_border_css_color(style, inline_sides(style.text.direction).0, value)
        }
        Property::BorderInlineEndColor(value) => {
            set_border_css_color(style, inline_sides(style.text.direction).1, value)
        }
        Property::BorderBlockColor(value) => {
            set_border_css_color(style, 0, &value.start);
            set_border_css_color(style, 2, &value.end);
        }
        Property::BorderInlineColor(value) => {
            let (start, end) = inline_sides(style.text.direction);
            set_border_css_color(style, start, &value.start);
            set_border_css_color(style, end, &value.end);
        }
        Property::BorderBlockStart(value) => {
            apply_logical_border(style, 0, value, doc.root_font_size(), doc.resolution)
        }
        Property::BorderBlockEnd(value) => {
            apply_logical_border(style, 2, value, doc.root_font_size(), doc.resolution)
        }
        Property::BorderInlineStart(value) => apply_logical_border(
            style,
            inline_sides(style.text.direction).0,
            value,
            doc.root_font_size(),
            doc.resolution,
        ),
        Property::BorderInlineEnd(value) => apply_logical_border(
            style,
            inline_sides(style.text.direction).1,
            value,
            doc.root_font_size(),
            doc.resolution,
        ),
        Property::BorderBlock(value) => {
            if checked_border_width(
                &value.width,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .is_none()
            {
                return ApplyResult::Invalid;
            }
            apply_logical_border(style, 0, value, doc.root_font_size(), doc.resolution);
            apply_logical_border(style, 2, value, doc.root_font_size(), doc.resolution);
        }
        Property::BorderInline(value) => {
            if checked_border_width(
                &value.width,
                style.font.font_size,
                doc.root_font_size(),
                doc.resolution,
            )
            .is_none()
            {
                return ApplyResult::Invalid;
            }
            let (start, end) = inline_sides(style.text.direction);
            apply_logical_border(style, start, value, doc.root_font_size(), doc.resolution);
            apply_logical_border(style, end, value, doc.root_font_size(), doc.resolution);
        }
        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
