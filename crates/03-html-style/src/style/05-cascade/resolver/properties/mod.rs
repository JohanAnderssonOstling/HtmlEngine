//! Central property routing and property-family application.

use super::context::{
    ApplyResult, CascadePhase, PropertyContext, ResolutionContext, ResolvedDeclaration,
};
use super::state::{ParentStyle, WorkingStyle};
use super::traversal::root_font_size_for_resolution;
use super::values::{rebind_line_height, resolve_logical_text_alignments};
use crate::style::cascade::resolver::prepared_native::PreparedNativeValue;
use crate::style::property_catalogue::PropertyKey;
use crate::style::syntax::capabilities;
use crate::style::syntax::prepared_renderer::PreparedRendererValue;
use crate::style::syntax::property_id::RendererPropertyId;
use html_dom::Document;
use html_style_model::{ComputedStylesBuilder, InheritedText, TextDirection};
use lightningcss::properties::{Property, PropertyId};

pub(super) mod box_model;
mod generated;
mod layout;
mod misc;
mod paint;
mod text;
mod typography;
mod unparsed;

/// Apply only the selected longhands, preserving shorthand reset semantics.
pub(super) fn compute_declaration<'css>(
    doc: &Document,
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    property: &Property<'css>,
    resolved: &ResolvedDeclaration<'css>,
    prepared_longhands: Option<&[Property<'css>]>,
    parent: &ParentStyle,
    resolution: &mut ResolutionContext,
    prepared_renderer: Option<&PreparedRendererValue>,
    targets: &[PropertyKey],
    phase: CascadePhase,
    native_values: &[Option<PreparedNativeValue>],
) {
    let selected = |key| targets.contains(&key) && key.phase() == phase;
    let property = resolved.property(property);
    if matches!(property, Property::Font(_)) {
        if selected(PropertyKey::FontKerning) {
            style.font_mut(styles).font_kerning_features.clear();
        }
        if selected(PropertyKey::FontFeatureSettings) {
            style.font_mut(styles).font_feature_settings.clear();
        }
        if selected(PropertyKey::FontVariantNumeric) {
            style.font_mut(styles).font_variant_numeric_features.clear();
        }
        if selected(PropertyKey::FontVariantLigatures) {
            style
                .font_mut(styles)
                .font_variant_ligature_features
                .clear();
        }
    }
    let longhands = if let Some(longhands) = prepared_longhands {
        longhands
    } else if resolved.expanded() {
        resolved.longhands.as_slice()
    } else {
        std::slice::from_ref(property)
    };
    for (index, property) in longhands.iter().enumerate() {
        if let Property::WhiteSpace(value) = property {
            use html_style_model::WhiteSpace;
            use lightningcss::properties::text::WhiteSpace as CssWhiteSpace;
            let value = match value {
                CssWhiteSpace::Normal => WhiteSpace::Normal,
                CssWhiteSpace::Pre => WhiteSpace::Pre,
                CssWhiteSpace::NoWrap => WhiteSpace::NoWrap,
                CssWhiteSpace::PreWrap => WhiteSpace::PreWrap,
                CssWhiteSpace::PreLine => WhiteSpace::PreLine,
                CssWhiteSpace::BreakSpaces => WhiteSpace::BreakSpaces,
            };
            if selected(PropertyKey::WhiteSpaceCollapse) {
                style.text_mut(styles).white_space =
                    value.with_wrapping(style.text(styles).white_space.allows_wrap());
            }
            if selected(PropertyKey::TextWrapMode) {
                style.text_mut(styles).white_space = style
                    .text(styles)
                    .white_space
                    .with_wrapping(value.allows_wrap());
            }
            continue;
        }
        if RendererPropertyId::raw(property).is_none()
            && !PropertyKey::from_name(property.property_id().name()).is_some_and(selected)
        {
            continue;
        }
        apply_longhand(
            doc,
            styles,
            style,
            property,
            parent,
            resolution,
            prepared_renderer,
            &selected,
            native_values.get(index).and_then(Option::as_ref),
        );
    }
}

