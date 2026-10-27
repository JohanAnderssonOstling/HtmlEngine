//! Dimensional products and quotients that the native calc tree cannot store.

use super::{random_math, tree_counting};
use cssparser::{ParseError, Parser, ParserInput, Token, match_ignore_ascii_case};
use lightningcss::properties::PropertyId;
use lightningcss::traits::Parse;
use lightningcss::values::length::LengthValue;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Dimension([i16; 7]);

impl Dimension {
    pub(super) const NUMBER: Self = Self([0; 7]);
    pub(super) const LENGTH: Self = Self([1, 0, 0, 0, 0, 0, 0]);
    pub(super) const ANGLE: Self = Self([0, 1, 0, 0, 0, 0, 0]);
    pub(super) const PERCENTAGE: Self = Self([0, 0, 0, 0, 0, 0, 1]);

    fn product(self, other: Self, divide: bool) -> Option<Self> {
        let mut result = self;
        for (value, rhs) in result.0.iter_mut().zip(other.0) {
            *value = value.checked_add(if divide { -rhs } else { rhs })?;
            if value.abs() > 128 {
                return None;
            }
        }
        Some(result)
    }
}

pub(crate) struct Calculation {
    pub(super) dimension: Dimension,
    pub(super) extended: bool,
    node: Node,
}

enum Node {
    Scalar(f64),
    Length(LengthValue),
    Percentage(f64),
    Binary(char, Box<Calculation>, Box<Calculation>),
    Function(Function, Vec<Calculation>),
}

#[derive(Clone, Copy)]
enum Function {
    Min,
    Max,
    Clamp,
    Abs,
    Sign,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Pow,
    Sqrt,
    Hypot,
    Exp,
    Log,
    Mod,
    Rem,
}

impl Calculation {
    pub(super) fn parse(source: &str, percentage_is_length: bool) -> Option<Self> {
        let mut budget = 256;
        let mut input = ParserInput::new(source);
        Parser::new(&mut input)
            .parse_entirely(|parser| {
                // A property component is a math function, rather than a bare
                // arithmetic expression or a parenthesized declaration value.
                let name = parser.expect_function()?.clone();
                parser.parse_nested_block(|parser| {
                    function(parser, &name, percentage_is_length, 0, &mut budget)
                })
            })
            .ok()
    }

    fn scalar(value: f64, dimension: Dimension) -> Self {
        Self {
            dimension,
            extended: false,
            node: Node::Scalar(value),
        }
    }

    fn degree(&self) -> Option<u8> {
        match &self.node {
            Node::Scalar(value) => value.is_finite().then_some(0),
            Node::Length(LengthValue::Ex(_) | LengthValue::Ch(_) | LengthValue::Cap(_)) => None,
            Node::Length(_) => Some(0),
            Node::Percentage(_) => Some(1),
            Node::Binary(op, a, b) => {
                let a = a.degree()?;
                let b = b.degree()?;
                let degree = match op {
                    '+' | '-' => a.max(b),
                    '*' => a + b,
                    '/' if b == 0 => a,
                    _ => return None,
                };
                (degree <= 1).then_some(degree)
            }
            Node::Function(_, values) => values
                .iter()
                .all(|value| value.degree() == Some(0))
                .then_some(0),
        }
    }

    pub(crate) fn is_linear_length(&self) -> bool {
        self.dimension == Dimension::LENGTH && self.degree().is_some()
    }

    /// Resolve real font/viewport lengths while keeping the layout percentage
    /// basis symbolic. No syntax-validation stand-ins enter this calculation.
    pub(crate) fn resolve_length(
        &self,
        resolve: impl Fn(&LengthValue) -> Option<f32>,
    ) -> Option<String> {
        if !self.is_linear_length() {
            return None;
        }
        let value = self.evaluate(&resolve)?;
        if !value.absolute.is_finite() || !value.percentage.is_finite() {
            return None;
        }
        Some(if value.has_percentage {
            format!("calc({}px + {}%)", value.absolute, value.percentage * 100.0)
        } else {
            format!("{}px", value.absolute)
        })
    }

