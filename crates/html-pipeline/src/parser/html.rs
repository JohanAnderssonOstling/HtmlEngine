use encoding_rs::{Encoding, UTF_8};
use html_dom::{Document, DomNodeId, ImageSource, NodeRef, RootFontSize};
use crate::engine::{FormattingDocument, PreparedDocument};
use html_style_model::ComputedStyles;
#[cfg(feature = "profiling")]
use crate::engine::profiling::scope;
use html_parse::{ParsedHtml, StylesheetReference, style_type_is_css};
use html_resources::{ResourceMetadata, ResourceProvider};
use html_style::{FontFace, StylesheetEntry, resolve_font_face_urls};
use std::collections::{HashMap, HashSet};
use std::mem::take;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use html_parse::parse_html_document;

/// Optional wall-clock telemetry for document preparation. Style substage
/// timings are inclusive; they must not be added to `style` again.
#[derive(Clone, Debug, Default)]
pub struct PreparationTimings {
    pub dom: Duration,
    pub stylesheets: Duration,
    pub style: Duration,
    /// Total preparation, including formatting and logical text.
    pub prepare: Duration,
    /// Formatting structure construction before normalization.
    pub formatting: Duration,
    /// Logical text preparation and immutable paint bindings.
    pub text: Duration,
    pub style_details: html_style::StyleTimings,
}

pub struct DocumentFactory {
    root_font_size: RootFontSize,
    media_environment: html_style::MediaEnvironment,
    title: Option<String>,
    stylesheet_cache: Option<BookStylesheetCache>,
    style_program_cache: html_style::StyleProgramCache,
    reader_overrides: crate::engine::ReaderStyleOverrides,
    note_flow: crate::engine::NoteFlow,
    font_faces: Vec<FontFace>,
}

/// Borrowed resources for one preparation operation. The provider and base
/// URI remain owned by the session or host that invokes the factory.
#[derive(Clone, Copy)]
pub struct ResourceContext<'a> {
    provider: &'a dyn ResourceProvider,
    base_uri: &'a str,
}

impl<'a> ResourceContext<'a> {
    pub fn new(provider: &'a dyn ResourceProvider, base_uri: &'a str) -> Self {
        Self { provider, base_uri }
    }
}

/// A preparation view that borrows a factory and the current resource context.
/// It stores no resource ownership in the factory between operations.
pub struct ResourceDocumentFactory<'a> {
    factory: &'a mut DocumentFactory,
    resources: ResourceContext<'a>,
}

impl ResourceDocumentFactory<'_> {
    pub fn parse_to_dom(&mut self, source: &str) -> Document {
        let parsed = parse_html_document(source);
        let title = parsed.title();
        self.factory.configure_dom(parsed.into_dom(), title, Some(self.resources)).0
    }

    pub fn parse_with_new_pipeline(&mut self, source: &str, css: Option<&str>) -> PreparedDocument<'static> {
        match css {
            Some(css) => self.parse_with_new_pipeline_css_chunks(source, &[css]),
            None => self.parse_with_new_pipeline_css_chunks(source, &[]),
        }
    }

    pub fn parse_with_new_pipeline_css_chunks(&mut self, source: &str, css: &[&str]) -> PreparedDocument<'static> {
        self.build_pipeline_from_owned_parsed(parse_html_document(source), css).0
    }

    pub fn build_pipeline_from_parsed(&mut self, parsed: &ParsedHtml, css: &[&str]) -> PreparedDocument<'static> {
        self.factory.build_pipeline_from_parsed_impl(parsed, css, Some(self.resources)).0
    }

    pub fn build_pipeline_from_parsed_with_timings(&mut self, parsed: &ParsedHtml, css: &[&str]) -> (PreparedDocument<'static>, PreparationTimings) {
        let mut timings = PreparationTimings::default();
        let (prepared, _) = self.factory.build_pipeline_from_parsed_instrumented(parsed, css, Some(&mut timings), Some(self.resources));
        (prepared, timings)
    }

    pub(crate) fn build_pipeline_from_owned_parsed(&mut self, parsed: ParsedHtml, css: &[&str]) -> (PreparedDocument<'static>, html_style::MediaQuerySet) {
        self.factory.build_pipeline_from_owned_parsed(parsed, css, Some(self.resources))
    }

    pub(crate) fn resolve_canonical_dom(&mut self, document: &Document, encoding: Option<&str>, css: &[&str]) -> (ComputedStyles, html_style::MediaQuerySet) {
        self.factory.resolve_canonical_dom(document, encoding, css, Some(self.resources))
    }
}

struct ExpandedStylesheet {
    css: String,
    implicit_scope_root: Option<DomNodeId>,
}

/// Reuses decompressed external stylesheet text across document factories for
/// one immutable book. Drop it when the book closes, or call `clear` after its
/// stylesheet revision changes.
#[derive(Clone, Default)]
pub struct BookStylesheetCache {
    entries: Arc<Mutex<HashMap<(String, Option<String>, Option<String>), String>>>,
    programs: Arc<Mutex<html_style::StyleProgramCache>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BookStylesheetCacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub compiled_entries: usize,
    pub compiled_hits: u64,
    pub compiled_misses: u64,
    pub parsed_entries: usize,
    pub parsed_hits: u64,
    pub parsed_misses: u64,
    pub inline_entries: usize,
    pub inline_hits: u64,
    pub inline_misses: u64,
}

impl BookStylesheetCache {
    pub fn clear(&self) {
        self.entries.lock().expect("stylesheet cache lock").clear();
        self.programs.lock().expect("style program cache lock").clear();
    }

