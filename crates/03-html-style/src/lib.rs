#[cfg(test)]
pub(crate) mod document {
    pub use html_dom::*;
    pub use html_style_model::*;
}

#[cfg(test)]
pub(crate) mod parser {
    pub(crate) struct DocumentFactory;

    impl DocumentFactory {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn parse_with_new_pipeline(&mut self, html: &str, css: Option<&str>) -> crate::StyledDocument {
            match css {
                Some(css) => self.parse_with_new_pipeline_css_chunks(html, &[css]),
                None => self.parse_with_new_pipeline_css_chunks(html, &[]),
            }
        }

        pub(crate) fn parse_with_new_pipeline_css_chunks(&mut self, html: &str, css: &[&str]) -> crate::StyledDocument {
            let standards_html = if html.trim_start().to_ascii_lowercase().starts_with("<!doctype html") { html.to_owned() } else { format!("<!doctype html>{html}") };
            let document = html_parse::parse_dom_document(&standards_html).expect("parser produced a valid standards-mode DOM");
            crate::style_document(document, css)
        }
    }
}

#[path = "style/mod.rs"]
mod style;

pub use style::DEFAULT_CSS;
pub use style::source::imports::{ImportLayer, StylesheetEntry, StylesheetImport, stylesheet_entries};
pub use style::syntax::capabilities::property_name_is_supported;

use html_dom::{Document, DomNodeId};
use html_style_model::{Background, Border, BorderRadii, BoxModel, ComputedStyles, Font, InheritedText, StyleIndices, StyleView};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::selector::SelectorList;
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use lightningcss::traits::ParseWithOptions;
use std::time::Duration;

pub use style::rules::media::{MediaEnvironment, MediaMatchKey, MediaQuerySet, MediaType};
pub use style::rules::program::{StyleProgram, StyleProgramCache, StyleProgramCacheStats, style_document_with_cached_program_and_timings, style_document_with_program_and_timings};

#[cfg(test)]
mod allocation_test_support {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ALLOCATION_COUNT: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub(crate) struct TrackingAllocator;

    fn record_allocation() {
        let _ = ALLOCATION_COUNT.try_with(|count| {
            if let Some(current) = count.get() {
                count.set(Some(current + 1));
            }
        });
    }

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_allocation();
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    pub(crate) fn count_allocations<T>(operation: impl FnOnce() -> T) -> (T, usize) {
        ALLOCATION_COUNT.with(|count| {
            assert!(count.get().is_none(), "allocation counters cannot be nested");
            count.set(Some(0));
        });
        let output = operation();
        let allocations = ALLOCATION_COUNT.with(|count| count.take().expect("allocation tracking is active"));
        (output, allocations)
    }
}

#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: allocation_test_support::TrackingAllocator = allocation_test_support::TrackingAllocator;

/// Result of checking a declaration against the CSS parser owned by this
/// rendering stage. Unknown properties are kept distinct from invalid values
/// so conformance adapters cannot accidentally count them as passes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyValueSyntax {
    Valid,
    Invalid,
    UnsupportedProperty,
}

/// Whether a declaration's value is valid for its property, independent of
/// whether this renderer can compute that property.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertySyntax {
    Valid,
    Invalid,
}

/// Whether the style stage implements the property's computed behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyCapability {
    Supported,
    Unsupported(UnsupportedStyleFeature),
}

/// Why syntactically recognizable CSS cannot participate in this renderer's
/// cascade. Keeping the reason in the type prevents parser acceptance from
/// being mistaken for rendered support.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedStyleFeature {
    Property,
    Value,
    Gradient,
    GeneratedContent,
    BackgroundImage,
    BorderImage,
    BoxShadow,
    FilterEffects,
    Transitions,
    Animations,
}

/// The two independent answers needed by feature queries and conformance
/// adapters. Valid CSS is not automatically a supported renderer feature.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeclarationSupport {
    pub syntax: PropertySyntax,
    pub capability: PropertyCapability,
}

pub fn declaration_support(property_name: &str, value: &str) -> DeclarationSupport {
    let normalized_name = property_name.to_ascii_lowercase();
    let syntax = declaration_syntax_impl(&normalized_name, value);
    let capability = style::syntax::capabilities::declaration_capability(&normalized_name, value);
    DeclarationSupport { syntax, capability }
}

pub fn declaration_syntax(property_name: &str, value: &str) -> PropertySyntax {
    declaration_syntax_impl(&property_name.to_ascii_lowercase(), value)
}

pub fn property_value_syntax(property_name: &str, value: &str) -> PropertyValueSyntax {
    let normalized_name = property_name.to_ascii_lowercase();
    let property_id = PropertyId::from(normalized_name.as_str());
    if matches!(property_id, PropertyId::Custom(_)) && !normalized_name.starts_with("--") && !style::syntax::capabilities::is_supported_property_name(&normalized_name) {
        return PropertyValueSyntax::UnsupportedProperty;
    }
    match declaration_syntax_impl(&normalized_name, value) {
        PropertySyntax::Valid => PropertyValueSyntax::Valid,
        PropertySyntax::Invalid => PropertyValueSyntax::Invalid,
    }
}

