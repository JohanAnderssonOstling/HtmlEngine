//! Public native-parser adapters and renderer capability queries.
use crate::style::support::{
    DeclarationSupport, PropertyCapability, PropertySyntax, PropertyValueSyntax,
};
use crate::style::syntax::capabilities::{
    parsed_declaration_capability, property_name_is_supported,
};
use cssparser::{Delimiter, Parser, ParserInput, parse_important};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::selector::SelectorList;
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::ParseWithOptions;

pub fn declaration_support(property_name: &str, value: &str) -> DeclarationSupport {
    let normalized_name = property_name.to_ascii_lowercase();
    let capability = parsed_declaration_capability(&normalized_name, None);
    if capability != PropertyCapability::Supported {
        return DeclarationSupport {
            syntax: PropertySyntax::Unsupported,
            capability,
        };
    }
    let parsed = super::value::parsed_property(PropertyId::from(normalized_name.as_str()), value);
    let capability = parsed_declaration_capability(&normalized_name, parsed.as_ref());
    let syntax = if capability == PropertyCapability::Supported {
        declaration_syntax_impl(parsed.as_ref(), super::value::css_wide_keyword(value))
    } else {
        PropertySyntax::Unsupported
    };
    DeclarationSupport { syntax, capability }
}

pub fn declaration_syntax(property_name: &str, value: &str) -> PropertySyntax {
    declaration_support(property_name, value).syntax
}

pub fn property_value_syntax(property_name: &str, value: &str) -> PropertyValueSyntax {
    if !property_name_is_supported(property_name) {
        return PropertyValueSyntax::UnsupportedProperty;
    }
    match declaration_support(property_name, value).syntax {
        PropertySyntax::Valid => PropertyValueSyntax::Valid,
        PropertySyntax::Invalid => PropertyValueSyntax::Invalid,
        PropertySyntax::Unsupported => PropertyValueSyntax::UnsupportedValue,
    }
}

fn declaration_syntax_impl(property: Option<&Property<'_>>, css_wide: bool) -> PropertySyntax {
    if css_wide {
        return PropertySyntax::Valid;
    }
    match property {
        Some(Property::Unparsed(raw)) if !super::substitution::needs_substitution(&raw.value) => {
            PropertySyntax::Invalid
        }
        Some(_) => PropertySyntax::Valid,
        None => PropertySyntax::Invalid,
    }
}

/// Let the native declaration parser handle comments, escapes, and priority.
pub(crate) fn supports_declaration_is_supported(id: &PropertyId<'_>, value: &str) -> bool {
    if parsed_declaration_capability(id.name(), None) != PropertyCapability::Supported {
        return false;
    }
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let Ok(property) = parser.parse_until_before(Delimiter::Bang, |input| {
        Property::parse(id.clone(), input, &ParserOptions::default())
    }) else {
        return false;
    };
    if !parser.is_exhausted() && parser.try_parse(parse_important).is_err() {
        return false;
    }
    if parser.expect_exhausted().is_err() {
        return false;
    }
    parsed_declaration_capability(id.name(), Some(&property)) == PropertyCapability::Supported
        && declaration_syntax_impl(
            Some(&property),
            super::wide_keywords::property_css_wide_keyword(&property).is_some(),
        ) == PropertySyntax::Valid
}

/// Check selector syntax accepted by Lightning CSS.
pub fn selector_syntax_is_valid(selector: &str) -> bool {
    SelectorList::parse_string_with_options(selector, ParserOptions::default()).is_ok()
}

pub(crate) fn supports_selector_syntax_is_valid(selector: &str) -> bool {
    SelectorList::parse_string_with_options(selector, ParserOptions::default()).is_ok_and(
        |selectors| super::selectors::selector_list_is_web_valid_for_supports(&selectors),
    )
}

/// Parse a complete stylesheet/rule string without exposing the parser's AST.
pub fn stylesheet_syntax_is_valid(css: &str) -> bool {
    super::rules::stylesheet_is_valid(css)
}
