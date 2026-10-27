use super::*;
use crate::style::syntax::value::{parse_integer, parse_number, parse_signed_integer, parsed_property};
use crate::style::syntax::{contain, flex, font_features, renderer_owned, unit_algebra};

pub(super) fn apply_unparsed_property(
    styles: &mut ComputedStylesBuilder,
    style: &mut WorkingStyle,
    property: &Property<'_>,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<ApplyResult> {
    if let Property::Unparsed(value) = property
        && unit_algebra::size_property(&value.property_id)
    {
        let calculated = (|| {
            let source = property.value_to_css_string(PrinterOptions::default()).ok()?;
            let value = unit_algebra::size_calculation(&source)?;
            let resolved = value.resolve_length(|length| {
                length_to_px(length, style.font.font_size, root_font_size, resolution)
            })?;
            let native = parsed_property(property.property_id(), &resolved)?;
            let (Property::Width(size) | Property::Height(size)) = native else {
                return None;
            };
            size_to_preferred(&size, &style.font, root_font_size, styles, resolution)
                .and_then(non_negative_preferred_size)
        })();
        return Some(match calculated {
            Some(calculated) => {
                if matches!(value.property_id, PropertyId::Width) {
                    style.box_model.width = calculated;
                } else {
                    style.box_model.height = calculated;
                }
                ApplyResult::Applied
            }
            None => ApplyResult::Invalid,
        });
    }
    if let Property::Unparsed(value) = property
        && matches!(value.property_id, PropertyId::LetterSpacing | PropertyId::WordSpacing)
    {
        let spacing = property.value_to_css_string(PrinterOptions::default()).ok()
            .and_then(|css| parse_text_spacing(&css))
            .and_then(|value| parsed_text_spacing_to_computed(&value, style.font.font_size, root_font_size, resolution));
        return Some(match spacing {
            Some(spacing) => {
                if matches!(value.property_id, PropertyId::LetterSpacing) {
                    style.text.letter_spacing = spacing;
                } else {
                    style.text.word_spacing = spacing;
                }
                ApplyResult::Applied
            }
            None => ApplyResult::Invalid,
        });
    }
    if let Property::Unparsed(value) = property
        && matches!(value.property_id, PropertyId::FontWeight)
    {
        let weight = property
            .value_to_css_string(PrinterOptions::default())
            .ok()
            .and_then(|css| parse_number(&css))
            .and_then(|value| {
                resolve_number(&value, style.font.font_size, root_font_size, resolution)
            });
        return Some(match weight {
            Some(value) => {
                style.font.font_weight = value.max(1.0).min(1000.0) as u16;
                ApplyResult::Applied
            }
            None => ApplyResult::Invalid,
        });
    }
    if let Property::Unparsed(value) = property {
        if matches!(value.property_id, PropertyId::Flex(_) | PropertyId::FlexBasis(_)) {
            let parsed = (|| {
                let css = property.value_to_css_string(PrinterOptions::default()).ok()?;
                let (basis, factors) = if matches!(value.property_id, PropertyId::Flex(_)) {
                    let value = flex::parse(&css)?;
                    (value.basis, Some((value.grow, value.shrink)))
                } else {
                    (flex::basis(&css)?, None)
                };
                let basis = match basis {
                    flex::Basis::Content => PreferredSize::Content,
                    flex::Basis::Size(size) => non_negative_preferred_size(size_to_preferred(
                        &size, &style.font, root_font_size, styles, resolution,
                    )?)?,
                };
                style.layout.flex_basis = basis;
                if let Some((grow, shrink)) = factors {
                    style.layout.flex_grow = computed_flex_factor(grow);
                    style.layout.flex_shrink = computed_flex_factor(shrink);
                }
                Some(())
            })();
            return Some(if parsed.is_some() { ApplyResult::Applied } else { ApplyResult::Invalid });
        }
    }
    if let Property::Unparsed(value) = property
        && matches!(value.property_id, PropertyId::ZIndex | PropertyId::Order(_))
    {
        let value = token_list_to_css_string(&value.value)
            .and_then(|css| parse_signed_integer(&css))
            .and_then(|value| resolve_signed_integer(&value, style.font.font_size, root_font_size, resolution));
        return Some(match value {
            Some(value) => {
                if matches!(property.property_id(), PropertyId::ZIndex) {
                    style.layout.z_index = Some(value);
                } else {
                    style.layout.order = value;
                }
                ApplyResult::Applied
            }
            None => ApplyResult::Invalid,
        });
    }
    if let Some((id, tokens)) = RendererPropertyId::raw(property) {
        match id {
            RendererPropertyId::ObjectFit => {
                return Some(match renderer_owned::object_fit(tokens) {
                    Some(value) => {
                        style.box_model.object_fit = value;
                        ApplyResult::Applied
                    }
                    None => ApplyResult::Invalid,
                });
            }
            RendererPropertyId::ObjectPosition => {
                return Some(
                    match parse_object_position(
                        tokens,
                        style.font.font_size,
                        root_font_size,
                        resolution,
                    ) {
                        Some(value) => {
                            style.box_model.object_position = value;
                            ApplyResult::Applied
                        }
                        None => ApplyResult::Invalid,
                    },
                );
            }
            RendererPropertyId::OutlineOffset => {
                return Some(
                    match parse_outline_offset(
                        tokens,
                        style.font.font_size,
                        root_font_size,
                        resolution,
                    ) {
                        Some(value) => {
                            match value {
                                ParsedOutlineOffset::Length(value) => {
                                    style.background.outline.set_offset(value)
                                }
                                ParsedOutlineOffset::Inset => {
                                    style.background.outline.set_offset_inset()
                                }
                            }
                            ApplyResult::Applied
                        }
                        None => ApplyResult::Invalid,
                    },
                );
            }
            RendererPropertyId::TextBox
            | RendererPropertyId::TextBoxTrim
            | RendererPropertyId::TextBoxEdge => {
                let _ = apply_text_box_property(style, id, tokens);
                return Some(ApplyResult::Applied);
            }
            RendererPropertyId::FontFeatureSettings => {
                let value = token_list_to_css_string(tokens)
                    .and_then(|css| font_features::parse_settings(&css))
                    .and_then(|settings| {
                        resolve_font_feature_settings(
                            &settings,
                            style.font.font_size,
                            root_font_size,
                            resolution,
                        )
                    });
                return Some(match value {
                    Some(value) => {
                        style.font.font_feature_settings = value;
                        ApplyResult::Applied
                    }
                    None => ApplyResult::Invalid,
                });
            }
            RendererPropertyId::FontKerning
            | RendererPropertyId::FontVariantLigatures
            | RendererPropertyId::FontVariantNumeric => {
                let Some(features) = font_features::parse_tokens(id, tokens) else {
                    return Some(ApplyResult::Invalid);
                };
                match id {
                    RendererPropertyId::FontKerning => style.font.font_kerning_features = features,
                    RendererPropertyId::FontVariantLigatures => {
                        style.font.font_variant_ligature_features = features
                    }
                    RendererPropertyId::FontVariantNumeric => {
                        style.font.font_variant_numeric_features = features
                    }
                    _ => unreachable!(),
                }
                return Some(ApplyResult::Applied);
            }
            RendererPropertyId::BreakBefore | RendererPropertyId::PageBreakBefore => {
                let Some(value) = renderer_owned::break_between(id, tokens) else {
                    return Some(ApplyResult::Invalid);
                };
                style.layout.break_before = value;
                return Some(ApplyResult::Applied);
            }
            RendererPropertyId::BreakAfter | RendererPropertyId::PageBreakAfter => {
                let Some(value) = renderer_owned::break_between(id, tokens) else {
                    return Some(ApplyResult::Invalid);
                };
                style.layout.break_after = value;
                return Some(ApplyResult::Applied);
            }
            RendererPropertyId::BreakInside | RendererPropertyId::PageBreakInside => {
                let Some(value) = renderer_owned::break_inside(id, tokens) else {
                    return Some(ApplyResult::Invalid);
                };
                style.layout.break_inside = value;
                return Some(ApplyResult::Applied);
            }
            RendererPropertyId::Widows | RendererPropertyId::Orphans => {
                let value = token_list_to_css_string(tokens)
                    .and_then(|css| parse_integer(&css, 1))
                    .and_then(|value| {
                        resolve_integer(
                            &value,
                            1..=u32::from(u8::MAX),
                            style.font.font_size,
                            root_font_size,
                            resolution,
                        )
                    });
                let Some(value) = value else {
                    return Some(ApplyResult::Invalid);
                };
                match id {
                    RendererPropertyId::Widows => style.text.widows = value as u8,
                    RendererPropertyId::Orphans => style.text.orphans = value as u8,
                    _ => unreachable!(),
                }
                return Some(ApplyResult::Applied);
            }
            RendererPropertyId::Contain => {
                if let Some(value) = contain::parse_tokens(tokens) {
                    style.box_model.size_containment = value.size;
                }
                return Some(ApplyResult::Applied);
            }
            _ => {}
        }
        if apply_custom_box_keyword(style, id, tokens) {
            return Some(ApplyResult::Applied);
        }
        if id == RendererPropertyId::Content
            && let Some(content) = parse_generated_content(styles, tokens)
        {
            style.generated_content = content;
            return Some(ApplyResult::Applied);
        }
        if id == RendererPropertyId::CounterReset
            && let Some(resets) = parse_counter_directives(styles, tokens, 0)
        {
            style.counters.resets = resets;
            return Some(ApplyResult::Applied);
        }
        if id == RendererPropertyId::CounterIncrement
            && let Some(increments) = parse_counter_directives(styles, tokens, 1)
        {
            style.counters.increments = increments;
            return Some(ApplyResult::Applied);
        }
        if id == RendererPropertyId::Quotes
            && let Some(quotes) = parse_quotes(styles, tokens)
        {
            style.text.quotes = quotes;
            return Some(ApplyResult::Applied);
        }
    }
    None
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
    fn axis<S>(
        component: PositionComponent<S>,
        end: S,
        font_size: f32,
        root_font_size: f32,
        resolution: &ResolutionContext,
    ) -> Option<ObjectPositionAxis>
    where
        S: Copy + PartialEq,
    {
        match component {
            PositionComponent::Center => Some(ObjectPositionAxis {
                origin: ObjectPositionOrigin::Start,
                offset: LengthPct::Pct(0.5),
            }),
            PositionComponent::Length(value) => Some(ObjectPositionAxis {
                origin: ObjectPositionOrigin::Start,
                offset: computed_length_pct(&value, font_size, root_font_size, resolution)?,
            }),
            PositionComponent::Side { side, offset } => Some(ObjectPositionAxis {
                origin: if side == end {
                    ObjectPositionOrigin::End
                } else {
                    ObjectPositionOrigin::Start
                },
                offset: offset.as_ref().map_or(Some(LengthPct::Px(0.0)), |value| {
                    computed_length_pct(value, font_size, root_font_size, resolution)
                })?,
            }),
        }
    }
    Some(ObjectPosition {
        x: axis(
            position.x,
            HorizontalPositionKeyword::Right,
            font_size,
            root_font_size,
            resolution,
        )?,
        y: axis(
            position.y,
            VerticalPositionKeyword::Bottom,
            font_size,
            root_font_size,
            resolution,
        )?,
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
    use lightningcss::values::length::{Length, LengthPercentage};

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
