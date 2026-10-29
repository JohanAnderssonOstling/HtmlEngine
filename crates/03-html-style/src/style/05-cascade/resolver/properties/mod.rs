//! Central property routing and property-family application.

use super::context::{ApplyResult, PropertyContext, ResolutionContext, ResolvedDeclaration};
use super::state::WorkingStyle;
use super::traversal::root_font_size_for_resolution;
use super::values::fonts::checked_font_shorthand;
use super::values::{rebind_line_height, resolve_logical_text_alignments};
use crate::style::property_catalogue::PropertyKey;
use crate::style::syntax::capabilities;
use crate::style::syntax::property_id::RendererPropertyId;
use html_dom::Document;
use html_style_model::{ComputedStylesBuilder, InheritedText, TextDirection};
use lightningcss::properties::{Property, PropertyId};

mod box_model;
mod generated;
mod layout;
mod misc;
mod paint;
mod text;
mod typography;
mod unparsed;

/// Expand supported native longhands once, retaining the original declaration
/// as the atomic validation group, including components that lose the cascade.
pub(super) fn expand_declaration<'css>(
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

/// Compute the complete declaration atomically from its cached native values.
pub(super) fn compute_declaration<'css>(
    doc: &Document,
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    property: &Property<'css>,
    resolved: &ResolvedDeclaration<'css>,
    parent: &WorkingStyle,
    resolution: &ResolutionContext,
) -> Result<(), ()> {
    let property = resolved.value.as_deref().unwrap_or(property);
    if let Property::Font(font) = property {
        set_line_height_resolution_bases(doc, styles, true, style, parent, resolution);
        checked_font_shorthand(
            font,
            parent.font.font_size,
            parent.font.font_weight,
            doc.root_font_size(),
            resolution,
        )
        .ok_or(())?;
        // The font shorthand resets these renderer-owned features to auto/normal.
        style.font.font_kerning_features.clear();
        style.font.font_feature_settings.clear();
        style.font.font_variant_numeric_features.clear();
        style.font.font_variant_ligature_features.clear();
    }
    let longhands = if resolved.expanded {
        resolved.longhands.as_slice()
    } else {
        std::slice::from_ref(property)
    };
    for property in longhands {
        apply_longhand(doc, styles, style, property, parent, resolution)?;
    }
    Ok(())
}

fn apply_longhand(
    doc: &Document,
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    property: &Property<'_>,
    parent: &WorkingStyle,
    resolution: &ResolutionContext,
) -> Result<(), ()> {
    let parent_relative = matches!(
        property.property_id(),
        PropertyId::FontSize | PropertyId::LineHeight | PropertyId::Font
    );
    set_line_height_resolution_bases(doc, styles, parent_relative, style, parent, resolution);
    if let Property::Direction(direction) = property {
        use lightningcss::properties::text::Direction;
        style.text.direction = match direction {
            Direction::Ltr => TextDirection::Ltr,
            Direction::Rtl => TextDirection::Rtl,
        };
        resolve_logical_text_alignments(&mut style.text);
        return Ok(());
    }
    // Keep image presence even when the renderer cannot paint the image.
    if capabilities::property_uses_background_image(property) {
        style.background.background_image_present = true;
        return Ok(());
    }
    let root_font_size = root_font_size_for_resolution(doc, styles);
    if let Some((id, tokens)) = RendererPropertyId::raw(property) {
        return unparsed::apply(styles, style, id, tokens, root_font_size, resolution).ok_or(());
    }
    let mut context = PropertyContext {
        styles,
        style,
        resolution,
        root_font_size,
        parent,
    };
    for apply in [
        typography::apply,
        text::apply,
        paint::apply,
        box_model::apply,
        layout::apply,
        misc::apply,
    ] {
        match apply(&mut context, property) {
            ApplyResult::Applied => return Ok(()),
            ApplyResult::Invalid => return Err(()),
            ApplyResult::Unhandled => {}
        }
    }
    Ok(())
}

/// Rebind a selected line height after the final font-size winner is known.
pub(super) fn finish_prerequisites(
    doc: &Document,
    styles: &ComputedStylesBuilder,
    style: &mut WorkingStyle,
    parent: &WorkingStyle,
    resolution: &ResolutionContext,
) {
    set_line_height_resolution_bases(doc, styles, true, style, parent, resolution);
    rebind_line_height(
        style,
        || root_font_size_for_resolution(doc, styles),
        resolution,
    );
}

fn computed_line_height(font_size: f32, text: &InheritedText) -> Option<f32> {
    // Keep `lh` aligned with the renderer's established used-value behavior
    // for `normal` (the same 1.2 multiplier used by shaping and inline layout).
    let value = if text.line_height_normal {
        font_size * 1.2
    } else {
        text.line_height
    };
    value.is_finite().then_some(value)
}

/// CSS Values makes `lh` in font-size and line-height refer to the parent's
/// computed line height, avoiding a dependency cycle. Other properties use
/// the element's own (already prerequisite-resolved) computed line height.
fn set_line_height_resolution_bases(
    doc: &Document,
    styles: &ComputedStylesBuilder,
    parent_relative: bool,
    style: &WorkingStyle,
    parent: &WorkingStyle,
    resolution: &ResolutionContext,
) {
    let basis = if parent_relative { parent } else { style };
    let line_height = computed_line_height(basis.font.font_size, &basis.text);
    resolution.line_height.set(line_height);

    let root_line_height = stored_root_line_height(doc, styles).or_else(|| {
        // While resolving the root itself, font-size and line-height must
        // use their initial basis to avoid a cycle. Other root properties
        // can use the prerequisite-resolved root line height.
        if parent_relative {
            computed_line_height(doc.root_font_size(), &InheritedText::default())
        } else {
            line_height
        }
    });
    resolution.root_line_height.set(root_line_height);
}

fn stored_root_line_height(doc: &Document, styles: &ComputedStylesBuilder) -> Option<f32> {
    let indices = styles.style_for_node(doc.dom_root()?)?;
    computed_line_height(
        styles.font_style(indices)?.font_size,
        styles.text_style(indices)?,
    )
}
