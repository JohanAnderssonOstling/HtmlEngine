use super::super::context::{ApplyResult, PropertyContext};
use super::super::values::fonts::font_variant_caps_features;
use super::super::values::lists::{list_style_image_to_interned, map_list_style_position};
use super::super::values::{
    decoration_color, overflow_mode, resolve_logical_text_alignments, text_decoration_lines,
    text_end_alignment, text_start_alignment,
};
use html_style_model::{LogicalTextAlign, TextAlign, TextOverflow, WhiteSpace};
use lightningcss::properties::Property;
use lightningcss::properties::overflow::TextOverflow as CssTextOverflow;

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    match property {
        Property::LineBreak(value) => {
            use html_style_model::LineBreak;
            use lightningcss::properties::text::LineBreak as CssLineBreak;
            style.text_mut(styles).line_break = match value {
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
        Property::TextAlign(ta) => {
            use LogicalTextAlign::{End, Physical, Start};
            use lightningcss::properties::text::TextAlign as LcTextAlign;
            let (alignment, logical) = match ta {
                LcTextAlign::Left => (TextAlign::Left, Physical),
                LcTextAlign::Right => (TextAlign::Right, Physical),
                LcTextAlign::Center => (TextAlign::Center, Physical),
                LcTextAlign::Justify | LcTextAlign::JustifyAll => (TextAlign::Justify, Physical),
                LcTextAlign::Start => (text_start_alignment(style.text(styles).direction), Start),
                LcTextAlign::End => (text_end_alignment(style.text(styles).direction), End),
                // The parent's logical alignment has already been resolved
                // against its direction. Freeze that physical result here.
                LcTextAlign::MatchParent if context.is_root => {
                    (text_start_alignment(style.text(styles).direction), Start)
                }
                LcTextAlign::MatchParent => (
                    context.parent.with(styles, |parent| parent.text.text_align),
                    Physical,
                ),
            };
            let text = style.text_mut(styles);
            text.text_align = alignment;
            text.text_align_logical = logical;
            text.text_align_justify_all = matches!(ta, LcTextAlign::JustifyAll);
            // Auto follows the paragraph alignment, except justify uses start.
            resolve_logical_text_alignments(style.text_mut(styles));
        }
        Property::TextAlignLast(ta, _) => {
            use LogicalTextAlign::{End, Physical, Start};
            use lightningcss::properties::text::TextAlignLast as LcTextAlignLast;
            let (explicit, logical) = match ta {
                LcTextAlignLast::Auto => (None, style.text(styles).text_align_logical),
                LcTextAlignLast::Left => (Some(TextAlign::Left), Physical),
                LcTextAlignLast::Right => (Some(TextAlign::Right), Physical),
                LcTextAlignLast::Center => (Some(TextAlign::Center), Physical),
                LcTextAlignLast::Justify => (Some(TextAlign::Justify), Physical),
                LcTextAlignLast::Start => (
                    Some(text_start_alignment(style.text(styles).direction)),
                    Start,
                ),
                LcTextAlignLast::End => {
                    (Some(text_end_alignment(style.text(styles).direction)), End)
                }
                _ => (None, Physical),
            };
            style.text_mut(styles).text_align_last_explicit = explicit.is_some();
            style.text_mut(styles).text_align_last =
                explicit.unwrap_or(style.text(styles).text_align);
            style.text_mut(styles).text_align_last_logical = logical;
            resolve_logical_text_alignments(style.text_mut(styles));
        }
        Property::FontVariantCaps(caps) => {
            style.font_mut(styles).font_variant_caps_features = font_variant_caps_features(caps);
        }
        Property::ListStylePosition(pos) => {
            style.text_mut(styles).list_style_position = map_list_style_position(pos);
        }
        Property::ListStyleImage(image) => {
            style.text_mut(styles).list_style_image = list_style_image_to_interned(styles, image);
        }

        Property::TextDecorationLine(line, _) => {
            style.background.text_decoration.lines = text_decoration_lines(line);
        }

        Property::TextDecorationColor(color, _) => {
            style.background.text_decoration.color =
                decoration_color(color, style.text(styles).color);
        }
        Property::OutlineColor(color) => {
            style.background.outline.color = decoration_color(color, style.text(styles).color);
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
            style.text_mut(styles).white_space = match ws {
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
