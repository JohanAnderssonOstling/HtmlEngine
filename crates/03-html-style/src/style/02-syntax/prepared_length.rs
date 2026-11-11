//! Supported, parser-independent length calculations. Construction is fallible;
//! evaluation only binds the numeric bases of an already supported expression.
use lightningcss::values::calc::{Calc, MathFunction};
use lightningcss::values::length::{Length, LengthPercentage, LengthValue};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Basis {
    Font,
    RootFont,
    Line,
    RootLine,
    ViewWidth,
    ViewHeight,
    ViewMin,
    ViewMax,
}

#[derive(Clone, Debug)]
enum Scalar {
    Constant(f32),
    Relative(Basis, f32),
    Sum(Box<[Self; 2]>),
    Product(f32, Box<Self>),
    Min(Box<[Self]>),
    Max(Box<[Self]>),
    Clamp(Box<[Self; 3]>),
    Abs(Box<Self>),
    Hypot(Box<[Self]>),
}

/// Saturation keeps contextual multiplication finite without inventing a new
/// cascade failure for an otherwise supported numeric expression.
pub(crate) fn finite(value: f64) -> f32 {
    value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
}

impl Scalar {
    fn evaluate(&self, basis: &impl Fn(Basis) -> f32) -> f32 {
        let eval = |value: &Self| value.evaluate(basis);
        finite(match self {
            Self::Constant(value) => f64::from(*value),
            Self::Relative(unit, value) => f64::from(*value) * f64::from(basis(*unit)),
            Self::Sum(values) => f64::from(eval(&values[0])) + f64::from(eval(&values[1])),
            Self::Product(factor, value) => f64::from(*factor) * f64::from(eval(value)),
            Self::Min(values) => f64::from(values.iter().map(eval).fold(f32::MAX, f32::min)),
            Self::Max(values) => f64::from(values.iter().map(eval).fold(-f32::MAX, f32::max)),
            Self::Clamp(values) => {
                f64::from(eval(&values[1]).min(eval(&values[2])).max(eval(&values[0])))
            }
            Self::Abs(value) => f64::from(eval(value).abs()),
            Self::Hypot(values) => values
                .iter()
                .map(|v| f64::from(eval(v)).powi(2))
                .sum::<f64>()
                .sqrt(),
        })
    }
    fn nonnegative(&self) -> bool {
        match self {
            Self::Constant(v) | Self::Relative(_, v) => *v >= 0.0,
            Self::Sum(v) => v.iter().all(Self::nonnegative),
            Self::Product(f, v) => *f >= 0.0 && v.nonnegative(),
            Self::Min(v) => v.iter().all(Self::nonnegative),
            Self::Max(v) => v.iter().any(Self::nonnegative),
            Self::Clamp(v) => v[0].nonnegative() || v[1].nonnegative() && v[2].nonnegative(),
            Self::Abs(_) | Self::Hypot(_) => true,
        }
    }
    fn zero(&self) -> bool {
        matches!(self, Self::Constant(0.0))
    }
    fn sum(a: Self, b: Self) -> Self {
        if a.zero() {
            b
        } else if b.zero() {
            a
        } else {
            Self::Sum(Box::new([a, b]))
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum LengthMode {
    Scalar,
    Affine,
    Ex,
    Border,
    Spacing,
    Font,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedLength {
    // px, percentage, ex, ch, cap. Each coefficient may depend on style bases.
    components: Box<[Scalar; 5]>,
    pub(crate) percentage_dependent: bool,
    pub(crate) viewport: bool,
    pub(crate) viewport_height: bool,
    // Preserve literal representations, including zero percentages.
    pub(crate) literal_axis: Option<usize>,
    pub(crate) em: Option<f32>,
}

impl PreparedLength {
    pub(crate) fn zero() -> Self {
        Self::constant(0.0)
    }
    pub(crate) fn constant(value: f32) -> Self {
        Self::leaf(0, Scalar::Constant(value))
    }
    fn leaf(axis: usize, value: Scalar) -> Self {
        let mut components = std::array::from_fn(|_| Scalar::Constant(0.0));
        components[axis] = value;
        Self {
            components: Box::new(components),
            percentage_dependent: axis == 1,
            viewport: false,
            viewport_height: false,
            literal_axis: Some(axis),
            em: None,
        }
    }
    pub(crate) fn components(&self, basis: impl Fn(Basis) -> f32) -> [f32; 5] {
        std::array::from_fn(|i| self.components[i].evaluate(&basis))
    }
    /// Font and line-height storage cannot retain a negative pixel component
    /// beside deferred metrics. Prove that constraint when constructing it.
    pub(crate) fn font_components(self, nonnegative_metrics: bool) -> Option<Self> {
        if self.components[2..].iter().any(|v| !v.zero()) {
            if !self.components[0].nonnegative()
                || !self.components[1].nonnegative()
                || nonnegative_metrics && self.components[2..].iter().any(|v| !v.nonnegative())
            {
                return None;
            }
        }
        Some(self)
    }
    pub(crate) fn nonnegative_percentage(
        value: &LengthPercentage,
        mode: LengthMode,
    ) -> Option<Self> {
        match value {
            LengthPercentage::Dimension(v) if v.to_unit_value().0 < 0.0 => return None,
            LengthPercentage::Percentage(v) if v.0 < 0.0 => return None,
            _ => {}
        }
        Self::percentage(value, mode)
    }
    pub(crate) fn nonnegative_length(value: &Length, mode: LengthMode) -> Option<Self> {
        if let Length::Value(v) = value {
            if v.to_unit_value().0 < 0.0 {
                return None;
            }
        }
        Self::length(value, mode)
    }
    pub(crate) fn percentage(value: &LengthPercentage, mode: LengthMode) -> Option<Self> {
        match value {
            LengthPercentage::Dimension(value) => Self::unit(value, mode, false),
            LengthPercentage::Percentage(value) => value
                .0
                .is_finite()
                .then(|| Self::leaf(1, Scalar::Constant(value.0))),
            LengthPercentage::Calc(value) => Self::calc(value, mode, true, Self::percentage_leaf),
        }
    }
    fn percentage_leaf(value: &LengthPercentage, mode: LengthMode) -> Option<Self> {
        match value {
            LengthPercentage::Dimension(value) => Self::unit(value, mode, true),
            _ => Self::percentage(value, mode),
        }
    }
    pub(crate) fn length(value: &Length, mode: LengthMode) -> Option<Self> {
        match value {
            Length::Value(value) => Self::unit(value, mode, false),
            Length::Calc(value) => Self::calc(value, mode, false, Self::length_leaf),
        }
    }
    fn length_leaf(value: &Length, mode: LengthMode) -> Option<Self> {
        match value {
            Length::Value(value) => Self::unit(value, mode, true),
            _ => Self::length(value, mode),
        }
    }
    fn unit(value: &LengthValue, mode: LengthMode, in_calc: bool) -> Option<Self> {
        use LengthValue::*;
        let (v, _) = value.to_unit_value();
        if !v.is_finite() {
            return None;
        }
        let mut result = match value {
            Ex(_)
                if matches!(
                    mode,
                    LengthMode::Affine | LengthMode::Ex | LengthMode::Spacing
                ) || matches!(mode, LengthMode::Border) && !in_calc
                    || matches!(mode, LengthMode::Font) && in_calc =>
            {
                Self::leaf(2, Scalar::Relative(Basis::Font, v))
            }
            Ch(_) | Cap(_)
                if matches!(mode, LengthMode::Affine | LengthMode::Spacing)
                    || matches!(mode, LengthMode::Font) && (in_calc || matches!(value, Cap(_))) =>
            {
                Self::leaf(
                    if matches!(value, Ch(_)) { 3 } else { 4 },
                    Scalar::Relative(Basis::Font, v),
                )
            }
            Ex(_) | Ch(_) | Cap(_) | Rex(_) | Rch(_) | Rcap(_) | Ic(_) | Ric(_) => return None,
            Px(_) => Self::constant(v),
            Pt(_) => Self::constant(finite(f64::from(v) * 96.0 / 72.0)),
            In(_) => Self::constant(finite(f64::from(v) * 96.0)),
            Cm(_) => Self::constant(finite(f64::from(v) * 96.0 / 2.54)),
            Mm(_) => Self::constant(finite(f64::from(v) * 96.0 / 25.4)),
            Q(_) => Self::constant(finite(f64::from(v) * 96.0 / 101.6)),
            Pc(_) => Self::constant(finite(f64::from(v) * 16.0)),
            Em(_) => Self::leaf(0, Scalar::Relative(Basis::Font, v)),
            Rem(_) => Self::leaf(0, Scalar::Relative(Basis::RootFont, v)),
            Lh(_) => Self::leaf(0, Scalar::Relative(Basis::Line, v)),
            Rlh(_) => Self::leaf(0, Scalar::Relative(Basis::RootLine, v)),
            _ => {
                let basis = match value {
                    Vw(_) | Vi(_) | Svw(_) | Svi(_) | Lvw(_) | Lvi(_) | Dvw(_) | Dvi(_)
                    | Cqw(_) | Cqi(_) => Basis::ViewWidth,
                    Vh(_) | Vb(_) | Svh(_) | Svb(_) | Lvh(_) | Lvb(_) | Dvh(_) | Dvb(_)
                    | Cqh(_) | Cqb(_) => Basis::ViewHeight,
                    Vmin(_) | Svmin(_) | Lvmin(_) | Dvmin(_) | Cqmin(_) => Basis::ViewMin,
                    Vmax(_) | Svmax(_) | Lvmax(_) | Dvmax(_) | Cqmax(_) => Basis::ViewMax,
                    _ => return None,
                };
                let mut value = Self::leaf(0, Scalar::Relative(basis, v / 100.0));
                value.viewport = true;
                value.viewport_height = !matches!(basis, Basis::ViewWidth);
                value
            }
        };
        if matches!(value, Em(_)) && !in_calc {
            result.em = Some(v);
        }
        Some(result)
    }
    pub(crate) fn calc<T>(
        value: &Calc<T>,
        mode: LengthMode,
        percentages: bool,
        leaf: fn(&T, LengthMode) -> Option<Self>,
    ) -> Option<Self> {
        let resolve = |value| Self::calc(value, mode, percentages, leaf);
        let mut result = match value {
            Calc::Value(value) => leaf(value, mode)?,
            Calc::Number(value) if *value == 0.0 => Self::zero(),
            Calc::Number(_) => return None,
            Calc::Sum(a, b) => {
                let a = resolve(a)?;
                let b = resolve(b)?;
                let mut result = Self::zero();
                result.percentage_dependent = a.percentage_dependent || b.percentage_dependent;
                result.viewport = a.viewport || b.viewport;
                result.viewport_height = a.viewport_height || b.viewport_height;
                result.components = Box::new(std::array::from_fn(|i| {
                    Scalar::sum(a.components[i].clone(), b.components[i].clone())
                }));
                result
            }
            Calc::Product(factor, value) => {
                if !factor.is_finite() {
                    return None;
                }
                let mut value = resolve(value)?;
                for scalar in value.components.iter_mut() {
                    if !scalar.zero() {
                        *scalar = Scalar::Product(*factor, Box::new(scalar.clone()));
                    }
                }
                value
            }
            Calc::Function(function) => match function.as_ref() {
                MathFunction::Calc(value) => resolve(value)?,
                MathFunction::Min(values) | MathFunction::Max(values) => {
                    let values = values.iter().map(resolve).collect::<Option<Vec<_>>>()?;
                    Self::comparison(
                        values,
                        if matches!(function.as_ref(), MathFunction::Min(_)) {
                            0
                        } else {
                            1
                        },
                        percentages,
                        mode,
                    )?
                }
                MathFunction::Clamp(a, b, c) => Self::comparison(
                    vec![resolve(a)?, resolve(b)?, resolve(c)?],
                    2,
                    percentages,
                    mode,
                )?,
                MathFunction::Abs(value) if !percentages => {
                    let mut value = resolve(value)?;
                    if value.components[1..].iter().any(|v| !v.zero()) {
                        return None;
                    }
                    value.components[0] = Scalar::Abs(Box::new(value.components[0].clone()));
                    value
                }
                MathFunction::Hypot(values) if !percentages => {
                    let values = values.iter().map(resolve).collect::<Option<Vec<_>>>()?;
                    Self::comparison(values, 3, percentages, mode)?
                }
                _ => return None,
            },
        };
        result.literal_axis = None;
        result.em = None;
        Some(result)
    }
    fn comparison(
        values: Vec<Self>,
        kind: u8,
        percentages: bool,
        mode: LengthMode,
    ) -> Option<Self> {
        if values.is_empty()
            || percentages && matches!(mode, LengthMode::Scalar | LengthMode::Spacing)
        {
            return None;
        }
        let axes: Vec<_> = (0..5)
            .filter(|&i| values.iter().any(|v| !v.components[i].zero()))
            .collect();
        if axes.len() > 1 {
            return None;
        }
        let axis = axes.first().copied().unwrap_or(0);
        if kind == 3 && axis != 0 {
            return None;
        }
        let mut result = Self::zero();
        result.percentage_dependent = values.iter().any(|v| v.percentage_dependent);
        result.viewport = values.iter().any(|v| v.viewport);
        result.viewport_height = values.iter().any(|v| v.viewport_height);
        let mut scalars = values
            .into_iter()
            .map(|v| v.components[axis].clone())
            .collect::<Vec<_>>();
        result.components[axis] = match kind {
            0 => Scalar::Min(scalars.into_boxed_slice()),
            1 => Scalar::Max(scalars.into_boxed_slice()),
            2 => {
                let c = scalars.pop()?;
                let b = scalars.pop()?;
                let a = scalars.pop()?;
                Scalar::Clamp(Box::new([a, b, c]))
            }
            _ => Scalar::Hypot(scalars.into_boxed_slice()),
        };
        Some(result)
    }
    pub(crate) fn calc_percentage(value: &Calc<LengthPercentage>) -> Option<Self> {
        Self::calc(value, LengthMode::Affine, true, Self::percentage_leaf)
    }
}
