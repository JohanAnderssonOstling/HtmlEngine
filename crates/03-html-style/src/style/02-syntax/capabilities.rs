//! Renderer capability classification.
//!
//! Parser acceptance and renderer support are deliberately separate facts. A
//! declaration such as `opacity: 0.5` can be valid CSS while still being a
//! feature this renderer does not implement.

use crate::{PropertyCapability, UnsupportedStyleFeature};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::values::image::Image;

pub(crate) fn is_supported_property_name(name: &str) -> bool {
    matches!(
        name,
        "all"
            | "font-size"
            | "font-weight"
            | "font-style"
            | "font-family"
            | "font"
            | "line-height"
            | "letter-spacing"
            | "word-spacing"
            | "tab-size"
            | "text-align"
            | "text-align-last"
            | "text-indent"
            | "text-overflow"
            | "text-box"
            | "text-box-trim"
            | "text-box-edge"
            | "text-transform"
            | "direction"
            | "font-variant-caps"
            | "font-variant-numeric"
            | "font-variant-ligatures"
            | "font-kerning"
            | "font-feature-settings"
            | "text-decoration-line"
            | "text-decoration"
            | "text-decoration-color"
            | "text-decoration-style"
            | "text-decoration-thickness"
            | "vertical-align"
            | "white-space"
            | "list-style-type"
            | "list-style-position"
            | "list-style-image"
            | "list-style"
            | "color"
            | "background-color"
            | "background"
            | "width"
            | "height"
            | "aspect-ratio"
            | "object-fit"
            | "object-position"
            | "visibility"
            | "outline-offset"
            | "min-width"
            | "min-height"
            | "max-width"
            | "max-height"
            | "inline-size"
            | "block-size"
            | "min-inline-size"
            | "min-block-size"
            | "max-inline-size"
            | "max-block-size"
            | "margin-top"
            | "margin-bottom"
            | "margin-left"
            | "margin-right"
            | "margin"
            | "margin-block"
            | "margin-block-start"
            | "margin-block-end"
            | "margin-inline"
            | "margin-inline-start"
            | "margin-inline-end"
            | "padding-top"
            | "padding-bottom"
            | "padding-left"
            | "padding-right"
            | "padding"
            | "padding-block"
            | "padding-block-start"
            | "padding-block-end"
            | "padding-inline"
            | "padding-inline-start"
            | "padding-inline-end"
            | "border-width"
            | "border-top-width"
            | "border-right-width"
            | "border-bottom-width"
            | "border-left-width"
            | "border-block-start-width"
            | "border-block-end-width"
            | "border-inline-start-width"
            | "border-inline-end-width"
            | "border-block-width"
            | "border-inline-width"
            | "border-color"
            | "border-top-color"
            | "border-right-color"
            | "border-bottom-color"
            | "border-left-color"
            | "border-block-start-color"
            | "border-block-end-color"
            | "border-inline-start-color"
            | "border-inline-end-color"
            | "border-block-color"
            | "border-inline-color"
            | "border-style"
            | "border-top-style"
            | "border-right-style"
            | "border-bottom-style"
            | "border-left-style"
            | "border-block-start-style"
            | "border-block-end-style"
            | "border-inline-start-style"
            | "border-inline-end-style"
            | "border-block-style"
            | "border-inline-style"
            | "border"
            | "border-top"
            | "border-bottom"
            | "border-left"
            | "border-right"
            | "border-block"
            | "border-block-start"
            | "border-block-end"
            | "border-inline"
            | "border-inline-start"
            | "border-inline-end"
            | "border-radius"
            | "border-top-left-radius"
            | "border-top-right-radius"
            | "border-bottom-right-radius"
            | "border-bottom-left-radius"
            | "border-start-start-radius"
            | "border-start-end-radius"
            | "border-end-start-radius"
            | "border-end-end-radius"
            | "border-spacing"
            | "table-layout"
            | "border-collapse"
            | "caption-side"
            | "empty-cells"
            | "outline"
            | "outline-width"
            | "outline-style"
            | "outline-color"
            | "float"
            | "clear"
            | "hyphens"
            | "word-break"
            | "overflow-wrap"
            | "word-wrap"
            | "overflow"
            | "overflow-x"
            | "overflow-y"
            | "box-sizing"
            | "contain"
            | "break-before"
            | "break-after"
            | "break-inside"
            | "page-break-before"
            | "page-break-after"
            | "page-break-inside"
            | "widows"
            | "orphans"
            | "display"
            | "position"
            | "z-index"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "inset"
            | "flex-direction"
            | "flex-wrap"
            | "flex-flow"
            | "flex-grow"
            | "flex-shrink"
            | "flex-basis"
            | "flex"
            | "order"
            | "align-content"
            | "justify-content"
            | "align-items"
            | "align-self"
            | "justify-items"
            | "justify-self"
            | "place-content"
            | "place-items"
            | "place-self"
            | "row-gap"
            | "column-gap"
            | "grid-row-gap"
            | "grid-column-gap"
            | "grid-gap"
            | "gap"
            | "grid-template-rows"
            | "grid-template-columns"
            | "grid-template-areas"
            | "grid-auto-rows"
            | "grid-auto-columns"
            | "grid-auto-flow"
            | "grid-template"
            | "grid"
            | "grid-row-start"
            | "grid-row-end"
            | "grid-column-start"
            | "grid-column-end"
            | "grid-row"
            | "grid-column"
            | "grid-area"
            | "content"
            | "counter-reset"
            | "counter-increment"
            | "quotes"
    )
}

