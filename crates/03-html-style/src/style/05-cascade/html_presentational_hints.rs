//! HTML attributes lowered to declarations below normal author CSS.
//! Native values and renderer keyword tokens enter ordinary winner selection;
//! attribute values are never serialized to CSS text and reparsed.
use super::values::colors::css_color_to_u32;
use html_dom::{Document, DomNodeId};
use html_style_model::PreferredSize;
use lightningcss::properties::Property;
use lightningcss::properties::border::{
    BorderColor, BorderSideWidth, BorderStyle, BorderWidth, LineStyle,
};
use lightningcss::properties::custom::{
    CustomProperty, CustomPropertyName, Token, TokenList, TokenOrValue,
};
use lightningcss::properties::display::{Display, DisplayKeyword};
use lightningcss::properties::font::{FontSize, VerticalAlign, VerticalAlignKeyword};
use lightningcss::properties::list::{CounterStyle, ListStyleType, PredefinedCounterStyle};
use lightningcss::properties::size::{BoxSizing, Size};
use lightningcss::properties::text::{Direction, TextAlign, UnicodeBidi, WhiteSpace};
use lightningcss::traits::Parse;
use lightningcss::values::color::{CssColor, RGBA};
use lightningcss::values::ident::Ident;
use lightningcss::values::length::{Length, LengthPercentage, LengthPercentageOrAuto};
use lightningcss::values::percentage::Percentage;
use lightningcss::values::size::Size2D;
use lightningcss::vendor_prefix::VendorPrefix;

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";

macro_rules! sides {
    ($type:ident, $value:expr) => {{
        let value = $value;
        $type {
            top: value.clone(),
            right: value.clone(),
            bottom: value.clone(),
            left: value,
        }
    }};
}

