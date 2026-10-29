use super::super::context::ResolutionContext;
use super::super::custom_properties::token_list_to_css_string;
use super::super::state::WorkingStyle;
use super::super::values::lengths::computed_length_pct;
use super::generated::{parse_counter_directives, parse_generated_content, parse_quotes};
use crate::style::syntax::property_id::RendererPropertyId;
use crate::style::syntax::value::parse_integer;
use crate::style::syntax::{contain, font_features, renderer_owned};
use html_style_model::{
    ComputedStylesBuilder, LengthPct, ObjectPosition, ObjectPositionAxis, ObjectPositionOrigin,
};
use lightningcss::properties::custom::TokenList;
use lightningcss::traits::Parse;
use lightningcss::values::length::LengthPercentage;

/// Convert renderer-owned raw values after declaration eligibility checking.
pub(super) fn apply(
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    id: RendererPropertyId,
    tokens: &TokenList<'_>,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<()> {
    match id {
        RendererPropertyId::ObjectFit => {
            style.box_model.object_fit = renderer_owned::object_fit(tokens)?;
        }
        RendererPropertyId::ObjectPosition => {
            style.box_model.object_position =
                parse_object_position(tokens, style.font.font_size, root_font_size, resolution)?;
        }
        RendererPropertyId::OutlineOffset => {
            match parse_outline_offset(tokens, style.font.font_size, root_font_size, resolution)? {
                ParsedOutlineOffset::Length(value) => style.background.outline.set_offset(value),
                ParsedOutlineOffset::Inset => style.background.outline.set_offset_inset(),
            }
        }
        RendererPropertyId::FontFeatureSettings => {
            let mut features = font_features::parse_settings(tokens)?;
            // Stable sorting after reversing keeps the last authored value per tag.
            features.reverse();
            features.sort_by_key(|feature| feature.tag());
            features.dedup_by_key(|feature| feature.tag());
            style.font.font_feature_settings = features;
        }
        RendererPropertyId::FontKerning
        | RendererPropertyId::FontVariantLigatures
        | RendererPropertyId::FontVariantNumeric => {
            *match id {
                RendererPropertyId::FontKerning => &mut style.font.font_kerning_features,
                RendererPropertyId::FontVariantLigatures => {
                    &mut style.font.font_variant_ligature_features
                }
                RendererPropertyId::FontVariantNumeric => {
                    &mut style.font.font_variant_numeric_features
                }
                _ => unreachable!(),
            } = font_features::parse_tokens(id, tokens)?;
        }
        RendererPropertyId::BreakBefore
        | RendererPropertyId::PageBreakBefore
        | RendererPropertyId::BreakAfter
        | RendererPropertyId::PageBreakAfter => {
            *match id {
                RendererPropertyId::BreakBefore | RendererPropertyId::PageBreakBefore => {
                    &mut style.layout.break_before
                }
                _ => &mut style.layout.break_after,
            } = renderer_owned::break_between(id, tokens)?;
        }
        RendererPropertyId::BreakInside | RendererPropertyId::PageBreakInside => {
            style.layout.break_inside = renderer_owned::break_inside(id, tokens)?;
        }
        RendererPropertyId::Widows | RendererPropertyId::Orphans => {
            let value = token_list_to_css_string(tokens)
                .and_then(|css| parse_integer(&css, 1))?
                .min(u32::from(u8::MAX)) as u8;
            *match id {
                RendererPropertyId::Widows => &mut style.text.widows,
                _ => &mut style.text.orphans,
            } = value;
        }
        // These conversions have already been checked for renderer eligibility.
        RendererPropertyId::Float => {
            if let Some(value) = renderer_owned::float(tokens, style.text.direction) {
                style.box_model.float = value;
            }
        }
        RendererPropertyId::Clear => {
            if let Some(value) = renderer_owned::clear(tokens, style.text.direction) {
                style.box_model.clear = value;
            }
        }
        RendererPropertyId::TableLayout => {
            if let Some(value) = renderer_owned::table_layout(tokens) {
                style.box_model.table_layout = value;
            }
        }
        RendererPropertyId::BorderCollapse => {
            if let Some(value) = renderer_owned::border_collapse(tokens) {
                style.box_model.border_collapse = value;
            }
        }
        RendererPropertyId::CaptionSide => {
            if let Some(value) = renderer_owned::caption_side(tokens) {
                style.box_model.caption_side = value;
            }
        }
        RendererPropertyId::EmptyCells => {
            if let Some(value) = renderer_owned::empty_cells(tokens) {
                style.box_model.empty_cells = value;
            }
        }
        RendererPropertyId::TextBoxTrim => {
            if let Some(value) = renderer_owned::text_box_trim(tokens) {
                style.box_model.text_box_trim = value;
            }
        }
        RendererPropertyId::TextBoxEdge => {
            if let Some(value) = renderer_owned::text_box_edge(tokens) {
                style.text.text_box_edge = value;
            }
        }
        RendererPropertyId::TextBox => {
            if let Some((trim, edge)) = renderer_owned::text_box(tokens) {
                style.box_model.text_box_trim = trim;
                style.text.text_box_edge = edge;
            }
        }
        RendererPropertyId::Contain => {
            if let Some(value) = contain::parse_tokens(tokens) {
                style.box_model.size_containment = value.size;
            }
        }
        RendererPropertyId::Content => {
            if let Some(value) = parse_generated_content(styles, tokens) {
                style.generated_content = value;
            }
        }
        RendererPropertyId::CounterReset => {
            if let Some(value) = parse_counter_directives(styles, tokens, 0) {
                style.counters.resets = value;
            }
        }
        RendererPropertyId::CounterIncrement => {
            if let Some(value) = parse_counter_directives(styles, tokens, 1) {
                style.counters.increments = value;
            }
        }
        RendererPropertyId::Quotes => {
            if let Some(value) = parse_quotes(styles, tokens) {
                style.text.quotes = value;
            }
        }
        _ => {}
    }
    Some(())
}