    pub fn stats(&self) -> BookStylesheetCacheStats {
        let entries = self.entries.lock().expect("stylesheet cache lock");
        let programs = self.programs.lock().expect("style program cache lock").stats();
        BookStylesheetCacheStats { entries: entries.len(), bytes: entries.values().map(String::len).sum(), compiled_entries: programs.entries, compiled_hits: programs.hits, compiled_misses: programs.misses, parsed_entries: programs.parsed_entries, parsed_hits: programs.parsed_hits, parsed_misses: programs.parsed_misses, inline_entries: programs.inline_entries, inline_hits: programs.inline_hits, inline_misses: programs.inline_misses }
    }

    fn read(&self, provider: &dyn ResourceProvider, uri: &str, transport_label: Option<&str>, fallback_label: Option<&str>) -> std::io::Result<String> {
        let key = (uri.to_owned(), transport_label.map(str::to_ascii_lowercase), fallback_label.map(str::to_ascii_lowercase));
        if let Some(css) = self.entries.lock().expect("stylesheet cache lock").get(&key).cloned() {
            return Ok(css);
        }
        let css = decode_stylesheet_bytes(&provider.read_bytes(uri)?, transport_label, fallback_label);
        self.entries.lock().expect("stylesheet cache lock").insert(key, css.clone());
        Ok(css)
    }
}

#[allow(dead_code)]
impl Default for DocumentFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentFactory {
    pub fn new() -> Self {
        Self { root_font_size: RootFontSize::default(), media_environment: html_style::MediaEnvironment::default(), title: None, stylesheet_cache: None, style_program_cache: html_style::StyleProgramCache::default(), reader_overrides: Default::default(), note_flow: Default::default(), font_faces: Vec::new() }
    }

    /// Active font declarations from the most recent preparation. Font bytes
    /// are left to the host; URLs are resolved through the resource provider.
    pub fn font_faces(&self) -> &[FontFace] {
        &self.font_faces
    }

    /// Transfer prepared declarations to the session that publishes them.
    pub(crate) fn take_font_faces(&mut self) -> Vec<FontFace> {
        take(&mut self.font_faces)
    }

    pub fn with_resources<'a>(&'a mut self, resources: ResourceContext<'a>) -> ResourceDocumentFactory<'a> {
        ResourceDocumentFactory { factory: self, resources }
    }

    pub fn set_book_stylesheet_cache(&mut self, cache: BookStylesheetCache) {
        self.stylesheet_cache = Some(cache);
    }

    pub fn style_program_cache_stats(&self) -> html_style::StyleProgramCacheStats {
        self.stylesheet_cache.as_ref().map_or_else(|| self.style_program_cache.stats(), |cache| cache.programs.lock().expect("style program cache lock").stats())
    }

    pub fn clear_style_program_cache(&mut self) {
        self.style_program_cache.clear();
        if let Some(cache) = &self.stylesheet_cache {
            cache.programs.lock().expect("style program cache lock").clear();
        }
    }

    pub fn set_root_font_size(&mut self, root_font_size: RootFontSize) {
        self.root_font_size = root_font_size;
    }

    pub fn set_media_environment(&mut self, media_environment: html_style::MediaEnvironment) {
        self.media_environment = media_environment;
    }

    pub fn set_note_flow(&mut self, note_flow: crate::engine::NoteFlow) {
        self.note_flow = note_flow;
    }

    pub fn set_reader_overrides(&mut self, overrides: crate::engine::ReaderStyleOverrides) {
        self.reader_overrides = overrides;
    }

    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    pub fn parse_to_dom(&mut self, html: &str) -> Document {
        let parsed = parse_html_document(html);
        let title = parsed.title();
        self.configure_dom(parsed.into_dom(), title, None).0
    }

