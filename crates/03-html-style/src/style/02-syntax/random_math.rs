//! Typed math lowering for random() over the native calculation grammar.

use super::{numeric_functions, random_key};
use cssparser::{Parser, ParserInput, Token};
use lightningcss::traits::{Parse, TrySign};
use lightningcss::values::angle::Angle;
use lightningcss::values::calc::{Calc, MathFunction};
use lightningcss::values::length::LengthPercentage;
use lightningcss::values::time::Time;
use std::borrow::Cow;

pub(crate) struct Arguments<'a> {
    pub key: &'a str,
    pub minimum: &'a str,
    pub maximum: &'a str,
    pub step: Option<&'a str>,
}

pub(crate) fn arguments(source: &str) -> Option<Arguments<'_>> {
    let mut input = ParserInput::new(source);
    let mut parser = Parser::new(&mut input);
    let mut arguments = parser
        .parse_comma_separated(|parser| {
            let start = parser.position();
            while parser.next().is_ok() {}
            Ok::<_, cssparser::ParseError<'_, ()>>(parser.slice_from(start).trim())
        })
        .ok()?;
    if arguments.iter().any(|argument| argument.is_empty()) {
        return None;
    }
    let mut first_input = ParserInput::new(arguments.first()?);
    let first = Parser::new(&mut first_input).next().ok()?.clone();
    let key = if matches!(first, Token::Ident(name) if name.starts_with("--") || name.starts_with("ua-") || ["auto","fixed","element-scoped","property-scoped","property-index-scoped"].iter().any(|keyword|name.eq_ignore_ascii_case(keyword)))
    {
        let key = arguments.remove(0);
        random_key::parse_key(key)?;
        key
    } else {
        "auto"
    };
    match arguments.as_slice() {
        [minimum, maximum] => Some(Arguments {
            key,
            minimum,
            maximum,
            step: None,
        }),
        [minimum, maximum, step] => Some(Arguments {
            key,
            minimum,
            maximum,
            step: Some(step),
        }),
        _ => None,
    }
}

pub(crate) fn for_syntax(source: &str) -> Result<Cow<'_, str>, ()> {
    lower(source, true, |_| Some(0.5))
}

pub(crate) fn for_computation<'a>(
    source: &'a str,
    base: impl FnMut(&str) -> Option<f64>,
) -> Result<Cow<'a, str>, ()> {
    lower(source, false, base)
}

fn lower<'a>(
    source: &'a str,
    syntax: bool,
    mut base: impl FnMut(&str) -> Option<f64>,
) -> Result<Cow<'a, str>, ()> {
    if !source.contains('\\')
        && !source
            .as_bytes()
            .windows(7)
            .any(|part| part.eq_ignore_ascii_case(b"random("))
    {
        return Ok(Cow::Borrowed(source));
    }
    numeric_functions::replace(source, |name, source| {
        if !name.eq_ignore_ascii_case("random") {
            return Ok(None);
        }
        let Arguments {
            key,
            minimum: a,
            maximum: b,
            step,
        } = arguments(source).ok_or(())?;
        if syntax {
            // Every argument participates in type checking, including ignored
            // non-positive steps. The outer property checks the resulting type.
            let step = step.map(|step| format!(",({step})")).unwrap_or_default();
            return Ok(Some(format!("max(({a}),({b}){step})")));
        }
        let r = base(key).ok_or(())?;
        if let Some(value) = constant_sample(a, b, step, r) {
            return Ok(Some(value));
        }
        let blend = format!("max(({a}),({} * ({a}) + {r} * ({b})))", 1.0 - r);
        let Some(step) = step else {
            return Ok(Some(blend));
        };
        let c = format!("calc({step})");
        let sign = LengthPercentage::parse_string(&c)
            .ok()
            .and_then(|v| v.try_sign())
            .or_else(|| {
                Calc::<Angle>::parse_string(&c)
                    .ok()
                    .and_then(|v| v.try_sign())
            })
            .or_else(|| {
                Calc::<Time>::parse_string(&c)
                    .ok()
                    .and_then(|v| v.try_sign())
            })
            .ok_or(())?;
        if sign <= 0.0 {
            return Ok(Some(blend));
        }
        // Select uniformly among the stepped values. Include a maximum lying
        // within step/1000 of the last step, as required by CSS Values 5.
        let range = format!("max(calc(0 * ({a})),calc(({b}) - ({a})))");
        let intervals = format!("round(down,calc({range} + ({step}) / 1000),calc({step}))");
        let selected = format!("round(down,calc({r} * ({intervals} + ({step}))),calc({step}))");
        Ok(Some(format!(
            "min(max(calc({a}),calc({b})),calc(({a}) + {selected}))"
        )))
    })
}

