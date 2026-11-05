//! Semantic constraints on values already parsed by Lightning CSS.
use lightningcss::properties::Property;
use lightningcss::properties::border::{BorderSideWidth, LineStyle};
use lightningcss::properties::outline::OutlineStyle;
use lightningcss::values::calc::{Calc, MathFunction};
use lightningcss::values::color::{CssColor, RGBA};
use lightningcss::values::length::{Length, LengthPercentage, LengthPercentageOrAuto};

/// A bare number is not a length inside a math function, including zero.
/// Negative lengths are allowed here: math ranges are clamped after parsing.
fn calc_has_length_type<V>(value: &Calc<V>, leaf: fn(&V) -> bool) -> bool {
    let valid = |value: &Calc<V>| calc_has_length_type(value, leaf);
    match value {
        Calc::Number(_) => false,
        Calc::Value(value) => leaf(value),
        Calc::Sum(a, b) => valid(a) && valid(b),
        Calc::Product(_, value) => valid(value),
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) | MathFunction::Abs(value) => valid(value),
            MathFunction::Sign(_) => false,
            MathFunction::Min(values) | MathFunction::Max(values) | MathFunction::Hypot(values) => {
                values.iter().all(valid)
            }
            MathFunction::Clamp(a, b, c) => valid(a) && valid(b) && valid(c),
            MathFunction::Round(_, a, b) | MathFunction::Rem(a, b) | MathFunction::Mod(a, b) => {
                valid(a) && valid(b)
            }
        },
    }
}

pub(crate) fn length_has_valid_math(value: &Length) -> bool {
    match value {
        Length::Calc(value) => calc_has_length_type(value, length_has_valid_math),
        Length::Value(_) => true,
    }
}

pub(crate) fn length_percentage_has_valid_math(value: &LengthPercentage) -> bool {
    match value {
        LengthPercentage::Calc(value) => {
            calc_has_length_type(value, length_percentage_has_valid_math)
        }
        _ => true,
    }
}

fn length_or_auto_has_valid_math(value: &LengthPercentageOrAuto) -> bool {
    match value {
        LengthPercentageOrAuto::LengthPercentage(value) => length_percentage_has_valid_math(value),
        LengthPercentageOrAuto::Auto => true,
    }
}

fn padding_is_valid(value: &LengthPercentageOrAuto) -> bool {
    !matches!(value, LengthPercentageOrAuto::Auto) && length_or_auto_has_valid_math(value)
}

fn border_width_is_valid(value: &BorderSideWidth) -> bool {
    match value {
        BorderSideWidth::Length(value) => length_has_valid_math(value),
        _ => true,
    }
}

/// These checks inspect native structure; they never parse or rewrite CSS text.
pub(crate) fn property_is_valid(property: &Property<'_>) -> bool {
    match property {
        Property::Custom(_) => super::generated_content::native_value_is_valid(property),
        Property::Padding(value) => [&value.top, &value.right, &value.bottom, &value.left]
            .into_iter()
            .all(padding_is_valid),
        Property::PaddingBlock(value) => {
            padding_is_valid(&value.block_start) && padding_is_valid(&value.block_end)
        }
        Property::PaddingInline(value) => {
            padding_is_valid(&value.inline_start) && padding_is_valid(&value.inline_end)
        }
        Property::PaddingTop(value)
        | Property::PaddingRight(value)
        | Property::PaddingBottom(value)
        | Property::PaddingLeft(value)
        | Property::PaddingBlockStart(value)
        | Property::PaddingBlockEnd(value)
        | Property::PaddingInlineStart(value)
        | Property::PaddingInlineEnd(value) => padding_is_valid(value),
        Property::Margin(value) => [&value.top, &value.right, &value.bottom, &value.left]
            .into_iter()
            .all(length_or_auto_has_valid_math),
        Property::MarginBlock(value) => {
            length_or_auto_has_valid_math(&value.block_start)
                && length_or_auto_has_valid_math(&value.block_end)
        }
        Property::MarginInline(value) => {
            length_or_auto_has_valid_math(&value.inline_start)
                && length_or_auto_has_valid_math(&value.inline_end)
        }
        Property::MarginTop(value)
        | Property::MarginRight(value)
        | Property::MarginBottom(value)
        | Property::MarginLeft(value)
        | Property::MarginBlockStart(value)
        | Property::MarginBlockEnd(value)
        | Property::MarginInlineStart(value)
        | Property::MarginInlineEnd(value)
        | Property::FlexBasis(value, _) => length_or_auto_has_valid_math(value),
        Property::Flex(value, _) => length_or_auto_has_valid_math(&value.basis),
        Property::BorderTopWidth(value)
        | Property::BorderRightWidth(value)
        | Property::BorderBottomWidth(value)
        | Property::BorderLeftWidth(value)
        | Property::BorderBlockStartWidth(value)
        | Property::BorderBlockEndWidth(value)
        | Property::BorderInlineStartWidth(value)
        | Property::BorderInlineEndWidth(value)
        | Property::OutlineWidth(value) => border_width_is_valid(value),
        Property::OutlineStyle(OutlineStyle::LineStyle(LineStyle::Hidden)) => false,
        Property::Outline(value) => {
            !matches!(value.style, OutlineStyle::LineStyle(LineStyle::Hidden))
                && border_width_is_valid(&value.width)
        }
        Property::Background(layers) => {
            let transparent = CssColor::RGBA(RGBA {
                red: 0,
                green: 0,
                blue: 0,
                alpha: 0,
            });
            // Only the final layer may carry a color. An explicit transparent
            // color is indistinguishable from the native AST's default.
            layers
                .iter()
                .take(layers.len().saturating_sub(1))
                .all(|layer| layer.color == transparent)
        }
        _ => super::grid::native_value_is_valid(property),
    }
}
