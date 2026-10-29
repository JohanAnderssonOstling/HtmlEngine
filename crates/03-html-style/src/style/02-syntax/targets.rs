//! Native shorthand targets shared by stylesheet preparation and inline cascade.
use crate::style::property_catalogue::PropertyKey;
use crate::style::syntax::capabilities::{declaration_is_custom_property, property_uses_gradient};
use crate::style::syntax::property_id::RendererPropertyId;
use lightningcss::properties::{Property, PropertyId};

pub(crate) fn append_property_targets(property: &Property<'_>, output: &mut Vec<PropertyKey>) {
    if declaration_is_custom_property(property) {
        return;
    }
    let id = property.property_id();
    if let Some((raw, _)) = RendererPropertyId::raw(property) {
        if raw == RendererPropertyId::TextBox {
            output.extend([PropertyKey::TextBoxTrim, PropertyKey::TextBoxEdge]);
        } else if let Some(canonical) = raw.gap_property() {
            append_targets(canonical, output);
        } else {
            output.push(raw.key());
        }
        return;
    }
    if matches!(id, PropertyId::Font) {
        output.extend([
            PropertyKey::FontKerning,
            PropertyKey::FontFeatureSettings,
            PropertyKey::FontVariantNumeric,
            PropertyKey::FontVariantLigatures,
        ]);
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