pub(super) fn collect<'css>(doc: &Document, node: DomNodeId, hints: &mut Vec<Property<'css>>) {
    hints.clear();
    let Some(tag) = doc.get_dom_tag(node) else {
        return;
    };
    let namespace = doc.get_dom_namespace(node);
    if namespace == Some(SVG_NAMESPACE) {
        if tag.eq_ignore_ascii_case("svg") {
            replaced_dimensions(doc, node, hints);
        }
        return;
    }
    if namespace != Some(HTML_NAMESPACE) {
        return;
    }
    let tag = tag.to_ascii_lowercase();
    let tag = tag.as_str();
    let attr = |name| doc.get_dom_attr(node, name);

    if tag == "bdi" {
        hints.push(Property::UnicodeBidi(UnicodeBidi::Isolate));
        hints.push(Property::Direction(html_direction(doc, node)));
    } else if tag == "bdo" {
        hints.push(Property::UnicodeBidi(UnicodeBidi::BidiOverride));
    }
    if let Some(direction) = attr("dir").and_then(|value| {
        if value.eq_ignore_ascii_case("ltr") {
            Some(Direction::Ltr)
        } else if value.eq_ignore_ascii_case("rtl") {
            Some(Direction::Rtl)
        } else if value.eq_ignore_ascii_case("auto") {
            Some(html_direction(doc, node))
        } else {
            None
        }
    }) {
        hints.push(Property::Direction(direction));
        if tag != "bdo" {
            hints.push(Property::UnicodeBidi(UnicodeBidi::Isolate));
        }
    }
    if let Some(color) = match tag {
        "body" => attr("text"),
        "font" | "hr" => attr("color"),
        _ => None,
    }
    .and_then(html_color_hint)
    {
        hints.push(Property::Color(css_color(color)));
    }
    if tag == "font"
        && let Some(size) = attr("size").and_then(html_legacy_font_size)
    {
        hints.push(Property::FontSize(FontSize::Length(LengthPercentage::px(
            size,
        ))));
    }
    if attr("hidden").is_some_and(|value| !value.eq_ignore_ascii_case("until-found")) {
        hints.push(Property::Display(Display::Keyword(DisplayKeyword::None)));
    }
    if matches!(tag, "img" | "object" | "embed" | "iframe" | "input") {
        replaced_dimensions(doc, node, hints);
    }
    if matches!(tag, "table" | "td" | "th" | "col" | "colgroup" | "hr")
        && let Some(width) = attr("width").and_then(if tag == "hr" {
            html_hr_width_hint
        } else {
            html_nonzero_dimension_hint
        })
    {
        hints.push(Property::Width(css_size(width)));
    }
    if matches!(tag, "table" | "tr" | "td" | "th")
        && let Some(height) = attr("height").and_then(html_nonzero_dimension_hint)
    {
        hints.push(Property::Height(css_size(height)));
    }
    if matches!(tag, "body" | "table" | "tr" | "td" | "th")
        && let Some(color) = attr("bgcolor").and_then(html_color_hint)
    {
        hints.push(Property::BackgroundColor(css_color(color)));
    }
    if let Some(align) = attr("align") {
        align_hint(tag, align, hints);
    }
    if matches!(
        tag,
        "col" | "colgroup" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th"
    ) && let Some(align) = attr("valign").and_then(html_vertical_align_hint)
    {
        hints.push(Property::VerticalAlign(VerticalAlign::Keyword(align)));
    }
    if let Some(color) = attr("bordercolor").and_then(html_color_hint) {
        hints.push(Property::BorderColor(sides!(BorderColor, css_color(color))));
    }
    if matches!(tag, "table" | "img" | "object")
        && let Some(width) = attr("border").and_then(html_border_width_hint)
    {
        border_hint(hints, width);
    }
    if matches!(tag, "ol" | "ul" | "li")
        && let Some(value) = attr("type").and_then(|value| html_list_style_hint(tag, value))
    {
        hints.push(Property::ListStyleType(value));
    }
    if tag == "br"
        && let Some(clear) = attr("clear").and_then(html_clear_hint)
    {
        hints.push(keyword("clear", clear));
    }
    if matches!(tag, "img" | "object" | "input" | "embed") {
        if let Some(value) = attr("align") {
            if value.eq_ignore_ascii_case("left") {
                hints.push(keyword("float", "left"));
            } else if value.eq_ignore_ascii_case("right") {
                hints.push(keyword("float", "right"));
            } else if let Some(align) = html_vertical_align_hint(value) {
                hints.push(Property::VerticalAlign(VerticalAlign::Keyword(align)));
            }
        }
        if let Some(value) = attr("hspace").and_then(html_replaced_spacing_hint) {
            hints.push(Property::MarginLeft(value.clone()));
            hints.push(Property::MarginRight(value));
        }
        if let Some(value) = attr("vspace").and_then(html_replaced_spacing_hint) {
            hints.push(Property::MarginTop(value.clone()));
            hints.push(Property::MarginBottom(value));
        }
    }
    if tag == "body" {
        body_margin_hints(doc, node, hints);
    }
    if tag == "hr" {
        hr_hints(doc, node, hints);
    }
    if tag == "table"
        && let Some(spacing) = attr("cellspacing").and_then(html_nonnegative_pixel_hint)
    {
        hints.push(Property::BorderSpacing(Size2D(
            Length::px(spacing),
            Length::px(spacing),
        )));
    }
    if !matches!(tag, "td" | "th") {
        return;
    }
    let table_attr = |name| {
        doc.dom_ancestors(node)
            .filter(|&ancestor| is_html_tag(doc, ancestor, "table"))
            .filter_map(move |ancestor| doc.get_dom_attr(ancestor, name))
    };
    if table_attr("border")
        .find_map(html_border_width_hint)
        .is_some_and(|width| width > 0.0)
    {
        border_hint(hints, 1.0);
    }
    if attr("nowrap").is_some() {
        hints.push(Property::WhiteSpace(WhiteSpace::NoWrap));
    }
    if let Some(padding) = table_attr("cellpadding").find_map(html_nonnegative_pixel_hint) {
        hints.extend([
            Property::PaddingTop(px(padding)),
            Property::PaddingRight(px(padding)),
            Property::PaddingBottom(px(padding)),
            Property::PaddingLeft(px(padding)),
        ]);
    }
}

fn html_direction(doc: &Document, node: DomNodeId) -> Direction {
    if doc
        .element_ref(node)
        .is_some_and(|element| element.html_state().is_rtl())
    {
        Direction::Rtl
    } else {
        Direction::Ltr
    }
}

fn css_color(color: u32) -> CssColor {
    let [red, green, blue, alpha] = color.to_be_bytes();
    CssColor::RGBA(RGBA {
        red,
        green,
        blue,
        alpha,
    })
}

fn css_size(value: PreferredSize) -> Size {
    Size::LengthPercentage(match value {
        PreferredSize::Px(value) => LengthPercentage::px(value),
        PreferredSize::Percent(value) => LengthPercentage::Percentage(Percentage(value)),
        _ => unreachable!("HTML dimensions only produce pixels and percentages"),
    })
}

fn px(value: f32) -> LengthPercentageOrAuto {
    LengthPercentageOrAuto::LengthPercentage(LengthPercentage::px(value))
}

