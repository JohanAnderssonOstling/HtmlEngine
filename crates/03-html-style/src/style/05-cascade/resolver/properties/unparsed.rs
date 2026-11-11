//! Apply renderer-owned values whose grammar was prepared independently of the element.
use super::super::context::ResolutionContext;
use super::super::prepared_native::{length_pct, typography::evaluate};
use super::super::state::WorkingStyle;
use super::super::values::fonts::font_variant_caps_features;
use crate::style::property_catalogue::PropertyKey;
use crate::style::syntax::prepared_generated::{PreparedContentItem, PreparedQuoteStyle};
use crate::style::syntax::prepared_renderer::PreparedRendererValue;
use crate::style::syntax::property_id::RendererPropertyId;
use html_style_model::{
    ComputedStylesBuilder, CounterDirective, GeneratedContent, GeneratedContentItem,
    ObjectPosition, ObjectPositionAxis, QuoteStyle, TextDirection,
};

pub(super) fn apply(
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    id: RendererPropertyId,
    root_font_size: f32,
    resolution: &mut ResolutionContext,
    prepared: &PreparedRendererValue,
    selected: &impl Fn(PropertyKey) -> bool,
) {
    use PreparedRendererValue as Value;
    use RendererPropertyId as Id;
    if !matches!(
        id,
        Id::TextWrap | Id::FontVariant | Id::FontSynthesis | Id::TextBox
    ) && !selected(id.key())
    {
        return;
    }
    let value = prepared;
    match value {
        Value::Spacing(value) => {
            let c = evaluate(
                value,
                style.font(styles).font_size,
                root_font_size,
                resolution,
            );
            let value =
                html_style_model::TextSpacing::new(c[0], c[1]).expect("finite prepared spacing");
            *if id == Id::LetterSpacing {
                &mut style.text_mut(styles).letter_spacing
            } else {
                &mut style.text_mut(styles).word_spacing
            } = value;
        }
        Value::WhiteSpaceCollapse(value) => {
            style.text_mut(styles).white_space =
                value.with_wrapping(style.text(styles).white_space.allows_wrap())
        }
        Value::TextWrapMode(value) => {
            style.text_mut(styles).white_space =
                style.text(styles).white_space.with_wrapping(*value)
        }
        Value::TextWrapStyle(value) => style.text_mut(styles).text_wrap_style = *value,
        Value::TextWrap(mode, value) => {
            if selected(PropertyKey::TextWrapMode) {
                style.text_mut(styles).white_space =
                    style.text(styles).white_space.with_wrapping(*mode);
            }
            if selected(PropertyKey::TextWrapStyle) {
                style.text_mut(styles).text_wrap_style = *value;
            }
        }
        Value::HangingPunctuation(value) => style.text_mut(styles).hanging_punctuation = *value,
        Value::FontVariant(caps, numeric, ligatures) => {
            if selected(PropertyKey::FontVariantCaps) {
                style.font_mut(styles).font_variant_caps_features =
                    font_variant_caps_features(caps);
            }
            if selected(PropertyKey::FontVariantNumeric) {
                style
                    .font_mut(styles)
                    .font_variant_numeric_features
                    .clone_from(numeric);
            }
            if selected(PropertyKey::FontVariantLigatures) {
                style
                    .font_mut(styles)
                    .font_variant_ligature_features
                    .clone_from(ligatures);
            }
        }
        Value::FontSynthesis {
            small_caps,
            style: synthesis_style,
        } => {
            if let Some(value) = small_caps
                && selected(PropertyKey::FontSynthesisSmallCaps)
            {
                style.font_mut(styles).font_synthesis_small_caps = *value;
            }
            if let Some(value) = synthesis_style
                && selected(PropertyKey::FontSynthesisStyle)
            {
                style.font_mut(styles).font_synthesis_style = *value;
            }
        }
        Value::Features(values) => {
            let target = match id {
                Id::FontFeatureSettings => &mut style.font_mut(styles).font_feature_settings,
                Id::FontKerning => &mut style.font_mut(styles).font_kerning_features,
                Id::FontVariantLigatures => {
                    &mut style.font_mut(styles).font_variant_ligature_features
                }
                Id::FontVariantNumeric => &mut style.font_mut(styles).font_variant_numeric_features,
                _ => unreachable!("prepared feature property"),
            };
            target.clone_from(values);
        }
        Value::ObjectFit(value) => style.box_model.object_fit = *value,
        Value::ObjectPosition(value) => {
            let x = length_pct(
                &value.x.1,
                evaluate(
                    &value.x.1,
                    style.font(styles).font_size,
                    root_font_size,
                    resolution,
                ),
                false,
            );
            let y = length_pct(
                &value.y.1,
                evaluate(
                    &value.y.1,
                    style.font(styles).font_size,
                    root_font_size,
                    resolution,
                ),
                false,
            );
            style.box_model.object_position = ObjectPosition {
                x: ObjectPositionAxis {
                    origin: value.x.0,
                    offset: x,
                },
                y: ObjectPositionAxis {
                    origin: value.y.0,
                    offset: y,
                },
            };
        }
        Value::OutlineOffset(None) => style.background.outline.set_offset_inset(),
        Value::OutlineOffset(Some(value)) => {
            let c = evaluate(
                value,
                style.font(styles).font_size,
                root_font_size,
                resolution,
            );
            style
                .background
                .outline
                .set_offset(length_pct(value, c, false));
        }

        Value::BreakBetween(value) => {
            *if matches!(id, Id::BreakBefore | Id::PageBreakBefore) {
                &mut style.layout.break_before
            } else {
                &mut style.layout.break_after
            } = *value
        }
        Value::BreakInside(value) => style.layout.break_inside = *value,
        Value::LineCount(value) => {
            *if id == Id::Widows {
                &mut style.text_mut(styles).widows
            } else {
                &mut style.text_mut(styles).orphans
            } = *value
        }
        Value::Float(ltr, rtl) => {
            style.box_model.float = if style.text(styles).direction == TextDirection::Ltr {
                *ltr
            } else {
                *rtl
            }
        }
        Value::Clear(ltr, rtl) => {
            style.box_model.clear = if style.text(styles).direction == TextDirection::Ltr {
                *ltr
            } else {
                *rtl
            }
        }
        Value::TableLayout(value) => style.box_model.table_layout = *value,
        Value::BorderCollapse(value) => style.box_model.border_collapse = *value,
        Value::CaptionSide(value) => style.box_model.caption_side = *value,
        Value::EmptyCells(value) => style.box_model.empty_cells = *value,
        Value::TextBoxTrim(value) => style.box_model.text_box_trim = *value,
        Value::TextBoxEdge(value) => style.text_mut(styles).text_box_edge = *value,
        Value::TextBox(trim, edge) => {
            if selected(PropertyKey::TextBoxTrim) {
                style.box_model.text_box_trim = *trim;
            }
            if selected(PropertyKey::TextBoxEdge) {
                style.text_mut(styles).text_box_edge = *edge;
            }
        }
        Value::Contain(value) => style.box_model.size_containment = *value,
        Value::Content(value) => {
            style.generated_content = value.as_ref().map(|content| GeneratedContent {
                items: content
                    .items
                    .iter()
                    .map(|item| match item {
                        PreparedContentItem::Text(value) => {
                            GeneratedContentItem::Text(styles.intern_string(value))
                        }
                        PreparedContentItem::Attribute(value) => {
                            GeneratedContentItem::Attribute(styles.intern_string(value))
                        }
                        PreparedContentItem::Counter { name, style } => {
                            GeneratedContentItem::Counter {
                                name: styles.intern_string(name),
                                style: *style,
                            }
                        }
                        PreparedContentItem::Counters {
                            name,
                            separator,
                            style,
                        } => GeneratedContentItem::Counters {
                            name: styles.intern_string(name),
                            separator: styles.intern_string(separator),
                            style: *style,
                        },
                        PreparedContentItem::OpenQuote => GeneratedContentItem::OpenQuote,
                        PreparedContentItem::CloseQuote => GeneratedContentItem::CloseQuote,
                        PreparedContentItem::NoOpenQuote => GeneratedContentItem::NoOpenQuote,
                        PreparedContentItem::NoCloseQuote => GeneratedContentItem::NoCloseQuote,
                    })
                    .collect(),
            })
        }
        Value::Counters(values) => {
            let values = values
                .iter()
                .map(|value| CounterDirective {
                    name: styles.intern_string(&value.name),
                    value: value.value,
                })
                .collect();
            if id == Id::CounterReset {
                style.counters.resets = values;
            } else {
                style.counters.increments = values;
            }
        }
        Value::Quotes(value) => {
            style.text_mut(styles).quotes = match value {
                PreparedQuoteStyle::Auto => QuoteStyle::Auto,
                PreparedQuoteStyle::None => QuoteStyle::None,
                PreparedQuoteStyle::Pairs(value) => QuoteStyle::Pairs(styles.intern_string(value)),
            }
        }
    }
}
