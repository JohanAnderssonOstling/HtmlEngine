//! Renderer capability classification.
//!
//! Parser acceptance and renderer support are deliberately separate facts. A
//! declaration such as `opacity: 0.5` can be valid CSS while still being a
//! feature this renderer does not implement.

use crate::style::property_catalogue::PropertyKey;
use crate::style::support::{PropertyCapability, UnsupportedStyleFeature};
use crate::style::syntax::property_id::RendererPropertyId;
use crate::style::syntax::substitution::{contains_function, needs_substitution};
#[cfg(test)]
use crate::style::syntax::value::parsed_property;
use crate::style::syntax::wide_keywords::token_list_css_wide_keyword;
use lightningcss::properties::custom::{Token, TokenOrValue};
use lightningcss::properties::display::Visibility;
use lightningcss::properties::position::Position;
use lightningcss::properties::{Property, PropertyId};
use lightningcss::values::image::Image;

pub(crate) fn declaration_is_custom_property(property: &Property<'_>) -> bool {
    match property {
        Property::Custom(value) => value.name.as_ref().starts_with("--"),
        Property::Unparsed(value) => value.property_id.name().starts_with("--"),
        _ => false,
    }
}

/// Target-independent eligibility that can be decided without serializing the
/// parsed value. Raw declarations retain the runtime fallback.
pub(crate) fn declaration_renderer_eligibility(property: &Property<'_>) -> Option<bool> {
    if declaration_is_custom_property(property) {
        return Some(false);
    }
    if matches!(property, Property::All(_))
        || matches!(property, Property::Unparsed(value) if token_list_css_wide_keyword(&value.value).is_some() || needs_substitution(&value.value))
        || matches!(property, Property::Custom(value) if token_list_css_wide_keyword(&value.value).is_some() || needs_substitution(&value.value))
    {
        return Some(true);
    }
    if property_uses_unsupported_text_decoration_style(property)
        || property_uses_unsupported_outline_style(property)
        || super::grid::native_value_is_unsupported(property)
    {
        return Some(false);
    }
    match property {
        Property::Unparsed(_) => Some(false),
        Property::Custom(_) => None,
        _ => Some(true),
    }
}

/// Complete predicate for inline, substituted, and raw declarations whose
/// eligibility cannot be cached without allocating during preparation.
pub(crate) fn declaration_is_renderer_eligible(property: &Property<'_>) -> bool {
    if let Some(eligible) = declaration_renderer_eligibility(property) {
        return eligible;
    }
    if let Some((RendererPropertyId::Contain, tokens)) = RendererPropertyId::raw(property) {
        return super::contain::parse_tokens(tokens).is_some();
    }
    parsed_declaration_capability(property.property_id().name(), Some(property))
        == PropertyCapability::Supported
}

pub(crate) fn is_supported_property_name(name: &str) -> bool {
    if name.starts_with('-') && !matches!(PropertyId::from(name), PropertyId::Custom(_)) {
        for prefix in ["-webkit-", "-moz-", "-ms-", "-o-"] {
            if let Some(name) = name.strip_prefix(prefix) {
                return is_supported_property_name(name);
            }
        }
    }
    name.starts_with("--")
        || PropertyKey::from_name(name).is_some_and(|key| key != PropertyKey::BackgroundImage)
}

pub fn property_name_is_supported(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    normalized.starts_with("--") || is_supported_property_name(&normalized)
}

/// Decide renderer support before doing any property-specific validation.
/// Unsupported features do not need a second implementation of their grammar.
#[cfg(test)]
fn declaration_capability(name: &str, value: &str) -> PropertyCapability {
    parsed_declaration_capability(
        name,
        parsed_property(PropertyId::from(name), value).as_ref(),
    )
}