pub fn property_name_is_supported(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    normalized.starts_with("--") || is_supported_property_name(&normalized)
}

pub(crate) fn declaration_capability(name: &str, value: &str) -> PropertyCapability {
    if name.eq_ignore_ascii_case("content") {
        return match super::generated_content::content(value) {
            super::generated_content::ValueSupport::Unsupported => {
                PropertyCapability::Unsupported(UnsupportedStyleFeature::GeneratedContent)
            }
            super::generated_content::ValueSupport::Supported
            | super::generated_content::ValueSupport::Invalid => PropertyCapability::Supported,
        };
    }
    if name.eq_ignore_ascii_case("counter-reset")
        || name.eq_ignore_ascii_case("counter-increment")
    {
        return match super::generated_content::counter_directive(name, value) {
            super::generated_content::ValueSupport::Unsupported => {
                PropertyCapability::Unsupported(UnsupportedStyleFeature::GeneratedContent)
            }
            super::generated_content::ValueSupport::Supported
            | super::generated_content::ValueSupport::Invalid => PropertyCapability::Supported,
        };
    }
    if name.eq_ignore_ascii_case("contain") {
        let css_wide = matches!(value.trim().to_ascii_lowercase().as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer");
        return if css_wide || crate::style::syntax::contain::parse(value).is_some_and(|contain| contain.fully_supported) {
            PropertyCapability::Supported
        } else {
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        };
    }
    if is_border_image_property_name(name) {
        return PropertyCapability::Unsupported(UnsupportedStyleFeature::BorderImage);
    }
    if is_box_shadow_property_name(name) {
        return PropertyCapability::Unsupported(UnsupportedStyleFeature::BoxShadow);
    }
    if is_filter_effect_property_name(name) {
        return PropertyCapability::Unsupported(UnsupportedStyleFeature::FilterEffects);
    }
    if is_transition_property_name(name) {
        return PropertyCapability::Unsupported(UnsupportedStyleFeature::Transitions);
    }
    if is_animation_property_name(name) {
        return PropertyCapability::Unsupported(UnsupportedStyleFeature::Animations);
    }
    // Avoid a second parse on the overwhelmingly common non-gradient path.
    // The typed check prevents strings such as url('gradient.png') from being
    // mistaken for a gradient function.
    if contains_gradient_function(value) {
        let parsed = Property::parse_string(PropertyId::from(name), value, ParserOptions::default()).ok();
        if parsed.as_ref().is_some_and(property_uses_gradient) || parsed.as_ref().is_none_or(|property| matches!(property, Property::Unparsed(_))) {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::Gradient);
        }
    }
    if name.eq_ignore_ascii_case("background") || name.eq_ignore_ascii_case("background-image") {
        let parsed = Property::parse_string(PropertyId::from(name), value, ParserOptions::default()).ok();
        if parsed.as_ref().is_some_and(property_uses_background_image) {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::BackgroundImage);
        }
    }
    if name.eq_ignore_ascii_case("display") {
        let parsed = Property::parse_string(PropertyId::from(name), value, ParserOptions::default()).ok();
        if parsed.as_ref().is_some_and(display_value_is_unsupported) {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::Value);
        }
    }
    if name.eq_ignore_ascii_case("position") {
        let parsed = Property::parse_string(PropertyId::from(name), value, ParserOptions::default()).ok();
        if parsed.as_ref().is_some_and(|property| {
            !matches!(property, Property::Position(lightningcss::properties::position::Position::Static | lightningcss::properties::position::Position::Relative | lightningcss::properties::position::Position::Absolute))
        }) {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::Value);
        }
    }
    if name.eq_ignore_ascii_case("visibility") {
        let parsed = Property::parse_string(PropertyId::from(name), value, ParserOptions::default()).ok();
        if matches!(parsed, Some(Property::Visibility(lightningcss::properties::display::Visibility::Collapse))) {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::Value);
        }
    }
    if name.eq_ignore_ascii_case("list-style-type") || name.eq_ignore_ascii_case("list-style") {
        let parsed = Property::parse_string(PropertyId::from(name), value, ParserOptions::default()).ok();
        if parsed.as_ref().is_some_and(list_style_value_is_unsupported) {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::Value);
        }
    }
    if name.eq_ignore_ascii_case("text-transform") {
        let parsed = Property::parse_string(PropertyId::from(name), value, ParserOptions::default()).ok();
        if parsed.as_ref().is_some_and(text_transform_value_is_unsupported) {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::Value);
        }
    }
    if matches!(name, "text-decoration" | "text-decoration-line" | "text-decoration-style" | "outline" | "outline-style") {
        let parsed = Property::parse_string(PropertyId::from(name), value, ParserOptions::default()).ok();
        if parsed.as_ref().is_some_and(|property| property_uses_unsupported_text_decoration_style(property) || property_uses_unsupported_outline_style(property)) {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::Value);
        }
    }
    if property_name_is_supported(name) { PropertyCapability::Supported } else { PropertyCapability::Unsupported(UnsupportedStyleFeature::Property) }
}

