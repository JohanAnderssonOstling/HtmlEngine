use encoding_rs::{Encoding, UTF_8};
use html_dom::{Document, DomNodeId, ImageSource, NodeRef, RootFontSize};
use html_layout::PreparedDocument;
use html_parse::{ParsedHtml, StylesheetReference};
use html_resources::{ResourceMetadata, ResourceProvider};
use html_style::StylesheetEntry;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use html_parse::parse_html_document;

pub struct DocumentFactory {
    root_font_size: RootFontSize,
    media_environment: html_style::MediaEnvironment,
    title: Option<String>,
    resources: Option<ResourceContext>,
    stylesheet_cache: Option<BookStylesheetCache>,
    reader_overrides: html_layout::ReaderStyleOverrides,
    note_flow: html_layout::NoteFlow,
}

struct ResourceContext {
    provider: Arc<dyn ResourceProvider>,
    base_uri: String,
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
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BookStylesheetCacheStats {
    pub entries: usize,
    pub bytes: usize,
}

impl BookStylesheetCache {
    pub fn clear(&self) {
        self.entries.lock().expect("stylesheet cache lock").clear();
    }

    pub fn stats(&self) -> BookStylesheetCacheStats {
        let entries = self.entries.lock().expect("stylesheet cache lock");
        BookStylesheetCacheStats { entries: entries.len(), bytes: entries.values().map(String::len).sum() }
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
        Self { root_font_size: RootFontSize::default(), media_environment: html_style::MediaEnvironment::default(), title: None, resources: None, stylesheet_cache: None, reader_overrides: Default::default(), note_flow: Default::default() }
    }

    pub fn set_resource_context(&mut self, provider: Arc<dyn ResourceProvider>, base_uri: impl Into<String>) {
        self.resources = Some(ResourceContext { provider, base_uri: base_uri.into() });
    }

    pub fn set_book_stylesheet_cache(&mut self, cache: BookStylesheetCache) {
        self.stylesheet_cache = Some(cache);
    }

    pub fn set_root_font_size(&mut self, root_font_size: RootFontSize) {
        self.root_font_size = root_font_size;
    }

    pub fn set_media_environment(&mut self, media_environment: html_style::MediaEnvironment) {
        self.media_environment = media_environment;
    }

    pub fn set_note_flow(&mut self, note_flow: html_layout::NoteFlow) {
        self.note_flow = note_flow;
    }

    pub fn set_reader_overrides(&mut self, overrides: html_layout::ReaderStyleOverrides) {
        self.reader_overrides = overrides;
    }

    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    pub fn parse_to_dom(&mut self, html: &str) -> Document {
        let parsed = parse_html_document(html);
        self.configured_dom(&parsed)
    }

    pub fn parse_with_new_pipeline(&mut self, html: &str, css: Option<&str>) -> PreparedDocument {
        match css {
            Some(css) => self.parse_with_new_pipeline_css_chunks(html, &[css]),
            None => self.parse_with_new_pipeline_css_chunks(html, &[]),
        }
    }

    pub fn parse_with_new_pipeline_css_chunks(&mut self, html: &str, css_chunks: &[&str]) -> PreparedDocument {
        let parsed_html = parse_html_document(html);
        self.build_pipeline_from_parsed_impl(&parsed_html, css_chunks, None).0
    }

    pub fn parse_with_new_pipeline_css_chunks_timed(&mut self, html: &str, css_chunks: &[&str]) -> (PreparedDocument, BuildPipelineTimings) {
        let parsed_html = parse_html_document(html);
        self.build_pipeline_from_parsed_timed(&parsed_html, css_chunks)
    }

    pub fn build_pipeline_from_parsed(&mut self, parsed_html: &ParsedHtml, css_chunks: &[&str]) -> PreparedDocument {
        self.build_pipeline_from_parsed_impl(parsed_html, css_chunks, None).0
    }

    pub(crate) fn build_pipeline_from_parsed_with_media_queries(&mut self, parsed_html: &ParsedHtml, css_chunks: &[&str]) -> (PreparedDocument, html_style::MediaQuerySet) {
        let started = Instant::now();
        let mut timings = BuildPipelineTimings::default();
        let result = self.build_pipeline_from_parsed_impl(parsed_html, css_chunks, Some(&mut timings));
        println!(
            "HTML_PIPELINE_PREPARE total_ms={} dom_ms={} toc_ms={} css_imports_ms={} default_css_ms={} author_css_ms={} rules_ms={} resolve_styles_ms={} selector_index_ms={} resolver_setup_ms={} selector_match_ms={} cascade_ms={} style_store_ms={} layout_inputs_ms={}",
            started.elapsed().as_millis(),
            timings.build_dom_tree.as_millis(),
            timings.rebuild_document_toc.as_millis(),
            timings.resolve_css_imports.as_millis(),
            timings.parse_default_css.as_millis(),
            timings.parse_author_css.as_millis(),
            timings.prepare_style_rules.as_millis(),
            timings.resolve_styles.as_millis(),
            timings.selector_index.as_millis(),
            timings.resolver_setup.as_millis(),
            timings.selector_matching.as_millis(),
            timings.cascade.as_millis(),
            timings.style_store.as_millis(),
            timings.build_layout_inputs.as_millis(),
        );
        result
    }