    pub fn parse_with_new_pipeline(&mut self, html: &str, css: Option<&str>) -> PreparedDocument<'static> {
        match css {
            Some(css) => self.parse_with_new_pipeline_css_chunks(html, &[css]),
            None => self.parse_with_new_pipeline_css_chunks(html, &[]),
        }
    }

    pub fn parse_with_new_pipeline_css_chunks(&mut self, html: &str, css_chunks: &[&str]) -> PreparedDocument<'static> {
        let parsed_html = parse_html_document(html);
        self.build_pipeline_from_owned_parsed(parsed_html, css_chunks, None).0
    }

    pub fn build_pipeline_from_parsed(&mut self, parsed_html: &ParsedHtml, css_chunks: &[&str]) -> PreparedDocument<'static> {
        self.build_pipeline_from_parsed_impl(parsed_html, css_chunks, None).0
    }

    pub fn build_pipeline_from_parsed_with_timings(&mut self, parsed_html: &ParsedHtml, css_chunks: &[&str]) -> (PreparedDocument<'static>, PreparationTimings) {
        let mut timings = PreparationTimings::default();
        let (prepared, _) = self.build_pipeline_from_parsed_instrumented(parsed_html, css_chunks, Some(&mut timings), None);
        (prepared, timings)
    }

    fn build_pipeline_from_parsed_impl(&mut self, parsed_html: &ParsedHtml, css_chunks: &[&str], resources: Option<ResourceContext<'_>>) -> (PreparedDocument<'static>, html_style::MediaQuerySet) {
        self.build_pipeline_from_parsed_instrumented(parsed_html, css_chunks, None, resources)
    }

    fn build_pipeline_from_parsed_instrumented(&mut self, parsed_html: &ParsedHtml, css_chunks: &[&str], mut timings: Option<&mut PreparationTimings>, resources: Option<ResourceContext<'_>>) -> (PreparedDocument<'static>, html_style::MediaQuerySet) {
        #[cfg(feature = "profiling")]
        let dom_scope = scope("factory.dom");
        let started = timings.as_ref().map(|_| Instant::now());
        let (document, base_uri) = self.configured_dom_and_base(parsed_html, resources);
        #[cfg(feature = "profiling")]
        drop(dom_scope);
        if let Some(timings) = timings.as_deref_mut() {
            timings.dom = started.expect("timed preparation").elapsed();
        }
        self.build_pipeline_from_dom_instrumented(document, base_uri, parsed_html.source_encoding(), css_chunks, timings, resources)
    }

    pub(crate) fn build_pipeline_from_owned_parsed(&mut self, parsed: ParsedHtml, css_chunks: &[&str], resources: Option<ResourceContext<'_>>) -> (PreparedDocument<'static>, html_style::MediaQuerySet) {
        let title = parsed.title();
        let encoding = parsed.source_encoding().map(str::to_owned);
        let (document, base_uri) = self.configure_dom(parsed.into_dom(), title, resources);
        self.build_pipeline_from_dom_instrumented(document, base_uri, encoding.as_deref(), css_chunks, None, resources)
    }

    fn build_pipeline_from_dom_instrumented(&mut self, document: Document, base_uri: String, document_encoding: Option<&str>, css_chunks: &[&str], mut timings: Option<&mut PreparationTimings>, resources: Option<ResourceContext<'_>>) -> (PreparedDocument<'static>, html_style::MediaQuerySet) {
        let started = timings.as_ref().map(|_| Instant::now());
        #[cfg(feature = "profiling")]
        let stylesheets_scope = scope("factory.stylesheets");
        let expanded_css = self.expanded_document_stylesheets(&document, &base_uri, document_encoding, css_chunks, resources);
        let all_css = expanded_css.iter().map(|stylesheet| html_style::AuthorStylesheetInput { css: stylesheet.css.as_str(), implicit_scope_root: stylesheet.implicit_scope_root }).collect::<Vec<_>>();
        #[cfg(feature = "profiling")]
        drop(stylesheets_scope);
        if let Some(timings) = timings.as_deref_mut() {
            timings.stylesheets = started.expect("timed preparation").elapsed();
        }
        let started = timings.as_ref().map(|_| Instant::now());
        #[cfg(feature = "profiling")]
        let style_scope = scope("factory.style");
        let (styled, style_details, media_queries) = if let Some(cache) = &self.stylesheet_cache {
            let root_font_size = document.root_font_size();
            let (program, compilation) = cache.programs.lock().expect("style program cache lock").get_or_compile(&all_css, self.media_environment, root_font_size);
            let (styled, mut details, queries) = html_style::style_document_with_program_and_reader_overrides(document, &program, &all_css, &self.reader_overrides);
            details.parse_default_css = compilation.parse_default_css;
            details.parse_author_css = compilation.parse_author_css;
            details.prepare_rules = compilation.prepare_rules;
            details.selector_index = compilation.selector_index;
            (styled, details, queries)
        } else {
            html_style::style_document_with_cached_program_and_reader_overrides(&mut self.style_program_cache, document, &all_css, self.media_environment, &self.reader_overrides)
        };
        #[cfg(feature = "profiling")]
        drop(style_scope);
        if let Some(timings) = timings.as_deref_mut() {
            timings.style = started.expect("timed preparation").elapsed();
            timings.style_details = style_details;
        }
        let started = timings.as_ref().map(|_| Instant::now());
        let font_faces = styled.font_faces().to_vec();
        let (document, styles) = styled.into_parts();
        let language = resources.as_ref().and_then(|resources| resources.provider.primary_language());
        let styles = styles.with_hyphenation_fallback_language(language.as_deref());
        #[cfg(feature = "profiling")]
        let prepare_scope = scope("prepare.total");
        let formatting = FormattingDocument::try_new_with_note_flow(document, styles, self.note_flow)
            .expect("style resolver must produce complete styles for its document");
        if let Some(timings) = timings.as_deref_mut() {
            timings.formatting = started.expect("timed formatting construction").elapsed();
        }
        let text_started = timings.as_ref().map(|_| Instant::now());
        let prepared = formatting.prepare_text();
        if let Some(timings) = timings.as_deref_mut() {
            timings.text = text_started.expect("timed logical-text preparation").elapsed();
        }
        #[cfg(feature = "profiling")]
        drop(prepare_scope);
        self.font_faces = font_faces;
        if let Some(timings) = timings {
            timings.prepare = started.expect("timed preparation").elapsed();
        }
        (prepared, media_queries)
    }

    fn expanded_document_stylesheets(&self, document: &Document, base_uri: &str, document_encoding: Option<&str>, css_chunks: &[&str], resources: Option<ResourceContext<'_>>) -> Vec<ExpandedStylesheet> {
        let mut expanded_css = Vec::new();
        for (stylesheet, implicit_scope_root) in document_stylesheets(document) {
            match stylesheet {
                StylesheetReference::Inline(css) => self.expand_stylesheet(&css, base_uri, None, document_encoding, implicit_scope_root, &mut HashSet::new(), 0, &mut expanded_css, resources),
                StylesheetReference::External(href) => {
                    if let Some(resources) = resources.as_ref() {
                        let uri = resources.provider.resolve(base_uri, &href);
                        if let Ok(css) = self.read_stylesheet(resources.provider, &uri, document_encoding) {
                            self.expand_stylesheet(&css, &uri, Some(&uri), document_encoding, implicit_scope_root, &mut HashSet::new(), 0, &mut expanded_css, Some(*resources));
                        }
                    }
                }
                StylesheetReference::ExternalWithCharset { href, charset } => {
                    if let Some(resources) = resources.as_ref() {
                        let uri = resources.provider.resolve(base_uri, &href);
                        if let Ok(css) = self.read_stylesheet(resources.provider, &uri, Some(&charset)) {
                            self.expand_stylesheet(&css, &uri, Some(&uri), Some(&charset), implicit_scope_root, &mut HashSet::new(), 0, &mut expanded_css, Some(*resources));
                        }
                    }
                }
            }
        }
        // Reader styles participate in the author cascade, but are ordered
        // after every publication sheet so equal-specificity declarations win
        // as promised by PipelineInputs::user_styles.
        for css in css_chunks {
            self.expand_stylesheet(css, base_uri, None, document_encoding, document.dom_root(), &mut HashSet::new(), 0, &mut expanded_css, resources);
        }
        expanded_css
    }

    pub(crate) fn resolve_canonical_dom(&mut self, document: &Document, document_encoding: Option<&str>, css_chunks: &[&str], resources: Option<ResourceContext<'_>>) -> (ComputedStyles, html_style::MediaQuerySet) {
        let base_uri = resources.as_ref().map_or_else(String::new, |resources| effective_document_base_uri(document, resources));
        let expanded = self.expanded_document_stylesheets(document, &base_uri, document_encoding, css_chunks, resources);
        let inputs: Vec<_> = expanded.iter().map(|sheet| html_style::AuthorStylesheetInput { css: &sheet.css, implicit_scope_root: sheet.implicit_scope_root }).collect();
        let (program, timings) = if let Some(cache) = &self.stylesheet_cache {
            cache.programs.lock().expect("stylesheet cache lock").get_or_compile(&inputs, self.media_environment, document.root_font_size())
        } else {
            self.style_program_cache.get_or_compile(&inputs, self.media_environment, document.root_font_size())
        };
        let (styles, _, queries) = html_style::resolve_document_with_program(document, &program, &inputs, timings, &self.reader_overrides);
        self.font_faces = program.font_faces().to_vec();
        let language = resources.as_ref().and_then(|resources| resources.provider.primary_language());
        (styles.with_hyphenation_fallback_language(language.as_deref()), queries)
    }

    fn configured_dom_and_base(&self, parsed: &ParsedHtml, resources: Option<ResourceContext<'_>>) -> (Document, String) {
        self.configure_dom(parsed.build_dom(), parsed.title(), resources)
    }

    fn configure_dom(&self, mut document: Document, title: Option<String>, resources: Option<ResourceContext<'_>>) -> (Document, String) {
        document.set_root_font_size(self.root_font_size);
        document.set_title(self.title.clone().or(title));
        let base_uri = resources.as_ref().map_or_else(String::new, |resources| effective_document_base_uri(&document, resources));
        if let Some(resources) = resources.as_ref() {
            for image in document.images_mut() {
                match &mut image.source {
                    ImageSource::Uri(uri) => *uri = resources.provider.resolve(&base_uri, uri),
                    ImageSource::InlineSvg { base_uri: svg_base_uri, .. } => svg_base_uri.clone_from(&base_uri),
                    ImageSource::MathMl { .. } => {}
                }
            }
        }
        (document, base_uri)
    }

    fn expand_stylesheet(
        &self, css: &str, base_uri: &str, source_uri: Option<&str>, fallback_label: Option<&str>, implicit_scope_root: Option<DomNodeId>, active_imports: &mut HashSet<String>, depth: usize, output: &mut Vec<ExpandedStylesheet>, resources: Option<ResourceContext<'_>>,
    ) {
        const MAX_IMPORT_DEPTH: usize = 32;
        if depth > MAX_IMPORT_DEPTH {
            return;
        }
        if let Some(uri) = source_uri
            && !active_imports.insert(uri.to_owned())
        {
            return;
        }

        for entry in html_style::stylesheet_entries(css) {
            match entry {
                StylesheetEntry::Css(css) if !css.trim().is_empty() => {
                    // Resolve before imports are flattened: each imported sheet
                    // owns its font URL base, independent of the document base.
                    let css = if let Some(resources) = &resources {
                        resolve_font_face_urls(&css, |url| resources.provider.resolve(base_uri, url)).into_owned()
                    } else {
                        css
                    };
                    output.push(ExpandedStylesheet { css, implicit_scope_root });
                }
                StylesheetEntry::Css(_) => {}
                StylesheetEntry::Import(import) => {
                    let Some(resources) = resources.as_ref() else { continue };
                    let uri = resources.provider.resolve(base_uri, &import.url);
                    if active_imports.contains(&uri) {
                        continue;
                    }
                    let Ok(imported_css) = self.read_stylesheet(resources.provider, &uri, fallback_label) else { continue };
                    let mut imported_output = Vec::new();
                    self.expand_stylesheet(&imported_css, &uri, Some(&uri), fallback_label, implicit_scope_root, active_imports, depth + 1, &mut imported_output, Some(*resources));
                    if !imported_output.is_empty() {
                        let imported_css = imported_output.iter().map(|stylesheet| stylesheet.css.as_str()).collect::<Vec<_>>().join(" ");
                        output.push(ExpandedStylesheet { css: import.wrap_resolved_css(&imported_css), implicit_scope_root });
                    }
                }
            }
        }

        if let Some(uri) = source_uri {
            active_imports.remove(uri);
        }
    }

    fn read_stylesheet(&self, provider: &dyn ResourceProvider, uri: &str, fallback_label: Option<&str>) -> std::io::Result<String> {
        let ResourceMetadata { media_type, charset } = provider.metadata(uri)?;
        if media_type.as_deref().is_some_and(|media_type| !media_type.split(';').next().unwrap_or_default().trim().eq_ignore_ascii_case("text/css")) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "external stylesheet resource is not text/css"));
        }
        match &self.stylesheet_cache {
            Some(cache) => cache.read(provider, uri, charset.as_deref(), fallback_label),
            None => provider.read_bytes(uri).map(|bytes| decode_stylesheet_bytes(&bytes, charset.as_deref(), fallback_label)),
        }
    }
}