pub(crate) fn parsed_declaration_capability(
    name: &str,
    parsed: Option<&Property<'_>>,
) -> PropertyCapability {
    let unsupported = if is_border_image_property_name(name) {
        Some(UnsupportedStyleFeature::BorderImage)
    } else if is_box_shadow_property_name(name) {
        Some(UnsupportedStyleFeature::BoxShadow)
    } else if is_filter_effect_property_name(name) {
        Some(UnsupportedStyleFeature::FilterEffects)
    } else if is_transition_property_name(name) {
        Some(UnsupportedStyleFeature::Transitions)
    } else if is_animation_property_name(name) {
        Some(UnsupportedStyleFeature::Animations)
    } else if !name.starts_with("--") && !is_supported_property_name(name) {
        Some(UnsupportedStyleFeature::Property)
    } else {
        None
    };
    if let Some(reason) = unsupported {
        return PropertyCapability::Unsupported(reason);
    }
    if name.starts_with("--") {
        return PropertyCapability::Supported;
    }
    if let Some((id, tokens)) = parsed.and_then(RendererPropertyId::raw) {
        if !needs_substitution(tokens)
            && token_list_css_wide_keyword(tokens).is_none()
            && !super::renderer_owned::tokens_are_supported(id, tokens)
        {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::Value);
        }

        if matches!(
            id,
            RendererPropertyId::Content
                | RendererPropertyId::CounterReset
                | RendererPropertyId::CounterIncrement
        ) && (tokens.0.iter().any(|token| {
            matches!(
                token,
                TokenOrValue::Url(_) | TokenOrValue::Token(Token::Delim('/'))
            )
        }) || contains_function(tokens, |name| name.eq_ignore_ascii_case("reversed")))
        {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::GeneratedContent);
        }
        if id == RendererPropertyId::Contain
            && super::contain::parse_tokens(tokens).is_some_and(|contain| !contain.fully_supported)
        {
            return PropertyCapability::Unsupported(UnsupportedStyleFeature::Value);
        }
    }
    if parsed.is_some_and(|property| {
        super::grid::native_value_is_unsupported(property)
            || display_value_is_unsupported(property)
            || list_style_value_is_unsupported(property)
            || (matches!(property, Property::TextTransform(_))
                && text_transform_value_is_unsupported(property))
            || property_uses_unsupported_text_decoration_style(property)
            || property_uses_unsupported_outline_style(property)
            || matches!(property, Property::Position(position)
                if !matches!(position, Position::Static
                    | Position::Relative
                    | Position::Absolute))
            || matches!(property, Property::Visibility(Visibility::Collapse))
    }) {
        return PropertyCapability::Unsupported(UnsupportedStyleFeature::Value);
    }
    if parsed.is_some_and(property_uses_gradient) {
        return PropertyCapability::Unsupported(UnsupportedStyleFeature::Gradient);
    }
    if matches!(name, "background" | "background-image")
        && parsed.is_some_and(property_uses_background_image)
    {
        return PropertyCapability::Unsupported(UnsupportedStyleFeature::BackgroundImage);
    }
    PropertyCapability::Supported
}

fn is_border_image_property_name(name: &str) -> bool {
    matches!(
        name,
        "border-image"
            | "border-image-source"
            | "border-image-slice"
            | "border-image-width"
            | "border-image-outset"
            | "border-image-repeat"
    )
}

fn is_box_shadow_property_name(name: &str) -> bool {
    matches!(
        name,
        "box-shadow" | "-webkit-box-shadow" | "-moz-box-shadow"
    )
}

fn is_filter_effect_property_name(name: &str) -> bool {
    matches!(
        name,
        "filter"
            | "-webkit-filter"
            | "backdrop-filter"
            | "-webkit-backdrop-filter"
            | "color-interpolation-filters"
            | "flood-color"
            | "flood-opacity"
            | "lighting-color"
    )
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
    use lightningcss::properties::list::{
        CounterStyle, ListStyleType, PredefinedCounterStyle as P,
    };
    let supported = |value: &ListStyleType<'_>| match value {
        ListStyleType::None => true,
        ListStyleType::CounterStyle(CounterStyle::Predefined(
            P::Disc
            | P::Circle
            | P::Square
            | P::Decimal
            | P::DecimalLeadingZero
            | P::LowerAlpha
            | P::LowerLatin
            | P::UpperAlpha
            | P::UpperLatin
            | P::LowerRoman
            | P::UpperRoman,
        )) => true,
        ListStyleType::String(_)
        | ListStyleType::CounterStyle(CounterStyle::Name(_) | CounterStyle::Symbols { .. }) => {
            false
        }
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
    use lightningcss::properties::display::{
        Display, DisplayInside, DisplayKeyword, DisplayOutside,
    };
    match property {
        Property::Display(Display::Keyword(
            DisplayKeyword::RubyBase
            | DisplayKeyword::RubyText
            | DisplayKeyword::RubyBaseContainer
            | DisplayKeyword::RubyTextContainer,
        )) => true,
        Property::Display(Display::Pair(pair)) => {
            if pair.is_list_item {
                return !matches!(
                    (&pair.outside, &pair.inside),
                    (DisplayOutside::Block, DisplayInside::Flow)
                );
            }
            !matches!(
                (&pair.outside, &pair.inside),
                (
                    DisplayOutside::Block | DisplayOutside::Inline,
                    DisplayInside::Flow
                        | DisplayInside::Table
                        | DisplayInside::Grid
                        | DisplayInside::Flex(_)
                ) | (
                    DisplayOutside::Block | DisplayOutside::Inline,
                    DisplayInside::FlowRoot
                )
            )
        }
        _ => false,
    }
}