    pub fn build_pipeline_from_parsed_timed(&mut self, parsed_html: &ParsedHtml, css_chunks: &[&str]) -> (PreparedDocument, BuildPipelineTimings) {
        let mut timings = BuildPipelineTimings::default();
        let (document, _) = self.build_pipeline_from_parsed_impl(parsed_html, css_chunks, Some(&mut timings));
        (document, timings)
    }

    fn build_pipeline_from_parsed_impl(&mut self, parsed_html: &ParsedHtml, css_chunks: &[&str], mut timings: Option<&mut BuildPipelineTimings>) -> (PreparedDocument, html_style::MediaQuerySet) {
        let dom_started = Instant::now();
        let (mut document, base_uri) = self.configured_dom_and_base(parsed_html);
        if let Some(timings) = timings.as_deref_mut() {
            timings.build_dom_tree += dom_started.elapsed();
        }

        let toc_started = Instant::now();
        document.rebuild_document_toc_entries();
        if let Some(timings) = timings.as_deref_mut() {
            timings.rebuild_document_toc += toc_started.elapsed();
        }

        let css_started = Instant::now();
        let mut expanded_css = Vec::new();
        let document_encoding = parsed_html.source_encoding();
        for (stylesheet, implicit_scope_root) in document_stylesheets(&document) {
            match stylesheet {
                StylesheetReference::Inline(css) => self.expand_stylesheet(&css, &base_uri, None, document_encoding, implicit_scope_root, &mut HashSet::new(), 0, &mut expanded_css),
                StylesheetReference::External(href) => {
                    if let Some(resources) = self.resources.as_ref() {
                        let uri = resources.provider.resolve(&base_uri, &href);
                        if let Ok(css) = self.read_stylesheet(resources.provider.as_ref(), &uri, document_encoding) {
                            self.expand_stylesheet(&css, &uri, Some(&uri), document_encoding, implicit_scope_root, &mut HashSet::new(), 0, &mut expanded_css);
                        }
                    }
                }
                StylesheetReference::ExternalWithCharset { href, charset } => {
                    if let Some(resources) = self.resources.as_ref() {
                        let uri = resources.provider.resolve(&base_uri, &href);
                        if let Ok(css) = self.read_stylesheet(resources.provider.as_ref(), &uri, Some(&charset)) {
                            self.expand_stylesheet(&css, &uri, Some(&uri), Some(&charset), implicit_scope_root, &mut HashSet::new(), 0, &mut expanded_css);
                        }
                    }
                }
            }
        }
        // Reader styles participate in the author cascade, but are ordered
        // after every publication sheet so equal-specificity declarations win
        // as promised by PipelineInputs::user_styles.
        for css in css_chunks {
            self.expand_stylesheet(css, &base_uri, None, document_encoding, document.dom_root(), &mut HashSet::new(), 0, &mut expanded_css);
        }
        if let Some(timings) = timings.as_deref_mut() {
            timings.resolve_css_imports += css_started.elapsed();
        }
        let all_css = expanded_css.iter().map(|stylesheet| html_style::AuthorStylesheetInput { css: stylesheet.css.as_str(), implicit_scope_root: stylesheet.implicit_scope_root }).collect::<Vec<_>>();
        let (styled, style_timings, media_queries) = html_style::style_document_with_author_stylesheets_and_environment_and_timings(document, &all_css, self.media_environment);
        if let Some(timings) = timings.as_deref_mut() {
            timings.parse_default_css += style_timings.parse_default_css;
            timings.parse_author_css += style_timings.parse_author_css;
            timings.prepare_style_rules += style_timings.prepare_rules;
            timings.resolve_styles += style_timings.resolve_styles;
            timings.selector_index += style_timings.selector_index;
            timings.resolver_setup += style_timings.resolver_setup;
            timings.selector_matching += style_timings.selector_matching;
            timings.cascade += style_timings.cascade;
            timings.style_store += style_timings.style_store;
        }

        let layout_started = Instant::now();
        let (document, styles) = styled.into_parts();
        let styles = styles.with_reader_overrides(&document, &self.reader_overrides).expect("reader overrides must preserve complete computed styles");
        let prepared = PreparedDocument::try_new_with_note_flow(document, styles, self.note_flow).expect("style resolver must produce complete styles for its document");
        if let Some(timings) = timings {
            timings.build_layout_inputs += layout_started.elapsed();
        }
        (prepared, media_queries)
    }

