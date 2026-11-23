//! Public native-parser adapters and renderer capability queries.
use crate::style::support::{
    DeclarationSupport, PreparationError, PropertyCapability, PropertySyntax, PropertyValueSyntax,
};
use crate::style::syntax::capabilities::{prepare_support, property_name_is_supported};
use crate::style::syntax::property_id::canonical_property_id;
use cssparser::{Delimiter, Parser, ParserInput, parse_important};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::selector::SelectorList;
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::ParseWithOptions;

pub fn declaration_support(property_name: &str, value: &str) -> DeclarationSupport {
    let normalized_name = property_name.to_ascii_lowercase();
    if let Err(error) = prepare_support(&normalized_name, None) {
        return error.into();
    }
    let parsed = super::value::parsed_property(PropertyId::from(normalized_name.as_str()), value);
    parsed_declaration_support(&normalized_name, parsed.as_ref())
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

/// Both callers canonicalize aliases before parsing. Keep syntax and renderer
/// preparation results identical for public queries and CSS feature queries.
fn parsed_declaration_support(name: &str, property: Option<&Property<'_>>) -> DeclarationSupport {
    property
        .ok_or(PreparationError::InvalidSyntax)
        .and_then(|property| prepare_support(name, Some(property)))
        .map_or_else(DeclarationSupport::from, |()| DeclarationSupport {
            syntax: PropertySyntax::Valid,
            capability: PropertyCapability::Supported,
        })
}

/// Let the native declaration parser handle comments, escapes, and priority.
pub(crate) fn supports_declaration_is_supported(id: &PropertyId<'_>, value: &str) -> bool {
    if prepare_support(id.name(), None).is_err() {
        return false;
    }
    let id = canonical_property_id(id.clone());
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    let Ok(property) = parser.parse_until_before(Delimiter::Bang, |input| {
        Property::parse(id, input, &ParserOptions::default())
    }) else {
        return false;
    };
    if !parser.is_exhausted() && parser.try_parse(parse_important).is_err() {
        return false;
    }
    if parser.expect_exhausted().is_err() {
        return false;
    }
    parsed_declaration_support(property.property_id().name(), Some(&property)).syntax
        == PropertySyntax::Valid
}

/// Check selector syntax accepted by Lightning CSS.
pub fn selector_syntax_is_valid(selector: &str) -> bool {
    SelectorList::parse_string_with_options(selector, ParserOptions::default())
        .is_ok_and(|selectors| super::selectors::selector_list_is_web_valid(&selectors))
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