/// These renderer properties have no native Lightning CSS value. Construct
/// their existing keyword tokens directly, without a second CSS parser.
fn keyword<'css>(name: &'static str, value: &'static str) -> Property<'css> {
    Property::Custom(CustomProperty {
        name: CustomPropertyName::Unknown(Ident(name.into())),
        value: TokenList(vec![TokenOrValue::Token(Token::Ident(value.into()))]),
    })
}

fn replaced_dimensions<'css>(doc: &Document, node: DomNodeId, hints: &mut Vec<Property<'css>>) {
    if let Some(value) = doc
        .get_dom_attr(node, "width")
        .and_then(html_dimension_hint)
    {
        hints.push(Property::Width(css_size(value)));
    }
    if let Some(value) = doc
        .get_dom_attr(node, "height")
        .and_then(html_dimension_hint)
    {
        hints.push(Property::Height(css_size(value)));
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

fn html_color_hint(value: &str) -> Option<u32> {
    let value =
        value.trim_matches(|character| matches!(character, '\t' | '\n' | '\u{000C}' | '\r' | ' '));
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
    while red.len().saturating_sub(leading) > 2
        && red[leading] == b'0'
        && green[leading] == b'0'
        && blue[leading] == b'0'
    {
        leading += 1;
    }
    let parse = |component: &[u8]| {
        let component = &component[leading..];
        let end = component.len().min(2);
        u8::from_str_radix(
            std::str::from_utf8(&component[..end]).expect("legacy color digits are ASCII"),
            16,
        )
        .expect("legacy color digits are hexadecimal")
    };
    Some(
        ((parse(red) as u32) << 24)
            | ((parse(green) as u32) << 16)
            | ((parse(blue) as u32) << 8)
            | 0xFF,
    )
}

fn opaque_named_or_hex_color(value: &str) -> Option<u32> {
    let hexadecimal = value.strip_prefix('#').unwrap_or(value);
    if matches!(hexadecimal.len(), 3 | 6)
        && hexadecimal.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        let color = CssColor::parse_string(&format!("#{hexadecimal}")).ok()?;
        let CssColor::RGBA(color) = color else {
            return None;
        };
        return (color.alpha == 0xFF).then(|| css_color_to_u32(&CssColor::RGBA(color), 0x000000FF));
    }
    if !value.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return None;
    }
    let CssColor::RGBA(color) = CssColor::parse_string(value).ok()? else {
        return None;
    };
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

fn align_hint<'css>(tag: &str, value: &str, hints: &mut Vec<Property<'css>>) {
    let value = value.trim();
    if matches!(tag, "img" | "object" | "input" | "embed") {
        return;
    }
    if tag == "table" {
        if value.eq_ignore_ascii_case("left") {
            hints.push(keyword("float", "left"));
        } else if value.eq_ignore_ascii_case("right") {
            hints.push(keyword("float", "right"));
        } else if value.eq_ignore_ascii_case("center") {
            hints.push(Property::MarginLeft(LengthPercentageOrAuto::Auto));
            hints.push(Property::MarginRight(LengthPercentageOrAuto::Auto));
        }
        return;
    }
    if tag == "caption"
        && (value.eq_ignore_ascii_case("top") || value.eq_ignore_ascii_case("bottom"))
    {
        hints.push(keyword(
            "caption-side",
            if value.eq_ignore_ascii_case("top") {
                "top"
            } else {
                "bottom"
            },
        ));
        return;
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
        hints.push(Property::TextAlign(alignment));
    }
}

fn html_vertical_align_hint(value: &str) -> Option<VerticalAlignKeyword> {
    if value.eq_ignore_ascii_case("top") {
        Some(VerticalAlignKeyword::Top)
    } else if value.eq_ignore_ascii_case("middle") || value.eq_ignore_ascii_case("center") {
        Some(VerticalAlignKeyword::Middle)
    } else if value.eq_ignore_ascii_case("bottom") {
        Some(VerticalAlignKeyword::Bottom)
    } else if value.eq_ignore_ascii_case("baseline") {
        Some(VerticalAlignKeyword::Baseline)
    } else {
        None
    }
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

fn border_hint<'css>(hints: &mut Vec<Property<'css>>, width: f32) {
    hints.push(Property::BorderWidth(sides!(
        BorderWidth,
        BorderSideWidth::Length(Length::px(width))
    )));
    hints.push(Property::BorderStyle(sides!(
        BorderStyle,
        if width == 0.0 {
            LineStyle::None
        } else {
            LineStyle::Solid
        }
    )));
}

