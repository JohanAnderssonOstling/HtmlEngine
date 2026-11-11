//! Native declarations converted once, before selection, into supported values.
mod grid;
pub(crate) mod typography;
use super::context::PropertyContext;
use super::values;
use crate::style::syntax::prepared_length::{LengthMode, PreparedLength, finite};
use grid::PreparedGrid;
use html_style_model::Display as EngineDisplay;
use html_style_model::{
    AspectRatio, ComputedSizeComponent, ContentAlignment, FontRelativeLength, FontStyle,
    ItemAlignment, LengthPct, PositionMode, PreferredSize, SizeComparison, TextTransform,
    Visibility,
};
use lightningcss::properties::Property;
use lightningcss::properties::size::{MaxSize, Size};
use lightningcss::values::calc::{Calc, MathFunction};
use lightningcss::values::length::{LengthPercentage, LengthPercentageOrAuto};
use typography::PreparedTypography;

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

#[derive(Clone, Debug)]
pub(crate) enum PreparedNativeValue {
    Typography(PreparedTypography),
    Grid(PreparedGrid),
    Display(html_style_model::Display),
    ZIndex(i32),
    Size(PreparedSize),
    Length(PreparedLength),
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
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum PreparedWeight {
    Absolute(u16),
    Bolder,
    Lighter,
}

impl PreparedNativeValue {
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
                    NativeFallback::ZIndex(v) => Self::ZIndex(v),
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
            TextIndent(v) => Self::Length(length(&v.value)?),
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
                if let Some(v) = PreparedTypography::prepare(property)? {
                    return Ok(Some(Self::Typography(v)));
                }
                return PreparedGrid::prepare(property).map(|v| v.map(Self::Grid));
            }
        }))
    }
    pub(crate) fn requires_viewport_height(&self) -> bool {
        match self {
            Self::Grid(v) => v.requires_viewport_height(),
            Self::Typography(v) => v.requires_viewport_height(),
            Self::Size(v) => v.needs_height(),
            Self::Length(v) => v.viewport_height,
            Self::AutoLength(v) => v.as_ref().is_some_and(|v| v.viewport_height),
            Self::Pair(a, b) => a.viewport_height || b.viewport_height,
            _ => false,
        }
    }
    pub(super) fn apply(&self, property: &Property<'_>, context: &mut PropertyContext<'_>) {
        use Property::*;
        match self {
            Self::Grid(v) => v.apply(property, context),
            Self::Display(v) => {
                context.style.box_model.display = *v;
                context.style.layout.static_position_inline = false;
            }
            Self::ZIndex(v) => context.style.layout.z_index = Some(*v),
            Self::Typography(v) => v.apply(property, context),
            Self::Size(value) => {
                let value = value.evaluate(context);
                *match property {
                    Width(_) | InlineSize(_) => &mut context.style.box_model.width,
                    Height(_) | BlockSize(_) => &mut context.style.box_model.height,
                    MinWidth(_) | MinInlineSize(_) => &mut context.style.box_model.min_width,
                    MinHeight(_) | MinBlockSize(_) => &mut context.style.box_model.min_height,
                    MaxWidth(_) | MaxInlineSize(_) => &mut context.style.box_model.max_width,
                    MaxHeight(_) | MaxBlockSize(_) => &mut context.style.box_model.max_height,
                    FlexBasis(..) | Unparsed(_) => &mut context.style.layout.flex_basis,
                    _ => unreachable!("prepared size target"),
                } = value;
            }
            Self::Length(value) => {
                let c = components(value, context);
                let direction = context.style.text(context.styles).direction;
                match property {
                    PaddingTop(_)
                    | PaddingRight(_)
                    | PaddingBottom(_)
                    | PaddingLeft(_)
                    | PaddingBlockStart(_)
                    | PaddingBlockEnd(_)
                    | PaddingInlineStart(_)
                    | PaddingInlineEnd(_) => values::set_padding_side(
                        context.style,
                        super::properties::box_model::physical_side(property, direction),
                        length_pct(value, c, true),
                    ),
                    BorderTopWidth(_)
                    | BorderRightWidth(_)
                    | BorderBottomWidth(_)
                    | BorderLeftWidth(_)
                    | BorderBlockStartWidth(_)
                    | BorderBlockEndWidth(_)
                    | BorderInlineStartWidth(_)
                    | BorderInlineEndWidth(_)
                    | OutlineWidth(_) => {
                        let width = if value.literal_axis == Some(2) {
                            FontRelativeLength::ex(c[2].max(0.0))
                        } else {
                            FontRelativeLength::px(c[0].max(0.0))
                        }
                        .expect("finite prepared border width");
                        if matches!(property, OutlineWidth(_)) {
                            context.style.background.outline.set_width(width);
                        } else {
                            values::set_border_width(
                                context.style,
                                super::properties::box_model::physical_side(property, direction),
                                width,
                            );
                        }
                    }
                    TextIndent(v) => {
                        let text = context.style.text_mut(context.styles);
                        text.text_indent = length_pct(value, c, false);
                        text.text_indent_hanging = v.hanging;
                        text.text_indent_each_line = v.each_line;
                    }
                    RowGap(_) => context.style.layout.row_gap = length_pct(value, c, true),
                    ColumnGap(_) => context.style.layout.column_gap = length_pct(value, c, true),
                    _ => unreachable!("prepared length target"),
                }
            }
            Self::AutoLength(value) => {
                let computed = value
                    .as_ref()
                    .map(|v| length_pct(v, components(v, context), false));
                let side = super::properties::box_model::physical_side(
                    property,
                    context.style.text(context.styles).direction,
                );
                if matches!(property, Top(_) | Right(_) | Bottom(_) | Left(_)) {
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
                if matches!(property, BorderSpacing(_)) {
                    context.style.box_model.border_spacing_horizontal = ac[0].max(0.0);
                    context.style.box_model.border_spacing_vertical = bc[0].max(0.0);
                } else {
                    use html_style_model::TextDirection::{Ltr, Rtl};
                    let corner = match (property, context.style.text(context.styles).direction) {
                        (BorderTopLeftRadius(..), _)
                        | (BorderStartStartRadius(_), Ltr)
                        | (BorderStartEndRadius(_), Rtl) => 0,
                        (BorderTopRightRadius(..), _)
                        | (BorderStartEndRadius(_), Ltr)
                        | (BorderStartStartRadius(_), Rtl) => 1,
                        (BorderBottomRightRadius(..), _)
                        | (BorderEndEndRadius(_), Ltr)
                        | (BorderEndStartRadius(_), Rtl) => 2,
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
                *match property {
                    AlignItems(..) => &mut context.style.layout.align_items,
                    AlignSelf(..) => &mut context.style.layout.align_self,
                    JustifyItems(_) => &mut context.style.layout.justify_items,
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

/// Omit allocation entirely for declarations handled by infallible mappings.
pub(crate) fn prepare_declaration(
    properties: &[Property<'_>],
) -> Result<Vec<Option<PreparedNativeValue>>, ()> {
    let mut values = Vec::new();
    for (index, property) in properties.iter().enumerate() {
        if let Some(value) = PreparedNativeValue::prepare(property)? {
            values.resize_with(index + 1, || None);
            values[index] = Some(value);
        }
    }
    Ok(values)
}