pub(crate) fn property_uses_gradient(property: &Property<'_>) -> bool {
    match property {
        Property::Background(layers) => {
            layers.iter().any(|layer| image_uses_gradient(&layer.image))
        }
        Property::BackgroundImage(images) => images.iter().any(image_uses_gradient),
        Property::ListStyleImage(image) => image_uses_gradient(image),
        Property::ListStyle(list) => image_uses_gradient(&list.image),
        _ => false,
    }
}

pub(crate) fn property_uses_background_image(property: &Property<'_>) -> bool {
    match property {
        Property::Background(layers) => layers
            .iter()
            .any(|layer| !matches!(layer.image, Image::None)),
        Property::BackgroundImage(images) => {
            images.iter().any(|image| !matches!(image, Image::None))
        }
        _ => false,
    }
}

pub(crate) fn property_uses_unsupported_text_decoration_style(property: &Property<'_>) -> bool {
    use lightningcss::properties::text::{TextDecorationLine, TextDecorationStyle};
    let unsupported_line = |line: &TextDecorationLine| {
        line.intersects(
            TextDecorationLine::Blink
                | TextDecorationLine::SpellingError
                | TextDecorationLine::GrammarError,
        )
    };
    match property {
        Property::TextDecoration(value, _) => {
            matches!(value.style, TextDecorationStyle::Wavy) || unsupported_line(&value.line)
        }
        Property::TextDecorationLine(value, _) => unsupported_line(value),
        Property::TextDecorationStyle(value, _) => matches!(value, TextDecorationStyle::Wavy),
        _ => false,
    }
}

pub(crate) fn property_uses_unsupported_outline_style(property: &Property<'_>) -> bool {
    use lightningcss::properties::border::LineStyle;
    use lightningcss::properties::outline::OutlineStyle;
    let unsupported = |style: &OutlineStyle| {
        matches!(
            style,
            OutlineStyle::LineStyle(
                LineStyle::Double
                    | LineStyle::Groove
                    | LineStyle::Ridge
                    | LineStyle::Inset
                    | LineStyle::Outset
            )
        )
    };
    match property {
        Property::Outline(value) => unsupported(&value.style),
        Property::OutlineStyle(value) => unsupported(value),
        _ => false,
    }
}

