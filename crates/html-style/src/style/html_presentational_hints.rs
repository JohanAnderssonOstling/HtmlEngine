//! HTML attributes whose rendering is expressed as CSS presentational hints.
//!
//! Hints are applied after normal UA declarations and before normal author
//! declarations. Values cross this boundary as renderer-owned typed values;
//! attribute values are never turned into CSS text and reparsed.

use super::{CascadePhase, WorkingStyle, css_color_to_u32, resolve_logical_text_alignments};
use html_dom::{Document, DomNodeId};
use html_style_model::{BorderStyle, BoxSizing, CaptionSide, Clear, Display, Float, FontRelativeLength, LengthPct, LogicalTextAlign, PreferredSize, TextAlign, TextDirection, VerticalAlignValue, WhiteSpace};
use lightningcss::traits::Parse;
use lightningcss::values::color::CssColor;

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

pub(super) fn apply(doc: &Document, node_idx: DomNodeId, style: &mut WorkingStyle, phase: CascadePhase) {
    let Some(tag) = doc.get_dom_tag(node_idx) else { return };
    let namespace = doc.get_dom_namespace(node_idx);

    // SVG width and height attributes are current SVG presentation behavior,
    // retained here because this is the single DOM-to-style hint boundary.
    if namespace == Some(SVG_NAMESPACE) {
        if phase == CascadePhase::Remaining && tag.eq_ignore_ascii_case("svg") {
            apply_replaced_dimensions(doc, node_idx, style);
        }
        return;
    }
    if namespace != Some(HTML_NAMESPACE) {
        return;
    }

    if phase == CascadePhase::Prerequisites {
        apply_prerequisite_hints(doc, node_idx, tag, style);
        return;
    }

    // The hidden-until-found state retains a principal box. Its
    // content-visibility behavior is separate from the ordinary hidden state.
    if doc.get_dom_attr(node_idx, "hidden").is_some_and(|value| !value.eq_ignore_ascii_case("until-found")) {
        style.box_model.display = Display::None;
    }

    if matches!(tag.to_ascii_lowercase().as_str(), "img" | "object" | "embed" | "iframe" | "input" | "video") {
        apply_replaced_dimensions(doc, node_idx, style);
    }
    if matches!(tag.to_ascii_lowercase().as_str(), "table" | "td" | "th" | "col" | "colgroup" | "hr")
        && let Some(width) = doc.get_dom_attr(node_idx, "width").and_then(|value| if tag.eq_ignore_ascii_case("hr") { html_hr_width_hint(value) } else { html_nonzero_dimension_hint(value) })
    {
        style.box_model.width = width;
    }
    if matches!(tag.to_ascii_lowercase().as_str(), "table" | "tr" | "td" | "th")
        && let Some(height) = doc.get_dom_attr(node_idx, "height").and_then(html_nonzero_dimension_hint)
    {
        style.box_model.height = height;
    }
    if matches!(tag.to_ascii_lowercase().as_str(), "body" | "table" | "tr" | "td" | "th")
        && let Some(color) = doc.get_dom_attr(node_idx, "bgcolor").and_then(html_color_hint)
    {
        style.background.background_color = color;
        style.background.background_color_current_color = false;
    }
    if let Some(align) = doc.get_dom_attr(node_idx, "align") {
        apply_html_align_hint(tag, align, style);
    }
    if matches!(tag.to_ascii_lowercase().as_str(), "col" | "colgroup" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th")
        && let Some(vertical_align) = doc.get_dom_attr(node_idx, "valign").and_then(html_vertical_align_hint)
    {
        style.box_model.vertical_align = vertical_align;
    }
    if let Some(color) = doc.get_dom_attr(node_idx, "bordercolor").and_then(html_color_hint) {
        set_html_border_color(style, color);
    }
    if tag.eq_ignore_ascii_case("table")
        && let Some(width) = doc.get_dom_attr(node_idx, "border").and_then(html_border_width_hint)
    {
        set_html_border(style, width);
    }
    if matches!(tag.to_ascii_lowercase().as_str(), "img" | "object")
        && let Some(width) = doc.get_dom_attr(node_idx, "border").and_then(html_border_width_hint)
    {
        set_html_border(style, width);
    }
    if matches!(tag.to_ascii_lowercase().as_str(), "ol" | "ul" | "li")
        && let Some(list_style) = doc.get_dom_attr(node_idx, "type").and_then(|value| html_list_style_hint(tag, value))
    {
        style.text.list_style_type = list_style;
    }
    if tag.eq_ignore_ascii_case("br")
        && let Some(clear) = doc.get_dom_attr(node_idx, "clear").and_then(html_clear_hint)
    {
        style.box_model.clear = clear;
    }
    if matches!(tag.to_ascii_lowercase().as_str(), "img" | "object" | "input" | "iframe" | "embed") {
        apply_html_replaced_alignment_hint(doc, node_idx, style);
        apply_html_replaced_spacing_hints(doc, node_idx, style);
    }
    if tag.eq_ignore_ascii_case("body") {
        apply_html_body_margin_hints(doc, node_idx, style);
    }
    if tag.eq_ignore_ascii_case("hr") {
        apply_html_hr_hints(doc, node_idx, style);
    }
    if tag.eq_ignore_ascii_case("table")
        && let Some(spacing) = doc.get_dom_attr(node_idx, "cellspacing").and_then(html_nonnegative_pixel_hint)
    {
        style.box_model.border_spacing_horizontal = spacing;
        style.box_model.border_spacing_vertical = spacing;
    }
    if !matches!(tag.to_ascii_lowercase().as_str(), "td" | "th") {
        return;
    }
    let ancestor_table_border = doc.dom_ancestors(node_idx).find_map(|ancestor| (is_html_tag(doc, ancestor, "table")).then(|| doc.get_dom_attr(ancestor, "border").and_then(html_border_width_hint)).flatten());
    if ancestor_table_border.is_some_and(|width| width > 0.0) {
        set_html_cell_border(style);
    }
    if doc.get_dom_attr(node_idx, "nowrap").is_some() {
        style.text.white_space = WhiteSpace::NoWrap;
    }
    let padding = doc.dom_ancestors(node_idx).find_map(|ancestor| (is_html_tag(doc, ancestor, "table")).then(|| doc.get_dom_attr(ancestor, "cellpadding").and_then(html_nonnegative_pixel_hint)).flatten());
    if let Some(padding) = padding {
        let padding = LengthPct::Px(padding);
        style.box_model.padding_top = padding;
        style.box_model.padding_right = padding;
        style.box_model.padding_bottom = padding;
        style.box_model.padding_left = padding;
    }
}

fn apply_prerequisite_hints(doc: &Document, node_idx: DomNodeId, tag: &str, style: &mut WorkingStyle) {
    if let Some(direction) = doc.get_dom_attr(node_idx, "dir") {
        if direction.eq_ignore_ascii_case("ltr") {
            style.text.direction = TextDirection::Ltr;
        } else if direction.eq_ignore_ascii_case("rtl") {
            style.text.direction = TextDirection::Rtl;
        }
        resolve_logical_text_alignments(&mut style.text);
    }
    if tag.eq_ignore_ascii_case("body")
        && let Some(color) = doc.get_dom_attr(node_idx, "text").and_then(html_color_hint)
    {
        style.text.color = color;
    }
    if tag.eq_ignore_ascii_case("font")
        && let Some(color) = doc.get_dom_attr(node_idx, "color").and_then(html_color_hint)
    {
        style.text.color = color;
    }
    if tag.eq_ignore_ascii_case("font")
        && let Some(font_size) = doc.get_dom_attr(node_idx, "size").and_then(html_legacy_font_size)
    {
        // A font-size hint participates in the prerequisite cascade so em
        // lengths declared later on the same element resolve against it.
        style.font.font_size = font_size;
        style.font.font_size_x_height_px = 0.0;
        style.font.font_size_ch_advance_px = 0.0;
        style.font.font_size_cap_height_px = 0.0;
        style.font.font_size_root_ch = 0.0;
        style.font.font_size_root_cap_height = 0.0;
        style.font.font_size_root_line_height = 0.0;
        if style.text.line_height_number.is_finite() {
            style.text.line_height = font_size * style.text.line_height_number;
            style.text.line_height_x_height_px = 0.0;
        }
    }
    if tag.eq_ignore_ascii_case("hr")
        && let Some(color) = doc.get_dom_attr(node_idx, "color").and_then(html_color_hint)
    {
        style.text.color = color;
    }
}

/// Parse the HTML `font[size]` syntax and return the CSS absolute-size
/// mapping used by this resolver. Relative values are based on legacy size 3
/// and all results are clamped to the 1..=7 range.
fn html_legacy_font_size(value: &str) -> Option<f32> {
    let value = value.trim_start_matches(|character: char| character.is_ascii_whitespace());
    let (mode, digits) = match value.as_bytes().first().copied() {
        Some(b'+') => (1i8, &value[1..]),
        Some(b'-') => (-1i8, &value[1..]),
        Some(_) => (0i8, value),
        None => return None,
    };
    let digit_count = digits.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    let parsed = digits[..digit_count].parse::<u64>().unwrap_or(u64::MAX);
    let legacy_size = match mode {
        1 => 3u64.saturating_add(parsed),
        -1 => 3u64.saturating_sub(parsed),
        _ => parsed,
    }
    .clamp(1, 7) as usize;
    Some([10.0, 13.0, 16.0, 18.0, 24.0, 32.0, 48.0][legacy_size - 1])
}

fn is_html_tag(doc: &Document, node: DomNodeId, expected: &str) -> bool {
    doc.get_dom_namespace(node) == Some(HTML_NAMESPACE) && doc.get_dom_tag(node).is_some_and(|tag| tag.eq_ignore_ascii_case(expected))
}

fn apply_replaced_dimensions(doc: &Document, node_idx: DomNodeId, style: &mut WorkingStyle) {
    if let Some(width) = doc.get_dom_attr(node_idx, "width").and_then(html_dimension_hint) {
        style.box_model.width = width;
    }
    if let Some(height) = doc.get_dom_attr(node_idx, "height").and_then(html_dimension_hint) {
        style.box_model.height = height;
    }
}

fn html_color_hint(value: &str) -> Option<u32> {
    let value = value.trim_matches(|character| matches!(character, '\t' | '\n' | '\u{000C}' | '\r' | ' '));
    if value.is_empty() || value.eq_ignore_ascii_case("transparent") {
        return None;
    }
    if let Some(color) = opaque_named_or_hex_color(value) {
        return Some(color);
    }

    // HTML's legacy color algorithm deliberately turns malformed values into
    // RGB instead of applying CSS color error recovery. Work on at most 128
    // source code points, and encode non-BMP characters as two replacement
    // digits as required by the legacy algorithm.
    let mut digits = Vec::with_capacity(value.len().min(128));
    for character in value.chars().take(128) {
        if character.is_ascii_hexdigit() {
            digits.push(character as u8);
        } else if u32::from(character) > 0xFFFF {
            digits.extend_from_slice(b"00");
        } else {
            digits.push(b'0');
        }
    }
    while digits.len() % 3 != 0 {
        digits.push(b'0');
    }

    let component_length = digits.len() / 3;
    let red = &digits[..component_length];
    let green = &digits[component_length..component_length * 2];
    let blue = &digits[component_length * 2..];
    let red = &red[red.len().saturating_sub(8)..];
    let green = &green[green.len().saturating_sub(8)..];
    let blue = &blue[blue.len().saturating_sub(8)..];
    let mut leading = 0usize;
    while red.len().saturating_sub(leading) > 2 && red[leading] == b'0' && green[leading] == b'0' && blue[leading] == b'0' {
        leading += 1;
    }
    let parse = |component: &[u8]| {
        let component = &component[leading..];
        let end = component.len().min(2);
        u8::from_str_radix(std::str::from_utf8(&component[..end]).expect("legacy color digits are ASCII"), 16).expect("legacy color digits are hexadecimal")
    };
    Some(((parse(red) as u32) << 24) | ((parse(green) as u32) << 16) | ((parse(blue) as u32) << 8) | 0xFF)
}

fn opaque_named_or_hex_color(value: &str) -> Option<u32> {
    let hexadecimal = value.strip_prefix('#').unwrap_or(value);
    if matches!(hexadecimal.len(), 3 | 6) && hexadecimal.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        let color = CssColor::parse_string(&format!("#{hexadecimal}")).ok()?;
        let CssColor::RGBA(color) = color else { return None };
        return (color.alpha == 0xFF).then(|| css_color_to_u32(&CssColor::RGBA(color), 0x000000FF));
    }
    if !value.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return None;
    }
    let CssColor::RGBA(color) = CssColor::parse_string(value).ok()? else { return None };
    (color.alpha == 0xFF).then(|| css_color_to_u32(&CssColor::RGBA(color), 0x000000FF))
}