fn document_stylesheets(document: &Document) -> Vec<(StylesheetReference, Option<DomNodeId>)> {
    const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";

    fn text_content(document: &Document, node: DomNodeId, output: &mut String) {
        match document.node_ref(node) {
            Some(NodeRef::Text(text)) => output.push_str(text.text()),
            Some(NodeRef::Element(element)) => element.children().for_each(|child| text_content(document, child, output)),
            None => {}
        }
    }

    let mut stylesheets = Vec::new();
    for node in document.node_ids() {
        let Some(element) = document.element_ref(node) else { continue };
        if element.namespace() != Some(HTML_NAMESPACE) {
            continue;
        }
        let implicit_scope_root = element.parent().or(document.dom_root());
        if element.tag().eq_ignore_ascii_case("style") && style_type_is_css(element.attr("type")) {
            let mut css = String::new();
            element.children().for_each(|child| text_content(document, child, &mut css));
            if !css.trim().is_empty() {
                stylesheets.push((StylesheetReference::Inline(css), implicit_scope_root));
            }
        } else if element.tag().eq_ignore_ascii_case("link")
            && let Some(stylesheet) = StylesheetReference::from_link(
                element.attr("rel"), element.attr("type"),
                element.attr("href"), element.attr("charset"),
            )
        {
            stylesheets.push((stylesheet, implicit_scope_root));
        }
    }
    stylesheets
}

