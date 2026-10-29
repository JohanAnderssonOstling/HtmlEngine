//! Identify CSS-wide values in the native property representation.
use crate::style::syntax::value::single_ident_keyword;
use lightningcss::properties::custom::TokenList;
use lightningcss::properties::font::{FontFamily, GenericFontFamily};
use lightningcss::properties::{CSSWideKeyword, Property};

pub(crate) fn token_list_css_wide_keyword(tokens: &TokenList<'_>) -> Option<&'static str> {
    let keyword = single_ident_keyword(tokens)?;
    ["inherit", "initial", "unset", "revert", "revert-layer"]
        .iter()
        .find(|candidate| keyword.eq_ignore_ascii_case(candidate))
        .copied()
}

pub(crate) fn property_css_wide_keyword(property: &Property<'_>) -> Option<&'static str> {
    match property {
        Property::All(keyword) => Some(match keyword {
            CSSWideKeyword::Initial => "initial",
            CSSWideKeyword::Inherit => "inherit",
            CSSWideKeyword::Unset => "unset",
            CSSWideKeyword::Revert => "revert",
            CSSWideKeyword::RevertLayer => "revert-layer",
        }),
        // The native parser stores unquoted CSS-wide family keywords as
        // generic families. Quoted names remain FamilyName values.
        Property::FontFamily(families) => match families.as_slice() {
            [FontFamily::Generic(GenericFontFamily::Inherit)] => Some("inherit"),
            [FontFamily::Generic(GenericFontFamily::Initial)] => Some("initial"),
            [FontFamily::Generic(GenericFontFamily::Unset)] => Some("unset"),
            [FontFamily::Generic(GenericFontFamily::Revert)] => Some("revert"),
            [FontFamily::Generic(GenericFontFamily::RevertLayer)] => Some("revert-layer"),
            _ => None,
        },
        Property::Unparsed(value) => token_list_css_wide_keyword(&value.value),
        Property::Custom(value) if !value.name.as_ref().starts_with("--") => {
            token_list_css_wide_keyword(&value.value)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{property_css_wide_keyword, token_list_css_wide_keyword};
    use crate::allocation_test_support::count_allocations;
    use lightningcss::properties::custom::TokenList;
    use lightningcss::properties::{Property, PropertyId};
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::traits::ParseWithOptions;

    #[test]
    fn native_keyword_recognition_preserves_boundaries_and_never_allocates() {
        for (css, expected) in [
            ("inherit", Some("inherit")),
            ("InItIaL", Some("initial")),
            ("/*before*/ UnSeT /**/", Some("unset")),
            ("REVERT", Some("revert")),
            ("revert-layer", Some("revert-layer")),
            ("\\69nherit", Some("inherit")),
            ("", None),
            ("'initial'", None),
            ("inherit initial", None),
            ("var(--keyword)", None),
            ("MiXeD", None),
        ] {
            let tokens =
                TokenList::parse_string_with_options(css, ParserOptions::default()).unwrap();
            let (keyword, allocations) = count_allocations(|| token_list_css_wide_keyword(&tokens));
            assert_eq!(keyword, expected, "{css}");
            assert_eq!(allocations, 0, "{css}");
        }
    }

    #[test]
    fn property_keywords_keep_custom_properties_and_quoted_families_separate() {
        for (name, css, expected) in [
            ("all", "uNsEt", Some("unset")),
            ("color", "InHeRiT", Some("inherit")),
            ("object-fit", "INITIAL", Some("initial")),
            ("font-family", "revert-layer", Some("revert-layer")),
            ("font-family", "'inherit'", None),
            ("font-family", "'initial', serif", None),
            ("--theme", "initial", None),
            ("--Theme", "InHeRiT", None),
        ] {
            let property =
                Property::parse_string(PropertyId::from(name), css, ParserOptions::default())
                    .unwrap();
            let (keyword, allocations) = count_allocations(|| property_css_wide_keyword(&property));
            assert_eq!(keyword, expected, "{name}:{css}");
            assert_eq!(allocations, 0, "{name}:{css}");
        }
    }
}