fn constant(source: &str) -> Option<(f64, String)> {
    fn length(value: LengthPercentage) -> Option<(f64, String)> {
        match value {
            LengthPercentage::Dimension(value) => {
                if let Some(value) = value.to_px() {
                    return Some((f64::from(value), "px".to_owned()));
                }
                let (value, unit) = value.to_unit_value();
                Some((f64::from(value), unit.to_owned()))
            }
            LengthPercentage::Percentage(value) => {
                Some((f64::from(value.0) * 100.0, "%".to_owned()))
            }
            LengthPercentage::Calc(value) => match *value {
                Calc::Value(value) => length(*value),
                Calc::Number(value) => Some((f64::from(value), String::new())),
                Calc::Function(value) => match *value {
                    MathFunction::Calc(value) => calc(value),
                    _ => None,
                },
                _ => None,
            },
        }
    }
    fn calc(value: Calc<LengthPercentage>) -> Option<(f64, String)> {
        match value {
            Calc::Value(value) => length(*value),
            Calc::Number(value) => Some((f64::from(value), String::new())),
            Calc::Function(value) => match *value {
                MathFunction::Calc(value) => calc(value),
                _ => None,
            },
            _ => None,
        }
    }
    let value = format!("calc({source})");
    if let Ok(value) = Calc::<LengthPercentage>::parse_string(&value)
        && let Some(value) = calc(value)
    {
        return Some(value);
    }
    if let Ok(Calc::Value(value)) = Calc::<Angle>::parse_string(&value) {
        return Some((f64::from(value.to_degrees()), "deg".to_owned()));
    }
    if let Ok(Calc::Value(value)) = Calc::<Time>::parse_string(&value) {
        return Some((f64::from(value.to_ms()), "ms".to_owned()));
    }
    None
}

fn constant_sample(a: &str, b: &str, c: Option<&str>, r: f64) -> Option<String> {
    let (a, unit) = constant(a)?;
    let (b, b_unit) = constant(b)?;
    if unit != b_unit {
        return None;
    }
    let c = match c {
        Some(source) => Some(constant(source)?),
        None => None,
    };
    if c.as_ref().is_some_and(|(_, c_unit)| c_unit != &unit) {
        return None;
    }
    let c = c.map(|(value, _)| value);
    let result = if a.is_nan() || b.is_nan() || c.is_some_and(f64::is_nan) {
        f64::NAN
    } else if a.is_infinite() {
        a
    } else {
        let b = b.max(a);
        let range = b - a;
        if !range.is_finite() {
            f64::NAN
        } else if let Some(c) = c.filter(|c| *c > 0.0) {
            if c.is_infinite() {
                a
            } else {
                let n = (range / c).floor();
                if !n.is_finite() {
                    a + r * range
                } else {
                    let epsilon = (c / 1000.0).max(f64::from_bits(1));
                    let n = if (a + (n + 1.0) * c - b).abs() <= epsilon {
                        n + 1.0
                    } else {
                        n
                    };
                    let index = (r * (n + 1.0)).floor().min(n);
                    let value = a + index * c;
                    if index == n && (value - b).abs() <= epsilon {
                        b
                    } else {
                        value
                    }
                }
            }
        } else {
            a + r * range
        }
    };
    let number = if result.is_nan() {
        "NaN".to_owned()
    } else if result == f64::INFINITY {
        "infinity".to_owned()
    } else if result == f64::NEG_INFINITY {
        "-infinity".to_owned()
    } else {
        return Some(format!("calc({result}{unit})"));
    };
    Some(if unit.is_empty() {
        format!("calc({number})")
    } else {
        format!("calc({number} * 1{unit})")
    })
}