fn html_dimension_hint(value: &str) -> Option<PreferredSize> {
    let value = value.trim();
    if value.starts_with(['+', '-']) {
        return None;
    }
    let numeric_end = value.char_indices().take_while(|(_, character)| character.is_ascii_digit() || *character == '.').map(|(index, character)| index + character.len_utf8()).last()?;
    let number = value[..numeric_end].parse::<f32>().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    if value[numeric_end..].trim_start().starts_with('%') { Some(PreferredSize::Percent(number / 100.0)) } else { Some(PreferredSize::Px(number)) }
}

fn html_nonzero_dimension_hint(value: &str) -> Option<PreferredSize> {
    let value = html_dimension_hint(value)?;
    match value {
        PreferredSize::Px(pixels) if pixels == 0.0 => None,
        PreferredSize::Percent(percent) if percent == 0.0 => None,
        _ => Some(value),
    }
}

fn html_hr_width_hint(value: &str) -> Option<PreferredSize> {
    match html_dimension_hint(value)? {
        PreferredSize::Px(pixels) if pixels == 0.0 => Some(PreferredSize::Percent(0.0)),
        value => Some(value),
    }
}

fn apply_html_align_hint(tag: &str, value: &str, style: &mut WorkingStyle) {
    let value = value.trim();
    if matches!(tag.to_ascii_lowercase().as_str(), "img" | "object" | "input" | "iframe" | "embed") {
        return;
    }
    if tag.eq_ignore_ascii_case("table") {
        if value.eq_ignore_ascii_case("left") {
            style.box_model.float = Float::Left;
        } else if value.eq_ignore_ascii_case("right") {
            style.box_model.float = Float::Right;
        } else if value.eq_ignore_ascii_case("center") {
            style.layout.margin_left_auto = true;
            style.layout.margin_right_auto = true;
        }
        return;
    }
    if tag.eq_ignore_ascii_case("caption") {
        if value.eq_ignore_ascii_case("top") {
            style.box_model.caption_side = CaptionSide::Top;
            return;
        }
        if value.eq_ignore_ascii_case("bottom") {
            style.box_model.caption_side = CaptionSide::Bottom;
            return;
        }
    }
    let alignment = if value.eq_ignore_ascii_case("left") {
        Some(TextAlign::Left)
    } else if value.eq_ignore_ascii_case("right") {
        Some(TextAlign::Right)
    } else if value.eq_ignore_ascii_case("center") || value.eq_ignore_ascii_case("middle") {
        Some(TextAlign::Center)
    } else if value.eq_ignore_ascii_case("justify") {
        Some(TextAlign::Justify)
    } else {
        None
    };
    if let Some(alignment) = alignment {
        style.text.text_align = alignment;
        style.text.text_align_logical = LogicalTextAlign::Physical;
        if !style.text.text_align_last_explicit {
            style.text.text_align_last = alignment;
            style.text.text_align_last_logical = LogicalTextAlign::Physical;
        }
    }
}