fn apply_longhand(
    doc: &Document,
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    property: &Property<'_>,
    parent: &ParentStyle,
    resolution: &mut ResolutionContext,
    prepared_renderer: Option<&PreparedRendererValue>,
    selected: &impl Fn(PropertyKey) -> bool,
    prepared_native: Option<&PreparedNativeValue>,
) {
    let parent_relative = matches!(
        property.property_id(),
        PropertyId::FontSize | PropertyId::LineHeight | PropertyId::Font
    );
    set_line_height_resolution_bases(doc, styles, parent_relative, style, parent, resolution);
    if let Property::Direction(direction) = property {
        use lightningcss::properties::text::Direction;
        style.text_mut(styles).direction = match direction {
            Direction::Ltr => TextDirection::Ltr,
            Direction::Rtl => TextDirection::Rtl,
        };
        resolve_logical_text_alignments(style.text_mut(styles));
        return;
    }
    // Keep image presence even when the renderer cannot paint the image.
    if capabilities::property_uses_background_image(property) {
        style.background.background_image_present = true;
        return;
    }
    let root_font_size = root_font_size_for_resolution(doc, styles);
    if let Some((id, _)) = RendererPropertyId::raw(property) {
        unparsed::apply(
            styles,
            style,
            id,
            root_font_size,
            resolution,
            prepared_renderer.expect("prepared renderer declaration"),
            selected,
        );
        return;
    }
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
    if let Some(value) = prepared_native {
        value.apply(property, &mut context);
        return;
    }
    match property {
        Property::FontSize(..)
        | Property::FontWeight(..)
        | Property::FontStretch(..)
        | Property::FontStyle(..)
        | Property::FontFamily(..) => typography::apply(&mut context, property),
        Property::LineBreak(..)
        | Property::UnicodeBidi(..)
        | Property::LineHeight(..)
        | Property::TabSize(..)
        | Property::LetterSpacing(..)
        | Property::WordSpacing(..)
        | Property::TextAlign(..)
        | Property::TextAlignLast(..)
        | Property::TextIndent(..)
        | Property::TextTransform(..)
        | Property::FontVariantCaps(..)
        | Property::ListStyleType(..)
        | Property::ListStylePosition(..)
        | Property::ListStyleImage(..)
        | Property::TextDecorationLine(..)
        | Property::TextDecorationColor(..)
        | Property::TextDecorationStyle(..)
        | Property::TextDecorationThickness(..)
        | Property::Visibility(..)
        | Property::OutlineWidth(..)
        | Property::OutlineStyle(..)
        | Property::OutlineColor(..)
        | Property::VerticalAlign(..)
        | Property::OverflowX(..)
        | Property::OverflowY(..)
        | Property::TextOverflow(..)
        | Property::WhiteSpace(..) => text::apply(&mut context, property),
        Property::Color(..) | Property::BackgroundColor(..) | Property::BackgroundImage(..) => {
            paint::apply(&mut context, property)
        }
        Property::Width(..)
        | Property::Height(..)
        | Property::InlineSize(..)
        | Property::BlockSize(..)
        | Property::MinWidth(..)
        | Property::MinHeight(..)
        | Property::MinInlineSize(..)
        | Property::MinBlockSize(..)
        | Property::MaxWidth(..)
        | Property::MaxHeight(..)
        | Property::MaxInlineSize(..)
        | Property::MaxBlockSize(..)
        | Property::AspectRatio(..)
        | Property::BorderSpacing(..)
        | Property::Position(..)
        | Property::ZIndex(..)
        | Property::Top(..)
        | Property::Right(..)
        | Property::Bottom(..)
        | Property::Left(..)
        | Property::MarginTop(..)
        | Property::MarginRight(..)
        | Property::MarginBottom(..)
        | Property::MarginLeft(..)
        | Property::MarginBlockStart(..)
        | Property::MarginBlockEnd(..)
        | Property::MarginInlineStart(..)
        | Property::MarginInlineEnd(..)
        | Property::PaddingTop(..)
        | Property::PaddingRight(..)
        | Property::PaddingBottom(..)
        | Property::PaddingLeft(..)
        | Property::PaddingBlockStart(..)
        | Property::PaddingBlockEnd(..)
        | Property::PaddingInlineStart(..)
        | Property::PaddingInlineEnd(..)
        | Property::BorderTopWidth(..)
        | Property::BorderRightWidth(..)
        | Property::BorderBottomWidth(..)
        | Property::BorderLeftWidth(..)
        | Property::BorderBlockStartWidth(..)
        | Property::BorderBlockEndWidth(..)
        | Property::BorderInlineStartWidth(..)
        | Property::BorderInlineEndWidth(..)
        | Property::BorderTopColor(..)
        | Property::BorderRightColor(..)
        | Property::BorderBottomColor(..)
        | Property::BorderLeftColor(..)
        | Property::BorderBlockStartColor(..)
        | Property::BorderBlockEndColor(..)
        | Property::BorderInlineStartColor(..)
        | Property::BorderInlineEndColor(..)
        | Property::BorderTopStyle(..)
        | Property::BorderRightStyle(..)
        | Property::BorderBottomStyle(..)
        | Property::BorderLeftStyle(..)
        | Property::BorderBlockStartStyle(..)
        | Property::BorderBlockEndStyle(..)
        | Property::BorderInlineStartStyle(..)
        | Property::BorderInlineEndStyle(..)
        | Property::BorderTopLeftRadius(..)
        | Property::BorderTopRightRadius(..)
        | Property::BorderBottomRightRadius(..)
        | Property::BorderBottomLeftRadius(..)
        | Property::BorderStartStartRadius(..)
        | Property::BorderStartEndRadius(..)
        | Property::BorderEndStartRadius(..)
        | Property::BorderEndEndRadius(..) => box_model::apply(&mut context, property),
        Property::Display(..)
        | Property::FlexDirection(..)
        | Property::FlexWrap(..)
        | Property::FlexGrow(..)
        | Property::FlexShrink(..)
        | Property::FlexBasis(..)
        | Property::Order(..)
        | Property::AlignContent(..)
        | Property::JustifyContent(..)
        | Property::AlignItems(..)
        | Property::AlignSelf(..)
        | Property::JustifyItems(..)
        | Property::JustifySelf(..)
        | Property::RowGap(..)
        | Property::ColumnGap(..)
        | Property::GridTemplateRows(..)
        | Property::GridTemplateColumns(..)
        | Property::GridAutoRows(..)
        | Property::GridAutoColumns(..)
        | Property::GridAutoFlow(..)
        | Property::GridTemplateAreas(..)
        | Property::GridRowStart(..)
        | Property::GridRowEnd(..)
        | Property::GridColumnStart(..)
        | Property::GridColumnEnd(..) => layout::apply(&mut context, property),
        Property::Hyphens(..)
        | Property::WordBreak(..)
        | Property::OverflowWrap(..)
        | Property::WordWrap(..)
        | Property::BoxSizing(..) => misc::apply(&mut context, property),
        Property::Unparsed(raw) => match raw.property_id {
            PropertyId::ZIndex => box_model::apply(&mut context, property),
            PropertyId::FlexBasis(_) => layout::apply(&mut context, property),
            _ => ApplyResult::Unhandled,
        },
        _ => ApplyResult::Unhandled,
    };
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
