use super::super::context::ResolutionContext;
use html_style_model::{
    BorderStyle, ComputedSizeComponent, ComputedStylesBuilder, Font, FontRelativeLength, LengthPct,
    PreferredSize, SizeComparison, TextSpacing, VerticalAlignValue,
};
use lightningcss::properties::border::{BorderSideWidth, LineStyle};
use lightningcss::properties::size::{MaxSize, Size};
use lightningcss::properties::text::Spacing;
use lightningcss::values::calc::{Calc, MathFunction};
use lightningcss::values::length::{Length, LengthPercentage, LengthPercentageOrAuto, LengthValue};

pub(in crate::style::cascade::resolver) fn length_to_px(
    length: &LengthValue,
    parent_font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    Some(match length {
        LengthValue::Px(px) => *px,
        LengthValue::Em(em) => em * parent_font_size,
        LengthValue::Rem(rem) => rem * root_font_size,
        LengthValue::Lh(lh) => lh * resolution.line_height.get()?,
        LengthValue::Rlh(rlh) => rlh * resolution.root_line_height.get()?,
        LengthValue::Pt(pt) => pt * (96.0 / 72.0),
        LengthValue::In(inches) => inches * 96.0,
        LengthValue::Cm(cm) => cm * (96.0 / 2.54),
        LengthValue::Mm(mm) => mm * (96.0 / 25.4),
        LengthValue::Q(q) => q * (96.0 / 101.6),
        LengthValue::Pc(pc) => pc * 16.0,
        // These require selected-face metrics that are deliberately absent
        // during computed-style construction. Callers either preserve `ex`
        // symbolically or reject the declaration; they never guess a value.
        LengthValue::Ex(_) | LengthValue::Ch(_) | LengthValue::Cap(_) => return None,
        _ => return viewport_length_to_px(length, resolution),
    })
}