fn effective_document_base_uri(document: &Document, resources: &ResourceContext<'_>) -> String {
    const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
    let Some(href) = document
        .node_ids()
        .filter_map(|node| document.element_ref(node))
        .find(|element| element.namespace() == Some(HTML_NAMESPACE) && element.tag().eq_ignore_ascii_case("base") && element.attr("href").is_some())
        .and_then(|element| element.attr("href"))
    else {
        return resources.base_uri.to_owned();
    };
    let path = href.split(['?', '#']).next().unwrap_or(href);
    if path.is_empty() {
        return resources.base_uri.to_owned();
    }
    let mut resolved = resources.provider.resolve(resources.base_uri, href);
    if path.ends_with('/') && !resolved.ends_with('/') {
        resolved.push('/');
    }
    resolved
}

/// Decode an external CSS resource before handing Unicode text to Lightning
/// CSS. Encoding detection is necessarily byte-oriented: converting through
/// `String::from_utf8_lossy` first destroys both legacy-encoded identifiers and
/// the UTF BOM used by the CSS encoding algorithm.
fn decode_stylesheet_bytes(bytes: &[u8], transport_label: Option<&str>, fallback_label: Option<&str>) -> String {
    let (encoding, bom_len) = Encoding::for_bom(bytes)
        .map(|(encoding, length)| (encoding, length))
        .or_else(|| transport_label.and_then(|label| Encoding::for_label(label.trim().as_bytes())).map(|encoding| (encoding, 0)))
        .or_else(|| css_charset_label(bytes).and_then(Encoding::for_label).map(|encoding| (encoding, 0)))
        .or_else(|| fallback_label.and_then(|label| Encoding::for_label(label.trim().as_bytes())).map(|encoding| (encoding, 0)))
        .unwrap_or((UTF_8, 0));
    encoding.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned()
}