fn is_border_image_property_name(name: &str) -> bool {
    matches!(name, "border-image" | "border-image-source" | "border-image-slice" | "border-image-width" | "border-image-outset" | "border-image-repeat")
}

fn is_box_shadow_property_name(name: &str) -> bool {
    matches!(name, "box-shadow" | "-webkit-box-shadow" | "-moz-box-shadow")
}

fn is_filter_effect_property_name(name: &str) -> bool {
    matches!(name, "filter" | "-webkit-filter" | "backdrop-filter" | "-webkit-backdrop-filter" | "color-interpolation-filters" | "flood-color" | "flood-opacity" | "lighting-color")
}

fn is_transition_property_name(name: &str) -> bool {
    matches!(
        name,
        "transition"
            | "transition-property"
            | "transition-duration"
            | "transition-delay"
            | "transition-timing-function"
            | "transition-behavior"
            | "-webkit-transition"
            | "-webkit-transition-property"
            | "-webkit-transition-duration"
            | "-webkit-transition-delay"
            | "-webkit-transition-timing-function"
    )
}

fn is_animation_property_name(name: &str) -> bool {
    matches!(
        name,
        "animation"
            | "animation-name"
            | "animation-duration"
            | "animation-timing-function"
            | "animation-iteration-count"
            | "animation-direction"
            | "animation-play-state"
            | "animation-delay"
            | "animation-fill-mode"
            | "animation-composition"
            | "animation-timeline"
            | "animation-range"
            | "animation-range-start"
            | "animation-range-end"
            | "-webkit-animation"
            | "-webkit-animation-name"
            | "-webkit-animation-duration"
            | "-webkit-animation-timing-function"
            | "-webkit-animation-iteration-count"
            | "-webkit-animation-direction"
            | "-webkit-animation-play-state"
            | "-webkit-animation-delay"
            | "-webkit-animation-fill-mode"
    )
}

