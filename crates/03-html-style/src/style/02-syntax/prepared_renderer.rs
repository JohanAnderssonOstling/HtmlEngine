//! Parsed renderer values retained independently of elements and style interners.
use super::prepared_generated::{
    self, PreparedContent, PreparedContentBuilder, PreparedCounterDirective, PreparedQuoteStyle,
};
use super::prepared_length::{LengthMode, PreparedLength};
use super::property_id::RendererPropertyId;
use super::value::{parse_integer, token_list_to_css_string};
use super::{contain, font_features, native_fallback, renderer_owned};
use html_style_model::ObjectPositionOrigin;
use html_style_model::{
    BorderCollapseMode, BreakBetween, BreakInside, CaptionSide, Clear, EmptyCellsMode, Float,
    FontSynthesisStyle, HangingPunctuation, ObjectFit, OpenTypeFeature, TableLayoutMode,
    TextBoxEdge, TextBoxTrim, TextDirection, TextWrapStyle, WhiteSpace, open_type,
};
use lightningcss::properties::custom::TokenList;
use lightningcss::properties::font::FontVariantCaps;
use lightningcss::values::length::LengthPercentage;
use lightningcss::values::position::Position;

pub(crate) enum PreparedRendererValue {
    Spacing(PreparedLength),
    WhiteSpaceCollapse(WhiteSpace),
    TextWrapMode(bool),
    TextWrapStyle(TextWrapStyle),
    TextWrap(bool, TextWrapStyle),
    HangingPunctuation(HangingPunctuation),
    FontVariant(FontVariantCaps, Vec<OpenTypeFeature>, Vec<OpenTypeFeature>),
    FontSynthesis {
        small_caps: Option<bool>,
        style: Option<FontSynthesisStyle>,
    },
    Features(Vec<OpenTypeFeature>),
    ObjectFit(ObjectFit),
    ObjectPosition(PreparedPosition),
    OutlineOffset(Option<PreparedLength>),
    BreakBetween(BreakBetween),
    BreakInside(BreakInside),
    LineCount(u8),
    Float(Float, Float),
    Clear(Clear, Clear),
    TableLayout(TableLayoutMode),
    BorderCollapse(BorderCollapseMode),
    CaptionSide(CaptionSide),
    EmptyCells(EmptyCellsMode),
    TextBoxTrim(TextBoxTrim),
    TextBoxEdge(TextBoxEdge),
    TextBox(TextBoxTrim, TextBoxEdge),
    Contain(bool),
    Content(Option<PreparedContent>),
    Counters(Vec<PreparedCounterDirective>),
    Quotes(PreparedQuoteStyle),
}

impl PreparedRendererValue {
    pub(crate) fn parse(id: RendererPropertyId, tokens: &TokenList<'_>) -> Option<Self> {
        use RendererPropertyId as Id;
        Some(match id {
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
                Self::TextWrap(mode, style)
            }
            Id::HangingPunctuation => {
                Self::HangingPunctuation(renderer_owned::hanging_punctuation(tokens)?)
            }
            Id::FontVariant => {
                let (caps, numeric, ligatures) = font_features::parse_variant(tokens)?;
                Self::FontVariant(caps, numeric, ligatures)
            }
            Id::FontSynthesis | Id::FontSynthesisStyle | Id::FontSynthesisSmallCaps => {
                Self::FontSynthesis {
                    small_caps: if id == Id::FontSynthesisStyle {
                        None
                    } else {
                        Some(renderer_owned::font_synthesis_small_caps(id, tokens)?)
                    },
                    style: if id == Id::FontSynthesisSmallCaps {
                        None
                    } else {
                        Some(renderer_owned::font_synthesis_style(id, tokens)?)
                    },
                }
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
            Id::Widows | Id::Orphans => Self::LineCount(
                parse_integer(&token_list_to_css_string(tokens)?, 1)?.min(u32::from(u8::MAX)) as u8,
            ),
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
                Self::TextBox(trim, edge)
            }
            Id::Contain => Self::Contain(contain::parse_tokens(tokens)?.size),
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
        })
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
impl PreparedRendererValue {
    pub(crate) fn requires_viewport_height(&self) -> bool {
        match self {
            Self::Spacing(v) | Self::OutlineOffset(Some(v)) => v.viewport_height,
            Self::ObjectPosition(v) => v.x.1.viewport_height || v.y.1.viewport_height,
            _ => false,
        }
    }
}
