//! Lower typed number-valued sign() atoms before parsing mixed-type math.
//!
//! The native calculation tree cannot multiply an unresolved sign(length)
//! by a percentage, angle or another unresolved number. Keep those atoms
//! until their units resolve, then reuse the native property grammar.

use super::numeric_functions;
use super::value::{NumberValue, parse_number};
use lightningcss::properties::PropertyId;
use lightningcss::traits::Parse;
use lightningcss::values::angle::Angle;
use lightningcss::values::calc::Calc;
use lightningcss::values::time::Time;
use std::borrow::Cow;

pub(crate) fn supports(id: &PropertyId<'_>) -> bool {
    matches!(
        id,
        PropertyId::FontSize
            | PropertyId::VerticalAlign
            | PropertyId::LineHeight
            | PropertyId::FontStyle
            | PropertyId::AspectRatio
            | PropertyId::Flex(_)
            | PropertyId::FlexGrow(_)
            | PropertyId::FlexShrink(_)
            | PropertyId::FlexBasis(_)
    )
}

pub(crate) fn lower<'a>(
    source: &'a str,
    mut resolve: impl FnMut(&NumberValue) -> Option<f32>,
) -> Result<Cow<'a, str>, ()> {
    numeric_functions::replace(source, |name, argument| {
        if !name.eq_ignore_ascii_case("sign") {
            return Ok(None);
        }
        let expression = format!("sign({argument})");
        let value = parse_number(&expression)
            .or_else(|| {
                // Angle and time units are absolute and reduce without a font
                // or layout context. Preserve combinations with nested signs.
                if let Ok(Calc::Number(number)) = Calc::<Angle>::parse_string(&expression) {
                    return Some(NumberValue::Number(number));
                }
                if let Ok(Calc::Number(number)) = Calc::<Time>::parse_string(&expression) {
                    return Some(NumberValue::Number(number));
                }
                None
            })
            .ok_or(())?;
        let value = resolve(&value).ok_or(())?;
        let number = if value.is_nan() {
            "NaN".to_owned()
        } else if value == f32::INFINITY {
            "infinity".to_owned()
        } else if value == f32::NEG_INFINITY {
            "-infinity".to_owned()
        } else {
            value.to_string()
        };
        // A math-function wrapper retains the atom's number type and its
        // computed range semantics, even for zero or negative results.
        Ok(Some(format!("calc({number})")))
    })
}

#[cfg(test)]
mod tests {
    use super::lower;
    use crate::style::syntax::value::NumberValue;
    use lightningcss::properties::font::FontStyle;
    use lightningcss::properties::{Property, PropertyId};
    use lightningcss::stylesheet::ParserOptions;

    #[test]
    fn sign_atoms_preserve_token_boundaries_and_native_angle_values() {
        let source = "/*é*/oblique calc(s\\69 gn(1rem - 1px) * 10deg)";
        let lowered = lower(source, |_| Some(-1.0)).unwrap();
        let Property::FontStyle(FontStyle::Oblique(angle)) =
            Property::parse_string(PropertyId::FontStyle, &lowered, ParserOptions::default())
                .unwrap()
        else {
            panic!("expected a native oblique angle")
        };
        assert_eq!(angle.to_degrees(), -10.0);
        assert_eq!(
            lower("'sign(1rem)' /*sign(2rem)*/ sign(1rem - 1px)", |_| Some(
                -0.0
            ))
            .unwrap(),
            "'sign(1rem)' /*sign(2rem)*/ calc(-0)"
        );
        assert!(lower("calc(sign(1rem + 1) * 10px)", |_| Some(1.0)).is_err());
        assert_eq!(
            lower("calc(sign(sign(1rem - 1px) * 10px) * 100%)", |value| {
                Some(match value {
                    NumberValue::Number(number) => *number,
                    NumberValue::Calculation(_) => -1.0,
                })
            })
            .unwrap(),
            "calc(calc(-1) * 100%)"
        );
    }
}
