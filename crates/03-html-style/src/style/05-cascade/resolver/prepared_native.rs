//! Declarations converted once, before selection, into keyed engine values.
mod grid;
pub(crate) mod typography;
use super::context::PropertyContext;
use super::properties::box_model::physical_side;
use super::values;
use super::values::colors::PreparedColor;
use crate::style::property_catalogue::PropertyKey;
use crate::style::support::{PreparationError, UnsupportedStyleFeature};
use crate::style::syntax::capabilities::property_uses_background_image;
use crate::style::syntax::font_features;
use crate::style::syntax::prepared_length::{LengthMode, PreparedLength, finite};
use crate::style::syntax::prepared_renderer::PreparedPosition;
use crate::style::syntax::targets::{FONT_RESET_TARGETS, expand_declaration};
use crate::style::syntax::wide_keywords::property_css_wide_keyword;
use crate::style::syntax::{
    contain,
    prepared_generated::{
        PreparedContent, PreparedContentItem, PreparedCounterDirective, PreparedQuoteStyle,
    },
};
use grid::{Area, Placement, TemplateTrack, Track};
use lightningcss::properties::Property;
use lightningcss::properties::size::{MaxSize, Size};
use lightningcss::properties::text::TextDecorationLine as DecorationLine;
use lightningcss::values::calc::{Calc, MathFunction};
use lightningcss::values::length::{LengthPercentage, LengthPercentageOrAuto};
use std::sync::Arc;
use typography::{PreparedFontSize, PreparedLineHeight, PreparedTextAlign};

use html_style_model::{
    AspectRatio, BorderCollapseMode, BorderStyle, BoxSizing as EngineBoxSizing, BreakBetween,
    BreakInside, CaptionSide, Clear, ComputedSizeComponent, ContentAlignment, CounterDirective,
    DecorationColor, Display as EngineDisplay, EmptyCellsMode,
    FlexDirection as EngineFlexDirection, FlexWrap as EngineFlexWrap, Float, FontRelativeLength,
    FontStyle, FontSynthesisStyle, GeneratedContent, GeneratedContentItem,
    GridAutoFlow as EngineGridAutoFlow, GridPlacement, GridTemplateTrack, HangingPunctuation,
    Hyphens as EngineHyphens, ItemAlignment, LengthPct, LineBreak as EngineLineBreak,
    ListStylePosition as EngineListStylePosition, ListStyleType, LogicalTextAlign, ObjectFit,
    ObjectPositionAxis, OpenTypeFeature, OverflowMode, OverflowWrap as EngineOverflowWrap,
    PositionMode, PreferredSize, QuoteStyle, SizeComparison, TabSize as EngineTabSize,
    TableLayoutMode, TextBoxEdge, TextBoxTrim, TextDecorationLines, TextDecorationStyle,
    TextDecorationThickness as EngineTextDecorationThickness, TextDirection,
    TextOverflow as EngineTextOverflow, TextTransform, TextWrapStyle,
    UnicodeBidi as EngineUnicodeBidi, VerticalAlignValue, Visibility,
    WhiteSpace as EngineWhiteSpace, WordBreak as EngineWordBreak,
};

#[derive(Clone, Debug)]
pub(crate) enum PreparedSize {
    Keyword(PreferredSize),
    Length(PreparedLength),
    Comparison(SizeComparison, Box<[PreparedLength]>),
}
impl PreparedSize {
    fn percentage(value: &LengthPercentage) -> Option<Self> {
        fn comparison(
            value: &Calc<LengthPercentage>,
        ) -> Option<(SizeComparison, Vec<&Calc<LengthPercentage>>)> {
            let Calc::Function(function) = value else {
                return None;
            };
            match function.as_ref() {
                MathFunction::Calc(value) => comparison(value),
                MathFunction::Min(values) if (1..=3).contains(&values.len()) => {
                    Some((SizeComparison::Min, values.iter().collect()))
                }
                MathFunction::Max(values) if (1..=3).contains(&values.len()) => {
                    Some((SizeComparison::Max, values.iter().collect()))
                }
                MathFunction::Clamp(a, b, c) => Some((SizeComparison::Clamp, vec![a, b, c])),
                _ => None,
            }
        }
        if let LengthPercentage::Calc(value) = value {
            if let Some((kind, values)) = comparison(value) {
                return Some(Self::Comparison(
                    kind,
                    values
                        .into_iter()
                        .map(PreparedLength::calc_percentage)
                        .collect::<Option<Vec<_>>>()?
                        .into_boxed_slice(),
                ));
            }
        }
        Some(Self::Length(PreparedLength::percentage(
            value,
            LengthMode::Affine,
        )?))
    }
    fn size(value: &Size) -> Option<Self> {
        Some(match value {
            Size::LengthPercentage(v) => return Self::percentage(v),
            Size::MinContent(_) => Self::Keyword(PreferredSize::MinContent),
            Size::MaxContent(_) => Self::Keyword(PreferredSize::MaxContent),
            Size::FitContent(_) => Self::Keyword(PreferredSize::FitContent),
            Size::Stretch(_) => Self::Keyword(PreferredSize::Stretch),
            _ => Self::Keyword(PreferredSize::Auto),
        })
    }
    fn max_size(value: &MaxSize) -> Option<Self> {
        Some(match value {
            MaxSize::LengthPercentage(v) => return Self::percentage(v),
            MaxSize::MinContent(_) => Self::Keyword(PreferredSize::MinContent),
            MaxSize::MaxContent(_) => Self::Keyword(PreferredSize::MaxContent),
            MaxSize::FitContent(_) => Self::Keyword(PreferredSize::FitContent),
            MaxSize::Stretch(_) => Self::Keyword(PreferredSize::Stretch),
            _ => Self::Keyword(PreferredSize::Auto),
        })
    }
    fn evaluate(&self, context: &mut PropertyContext<'_>) -> PreferredSize {
        match self {
            Self::Keyword(value) => *value,
            Self::Length(value) => {
                let mut c = components(value, context);
                if let Some(em) = value.em {
                    let font = context.style.font(context.styles);
                    c[2] = finite(f64::from(em) * f64::from(font.font_size_x_height_px));
                    c[3] = finite(f64::from(em) * f64::from(font.font_size_ch_advance_px));
                    c[4] = finite(f64::from(em) * f64::from(font.font_size_cap_height_px));
                }
                match value.literal_axis {
                    Some(0) if c[2..].iter().all(|v| *v == 0.0) => PreferredSize::Px(c[0].max(0.0)),
                    Some(1) => PreferredSize::Percent(c[1].max(0.0)),
                    Some(2) => PreferredSize::Ex(c[2].max(0.0)),
                    Some(3) => PreferredSize::Ch(c[3].max(0.0)),
                    Some(4) => PreferredSize::Cap(c[4].max(0.0)),
                    _ => PreferredSize::Calc {
                        absolute_px: c[0],
                        percentage: c[1],
                        x_height_px: c[2],
                        ch_advance_px: c[3],
                        cap_height_px: c[4],
                        percentage_dependent: value.percentage_dependent,
                    },
                }
            }
            Self::Comparison(kind, operands) => {
                let mut values = [ComputedSizeComponent::default(); 3];
                for (slot, value) in values.iter_mut().zip(operands.iter()) {
                    let c = components(value, context);
                    *slot = ComputedSizeComponent {
                        absolute_px: c[0],
                        percentage: c[1],
                        x_height_px: c[2],
                        ch_advance_px: c[3],
                        cap_height_px: c[4],
                        percentage_dependent: value.percentage_dependent,
                    };
                }
                // Construction permits exactly one to three operands; interning
                // has no other failure condition.
                context
                    .styles
                    .intern_size_comparison(*kind, values, operands.len() as u8)
                    .expect("prepared comparison arity")
            }
        }
    }
    fn needs_height(&self) -> bool {
        match self {
            Self::Keyword(_) => false,
            Self::Length(v) => v.viewport_height,
            Self::Comparison(_, v) => v.iter().any(|v| v.viewport_height),
        }
    }
}