fn declaration_syntax_impl(normalized_name: &str, value: &str) -> PropertySyntax {
    // cssparser-color currently asserts while resolving non-finite hue math.
    // Non-finite CSS math constants are nevertheless valid syntax, so validate
    // the surrounding grammar with finite stand-ins at this boundary.
    let finite_math_value = finite_css_math_surrogate(value);
    let value = finite_math_value.as_deref().unwrap_or(value);
    if normalized_name == "white-space" && style::syntax::values::white_space::parse_shorthand(value).is_some() {
        return PropertySyntax::Valid;
    }
    if matches!(normalized_name, "letter-spacing" | "word-spacing") {
        let normalized_value = value.trim().to_ascii_lowercase();
        return if style::syntax::values::text_spacing::parse(value).is_some() || matches!(normalized_value.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer") || value.contains("var(") || value.contains("env(") {
            PropertySyntax::Valid
        } else {
            PropertySyntax::Invalid
        };
    }
    if normalized_name == "tab-size" {
        let normalized_value = value.trim().to_ascii_lowercase();
        return if style::syntax::values::tab_size::parse(value).is_some() || matches!(normalized_value.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer") || value.contains("var(") || value.contains("env(") {
            PropertySyntax::Valid
        } else {
            PropertySyntax::Invalid
        };
    }
    if normalized_name == "font-kerning" {
        let normalized_value = value.trim().to_ascii_lowercase();
        return if style::cascade::resolver::parse_font_kerning(value).is_some() || matches!(normalized_value.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer") || value.contains("var(") || value.contains("env(") {
            PropertySyntax::Valid
        } else {
            PropertySyntax::Invalid
        };
    }
    if normalized_name == "contain" {
        let normalized_value = value.trim().to_ascii_lowercase();
        return if style::syntax::contain::parse(value).is_some()
            || matches!(normalized_value.as_str(), "inherit" | "initial" | "unset" | "revert" | "revert-layer")
            || value.contains("var(")
            || value.contains("env(")
        {
            PropertySyntax::Valid
        } else {
            PropertySyntax::Invalid
        };
    }
    if matches!(
        normalized_name,
        "content" | "counter-reset" | "counter-increment" | "quotes"
    ) {
        let support = match normalized_name {
            "content" => style::syntax::generated_content::content(value),
            "counter-reset" | "counter-increment" => {
                style::syntax::generated_content::counter_directive(normalized_name, value)
            }
            "quotes" => style::syntax::generated_content::quotes(value),
            _ => unreachable!(),
        };
        return if support.is_valid() || value.contains("var(") || value.contains("env(") {
            PropertySyntax::Valid
        } else {
            PropertySyntax::Invalid
        };
    }
    if matches!(
        normalized_name,
        "table-layout"
            | "border-collapse"
            | "caption-side"
            | "empty-cells"
            | "text-box"
            | "text-box-trim"
            | "text-box-edge"
    ) {
        return if style::syntax::renderer_owned::value_is_css_wide_keyword(value)
            || value.contains("var(")
            || value.contains("env(")
            || style::syntax::renderer_owned::property_value_is_valid(normalized_name, value)
                == Some(true)
        {
            PropertySyntax::Valid
        } else {
            PropertySyntax::Invalid
        };
    }
    if let Some(valid) = style::syntax::box_syntax::property_value_is_valid(normalized_name, value) {
        return if valid { PropertySyntax::Valid } else { PropertySyntax::Invalid };
    }
    // The legacy `grid-*-gap` aliases are retained by CSS Box Alignment.
    // Lightning CSS does not expose typed IDs for them, so validate their
    // values with the canonical property grammar.
    if matches!(normalized_name, "grid-row-gap" | "grid-column-gap" | "grid-gap") {
        let direct = value.trim();
        let is_unitless_number = direct.parse::<f64>().is_ok_and(|number| number != 0.0);
        let is_direct_negative = direct.strip_prefix('-').and_then(|rest| rest.chars().next()).is_some_and(|next| next.is_ascii_digit() || next == '.');
        if is_unitless_number || is_direct_negative {
            return PropertySyntax::Invalid;
        }
    }
    let parsed_name = match normalized_name {
        "grid-row-gap" => "row-gap",
        "grid-column-gap" => "column-gap",
        "grid-gap" => "gap",
        _ => normalized_name,
    };
    let property_id = PropertyId::from(parsed_name);
    let unknown_property = matches!(property_id, PropertyId::Custom(_)) && !normalized_name.starts_with("--") && !style::syntax::capabilities::is_supported_property_name(normalized_name);

    match Property::parse_string(property_id, value, ParserOptions::default()) {
        // Lightning CSS deliberately preserves values it cannot validate as
        // `Unparsed`. That is useful to a transforming stylesheet pipeline,
        // but this API is a syntax validator, so it must not turn that
        // recovery representation into a successful parse.
        Ok(Property::Unparsed(_)) if unknown_property || value.contains("var(") || value.contains("env(") => PropertySyntax::Valid,
        Ok(Property::Unparsed(_)) => PropertySyntax::Invalid,
        Ok(_) => PropertySyntax::Valid,
        Err(_) => PropertySyntax::Invalid,
    }
}

fn finite_css_math_surrogate(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let first_replacement = (0..bytes.len()).find(|cursor| css_math_replacement(bytes, *cursor).is_some())?;
    let mut output = String::with_capacity(value.len());
    output.push_str(&value[..first_replacement]);
    let mut cursor = first_replacement;
    while cursor < bytes.len() {
        if let Some((length, replacement)) = css_math_replacement(bytes, cursor) {
            output.push_str(replacement);
            cursor += length;
        } else {
            let character = value[cursor..].chars().next().expect("cursor remains on a character boundary");
            output.push(character);
            cursor += character.len_utf8();
        }
    }
    Some(output)
}

fn css_math_replacement(bytes: &[u8], cursor: usize) -> Option<(usize, &'static str)> {
    if bytes.get(cursor..cursor + 8).is_some_and(|token| token.eq_ignore_ascii_case(b"infinity")) && css_identifier_boundary(bytes, cursor, cursor + 8) {
        Some((8, "1"))
    } else if bytes.get(cursor..cursor + 3).is_some_and(|token| token.eq_ignore_ascii_case(b"nan")) && css_identifier_boundary(bytes, cursor, cursor + 3) {
        Some((3, "0"))
    } else {
        None
    }
}

fn css_identifier_boundary(bytes: &[u8], start: usize, end: usize) -> bool {
    let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    start.checked_sub(1).and_then(|index| bytes.get(index)).is_none_or(|byte| !identifier(*byte)) && bytes.get(end).is_none_or(|byte| !identifier(*byte))
}

/// Parse selector syntax without exposing Lightning CSS types across the
/// style-stage boundary.
pub fn selector_syntax_is_valid(selector: &str) -> bool {
    SelectorList::parse_string_with_options(selector, ParserOptions::default()).is_ok_and(|selectors| style::matching::selectors::selector_list_is_web_valid(&selectors))
}

fn supports_selector_syntax_is_valid(selector: &str) -> bool {
    SelectorList::parse_string_with_options(selector, ParserOptions::default()).is_ok_and(|selectors| style::matching::selectors::selector_list_is_web_valid_for_supports(&selectors))
}

/// Parse a complete stylesheet/rule string without exposing the parser's AST.
pub fn stylesheet_syntax_is_valid(css: &str) -> bool {
    StyleSheet::parse(css, ParserOptions::default()).is_ok()
}

/// A DOM whose cascade has been resolved.
///
/// The wrapped document is deliberately immutable. Changing the DOM or its
/// style inputs requires consuming this value with [`Self::into_unstyled`],
/// which prevents layout data from silently outliving the styles it used.
pub struct StyledDocument {
    document: Document,
    styles: ComputedStyles,
}

impl StyledDocument {
    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn styles(&self) -> &ComputedStyles {
        &self.styles
    }

    pub fn style_for_node(&self, node_idx: DomNodeId) -> Option<StyleIndices> {
        self.styles.style_for_node(node_idx)
    }

    pub fn style_view(&self, indices: StyleIndices) -> Option<StyleView<'_>> {
        self.styles.view(indices)
    }

    pub fn font_style(&self, indices: StyleIndices) -> Option<&Font> {
        self.styles.font_style(indices)
    }

    pub fn text_style(&self, indices: StyleIndices) -> Option<&InheritedText> {
        self.styles.text_style(indices)
    }

    pub fn box_model_style(&self, indices: StyleIndices) -> Option<&BoxModel> {
        self.styles.box_model_style(indices)
    }

    pub fn border_style(&self, indices: StyleIndices) -> Option<&Border> {
        self.styles.border_style(indices)
    }

    pub fn border_radii_style(&self, indices: StyleIndices) -> Option<&BorderRadii> {
        self.styles.border_radii_style(indices)
    }

    pub fn background_style(&self, indices: StyleIndices) -> Option<&Background> {
        self.styles.background_style(indices)
    }

    pub fn into_unstyled(self) -> Document {
        self.document
    }

    pub fn into_parts(self) -> (Document, ComputedStyles) {
        (self.document, self.styles)
    }
}

/// Resolve the user-agent stylesheet and all author stylesheet chunks.
///
/// Invalid author chunks are skipped, matching the renderer's previous
/// best-effort behavior. Lightning CSS values remain an implementation detail
/// of this crate rather than leaking into the pipeline API.
pub fn style_document(document: Document, css_chunks: &[&str]) -> StyledDocument {
    style_document_with_timings(document, css_chunks).0
}

#[derive(Clone, Debug, Default)]
pub struct StyleTimings {
    pub parse_default_css: Duration,
    pub parse_author_css: Duration,
    pub prepare_rules: Duration,
    pub resolve_styles: Duration,
    pub selector_index: Duration,
    pub resolver_setup: Duration,
    pub selector_matching: Duration,
    pub cascade: Duration,
    pub style_store: Duration,
}

#[derive(Clone, Copy)]
pub struct AuthorStylesheetInput<'a> {
    pub css: &'a str,
    /// The element that owns this stylesheet. It becomes the root for an
    /// `@scope` rule whose prelude is omitted.
    pub implicit_scope_root: Option<html_dom::DomNodeId>,
}

pub fn style_document_with_timings(document: Document, css_chunks: &[&str]) -> (StyledDocument, StyleTimings) {
    let (styled, timings, _) = style_document_with_environment_and_timings(document, css_chunks, MediaEnvironment::default());
    (styled, timings)
}

pub fn style_document_with_environment(document: Document, css_chunks: &[&str], environment: MediaEnvironment) -> (StyledDocument, MediaQuerySet) {
    let (styled, _, media_queries) = style_document_with_environment_and_timings(document, css_chunks, environment);
    (styled, media_queries)
}

pub fn style_document_with_environment_and_timings(document: Document, css_chunks: &[&str], environment: MediaEnvironment) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    let root = document.dom_root();
    let inputs = css_chunks.iter().map(|css| AuthorStylesheetInput { css, implicit_scope_root: root }).collect::<Vec<_>>();
    style_document_with_author_stylesheets_and_environment_and_timings(document, &inputs, environment)
}

pub fn style_document_with_author_stylesheets_and_environment_and_timings(document: Document, inputs: &[AuthorStylesheetInput<'_>], environment: MediaEnvironment) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    style::rules::program::compile_and_apply(document, inputs, environment)
}

#[cfg(test)]
mod boundary_tests {
    use super::{
        AuthorStylesheetInput, DeclarationSupport, MediaEnvironment, PropertyCapability, PropertySyntax, PropertyValueSyntax, StyleProgramCache, UnsupportedStyleFeature, declaration_support, property_value_syntax, selector_syntax_is_valid, style_document,
        style_document_with_cached_program_and_timings, style_document_with_environment,
    };
    use html_style_model::{Float, LengthPct, PreferredSize, SizeComparison, TabSizeKind, WhiteSpace, resolve_used_preferred_size};
    use std::time::Duration;

    fn standards_document(html: &str) -> html_dom::Document {
        html_parse::parse_dom_document(&format!("<!doctype html>{html}")).expect("valid standards-mode HTML")
    }

    #[test]
    fn compiled_style_programs_are_reused_and_rebind_implicit_scope_roots() {
        let css = "@scope { p { color: red } }";
        let mut cache = StyleProgramCache::default();
        for expected_hits in [0, 1] {
            let document = standards_document("<html><body><p>cached</p></body></html>");
            let root = document.dom_root();
            let inputs = [AuthorStylesheetInput { css, implicit_scope_root: root }];
            let (styled, timings, _) = style_document_with_cached_program_and_timings(&mut cache, document, &inputs, MediaEnvironment::default());
            let paragraph = styled.document().node_ids().find(|node| styled.document().get_dom_tag(*node) == Some("p")).expect("paragraph");
            let style = styled.style_for_node(paragraph).expect("paragraph style");
            assert_eq!(styled.text_style(style).expect("paragraph text style").color, 0xFF0000FF);
            assert_eq!(cache.stats().hits, expected_hits);
            if expected_hits == 1 {
                assert_eq!(timings.parse_default_css + timings.parse_author_css + timings.prepare_rules + timings.selector_index, Duration::ZERO);
            }
        }
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn property_syntax_keeps_invalid_and_unsupported_states_distinct() {
        assert_eq!(property_value_syntax("width", "10px"), PropertyValueSyntax::Valid);
        assert_eq!(property_value_syntax("width", "10px 20px"), PropertyValueSyntax::Invalid);
        assert_eq!(property_value_syntax("width", "var(--measure)"), PropertyValueSyntax::Valid);
        assert_eq!(property_value_syntax("not-a-renderer-property", "10px"), PropertyValueSyntax::UnsupportedProperty);
        assert_eq!(property_value_syntax("letter-spacing", "120%"), PropertyValueSyntax::Valid);
        assert_eq!(property_value_syntax("word-spacing", "calc(2ch - 30%)"), PropertyValueSyntax::Valid);
        assert_eq!(property_value_syntax("letter-spacing", "20"), PropertyValueSyntax::Invalid);
        assert_eq!(property_value_syntax("tab-size", "4"), PropertyValueSyntax::Valid);
        assert_eq!(property_value_syntax("tab-size", "2.5ch"), PropertyValueSyntax::Valid);
        assert_eq!(property_value_syntax("tab-size", "-20"), PropertyValueSyntax::Invalid);
        assert_eq!(property_value_syntax("tab-size", "-10px"), PropertyValueSyntax::Invalid);
        assert_eq!(property_value_syntax("color", "hsl(calc(infinity) 100% 50%)"), PropertyValueSyntax::Valid);
        assert_eq!(property_value_syntax("color", "rgb(calc(NaN), 0, 0)"), PropertyValueSyntax::Valid);
        assert_eq!(property_value_syntax("tab-size", "20%"), PropertyValueSyntax::Invalid);
    }

    #[test]
    fn valid_syntax_and_renderer_capability_are_independent() {
        assert_eq!(declaration_support("width", "10px"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("width", "red"), DeclarationSupport { syntax: PropertySyntax::Invalid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("opacity", "0.5"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Property) });
        assert_eq!(declaration_support("future-property", "some-value"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Property) });
        assert_eq!(declaration_support("grid-row-gap", "33px"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("grid-column-gap", "-1px"), DeclarationSupport { syntax: PropertySyntax::Invalid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("grid-row-gap", "10"), DeclarationSupport { syntax: PropertySyntax::Invalid, capability: PropertyCapability::Supported });
    }

    #[test]
    fn gradients_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(declaration_support("background", "linear-gradient(red, blue)"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Gradient) });
        assert_eq!(declaration_support("background", "red"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("background", "url('paper.png') blue"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::BackgroundImage) });
    }

    #[test]
    fn generated_content_support_is_value_aware() {
        assert_eq!(declaration_support("content", "'Chapter ' attr(title) counter(chapter, upper-roman)"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("counter-reset", "chapter 1 section"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("counter-increment", "chapter -1"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("quotes", "'«' '»' '‹' '›'"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("content", "url('marker.svg')"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::GeneratedContent) });
        assert_eq!(declaration_support("content", "'label' / counter(chapter)"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::GeneratedContent) });
        assert_eq!(declaration_support("content", "attr(title string)"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::GeneratedContent) });
        assert_eq!(declaration_support("counter-reset", "reversed(chapter)"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::GeneratedContent) });
        assert_eq!(declaration_support("content", "counter()"), DeclarationSupport { syntax: PropertySyntax::Invalid, capability: PropertyCapability::Supported });
    }

    #[test]
    fn rendered_raw_and_typed_properties_are_reported_as_supported() {
        for (name, value) in [
            ("all", "initial"),
            ("overflow", "hidden auto"),
            ("z-index", "2"),
            ("order", "-1"),
            ("grid-gap", "10px 20%"),
            ("table-layout", "fixed"),
            ("border-collapse", "collapse"),
            ("caption-side", "bottom"),
            ("empty-cells", "hide"),
            ("text-box-trim", "trim-start"),
            ("text-box-edge", "cap alphabetic"),
            ("text-box", "trim-start cap alphabetic"),
        ] {
            assert_eq!(declaration_support(name, value), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Supported }, "{name}: {value}");
        }

        for (name, value) in [
            ("table-layout", "auto fixed"),
            ("border-collapse", "none"),
            ("text-box-edge", "text cap"),
            ("text-box", "cap none alphabetic"),
        ] {
            assert_eq!(declaration_support(name, value).syntax, PropertySyntax::Invalid, "{name}: {value}");
        }
    }

    #[test]
    fn border_images_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(declaration_support("border-image", "url('frame.png') 30 round"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::BorderImage) });
        assert_eq!(declaration_support("border-image-slice", "30 fill"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::BorderImage) });
    }

    #[test]
    fn box_shadows_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(declaration_support("BOX-SHADOW", "2px 4px 8px black"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::BoxShadow) });
    }

    #[test]
    fn filter_effects_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(declaration_support("filter", "blur(2px)"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::FilterEffects) });
        assert_eq!(declaration_support("backdrop-filter", "contrast(150%)"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::FilterEffects) });
    }

    #[test]
    fn transitions_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(declaration_support("transition", "color 200ms ease-in"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Transitions) });
        assert_eq!(declaration_support("transition-duration", "200ms"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Transitions) });
    }

    #[test]
    fn animations_are_recognized_but_explicitly_not_rendered() {
        assert_eq!(declaration_support("animation", "chapter-fade 200ms ease-in"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Animations) });
        assert_eq!(declaration_support("animation-duration", "200ms"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Unsupported(UnsupportedStyleFeature::Animations) });
    }

    #[test]
    fn strict_selector_validation_rejects_custom_recovery_nodes() {
        assert!(selector_syntax_is_valid("p:not(.excluded)"));
        assert!(selector_syntax_is_valid(":heading(1, 2)"));
        assert!(!selector_syntax_is_valid(":heading(2n)"));
        assert!(!selector_syntax_is_valid("p:not(:unknown)"));
        assert!(!selector_syntax_is_valid(".a:has(.b:has(.c))"));
        assert!(!selector_syntax_is_valid("p, :unknown"));
    }

    #[test]
    fn invalid_selector_lists_cannot_leak_declarations_into_the_cascade() {
        let styled = styled_paragraph("p { color: green; } p, :unknown { color: red; } p:not(:unknown) { color: red; }");
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn a_bad_author_rule_does_not_discard_the_whole_stylesheet_chunk() {
        let styled = styled_paragraph("p { color: green; } ::part(foo):lang(en) { color: red; }");
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn conditional_rules_are_evaluated_once_during_preparation() {
        let styled = styled_paragraph(
            "p { color: black } @media all { p { color: red } } @media print { p { color: blue } } @supports (width: 10px) { p { color: green } } @supports (opacity: 0.5) { p { color: blue } } @supports (background: linear-gradient(red, blue)) { p { color: blue } }",
        );
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn supports_uses_the_renderer_font_kerning_grammar() {
        assert_eq!(declaration_support("font-kerning", "auto"), DeclarationSupport { syntax: PropertySyntax::Valid, capability: PropertyCapability::Supported });
        assert_eq!(declaration_support("font-kerning", "sideways"), DeclarationSupport { syntax: PropertySyntax::Invalid, capability: PropertyCapability::Supported });

        let styled = styled_paragraph("p { color: red } @supports (font-kerning: auto) { p { color: green } } @supports (font-kerning: sideways) { p { color: blue } }");
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn width_media_queries_use_the_renderer_environment() {
        let css = "p { color: black } @media (min-width: 500px) { p { color: green } }";
        let narrow_document = standards_document("<html><body><p>text</p></body></html>");
        let wide_document = standards_document("<html><body><p>text</p></body></html>");

        let (narrow, _) = style_document_with_environment(narrow_document, &[css], MediaEnvironment::screen(400.0, Some(800.0)).unwrap());
        let (wide, _) = style_document_with_environment(wide_document, &[css], MediaEnvironment::screen(600.0, Some(800.0)).unwrap());

        assert_eq!(paragraph_color(&narrow), 0x000000FF);
        assert_eq!(paragraph_color(&wide), 0x008000FF);
    }

    #[test]
    fn viewport_font_units_use_the_effective_column_dimensions() {
        let document = standards_document("<html><body><p>text</p></body></html>");
        let environment = MediaEnvironment::screen(800.0, Some(600.0)).unwrap();
        let css = "html { font-size: 10vw } p { font-size: 10vh; width: calc(25vw + 5px); height: 10vmin; margin-left: 10vmax; line-height: 5vh }";
        let (styled, dependencies) = style_document_with_environment(document, &[css], environment);
        let root = styled.document().dom_root().expect("document element");
        let paragraph = paragraph_node(&styled);
        let paragraph_indices = styled.style_for_node(paragraph).expect("paragraph style");
        let box_model = styled.box_model_style(paragraph_indices).expect("validated box style");
        let text = styled.text_style(paragraph_indices).expect("validated text style");

        assert_eq!(styled.font_style(styled.style_for_node(root).expect("root style")).expect("root font").font_size, 80.0);
        assert_eq!(styled.font_style(paragraph_indices).expect("paragraph font").font_size, 60.0);
        assert_eq!(box_model.width, PreferredSize::Calc { absolute_px: 205.0, percentage: 0.0, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: 0.0, percentage_dependent: false });
        assert_eq!(box_model.height, PreferredSize::Px(60.0));
        assert_eq!(box_model.margin_left, LengthPct::Px(80.0));
        assert_eq!(text.line_height, 30.0);
        assert!(dependencies.uses_viewport_units());
    }

    #[test]
    fn height_dependent_viewport_units_do_not_guess_when_height_is_unavailable() {
        let document = standards_document("<html><body><p>text</p></body></html>");
        let environment = MediaEnvironment::screen(800.0, None).unwrap();
        let css = "p { font-size: 20px; font-size: 10vh; width: 123px; width: 10vmin; margin-left: 10vw }";
        let (styled, dependencies) = style_document_with_environment(document, &[css], environment);
        let indices = styled.style_for_node(paragraph_node(&styled)).expect("paragraph style");
        let box_model = styled.box_model_style(indices).expect("validated box style");

        assert_eq!(styled.font_style(indices).expect("paragraph font").font_size, 20.0);
        assert_eq!(box_model.width, PreferredSize::Px(123.0));
        assert_eq!(box_model.margin_left, LengthPct::Px(80.0));
        assert!(dependencies.uses_viewport_units());
    }

    #[test]
    fn lh_uses_parent_line_height_for_font_prerequisites_and_own_line_height_elsewhere() {
        let document = standards_document("<html><body><main><aside>font</aside><section>line</section><article>ordinary</article></main></body></html>");
        let css =
            "main { font-size: 50px; line-height: 1 } aside { font-size: 2lh; line-height: 42px; height: 1em } section { font-size: 42px; line-height: 2lh; width: 1lh } article { width: calc(1lh + 5px); line-height: 2; font-size: 25px }";
        let styled = style_document(document, &[css]);
        let element_style = |tag: &str| {
            let node = styled.document().nodes().find_map(|(id, node)| matches!(node, html_dom::NodeRef::Element(element) if element.tag() == tag).then_some(id)).expect("test element");
            styled.style_for_node(node).expect("element style")
        };

        let aside = element_style("aside");
        assert_eq!(styled.font_style(aside).expect("aside font").font_size, 100.0);
        assert_eq!(styled.box_model_style(aside).expect("aside box").height, PreferredSize::Px(100.0));

        let section = element_style("section");
        assert_eq!(styled.text_style(section).expect("section text").line_height, 100.0);
        assert_eq!(styled.box_model_style(section).expect("section box").width, PreferredSize::Px(100.0));

        let article = element_style("article");
        assert_eq!(styled.font_style(article).expect("article font").font_size, 25.0);
        assert_eq!(styled.text_style(article).expect("article text").line_height, 50.0);
        assert_eq!(styled.box_model_style(article).expect("article box").width, PreferredSize::Calc { absolute_px: 55.0, percentage: 0.0, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: 0.0, percentage_dependent: false });
    }

    #[test]
    fn lh_uses_the_renderers_normal_line_height() {
        let styled = styled_paragraph("p { font-size: 20px; height: 1lh }");
        let indices = styled.style_for_node(paragraph_node(&styled)).expect("paragraph style");
        assert_eq!(styled.box_model_style(indices).expect("paragraph box").height, PreferredSize::Px(24.0));
    }

    #[test]
    fn rlh_uses_the_final_root_line_height_in_direct_and_calculated_lengths() {
        let document = standards_document("<html><body><p>text</p></body></html>");
        let styled = style_document(document, &["html { line-height: 50px; width: 1rlh } p { line-height: 10px; width: calc(2rlh - 50px) }"]);
        let root = styled.document().dom_root().expect("document element");
        let root_indices = styled.style_for_node(root).expect("root style");
        let paragraph_indices = styled.style_for_node(paragraph_node(&styled)).expect("paragraph style");

        assert_eq!(styled.box_model_style(root_indices).expect("root box").width, PreferredSize::Px(50.0));
        assert_eq!(styled.box_model_style(paragraph_indices).expect("paragraph box").width, PreferredSize::Calc { absolute_px: 50.0, percentage: 0.0, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: 0.0, percentage_dependent: false });
    }

    #[test]
    fn cap_lengths_resolve_from_the_selected_fonts_cap_height() {
        let styled = styled_paragraph("p { font-size: 50px; width: 2.5cap; height: calc(180px - 2cap) }");
        let indices = styled.style_for_node(paragraph_node(&styled)).expect("paragraph style");
        let box_model = styled.box_model_style(indices).expect("paragraph box");

        assert_eq!(box_model.width, PreferredSize::Cap(125.0));
        assert_eq!(box_model.height, PreferredSize::Calc { absolute_px: 180.0, percentage: 0.0, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: -100.0, percentage_dependent: false });
        assert!(styled.styles().view(indices).expect("style view").requires_font_metrics());

        let used = styled.styles().used_view(indices, 0.5, 0.5, 0.8).expect("selected font metrics");
        assert_eq!(used.width(), html_style_model::UsedPreferredSize::Px(100.0));
        assert_eq!(used.height(), html_style_model::UsedPreferredSize::Calc { absolute_px: 100.0, percentage: 0.0, percentage_dependent: false });
    }

    #[test]
    fn conditional_layer_order_uses_the_renderer_environment() {
        let css = "@media (min-width: 500px) { @layer second, first; } @layer first { p { color: red } } @layer second { p { color: green } }";
        let narrow_document = standards_document("<html><body><p>text</p></body></html>");
        let wide_document = standards_document("<html><body><p>text</p></body></html>");

        let (narrow, _) = style_document_with_environment(narrow_document, &[css], MediaEnvironment::screen(400.0, Some(800.0)).unwrap());
        let (wide, _) = style_document_with_environment(wide_document, &[css], MediaEnvironment::screen(600.0, Some(800.0)).unwrap());

        assert_eq!(paragraph_color(&narrow), 0x008000FF);
        assert_eq!(paragraph_color(&wide), 0xFF0000FF);
    }

    #[test]
    fn normal_layer_order_and_unlayered_precedence_are_honored() {
        let styled = styled_paragraph("@layer first, second; @layer second { p { color: red } } @layer first { p { color: blue } } p { color: green }");
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn important_layer_order_is_reversed() {
        let styled = styled_paragraph("@layer first, second; @layer second { p { color: red !important } } @layer first { p { color: green !important } } p { color: blue !important }");
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn declarations_directly_in_a_layer_outrank_its_nested_layers() {
        let styled = styled_paragraph("@layer outer { p { color: green } @layer inner { p { color: red } } }");
        assert_eq!(paragraph_color(&styled), 0x008000FF);
    }

    #[test]
    fn css_text_four_white_space_reaches_the_computed_style() {
        let styled = styled_paragraph("p { white-space: preserve-breaks nowrap; }");
        assert_eq!(paragraph_white_space(&styled), WhiteSpace::PreserveBreaksNoWrap);
    }

    #[test]
    fn later_white_space_declaration_resets_the_compatibility_state() {
        let styled = styled_paragraph("p { white-space: preserve-breaks nowrap; white-space: normal; }");
        assert_eq!(paragraph_white_space(&styled), WhiteSpace::Normal);
    }

    #[test]
    fn important_white_space_declaration_wins_through_the_compatibility_layer() {
        let styled = styled_paragraph("p { white-space: preserve-breaks nowrap !important; white-space: normal; }");
        assert_eq!(paragraph_white_space(&styled), WhiteSpace::PreserveBreaksNoWrap);
    }

    #[test]
    fn inline_css_text_four_white_space_reaches_the_computed_style() {
        let document = standards_document("<html><body><p style='white-space: break-spaces nowrap'>text</p></body></html>");
        let styled = style_document(document, &[]);
        assert_eq!(paragraph_white_space(&styled), WhiteSpace::BreakSpacesNoWrap);
    }

    #[test]
    fn percentage_text_spacing_reaches_layout_as_font_relative_length() {
        let styled = styled_paragraph("p { font-size: 20px; letter-spacing: 120%; word-spacing: calc(50% + 2px); }");
        let paragraph = paragraph_node(&styled);
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        let text = styled.text_style(indices).expect("validated text style");
        let view = styled.style_view(indices).expect("validated style view");

        assert_eq!(text.letter_spacing.absolute_px(), 0.0);
        assert_eq!(text.letter_spacing.font_size_fraction(), 1.2);
        assert!((view.letter_spacing() - 24.0).abs() < 0.001);
        assert_eq!(text.word_spacing.absolute_px(), 2.0);
        assert_eq!(text.word_spacing.font_size_fraction(), 0.5);
        assert!((view.word_spacing() - 12.0).abs() < 0.001);
    }

    #[test]
    fn inherited_spacing_keeps_percent_relative_but_freezes_em_as_pixels() {
        let document = standards_document("<html><body><p><span>text</span></p></body></html>");
        let styled = style_document(document, &["p { font-size: 20px; letter-spacing: calc(2em + 50%); } span { font-size: 10px; }"]);
        let span = styled.document().nodes().find_map(|(id, node)| matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "span").then_some(id)).expect("span node");
        let indices = styled.style_for_node(span).expect("span style");
        let text = styled.text_style(indices).expect("validated text style");
        let view = styled.style_view(indices).expect("validated style view");

        assert_eq!(text.letter_spacing.absolute_px(), 40.0);
        assert_eq!(text.letter_spacing.font_size_fraction(), 0.5);
        assert!((view.letter_spacing() - 45.0).abs() < 0.001);
    }

    #[test]
    fn native_spacing_after_percentage_clears_internal_cascade_state() {
        let styled = styled_paragraph("p { font-size: 20px; letter-spacing: 120%; letter-spacing: 2em; }");
        let paragraph = paragraph_node(&styled);
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        let text = styled.text_style(indices).expect("validated text style");

        assert_eq!(text.letter_spacing.absolute_px(), 40.0);
        assert_eq!(text.letter_spacing.font_size_fraction(), 0.0);
    }

    #[test]
    fn metric_dependent_ch_tab_size_does_not_guess_a_computed_length() {
        let document = standards_document("<html><body><p><span>text</span></p></body></html>");
        let styled = style_document(document, &["p { tab-size: 2.5ch; } span { font-size: 40px; }"]);
        let span = styled.document().nodes().find_map(|(id, node)| matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "span").then_some(id)).expect("span node");
        let indices = styled.style_for_node(span).expect("span style");
        let tab_size = styled.text_style(indices).expect("validated text style").tab_size;

        assert_eq!(tab_size.kind(), TabSizeKind::Spaces);
        assert_eq!(tab_size.value(), 8.0, "unsupported ch lengths must preserve the inherited initial value instead of using a guessed 0.5em width");
    }

    #[test]
    fn invalid_tab_size_cannot_override_a_prior_valid_declaration() {
        let styled = styled_paragraph("p { tab-size: 3; tab-size: -2; }");
        let indices = styled.style_for_node(paragraph_node(&styled)).expect("paragraph style");
        let tab_size = styled.text_style(indices).expect("validated text style").tab_size;
        assert_eq!(tab_size.kind(), TabSizeKind::Spaces);
        assert_eq!(tab_size.value(), 3.0);
    }

    #[test]
    fn computed_negative_calc_tab_length_clamps_to_zero() {
        let styled = styled_paragraph("p { font-size: 20px; tab-size: calc(5px - 0.5em); }");
        let indices = styled.style_for_node(paragraph_node(&styled)).expect("paragraph style");
        let tab_size = styled.text_style(indices).expect("validated text style").tab_size;
        assert_eq!(tab_size.kind(), TabSizeKind::LengthPx);
        assert_eq!(tab_size.value(), 0.0);
    }

    #[test]
    fn computed_number_calc_keeps_number_semantics() {
        let styled = styled_paragraph("p { tab-size: calc(2 + 3); }");
        let indices = styled.style_for_node(paragraph_node(&styled)).expect("paragraph style");
        let tab_size = styled.text_style(indices).expect("validated text style").tab_size;
        assert_eq!(tab_size.kind(), TabSizeKind::Spaces);
        assert_eq!(tab_size.value(), 5.0);
    }

    #[test]
    fn tab_size_marker_preserves_important_and_initial_cascade_semantics() {
        let document = standards_document("<html><body><p><span>text</span></p></body></html>");
        let styled = style_document(document, &["p { tab-size: 2 !important; tab-size: 4; } span { tab-size: initial; }"]);
        let paragraph = paragraph_node(&styled);
        let paragraph_tab = styled.text_style(styled.style_for_node(paragraph).expect("paragraph style")).expect("paragraph text style").tab_size;
        let span = styled.document().nodes().find_map(|(id, node)| matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "span").then_some(id)).expect("span node");
        let span_tab = styled.text_style(styled.style_for_node(span).expect("span style")).expect("span text style").tab_size;

        assert_eq!(paragraph_tab.value(), 2.0);
        assert_eq!(span_tab, html_style_model::TabSize::DEFAULT);
    }

    #[test]
    fn invalid_box_declarations_cannot_override_prior_valid_values() {
        let styled = styled_paragraph("p { float: inline-start; float: left right; width: 50px; width: -10px; padding-left: 12px; padding-left: auto; }");
        let paragraph = paragraph_node(&styled);
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        let box_model = styled.box_model_style(indices).expect("validated box style");

        assert_eq!(box_model.float, Float::Left);
        assert_eq!(box_model.width, PreferredSize::Px(50.0));
        assert_eq!(box_model.padding_left, LengthPct::Px(12.0));
    }

    #[test]
    fn mixed_length_percentage_calc_survives_computed_style() {
        let styled = styled_paragraph(
            "p { font-size: 10px; width: calc(50% - 3px); min-height: calc(50% - 100px); \
             margin-left: calc(10% - 4px); padding-right: calc(5% + 1px); \
             right: calc(20% - 7px); text-indent: calc(30% + 6px); }",
        );
        let indices = styled.style_for_node(paragraph_node(&styled)).expect("paragraph style");
        let box_model = styled.box_model_style(indices).expect("validated box style");
        let layout = styled.styles().layout_style(indices).expect("validated layout style");
        let text = styled.text_style(indices).expect("validated text style");

        assert_eq!(box_model.width, PreferredSize::Calc { absolute_px: -3.0, percentage: 0.5, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: 0.0, percentage_dependent: true });
        assert_eq!(box_model.min_height, PreferredSize::Calc { absolute_px: -100.0, percentage: 0.5, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: 0.0, percentage_dependent: true });
        assert_eq!(box_model.margin_left, LengthPct::Calc { absolute_px: -4.0, percentage: 0.1, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: 0.0, percentage_dependent: true });
        assert_eq!(box_model.padding_right, LengthPct::Calc { absolute_px: 1.0, percentage: 0.05, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: 0.0, percentage_dependent: true });
        assert_eq!(layout.inset_right, Some(LengthPct::Calc { absolute_px: -7.0, percentage: 0.2, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: 0.0, percentage_dependent: true }));
        assert_eq!(text.text_indent, LengthPct::Calc { absolute_px: 6.0, percentage: 0.3, x_height_px: 0.0, ch_advance_px: 0.0, cap_height_px: 0.0, percentage_dependent: true });
    }

    #[test]
    fn mixed_length_percentage_comparisons_resolve_against_the_containing_block() {
        let styled = styled_paragraph("p { width: max(100px, 25% + 100px, 150px + 10%); height: min(300px, 25% + 100px, 50px + 50%); min-height: clamp(100px, 50%, 300px) }");
        let indices = styled.style_for_node(paragraph_node(&styled)).expect("paragraph style");
        let box_model = styled.box_model_style(indices).expect("validated box style");

        assert!(matches!(box_model.width, PreferredSize::Comparison(_)));
        assert!(matches!(box_model.height, PreferredSize::Comparison(_)));
        assert!(matches!(box_model.min_height, PreferredSize::Comparison(_)));

        let used = styled.styles().used_view(indices, 0.5, 0.5, 0.8).expect("font metrics resolve");
        assert!(matches!(used.width(), html_style_model::UsedPreferredSize::Comparison { kind: SizeComparison::Max, count: 3, .. }));
        assert!(matches!(used.height(), html_style_model::UsedPreferredSize::Comparison { kind: SizeComparison::Min, count: 3, .. }));
        assert!(matches!(used.min_height(), html_style_model::UsedPreferredSize::Comparison { kind: SizeComparison::Clamp, count: 3, .. }));
        assert_eq!(resolve_used_preferred_size(used.width(), 0.0, 400.0), 200.0);
        assert_eq!(resolve_used_preferred_size(used.height(), 0.0, 400.0), 200.0);
        assert_eq!(resolve_used_preferred_size(used.min_height(), 0.0, 400.0), 200.0);
    }

    #[test]
    fn finished_styles_are_dense_and_drop_construction_indexes() {
        let document = standards_document("<html><body><p class='same'>one</p><p class='same'>two</p></body></html>");
        let styled = style_document(document, &[".same { color: red; margin-left: 1em; }"]);

        assert_eq!(styled.styles().node_count(), styled.document().node_count());
        let report = styled.styles().memory_usage_report();
        for entry in report.entries.iter().filter(|entry| entry.label.contains("_dedup.storage")) {
            assert_eq!(entry.count, 0, "{} retained construction entries", entry.label);
            assert_eq!(entry.bytes, 0, "{} retained construction capacity", entry.label);
        }
    }

    #[test]
    fn css_strings_belong_to_style_output_not_the_parsed_dom() {
        let document = standards_document("<html><body><p>text</p></body></html>");
        assert!(document.lookup_string("Boundary Family").is_none());

        let styled = style_document(document, &["p { font-family: 'Boundary Family'; }"]);
        let paragraph = styled.document().nodes().find_map(|(id, node)| matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "p").then_some(id)).expect("paragraph node");
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        let family = styled.font_style(indices).expect("validated style handle").font_family.expect("authored family");

        assert!(styled.document().lookup_string("Boundary Family").is_none());
        assert!(styled.styles().string(family).expect("style-owned family").contains("Boundary Family"));
    }

    fn styled_paragraph(css: &str) -> super::StyledDocument {
        let document = standards_document("<html><body><p>text</p></body></html>");
        style_document(document, &[css])
    }

    fn paragraph_color(styled: &super::StyledDocument) -> u32 {
        let paragraph = paragraph_node(styled);
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        styled.text_style(indices).expect("validated text style").color
    }

    fn paragraph_white_space(styled: &super::StyledDocument) -> WhiteSpace {
        let paragraph = paragraph_node(styled);
        let indices = styled.style_for_node(paragraph).expect("paragraph style");
        styled.text_style(indices).expect("validated text style").white_space
    }

    fn paragraph_node(styled: &super::StyledDocument) -> html_dom::DomNodeId {
        styled.document().nodes().find_map(|(id, node)| matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "p").then_some(id)).expect("paragraph node")
    }
}