fn list_style_value_is_unsupported(property: &Property<'_>) -> bool {
    use lightningcss::properties::list::{CounterStyle, ListStyleType, PredefinedCounterStyle as P};
    let supported = |value: &ListStyleType<'_>| match value {
        ListStyleType::None => true,
        ListStyleType::CounterStyle(CounterStyle::Predefined(P::Disc | P::Circle | P::Square | P::Decimal | P::DecimalLeadingZero | P::LowerAlpha | P::LowerLatin | P::UpperAlpha | P::UpperLatin | P::LowerRoman | P::UpperRoman)) => true,
        ListStyleType::String(_) | ListStyleType::CounterStyle(CounterStyle::Name(_) | CounterStyle::Symbols { .. }) => false,
        ListStyleType::CounterStyle(CounterStyle::Predefined(_)) => false,
    };
    match property {
        Property::ListStyleType(value) => !supported(value),
        Property::ListStyle(value) => !supported(&value.list_style_type),
        _ => false,
    }
}

fn text_transform_value_is_unsupported(property: &Property<'_>) -> bool {
    match property {
        Property::TextTransform(value) => !value.other.is_empty(),
        Property::Unparsed(_) => true,
        _ => false,
    }
}

fn display_value_is_unsupported(property: &Property<'_>) -> bool {
    use lightningcss::properties::display::{Display, DisplayInside, DisplayKeyword, DisplayOutside};
    match property {
        Property::Display(Display::Keyword(DisplayKeyword::RubyBase | DisplayKeyword::RubyText | DisplayKeyword::RubyBaseContainer | DisplayKeyword::RubyTextContainer)) => true,
        Property::Display(Display::Pair(pair)) => {
            if pair.is_list_item {
                return !matches!((&pair.outside, &pair.inside), (DisplayOutside::Block, DisplayInside::Flow));
            }
            !matches!(
                (&pair.outside, &pair.inside),
                (DisplayOutside::Block | DisplayOutside::Inline, DisplayInside::Flow | DisplayInside::Table | DisplayInside::Grid | DisplayInside::Flex(_)) | (DisplayOutside::Block | DisplayOutside::Inline, DisplayInside::FlowRoot)
            )
        }
        _ => false,
    }
}

pub(crate) fn property_uses_gradient(property: &Property<'_>) -> bool {
    match property {
        Property::Background(layers) => layers.iter().any(|layer| image_uses_gradient(&layer.image)),
        Property::BackgroundImage(images) => images.iter().any(image_uses_gradient),
        Property::ListStyleImage(image) => image_uses_gradient(image),
        Property::ListStyle(list) => image_uses_gradient(&list.image),
        _ => false,
    }
}

pub(crate) fn property_uses_background_image(property: &Property<'_>) -> bool {
    match property {
        Property::Background(layers) => layers.iter().any(|layer| !matches!(layer.image, Image::None)),
        Property::BackgroundImage(images) => images.iter().any(|image| !matches!(image, Image::None)),
        _ => false,
    }
}

pub(crate) fn property_uses_unsupported_text_decoration_style(property: &Property<'_>) -> bool {
    use lightningcss::properties::text::{TextDecorationLine, TextDecorationStyle};
    let unsupported_line = |line: &TextDecorationLine| line.intersects(TextDecorationLine::Blink | TextDecorationLine::SpellingError | TextDecorationLine::GrammarError);
    match property {
        Property::TextDecoration(value, _) => matches!(value.style, TextDecorationStyle::Wavy) || unsupported_line(&value.line),
        Property::TextDecorationLine(value, _) => unsupported_line(value),
        Property::TextDecorationStyle(value, _) => matches!(value, TextDecorationStyle::Wavy),
        _ => false,
    }
}