pub(crate) enum PreparedValue {
    Spacing(PreparedLength),
    WhiteSpaceCollapse(EngineWhiteSpace),
    TextWrapMode(bool),
    TextWrapStyle(TextWrapStyle),
    HangingPunctuation(HangingPunctuation),
    FontSynthesisSmallCaps(bool),
    FontSynthesisStyle(FontSynthesisStyle),
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
    Contain(contain::ParsedContain),
    Content(Option<PreparedContent>),
    Counters(Vec<PreparedCounterDirective>),
    Quotes(PreparedQuoteStyle),
    TextAlign(PreparedTextAlign, bool),
    TextAlignLast(PreparedTextAlign),
    FontFamily(Box<str>),
    FontSize(PreparedFontSize),
    LineHeight(Arc<PreparedLineHeight>),
    TabSpaces(f32),
    TabLength(PreparedLength),
    Vertical(VerticalAlignValue),
    VerticalLength(PreparedLength),
    Thickness(EngineTextDecorationThickness),
    ThicknessLength(PreparedLength),
    OutlineStyle(BorderStyle),
    DecorationStyle(TextDecorationStyle),
    ListStyle(ListStyleType),
    GridTemplate(Vec<TemplateTrack>, Vec<Vec<Box<str>>>),
    GridAuto(Vec<Track>),
    GridPlacement(Placement),
    GridAreas(Vec<Area>, u16, u16),
    Display(html_style_model::Display),
    ZIndex(Option<i32>),
    Size(PreparedSize),
    Length(PreparedLength),
    TextIndent(PreparedLength, bool, bool),
    AutoLength(Option<PreparedLength>),
    Pair(PreparedLength, PreparedLength),
    Position(PositionMode),
    AspectRatio(AspectRatio),
    Alignment(ItemAlignment),
    ContentAlignment(ContentAlignment),
    FontStretch(f32),
    FontStyle(FontStyle, f32),
    FontWeight(PreparedWeight),
    Visibility(Visibility),
    TextTransform(TextTransform),
    Direction(TextDirection),
    Hyphens(EngineHyphens),
    WordBreak(EngineWordBreak),
    OverflowWrap(EngineOverflowWrap),
    BoxSizing(EngineBoxSizing),
    FlexDirection(EngineFlexDirection),
    FlexWrap(EngineFlexWrap),
    FlexGrow(f32),
    FlexShrink(f32),
    Order(i32),
    JustifyContent(ContentAlignment),
    GridAutoFlow(EngineGridAutoFlow),
    LineBreak(EngineLineBreak),
    UnicodeBidi(EngineUnicodeBidi),
    ListStylePosition(EngineListStylePosition),
    ListStyleImage(Option<Box<str>>),
    OverflowX(OverflowMode),
    OverflowY(OverflowMode),
    TextOverflow(EngineTextOverflow),
    TextDecorationLine(TextDecorationLines),
    BorderStyle(BorderStyle),
    Color(PreparedColor),
    BackgroundImage(bool),
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum PreparedWeight {
    Absolute(u16),
    Bolder,
    Lighter,
}

impl PreparedValue {
    pub(crate) fn prepare(property: &Property<'_>) -> Result<Option<Self>, ()> {
        use Property::*;
        if crate::style::syntax::wide_keywords::property_css_wide_keyword(property).is_some()
            || matches!(property,Unparsed(raw) if crate::style::syntax::substitution::needs_substitution(&raw.value))
        {
            return Ok(None);
        }
        let length =
            |v: &LengthPercentage| PreparedLength::percentage(v, LengthMode::Affine).ok_or(());
        let auto = |v: &LengthPercentageOrAuto| match v {
            LengthPercentageOrAuto::Auto => Ok(None),
            LengthPercentageOrAuto::LengthPercentage(v) => length(v).map(Some),
        };
        Ok(Some(match property {
            LetterSpacing(v) | WordSpacing(v) => Self::Spacing(match v {
                lightningcss::properties::text::Spacing::Normal => PreparedLength::zero(),
                lightningcss::properties::text::Spacing::Length(v) => {
                    PreparedLength::length(v, LengthMode::Spacing).ok_or(())?
                }
            }),
            FontVariantCaps(value) => Self::Features(font_features::caps_features(value)),
            BackgroundImage(_) => Self::BackgroundImage(property_uses_background_image(property)),
            Color(value)
            | BackgroundColor(value)
            | TextDecorationColor(value, _)
            | OutlineColor(value)
            | BorderTopColor(value)
            | BorderRightColor(value)
            | BorderBottomColor(value)
            | BorderLeftColor(value)
            | BorderBlockStartColor(value)
            | BorderBlockEndColor(value)
            | BorderInlineStartColor(value)
            | BorderInlineEndColor(value) => Self::Color(PreparedColor::new(value)),
            BorderTopStyle(value)
            | BorderRightStyle(value)
            | BorderBottomStyle(value)
            | BorderLeftStyle(value)
            | BorderBlockStartStyle(value)
            | BorderBlockEndStyle(value)
            | BorderInlineStartStyle(value)
            | BorderInlineEndStyle(value) => {
                Self::BorderStyle(values::lengths::line_style_to_border_style(value))
            }
            ListStyleImage(image) => {
                use lightningcss::values::image::Image;
                Self::ListStyleImage(match image {
                    Image::Url(url) => {
                        let url = url.url.as_ref().trim();
                        (!url.is_empty()).then(|| url.into())
                    }
                    _ => None,
                })
            }
            ZIndex(value) => {
                use lightningcss::properties::position::ZIndex as CssZIndex;
                Self::ZIndex(match value {
                    CssZIndex::Auto => None,
                    CssZIndex::Integer(value) => Some(*value),
                })
            }
            LineBreak(value) => {
                use lightningcss::properties::text::LineBreak as CssLineBreak;
                Self::LineBreak(match value {
                    CssLineBreak::Auto => EngineLineBreak::Auto,
                    CssLineBreak::Loose => EngineLineBreak::Loose,
                    CssLineBreak::Normal => EngineLineBreak::Normal,
                    CssLineBreak::Strict => EngineLineBreak::Strict,
                    CssLineBreak::Anywhere => EngineLineBreak::Anywhere,
                })
            }
            UnicodeBidi(value) => {
                use lightningcss::properties::text::UnicodeBidi as CssUnicodeBidi;
                Self::UnicodeBidi(match value {
                    CssUnicodeBidi::Normal => EngineUnicodeBidi::Normal,
                    CssUnicodeBidi::Embed => EngineUnicodeBidi::Embed,
                    CssUnicodeBidi::Isolate => EngineUnicodeBidi::Isolate,
                    CssUnicodeBidi::BidiOverride => EngineUnicodeBidi::BidiOverride,
                    CssUnicodeBidi::IsolateOverride => EngineUnicodeBidi::IsolateOverride,
                    CssUnicodeBidi::Plaintext => EngineUnicodeBidi::Plaintext,
                })
            }

            ListStylePosition(value) => {
                Self::ListStylePosition(values::lists::map_list_style_position(value))
            }
            OverflowX(value) => Self::OverflowX(values::overflow_mode(*value)),
            OverflowY(value) => Self::OverflowY(values::overflow_mode(*value)),
            TextOverflow(value, _) => {
                use lightningcss::properties::overflow::TextOverflow as CssTextOverflow;
                Self::TextOverflow(match value {
                    CssTextOverflow::Clip => EngineTextOverflow::Clip,
                    CssTextOverflow::Ellipsis => EngineTextOverflow::Ellipsis,
                })
            }
            FlexDirection(value, _) => Self::FlexDirection(values::flex_direction(value)),
            FlexWrap(value, _) => Self::FlexWrap(values::flex_wrap(value)),
            FlexGrow(value, _) => Self::FlexGrow(values::fonts::computed_flex_factor(*value)),
            FlexShrink(value, _) => Self::FlexShrink(values::fonts::computed_flex_factor(*value)),
            Order(value, _) => Self::Order(*value),
            JustifyContent(value, _) => Self::JustifyContent(values::justify_content(value)),
            GridAutoFlow(value) => Self::GridAutoFlow(values::grid::grid_auto_flow(*value)),
            Hyphens(value, _) => {
                use lightningcss::properties::text::Hyphens as CssHyphens;
                Self::Hyphens(match value {
                    CssHyphens::None => EngineHyphens::None,
                    CssHyphens::Manual => EngineHyphens::Manual,
                    CssHyphens::Auto => EngineHyphens::Auto,
                })
            }
            WordBreak(value) => {
                use lightningcss::properties::text::WordBreak as CssWordBreak;
                Self::WordBreak(match value {
                    CssWordBreak::Normal => EngineWordBreak::Normal,
                    CssWordBreak::BreakAll => EngineWordBreak::BreakAll,
                    CssWordBreak::KeepAll => EngineWordBreak::KeepAll,
                    CssWordBreak::BreakWord => EngineWordBreak::BreakWord,
                })
            }
            OverflowWrap(value) | WordWrap(value) => {
                use lightningcss::properties::text::OverflowWrap as CssOverflowWrap;
                Self::OverflowWrap(match value {
                    CssOverflowWrap::Normal => EngineOverflowWrap::Normal,
                    CssOverflowWrap::BreakWord => EngineOverflowWrap::BreakWord,
                    CssOverflowWrap::Anywhere => EngineOverflowWrap::Anywhere,
                })
            }
            BoxSizing(value, _) => {
                use lightningcss::properties::size::BoxSizing as CssBoxSizing;
                Self::BoxSizing(match value {
                    CssBoxSizing::ContentBox => EngineBoxSizing::ContentBox,
                    CssBoxSizing::BorderBox => EngineBoxSizing::BorderBox,
                })
            }
            Direction(value) => {
                use lightningcss::properties::text::Direction as CssDirection;
                Self::Direction(match value {
                    CssDirection::Ltr => TextDirection::Ltr,
                    CssDirection::Rtl => TextDirection::Rtl,
                })
            }
            Display(d) => {
                use lightningcss::properties::display::{
                    Display as LcDisplay, DisplayInside, DisplayKeyword, DisplayOutside,
                };
                let display = match d {
                    LcDisplay::Keyword(kw) => match kw {
                        DisplayKeyword::None => EngineDisplay::None,
                        DisplayKeyword::TableRowGroup => EngineDisplay::TableRowGroup,
                        DisplayKeyword::TableHeaderGroup => EngineDisplay::TableHeaderGroup,
                        DisplayKeyword::TableFooterGroup => EngineDisplay::TableFooterGroup,
                        DisplayKeyword::TableRow => EngineDisplay::TableRow,
                        DisplayKeyword::TableColumnGroup => EngineDisplay::TableColumnGroup,
                        DisplayKeyword::TableColumn => EngineDisplay::TableColumn,
                        DisplayKeyword::TableCell => EngineDisplay::TableCell,
                        DisplayKeyword::TableCaption => EngineDisplay::TableCaption,
                        DisplayKeyword::Contents => EngineDisplay::Contents,
                        // Ruby internals need their own formatting context. Keep an
                        // earlier supported fallback declaration active until it exists.
                        DisplayKeyword::RubyBase
                        | DisplayKeyword::RubyText
                        | DisplayKeyword::RubyBaseContainer
                        | DisplayKeyword::RubyTextContainer => return Err(()),
                    },
                    LcDisplay::Pair(pair)
                        if pair.is_list_item
                            && matches!(pair.outside, DisplayOutside::Block)
                            && matches!(pair.inside, DisplayInside::Flow) =>
                    {
                        EngineDisplay::ListItem
                    }
                    LcDisplay::Pair(pair)
                        if pair.is_list_item
                            && matches!(pair.outside, DisplayOutside::Block)
                            && matches!(pair.inside, DisplayInside::FlowRoot) =>
                    {
                        EngineDisplay::FlowRootListItem
                    }
                    LcDisplay::Pair(pair) if pair.is_list_item => return Err(()),
                    LcDisplay::Pair(pair) => match (&pair.outside, &pair.inside) {
                        (DisplayOutside::Block, DisplayInside::Table) => EngineDisplay::Table,
                        (DisplayOutside::Inline, DisplayInside::Table) => {
                            EngineDisplay::InlineTable
                        }
                        (DisplayOutside::Block, DisplayInside::Flow) => EngineDisplay::Block,
                        (DisplayOutside::Block, DisplayInside::FlowRoot) => EngineDisplay::FlowRoot,
                        (DisplayOutside::Inline, DisplayInside::Flow) => EngineDisplay::Inline,
                        (DisplayOutside::Inline, DisplayInside::FlowRoot) => {
                            EngineDisplay::InlineBlock
                        }
                        (DisplayOutside::Block, DisplayInside::Flex(_)) => EngineDisplay::Flex,
                        (DisplayOutside::Inline, DisplayInside::Flex(_)) => {
                            EngineDisplay::InlineFlex
                        }
                        (DisplayOutside::Block, DisplayInside::Grid) => EngineDisplay::Grid,
                        (DisplayOutside::Inline, DisplayInside::Grid) => EngineDisplay::InlineGrid,
                        // Flow-root, ruby, run-in, and legacy box layout are not
                        // silently approximated by normal flow.
                        _ => return Err(()),
                    },
                };
                Self::Display(display)
            }
            Unparsed(raw)
                if matches!(
                    raw.property_id,
                    lightningcss::properties::PropertyId::ZIndex
                        | lightningcss::properties::PropertyId::FlexBasis(_)
                ) =>
            {
                use crate::style::syntax::native_fallback::{self, NativeFallback};
                match native_fallback::parse(property).ok_or(())? {
                    NativeFallback::ZIndex(v) => Self::ZIndex(Some(v)),
                    NativeFallback::FlexBasis(v) => Self::Size(PreparedSize::size(&v).ok_or(())?),
                    _ => return Err(()),
                }
            }
            Width(v) | Height(v) | InlineSize(v) | BlockSize(v) | MinWidth(v) | MinHeight(v)
            | MinInlineSize(v) | MinBlockSize(v) => Self::Size(PreparedSize::size(v).ok_or(())?),
            MaxWidth(v) | MaxHeight(v) | MaxInlineSize(v) | MaxBlockSize(v) => {
                Self::Size(PreparedSize::max_size(v).ok_or(())?)
            }
            FlexBasis(v, _) => Self::Size(match v {
                LengthPercentageOrAuto::Auto => PreparedSize::Keyword(PreferredSize::Auto),
                LengthPercentageOrAuto::LengthPercentage(v) => {
                    PreparedSize::percentage(v).ok_or(())?
                }
            }),
            MarginTop(v) | MarginRight(v) | MarginBottom(v) | MarginLeft(v)
            | MarginBlockStart(v) | MarginBlockEnd(v) | MarginInlineStart(v)
            | MarginInlineEnd(v) | Top(v) | Right(v) | Bottom(v) | Left(v) => {
                Self::AutoLength(auto(v)?)
            }
            PaddingTop(v)
            | PaddingRight(v)
            | PaddingBottom(v)
            | PaddingLeft(v)
            | PaddingBlockStart(v)
            | PaddingBlockEnd(v)
            | PaddingInlineStart(v)
            | PaddingInlineEnd(v) => Self::Length(match v {
                LengthPercentageOrAuto::LengthPercentage(v) => {
                    PreparedLength::nonnegative_percentage(v, LengthMode::Affine).ok_or(())?
                }
                _ => return Err(()),
            }),
            BorderTopWidth(v)
            | BorderRightWidth(v)
            | BorderBottomWidth(v)
            | BorderLeftWidth(v)
            | BorderBlockStartWidth(v)
            | BorderBlockEndWidth(v)
            | BorderInlineStartWidth(v)
            | BorderInlineEndWidth(v)
            | OutlineWidth(v) => {
                use lightningcss::properties::border::BorderSideWidth;
                Self::Length(match v {
                    BorderSideWidth::Thin => PreparedLength::constant(1.0),
                    BorderSideWidth::Medium => PreparedLength::constant(3.0),
                    BorderSideWidth::Thick => PreparedLength::constant(5.0),
                    BorderSideWidth::Length(v) => {
                        PreparedLength::nonnegative_length(v, LengthMode::Border).ok_or(())?
                    }
                })
            }
            BorderSpacing(v) => Self::Pair(
                PreparedLength::nonnegative_length(&v.0, LengthMode::Scalar).ok_or(())?,
                PreparedLength::nonnegative_length(&v.1, LengthMode::Scalar).ok_or(())?,
            ),
            BorderTopLeftRadius(v, _)
            | BorderTopRightRadius(v, _)
            | BorderBottomRightRadius(v, _)
            | BorderBottomLeftRadius(v, _)
            | BorderStartStartRadius(v)
            | BorderStartEndRadius(v)
            | BorderEndStartRadius(v)
            | BorderEndEndRadius(v) => Self::Pair(
                PreparedLength::nonnegative_percentage(&v.0, LengthMode::Affine).ok_or(())?,
                PreparedLength::nonnegative_percentage(&v.1, LengthMode::Affine).ok_or(())?,
            ),
            TextIndent(v) => Self::TextIndent(length(&v.value)?, v.hanging, v.each_line),
            RowGap(v) | ColumnGap(v) => Self::Length(match v {
                lightningcss::properties::align::GapValue::Normal => PreparedLength::zero(),
                lightningcss::properties::align::GapValue::LengthPercentage(v) => {
                    PreparedLength::nonnegative_percentage(v, LengthMode::Affine).ok_or(())?
                }
            }),
            Position(v) => Self::Position(match v {
                lightningcss::properties::position::Position::Static => PositionMode::Static,
                lightningcss::properties::position::Position::Relative => PositionMode::Relative,
                lightningcss::properties::position::Position::Absolute => PositionMode::Absolute,
                _ => return Err(()),
            }),
            AspectRatio(v) => Self::AspectRatio(
                html_style_model::AspectRatio::new(
                    v.auto,
                    v.ratio.as_ref().map(|v| (v.0.max(0.0), v.1.max(0.0))),
                )
                .ok_or(())?,
            ),
            AlignContent(v, _) => Self::ContentAlignment(values::content_alignment(v).ok_or(())?),
            AlignItems(v, _) => Self::Alignment(values::align_items(v).ok_or(())?),
            AlignSelf(v, _) => Self::Alignment(values::align_self(v).ok_or(())?),
            JustifyItems(v) => Self::Alignment(values::justify_items(v).ok_or(())?),
            JustifySelf(v) => Self::Alignment(values::justify_self(v).ok_or(())?),
            FontStretch(v) => {
                let p: lightningcss::values::percentage::Percentage = v.into();
                if !p.0.is_finite() {
                    return Err(());
                }
                Self::FontStretch(finite(f64::from(p.0) * 100.0).max(0.0))
            }
            FontStyle(v) => {
                use lightningcss::properties::font::FontStyle as Css;
                let (style, angle) = match v {
                    Css::Normal => (html_style_model::FontStyle::Normal, 0.0),
                    Css::Italic => (html_style_model::FontStyle::Italic, 0.0),
                    Css::Oblique(v) => (html_style_model::FontStyle::Oblique, v.to_degrees()),
                };
                if !angle.is_finite() {
                    return Err(());
                }
                Self::FontStyle(style, angle.clamp(-90.0, 90.0))
            }
            FontWeight(v) => {
                use lightningcss::properties::font::FontWeight as Css;
                Self::FontWeight(match v {
                    Css::Bolder => PreparedWeight::Bolder,
                    Css::Lighter => PreparedWeight::Lighter,
                    _ => PreparedWeight::Absolute(
                        values::fonts::checked_font_weight(v, 400).ok_or(())?,
                    ),
                })
            }
            Visibility(v) => Self::Visibility(match v {
                lightningcss::properties::display::Visibility::Visible => {
                    html_style_model::Visibility::Visible
                }
                lightningcss::properties::display::Visibility::Hidden => {
                    html_style_model::Visibility::Hidden
                }
                _ => return Err(()),
            }),
            TextDecorationLine(line, _) => {
                if line.intersects(
                    DecorationLine::Blink
                        | DecorationLine::SpellingError
                        | DecorationLine::GrammarError,
                ) {
                    return Err(());
                }
                Self::TextDecorationLine(values::text_decoration_lines(line))
            }
            TextTransform(v) => {
                use lightningcss::properties::text::TextTransformCase as Css;
                if !v.other.is_empty() {
                    return Err(());
                }
                Self::TextTransform(match v.case {
                    Css::Uppercase => html_style_model::TextTransform::Uppercase,
                    Css::Lowercase => html_style_model::TextTransform::Lowercase,
                    Css::Capitalize => html_style_model::TextTransform::Capitalize,
                    _ => html_style_model::TextTransform::None,
                })
            }
            _ => {
                if let Some(value) = Self::prepare_typography(property)? {
                    return Ok(Some(value));
                }
                return Self::prepare_grid(property);
            }
        }))
    }
    pub(crate) fn requires_viewport_height(&self) -> bool {
        match self {
            Self::Spacing(v) | Self::OutlineOffset(Some(v)) => v.viewport_height,
            Self::ObjectPosition(v) => v.x.1.viewport_height || v.y.1.viewport_height,
            Self::GridTemplate(tracks, _) => tracks.iter().any(|track| match track {
                TemplateTrack::Single(v) => v.needs_height(),
                TemplateTrack::Repeat(_, v, _) => v.iter().any(Track::needs_height),
            }),
            Self::GridAuto(v) => v.iter().any(Track::needs_height),
            Self::FontSize(PreparedFontSize::Length(v))
            | Self::TabLength(v)
            | Self::VerticalLength(v)
            | Self::ThicknessLength(v) => v.viewport_height,
            Self::LineHeight(v) => v.needs_height(),
            Self::Size(v) => v.needs_height(),
            Self::Length(v) | Self::TextIndent(v, ..) => v.viewport_height,
            Self::AutoLength(v) => v.as_ref().is_some_and(|v| v.viewport_height),
            Self::Pair(a, b) => a.viewport_height || b.viewport_height,
            _ => false,
        }
    }
    pub(super) fn apply(&self, key: PropertyKey, context: &mut PropertyContext<'_>) {
        use PropertyKey::*;
        match self {
            Self::Spacing(v) => {
                let c = components(v, context);
                let spacing = if c[2..].iter().all(|v| *v == 0.0) {
                    html_style_model::TextSpacing::new(c[0], c[1])
                } else {
                    context.styles.font_relative_text_spacing_group(
                        context.style.font_group(),
                        [c[0], c[2], c[3], c[4]],
                    )
                }
                .expect("finite prepared spacing");
                if key == PropertyKey::LetterSpacing {
                    context.style.text_mut(context.styles).letter_spacing = spacing;
                } else {
                    context.style.text_mut(context.styles).word_spacing = spacing;
                }
            }
            Self::WhiteSpaceCollapse(value) => {
                context.style.text_mut(context.styles).white_space = value
                    .with_wrapping(context.style.text(context.styles).white_space.allows_wrap())
            }
            Self::TextWrapMode(value) => {
                context.style.text_mut(context.styles).white_space = context
                    .style
                    .text(context.styles)
                    .white_space
                    .with_wrapping(*value)
            }
            Self::TextWrapStyle(value) => {
                context.style.text_mut(context.styles).text_wrap_style = *value
            }
            Self::HangingPunctuation(value) => {
                context.style.text_mut(context.styles).hanging_punctuation = *value
            }
            Self::FontSynthesisSmallCaps(value) => {
                context
                    .style
                    .font_mut(context.styles)
                    .font_synthesis_small_caps = *value;
            }
            Self::FontSynthesisStyle(value) => {
                context.style.font_mut(context.styles).font_synthesis_style = *value;
            }
            Self::Features(values) => {
                let font = context.style.font_mut(context.styles);
                match key {
                    FontVariantCaps => &mut font.font_variant_caps_features,
                    FontFeatureSettings => &mut font.font_feature_settings,
                    FontKerning => &mut font.font_kerning_features,
                    FontVariantLigatures => &mut font.font_variant_ligature_features,
                    FontVariantNumeric => &mut font.font_variant_numeric_features,
                    _ => unreachable!("prepared feature property"),
                }
                .clone_from(values);
            }
            Self::ObjectFit(value) => context.style.box_model.object_fit = *value,
            Self::ObjectPosition(value) => {
                let x = length_pct(
                    &value.x.1,
                    typography::evaluate(
                        &value.x.1,
                        context.style.font(context.styles).font_size,
                        context.root_font_size,
                        context.resolution,
                    ),
                    false,
                );
                let y = length_pct(
                    &value.y.1,
                    typography::evaluate(
                        &value.y.1,
                        context.style.font(context.styles).font_size,
                        context.root_font_size,
                        context.resolution,
                    ),
                    false,
                );
                context.style.box_model.object_position = html_style_model::ObjectPosition {
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
            Self::OutlineOffset(None) => context.style.background.outline.set_offset_inset(),
            Self::OutlineOffset(Some(value)) => {
                let c = typography::evaluate(
                    value,
                    context.style.font(context.styles).font_size,
                    context.root_font_size,
                    context.resolution,
                );
                context
                    .style
                    .background
                    .outline
                    .set_offset(length_pct(value, c, false));
            }

            Self::BreakBetween(value) => {
                *if matches!(key, PropertyKey::BreakBefore) {
                    &mut context.style.layout.break_before
                } else {
                    &mut context.style.layout.break_after
                } = *value
            }
            Self::BreakInside(value) => context.style.layout.break_inside = *value,
            Self::LineCount(value) => {
                *if key == PropertyKey::Widows {
                    &mut context.style.text_mut(context.styles).widows
                } else {
                    &mut context.style.text_mut(context.styles).orphans
                } = *value
            }
            Self::Float(ltr, rtl) => {
                context.style.box_model.float =
                    if context.style.text(context.styles).direction == TextDirection::Ltr {
                        *ltr
                    } else {
                        *rtl
                    }
            }
            Self::Clear(ltr, rtl) => {
                context.style.box_model.clear =
                    if context.style.text(context.styles).direction == TextDirection::Ltr {
                        *ltr
                    } else {
                        *rtl
                    }
            }
            Self::TableLayout(value) => context.style.box_model.table_layout = *value,
            Self::BorderCollapse(value) => context.style.box_model.border_collapse = *value,
            Self::CaptionSide(value) => context.style.box_model.caption_side = *value,
            Self::EmptyCells(value) => context.style.box_model.empty_cells = *value,
            Self::TextBoxTrim(value) => context.style.box_model.text_box_trim = *value,
            Self::TextBoxEdge(value) => {
                context.style.text_mut(context.styles).text_box_edge = *value
            }
            Self::Contain(value) => context.style.box_model.size_containment = value.size,
            Self::Content(value) => {
                context.style.generated_content = value.as_ref().map(|content| GeneratedContent {
                    items: content
                        .items
                        .iter()
                        .map(|item| match item {
                            PreparedContentItem::Text(value) => {
                                GeneratedContentItem::Text(context.styles.intern_string(value))
                            }
                            PreparedContentItem::Attribute(value) => {
                                GeneratedContentItem::Attribute(context.styles.intern_string(value))
                            }
                            PreparedContentItem::Counter { name, style } => {
                                GeneratedContentItem::Counter {
                                    name: context.styles.intern_string(name),
                                    style: *style,
                                }
                            }
                            PreparedContentItem::Counters {
                                name,
                                separator,
                                style,
                            } => GeneratedContentItem::Counters {
                                name: context.styles.intern_string(name),
                                separator: context.styles.intern_string(separator),
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
            Self::Counters(values) => {
                let values = values
                    .iter()
                    .map(|value| CounterDirective {
                        name: context.styles.intern_string(&value.name),
                        value: value.value,
                    })
                    .collect();
                if key == PropertyKey::CounterReset {
                    context.style.counters.resets = values;
                } else {
                    context.style.counters.increments = values;
                }
            }
            Self::Quotes(value) => {
                context.style.text_mut(context.styles).quotes = match value {
                    PreparedQuoteStyle::Auto => QuoteStyle::Auto,
                    PreparedQuoteStyle::None => QuoteStyle::None,
                    PreparedQuoteStyle::Pairs(value) => {
                        QuoteStyle::Pairs(context.styles.intern_string(value))
                    }
                }
            }
            Self::GridTemplate(tracks, names) => {
                let names = grid::intern_names(names, context.styles);
                let tracks = tracks
                    .iter()
                    .map(|track| match track {
                        TemplateTrack::Single(v) => GridTemplateTrack::Single(v.evaluate(context)),
                        TemplateTrack::Repeat(count, tracks, names) => GridTemplateTrack::Repeat {
                            count: *count,
                            tracks: tracks.iter().map(|v| v.evaluate(context)).collect(),
                            line_names: grid::intern_names(names, context.styles),
                        },
                    })
                    .collect();
                if key == PropertyKey::GridTemplateRows {
                    context.style.layout.grid_template_rows = tracks;
                    context.style.layout.grid_template_row_names = names;
                } else {
                    context.style.layout.grid_template_columns = tracks;
                    context.style.layout.grid_template_column_names = names;
                }
            }
            Self::GridAuto(tracks) => {
                let tracks = tracks.iter().map(|v| v.evaluate(context)).collect();
                if key == PropertyKey::GridAutoRows {
                    context.style.layout.grid_auto_rows = tracks;
                } else {
                    context.style.layout.grid_auto_columns = tracks;
                }
            }
            Self::GridPlacement(v) => {
                let value = match v {
                    Placement::Auto => GridPlacement::Auto,
                    Placement::Line(v) => GridPlacement::Line(*v),
                    Placement::Span(v) => GridPlacement::Span(*v),
                    Placement::NamedLine(name, index) => GridPlacement::NamedLine {
                        name: context.styles.intern_string(name),
                        index: *index,
                    },
                    Placement::NamedSpan(name, count) => GridPlacement::NamedSpan {
                        name: context.styles.intern_string(name),
                        count: *count,
                    },
                };
                *match key {
                    PropertyKey::GridRowStart => &mut context.style.layout.grid_row.start,
                    PropertyKey::GridRowEnd => &mut context.style.layout.grid_row.end,
                    PropertyKey::GridColumnStart => &mut context.style.layout.grid_column.start,
                    _ => &mut context.style.layout.grid_column.end,
                } = value;
            }
            Self::GridAreas(areas, rows, columns) => {
                context.style.layout.grid_template_areas =
                    grid::intern_areas(areas, context.styles);
                context.style.layout.grid_template_area_rows = *rows;
                context.style.layout.grid_template_area_columns = *columns;
            }
            Self::BackgroundImage(present) => {
                context.style.background.background_image_present = *present
            }
            Self::Color(value) => {
                let current = if matches!(key, Color) {
                    context
                        .parent
                        .with(context.styles, |parent| parent.text.color)
                } else {
                    context.style.text(context.styles).color
                };
                let color = value.resolve(current);
                match key {
                    Color => context.style.text_mut(context.styles).color = color,
                    BackgroundColor => {
                        context.style.background.background_color = color;
                        context.style.background.background_color_current_color =
                            value.current_color;
                    }
                    TextDecorationColor | OutlineColor => {
                        let decoration = if value.current_color {
                            DecorationColor::CurrentColor
                        } else {
                            DecorationColor::Rgba(color)
                        };
                        if matches!(key, OutlineColor) {
                            context.style.background.outline.color = decoration;
                        } else {
                            context.style.background.text_decoration.color = decoration;
                        }
                    }
                    _ => {
                        let side = physical_side(key, context.style.text(context.styles).direction);
                        values::set_border_color(context.style, side, color);
                        let bit = 1 << side;
                        if value.current_color {
                            context.style.border.current_color_sides |= bit;
                        } else {
                            context.style.border.current_color_sides &= !bit;
                        }
                    }
                }
            }
            Self::BorderStyle(value) => {
                let side = physical_side(key, context.style.text(context.styles).direction);
                values::set_border_style(context.style, side, *value);
            }
            Self::ListStyleImage(value) => {
                context.style.text_mut(context.styles).list_style_image = value
                    .as_deref()
                    .map(|url| context.styles.intern_string(url));
            }
            Self::TextDecorationLine(value) => {
                context.style.background.text_decoration.lines = *value
            }
            Self::LineBreak(value) => context.style.text_mut(context.styles).line_break = *value,
            Self::UnicodeBidi(value) => context.style.box_model.unicode_bidi = *value,
            Self::ListStylePosition(value) => {
                context.style.text_mut(context.styles).list_style_position = *value
            }
            Self::OverflowX(value) => context.style.box_model.overflow_x = *value,
            Self::OverflowY(value) => context.style.box_model.overflow_y = *value,
            Self::TextOverflow(value) => context.style.box_model.text_overflow = *value,
            Self::FlexDirection(value) => context.style.layout.flex_direction = *value,
            Self::FlexWrap(value) => context.style.layout.flex_wrap = *value,
            Self::FlexGrow(value) => context.style.layout.flex_grow = *value,
            Self::FlexShrink(value) => context.style.layout.flex_shrink = *value,
            Self::Order(value) => context.style.layout.order = *value,
            Self::JustifyContent(value) => context.style.layout.justify_content = *value,
            Self::GridAutoFlow(value) => context.style.layout.grid_auto_flow = *value,
            Self::Hyphens(value) => context.style.text_mut(context.styles).hyphens = *value,
            Self::WordBreak(value) => context.style.text_mut(context.styles).word_break = *value,
            Self::OverflowWrap(value) => {
                context.style.text_mut(context.styles).overflow_wrap = *value
            }
            Self::BoxSizing(value) => context.style.box_model.box_sizing = *value,
            Self::Direction(value) => {
                context.style.text_mut(context.styles).direction = *value;
                values::resolve_logical_text_alignments(context.style.text_mut(context.styles));
            }
            Self::Display(v) => {
                context.style.box_model.display = *v;
                context.style.layout.static_position_inline = false;
            }
            Self::ZIndex(v) => context.style.layout.z_index = *v,
            Self::TextAlign(value, _) | Self::TextAlignLast(value) => {
                let (alignment, logical) = match value {
                    PreparedTextAlign::Physical(value) => {
                        (Some(*value), LogicalTextAlign::Physical)
                    }
                    PreparedTextAlign::Start => (
                        Some(values::text_start_alignment(
                            context.style.text(context.styles).direction,
                        )),
                        LogicalTextAlign::Start,
                    ),
                    PreparedTextAlign::End => (
                        Some(values::text_end_alignment(
                            context.style.text(context.styles).direction,
                        )),
                        LogicalTextAlign::End,
                    ),
                    PreparedTextAlign::MatchParent if context.is_root => (
                        Some(values::text_start_alignment(
                            context.style.text(context.styles).direction,
                        )),
                        LogicalTextAlign::Start,
                    ),
                    PreparedTextAlign::MatchParent => (
                        Some(
                            context
                                .parent
                                .with(context.styles, |parent| parent.text.text_align),
                        ),
                        LogicalTextAlign::Physical,
                    ),
                    PreparedTextAlign::Auto => {
                        (None, context.style.text(context.styles).text_align_logical)
                    }
                    PreparedTextAlign::Unspecified => (None, LogicalTextAlign::Physical),
                };
                let text = context.style.text_mut(context.styles);
                if let Self::TextAlign(_, justify_all) = self {
                    text.text_align = alignment.expect("prepared text-align is explicit");
                    text.text_align_logical = logical;
                    text.text_align_justify_all = *justify_all;
                } else {
                    text.text_align_last_explicit = alignment.is_some();
                    text.text_align_last = alignment.unwrap_or(text.text_align);
                    text.text_align_last_logical = logical;
                }
                values::resolve_logical_text_alignments(text);
            }
            Self::FontFamily(value) => {
                context.style.font_mut(context.styles).font_family =
                    Some(context.styles.intern_string(value));
            }
            Self::FontSize(value) => {
                let scale = context.resolution.font_size_scale;
                let parent = finite(
                    f64::from(context.parent.with(context.styles, |p| p.font.font_size))
                        / f64::from(scale),
                );
                let inherited = finite(
                    f64::from(context.style.font(context.styles).font_size) / f64::from(scale),
                );
                let root = finite(f64::from(context.root_font_size) / f64::from(scale));
                let mut c =
                    context
                        .resolution
                        .with_canonical_font_sizes(|resolution| match value {
                            PreparedFontSize::Absolute(v) => [*v, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                            PreparedFontSize::Relative(v) => [
                                finite(f64::from(*v) * f64::from(parent)),
                                0.0,
                                0.0,
                                0.0,
                                0.0,
                                0.0,
                                0.0,
                            ],
                            PreparedFontSize::RootMetric(axis, v) => {
                                let mut c = [0.0; 7];
                                c[*axis] = *v;
                                c
                            }
                            PreparedFontSize::Length(v) => {
                                let basis = if v.literal_axis == Some(0) {
                                    parent
                                } else {
                                    inherited
                                };
                                let c = typography::evaluate(v, basis, root, resolution);
                                [
                                    finite(f64::from(c[0]) + f64::from(c[1]) * f64::from(parent))
                                        .max(0.0),
                                    c[2],
                                    c[3],
                                    c[4],
                                    0.0,
                                    0.0,
                                    0.0,
                                ]
                            }
                        });
                for v in &mut c[..4] {
                    *v = finite(f64::from(*v) * f64::from(scale));
                }
                let font = context.style.font_mut(context.styles);
                [
                    font.font_size,
                    font.font_size_x_height_px,
                    font.font_size_ch_advance_px,
                    font.font_size_cap_height_px,
                    font.font_size_root_ch,
                    font.font_size_root_cap_height,
                    font.font_size_root_line_height,
                ] = c;
                values::rebind_line_height(
                    context.style,
                    context.styles,
                    || context.root_font_size,
                    context.resolution,
                );
            }
            Self::LineHeight(value) => {
                let (px, ex) = value.evaluate(
                    context.style.font(context.styles).font_size,
                    context.root_font_size,
                    context.resolution,
                );
                context.style.line_height_spec = Some(value.clone());
                let text = context.style.text_mut(context.styles);
                text.line_height = px;
                text.line_height_x_height_px = ex;
                text.line_height_number = match value.as_ref() {
                    PreparedLineHeight::Number(v) => *v,
                    _ => 0.0,
                };
                text.line_height_normal = matches!(value.as_ref(), PreparedLineHeight::Normal);
            }
            Self::TabSpaces(v) => {
                context.style.text_mut(context.styles).tab_size =
                    EngineTabSize::spaces(*v).expect("finite prepared tab count")
            }
            Self::TabLength(v) => {
                let c = components(v, context);
                context.style.text_mut(context.styles).tab_size =
                    EngineTabSize::length_px(c[0].max(0.0)).expect("finite prepared tab length");
            }
            Self::Vertical(v) => context.style.box_model.vertical_align = *v,
            Self::VerticalLength(v) => {
                let c = components(v, context);
                context.style.box_model.vertical_align = match v.literal_axis {
                    Some(0) => VerticalAlignValue::Length(c[0]),
                    Some(1) => VerticalAlignValue::Percent(c[1]),
                    _ => VerticalAlignValue::Calc {
                        absolute_px: c[0],
                        line_height_fraction: c[1],
                        x_height_px: c[2],
                    },
                };
            }
            Self::Thickness(v) => context.style.background.text_decoration.thickness = *v,
            Self::ThicknessLength(v) => {
                let c = components(v, context);
                context.style.background.text_decoration.thickness =
                    EngineTextDecorationThickness::length(length_pct(v, c, true))
                        .expect("finite prepared thickness");
            }
            Self::OutlineStyle(v) => context.style.background.outline.style = *v,
            Self::DecorationStyle(v) => context.style.background.text_decoration.style = *v,
            Self::ListStyle(v) => context.style.text_mut(context.styles).list_style_type = *v,
            Self::Size(value) => {
                let value = value.evaluate(context);
                *match key {
                    Width | InlineSize => &mut context.style.box_model.width,
                    Height | BlockSize => &mut context.style.box_model.height,
                    MinWidth | MinInlineSize => &mut context.style.box_model.min_width,
                    MinHeight | MinBlockSize => &mut context.style.box_model.min_height,
                    MaxWidth | MaxInlineSize => &mut context.style.box_model.max_width,
                    MaxHeight | MaxBlockSize => &mut context.style.box_model.max_height,
                    FlexBasis => &mut context.style.layout.flex_basis,
                    _ => unreachable!("prepared size target"),
                } = value;
            }
            Self::TextIndent(value, hanging, each_line) => {
                let c = components(value, context);
                let text = context.style.text_mut(context.styles);
                text.text_indent = length_pct(value, c, false);
                text.text_indent_hanging = *hanging;
                text.text_indent_each_line = *each_line;
            }
            Self::Length(value) => {
                let c = components(value, context);
                let direction = context.style.text(context.styles).direction;
                match key {
                    PaddingTop | PaddingRight | PaddingBottom | PaddingLeft | PaddingBlockStart
                    | PaddingBlockEnd | PaddingInlineStart | PaddingInlineEnd => {
                        values::set_padding_side(
                            context.style,
                            physical_side(key, direction),
                            length_pct(value, c, true),
                        )
                    }
                    BorderTopWidth
                    | BorderRightWidth
                    | BorderBottomWidth
                    | BorderLeftWidth
                    | BorderBlockStartWidth
                    | BorderBlockEndWidth
                    | BorderInlineStartWidth
                    | BorderInlineEndWidth
                    | OutlineWidth => {
                        let width = if value.literal_axis == Some(2) {
                            FontRelativeLength::ex(c[2].max(0.0))
                        } else {
                            FontRelativeLength::px(c[0].max(0.0))
                        }
                        .expect("finite prepared border width");
                        if matches!(key, OutlineWidth) {
                            context.style.background.outline.set_width(width);
                        } else {
                            values::set_border_width(
                                context.style,
                                physical_side(key, direction),
                                width,
                            );
                        }
                    }
                    RowGap => context.style.layout.row_gap = length_pct(value, c, true),
                    ColumnGap => context.style.layout.column_gap = length_pct(value, c, true),
                    _ => unreachable!("prepared length target"),
                }
            }
            Self::AutoLength(value) => {
                let computed = value
                    .as_ref()
                    .map(|v| length_pct(v, components(v, context), false));
                let side = physical_side(key, context.style.text(context.styles).direction);
                if matches!(key, Top | Right | Bottom | Left) {
                    *match side {
                        0 => &mut context.style.layout.inset_top,
                        1 => &mut context.style.layout.inset_right,
                        2 => &mut context.style.layout.inset_bottom,
                        _ => &mut context.style.layout.inset_left,
                    } = computed;
                } else {
                    values::set_margin_side(
                        context.style,
                        side,
                        (computed.unwrap_or(LengthPct::Px(0.0)), value.is_none()),
                    );
                }
            }
            Self::Pair(a, b) => {
                let ac = components(a, context);
                let bc = components(b, context);
                if matches!(key, BorderSpacing) {
                    context.style.box_model.border_spacing_horizontal = ac[0].max(0.0);
                    context.style.box_model.border_spacing_vertical = bc[0].max(0.0);
                } else {
                    use html_style_model::TextDirection::{Ltr, Rtl};
                    let corner = match (key, context.style.text(context.styles).direction) {
                        (BorderTopLeftRadius, _)
                        | (BorderStartStartRadius, Ltr)
                        | (BorderStartEndRadius, Rtl) => 0,
                        (BorderTopRightRadius, _)
                        | (BorderStartEndRadius, Ltr)
                        | (BorderStartStartRadius, Rtl) => 1,
                        (BorderBottomRightRadius, _)
                        | (BorderEndEndRadius, Ltr)
                        | (BorderEndStartRadius, Rtl) => 2,
                        _ => 3,
                    };
                    values::set_corner_radius(
                        context.style,
                        corner,
                        html_style_model::CornerRadius {
                            x: length_pct(a, ac, true),
                            y: length_pct(b, bc, true),
                        },
                    );
                }
            }
            Self::Position(v) => context.style.layout.position = *v,
            Self::AspectRatio(v) => context.style.box_model.aspect_ratio = *v,
            Self::ContentAlignment(v) => context.style.layout.align_content = *v,
            Self::Alignment(v) => {
                *match key {
                    AlignItems => &mut context.style.layout.align_items,
                    AlignSelf => &mut context.style.layout.align_self,
                    JustifyItems => &mut context.style.layout.justify_items,
                    _ => &mut context.style.layout.justify_self,
                } = *v;
            }
            Self::FontStretch(v) => context.style.font_mut(context.styles).font_stretch = *v,
            Self::FontStyle(v, angle) => {
                let font = context.style.font_mut(context.styles);
                font.font_style = *v;
                font.font_oblique_angle = *angle;
            }
            Self::FontWeight(v) => {
                let parent = context.parent.with(context.styles, |p| p.font.font_weight);
                let weight = match v {
                    PreparedWeight::Absolute(v) => *v,
                    PreparedWeight::Bolder => match parent {
                        0..=349 => 400,
                        350..=549 => 700,
                        _ => 900,
                    },
                    PreparedWeight::Lighter => match parent {
                        0..=549 => 100,
                        550..=749 => 400,
                        _ => 700,
                    },
                };
                context.style.font_mut(context.styles).font_weight = weight;
            }
            Self::Visibility(v) => context.style.text_mut(context.styles).visibility = *v,
            Self::TextTransform(v) => context.style.text_mut(context.styles).text_transform = *v,
        }
    }
}

fn components(value: &PreparedLength, context: &mut PropertyContext<'_>) -> [f32; 5] {
    typography::evaluate(
        value,
        context.style.font(context.styles).font_size,
        context.root_font_size,
        context.resolution,
    )
}

pub(super) fn length_pct(value: &PreparedLength, c: [f32; 5], nonnegative: bool) -> LengthPct {
    let scalar = |v: f32| if nonnegative { v.max(0.0) } else { v };
    match value.literal_axis {
        Some(0) => LengthPct::Px(scalar(c[0])),
        Some(1) => LengthPct::Pct(scalar(c[1])),
        Some(2) => LengthPct::Ex(scalar(c[2])),
        Some(3) => LengthPct::Ch(scalar(c[3])),
        Some(4) => LengthPct::Cap(scalar(c[4])),
        _ if !value.percentage_dependent && c[2..].iter().all(|v| *v == 0.0) => {
            LengthPct::Px(scalar(c[0]))
        }
        _ => LengthPct::Calc {
            absolute_px: c[0],
            percentage: c[1],
            x_height_px: c[2],
            ch_advance_px: c[3],
            cap_height_px: c[4],
            percentage_dependent: value.percentage_dependent,
        },
    }
}

pub(crate) type PreparedLonghand = (PropertyKey, PreparedValue);

/// Shared value preparation for stored, substituted, and queried declarations.
#[derive(Default)]
pub(crate) struct PreparedDeclarationValues {
    pub(crate) values: Vec<PreparedLonghand>,
}

impl PreparedDeclarationValues {
    /// Appends expanded longhands to the caller's reusable pool. On error the
    /// caller discards or truncates any appended longhands.
    pub(crate) fn prepare<'css>(
        property: &Property<'css>,
        longhands: &mut Vec<Property<'css>>,
    ) -> Result<Self, PreparationError> {
        let mut values = Vec::new();
        if PreparedValue::prepare_renderer(property, |key, value| {
            values.push((key, value));
        })? {
            return Ok(Self { values });
        }
        let start = longhands.len();
        let expanded = !matches!(property, Property::Unparsed(_) | Property::Custom(_))
            && property_css_wide_keyword(property).is_none()
            && expand_declaration(property, longhands)
                .map_err(|()| PreparationError::Unsupported(UnsupportedStyleFeature::Value))?;
        let properties = if expanded {
            &longhands[start..]
        } else {
            std::slice::from_ref(property)
        };
        if matches!(property, Property::Font(_)) {
            for key in FONT_RESET_TARGETS {
                values.push((
                    key,
                    PreparedValue::Features(Vec::new()),
                ));
            }
        }
        for property in properties {
            if let Property::WhiteSpace(value) = property {
                use lightningcss::properties::text::WhiteSpace as CssWhiteSpace;
                let value = match value {
                    CssWhiteSpace::Normal => EngineWhiteSpace::Normal,
                    CssWhiteSpace::Pre => EngineWhiteSpace::Pre,
                    CssWhiteSpace::NoWrap => EngineWhiteSpace::NoWrap,
                    CssWhiteSpace::PreWrap => EngineWhiteSpace::PreWrap,
                    CssWhiteSpace::PreLine => EngineWhiteSpace::PreLine,
                    CssWhiteSpace::BreakSpaces => EngineWhiteSpace::BreakSpaces,
                };
                for (key, value) in [
                    (
                        PropertyKey::WhiteSpaceCollapse,
                        PreparedValue::WhiteSpaceCollapse(value),
                    ),
                    (
                        PropertyKey::TextWrapMode,
                        PreparedValue::TextWrapMode(value.allows_wrap()),
                    ),
                ] {
                    values.push((key, value));
                }
                continue;
            }
            if let Some(value) = PreparedValue::prepare(property)
                .map_err(|()| PreparationError::Unsupported(UnsupportedStyleFeature::Value))?
            {
                values.push((
                    PropertyKey::from_name(property.property_id().name())
                        .expect("prepared values property key"),
                    value,
                ));
            }
        }
        Ok(Self { values })
    }

    pub(crate) fn requires_viewport_height(&self) -> bool {
        self.values
            .iter()
            .any(|(_, value)| value.requires_viewport_height())
    }
}