    fn evaluate(&self, resolve: &impl Fn(&LengthValue) -> Option<f32>) -> Option<Linear> {
        match &self.node {
            Node::Scalar(value) => Some(Linear::scalar(*value)),
            Node::Length(value) => Some(Linear::scalar(f64::from(resolve(value)?))),
            Node::Percentage(value) => Some(Linear {
                absolute: 0.0,
                percentage: *value,
                has_percentage: true,
            }),
            Node::Binary(op, a, b) => {
                let a = a.evaluate(resolve)?;
                let b = b.evaluate(resolve)?;
                let (absolute, percentage) = match op {
                    '+' => (a.absolute + b.absolute, a.percentage + b.percentage),
                    '-' => (a.absolute - b.absolute, a.percentage - b.percentage),
                    '*' if a.percentage == 0.0 || b.percentage == 0.0 => (
                        a.absolute * b.absolute,
                        a.percentage * b.absolute + a.absolute * b.percentage,
                    ),
                    '/' if !b.has_percentage => {
                        (a.absolute / b.absolute, a.percentage / b.absolute)
                    }
                    _ => return None,
                };
                Some(Linear {
                    absolute,
                    percentage,
                    has_percentage: a.has_percentage || b.has_percentage,
                })
            }
            Node::Function(function, arguments) => {
                let values = arguments
                    .iter()
                    .map(|argument| {
                        let value = argument.evaluate(resolve)?;
                        (!value.has_percentage).then_some(value.absolute)
                    })
                    .collect::<Option<Vec<_>>>()?;
                let a = values[0];
                let value = match function {
                    Function::Min => values.iter().copied().reduce(f64::min)?,
                    Function::Max => values.iter().copied().reduce(f64::max)?,
                    Function::Clamp => a.max(values[1].min(values[2])),
                    Function::Abs => a.abs(),
                    Function::Sign => {
                        if a.is_nan() {
                            a
                        } else if a == 0.0 {
                            a
                        } else {
                            a.signum()
                        }
                    }
                    Function::Sin | Function::Cos | Function::Tan => {
                        let angle = if arguments[0].dimension == Dimension::ANGLE {
                            a.to_radians()
                        } else {
                            a
                        };
                        match function {
                            Function::Sin => angle.sin(),
                            Function::Cos => angle.cos(),
                            _ => angle.tan(),
                        }
                    }
                    Function::Asin => a.asin().to_degrees(),
                    Function::Acos => a.acos().to_degrees(),
                    Function::Atan => a.atan().to_degrees(),
                    Function::Atan2 => a.atan2(values[1]).to_degrees(),
                    Function::Pow => a.powf(values[1]),
                    Function::Sqrt => a.sqrt(),
                    Function::Hypot => values
                        .iter()
                        .fold(0.0_f64, |result, value| result.hypot(*value)),
                    Function::Exp => a.exp(),
                    Function::Log => {
                        if values.len() == 1 {
                            a.ln()
                        } else {
                            a.log(values[1])
                        }
                    }
                    Function::Mod => a - values[1] * (a / values[1]).floor(),
                    Function::Rem => a % values[1],
                };
                Some(Linear::scalar(value))
            }
        }
    }
}

struct Linear {
    absolute: f64,
    percentage: f64,
    has_percentage: bool,
}
impl Linear {
    fn scalar(value: f64) -> Self {
        Self {
            absolute: value,
            percentage: 0.0,
            has_percentage: false,
        }
    }
}

fn sum<'i>(
    parser: &mut Parser<'i, '_>,
    hint: bool,
    depth: usize,
    budget: &mut usize,
) -> Result<Calculation, ParseError<'i, ()>> {
    let mut left = product(parser, hint, depth, budget)?;
    loop {
        let state = parser.state();
        if !matches!(parser.next_including_whitespace(), Ok(Token::WhiteSpace(_))) {
            parser.reset(&state);
            break;
        }
        let op = match parser.next() {
            Ok(Token::Delim(op @ ('+' | '-'))) => *op,
            _ => {
                parser.reset(&state);
                break;
            }
        };
        if !matches!(parser.next_including_whitespace(), Ok(Token::WhiteSpace(_))) {
            return Err(parser.new_custom_error(()));
        }
        let right = product(parser, hint, depth, budget)?;
        if left.dimension != right.dimension {
            return Err(parser.new_custom_error(()));
        }
        left = Calculation {
            dimension: left.dimension,
            extended: left.extended || right.extended,
            node: Node::Binary(op, Box::new(left), Box::new(right)),
        };
    }
    Ok(left)
}

