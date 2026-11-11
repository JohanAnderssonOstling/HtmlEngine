use super::super::context::{PropertyContext, ResolutionContext};
use super::super::values;
use super::{components, length_pct};
use crate::style::syntax::prepared_length::{Basis, LengthMode, PreparedLength, finite};
use crate::style::syntax::renderer_owned;
use html_style_model::{
    BorderStyle, ListStyleType, TabSize, TextDecorationStyle, TextDecorationThickness, TextSpacing,
    VerticalAlignValue,
};
use lightningcss::properties::Property;
use lightningcss::properties::font::{FontSize, LineHeight, VerticalAlign};
use lightningcss::values::length::{LengthOrNumber, LengthPercentage, LengthValue};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) enum PreparedLineHeight {
    Normal,
    Number(f32),
    Length(PreparedLength),
}
impl PreparedLineHeight {
    fn prepare(value: &LineHeight) -> Option<Self> {
        Some(match value {
            LineHeight::Normal => Self::Normal,
            LineHeight::Number(v) if v.is_finite() => Self::Number(v.max(0.0)),
            LineHeight::Number(_) => return None,
            LineHeight::Length(v) => {
                Self::Length(PreparedLength::percentage(v, LengthMode::Ex)?.font_components(true)?)
            }
        })
    }
    pub(in crate::style::cascade::resolver) fn evaluate(
        &self,
        font: f32,
        root: f32,
        resolution: &mut ResolutionContext,
    ) -> (f32, f32) {
        let scale = resolution.font_size_scale;
        let (px, ex) = resolution.with_canonical_font_sizes(|resolution| {
            let font = finite(f64::from(font) / f64::from(scale));
            let root = finite(f64::from(root) / f64::from(scale));
            match self {
                Self::Normal => (0.0, 0.0),
                Self::Number(v) => (finite(f64::from(font) * f64::from(*v)), 0.0),
                Self::Length(v) => {
                    let c = evaluate(v, font, root, resolution);
                    (
                        finite(f64::from(c[0]) + f64::from(c[1]) * f64::from(font)).max(0.0),
                        c[2].max(0.0),
                    )
                }
            }
        });
        (
            finite(f64::from(px) * f64::from(scale)),
            finite(f64::from(ex) * f64::from(scale)),
        )
    }
    fn needs_height(&self) -> bool {
        matches!(self,Self::Length(v) if v.viewport_height)
    }
}
#[derive(Clone, Debug)]
pub(crate) enum PreparedFontSize {
    Length(PreparedLength),
    Absolute(f32),
    Relative(f32),
    RootMetric(usize, f32),
}
#[derive(Clone, Debug)]
pub(crate) enum PreparedTypography {
    FontSize(PreparedFontSize),
    LineHeight(Arc<PreparedLineHeight>),
    TabSpaces(f32),
    TabLength(PreparedLength),
    Spacing(PreparedLength),
    Vertical(VerticalAlignValue),
    VerticalLength(PreparedLength),
    Thickness(TextDecorationThickness),
    ThicknessLength(PreparedLength),
    OutlineStyle(BorderStyle),
    DecorationStyle(TextDecorationStyle),
    ListStyle(ListStyleType),
}
impl PreparedTypography {
    pub(crate) fn prepare(property: &Property<'_>) -> Result<Option<Self>, ()> {
        Ok(Some(match property {
            Property::FontSize(v) => Self::FontSize(match v {
                FontSize::Absolute(v) => {
                    use lightningcss::properties::font::AbsoluteFontSize::*;
                    PreparedFontSize::Absolute(match v {
                        XXSmall => 9.0,
                        XSmall => 10.0,
                        Small => 13.0,
                        Medium => 16.0,
                        Large => 18.0,
                        XLarge => 24.0,
                        XXLarge => 32.0,
                        XXXLarge => 48.0,
                    })
                }
                FontSize::Relative(v) => PreparedFontSize::Relative(match v {
                    lightningcss::properties::font::RelativeFontSize::Smaller => 0.833,
                    _ => 1.2,
                }),
                FontSize::Length(LengthPercentage::Dimension(
                    v @ (LengthValue::Rch(_) | LengthValue::Rcap(_) | LengthValue::Rlh(_)),
                )) => {
                    let (n, _) = v.to_unit_value();
                    if !n.is_finite() || n < 0.0 {
                        return Err(());
                    }
                    PreparedFontSize::RootMetric(
                        match v {
                            LengthValue::Rch(_) => 4,
                            LengthValue::Rcap(_) => 5,
                            _ => 6,
                        },
                        n,
                    )
                }
                FontSize::Length(v) => PreparedFontSize::Length(
                    PreparedLength::percentage(v, LengthMode::Font)
                        .and_then(|v| v.font_components(false))
                        .ok_or(())?,
                ),
            }),
            Property::LineHeight(v) => {
                Self::LineHeight(Arc::new(PreparedLineHeight::prepare(v).ok_or(())?))
            }
            Property::TabSize(LengthOrNumber::Number(v), _) => {
                if !v.is_finite() {
                    return Err(());
                }
                Self::TabSpaces(v.max(0.0))
            }
            Property::TabSize(LengthOrNumber::Length(v), _) => {
                Self::TabLength(PreparedLength::length(v, LengthMode::Scalar).ok_or(())?)
            }
            Property::LetterSpacing(v) | Property::WordSpacing(v) => Self::Spacing(match v {
                lightningcss::properties::text::Spacing::Normal => PreparedLength::zero(),
                lightningcss::properties::text::Spacing::Length(v) => {
                    PreparedLength::length(v, LengthMode::Spacing).ok_or(())?
                }
            }),
            Property::VerticalAlign(VerticalAlign::Length(v)) => {
                Self::VerticalLength(PreparedLength::percentage(v, LengthMode::Ex).ok_or(())?)
            }
            Property::VerticalAlign(VerticalAlign::Keyword(v)) => {
                use lightningcss::properties::font::VerticalAlignKeyword as K;
                Self::Vertical(match v {
                    K::Baseline => VerticalAlignValue::Baseline,
                    K::Sub => VerticalAlignValue::Sub,
                    K::Super => VerticalAlignValue::Super,
                    K::Top => VerticalAlignValue::Top,
                    K::TextTop => VerticalAlignValue::TextTop,
                    K::Middle => VerticalAlignValue::Middle,
                    K::Bottom => VerticalAlignValue::Bottom,
                    K::TextBottom => VerticalAlignValue::TextBottom,
                })
            }
            Property::TextDecorationThickness(v) => {
                use lightningcss::properties::text::TextDecorationThickness as T;
                match v {
                    T::Auto => Self::Thickness(TextDecorationThickness::Auto),
                    T::FromFont => Self::Thickness(TextDecorationThickness::FromFont),
                    T::LengthPercentage(v) => Self::ThicknessLength(
                        PreparedLength::percentage(v, LengthMode::Affine).ok_or(())?,
                    ),
                }
            }
            Property::OutlineStyle(v) => {
                Self::OutlineStyle(renderer_owned::outline_style(v).ok_or(())?)
            }
            Property::TextDecorationStyle(v, _) => {
                Self::DecorationStyle(renderer_owned::text_decoration_style(v).ok_or(())?)
            }
            Property::ListStyleType(v) => {
                Self::ListStyle(renderer_owned::list_style_type(v).ok_or(())?)
            }
            _ => return Ok(None),
        }))
    }
    pub(crate) fn requires_viewport_height(&self) -> bool {
        match self {
            Self::FontSize(PreparedFontSize::Length(v))
            | Self::TabLength(v)
            | Self::Spacing(v)
            | Self::VerticalLength(v)
            | Self::ThicknessLength(v) => v.viewport_height,
            Self::LineHeight(v) => v.needs_height(),
            _ => false,
        }
    }
    pub(in crate::style::cascade::resolver) fn apply(
        &self,
        property: &Property<'_>,
        context: &mut PropertyContext<'_>,
    ) {
        match self {
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
                                let c = evaluate(v, basis, root, resolution);
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
                    TabSize::spaces(*v).expect("finite prepared tab count")
            }
            Self::TabLength(v) => {
                let c = components(v, context);
                context.style.text_mut(context.styles).tab_size =
                    TabSize::length_px(c[0].max(0.0)).expect("finite prepared tab length");
            }
            Self::Spacing(v) => {
                let c = components(v, context);
                let spacing = if c[2..].iter().all(|v| *v == 0.0) {
                    TextSpacing::from_px(c[0])
                } else {
                    context.styles.font_relative_text_spacing_group(
                        context.style.font_group(),
                        [c[0], c[2], c[3], c[4]],
                    )
                }
                .expect("finite prepared spacing");
                if matches!(property, Property::LetterSpacing(_)) {
                    context.style.text_mut(context.styles).letter_spacing = spacing;
                } else {
                    context.style.text_mut(context.styles).word_spacing = spacing;
                }
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
                    TextDecorationThickness::length(length_pct(v, c, true))
                        .expect("finite prepared thickness");
            }
            Self::OutlineStyle(v) => context.style.background.outline.style = *v,
            Self::DecorationStyle(v) => context.style.background.text_decoration.style = *v,
            Self::ListStyle(v) => context.style.text_mut(context.styles).list_style_type = *v,
        }
    }
}
pub(in crate::style::cascade::resolver) fn evaluate(
    value: &PreparedLength,
    font: f32,
    root: f32,
    resolution: &mut ResolutionContext,
) -> [f32; 5] {
    resolution.uses_viewport_units |= value.viewport;
    let width = finite(resolution.environment.viewport_width());
    let height = resolution
        .environment
        .viewport_height()
        .map(finite)
        .unwrap_or(0.0);
    value.components(|basis| match basis {
        Basis::Font => font,
        Basis::RootFont => root,
        Basis::Line => resolution.line_height.expect("bound line height"),
        Basis::RootLine => resolution.root_line_height.expect("bound root line height"),
        Basis::ViewWidth => width,
        Basis::ViewHeight => height,
        Basis::ViewMin => width.min(height),
        Basis::ViewMax => width.max(height),
    })
}
