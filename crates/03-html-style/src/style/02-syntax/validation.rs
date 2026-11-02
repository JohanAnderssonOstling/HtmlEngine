//! Public native-parser adapters and renderer capability queries.
use crate::style::support::{
    DeclarationSupport, PropertyCapability, PropertySyntax, PropertyValueSyntax,
};
use crate::style::syntax::capabilities::{
    parsed_declaration_capability, property_name_is_supported,
};
use crate::style::syntax::property_id::{RendererPropertyId, canonical_property_id};
use crate::style::syntax::renderer_owned::{parse_object_position, parse_outline_offset};
use crate::style::syntax::value::token_list_to_css_string;
use crate::style::syntax::wide_keywords::property_css_wide_keyword;
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
    if parsed
        .as_ref()
        .is_some_and(|property| !super::ast_validation::property_is_valid(property))
    {
        return DeclarationSupport {
            syntax: PropertySyntax::Invalid,
            capability: PropertyCapability::Supported,
        };
    }
    let capability = parsed_declaration_capability(&normalized_name, parsed.as_ref());
    let syntax = if capability == PropertyCapability::Supported {
        declaration_syntax_impl(parsed.as_ref())
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

pub(crate) fn declaration_syntax_impl(property: Option<&Property<'_>>) -> PropertySyntax {
    if property.is_some_and(|property| !super::ast_validation::property_is_valid(property)) {
        return PropertySyntax::Invalid;
    }
    if property.and_then(property_css_wide_keyword).is_some() {
        return PropertySyntax::Valid;
    }
    match property {
        Some(property @ Property::Unparsed(raw))
            if super::native_fallback::applies_to(&raw.property_id)
                && super::native_fallback::parse(property).is_some() => PropertySyntax::Valid,
        Some(Property::Unparsed(raw)) if raw.property_id == PropertyId::FontPalette
            && super::renderer_owned::font_palette_keyword(&raw.value).is_some() => {
            PropertySyntax::Valid
        }
        Some(Property::Unparsed(raw)) if !super::substitution::needs_substitution(&raw.value) => {
            PropertySyntax::Invalid
        }
        Some(Property::Custom(raw)) if !super::substitution::needs_substitution(&raw.value) => {
            if let Some(id) = RendererPropertyId::from_name(raw.name.as_ref())
                .and_then(RendererPropertyId::native_property)
            {
                return token_list_to_css_string(&raw.value)
                    .map(|css| {
                        declaration_syntax_impl(super::value::parsed_property(id, &css).as_ref())
                    })
                    .unwrap_or(PropertySyntax::Invalid);
            }
            let valid = match RendererPropertyId::from_name(raw.name.as_ref()) {
                Some(RendererPropertyId::ObjectPosition) => token_list_to_css_string(&raw.value)
                    .and_then(|css| parse_object_position(&css))
                    .is_some(),
                Some(RendererPropertyId::OutlineOffset) => token_list_to_css_string(&raw.value)
                    .and_then(|css| parse_outline_offset(&css))
                    .is_some(),
                _ => true,
            };
            if valid {
                PropertySyntax::Valid
            } else {
                PropertySyntax::Invalid
            }
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
    parsed_declaration_capability(property.property_id().name(), Some(&property))
        == PropertyCapability::Supported
        && declaration_syntax_impl(Some(&property)) == PropertySyntax::Valid
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
