//! Central property routing and property-family application.

use super::context::{CascadePhase, PropertyContext, ResolutionContext};
use super::prepared_native::PreparedLonghand;
use super::state::{ParentStyle, WorkingStyle};
use super::traversal::root_font_size_for_resolution;
use super::values::rebind_line_height;
use crate::style::property_catalogue::PropertyKey;
use html_dom::Document;
use html_style_model::{ComputedStylesBuilder, InheritedText};

pub(super) mod box_model;
mod generated;

/// Apply only the selected longhands, preserving shorthand reset semantics.
pub(super) fn compute_declaration(
    doc: &Document,
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    values: &[PreparedLonghand],
    targets: &[PropertyKey],
    parent: &ParentStyle,
    resolution: &mut ResolutionContext,
    phase: CascadePhase,
) {
    let selected = |key: PropertyKey| targets.contains(&key) && key.phase() == phase;
    let root_font_size = root_font_size_for_resolution(doc, styles);
    for (key, value) in values {
        if !selected(*key) {
            continue;
        }
        set_line_height_resolution_bases(
            doc,
            styles,
            matches!(key, PropertyKey::FontSize | PropertyKey::LineHeight),
            style,
            parent,
            resolution,
        );
        let mut context = PropertyContext {
            is_root: doc
                .dom_root()
                .is_some_and(|root| styles.style_for_node(root).is_none()),
            styles,
            style,
            resolution,
            root_font_size,
            parent,
        };
        value.apply(*key, &mut context);
    }
}

/// Rebind a selected line height after the final font-size winner is known.
pub(super) fn finish_prerequisites(
    doc: &Document,
    styles: &ComputedStylesBuilder,
    style: &mut WorkingStyle,
    parent: &ParentStyle,
    resolution: &mut ResolutionContext,
) {
    set_line_height_resolution_bases(doc, styles, true, style, parent, resolution);
    rebind_line_height(
        style,
        styles,
        || root_font_size_for_resolution(doc, styles),
        resolution,
    );
}

fn computed_line_height(font_size: f32, text: &InheritedText) -> Option<f32> {
    // Keep `lh` aligned with the renderer's established used-value behavior
    // for `normal` (the same 1.2 multiplier used by shaping and inline layout).
    let value = if text.line_height_normal {
        crate::style::syntax::prepared_length::finite(f64::from(font_size) * 1.2)
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
    parent: &ParentStyle,
    resolution: &mut ResolutionContext,
) {
    let line_height = if parent_relative {
        parent.with(styles, |parent| {
            computed_line_height(parent.font.font_size, parent.text)
        })
    } else {
        computed_line_height(style.font(styles).font_size, style.text(styles))
    };
    resolution.line_height = line_height;

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
    resolution.root_line_height = root_line_height;
}

fn stored_root_line_height(doc: &Document, styles: &ComputedStylesBuilder) -> Option<f32> {
    let indices = styles.style_for_node(doc.dom_root()?)?;
    computed_line_height(
        styles.font_style(indices)?.font_size,
        styles.text_style(indices)?,
    )
}