fn product<'i>(
    parser: &mut Parser<'i, '_>,
    hint: bool,
    depth: usize,
    budget: &mut usize,
) -> Result<Calculation, ParseError<'i, ()>> {
    let mut left = atom(parser, hint, depth, budget)?;
    loop {
        let state = parser.state();
        let op = match parser.next() {
            Ok(Token::Delim(op @ ('*' | '/'))) => *op,
            _ => {
                parser.reset(&state);
                break;
            }
        };
        let right = atom(parser, hint, depth, budget)?;
        let extended = left.extended
            || right.extended
            || (right.dimension != Dimension::NUMBER
                && (op == '/' || left.dimension != Dimension::NUMBER));
        let dimension = left
            .dimension
            .product(right.dimension, op == '/')
            .ok_or_else(|| parser.new_custom_error(()))?;
        left = Calculation {
            dimension,
            extended,
            node: Node::Binary(op, Box::new(left), Box::new(right)),
        };
    }
    Ok(left)
}

fn atom<'i>(
    parser: &mut Parser<'i, '_>,
    hint: bool,
    depth: usize,
    budget: &mut usize,
) -> Result<Calculation, ParseError<'i, ()>> {
    if depth >= 128 {
        return Err(parser.new_custom_error(()));
    }
    *budget = budget
        .checked_sub(1)
        .ok_or_else(|| parser.new_custom_error(()))?;
    let start = parser.position();
    match parser.next()?.clone() {
        Token::Number { value, .. } => Ok(Calculation::scalar(f64::from(value), Dimension::NUMBER)),
        Token::Percentage { unit_value, .. } => Ok(Calculation {
            dimension: if hint {
                Dimension::LENGTH
            } else {
                Dimension::PERCENTAGE
            },
            extended: false,
            node: if hint {
                Node::Percentage(f64::from(unit_value))
            } else {
                Node::Scalar(f64::from(unit_value))
            },
        }),
        Token::Dimension { value, unit, .. } => {
            if let Ok(value) = LengthValue::parse_string(parser.slice_from(start)) {
                return Ok(Calculation {
                    dimension: Dimension::LENGTH,
                    extended: false,
                    node: Node::Length(value),
                });
            }
            let value = f64::from(value);
            let (axis, value) = match_ignore_ascii_case! {unit.as_ref(),
                "deg" => (1, value), "grad" => (1, value * 0.9), "rad" => (1, value.to_degrees()), "turn" => (1, value * 360.0),
                "s" => (2, value), "ms" => (2, value / 1000.0), "hz" => (3, value), "khz" => (3, value * 1000.0),
                "dppx" | "x" => (4, value), "dpi" => (4, value / 96.0), "dpcm" => (4, value * 2.54 / 96.0),
                _ => return Err(parser.new_custom_error(())),
            };
            let mut dimension = Dimension::NUMBER;
            dimension.0[axis] = 1;
            Ok(Calculation::scalar(value, dimension))
        }
        Token::Ident(name) => {
            let value = match_ignore_ascii_case! {name.as_ref(), "pi" => std::f64::consts::PI, "e" => std::f64::consts::E, "infinity" => f64::INFINITY, "-infinity" => f64::NEG_INFINITY, "nan" => f64::NAN, _ => return Err(parser.new_custom_error(())) };
            Ok(Calculation::scalar(value, Dimension::NUMBER))
        }
        Token::ParenthesisBlock => {
            parser.parse_nested_block(|parser| sum(parser, hint, depth + 1, budget))
        }
        Token::Function(name) => {
            parser.parse_nested_block(|parser| function(parser, &name, hint, depth + 1, budget))
        }
        _ => Err(parser.new_custom_error(())),
    }
}