pub(crate) fn property_uses_unsupported_outline_style(property: &Property<'_>) -> bool {
    use lightningcss::properties::border::LineStyle;
    use lightningcss::properties::outline::OutlineStyle;
    let unsupported = |style: &OutlineStyle| matches!(style, OutlineStyle::LineStyle(LineStyle::Double | LineStyle::Groove | LineStyle::Ridge | LineStyle::Inset | LineStyle::Outset));
    match property {
        Property::Outline(value) => unsupported(&value.style),
        Property::OutlineStyle(value) => unsupported(value),
        _ => false,
    }
}

fn image_uses_gradient(image: &Image<'_>) -> bool {
    match image {
        Image::Gradient(_) => true,
        Image::ImageSet(set) => set.options.iter().any(|option| image_uses_gradient(&option.image)),
        Image::None | Image::Url(_) => false,
    }
}

fn contains_gradient_function(value: &str) -> bool {
    value.as_bytes().windows(b"gradient(".len()).any(|window| window.eq_ignore_ascii_case(b"gradient("))
}

#[cfg(test)]
mod tests {
    use super::{declaration_capability, property_name_is_supported};
    use crate::{PropertyCapability, UnsupportedStyleFeature};

    #[test]
    fn capability_queries_do_not_require_parser_ast_types() {
        assert!(property_name_is_supported("WIDTH"));
        assert!(property_name_is_supported("all"));
        assert!(property_name_is_supported("--book-accent"));
        assert!(property_name_is_supported("border-spacing"));
        assert!(property_name_is_supported("text-overflow"));
        assert!(property_name_is_supported("text-decoration-thickness"));
        assert!(property_name_is_supported("outline"));
        assert!(property_name_is_supported("grid-template-columns"));
        assert!(property_name_is_supported("grid-row-gap"));
        assert!(property_name_is_supported("grid-column-gap"));
        assert!(property_name_is_supported("grid-gap"));
        assert!(property_name_is_supported("flex"));
        assert!(property_name_is_supported("order"));
        assert!(property_name_is_supported("direction"));
        assert!(property_name_is_supported("inline-size"));
        assert!(property_name_is_supported("margin-inline-start"));
        assert!(property_name_is_supported("padding-block"));
        assert!(property_name_is_supported("border-inline-start"));
        assert!(property_name_is_supported("border-block-color"));
        assert!(property_name_is_supported("border-radius"));
        assert!(property_name_is_supported("border-start-start-radius"));
        assert!(property_name_is_supported("contain"));
        assert!(property_name_is_supported("overflow"));
        assert!(property_name_is_supported("z-index"));
        assert!(property_name_is_supported("border-collapse"));
        assert!(property_name_is_supported("text-box-edge"));
        assert!(!property_name_is_supported("writing-mode"));
        assert!(!property_name_is_supported("inset-inline-start"));
        assert!(property_name_is_supported("break-inside"));
        assert!(property_name_is_supported("page-break-after"));
        assert!(property_name_is_supported("widows"));
        assert!(!property_name_is_supported("opacity"));
    }

