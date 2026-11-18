//! Parsed renderer values retained independently of elements and style interners.
use crate::style::cascade::resolver::prepared_native::PreparedValue;
use super::prepared_generated::{self, PreparedContentBuilder};
use super::prepared_length::{LengthMode, PreparedLength};
use super::property_id::RendererPropertyId;
use crate::style::property_catalogue::PropertyKey;
use super::substitution::{contains_function, needs_substitution};
use crate::style::support::{PreparationError, UnsupportedStyleFeature};
use super::wide_keywords::property_css_wide_keyword;
use super::value::{significant_tokens, token_list_to_css_string};
use super::{contain, font_features, native_fallback, renderer_owned};
use html_style_model::ObjectPositionOrigin;
use html_style_model::{TextDirection, open_type};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::values::length::LengthPercentage;
use lightningcss::values::position::Position;

impl PreparedValue {
    /// Validate admission and prepare raw values once. Native values are
    /// converted separately after shorthand expansion.
    pub(crate) fn prepare_renderer(
        property: &Property<'_>,
        emit: impl FnMut(PropertyKey, Self),
    ) -> Result<bool, PreparationError> {
        use PreparationError::{InvalidSyntax, Unsupported};
        if !super::ast_validation::property_is_valid(property) {
            return Err(InvalidSyntax);
        }
        if super::capabilities::declaration_is_custom_property(property) {
            return Err(Unsupported(UnsupportedStyleFeature::Property));
        }
        if property_css_wide_keyword(property).is_some()
            || matches!(property, Property::Unparsed(value) if needs_substitution(&value.value))
            || matches!(property, Property::Custom(value) if needs_substitution(&value.value))
        {
            return Ok(false);
        }
        if let Some((id, tokens)) = RendererPropertyId::raw(property) {
            return Self::parse_renderer(id, tokens, emit).map(|()| true).ok_or_else(|| {
                if matches!(
                    id,
                    RendererPropertyId::ObjectPosition | RendererPropertyId::OutlineOffset
                ) {
                    let syntax_valid = token_list_to_css_string(tokens).is_some_and(|css| {
                        if id == RendererPropertyId::ObjectPosition {
                            renderer_owned::parse_object_position(&css).is_some()
                        } else {
                            renderer_owned::parse_outline_offset(&css).is_some()
                        }
                    });
                    if syntax_valid {
                        Unsupported(UnsupportedStyleFeature::Value)
                    } else {
                        InvalidSyntax
                    }
                } else if matches!(
                    id,
                    RendererPropertyId::Content
                        | RendererPropertyId::CounterReset
                        | RendererPropertyId::CounterIncrement
                ) && (tokens.0.iter().any(|token| {
                    matches!(
                        token,
                        TokenOrValue::Url(_) | TokenOrValue::Token(Token::Delim('/'))
                    )
                }) || contains_function(tokens, |name| {
                    name.eq_ignore_ascii_case("reversed")
                })) {
                    Unsupported(UnsupportedStyleFeature::GeneratedContent)
                } else {
                    Unsupported(UnsupportedStyleFeature::Value)
                }
            });
        }
        match property {
            Property::Unparsed(raw) if native_fallback::applies_to(&raw.property_id) => Ok(false),
            Property::Unparsed(raw) if matches!(raw.property_id, PropertyId::FontStretch) => {
                Err(Unsupported(UnsupportedStyleFeature::Value))
            }
            Property::Unparsed(_) => Err(InvalidSyntax),
            Property::Custom(_) => Err(Unsupported(UnsupportedStyleFeature::Property)),
            _ => Ok(false),
        }
    }

