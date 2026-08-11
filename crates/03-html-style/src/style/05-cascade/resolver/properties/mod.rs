//! Central property routing and property-family application.

use super::*;

mod box_model;
mod generated;
mod layout;
mod legacy;
mod misc;
mod paint;
mod raw;
mod text;
mod typography;
mod unparsed;

use generated::*;
use legacy::*;
use raw::*;
use unparsed::apply_unparsed_property;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ApplyResult {
    Applied,
    Invalid,
    Unhandled,
}

pub(super) struct PropertyContext<'a, 'doc> {
    pub(super) doc: &'a ResolutionDocument<'doc>,
    pub(super) styles: &'a mut ComputedStylesBuilder,
    pub(super) style: &'a mut WorkingStyle,
    pub(super) resolved_root_font_size: f32,
    pub(super) parent_font_size: f32,
    pub(super) parent_font_weight: u16,
    pub(super) parent_color: u32,
    pub(super) environment: crate::MediaEnvironment,
}

pub(super) fn apply_property_in_phase<'a>(
    doc: &Document,
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    property: &Property<'a>,
    parent_font_size: f32,
    environment: crate::MediaEnvironment,
    phase: CascadePhase,
    parent: &ParentStyle,
) {
    set_line_height_resolution_bases(doc, styles, property, style, parent);
    // Preserve whether the winning background establishes an image layer even
    // while image painting is unsupported. Canvas background propagation
    // depends on the computed image being `none`, not merely on whether a
    // drawable image fragment was produced.
    if crate::style::syntax::capabilities::property_uses_background_image(property) {
        if matches!(phase, CascadePhase::Remaining) {
            style.background.background_image_present = true;
        }
        // A background shorthand's color remains independently paintable
        // underneath an image that this renderer cannot draw. Keep rejecting
        // standalone background-image declarations after recording their
        // computed presence, but allow the shorthand's supported color through.
        if !matches!(property, Property::Background(_)) {
            return;
        }
    }
    // Parsing a gradient does not imply that this renderer can paint it. Skip
    // the entire declaration so an earlier usable fallback remains active.
    if crate::style::syntax::capabilities::property_uses_gradient(property) {
        return;
    }
    // Unsupported paint styles are rejected as whole declarations so their
    // shorthand color/width/line components cannot leak into computed style.
    if crate::style::syntax::capabilities::property_uses_unsupported_text_decoration_style(property)
        || crate::style::syntax::capabilities::property_uses_unsupported_outline_style(property)
    {
        return;
    }
    if try_apply_legacy_grid_gap_alias(doc, style, property, parent) {
        return;
    }
    // CSS-wide keywords copy already-computed values, so applying them in both
    // phases is idempotent and keeps cascade order within each phase.
    if let Property::All(keyword) = property {
        match keyword {
            CSSWideKeyword::Initial => {
                let basis = ParentStyle::initial(doc.root_font_size()).to_working_style();
                apply_all_from_basis(style, &basis, phase);
            }
            CSSWideKeyword::Inherit => {
                let basis = parent.to_working_style();
                apply_all_from_basis(style, &basis, phase);
            }
            CSSWideKeyword::Unset => {
                let basis = parent.unset_working_style(doc.root_font_size());
                apply_all_from_basis(style, &basis, phase);
            }
            // Winner selection consumes rollback keywords before conversion.
            CSSWideKeyword::Revert | CSSWideKeyword::RevertLayer => {}
        }
        return;
    }
    if try_apply_css_wide_keyword(style, property, parent, doc.root_font_size(), phase) {
        return;
    }
    match phase {
        CascadePhase::Prerequisites => match property {
            Property::Direction(direction) => {
                use lightningcss::properties::text::Direction;
                style.text.direction = match direction {
                    Direction::Ltr => TextDirection::Ltr,
                    Direction::Rtl => TextDirection::Rtl,
                };
                resolve_logical_text_alignments(&mut style.text);
            }
            Property::FontSize(_) | Property::LineHeight(_) | Property::Color(_) => {
                let _ = apply_property(
                    doc,
                    styles,
                    style,
                    property,
                    parent_font_size,
                    parent.font.font_weight,
                    parent.text.color,
                    environment,
                );
            }
            Property::Font(font) => {
                let Some(values) = checked_font_shorthand(
                    font,
                    parent_font_size,
                    parent.font.font_weight,
                    doc.root_font_size(),
                    environment,
                ) else {
                    return;
                };
                style.font.font_size = values.font_size;
                style.font.font_size_x_height_px = 0.0;
                style.font.font_size_ch_advance_px = 0.0;
                style.font.font_size_cap_height_px = 0.0;
                style.font.font_size_root_ch = 0.0;
                style.font.font_size_root_cap_height = 0.0;
                style.font.font_size_root_line_height = 0.0;
                style.line_height_spec = Some(font.line_height.clone());
                style.text.line_height_number = values.line_height_number;
                style.text.line_height = values.line_height;
                style.text.line_height_x_height_px = values.line_height_x_height_px;
                style.text.line_height_normal = values.line_height_normal;
            }
            _ => {}
        },
        CascadePhase::Remaining => match property {
            // Already final from the first pass.
            Property::Direction(_)
            | Property::FontSize(_)
            | Property::LineHeight(_)
            | Property::Color(_) => {}
            // Apply the shorthand but keep the font size resolved by the first
            // pass, re-binding the shorthand's line-height to it.
            Property::Font(_) => {
                let final_font_size = (
                    style.font.font_size,
                    style.font.font_size_x_height_px,
                    style.font.font_size_ch_advance_px,
                    style.font.font_size_cap_height_px,
                    style.font.font_size_root_ch,
                    style.font.font_size_root_cap_height,
                    style.font.font_size_root_line_height,
                );
                let final_line_height = (
                    style.line_height_spec.clone(),
                    style.text.line_height_number,
                    style.text.line_height,
                    style.text.line_height_x_height_px,
                    style.text.line_height_normal,
                );
                let _ = apply_property(
                    doc,
                    styles,
                    style,
                    property,
                    parent_font_size,
                    parent.font.font_weight,
                    parent.text.color,
                    environment,
                );
                style.font.font_size = final_font_size.0;
                style.font.font_size_x_height_px = final_font_size.1;
                style.font.font_size_ch_advance_px = final_font_size.2;
                style.font.font_size_cap_height_px = final_font_size.3;
                style.font.font_size_root_ch = final_font_size.4;
                style.font.font_size_root_cap_height = final_font_size.5;
                style.font.font_size_root_line_height = final_font_size.6;
                style.line_height_spec = final_line_height.0;
                style.text.line_height_number = final_line_height.1;
                style.text.line_height = final_line_height.2;
                style.text.line_height_x_height_px = final_line_height.3;
                style.text.line_height_normal = final_line_height.4;
                if let Some(spec) = style.line_height_spec.as_ref() {
                    if let Some((line_height, x_height_px)) = checked_line_height_components(
                        spec,
                        final_font_size.0,
                        doc.root_font_size(),
                    ) {
                        style.text.line_height = line_height;
                        style.text.line_height_x_height_px = x_height_px;
                    }
                }
            }
            _ => {
                let _ = apply_property(
                    doc,
                    styles,
                    style,
                    property,
                    parent_font_size,
                    parent.font.font_weight,
                    parent.text.color,
                    environment,
                );
            }
        },
    }
}