fn parse_object_position(
    tokens: &TokenList<'_>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<ObjectPosition> {
    use lightningcss::values::position::{
        HorizontalPositionKeyword, Position, PositionComponent, VerticalPositionKeyword,
    };

    let css = token_list_to_css_string(tokens)?;
    let position = Position::parse_string(&css).ok()?;
    fn axis<S: Copy + PartialEq>(
        component: PositionComponent<S>,
        end: S,
        resolve: impl FnOnce(&LengthPercentage) -> Option<LengthPct>,
    ) -> Option<ObjectPositionAxis> {
        let (origin, offset) = match component {
            PositionComponent::Center => (ObjectPositionOrigin::Start, LengthPct::Pct(0.5)),
            PositionComponent::Length(value) => (ObjectPositionOrigin::Start, resolve(&value)?),
            PositionComponent::Side { side, offset } => (
                if side == end {
                    ObjectPositionOrigin::End
                } else {
                    ObjectPositionOrigin::Start
                },
                offset.as_ref().map_or(Some(LengthPct::Px(0.0)), resolve)?,
            ),
        };
        Some(ObjectPositionAxis { origin, offset })
    }
    let resolve = |value: &LengthPercentage| {
        computed_length_pct(value, font_size, root_font_size, resolution)
    };
    Some(ObjectPosition {
        x: axis(position.x, HorizontalPositionKeyword::Right, resolve)?,
        y: axis(position.y, VerticalPositionKeyword::Bottom, resolve)?,
    })
}

enum ParsedOutlineOffset {
    Length(LengthPct),
    Inset,
}

fn parse_outline_offset(
    tokens: &TokenList<'_>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<ParsedOutlineOffset> {
    use lightningcss::values::length::Length;

    let css = token_list_to_css_string(tokens)?;
    if css.eq_ignore_ascii_case("inset") {
        return Some(ParsedOutlineOffset::Inset);
    }
    Length::parse_string(&css).ok()?;
    match computed_length_pct(
        &LengthPercentage::parse_string(&css).ok()?,
        font_size,
        root_font_size,
        resolution,
    )? {
        LengthPct::Pct(_)
        | LengthPct::Calc {
            percentage_dependent: true,
            ..
        } => None,
        value => Some(ParsedOutlineOffset::Length(value)),
    }
}