fn html_vertical_align_hint(value: &str) -> Option<VerticalAlignValue> {
    if value.eq_ignore_ascii_case("top") {
        Some(VerticalAlignValue::Top)
    } else if value.eq_ignore_ascii_case("middle") || value.eq_ignore_ascii_case("center") {
        Some(VerticalAlignValue::Middle)
    } else if value.eq_ignore_ascii_case("bottom") {
        Some(VerticalAlignValue::Bottom)
    } else if value.eq_ignore_ascii_case("baseline") {
        Some(VerticalAlignValue::Baseline)
    } else {
        None
    }
}

fn set_html_border_color(style: &mut WorkingStyle, color: u32) {
    style.border.border_top_color = color;
    style.border.border_right_color = color;
    style.border.border_bottom_color = color;
    style.border.border_left_color = color;
    style.border.current_color_sides = 0;
}

fn html_border_width_hint(value: &str) -> Option<f32> {
    let value = value.trim_start();
    let (negative, digits) = if let Some(value) = value.strip_prefix('-') { (true, value) } else { (false, value.strip_prefix('+').unwrap_or(value)) };
    let digits = &digits[..digits.bytes().take_while(|byte| byte.is_ascii_digit()).count()];
    if digits.is_empty() {
        return Some(1.0);
    }
    let integer = digits.parse::<u64>().unwrap_or(u64::MAX);
    if integer == 0 {
        Some(0.0)
    } else if negative {
        Some(1.0)
    } else {
        Some((integer as f32).max(1.0))
    }
}