fn apply_property<'a>(
    doc: &Document,
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    property: &Property<'a>,
    parent_font_size: f32,
    parent_font_weight: u16,
    parent_color: u32,
    environment: crate::MediaEnvironment,
) -> ApplyResult {
    let resolved_root_font_size = root_font_size_for_resolution(doc, styles);
    let resolved_document = ResolutionDocument {
        document: doc,
        root_font_size: resolved_root_font_size,
    };
    let doc = &resolved_document;
    if apply_unparsed_property(styles, style, property) {
        return ApplyResult::Applied;
    }
    let mut context = PropertyContext {
        doc,
        styles,
        style,
        resolved_root_font_size,
        parent_font_size,
        parent_font_weight,
        parent_color,
        environment,
    };
    for result in [
        typography::apply(&mut context, property),
        text::apply(&mut context, property),
    ] {
        if result != ApplyResult::Unhandled {
            return result;
        }
    }
    if paint::apply(&mut context, property) {
        return ApplyResult::Applied;
    }
    for result in [
        box_model::apply(&mut context, property),
        layout::apply(&mut context, property),
    ] {
        if result != ApplyResult::Unhandled {
            return result;
        }
    }
    if misc::apply(&mut context, property) {
        return ApplyResult::Applied;
    }
    ApplyResult::Unhandled
}

pub(super) fn property_is_computable(
    doc: &Document,
    styles: &mut ComputedStylesBuilder,
    base: &WorkingStyle,
    property: &Property<'_>,
    parent_font_size: f32,
    parent: &ParentStyle,
    environment: crate::MediaEnvironment,
) -> bool {
    let mut scratch = base.clone();
    set_line_height_resolution_bases(doc, styles, property, &scratch, parent);
    apply_property(
        doc,
        styles,
        &mut scratch,
        property,
        parent_font_size,
        parent.font.font_weight,
        parent.text.color,
        environment,
    ) != ApplyResult::Invalid
}