    fn configured_dom(&self, parsed: &ParsedHtml) -> Document {
        self.configured_dom_and_base(parsed).0
    }

    fn configured_dom_and_base(&self, parsed: &ParsedHtml) -> (Document, String) {
        let metadata = parsed.head_metadata();
        let mut document = parsed.build_dom();
        document.set_root_font_size(self.root_font_size);
        document.set_title(self.title.clone().or(metadata.title));
        let base_uri = self.resources.as_ref().map_or_else(String::new, |resources| effective_document_base_uri(&document, resources));
        if let Some(resources) = self.resources.as_ref() {
            for image in document.images_mut() {
                if let ImageSource::Uri(uri) = &mut image.source {
                    *uri = resources.provider.resolve(&base_uri, uri);
                }
            }
        }
        (document, base_uri)
    }

    fn expand_stylesheet(
        &self, css: &str, base_uri: &str, source_uri: Option<&str>, fallback_label: Option<&str>, implicit_scope_root: Option<DomNodeId>, active_imports: &mut HashSet<String>, depth: usize, output: &mut Vec<ExpandedStylesheet>,
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
                StylesheetEntry::Css(css) if !css.trim().is_empty() => output.push(ExpandedStylesheet { css, implicit_scope_root }),
                StylesheetEntry::Css(_) => {}
                StylesheetEntry::Import(import) => {
                    let Some(resources) = self.resources.as_ref() else { continue };
                    let uri = resources.provider.resolve(base_uri, &import.url);
                    if active_imports.contains(&uri) {
                        continue;
                    }
                    let Ok(imported_css) = self.read_stylesheet(resources.provider.as_ref(), &uri, fallback_label) else { continue };
                    let mut imported_output = Vec::new();
                    self.expand_stylesheet(&imported_css, &uri, Some(&uri), fallback_label, implicit_scope_root, active_imports, depth + 1, &mut imported_output);
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
        } else if element.tag().eq_ignore_ascii_case("link") && link_is_active_stylesheet(element.attr("rel")) && link_type_is_css(element.attr("type")) {
            let Some(href) = element.attr("href").map(str::trim).filter(|href| !href.is_empty()).map(str::to_owned) else { continue };
            let stylesheet = match element.attr("charset").map(str::trim).filter(|charset| !charset.is_empty()) {
                Some(charset) => StylesheetReference::ExternalWithCharset { href, charset: charset.to_owned() },
                None => StylesheetReference::External(href),
            };
            stylesheets.push((stylesheet, implicit_scope_root));
        }
    }
    stylesheets
}

fn link_is_active_stylesheet(rel: Option<&str>) -> bool {
    let Some(rel) = rel else { return false };
    let mut stylesheet = false;
    for token in rel.split_ascii_whitespace() {
        if token.eq_ignore_ascii_case("alternate") {
            return false;
        }
        stylesheet |= token.eq_ignore_ascii_case("stylesheet");
    }
    stylesheet
}

fn link_type_is_css(media_type: Option<&str>) -> bool {
    media_type.map(|media_type| media_type.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/css")).unwrap_or(true)
}

fn style_type_is_css(media_type: Option<&str>) -> bool {
    media_type.map(|media_type| media_type.trim().is_empty() || media_type.trim().eq_ignore_ascii_case("text/css")).unwrap_or(true)
}

fn effective_document_base_uri(document: &Document, resources: &ResourceContext) -> String {
    const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
    let Some(href) = document
        .node_ids()
        .filter_map(|node| document.element_ref(node))
        .find(|element| element.namespace() == Some(HTML_NAMESPACE) && element.tag().eq_ignore_ascii_case("base") && element.attr("href").is_some())
        .and_then(|element| element.attr("href"))
    else {
        return resources.base_uri.clone();
    };
    let path = href.split(['?', '#']).next().unwrap_or(href);
    if path.is_empty() {
        return resources.base_uri.clone();
    }
    let mut resolved = resources.provider.resolve(&resources.base_uri, href);
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

#[derive(Default, Debug, Clone)]
pub struct BuildPipelineTimings {
    pub build_dom_tree: Duration,
    pub parse_default_css: Duration,
    pub parse_author_css: Duration,
    pub resolve_css_imports: Duration,
    pub prepare_style_rules: Duration,
    pub resolve_styles: Duration,
    pub selector_index: Duration,
    pub resolver_setup: Duration,
    pub selector_matching: Duration,
    pub cascade: Duration,
    pub style_store: Duration,
    pub build_layout_inputs: Duration,
    pub rebuild_document_toc: Duration,
}

#[cfg(test)]
mod tests {
    use super::{DocumentFactory, decode_stylesheet_bytes};
    use html_dom::{Document, ElementRef, NodeRef};
    use html_resources::ResourceProvider;
    use std::collections::HashMap;
    use std::io;
    use std::sync::Arc;

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
    fn builds_toc_entries_from_html_headers() {
        let html = "<html><body><h1>Intro</h1><h2 id=\"chapter-a\">Chapter <em>A</em></h2></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_to_dom(html);

        assert_eq!(document.toc_entries().len(), 2);

        let first = &document.toc_entries()[0];
        assert_eq!(first.level, 1);
        assert_eq!(document.string(first.title), "Intro");
        assert!(document.string(first.href).starts_with("#heading-"));

        let second = &document.toc_entries()[1];
        assert_eq!(second.level, 2);
        assert_eq!(document.string(second.title), "Chapter A");
        assert_eq!(document.string(second.href), "#chapter-a");
    }

    #[test]
    fn handles_self_closing_anchor_without_leaking_link_scope() {
        let html = "<html><body><p><a id=\"p77\"/>After</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_to_dom(html);

        let body = find_body(&document);
        let p = child_element_by_tag(&document, body, "p");
        let children: Vec<_> = p.children().collect();

        assert_eq!(children.len(), 2, "self-closed anchor should not wrap following text");
        assert_eq!(document.element_ref(children[0]).expect("first child should be anchor").tag(), "a");
        assert!(matches!(document.node_ref(children[1]), Some(NodeRef::Text(text)) if text.text() == "After"));
    }

    #[test]
    fn resolves_linked_and_inline_stylesheets_in_document_order() {
        let provider = Arc::new(MemoryProvider { resources: HashMap::from([("book/first.css".to_owned(), b"p { color: red; }".to_vec()), ("book/last.css".to_owned(), b"p { color: blue; }".to_vec())]) });
        let html = r#"
            <html><head>
                <link rel="stylesheet" href="first.css">
                <style>p { color: green; }</style>
                <link rel="stylesheet" href="last.css">
            </head><body><p>text</p></body></html>
        "#;
        let mut factory = DocumentFactory::new();
        factory.set_resource_context(provider, "book/chapter.xhtml");

        let prepared = factory.parse_with_new_pipeline(html, None);
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
            </head><body><p>text</p><img src="cat.png"></body></html>
        "#;
        let mut factory = DocumentFactory::new();
        factory.set_resource_context(provider, "book/chapter.html");

        let document = factory.parse_to_dom(html);
        assert!(matches!(&document.images()[0].source, html_dom::ImageSource::Uri(uri) if uri == "book/resources/cat.png"));

        let prepared = factory.parse_with_new_pipeline(html, None);
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
        factory.set_resource_context(provider, "book/chapter.xhtml");

        let prepared = factory.parse_with_new_pipeline(html, Some("p { color: green; }"));
        let paragraph = (0..prepared.box_count()).find(|&box_idx| prepared.get_tag(box_idx) == "p").expect("paragraph box");
        assert_eq!(prepared.box_text_color(paragraph), Some(0x008000FF));
    }

    #[test]
    fn resolves_nested_imports_relative_to_the_importing_stylesheet_and_breaks_cycles() {
        let provider = Arc::new(MemoryProvider { resources: HashMap::from([("book/a.css".to_owned(), b"@import 'b.css'; p { color: red; }".to_vec()), ("book/b.css".to_owned(), b"@import 'a.css'; p { font-weight: 700; }".to_vec())]) });
        let html = "<html><head><style>@import 'a.css';</style></head><body><p>text</p></body></html>";
        let mut factory = DocumentFactory::new();
        factory.set_resource_context(provider, "book/chapter.xhtml");

        let prepared = factory.parse_with_new_pipeline(html, None);
        let paragraph = (0..prepared.box_count()).find(|&box_idx| prepared.get_tag(box_idx) == "p").expect("paragraph box");
        assert_eq!(prepared.box_text_color(paragraph), Some(0xFF0000FF));
    }

    #[test]
    fn import_conditions_and_layers_survive_resource_expansion() {
        let provider = Arc::new(MemoryProvider { resources: HashMap::from([("book/theme.css".to_owned(), b"p { color: red !important; }".to_vec())]) });
        let html = "<html><head><style>@import 'theme.css' layer(theme) supports(width: 1px) screen; p { color: green !important; }</style></head><body><p>text</p></body></html>";
        let mut factory = DocumentFactory::new();
        factory.set_resource_context(provider, "book/chapter.xhtml");

        let prepared = factory.parse_with_new_pipeline(html, None);
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
