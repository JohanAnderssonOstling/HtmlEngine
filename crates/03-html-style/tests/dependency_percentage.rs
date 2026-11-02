use html_style::{
    PropertySyntax, PropertyValueSyntax, declaration_support, declaration_syntax,
    property_value_syntax,
};
use lightningcss::traits::Parse;
use lightningcss::values::percentage::Percentage;

const MIXED_CALCULATIONS: [&str; 5] = [
    "min(1, 1%)",
    "max(1, 1%)",
    "round(1, 1%)",
    "mod(1, 1%)",
    "rem(1, 1%)",
];

#[test]
fn dependency_percentage_preserves_valid_percentage_calculations() {
    for (value, expected) in [
        ("50%", 0.5),
        ("-25%", -0.25),
        ("calc(25% + 25%)", 0.5),
        ("calc(25% * 2)", 0.5),
        ("min(25%, 50%)", 0.25),
        ("max(25%, 50%)", 0.5),
        ("clamp(25%, 50%, 75%)", 0.5),
        ("round(26%, 10%)", 0.3),
        ("mod(55%, 20%)", 0.15),
        ("rem(55%, 20%)", 0.15),
    ] {
        let actual = Percentage::parse_string(value)
            .unwrap_or_else(|error| panic!("valid percentage {value} failed to parse: {error:?}"));
        assert!(
            (actual.0 - expected).abs() < 0.000001,
            "{value}: {actual:?}"
        );
    }
}

#[test]
fn unsupported_properties_skip_syntax_validation() {
    for value in MIXED_CALCULATIONS.into_iter().chain([
        "50%",
        "calc(25% + 25%)",
        "min(25%, 50%)",
        "min(0.25, 0.5)",
        "var(--opacity)",
        "inherit",
    ]) {
        assert_eq!(
            property_value_syntax("opacity", value),
            PropertyValueSyntax::UnsupportedProperty
        );
        assert_eq!(
            declaration_syntax("opacity", value),
            PropertySyntax::Unsupported
        );
        assert_eq!(
            declaration_support("opacity", value).syntax,
            PropertySyntax::Unsupported
        );
    }
}
