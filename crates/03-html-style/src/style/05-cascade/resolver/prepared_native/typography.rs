use super::super::context::ResolutionContext;
use super::PreparedValue;
use crate::style::syntax::prepared_length::{Basis, LengthMode, PreparedLength, finite};
use crate::style::syntax::renderer_owned;
use html_style_model::{
    BorderStyle, ListStyleType, TextAlign, TextDecorationStyle, TextDecorationThickness,
    VerticalAlignValue,
};
use lightningcss::printer::{Printer, PrinterOptions};
use lightningcss::properties::Property;
use lightningcss::traits::ToCss;
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
    pub(super) fn needs_height(&self) -> bool {
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
#[derive(Clone, Copy, Debug)]
pub(crate) enum PreparedTextAlign {
    Physical(TextAlign),
    Start,
    End,
    MatchParent,
    Auto,
    Unspecified,
}

impl PreparedValue {
    pub(crate) fn prepare_typography(property: &Property<'_>) -> Result<Option<Self>, ()> {
        Ok(Some(match property {
            Property::TextAlign(value) => {
                use lightningcss::properties::text::TextAlign as Css;
                Self::TextAlign(
                    match value {
                        Css::Left => PreparedTextAlign::Physical(TextAlign::Left),
                        Css::Right => PreparedTextAlign::Physical(TextAlign::Right),
                        Css::Center => PreparedTextAlign::Physical(TextAlign::Center),
                        Css::Justify | Css::JustifyAll => {
                            PreparedTextAlign::Physical(TextAlign::Justify)
                        }
                        Css::Start => PreparedTextAlign::Start,
                        Css::End => PreparedTextAlign::End,
                        Css::MatchParent => PreparedTextAlign::MatchParent,
                    },
                    matches!(value, Css::JustifyAll),
                )
            }
            Property::TextAlignLast(value, _) => {
                use lightningcss::properties::text::TextAlignLast as Css;
                Self::TextAlignLast(match value {
                    Css::Auto => PreparedTextAlign::Auto,
                    Css::Left => PreparedTextAlign::Physical(TextAlign::Left),
                    Css::Right => PreparedTextAlign::Physical(TextAlign::Right),
                    Css::Center => PreparedTextAlign::Physical(TextAlign::Center),
                    Css::Justify => PreparedTextAlign::Physical(TextAlign::Justify),
                    Css::Start => PreparedTextAlign::Start,
                    Css::End => PreparedTextAlign::End,
                    _ => PreparedTextAlign::Unspecified,
                })
            }
            Property::FontFamily(families) => {
                let mut out = String::new();
                for (i, family) in families.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    let _ = family.to_css(&mut Printer::new(&mut out, PrinterOptions::default()));
                }
                Self::FontFamily(out.into_boxed_str())
            }
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
