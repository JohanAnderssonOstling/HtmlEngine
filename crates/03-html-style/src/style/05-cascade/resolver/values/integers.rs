//! Contextual calculation and range clamping for CSS numeric values.

use super::*;
use crate::style::syntax::value::{IntegerValue, NumberValue, normalize_math};
use lightningcss::visitor::VisitTypes;
use std::ops::RangeInclusive;

pub(in crate::style::cascade::resolver) fn resolve_number(
    value: &NumberValue,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    match value {
        NumberValue::Number(value) => Some(*value),
        NumberValue::Calculation(tokens) => {
            resolve_calculation(tokens, font_size, root_font_size, resolution)
        }
    }
}

pub(in crate::style::cascade::resolver) fn resolve_integer(
    value: &IntegerValue,
    range: RangeInclusive<u32>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<u32> {
    let minimum = *range.start();
    let maximum = *range.end();
    match value {
        IntegerValue::Integer(value) => Some(u32::try_from(*value).ok()?.clamp(minimum, maximum)),
        IntegerValue::Calculation(tokens) => {
            let value = resolve_calculation(tokens, font_size, root_font_size, resolution)?;
            // CSS censors NaN to zero, then clamps to the property's range.
            Some(value.round().max(minimum as f32).min(maximum as f32) as u32)
        }
    }
}

pub(in crate::style::cascade::resolver) fn resolve_signed_integer(
    value: &IntegerValue,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<i32> {
    match value {
        IntegerValue::Integer(value) => i32::try_from(*value).ok(),
        IntegerValue::Calculation(tokens) => {
            let value = f64::from(resolve_calculation(
                tokens,
                font_size,
                root_font_size,
                resolution,
            )?);
            let value = if value.is_nan() { 0.0 } else { value };
            // Integer calculations round halfway toward positive infinity.
            // Widen first so adding one half preserves large f32 integers.
            Some(
                (value + 0.5)
                    .floor()
                    .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
            )
        }
    }
}

fn resolve_calculation(
    tokens: &TokenList<'_>,
    font_size: f32,
    root_font_size: f32,
    resolution: &ResolutionContext,
) -> Option<f32> {
    // Resolve supported relative units, then reduce the original math tokens
    // without losing NaN constants to the dependency's serializer.
    let mut tokens = tokens.clone();
    tokens
        .visit(&mut NumberLengthResolver {
            font_size,
            root_font_size,
            resolution,
        })
        .ok()?;
    let css = token_list_to_css_string(&tokens)?;
    let normalized = normalize_math(&css);
    let Calc::Number(value) = Calc::<Length>::parse_string(&normalized).ok()? else {
        return None;
    };
    Some(value)
}

struct NumberLengthResolver<'a> {
    font_size: f32,
    root_font_size: f32,
    resolution: &'a ResolutionContext,
}

impl<'css> Visitor<'css> for NumberLengthResolver<'_> {
    type Error = ();

    fn visit_types(&self) -> VisitTypes {
        lightningcss::visit_types!(LENGTHS)
    }

    fn visit_length(&mut self, length: &mut LengthValue) -> Result<(), Self::Error> {
        *length = LengthValue::Px(
            length_to_px(length, self.font_size, self.root_font_size, self.resolution).ok_or(())?,
        );
        Ok(())
    }
}