fn html_list_style_hint<'css>(tag: &str, value: &str) -> Option<ListStyleType<'css>> {
    let value = value.trim();
    if tag.eq_ignore_ascii_case("ol") || tag.eq_ignore_ascii_case("li") {
        let ordered = match value {
            "1" => Some(PredefinedCounterStyle::Decimal),
            "a" => Some(PredefinedCounterStyle::LowerAlpha),
            "A" => Some(PredefinedCounterStyle::UpperAlpha),
            "i" => Some(PredefinedCounterStyle::LowerRoman),
            "I" => Some(PredefinedCounterStyle::UpperRoman),
            _ => None,
        };
        return ordered
            .map(|value| ListStyleType::CounterStyle(CounterStyle::Predefined(value)))
            .or_else(|| {
                tag.eq_ignore_ascii_case("li")
                    .then(|| html_bullet_style_hint(value))
                    .flatten()
            });
    }
    html_bullet_style_hint(value)
}

fn html_bullet_style_hint<'css>(value: &str) -> Option<ListStyleType<'css>> {
    let value = if value.eq_ignore_ascii_case("disc") {
        PredefinedCounterStyle::Disc
    } else if value.eq_ignore_ascii_case("circle") {
        PredefinedCounterStyle::Circle
    } else if value.eq_ignore_ascii_case("square") {
        PredefinedCounterStyle::Square
    } else if value.eq_ignore_ascii_case("none") {
        return Some(ListStyleType::None);
    } else {
        return None;
    };
    Some(ListStyleType::CounterStyle(CounterStyle::Predefined(value)))
}

fn html_clear_hint(value: &str) -> Option<&'static str> {
    if value.eq_ignore_ascii_case("left") {
        Some("left")
    } else if value.eq_ignore_ascii_case("right") {
        Some("right")
    } else if value.eq_ignore_ascii_case("all") || value.eq_ignore_ascii_case("both") {
        Some("both")
    } else if value.eq_ignore_ascii_case("none") {
        Some("none")
    } else {
        None
    }
}

fn html_replaced_spacing_hint(value: &str) -> Option<LengthPercentageOrAuto> {
    let Size::LengthPercentage(value) = css_size(html_dimension_hint(value)?) else {
        unreachable!()
    };
    Some(LengthPercentageOrAuto::LengthPercentage(value))
}

fn body_margin_hints<'css>(doc: &Document, node: DomNodeId, hints: &mut Vec<Property<'css>>) {
    for (attribute, properties) in [
        (
            "marginwidth",
            &[Property::MarginLeft as fn(_) -> _, Property::MarginRight][..],
        ),
        (
            "marginheight",
            &[Property::MarginTop as fn(_) -> _, Property::MarginBottom][..],
        ),
        ("topmargin", &[Property::MarginTop as fn(_) -> _][..]),
        ("rightmargin", &[Property::MarginRight as fn(_) -> _][..]),
        ("bottommargin", &[Property::MarginBottom as fn(_) -> _][..]),
        ("leftmargin", &[Property::MarginLeft as fn(_) -> _][..]),
    ] {
        if let Some(margin) = doc
            .get_dom_attr(node, attribute)
            .and_then(html_nonnegative_pixel_hint)
        {
            hints.extend(properties.iter().map(|property| property(px(margin))));
        }
    }
}

fn hr_hints<'css>(doc: &Document, node: DomNodeId, hints: &mut Vec<Property<'css>>) {
    let flat_border =
        doc.get_dom_attr(node, "color").is_some() || doc.get_dom_attr(node, "noshade").is_some();
    if flat_border {
        hints.push(Property::BorderStyle(sides!(BorderStyle, LineStyle::Solid)));
    }
    let Some(size) = doc
        .get_dom_attr(node, "size")
        .and_then(html_border_width_hint)
        .map(|size| size.max(1.0))
    else {
        return;
    };
    if flat_border {
        hints.push(Property::BorderWidth(sides!(
            BorderWidth,
            BorderSideWidth::Length(Length::px(size / 2.0))
        )));
    } else {
        hints.push(Property::Height(Size::LengthPercentage(
            LengthPercentage::px(size),
        )));
        hints.push(Property::BoxSizing(
            BoxSizing::BorderBox,
            VendorPrefix::None,
        ));
        if size == 1.0 {
            hints.push(Property::BorderBottomWidth(BorderSideWidth::Length(
                Length::px(0.0),
            )));
        }
    }
}

fn html_nonnegative_pixel_hint(value: &str) -> Option<f32> {
    let pixels = value.trim().parse::<f32>().ok()?;
    (pixels.is_finite() && pixels >= 0.0).then_some(pixels)
}

#[cfg(test)]
mod tests {
    use super::{
        html_color_hint, html_dimension_hint, html_hr_width_hint, html_legacy_font_size,
        html_nonzero_dimension_hint,
    };
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
