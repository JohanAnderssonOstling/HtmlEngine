use super::*;

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
        LengthValue::Vw(_) | LengthValue::Vh(_) | LengthValue::Vmin(_) | LengthValue::Vmax(_) => {
            return viewport_length_to_px(length, resolution);
        }
        // These require selected-face metrics that are deliberately absent
        // during computed-style construction. Callers either preserve `ex`
        // symbolically or reject the declaration; they never guess a value.
        LengthValue::Ex(_) | LengthValue::Ch(_) | LengthValue::Cap(_) => return None,
        _ => return None,
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
        LengthValue::Ex(ex) => FontRelativeLength::ex(ex * font_size),
        _ => FontRelativeLength::px(length_to_px(length, font_size, root_font_size, resolution)?),
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

pub(in crate::style::cascade::resolver) fn parsed_text_spacing_to_computed(
    parsed: &ParsedSpacing,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<TextSpacing> {
    match parsed {
        ParsedSpacing::Normal => Some(TextSpacing::ZERO),
        ParsedSpacing::Value(value) => {
            length_percentage_to_text_spacing(value, font_size, root_font_size, resolution)
        }
    }
}

pub(in crate::style::cascade::resolver) fn parsed_tab_size_to_computed(
    parsed: &ParsedTabSize,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<TabSize> {
    match parsed {
        ParsedTabSize::Spaces(value) => TabSize::spaces(*value),
        ParsedTabSize::Length(value) => {
            if let LengthPercentage::Calc(calc) = value
                && let Some(number) = calc_tab_number(calc)
            {
                return TabSize::spaces(number.max(0.0));
            }
            let resolved =
                length_percentage_to_text_spacing(value, font_size, root_font_size, resolution)?;
            (resolved.font_size_fraction() == 0.0)
                .then(|| TabSize::length_px(resolved.absolute_px().max(0.0)))
                .flatten()
        }
    }
}

pub(in crate::style::cascade::resolver) fn calc_tab_number(
    calc: &Calc<LengthPercentage>,
) -> Option<f32> {
    match calc {
        Calc::Value(value) => match value.as_ref() {
            LengthPercentage::Calc(nested) => calc_tab_number(nested),
            _ => None,
        },
        Calc::Number(value) => Some(*value),
        Calc::Sum(left, right) => Some(calc_tab_number(left)? + calc_tab_number(right)?),
        Calc::Product(factor, value) => Some(*factor * calc_tab_number(value)?),
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => calc_tab_number(value),
            MathFunction::Min(values) => values
                .iter()
                .map(calc_tab_number)
                .reduce(|left, right| Some(left?.min(right?)))?,
            MathFunction::Max(values) => values
                .iter()
                .map(calc_tab_number)
                .reduce(|left, right| Some(left?.max(right?)))?,
            MathFunction::Clamp(min, value, max) => {
                Some(calc_tab_number(value)?.clamp(calc_tab_number(min)?, calc_tab_number(max)?))
            }
            MathFunction::Abs(value) => Some(calc_tab_number(value)?.abs()),
            MathFunction::Sign(value) => Some(calc_tab_number(value)?.signum()),
            _ => None,
        },
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
            let (absolute_px, line_height_fraction, x_height_px, ch_advance_px, cap_height_px) =
                calc_box_length_percentage_components(calc, font_size, root_font_size, resolution)?;
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

/// Reduce Lightning CSS's typed linear calc tree while retaining the two
/// selected-face metric terms that cannot become pixels until shaping.
pub(in crate::style::cascade::resolver) fn calc_box_length_percentage_components(
    calc: &Calc<LengthPercentage>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<(f32, f32, f32, f32, f32)> {
    match calc {
        Calc::Value(value) => match value.as_ref() {
            LengthPercentage::Dimension(LengthValue::Ex(value)) => {
                Some((0.0, 0.0, value * font_size, 0.0, 0.0))
            }
            LengthPercentage::Dimension(LengthValue::Ch(value)) => {
                Some((0.0, 0.0, 0.0, value * font_size, 0.0))
            }
            LengthPercentage::Dimension(LengthValue::Cap(value)) => {
                Some((0.0, 0.0, 0.0, 0.0, value * font_size))
            }
            LengthPercentage::Dimension(value) => Some((
                length_to_px(value, font_size, root_font_size, resolution)?,
                0.0,
                0.0,
                0.0,
                0.0,
            )),
            LengthPercentage::Percentage(value) => Some((0.0, value.0, 0.0, 0.0, 0.0)),
            LengthPercentage::Calc(value) => {
                calc_box_length_percentage_components(value, font_size, root_font_size, resolution)
            }
        },
        Calc::Number(value) => (*value == 0.0).then_some((0.0, 0.0, 0.0, 0.0, 0.0)),
        Calc::Sum(left, right) => {
            let left =
                calc_box_length_percentage_components(left, font_size, root_font_size, resolution)?;
            let right = calc_box_length_percentage_components(
                right,
                font_size,
                root_font_size,
                resolution,
            )?;
            Some((
                left.0 + right.0,
                left.1 + right.1,
                left.2 + right.2,
                left.3 + right.3,
                left.4 + right.4,
            ))
        }
        Calc::Product(factor, value) => {
            let value = calc_box_length_percentage_components(
                value,
                font_size,
                root_font_size,
                resolution,
            )?;
            Some((
                *factor * value.0,
                *factor * value.1,
                *factor * value.2,
                *factor * value.3,
                *factor * value.4,
            ))
        }
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => {
                calc_box_length_percentage_components(value, font_size, root_font_size, resolution)
            }
            _ => None,
        },
    }
}

pub(in crate::style::cascade::resolver) fn calc_length_percentage_components(
    calc: &Calc<LengthPercentage>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<(f32, f32)> {
    match calc {
        Calc::Value(value) => {
            let spacing =
                length_percentage_to_text_spacing(value, font_size, root_font_size, resolution)?;
            Some((spacing.absolute_px(), spacing.font_size_fraction()))
        }
        Calc::Number(value) => (*value == 0.0).then_some((0.0, 0.0)),
        Calc::Sum(left, right) => {
            let left =
                calc_length_percentage_components(left, font_size, root_font_size, resolution)?;
            let right =
                calc_length_percentage_components(right, font_size, root_font_size, resolution)?;
            Some((left.0 + right.0, left.1 + right.1))
        }
        Calc::Product(factor, value) => {
            let value =
                calc_length_percentage_components(value, font_size, root_font_size, resolution)?;
            Some((*factor * value.0, *factor * value.1))
        }
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => {
                calc_length_percentage_components(value, font_size, root_font_size, resolution)
            }
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
            _ => false,
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
            length_percentage_to_lengthpct(lp, font_size, root_font_size, resolution)
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
        LengthPercentageOrAuto::LengthPercentage(value) => Some(Some(
            length_percentage_to_lengthpct(value, font_size, root_font_size, resolution)?,
        )),
    }
}

/// Convert a `<length-percentage>` (e.g. `text-indent`) into a `LengthPct`,
/// deferring percentage resolution to layout (against the containing-block width).
pub(in crate::style::cascade::resolver) fn length_percentage_to_lengthpct(
    lp: &LengthPercentage,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<LengthPct> {
    match lp {
        LengthPercentage::Dimension(len) => {
            length_pct_from_length(len, font_size, root_font_size, resolution)
        }
        LengthPercentage::Percentage(p) => Some(LengthPct::Pct(p.0)),
        LengthPercentage::Calc(value) => computed_length_pct(
            &LengthPercentage::Calc(value.clone()),
            font_size,
            root_font_size,
            resolution,
        ),
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
        Size::LengthPercentage(lp) => match lp {
            LengthPercentage::Dimension(len) => {
                preferred_from_length(len, font, root_font_size, resolution)?
            }
            LengthPercentage::Percentage(p) => PreferredSize::Percent(p.0),
            LengthPercentage::Calc(value) => length_percentage_to_preferred(
                &LengthPercentage::Calc(value.clone()),
                font,
                root_font_size,
                styles,
                resolution,
            )?,
        },
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
        MaxSize::LengthPercentage(lp) => match lp {
            LengthPercentage::Dimension(len) => {
                preferred_from_length(len, font, root_font_size, resolution)?
            }
            LengthPercentage::Percentage(p) => PreferredSize::Percent(p.0),
            LengthPercentage::Calc(value) => length_percentage_to_preferred(
                &LengthPercentage::Calc(value.clone()),
                font,
                root_font_size,
                styles,
                resolution,
            )?,
        },
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
        Spacing::Length(len) => match len {
            Length::Value(v) => length_to_px(v, font_size, root_font_size, resolution)
                .and_then(TextSpacing::from_px),
            Length::Calc(calc) => calc_length_to_px(calc, font_size, root_font_size, resolution)
                .and_then(TextSpacing::from_px),
        },
    }
}

pub(in crate::style::cascade::resolver) fn calc_length_to_px(
    calc: &Calc<Length>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    match calc {
        Calc::Value(length) => match length.as_ref() {
            Length::Value(value) => length_to_px(value, font_size, root_font_size, resolution),
            Length::Calc(nested) => {
                calc_length_to_px(nested, font_size, root_font_size, resolution)
            }
        },
        Calc::Number(value) => (*value == 0.0).then_some(0.0),
        Calc::Sum(left, right) => Some(
            calc_length_to_px(left, font_size, root_font_size, resolution)?
                + calc_length_to_px(right, font_size, root_font_size, resolution)?,
        ),
        Calc::Product(factor, value) => {
            Some(*factor * calc_length_to_px(value, font_size, root_font_size, resolution)?)
        }
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => {
                calc_length_to_px(value, font_size, root_font_size, resolution)
            }
            MathFunction::Min(values) => values
                .iter()
                .map(|value| calc_length_to_px(value, font_size, root_font_size, resolution))
                .reduce(|left, right| Some(left?.min(right?)))?,
            MathFunction::Max(values) => values
                .iter()
                .map(|value| calc_length_to_px(value, font_size, root_font_size, resolution))
                .reduce(|left, right| Some(left?.max(right?)))?,
            MathFunction::Clamp(min, value, max) => Some(
                calc_length_to_px(value, font_size, root_font_size, resolution)?.clamp(
                    calc_length_to_px(min, font_size, root_font_size, resolution)?,
                    calc_length_to_px(max, font_size, root_font_size, resolution)?,
                ),
            ),
            MathFunction::Abs(value) => {
                Some(calc_length_to_px(value, font_size, root_font_size, resolution)?.abs())
            }
            MathFunction::Hypot(values) => {
                let squared = values.iter().try_fold(0.0, |sum, value| {
                    let value = calc_length_to_px(value, font_size, root_font_size, resolution)?;
                    Some(sum + value * value)
                })?;
                Some(squared.sqrt())
            }
            _ => None,
        },
    }
}

pub(in crate::style::cascade::resolver) fn length_to_px_from_length(
    length: &lightningcss::values::length::Length,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    match length {
        lightningcss::values::length::Length::Value(v) => {
            length_to_px(v, font_size, root_font_size, resolution)
        }
        lightningcss::values::length::Length::Calc(value) => {
            calc_length_to_px(value, font_size, root_font_size, resolution)
        }
    }
}

pub(in crate::style::cascade::resolver) fn border_width(
    width: &BorderSideWidth,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<FontRelativeLength> {
    use lightningcss::values::length::Length;
    match width {
        BorderSideWidth::Thin => FontRelativeLength::px(1.0),
        BorderSideWidth::Medium => FontRelativeLength::px(3.0),
        BorderSideWidth::Thick => FontRelativeLength::px(5.0),
        BorderSideWidth::Length(len) => match len {
            Length::Value(v) => border_length_from_value(v, font_size, root_font_size, resolution),
            Length::Calc(_) => FontRelativeLength::px(3.0), // Default to medium for calc
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