fn set_html_border(style: &mut WorkingStyle, width: f32) {
    let border_style = if width == 0.0 { BorderStyle::None } else { BorderStyle::Solid };
    let width = FontRelativeLength::px(width).expect("legacy HTML border width is finite and non-negative");
    style.border.border_top_width = width;
    style.border.border_right_width = width;
    style.border.border_bottom_width = width;
    style.border.border_left_width = width;
    style.border.border_top_style = border_style;
    style.border.border_right_style = border_style;
    style.border.border_bottom_style = border_style;
    style.border.border_left_style = border_style;
}

fn set_html_cell_border(style: &mut WorkingStyle) {
    let width = FontRelativeLength::px(1.0).expect("legacy HTML cell border width is finite and non-negative");
    style.border.border_top_width = width;
    style.border.border_right_width = width;
    style.border.border_bottom_width = width;
    style.border.border_left_width = width;
    style.border.border_top_style = BorderStyle::Solid;
    style.border.border_right_style = BorderStyle::Solid;
    style.border.border_bottom_style = BorderStyle::Solid;
    style.border.border_left_style = BorderStyle::Solid;
}

fn html_list_style_hint(tag: &str, value: &str) -> Option<html_style_model::ListStyleType> {
    let value = value.trim();
    if tag.eq_ignore_ascii_case("ol") || tag.eq_ignore_ascii_case("li") {
        let ordered = match value {
            "1" => Some(html_style_model::ListStyleType::Decimal),
            "a" => Some(html_style_model::ListStyleType::LowerAlpha),
            "A" => Some(html_style_model::ListStyleType::UpperAlpha),
            "i" => Some(html_style_model::ListStyleType::LowerRoman),
            "I" => Some(html_style_model::ListStyleType::UpperRoman),
            _ => None,
        };
        return ordered.or_else(|| tag.eq_ignore_ascii_case("li").then(|| html_bullet_style_hint(value)).flatten());
    }
    html_bullet_style_hint(value)
}