/// CSS `@charset` participates in encoding detection only in its exact leading
/// byte form. Other at-rules named `charset` are ordinary parser input and must
/// not affect decoding.
fn css_charset_label(bytes: &[u8]) -> Option<&[u8]> {
    const PREFIX: &[u8] = b"@charset \"";
    let rest = bytes.strip_prefix(PREFIX)?;
    let end = rest.iter().position(|&byte| byte == b'\"')?;
    (rest.get(end + 1) == Some(&b';')).then_some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::{BookStylesheetCache, DocumentFactory, ResourceContext, decode_stylesheet_bytes, parse_html_document};
    use html_dom::{Document, ElementRef, NodeRef};
    use html_resources::ResourceProvider;
    use std::collections::HashMap;
    use std::io;
    use std::sync::Arc;

    #[test]
    fn resource_context_is_borrowed_for_one_operation() {
        let mut factory = DocumentFactory::new();
        let prepared = {
            let provider = MemoryProvider { resources: HashMap::from([("book/book.css".to_owned(), b"#target {color:red}".to_vec())]) };
            let base = String::from("book/index.html");
            factory.with_resources(ResourceContext::new(&provider, &base))
                .parse_with_new_pipeline("<link rel='stylesheet' href='book.css'><p id='target'>Text</p>", None)
        };
        let target = (0..prepared.box_count()).find(|&index| prepared.get_id(index) == Some("target")).unwrap();
        assert_eq!(prepared.box_text_color(target), Some(0xff0000ff));
        let without_context = factory.parse_with_new_pipeline("<link rel='stylesheet' href='book.css'><p id='target'>Text</p>", None);
        let target = (0..without_context.box_count()).find(|&index| without_context.get_id(index) == Some("target")).unwrap();
        assert_ne!(without_context.box_text_color(target), Some(0xff0000ff));
    }

    struct MemoryProvider {
        resources: HashMap<String, Vec<u8>>,
    }

    impl ResourceProvider for MemoryProvider {
        fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
            self.resources.get(uri).cloned().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, uri.to_owned()))
        }

        fn exists(&self, uri: &str) -> bool {
            self.resources.contains_key(uri)
        }

        fn resolve(&self, base: &str, href: &str) -> String {
            let parent = base.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("");
            if parent.is_empty() { href.to_owned() } else { format!("{parent}/{href}") }
        }

        fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn title_queries_preserve_factory_overrides_with_and_without_resources() {
        let source = "<head><style>p { color: blue }</style><title> Book title </title></head><body>Text</body>";
        let parsed = parse_html_document(source);
        let provider = MemoryProvider { resources: HashMap::new() };
        let mut factory = DocumentFactory::new();
        for title in [None, Some("Reader title".to_owned())] {
            factory.set_title(title.clone());
            let expected = title.as_deref().unwrap_or("Book title");
            assert_eq!(factory.parse_to_dom(source).title(), Some(expected));
            assert_eq!(factory.configured_dom_and_base(&parsed, None).0.title(), Some(expected));
            let document = factory.with_resources(ResourceContext::new(&provider, "chapter.html"))
                .parse_to_dom(source);
            assert_eq!(document.title(), Some(expected));
        }
    }

    #[test]
    fn font_faces_resolve_against_each_declaring_stylesheet() {
        use html_style::FontFaceSource;

        let provider = Arc::new(MemoryProvider { resources: HashMap::from([
            ("OPS/book/styles/main.css".into(), br#"
                @import "nested/extra.css" layer(fonts) supports(display: block) screen;
                @font-face { font-family: Linked; src: url("main.woff2") format(woff2); }
            "#.to_vec()),
            ("OPS/book/styles/nested/extra.css".into(), br#"
                @font-face { font-family: Imported; src: local("Installed"), url("extra.ttf"); }
            "#.to_vec()),
        ]) });
        let mut factory = DocumentFactory::new();
        let factory_resources = ResourceContext::new(provider.as_ref(), ("OPS/chapter.xhtml").as_ref());
        let html = r#"<html><head>
            <base href="book/">
            <link rel="stylesheet" href="styles/main.css">
            <style>@font-face { font-family: Inline; src: url("inline.ttf"); }</style>
        </head><body><p>Hello</p></body></html>"#;
        factory.with_resources(factory_resources).parse_with_new_pipeline(html, Some("@font-face { font-family: User; src: url(user.ttf) }"));
        let faces = factory.font_faces();
        assert_eq!(faces.iter().map(|face| face.family.as_str()).collect::<Vec<_>>(), ["Imported", "Linked", "Inline", "User"]);
        let urls = faces.iter().map(|face| face.sources.iter().find_map(|source| match source {
            FontFaceSource::Url { url, .. } => Some(url.as_str()),
            FontFaceSource::Local(_) => None,
        }).expect("URL source")).collect::<Vec<_>>();
        assert_eq!(urls, ["OPS/book/styles/nested/extra.ttf", "OPS/book/styles/main.woff2", "OPS/book/inline.ttf", "OPS/book/user.ttf"]);

        // A second chapter must not retain the first chapter's declarations.
        factory.with_resources(factory_resources).parse_with_new_pipeline("<html><body>No fonts</body></html>", None);
        assert!(factory.font_faces().is_empty());
    }

    #[test]
    fn keeps_whitespace_only_text_nodes_in_inline_content() {
        let html = "<html><body><span>a</span> <span>b</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_to_dom(html);

        let body = find_body(&document);
        let children: Vec<_> = body.children().collect();

        assert_eq!(children.len(), 3, "body should keep the space node");

        let middle_text = document.text_ref(children[1]).expect("expected whitespace text node");

        assert_eq!(middle_text.text(), " ");
    }

    #[test]
    fn preparation_timings_preserve_styles_and_report_shared_cache_compilation() {
        let parsed = parse_html_document("<html><body><p>text</p></body></html>");
        let cache = BookStylesheetCache::default();
        let css = ["p { color: red; margin: 1em }"];
        let mut reference_factory = DocumentFactory::new();
        let reference = reference_factory.build_pipeline_from_parsed(&parsed, &css);
        for cached in [false, true] {
            let mut factory = DocumentFactory::new();
            factory.set_book_stylesheet_cache(cache.clone());
            let (prepared, timings) = factory.build_pipeline_from_parsed_with_timings(&parsed, &css);
            assert_eq!(prepared.box_count(), reference.box_count());
            for index in 0..prepared.box_count() {
                assert_eq!(prepared.get_tag(index), reference.get_tag(index));
                assert_eq!(prepared.box_text_color(index), reference.box_text_color(index));
            }
            assert!(!timings.dom.is_zero());
            assert!(!timings.style.is_zero());
            assert!(!timings.prepare.is_zero());
            assert!(!timings.formatting.is_zero());
            assert!(!timings.text.is_zero());
            assert!(timings.prepare >= timings.formatting + timings.text);
            assert_eq!(timings.style_details.parse_author_css.is_zero(), cached);
            // The reference preparation already initialized the process-wide
            // default sheet, independently of this book's program cache.
            assert!(timings.style_details.parse_default_css.is_zero());
        }
        assert_eq!(cache.stats().compiled_hits, 1);
    }

    #[test]
    fn reuses_compiled_style_programs_across_document_loads() {
        let cache = BookStylesheetCache::default();
        let css = ["p { color: red; margin: 1em }"];
        for body in ["one", "two"] {
            let mut factory = DocumentFactory::new();
            factory.set_book_stylesheet_cache(cache.clone());
            let _document = factory.parse_with_new_pipeline_css_chunks(&format!("<html><body><p>{body}</p></body></html>"), &css);
        }
        let stats = cache.stats();
        assert_eq!((stats.compiled_entries, stats.compiled_hits, stats.compiled_misses), (1, 1, 1));
    }

    #[test]
    fn reuses_individual_parsed_stylesheets_across_spine_items() {
        let cache = BookStylesheetCache::default();
        let shared = "p { color: red; margin: 1em }";
        for chapter_css in [".chapter-one { display: block }", ".chapter-two { display: block }"] {
            let mut factory = DocumentFactory::new();
            factory.set_book_stylesheet_cache(cache.clone());
            let _document = factory.parse_with_new_pipeline_css_chunks("<html><body><p>text</p></body></html>", &[shared, chapter_css]);
        }
        let stats = cache.stats();
        assert_eq!((stats.compiled_hits, stats.compiled_misses), (0, 2));
        assert_eq!((stats.parsed_entries, stats.parsed_hits, stats.parsed_misses), (3, 1, 3));
    }

    #[test]
    fn reuses_inline_styles_across_spine_items() {
        let cache = BookStylesheetCache::default();
        for chapter_css in [".chapter-one { display: block }", ".chapter-two { display: block }"] {
            let mut factory = DocumentFactory::new();
            factory.set_book_stylesheet_cache(cache.clone());
            let _document = factory.parse_with_new_pipeline("<html><body><p style='color: red; margin: 1em'>text</p></body></html>", Some(chapter_css));
        }
        let stats = cache.stats();
        assert_eq!((stats.compiled_hits, stats.compiled_misses), (0, 2));
        assert_eq!((stats.inline_entries, stats.inline_hits, stats.inline_misses), (1, 1, 1));
    }

    #[test]
    fn follows_html_anchor_scope_for_ignored_self_closing_flags() {
        let html = "<html><body><p><a id=\"p77\"/>After</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_to_dom(html);

        let body = find_body(&document);
        let p = child_element_by_tag(&document, body, "p");
        let children: Vec<_> = p.children().collect();

        assert_eq!(children.len(), 1);
        let anchor = document.element_ref(children[0]).expect("paragraph child should be an anchor");
        assert_eq!(anchor.tag(), "a");
        assert!(matches!(anchor.children().next().and_then(|node| document.node_ref(node)), Some(NodeRef::Text(text)) if text.text() == "After"));
    }

    #[test]
    fn resolves_linked_and_inline_stylesheets_in_document_order() {
        let provider = Arc::new(MemoryProvider { resources: HashMap::from([("book/first.css".to_owned(), b"p { color: red; }".to_vec()), ("book/last.css".to_owned(), b"p { color: blue; }".to_vec())]) });
        let html = r#"
            <html><head>
                <link rel="stylesheet" href="first.css">
                <style>p { color: green; }</style>
                <link rel="preload StyleSheet" type="Text/CSS; charset=utf-8" href=" last.css " charset=" ">
                <link rel="alternate STYLESHEET" href="first.css">
                <link rel="stylesheet" type="text/plain" href="first.css">
                <link rel="preload" href="first.css">
            </head><body><p>text</p></body></html>
        "#;
        let mut factory = DocumentFactory::new();
        let factory_resources = ResourceContext::new(provider.as_ref(), ("book/chapter.xhtml").as_ref());

        let prepared = factory.with_resources(factory_resources).parse_with_new_pipeline(html, None);
        let paragraph = (0..prepared.box_count()).find(|&box_idx| prepared.get_tag(box_idx) == "p").expect("paragraph box");
        assert_eq!(prepared.box_text_color(paragraph), Some(0x0000FFFF));
    }

    #[test]
    fn first_html_base_resolves_stylesheets_and_images_once() {
        let provider = Arc::new(MemoryProvider { resources: HashMap::from([("book/resources/style.css".to_owned(), b"p { color: green; }".to_vec()), ("book/resources/cat.png".to_owned(), Vec::new())]) });
        let html = r#"
            <html><head>
                <base href="resources/">
                <base href="wrong/">
                <link rel="stylesheet" href="style.css">
            </head><body><p>text</p><img src="cat.png"><svg xmlns="http://www.w3.org/2000/svg"><image href="cover.png"/></svg></body></html>
        "#;
        let mut factory = DocumentFactory::new();
        let factory_resources = ResourceContext::new(provider.as_ref(), ("book/chapter.html").as_ref());

        let document = factory.with_resources(factory_resources).parse_to_dom(html);
        assert!(matches!(&document.images()[0].source, html_dom::ImageSource::Uri(uri) if uri == "book/resources/cat.png"));
        assert!(matches!(
            &document.images()[1].source,
            html_dom::ImageSource::InlineSvg { base_uri, .. }
                if base_uri == "book/resources/"
        ));

        let prepared = factory.with_resources(factory_resources).parse_with_new_pipeline(html, None);
        let paragraph = (0..prepared.box_count()).find(|&box_idx| prepared.get_tag(box_idx) == "p").expect("paragraph box");
        assert_eq!(prepared.box_text_color(paragraph), Some(0x008000FF));
    }

    #[test]
    fn html_style_type_is_ascii_case_insensitive() {
        let html = r#"
            <html><head>
                <style>p { color: red; }</style>
                <style type="TeXt/CsS">p { color: green; }</style>
                <style type="text/cſs">p { color: red; }</style>
            </head><body><p>text</p></body></html>
        "#;
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(html, None);
        let paragraph = (0..prepared.box_count()).find(|&box_idx| prepared.get_tag(box_idx) == "p").expect("paragraph box");
        assert_eq!(prepared.box_text_color(paragraph), Some(0x008000FF));
    }

    #[test]
    fn omitted_scope_prelude_uses_the_stylesheet_elements_parent() {
        let html = r#"
            <html><body>
                <div><style>@scope { .target { color: green; } }</style><p id="inside" class="target">inside</p></div>
                <p id="outside" class="target">outside</p>
            </body></html>
        "#;
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(html, None);
        let box_by_id = |id| (0..prepared.box_count()).find(|&box_idx| prepared.get_id(box_idx) == Some(id)).expect("element box");

        assert_eq!(prepared.box_text_color(box_by_id("inside")), Some(0x008000FF));
        assert_eq!(prepared.box_text_color(box_by_id("outside")), Some(0x000000FF));
    }

    #[test]
    fn reader_styles_follow_all_publication_stylesheets() {
        let provider = Arc::new(MemoryProvider { resources: HashMap::from([("book/publication.css".to_owned(), b"p { color: blue; }".to_vec())]) });
        let html = r#"
            <html><head>
                <style>p { color: red; }</style>
                <link rel="stylesheet" href="publication.css">
            </head><body><p>text</p></body></html>
        "#;
        let mut factory = DocumentFactory::new();
        let factory_resources = ResourceContext::new(provider.as_ref(), ("book/chapter.xhtml").as_ref());

        let prepared = factory.with_resources(factory_resources).parse_with_new_pipeline(html, Some("p { color: green; }"));
        let paragraph = (0..prepared.box_count()).find(|&box_idx| prepared.get_tag(box_idx) == "p").expect("paragraph box");
        assert_eq!(prepared.box_text_color(paragraph), Some(0x008000FF));
    }

    #[test]
    fn resolves_nested_imports_relative_to_the_importing_stylesheet_and_breaks_cycles() {
        let provider = Arc::new(MemoryProvider { resources: HashMap::from([("book/a.css".to_owned(), b"@import 'b.css'; p { color: red; }".to_vec()), ("book/b.css".to_owned(), b"@import 'a.css'; p { font-weight: 700; }".to_vec())]) });
        let html = "<html><head><style>@import 'a.css';</style></head><body><p>text</p></body></html>";
        let mut factory = DocumentFactory::new();
        let factory_resources = ResourceContext::new(provider.as_ref(), ("book/chapter.xhtml").as_ref());

        let prepared = factory.with_resources(factory_resources).parse_with_new_pipeline(html, None);
        let paragraph = (0..prepared.box_count()).find(|&box_idx| prepared.get_tag(box_idx) == "p").expect("paragraph box");
        assert_eq!(prepared.box_text_color(paragraph), Some(0xFF0000FF));
    }

    #[test]
    fn import_conditions_and_layers_survive_resource_expansion() {
        let provider = Arc::new(MemoryProvider { resources: HashMap::from([("book/theme.css".to_owned(), b"p { color: red !important; }".to_vec())]) });
        let html = "<html><head><style>@import 'theme.css' layer(theme) supports(width: 1px) screen; p { color: green !important; }</style></head><body><p>text</p></body></html>";
        let mut factory = DocumentFactory::new();
        let factory_resources = ResourceContext::new(provider.as_ref(), ("book/chapter.xhtml").as_ref());

        let prepared = factory.with_resources(factory_resources).parse_with_new_pipeline(html, None);
        let paragraph = (0..prepared.box_count()).find(|&box_idx| prepared.get_tag(box_idx) == "p").expect("paragraph box");
        assert_eq!(prepared.box_text_color(paragraph), Some(0xFF0000FF), "important layered imports outrank important unlayered author rules");
    }

    #[test]
    fn external_stylesheet_charset_decodes_legacy_identifiers() {
        let bytes = b"@charset \"windows-1252\"; .t\xe9st { color: green; }";
        assert_eq!(decode_stylesheet_bytes(bytes, None, None), "@charset \"windows-1252\"; .t\u{e9}st { color: green; }");
    }

    #[test]
    fn stylesheet_bom_takes_precedence_over_charset_rule() {
        let css = "@charset \"windows-1252\"; .\u{5e73}\u{548c} { color: green; }";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(css.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode_stylesheet_bytes(&bytes, Some("shift_jis"), None), css);
    }

    fn find_body(document: &Document) -> ElementRef<'_> {
        let html_idx = document.dom_root().expect("html root should exist");
        let html = document.element_ref(html_idx).expect("html should be element");
        child_element_by_tag(document, html, "body")
    }

    fn child_element_by_tag<'a>(document: &'a Document, parent: ElementRef<'a>, tag: &str) -> ElementRef<'a> {
        parent.children().find_map(|child_idx| document.element_ref(child_idx).filter(|element| element.tag() == tag)).expect("child element should exist")
    }
}
