//! Native shorthand targets shared by stylesheet preparation and inline cascade.
use crate::style::property_catalogue::PropertyKey;
use crate::style::syntax::capabilities::{declaration_is_custom_property, property_uses_gradient};
use crate::style::syntax::property_id::RendererPropertyId;
use lightningcss::properties::{Property, PropertyId};

pub(crate) const FONT_RESET_TARGETS: [PropertyKey; 4] = [
    PropertyKey::FontKerning,
    PropertyKey::FontFeatureSettings,
    PropertyKey::FontVariantNumeric,
    PropertyKey::FontVariantLigatures,
];

pub(crate) fn append_property_targets(property: &Property<'_>, output: &mut Vec<PropertyKey>) {
    if declaration_is_custom_property(property) {
        return;
    }
    let id = property.property_id();
    if let Some((raw, _)) = RendererPropertyId::raw(property) {
        if raw == RendererPropertyId::TextWrap {
            output.extend([PropertyKey::TextWrapMode, PropertyKey::TextWrapStyle]);
        } else if raw == RendererPropertyId::FontVariant {
            output.extend([
                PropertyKey::FontVariantCaps,
                PropertyKey::FontVariantNumeric,
                PropertyKey::FontVariantLigatures,
            ]);
        } else if raw == RendererPropertyId::FontSynthesis {
            output.extend([PropertyKey::FontSynthesisSmallCaps, PropertyKey::FontSynthesisStyle]);
        } else if raw == RendererPropertyId::TextBox {
            output.extend([PropertyKey::TextBoxTrim, PropertyKey::TextBoxEdge]);
        } else if let Some(canonical) = raw.native_property() {
            append_targets(canonical, output);
        } else {
            output.push(raw.key());
        }
        return;
    }
    if matches!(id, PropertyId::WhiteSpace) {
        output.extend([PropertyKey::WhiteSpaceCollapse, PropertyKey::TextWrapMode]);
        return;
    }
    if matches!(id, PropertyId::Font) {
        output.extend(FONT_RESET_TARGETS);
    }
    if property_uses_gradient(property) {
        output.push(PropertyKey::BackgroundImage);
    } else {
        append_targets(id, output);
    }
}

fn append_targets(id: PropertyId<'_>, output: &mut Vec<PropertyKey>) {
    if let Some(longhands) = id.longhands() {
        output.extend(
            longhands
                .iter()
                .filter_map(|id| PropertyKey::from_name(id.name())),
        );
    } else if let Some(key) = PropertyKey::from_name(id.name()) {
        output.push(key);
    }
}

pub(crate) fn compile_property_targets(property: &Property<'_>, targets: &mut Vec<PropertyKey>) {
    targets.clear();
    append_property_targets(property, targets);
}

/// Expand supported native longhands once, retaining the original declaration
/// as the atomic validation group, including components that lose the cascade.
pub(crate) fn expand_declaration<'css>(
    property: &Property<'css>,
    output: &mut Vec<Property<'css>>,
) -> Result<bool, ()> {
    let Some(longhands) = property.property_id().longhands() else {
        return Ok(false);
    };
    for id in longhands {
        if PropertyKey::from_name(id.name()).is_none() {
            continue;
        }
        output.push(
            property
                .longhand(&id)
                .or_else(|| match (property, &id) {
                    (Property::TextDecoration(value, _), PropertyId::TextDecorationThickness) => {
                        Some(Property::TextDecorationThickness(value.thickness.clone()))
                    }
                    _ => None,
                })
                .ok_or(())?,
        );
    }
    Ok(true)
}