fn html_bullet_style_hint(value: &str) -> Option<html_style_model::ListStyleType> {
    if value.eq_ignore_ascii_case("disc") {
        Some(html_style_model::ListStyleType::Disc)
    } else if value.eq_ignore_ascii_case("circle") {
        Some(html_style_model::ListStyleType::Circle)
    } else if value.eq_ignore_ascii_case("square") {
        Some(html_style_model::ListStyleType::Square)
    } else if value.eq_ignore_ascii_case("none") {
        Some(html_style_model::ListStyleType::None)
    } else {
        None
    }
}

fn html_clear_hint(value: &str) -> Option<Clear> {
    if value.eq_ignore_ascii_case("left") {
        Some(Clear::Left)
    } else if value.eq_ignore_ascii_case("right") {
        Some(Clear::Right)
    } else if value.eq_ignore_ascii_case("all") || value.eq_ignore_ascii_case("both") {
        Some(Clear::Both)
    } else if value.eq_ignore_ascii_case("none") {
        Some(Clear::None)
    } else {
        None
    }
}

fn apply_html_replaced_alignment_hint(doc: &Document, node_idx: DomNodeId, style: &mut WorkingStyle) {
    let Some(value) = doc.get_dom_attr(node_idx, "align") else { return };
    if value.eq_ignore_ascii_case("left") {
        style.box_model.float = Float::Left;
    } else if value.eq_ignore_ascii_case("right") {
        style.box_model.float = Float::Right;
    } else if let Some(vertical_align) = html_vertical_align_hint(value) {
        style.box_model.vertical_align = vertical_align;
    }
}