fn function<'i>(
    parser: &mut Parser<'i, '_>,
    name: &str,
    hint: bool,
    depth: usize,
    budget: &mut usize,
) -> Result<Calculation, ParseError<'i, ()>> {
    if depth >= 128 {
        return Err(parser.new_custom_error(()));
    }
    if name.eq_ignore_ascii_case("calc") {
        return sum(parser, hint, depth + 1, budget);
    }
    let function = match_ignore_ascii_case! {name,
        "min" => Function::Min, "max" => Function::Max, "clamp" => Function::Clamp, "abs" => Function::Abs, "sign" => Function::Sign,
        "sin" => Function::Sin, "cos" => Function::Cos, "tan" => Function::Tan, "asin" => Function::Asin, "acos" => Function::Acos,
        "atan" => Function::Atan, "atan2" => Function::Atan2, "pow" => Function::Pow, "sqrt" => Function::Sqrt, "hypot" => Function::Hypot,
        "exp" => Function::Exp, "log" => Function::Log, "mod" => Function::Mod, "rem" => Function::Rem,
        _ => return Err(parser.new_custom_error(())),
    };
    let values = parser.parse_comma_separated(|parser| sum(parser, hint, depth + 1, budget))?;
    let first = values[0].dimension;
    let same = values.iter().all(|value| value.dimension == first);
    let numbers = values
        .iter()
        .all(|value| value.dimension == Dimension::NUMBER);
    let dimension = match function {
        Function::Min | Function::Max | Function::Hypot if same => first,
        Function::Clamp if same && values.len() == 3 => first,
        Function::Abs if values.len() == 1 => first,
        Function::Sign if values.len() == 1 => Dimension::NUMBER,
        Function::Sin | Function::Cos | Function::Tan
            if values.len() == 1 && (first == Dimension::NUMBER || first == Dimension::ANGLE) =>
        {
            Dimension::NUMBER
        }
        Function::Asin | Function::Acos | Function::Atan if numbers && values.len() == 1 => {
            Dimension::ANGLE
        }
        Function::Atan2 if same && values.len() == 2 => Dimension::ANGLE,
        Function::Pow if numbers && values.len() == 2 => Dimension::NUMBER,
        Function::Sqrt | Function::Exp if numbers && values.len() == 1 => Dimension::NUMBER,
        Function::Log if numbers && (1..=2).contains(&values.len()) => Dimension::NUMBER,
        Function::Mod | Function::Rem if same && values.len() == 2 => first,
        _ => return Err(parser.new_custom_error(())),
    };
    Ok(Calculation {
        dimension,
        extended: values.iter().any(|value| value.extended),
        node: Node::Function(function, values),
    })
}

pub(crate) fn size_property(id: &PropertyId<'_>) -> bool {
    matches!(id, PropertyId::Width | PropertyId::Height)
}

pub(crate) fn size_calculation(value: &str) -> Option<Calculation> {
    let value = Calculation::parse(value, true)?;
    (value.extended && value.dimension == Dimension::LENGTH).then_some(value)
}

fn size_syntax(value: &str, supported: bool) -> Option<bool> {
    if !value.contains('*') && !value.contains('/') {
        return None;
    }
    let tree = tree_counting::lower(value, |_| 1).ok()?;
    let random = random_math::for_syntax(&tree).ok()?;
    let value = Calculation::parse(&random, true)?;
    value.extended.then(|| {
        if supported {
            value.is_linear_length()
        } else {
            value.dimension == Dimension::LENGTH
        }
    })
}

pub(crate) fn property_value_is_valid(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    size_property(id)
        .then(|| size_syntax(value, false))
        .flatten()
}

pub(crate) fn value_is_supported(id: &PropertyId<'_>, value: &str) -> Option<bool> {
    size_property(id)
        .then(|| size_syntax(value, true))
        .flatten()
}