fn image_uses_gradient(image: &Image<'_>) -> bool {
    match image {
        Image::Gradient(_) => true,
        Image::ImageSet(set) => set
            .options
            .iter()
            .any(|option| image_uses_gradient(&option.image)),
        Image::None | Image::Url(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        declaration_capability, declaration_is_renderer_eligible, declaration_renderer_eligibility,
        property_name_is_supported,
    };
    use crate::{PropertyCapability, UnsupportedStyleFeature};
    use lightningcss::properties::{Property, PropertyId};
    use lightningcss::stylesheet::ParserOptions;

    #[test]
    fn renderer_eligibility_caches_only_allocation_free_decisions() {
        let color =
            Property::parse_string(PropertyId::from("color"), "red", ParserOptions::default())
                .unwrap();
        let custom = Property::parse_string(
            PropertyId::from("--accent"),
            "red",
            ParserOptions::default(),
        )
        .unwrap();
        let content = Property::parse_string(
            PropertyId::from("content"),
            "url('marker.svg')",
            ParserOptions::default(),
        )
        .unwrap();

        assert_eq!(declaration_renderer_eligibility(&color), Some(true));
        assert_eq!(declaration_renderer_eligibility(&custom), Some(false));
        assert_eq!(declaration_renderer_eligibility(&content), None);
        assert!(!declaration_is_renderer_eligible(&content));
    }

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
        assert_eq!(
            declaration_capability("contain", "none"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("contain", "size"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("contain", "layout size"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        );
        assert_eq!(
            declaration_capability("contain", "strict"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        );
    }

    #[test]
    fn gradient_values_are_not_renderer_capabilities() {
        assert_eq!(
            declaration_capability("background", "linear-gradient(red, blue)"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Gradient)
        );
        assert_eq!(
            declaration_capability("background-image", "LINEAR-GRADIENT(red, blue)"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Property)
        );
        assert_eq!(
            declaration_capability("background", "url('gradient.png') red"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::BackgroundImage)
        );
    }

    #[test]
    fn border_image_family_has_a_specific_unsupported_capability() {
        for name in [
            "border-image",
            "border-image-source",
            "border-image-slice",
            "border-image-width",
            "border-image-outset",
            "border-image-repeat",
        ] {
            assert_eq!(
                declaration_capability(name, "initial"),
                PropertyCapability::Unsupported(UnsupportedStyleFeature::BorderImage)
            );
        }
    }

    #[test]
    fn box_shadow_has_a_specific_unsupported_capability() {
        assert_eq!(
            declaration_capability("box-shadow", "none"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::BoxShadow)
        );
        assert_eq!(
            declaration_capability("-webkit-box-shadow", "none"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::BoxShadow)
        );
        assert_eq!(
            declaration_capability("-moz-box-shadow", "none"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::BoxShadow)
        );
    }

    #[test]
    fn filter_effect_family_has_a_specific_unsupported_capability() {
        for name in [
            "filter",
            "-webkit-filter",
            "backdrop-filter",
            "-webkit-backdrop-filter",
            "color-interpolation-filters",
            "flood-color",
            "flood-opacity",
            "lighting-color",
        ] {
            assert_eq!(
                declaration_capability(name, "initial"),
                PropertyCapability::Unsupported(UnsupportedStyleFeature::FilterEffects)
            );
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
            assert_eq!(
                declaration_capability(name, "initial"),
                PropertyCapability::Unsupported(UnsupportedStyleFeature::Transitions)
            );
        }
    }

    #[test]
    fn paint_style_capabilities_reject_only_unrenderable_variants() {
        assert_eq!(
            declaration_capability("text-decoration", "underline dashed red"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("text-decoration", "underline wavy red"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        );
        assert_eq!(
            declaration_capability("text-decoration-line", "blink"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        );
        assert_eq!(
            declaration_capability("outline", "2px dotted red"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("outline", "2px double red"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        );
        assert_eq!(
            declaration_capability("outline-offset", "2px"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("visibility", "hidden"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("visibility", "collapse"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        );
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
            assert_eq!(
                declaration_capability(name, "initial"),
                PropertyCapability::Unsupported(UnsupportedStyleFeature::Animations)
            );
        }
    }

    #[test]
    fn unsupported_display_algorithms_are_value_capabilities_not_parser_failures() {
        assert_eq!(
            declaration_capability("display", "flex"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("display", "grid"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("display", "flow-root"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("display", "contents"),
            PropertyCapability::Supported
        );
    }

    #[test]
    fn unsupported_list_marker_algorithms_are_value_capabilities() {
        assert_eq!(
            declaration_capability("list-style-type", "decimal"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("list-style-type", "symbols(cyclic 'x')"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        );
        assert_eq!(
            declaration_capability("list-style", "inside symbols(cyclic 'x')"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        );
    }

    #[test]
    fn unsupported_text_transform_algorithms_are_value_capabilities() {
        assert_eq!(
            declaration_capability("text-transform", "uppercase"),
            PropertyCapability::Supported
        );
        assert_eq!(
            declaration_capability("text-transform", "full-width"),
            PropertyCapability::Unsupported(UnsupportedStyleFeature::Value)
        );
        assert_eq!(
            declaration_capability("text-transform", "math-auto"),
            PropertyCapability::Supported
        );
    }
}