fn apply_html_replaced_spacing_hints(doc: &Document, node_idx: DomNodeId, style: &mut WorkingStyle) {
    if let Some(horizontal) = doc.get_dom_attr(node_idx, "hspace").and_then(html_replaced_spacing_hint) {
        style.box_model.margin_left = horizontal;
        style.box_model.margin_right = horizontal;
    }
    if let Some(vertical) = doc.get_dom_attr(node_idx, "vspace").and_then(html_replaced_spacing_hint) {
        style.box_model.margin_top = vertical;
        style.box_model.margin_bottom = vertical;
    }
}

fn html_replaced_spacing_hint(value: &str) -> Option<LengthPct> {
    match html_dimension_hint(value)? {
        PreferredSize::Px(pixels) => Some(LengthPct::Px(pixels)),
        PreferredSize::Percent(percentage) => Some(LengthPct::Pct(percentage)),
        _ => None,
    }
}

fn apply_html_body_margin_hints(doc: &Document, node_idx: DomNodeId, style: &mut WorkingStyle) {
    if let Some(horizontal) = doc.get_dom_attr(node_idx, "marginwidth").and_then(html_nonnegative_pixel_hint) {
        style.box_model.margin_left = LengthPct::Px(horizontal);
        style.box_model.margin_right = LengthPct::Px(horizontal);
    }
    if let Some(vertical) = doc.get_dom_attr(node_idx, "marginheight").and_then(html_nonnegative_pixel_hint) {
        style.box_model.margin_top = LengthPct::Px(vertical);
        style.box_model.margin_bottom = LengthPct::Px(vertical);
    }
    for (attribute, side) in [("topmargin", 0), ("rightmargin", 1), ("bottommargin", 2), ("leftmargin", 3)] {
        let Some(margin) = doc.get_dom_attr(node_idx, attribute).and_then(html_nonnegative_pixel_hint) else { continue };
        match side {
            0 => style.box_model.margin_top = LengthPct::Px(margin),
            1 => style.box_model.margin_right = LengthPct::Px(margin),
            2 => style.box_model.margin_bottom = LengthPct::Px(margin),
            _ => style.box_model.margin_left = LengthPct::Px(margin),
        }
    }
}