    pub(crate) fn parse_renderer(
        id: RendererPropertyId,
        tokens: &TokenList<'_>,
        mut emit: impl FnMut(PropertyKey, Self),
    ) -> Option<()> {
        use RendererPropertyId as Id;
        use PropertyKey as Key;
        let value = match id {
            Id::LetterSpacing | Id::WordSpacing => Self::Spacing(PreparedLength::percentage(
                &native_fallback::text_spacing(tokens)?,
                LengthMode::Scalar,
            )?),
            Id::WhiteSpaceCollapse => {
                Self::WhiteSpaceCollapse(renderer_owned::white_space_collapse(tokens)?)
            }
            Id::TextWrapMode => Self::TextWrapMode(renderer_owned::text_wrap_mode(tokens)?),
            Id::TextWrapStyle => Self::TextWrapStyle(renderer_owned::text_wrap_style(tokens)?),
            Id::TextWrap => {
                let (mode, style) = renderer_owned::text_wrap(tokens)?;
                emit(Key::TextWrapMode, Self::TextWrapMode(mode));
                emit(Key::TextWrapStyle, Self::TextWrapStyle(style));
                return Some(());
            }
            Id::HangingPunctuation => {
                Self::HangingPunctuation(renderer_owned::hanging_punctuation(tokens)?)
            }
            Id::FontVariant => {
                let (caps, numeric, ligatures) = font_features::parse_variant(tokens)?;
                emit(
                    Key::FontVariantCaps,
                    Self::Features(font_features::caps_features(&caps)),
                );
                emit(Key::FontVariantNumeric, Self::Features(numeric));
                emit(Key::FontVariantLigatures, Self::Features(ligatures));
                return Some(());
            }
            Id::FontSynthesis => {
                let caps = renderer_owned::font_synthesis_small_caps(id, tokens)?;
                let style = renderer_owned::font_synthesis_style(id, tokens)?;
                emit(
                    Key::FontSynthesisSmallCaps,
                    Self::FontSynthesisSmallCaps(caps),
                );
                emit(Key::FontSynthesisStyle, Self::FontSynthesisStyle(style));
                return Some(());
            }
            Id::FontSynthesisSmallCaps => {
                Self::FontSynthesisSmallCaps(renderer_owned::font_synthesis_small_caps(id, tokens)?)
            }
            Id::FontSynthesisStyle => {
                Self::FontSynthesisStyle(renderer_owned::font_synthesis_style(id, tokens)?)
            }
            Id::ObjectFit => Self::ObjectFit(renderer_owned::object_fit(tokens)?),
            Id::ObjectPosition => Self::ObjectPosition(PreparedPosition::prepare(
                &renderer_owned::parse_object_position(&token_list_to_css_string(tokens)?)?,
            )?),
            Id::OutlineOffset => {
                let css = token_list_to_css_string(tokens)?;
                let value = match renderer_owned::parse_outline_offset(&css)? {
                    renderer_owned::OutlineOffset::Inset => None,
                    renderer_owned::OutlineOffset::Length(value) => {
                        Some(PreparedLength::length(&value, LengthMode::Affine)?)
                    }
                };
                Self::OutlineOffset(value)
            }
            Id::FontFeatureSettings => {
                let mut values = font_features::parse_settings(tokens)?;
                values.reverse();
                values.sort_by_key(|value| open_type::open_type_feature_tag_bytes(*value));
                values.dedup_by_key(|value| open_type::open_type_feature_tag_bytes(*value));
                Self::Features(values)
            }
            Id::FontKerning | Id::FontVariantLigatures | Id::FontVariantNumeric => {
                Self::Features(font_features::parse_tokens(id, tokens)?)
            }
            Id::BreakBefore | Id::PageBreakBefore | Id::BreakAfter | Id::PageBreakAfter => {
                Self::BreakBetween(renderer_owned::break_between(id, tokens)?)
            }
            Id::BreakInside | Id::PageBreakInside => {
                Self::BreakInside(renderer_owned::break_inside(id, tokens)?)
            }
            Id::Widows | Id::Orphans => {
                let mut tokens = significant_tokens(tokens);
                let TokenOrValue::Token(Token::Number {
                    int_value: Some(value),
                    ..
                }) = tokens.next()?
                else {
                    return None;
                };
                if *value <= 0 || tokens.next().is_some() {
                    return None;
                }
                Self::LineCount((*value).min(i32::from(u8::MAX)) as u8)
            }
            Id::Float => Self::Float(
                renderer_owned::float(tokens, TextDirection::Ltr)?,
                renderer_owned::float(tokens, TextDirection::Rtl)?,
            ),
            Id::Clear => Self::Clear(
                renderer_owned::clear(tokens, TextDirection::Ltr)?,
                renderer_owned::clear(tokens, TextDirection::Rtl)?,
            ),
            Id::TableLayout => Self::TableLayout(renderer_owned::table_layout(tokens)?),
            Id::BorderCollapse => Self::BorderCollapse(renderer_owned::border_collapse(tokens)?),
            Id::CaptionSide => Self::CaptionSide(renderer_owned::caption_side(tokens)?),
            Id::EmptyCells => Self::EmptyCells(renderer_owned::empty_cells(tokens)?),
            Id::TextBoxTrim => Self::TextBoxTrim(renderer_owned::text_box_trim(tokens)?),
            Id::TextBoxEdge => Self::TextBoxEdge(renderer_owned::text_box_edge(tokens)?),
            Id::TextBox => {
                let (trim, edge) = renderer_owned::text_box(tokens)?;
                emit(Key::TextBoxTrim, Self::TextBoxTrim(trim));
                emit(Key::TextBoxEdge, Self::TextBoxEdge(edge));
                return Some(());
            }
            Id::Contain => Self::Contain(contain::parse_tokens(tokens)?),
            Id::Content => Self::Content(prepared_generated::parse_generated_content(
                tokens,
                &mut PreparedContentBuilder,
            )?),
            Id::CounterReset | Id::CounterIncrement => {
                Self::Counters(prepared_generated::parse_counter_directives(
                    tokens,
                    if id == Id::CounterReset { 0 } else { 1 },
                    &mut PreparedContentBuilder,
                )?)
            }
            Id::Quotes => Self::Quotes(prepared_generated::parse_quotes(
                tokens,
                &mut PreparedContentBuilder,
            )?),
            // These aliases are canonicalized to native properties before preparation.
            Id::FontWidth | Id::GridGap | Id::GridRowGap | Id::GridColumnGap => return None,
        };
        emit(id.key(), value);
        Some(())
    }
}

