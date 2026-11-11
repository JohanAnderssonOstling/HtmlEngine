//! Identify CSS-wide values in the native property representation.
use crate::style::syntax::value::single_ident_keyword;
use lightningcss::properties::custom::TokenList;
use lightningcss::properties::font::{FontFamily, GenericFontFamily};
use lightningcss::properties::{CSSWideKeyword, Property};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CssWideKeyword {
    Initial,
    Inherit,
    Unset,
    Revert,
    RevertLayer,
}

impl CssWideKeyword {
    pub(crate) fn parse(keyword: &str) -> Option<Self> {
        [
            ("initial", Self::Initial),
            ("inherit", Self::Inherit),
            ("unset", Self::Unset),
            ("revert", Self::Revert),
            ("revert-layer", Self::RevertLayer),
        ]
        .into_iter()
        .find_map(|(name, value)| keyword.eq_ignore_ascii_case(name).then_some(value))
    }
}

pub(crate) fn token_list_css_wide_keyword(tokens: &TokenList<'_>) -> Option<CssWideKeyword> {
    CssWideKeyword::parse(single_ident_keyword(tokens)?)
}

pub(crate) fn property_css_wide_keyword(property: &Property<'_>) -> Option<CssWideKeyword> {
    match property {
        Property::All(keyword) => Some(match keyword {
            CSSWideKeyword::Initial => CssWideKeyword::Initial,
            CSSWideKeyword::Inherit => CssWideKeyword::Inherit,
            CSSWideKeyword::Unset => CssWideKeyword::Unset,
            CSSWideKeyword::Revert => CssWideKeyword::Revert,
            CSSWideKeyword::RevertLayer => CssWideKeyword::RevertLayer,
        }),
        // The native parser stores unquoted CSS-wide family keywords as
        // generic families. Quoted names remain FamilyName values.
        Property::FontFamily(families) => match families.as_slice() {
            [FontFamily::Generic(GenericFontFamily::Inherit)] => Some(CssWideKeyword::Inherit),
            [FontFamily::Generic(GenericFontFamily::Initial)] => Some(CssWideKeyword::Initial),
            [FontFamily::Generic(GenericFontFamily::Unset)] => Some(CssWideKeyword::Unset),
            [FontFamily::Generic(GenericFontFamily::Revert)] => Some(CssWideKeyword::Revert),
            [FontFamily::Generic(GenericFontFamily::RevertLayer)] => {
                Some(CssWideKeyword::RevertLayer)
            }
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
    use super::{CssWideKeyword, property_css_wide_keyword, token_list_css_wide_keyword};
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
            assert_eq!(keyword, expected.and_then(CssWideKeyword::parse), "{css}");
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
            assert_eq!(
                keyword,
                expected.and_then(CssWideKeyword::parse),
                "{name}:{css}"
            );
            assert_eq!(allocations, 0, "{name}:{css}");
        }
    }
}