fn apply_html_hr_hints(doc: &Document, node_idx: DomNodeId, style: &mut WorkingStyle) {
    let flat_border = doc.get_dom_attr(node_idx, "color").is_some() || doc.get_dom_attr(node_idx, "noshade").is_some();
    if flat_border {
        style.border.border_top_style = BorderStyle::Solid;
        style.border.border_right_style = BorderStyle::Solid;
        style.border.border_bottom_style = BorderStyle::Solid;
        style.border.border_left_style = BorderStyle::Solid;
    }
    let Some(size) = doc.get_dom_attr(node_idx, "size").and_then(html_border_width_hint).map(|size| size.max(1.0)) else { return };
    if flat_border {
        let width = FontRelativeLength::px(size / 2.0).expect("legacy HTML hr border width is finite and non-negative");
        style.border.border_top_width = width;
        style.border.border_right_width = width;
        style.border.border_bottom_width = width;
        style.border.border_left_width = width;
    } else {
        style.box_model.height = PreferredSize::Px(size);
        style.box_model.box_sizing = BoxSizing::BorderBox;
        if size == 1.0 {
            style.border.border_bottom_width = FontRelativeLength::ZERO;
        }
    }
}

fn html_nonnegative_pixel_hint(value: &str) -> Option<f32> {
    let pixels = value.trim().parse::<f32>().ok()?;
    (pixels.is_finite() && pixels >= 0.0).then_some(pixels)
}

#[cfg(test)]
mod tests {
    use super::{html_color_hint, html_dimension_hint, html_hr_width_hint, html_legacy_font_size, html_nonzero_dimension_hint};
    use html_style_model::PreferredSize;

    #[test]
    fn legacy_dimensions_consume_valid_numeric_prefixes() {
        assert_eq!(html_dimension_hint("100foo"), Some(PreferredSize::Px(100.0)));
        assert_eq!(html_dimension_hint("100.99"), Some(PreferredSize::Px(100.99)));
        assert_eq!(html_dimension_hint(" 10% trailing"), Some(PreferredSize::Percent(0.1)));
        assert_eq!(html_dimension_hint("+0"), None);
        assert_eq!(html_dimension_hint("++0"), None);
    }

    #[test]
    fn zero_dimension_rules_are_element_specific() {
        assert_eq!(html_nonzero_dimension_hint("0"), None);
        assert_eq!(html_hr_width_hint("0"), Some(PreferredSize::Percent(0.0)));
        assert_eq!(html_hr_width_hint("0%"), Some(PreferredSize::Percent(0.0)));
    }

    #[test]
    fn legacy_colors_accept_bare_hexadecimal_rgb() {
        assert_eq!(html_color_hint("0000ff"), Some(0x0000ffff));
        assert_eq!(html_color_hint("0ff"), Some(0x00ffffff));
    }

    #[test]
    fn legacy_colors_use_ascii_transparent_matching_and_garbage_conversion() {
        assert_eq!(html_color_hint("transparent"), None);
        assert_eq!(html_color_hint("TrAnSpArEnT"), None);
        assert_eq!(html_color_hint("tranſparent"), Some(0x0000e0ff));
    }

    #[test]
    fn legacy_colors_keep_named_and_hash_notation_on_the_fast_path() {
        assert_eq!(html_color_hint("red"), Some(0xff0000ff));
        assert_eq!(html_color_hint("#0f0"), Some(0x00ff00ff));
        assert_eq!(html_color_hint("#0000ff"), Some(0x0000ffff));
    }

    #[test]
    fn legacy_font_sizes_map_relative_values_and_clamp() {
        assert_eq!(html_legacy_font_size("7"), Some(48.0));
        assert_eq!(html_legacy_font_size("+2 trailing"), Some(24.0));
        assert_eq!(html_legacy_font_size("-1"), Some(13.0));
        assert_eq!(html_legacy_font_size("0"), Some(10.0));
        assert_eq!(html_legacy_font_size("+999999999999999999999"), Some(48.0));
        assert_eq!(html_legacy_font_size(" +"), None);
    }
}