pub(crate) struct PreparedPosition {
    pub(crate) x: (ObjectPositionOrigin, PreparedLength),
    pub(crate) y: (ObjectPositionOrigin, PreparedLength),
}
impl PreparedPosition {
    fn prepare(value: &Position) -> Option<Self> {
        use lightningcss::values::position::{
            HorizontalPositionKeyword, PositionComponent, VerticalPositionKeyword,
        };
        fn axis<S: PartialEq>(
            value: &PositionComponent<S>,
            end: S,
        ) -> Option<(ObjectPositionOrigin, PreparedLength)> {
            Some(match value {
                PositionComponent::Center => (
                    ObjectPositionOrigin::Start,
                    PreparedLength::percentage(
                        &LengthPercentage::Percentage(
                            lightningcss::values::percentage::Percentage(0.5),
                        ),
                        LengthMode::Affine,
                    )?,
                ),
                PositionComponent::Length(v) => (
                    ObjectPositionOrigin::Start,
                    PreparedLength::percentage(v, LengthMode::Affine)?,
                ),
                PositionComponent::Side { side, offset } => (
                    if *side == end {
                        ObjectPositionOrigin::End
                    } else {
                        ObjectPositionOrigin::Start
                    },
                    match offset {
                        Some(v) => PreparedLength::percentage(v, LengthMode::Affine)?,
                        None => PreparedLength::zero(),
                    },
                ),
            })
        }
        Some(Self {
            x: axis(&value.x, HorizontalPositionKeyword::Right)?,
            y: axis(&value.y, VerticalPositionKeyword::Bottom)?,
        })
    }
}