    #[test]
    fn contain_capabilities_cover_only_the_implemented_size_subset() {
        assert_eq!(declaration_capability("contain", "none"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("contain", "size"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("contain", "layout size"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
        assert_eq!(declaration_capability("contain", "strict"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
    }

    #[test]
    fn gradient_values_are_not_renderer_capabilities() {
        assert_eq!(declaration_capability("background", "linear-gradient(red, blue)"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Gradient));
        assert_eq!(declaration_capability("background-image", "LINEAR-GRADIENT(red, blue)"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Gradient));
        assert_eq!(declaration_capability("background", "url('gradient.png') red"), PropertyCapability::Unsupported(UnsupportedStyleFeature::BackgroundImage));
    }

    #[test]
    fn border_image_family_has_a_specific_unsupported_capability() {
        for name in ["border-image", "border-image-source", "border-image-slice", "border-image-width", "border-image-outset", "border-image-repeat"] {
            assert_eq!(declaration_capability(name, "initial"), PropertyCapability::Unsupported(UnsupportedStyleFeature::BorderImage));
        }
    }

    #[test]
    fn box_shadow_has_a_specific_unsupported_capability() {
        assert_eq!(declaration_capability("box-shadow", "none"), PropertyCapability::Unsupported(UnsupportedStyleFeature::BoxShadow));
        assert_eq!(declaration_capability("-webkit-box-shadow", "none"), PropertyCapability::Unsupported(UnsupportedStyleFeature::BoxShadow));
        assert_eq!(declaration_capability("-moz-box-shadow", "none"), PropertyCapability::Unsupported(UnsupportedStyleFeature::BoxShadow));
    }

    #[test]
    fn filter_effect_family_has_a_specific_unsupported_capability() {
        for name in ["filter", "-webkit-filter", "backdrop-filter", "-webkit-backdrop-filter", "color-interpolation-filters", "flood-color", "flood-opacity", "lighting-color"] {
            assert_eq!(declaration_capability(name, "initial"), PropertyCapability::Unsupported(UnsupportedStyleFeature::FilterEffects));
        }
    }

    #[test]
    fn transition_family_has_a_specific_unsupported_capability() {
        for name in [
            "transition",
            "transition-property",
            "transition-duration",
            "transition-delay",
            "transition-timing-function",
            "transition-behavior",
            "-webkit-transition",
            "-webkit-transition-property",
            "-webkit-transition-duration",
            "-webkit-transition-delay",
            "-webkit-transition-timing-function",
        ] {
            assert_eq!(declaration_capability(name, "initial"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Transitions));
        }
    }

    #[test]
    fn paint_style_capabilities_reject_only_unrenderable_variants() {
        assert_eq!(declaration_capability("text-decoration", "underline dashed red"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("text-decoration", "underline wavy red"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
        assert_eq!(declaration_capability("text-decoration-line", "blink"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
        assert_eq!(declaration_capability("outline", "2px dotted red"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("outline", "2px double red"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
        assert_eq!(declaration_capability("outline-offset", "2px"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("visibility", "hidden"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("visibility", "collapse"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
    }

    #[test]
    fn animation_family_has_a_specific_unsupported_capability() {
        for name in [
            "animation",
            "animation-name",
            "animation-duration",
            "animation-timing-function",
            "animation-iteration-count",
            "animation-direction",
            "animation-play-state",
            "animation-delay",
            "animation-fill-mode",
            "animation-composition",
            "animation-timeline",
            "animation-range",
            "animation-range-start",
            "animation-range-end",
            "-webkit-animation",
        ] {
            assert_eq!(declaration_capability(name, "initial"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Animations));
        }
    }

    #[test]
    fn unsupported_display_algorithms_are_value_capabilities_not_parser_failures() {
        assert_eq!(declaration_capability("display", "flex"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("display", "grid"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("display", "flow-root"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("display", "contents"), PropertyCapability::Supported);
    }

    #[test]
    fn unsupported_list_marker_algorithms_are_value_capabilities() {
        assert_eq!(declaration_capability("list-style-type", "decimal"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("list-style-type", "symbols(cyclic 'x')"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
        assert_eq!(declaration_capability("list-style", "inside symbols(cyclic 'x')"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
    }

    #[test]
    fn unsupported_text_transform_algorithms_are_value_capabilities() {
        assert_eq!(declaration_capability("text-transform", "uppercase"), PropertyCapability::Supported);
        assert_eq!(declaration_capability("text-transform", "full-width"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
        assert_eq!(declaration_capability("text-transform", "math-auto"), PropertyCapability::Unsupported(UnsupportedStyleFeature::Value));
    }
}
