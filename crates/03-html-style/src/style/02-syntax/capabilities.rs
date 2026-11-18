//! Renderer capability classification.
//!
//! Parser acceptance and renderer support are deliberately separate facts. A
//! declaration such as `opacity: 0.5` can be valid CSS while still being a
//! feature this renderer does not implement.

use crate::style::cascade::resolver::prepared_native::PreparedDeclarationValues;
use crate::style::property_catalogue::PropertyKey;
use crate::style::support::{PreparationError, UnsupportedStyleFeature};
#[cfg(test)]
use crate::style::support::{DeclarationSupport, PropertyCapability};
use crate::style::cascade::resolver::prepared_native::PreparedValue;
#[cfg(test)]
use crate::style::syntax::value::parsed_property;
use lightningcss::properties::{Property, PropertyId};
use lightningcss::values::image::Image;

pub(crate) fn declaration_is_custom_property(property: &Property<'_>) -> bool {
    match property {
        Property::Custom(value) => value.name.as_ref().starts_with("--"),
        Property::Unparsed(value) => value.property_id.name().starts_with("--"),
        _ => false,
    }
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
    prepare_support(
        name,
        parsed_property(PropertyId::from(name), value).as_ref(),
    )
    .map_or_else(
        |error| DeclarationSupport::from(error).capability,
        |()| PropertyCapability::Supported,
    )
}

pub(crate) fn prepare_support(
    name: &str,
    parsed: Option<&Property<'_>>,
) -> Result<(), PreparationError> {
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
        return Err(PreparationError::Unsupported(reason));
    }
    let Some(property) = parsed else {
        return Ok(());
    };
    if name.starts_with("--") {
        return Ok(());
    }
    let prepared = PreparedDeclarationValues::prepare(property, &mut Vec::new())?;
    if prepared
        .values
        .iter()
        .any(|(_, value)| matches!(value, PreparedValue::Contain(value) if !value.fully_supported))
    {
        return Err(PreparationError::Unsupported(
            UnsupportedStyleFeature::Value,
        ));
    }
    if property_uses_gradient(property) {
        return Err(PreparationError::Unsupported(
            UnsupportedStyleFeature::Gradient,
        ));
    }
    if matches!(name, "background" | "background-image") && property_uses_background_image(property)
    {
        return Err(PreparationError::Unsupported(
            UnsupportedStyleFeature::BackgroundImage,
        ));
    }
    Ok(())
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
    use super::{declaration_capability, property_name_is_supported};
    use crate::style::cascade::resolver::prepared_native::PreparedValue;
    use crate::{PropertyCapability, UnsupportedStyleFeature};
    use lightningcss::properties::{Property, PropertyId};
    use lightningcss::stylesheet::ParserOptions;

    #[test]
    fn renderer_preparation_rejects_unsupported_raw_values() {
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

        assert!(PreparedValue::prepare_renderer(&color, |_, _| {}).is_ok());
        assert!(PreparedValue::prepare_renderer(&custom, |_, _| {}).is_err());
        assert!(PreparedValue::prepare_renderer(&content, |_, _| {}).is_err());
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