pub(in crate::style::cascade::resolver) fn length_pct_from_length(
    length: &LengthValue,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<LengthPct> {
    match length {
        LengthValue::Ex(ex) => Some(LengthPct::Ex(ex * font_size)),
        LengthValue::Ch(ch) => Some(LengthPct::Ch(ch * font_size)),
        LengthValue::Cap(cap) => Some(LengthPct::Cap(cap * font_size)),
        _ => Some(LengthPct::Px(length_to_px(
            length,
            font_size,
            root_font_size,
            resolution,
        )?)),
    }
}

pub(in crate::style::cascade::resolver) fn preferred_from_length(
    length: &LengthValue,
    font: &Font,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<PreferredSize> {
    match length {
        LengthValue::Ex(ex) => Some(PreferredSize::Ex(ex * font.font_size)),
        LengthValue::Ch(ch) => Some(PreferredSize::Ch(ch * font.font_size)),
        LengthValue::Cap(cap) => Some(PreferredSize::Cap(cap * font.font_size)),
        LengthValue::Em(em)
            if font.font_size_x_height_px != 0.0
                || font.font_size_ch_advance_px != 0.0
                || font.font_size_cap_height_px != 0.0 =>
        {
            Some(PreferredSize::Calc {
                absolute_px: em * font.font_size,
                percentage: 0.0,
                x_height_px: em * font.font_size_x_height_px,
                ch_advance_px: em * font.font_size_ch_advance_px,
                cap_height_px: em * font.font_size_cap_height_px,
                percentage_dependent: false,
            })
        }
        _ => Some(PreferredSize::Px(length_to_px(
            length,
            font.font_size,
            root_font_size,
            resolution,
        )?)),
    }
}

pub(in crate::style::cascade::resolver) fn border_length_from_value(
    length: &LengthValue,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<FontRelativeLength> {
    match length {
        LengthValue::Ex(ex) => FontRelativeLength::ex((ex * font_size).max(0.0)),
        _ => FontRelativeLength::px(
            length_to_px(length, font_size, root_font_size, resolution)?.max(0.0),
        ),
    }
}

pub(in crate::style::cascade::resolver) fn length_percentage_to_px(
    lp: &LengthPercentage,
    parent_font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    match lp {
        LengthPercentage::Dimension(len) => {
            length_to_px(len, parent_font_size, root_font_size, resolution)
        }
        LengthPercentage::Percentage(p) => Some(parent_font_size * p.0),
        LengthPercentage::Calc(value) => {
            let (px, fraction) = calc_length_percentage_components(
                value,
                parent_font_size,
                root_font_size,
                resolution,
            )?;
            Some(px + parent_font_size * fraction)
        }
    }
}

pub(in crate::style::cascade::resolver) fn length_percentage_to_text_spacing(
    value: &LengthPercentage,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<TextSpacing> {
    use lightningcss::values::percentage::DimensionPercentage;
    match value {
        DimensionPercentage::Dimension(length) => {
            TextSpacing::from_px(length_to_px(length, font_size, root_font_size, resolution)?)
        }
        DimensionPercentage::Percentage(percentage) => TextSpacing::new(0.0, percentage.0),
        DimensionPercentage::Calc(calc) => {
            let (absolute_px, font_size_fraction) =
                calc_length_percentage_components(calc, font_size, root_font_size, resolution)?;
            TextSpacing::new(absolute_px, font_size_fraction)
        }
    }
}

pub(in crate::style::cascade::resolver) fn vertical_align_value(
    value: &LengthPercentage,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<VerticalAlignValue> {
    match value {
        LengthPercentage::Dimension(LengthValue::Ex(value)) => Some(VerticalAlignValue::Calc {
            absolute_px: 0.0,
            line_height_fraction: 0.0,
            x_height_px: value * font_size,
        }),
        LengthPercentage::Dimension(length) => Some(VerticalAlignValue::Length(length_to_px(
            length,
            font_size,
            root_font_size,
            resolution,
        )?)),
        LengthPercentage::Percentage(percentage) => Some(VerticalAlignValue::Percent(percentage.0)),
        LengthPercentage::Calc(calc) => {
            let [
                absolute_px,
                line_height_fraction,
                x_height_px,
                ch_advance_px,
                cap_height_px,
            ] = calc_box_length_percentage_components(calc, font_size, root_font_size, resolution)?;
            if ch_advance_px != 0.0 || cap_height_px != 0.0 {
                return None;
            }
            Some(VerticalAlignValue::Calc {
                absolute_px,
                line_height_fraction,
                x_height_px,
            })
        }
    }
}

/// Reduce Lightning CSS's typed linear calc tree into coefficients for pixels,
/// percentages, and the selected-face metrics `ex`, `ch`, and `cap`.
pub(in crate::style::cascade::resolver) fn calc_box_length_percentage_components(
    calc: &Calc<LengthPercentage>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<[f32; 5]> {
    let resolve =
        |value| calc_box_length_percentage_components(value, font_size, root_font_size, resolution);
    match calc {
        Calc::Value(value) => match value.as_ref() {
            LengthPercentage::Dimension(LengthValue::Ex(value)) => {
                Some([0.0, 0.0, value * font_size, 0.0, 0.0])
            }
            LengthPercentage::Dimension(LengthValue::Ch(value)) => {
                Some([0.0, 0.0, 0.0, value * font_size, 0.0])
            }
            LengthPercentage::Dimension(LengthValue::Cap(value)) => {
                Some([0.0, 0.0, 0.0, 0.0, value * font_size])
            }
            LengthPercentage::Dimension(value) => Some([
                length_to_px(value, font_size, root_font_size, resolution)?,
                0.0,
                0.0,
                0.0,
                0.0,
            ]),
            LengthPercentage::Percentage(value) => Some([0.0, value.0, 0.0, 0.0, 0.0]),
            LengthPercentage::Calc(value) => resolve(value),
        },
        Calc::Number(value) => (*value == 0.0).then_some([0.0; 5]),
        Calc::Sum(left, right) => {
            let left = resolve(left)?;
            let right = resolve(right)?;
            Some(std::array::from_fn(|i| left[i] + right[i]))
        }
        Calc::Product(factor, value) => Some(resolve(value)?.map(|value| *factor * value)),
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => resolve(value),
            MathFunction::Min(values) | MathFunction::Max(values) => {
                let minimum = matches!(function.as_ref(), MathFunction::Min(_));
                values
                    .iter()
                    .map(resolve)
                    .reduce(|left, right| select_comparison_operand(left?, right?, minimum))?
            }
            MathFunction::Clamp(minimum, value, maximum) => {
                let minimum = resolve(minimum)?;
                let value = resolve(value)?;
                let maximum = resolve(maximum)?;
                select_comparison_operand(
                    minimum,
                    select_comparison_operand(value, maximum, true)?,
                    false,
                )
            }
            _ => None,
        },
    }
}

/// Affine lengths can be ordered without a containing-block or font metric
/// when they differ in just one coefficient. All five bases are nonnegative.
/// Crossing expressions stay unresolved instead of comparing coefficients as
/// if they were already pixel values.
fn select_comparison_operand(left: [f32; 5], right: [f32; 5], minimum: bool) -> Option<[f32; 5]> {
    let mut difference = None;
    for (a, b) in left.into_iter().zip(right) {
        if a != b {
            if difference.is_some() {
                return None;
            }
            difference = Some(a.partial_cmp(&b)?);
        }
    }
    let choose_left = match difference {
        Some(order) if minimum => order.is_lt(),
        Some(order) => order.is_gt(),
        None => true,
    };
    Some(if choose_left { left } else { right })
}

pub(in crate::style::cascade::resolver) fn calc_length_percentage_components(
    calc: &Calc<LengthPercentage>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<(f32, f32)> {
    let resolve =
        |value| calc_length_percentage_components(value, font_size, root_font_size, resolution);
    match calc {
        Calc::Value(value) => {
            let spacing =
                length_percentage_to_text_spacing(value, font_size, root_font_size, resolution)?;
            Some((spacing.absolute_px(), spacing.font_size_fraction()))
        }
        Calc::Number(value) => (*value == 0.0).then_some((0.0, 0.0)),
        Calc::Sum(left, right) => {
            let left = resolve(left)?;
            let right = resolve(right)?;
            Some((left.0 + right.0, left.1 + right.1))
        }
        Calc::Product(factor, value) => {
            let value = resolve(value)?;
            Some((*factor * value.0, *factor * value.1))
        }
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => resolve(value),
            // Non-linear math functions cannot retain an unresolved percentage
            // as a two-component computed value without keeping the parser AST.
            _ => None,
        },
    }
}

pub(in crate::style::cascade::resolver) fn calc_depends_on_percentage(
    calc: &Calc<LengthPercentage>,
) -> bool {
    match calc {
        Calc::Value(value) => match value.as_ref() {
            LengthPercentage::Percentage(_) => true,
            LengthPercentage::Calc(inner) => calc_depends_on_percentage(inner),
            LengthPercentage::Dimension(_) => false,
        },
        Calc::Number(_) => false,
        Calc::Sum(left, right) => {
            calc_depends_on_percentage(left) || calc_depends_on_percentage(right)
        }
        Calc::Product(_, value) => calc_depends_on_percentage(value),
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => calc_depends_on_percentage(value),
            MathFunction::Abs(value) | MathFunction::Sign(value) => {
                calc_depends_on_percentage(value)
            }
            MathFunction::Min(values) | MathFunction::Max(values) | MathFunction::Hypot(values) => {
                values.iter().any(calc_depends_on_percentage)
            }
            MathFunction::Clamp(a, b, c) => {
                calc_depends_on_percentage(a)
                    || calc_depends_on_percentage(b)
                    || calc_depends_on_percentage(c)
            }
            MathFunction::Round(_, a, b) | MathFunction::Rem(a, b) | MathFunction::Mod(a, b) => {
                calc_depends_on_percentage(a) || calc_depends_on_percentage(b)
            }
        },
    }
}

/// Convert a margin/padding value into a `LengthPct`. Lengths (including `em`/`rem`)
/// resolve to pixels now; percentages are kept as a fraction to be resolved against
/// the containing-block width at layout time. `auto` margins are treated as `0`
/// (auto-margin centering is not implemented), matching prior behavior.
pub(in crate::style::cascade::resolver) fn length_or_auto_to_lengthpct(
    value: &LengthPercentageOrAuto,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<LengthPct> {
    match value {
        LengthPercentageOrAuto::Auto => Some(LengthPct::Px(0.0)),
        LengthPercentageOrAuto::LengthPercentage(lp) => {
            computed_length_pct(lp, font_size, root_font_size, resolution)
        }
    }
}

pub(in crate::style::cascade::resolver) fn margin_value(
    value: &LengthPercentageOrAuto,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<(LengthPct, bool)> {
    Some((
        length_or_auto_to_lengthpct(value, font_size, root_font_size, resolution)?,
        matches!(value, LengthPercentageOrAuto::Auto),
    ))
}

pub(in crate::style::cascade::resolver) fn inset_value(
    value: &LengthPercentageOrAuto,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<Option<LengthPct>> {
    match value {
        LengthPercentageOrAuto::Auto => Some(None),
        LengthPercentageOrAuto::LengthPercentage(value) => Some(Some(computed_length_pct(
            value,
            font_size,
            root_font_size,
            resolution,
        )?)),
    }
}

pub(in crate::style::cascade::resolver) fn size_to_preferred(
    size: &Size,
    font: &Font,
    root_font_size: f32,
    styles: &mut ComputedStylesBuilder,
    resolution: &ResolutionContext,
) -> Option<PreferredSize> {
    Some(match size {
        Size::Auto => PreferredSize::Auto,
        Size::MinContent(_) => PreferredSize::MinContent,
        Size::MaxContent(_) => PreferredSize::MaxContent,
        Size::FitContent(_) => PreferredSize::FitContent,
        Size::Stretch(_) => PreferredSize::Stretch,
        Size::LengthPercentage(lp) => {
            length_percentage_to_preferred(lp, font, root_font_size, styles, resolution)?
        }
        _ => PreferredSize::Auto,
    })
}

pub(in crate::style::cascade::resolver) fn max_size_to_preferred(
    size: &MaxSize,
    font: &Font,
    root_font_size: f32,
    styles: &mut ComputedStylesBuilder,
    resolution: &ResolutionContext,
) -> Option<PreferredSize> {
    Some(match size {
        MaxSize::None => PreferredSize::Auto,
        MaxSize::MinContent(_) => PreferredSize::MinContent,
        MaxSize::MaxContent(_) => PreferredSize::MaxContent,
        MaxSize::FitContent(_) => PreferredSize::FitContent,
        MaxSize::Stretch(_) => PreferredSize::Stretch,
        MaxSize::LengthPercentage(lp) => {
            length_percentage_to_preferred(lp, font, root_font_size, styles, resolution)?
        }
        _ => PreferredSize::Auto,
    })
}

pub(in crate::style::cascade::resolver) fn spacing_to_text_spacing(
    spacing: &Spacing,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<TextSpacing> {
    match spacing {
        Spacing::Normal => Some(TextSpacing::ZERO),
        Spacing::Length(len) => {
            length_to_px_from_length(len, font_size, root_font_size, resolution)
                .and_then(TextSpacing::from_px)
        }
    }
}

pub(in crate::style::cascade::resolver) fn calc_length_to_px(
    calc: &Calc<Length>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    let resolve = |value| calc_length_to_px(value, font_size, root_font_size, resolution);
    match calc {
        Calc::Value(length) => {
            length_to_px_from_length(length, font_size, root_font_size, resolution)
        }
        Calc::Number(value) => (*value == 0.0).then_some(0.0),
        Calc::Sum(left, right) => Some(resolve(left)? + resolve(right)?),
        Calc::Product(factor, value) => Some(*factor * resolve(value)?),
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => resolve(value),
            MathFunction::Min(values) => values
                .iter()
                .map(resolve)
                .reduce(|left, right| Some(left?.min(right?)))?,
            MathFunction::Max(values) => values
                .iter()
                .map(resolve)
                .reduce(|left, right| Some(left?.max(right?)))?,
            MathFunction::Clamp(min, value, max) => {
                let minimum = resolve(min)?;
                let value = resolve(value)?;
                let maximum = resolve(max)?;
                // CSS gives the minimum precedence when limits conflict.
                Some(if minimum.is_nan() || value.is_nan() || maximum.is_nan() {
                    f32::NAN
                } else {
                    value.min(maximum).max(minimum)
                })
            }
            MathFunction::Abs(value) => Some(resolve(value)?.abs()),
            MathFunction::Hypot(values) => {
                let squared = values.iter().try_fold(0.0, |sum, value| {
                    let value = resolve(value)?;
                    Some(sum + value * value)
                })?;
                Some(squared.sqrt())
            }
            _ => None,
        },
    }
}

pub(in crate::style::cascade::resolver) fn length_to_px_from_length(
    length: &Length,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    match length {
        Length::Value(v) => length_to_px(v, font_size, root_font_size, resolution),
        Length::Calc(value) => calc_length_to_px(value, font_size, root_font_size, resolution),
    }
}

pub(in crate::style::cascade::resolver) fn border_width(
    width: &BorderSideWidth,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<FontRelativeLength> {
    match width {
        BorderSideWidth::Thin => FontRelativeLength::px(1.0),
        BorderSideWidth::Medium => FontRelativeLength::px(3.0),
        BorderSideWidth::Thick => FontRelativeLength::px(5.0),
        BorderSideWidth::Length(len) => match len {
            Length::Value(v) => border_length_from_value(v, font_size, root_font_size, resolution),
            Length::Calc(calc) => {
                let value = calc_length_to_px(calc, font_size, root_font_size, resolution)?;
                FontRelativeLength::px(value.max(0.0))
            }
        },
    }
}

pub(in crate::style::cascade::resolver) fn line_style_to_border_style(
    style: &LineStyle,
) -> BorderStyle {
    match style {
        LineStyle::None => BorderStyle::None,
        LineStyle::Hidden => BorderStyle::Hidden,
        LineStyle::Solid => BorderStyle::Solid,
        LineStyle::Dashed => BorderStyle::Dashed,
        LineStyle::Dotted => BorderStyle::Dotted,
        LineStyle::Groove => BorderStyle::Groove,
        LineStyle::Ridge => BorderStyle::Ridge,
        // The 3D styles (and `double`) paint as a plain line rather than
        // disappearing; only `none`/`hidden` suppress the border.
        LineStyle::Double | LineStyle::Inset | LineStyle::Outset => BorderStyle::Solid,
    }
}

pub(in crate::style::cascade::resolver) fn length_percentage_to_preferred(
    value: &LengthPercentage,
    font: &Font,
    root_font_size: f32,
    styles: &mut ComputedStylesBuilder,
    resolution: &ResolutionContext,
) -> Option<PreferredSize> {
    match value {
        LengthPercentage::Dimension(value) => {
            preferred_from_length(value, font, root_font_size, resolution)
        }
        LengthPercentage::Percentage(value) => Some(PreferredSize::Percent(value.0)),
        LengthPercentage::Calc(value) => {
            comparison_preferred_size(value, font.font_size, root_font_size, styles, resolution)
                .or_else(|| {
                    let [
                        absolute_px,
                        percentage,
                        x_height_px,
                        ch_advance_px,
                        cap_height_px,
                    ] = calc_box_length_percentage_components(
                        value,
                        font.font_size,
                        root_font_size,
                        resolution,
                    )?;
                    Some(PreferredSize::Calc {
                        absolute_px,
                        percentage,
                        x_height_px,
                        ch_advance_px,
                        cap_height_px,
                        percentage_dependent: calc_depends_on_percentage(value),
                    })
                })
        }
    }
}

pub(in crate::style::cascade::resolver) fn comparison_preferred_size(
    calc: &Calc<LengthPercentage>,
    font_size: f32,
    root_font_size: f32,
    styles: &mut ComputedStylesBuilder,
    resolution: &ResolutionContext,
) -> Option<PreferredSize> {
    let Calc::Function(function) = calc else {
        return None;
    };
    let mut operands = [calc; 3];
    let (kind, count) = match function.as_ref() {
        MathFunction::Min(values) | MathFunction::Max(values) => {
            for (slot, value) in operands.get_mut(..values.len())?.iter_mut().zip(values) {
                *slot = value;
            }
            let kind = if matches!(function.as_ref(), MathFunction::Min(_)) {
                SizeComparison::Min
            } else {
                SizeComparison::Max
            };
            (kind, values.len())
        }
        MathFunction::Clamp(min, value, max) => {
            operands = [min, value, max];
            (SizeComparison::Clamp, 3)
        }
        MathFunction::Calc(value) => {
            return comparison_preferred_size(value, font_size, root_font_size, styles, resolution);
        }
        _ => return None,
    };
    if count == 0 {
        return None;
    }
    let mut values = [ComputedSizeComponent::default(); 3];
    for (slot, operand) in values.iter_mut().zip(&operands[..count]) {
        let [
            absolute_px,
            percentage,
            x_height_px,
            ch_advance_px,
            cap_height_px,
        ] = calc_box_length_percentage_components(operand, font_size, root_font_size, resolution)?;
        *slot = ComputedSizeComponent {
            absolute_px,
            percentage,
            x_height_px,
            ch_advance_px,
            cap_height_px,
            percentage_dependent: calc_depends_on_percentage(operand),
        };
    }
    styles.intern_size_comparison(kind, values, u8::try_from(count).ok()?)
}

/// Convert a `<length-percentage>` (e.g. `text-indent`) into a `LengthPct`,
/// deferring percentage resolution to layout (against the containing-block width).
pub(in crate::style::cascade::resolver) fn computed_length_pct(
    value: &LengthPercentage,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<LengthPct> {
    match value {
        LengthPercentage::Dimension(value) => {
            length_pct_from_length(value, font_size, root_font_size, resolution)
        }
        LengthPercentage::Percentage(value) => Some(LengthPct::Pct(value.0)),
        LengthPercentage::Calc(value) => {
            let [
                absolute_px,
                percentage,
                x_height_px,
                ch_advance_px,
                cap_height_px,
            ] = calc_box_length_percentage_components(
                value,
                font_size,
                root_font_size,
                resolution,
            )?;
            let percentage_dependent = calc_depends_on_percentage(value);
            if !percentage_dependent
                && x_height_px == 0.0
                && ch_advance_px == 0.0
                && cap_height_px == 0.0
            {
                return Some(LengthPct::Px(absolute_px));
            }
            Some(LengthPct::Calc {
                absolute_px,
                percentage,
                x_height_px,
                ch_advance_px,
                cap_height_px,
                percentage_dependent,
            })
        }
    }
}

pub(in crate::style::cascade::resolver) fn viewport_length_to_px(
    length: &LengthValue,
    resolution: &ResolutionContext,
) -> Option<f32> {
    let environment = resolution.environment;
    let width = environment.viewport_width() as f32;
    let height = environment.viewport_height().map(|value| value as f32);
    let resolve = |factor: f32, basis: Option<f32>| {
        resolution.uses_viewport_units.set(true);
        let value = factor * basis? / 100.0;
        value.is_finite().then_some(value)
    };
    // The renderer has one static viewport and horizontal writing axes.
    // It has no eligible size-query containers: container units therefore
    // use the specified small-viewport fallback on the corresponding axis.
    match length {
        LengthValue::Vw(value)
        | LengthValue::Vi(value)
        | LengthValue::Svw(value)
        | LengthValue::Svi(value)
        | LengthValue::Lvw(value)
        | LengthValue::Lvi(value)
        | LengthValue::Dvw(value)
        | LengthValue::Dvi(value)
        | LengthValue::Cqw(value)
        | LengthValue::Cqi(value) => resolve(*value, Some(width)),
        LengthValue::Vh(value)
        | LengthValue::Vb(value)
        | LengthValue::Svh(value)
        | LengthValue::Svb(value)
        | LengthValue::Lvh(value)
        | LengthValue::Lvb(value)
        | LengthValue::Dvh(value)
        | LengthValue::Dvb(value)
        | LengthValue::Cqh(value)
        | LengthValue::Cqb(value) => resolve(*value, height),
        LengthValue::Vmin(value)
        | LengthValue::Svmin(value)
        | LengthValue::Lvmin(value)
        | LengthValue::Dvmin(value)
        | LengthValue::Cqmin(value) => resolve(*value, height.map(|height| width.min(height))),
        LengthValue::Vmax(value)
        | LengthValue::Svmax(value)
        | LengthValue::Lvmax(value)
        | LengthValue::Dvmax(value)
        | LengthValue::Cqmax(value) => resolve(*value, height.map(|height| width.max(height))),
        _ => None,
    }
}
