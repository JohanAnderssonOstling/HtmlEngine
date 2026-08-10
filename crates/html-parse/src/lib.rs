use encoding_rs::{Encoding, UTF_8};
use html_dom::{Document, DocumentBuildError, DocumentBuilder, DocumentMode, DocumentTocEntry, DomAttribute, DomElementAttributeCache, DomElementData, DomNodeId, ImageResource, ImageSource, NodeRef, Rootless};
use html5ever::driver::{self, ParseOpts};
use html5ever::tendril::TendrilSink;
use html5ever::tree_builder::{QuirksMode as Html5everQuirksMode, TreeBuilderOpts};
use html5ever::{LocalName, Namespace, QualName};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use scraper::{ElementRef, Html, Node};
use std::fmt;
use std::sync::Arc;

pub struct ParsedHtml {
    document: ParsedMarkup,
    errors: Vec<String>,
    source_encoding: Option<String>,
}

enum ParsedMarkup {
    Html(Html),
    Xml(HtmlSyntaxTree),
}

pub struct ParsedHtmlFragment {
    fragment: Html,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MarkupSyntax {
    #[default]
    Html,
    Xml,
}

impl MarkupSyntax {
    pub fn from_uri(uri: &str) -> Self {
        let path = uri.split(['?', '#']).next().unwrap_or(uri);
        match path.rsplit_once('.').map(|(_, extension)| extension) {
            Some(extension) if matches_ignore_ascii_case(extension, &["xht", "xhtml", "xml", "svg"]) => Self::Xml,
            _ => Self::Html,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmlParseError {
    offset: u64,
    message: String,
}

impl XmlParseError {
    fn new(offset: u64, message: impl Into<String>) -> Self {
        Self { offset, message: message.into() }
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for XmlParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "XML parse error at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for XmlParseError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HtmlScriptingMode {
    #[default]
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtmlParserOptions {
    pub scripting: HtmlScriptingMode,
    pub exact_errors: bool,
}

impl Default for HtmlParserOptions {
    fn default() -> Self {
        Self { scripting: HtmlScriptingMode::Disabled, exact_errors: false }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtmlNamespace {
    Html,
    MathMl,
    Svg,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HtmlFragmentContext {
    pub namespace: HtmlNamespace,
    pub local_name: String,
}

impl HtmlFragmentContext {
    pub fn html(local_name: impl Into<String>) -> Self {
        Self { namespace: HtmlNamespace::Html, local_name: local_name.into() }
    }

    pub fn svg(local_name: impl Into<String>) -> Self {
        Self { namespace: HtmlNamespace::Svg, local_name: local_name.into() }
    }

    pub fn math_ml(local_name: impl Into<String>) -> Self {
        Self { namespace: HtmlNamespace::MathMl, local_name: local_name.into() }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtmlQuirksMode {
    NoQuirks,
    LimitedQuirks,
    Quirks,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HtmlSyntaxTree {
    pub nodes: Vec<HtmlSyntaxNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HtmlSyntaxNode {
    Doctype { name: String, public_id: String, system_id: String },
    Comment(String),
    Text(String),
    Element(HtmlSyntaxElement),
    TemplateContents(Vec<HtmlSyntaxNode>),
    ProcessingInstruction { target: String, data: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HtmlSyntaxElement {
    pub local_name: String,
    pub namespace: Option<String>,
    pub attributes: Vec<HtmlSyntaxAttribute>,
    pub children: Vec<HtmlSyntaxNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HtmlSyntaxAttribute {
    pub name: String,
    pub local_name: String,
    pub namespace: Option<String>,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StylesheetReference {
    Inline(String),
    External(String),
    ExternalWithCharset { href: String, charset: String },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HeadMetadata {
    pub title: Option<String>,
    pub stylesheets: Vec<StylesheetReference>,
}

pub fn parse_html_document(html: &str) -> ParsedHtml {
    parse_html_document_with_options(html, HtmlParserOptions::default())
}

/// Decodes and parses an HTML byte stream before any invalid byte sequences
/// can be replaced by a resource adapter. A transport/container charset takes
/// precedence over an in-document declaration, as required by HTML.
pub fn parse_html_document_bytes(bytes: &[u8], transport_encoding: Option<&str>) -> ParsedHtml {
    let (source, encoding) = decode_html_bytes_with_encoding(bytes, transport_encoding);
    let mut parsed = parse_html_document(&source);
    parsed.source_encoding = Some(encoding.name().to_owned());
    parsed
}

/// Decodes an HTML resource while preserving the existing UTF-8 fast path.
/// Callers that subsequently parse the returned text should retain the source
/// bytes until this boundary rather than using `String::from_utf8_lossy`.
pub fn decode_html_bytes(bytes: &[u8], transport_encoding: Option<&str>) -> String {
    decode_html_bytes_with_encoding(bytes, transport_encoding).0.into_owned()
}

fn decode_html_bytes_with_encoding<'a>(bytes: &'a [u8], transport_encoding: Option<&str>) -> (std::borrow::Cow<'a, str>, &'static Encoding) {
    let (encoding, bom_len) = Encoding::for_bom(bytes)
        .map(|(encoding, length)| (encoding, length))
        .or_else(|| transport_encoding.and_then(|label| Encoding::for_label(label.trim().as_bytes())).map(|encoding| (encoding, 0)))
        .or_else(|| prescan_html_encoding(bytes).map(|encoding| (encoding, 0)))
        .unwrap_or((UTF_8, 0));
    (encoding.decode_without_bom_handling(&bytes[bom_len..]).0, encoding)
}

fn prescan_html_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    let mut cursor = 0usize;
    let mut in_noscript = false;
    let mut head_closed = false;
    let mut landed_on_boundary_from_tag = false;
    // The prescan covers byte offsets 0 through 1023. A tag may extend beyond
    // the window once its opening '<' was encountered inside it.
    let end = bytes.len().min(1024);
    while cursor <= end && cursor < bytes.len() {
        if starts_ascii_case_insensitive(bytes, cursor, b"<!--") {
            cursor = find_bytes(bytes, cursor + 4, b"-->").map_or(bytes.len(), |end| end + 3);
            continue;
        }
        if starts_raw_text_element(bytes, cursor, b"style") || starts_raw_text_element(bytes, cursor, b"title") {
            let name = if starts_raw_text_element(bytes, cursor, b"style") { b"style".as_slice() } else { b"title".as_slice() };
            let close = [b"</".as_slice(), name].concat();
            cursor = find_ascii_case_insensitive(bytes, tag_end(bytes, cursor + 1).saturating_add(1), &close).map_or(bytes.len(), |start| tag_end(bytes, start + close.len()).saturating_add(1));
            continue;
        }
        if starts_ascii_case_insensitive(bytes, cursor, b"<meta") && bytes.get(cursor + 5).is_none_or(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>')) {
            if cursor == end && (landed_on_boundary_from_tag || in_noscript || head_closed) {
                break;
            }
            let end = tag_end(bytes, cursor + 5);
            if let Some(label) = meta_charset_label(&bytes[cursor + 5..end]) {
                if let Some(encoding) = Encoding::for_label(label) {
                    return Some(encoding);
                }
                if let Some(decoded) = decode_ascii_numeric_references(label)
                    && let Some(encoding) = Encoding::for_label(&decoded)
                {
                    return Some(encoding);
                }
            }
            cursor = end.saturating_add(1);
            landed_on_boundary_from_tag = false;
            continue;
        }
        if bytes[cursor] == b'<' {
            if starts_ascii_case_insensitive(bytes, cursor, b"<noscript") {
                in_noscript = true;
            } else if starts_ascii_case_insensitive(bytes, cursor, b"</noscript") {
                in_noscript = false;
            }
            if starts_ascii_case_insensitive(bytes, cursor, b"</head") {
                head_closed = true;
            }
            cursor = tag_end(bytes, cursor + 1).saturating_add(1);
            landed_on_boundary_from_tag = cursor == end;
        } else {
            cursor += 1;
            landed_on_boundary_from_tag = false;
        }
    }
    None
}

fn decode_ascii_numeric_references(label: &[u8]) -> Option<Vec<u8>> {
    if !label.contains(&b'&') {
        return None;
    }
    let mut decoded = Vec::with_capacity(label.len());
    let mut cursor = 0usize;
    while cursor < label.len() {
        if label.get(cursor..cursor + 2) == Some(b"&#") {
            let hex = label.get(cursor + 2).is_some_and(|byte| matches!(byte, b'x' | b'X'));
            let digits_start = cursor + if hex { 3 } else { 2 };
            let Some(relative_end) = label.get(digits_start..)?.iter().position(|byte| *byte == b';') else {
                return None;
            };
            let digits_end = digits_start + relative_end;
            let digits = std::str::from_utf8(&label[digits_start..digits_end]).ok()?;
            let value = u32::from_str_radix(digits, if hex { 16 } else { 10 }).ok()?;
            decoded.push(u8::try_from(value).ok()?);
            cursor = digits_end + 1;
        } else {
            decoded.push(label[cursor]);
            cursor += 1;
        }
    }
    Some(decoded)
}

fn starts_raw_text_element(bytes: &[u8], cursor: usize, name: &[u8]) -> bool {
    let start = cursor.saturating_add(1);
    starts_ascii_case_insensitive(bytes, start, name) && bytes.get(start + name.len()).is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
}

fn meta_charset_label(attributes: &[u8]) -> Option<&[u8]> {
    let mut cursor = 0usize;
    while cursor < attributes.len() {
        while attributes.get(cursor).is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'/') {
            cursor += 1;
        }
        let name_start = cursor;
        while attributes.get(cursor).is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'/' | b'>')) {
            cursor += 1;
        }
        let name = &attributes[name_start..cursor];
        while attributes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if attributes.get(cursor) != Some(&b'=') {
            while attributes.get(cursor).is_some_and(|byte| !byte.is_ascii_whitespace()) {
                cursor += 1;
            }
            continue;
        }
        cursor += 1;
        while attributes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let (value_start, value_end) = match attributes.get(cursor).copied() {
            Some(quote @ (b'\'' | b'"')) => {
                cursor += 1;
                let start = cursor;
                while attributes.get(cursor).is_some_and(|byte| *byte != quote) {
                    cursor += 1;
                }
                (start, cursor)
            }
            Some(_) => {
                let start = cursor;
                while attributes.get(cursor).is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'/' | b'>')) {
                    cursor += 1;
                }
                (start, cursor)
            }
            None => return None,
        };
        if name.eq_ignore_ascii_case(b"charset") {
            return Some(&attributes[value_start..value_end]);
        }
        cursor = cursor.saturating_add(1);
    }
    None
}

fn starts_ascii_case_insensitive(bytes: &[u8], start: usize, needle: &[u8]) -> bool {
    bytes.get(start..start.saturating_add(needle.len())).is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
}

fn find_bytes(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    bytes.get(start..)?.windows(needle.len()).position(|window| window == needle).map(|offset| start + offset)
}

fn find_ascii_case_insensitive(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    bytes.get(start..)?.windows(needle.len()).position(|window| window.eq_ignore_ascii_case(needle)).map(|offset| start + offset)
}

fn tag_end(bytes: &[u8], start: usize) -> usize {
    let mut cursor = start;
    let mut quote = None;
    while cursor < bytes.len() {
        match (quote, bytes[cursor]) {
            (Some(expected), byte) if byte == expected => quote = None,
            (None, byte @ (b'\'' | b'"')) => quote = Some(byte),
            (None, b'>') => return cursor,
            _ => {}
        }
        cursor += 1;
    }
    bytes.len()
}

pub fn parse_html_document_with_options(html: &str, options: HtmlParserOptions) -> ParsedHtml {
    let parser = driver::parse_document(scraper::HtmlTreeSink::new(Html::new_document()), html5ever_options(options));
    let document = parser.one(normalize_self_closing_anchor_tags(html));
    let errors = document.errors.iter().map(ToString::to_string).collect();
    ParsedHtml { document: ParsedMarkup::Html(document), errors, source_encoding: declared_source_encoding(html) }
}

pub fn parse_document(source: &str, syntax: MarkupSyntax) -> Result<ParsedHtml, XmlParseError> {
    match syntax {
        MarkupSyntax::Html => Ok(parse_html_document(source)),
        MarkupSyntax::Xml => parse_xml_document(source),
    }
}

pub fn parse_xml_document(xml: &str) -> Result<ParsedHtml, XmlParseError> {
    let document = parse_xml_syntax_tree(xml)?;
    Ok(ParsedHtml { document: ParsedMarkup::Xml(document), errors: Vec::new(), source_encoding: declared_source_encoding(xml) })
}

fn declared_source_encoding(source: &str) -> Option<String> {
    let prefix = source.get(..source.len().min(1024))?;
    let lower = prefix.to_ascii_lowercase();
    for marker in ["encoding", "charset"] {
        let mut search_from = 0;
        while let Some(relative) = lower[search_from..].find(marker) {
            let after_name = search_from + relative + marker.len();
            let rest = &prefix[after_name..];
            let equals = rest.find('=')?;
            if !rest[..equals].chars().all(char::is_whitespace) {
                search_from = after_name;
                continue;
            }
            let value = rest[equals + 1..].trim_start();
            let quote = value.chars().next()?;
            if matches!(quote, '\'' | '"') {
                let quoted = &value[quote.len_utf8()..];
                let end = quoted.find(quote)?;
                return nonempty(quoted[..end].to_owned());
            }
            let end = value.find(|character: char| character.is_ascii_whitespace() || character == '>' || character == ';').unwrap_or(value.len());
            return nonempty(value[..end].to_owned());
        }
    }
    None
}

pub fn parse_html_fragment(html: &str, context: &HtmlFragmentContext, options: HtmlParserOptions) -> ParsedHtmlFragment {
    let context_name = QualName::new(None, context_namespace(context.namespace), LocalName::from(context.local_name.as_str()));
    let parser = driver::parse_fragment(scraper::HtmlTreeSink::new(Html::new_fragment()), html5ever_options(options), context_name, Vec::new(), options.scripting == HtmlScriptingMode::Enabled);
    ParsedHtmlFragment { fragment: parser.one(html) }
}

pub fn plain_text_from_fragment(html: &str) -> String {
    let parsed = parse_html_fragment(html, &HtmlFragmentContext::html("div"), HtmlParserOptions::default());
    let mut text = String::new();
    append_plain_text(&parsed.syntax_tree().nodes, &mut text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn append_plain_text(nodes: &[HtmlSyntaxNode], output: &mut String) {
    for node in nodes {
        match node {
            HtmlSyntaxNode::Text(text) => output.push_str(text),
            HtmlSyntaxNode::Element(element) if matches!(element.local_name.as_str(), "script" | "style" | "template" | "noscript" | "head") => {}
            HtmlSyntaxNode::Element(element) => {
                let block = is_text_boundary_element(&element.local_name);
                if block && !output.ends_with(char::is_whitespace) {
                    output.push(' ');
                }
                append_plain_text(&element.children, output);
                if block && !output.ends_with(char::is_whitespace) {
                    output.push(' ');
                }
            }
            HtmlSyntaxNode::TemplateContents(_) | HtmlSyntaxNode::Doctype { .. } | HtmlSyntaxNode::Comment(_) | HtmlSyntaxNode::ProcessingInstruction { .. } => {}
        }
    }
}

fn is_text_boundary_element(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "br"
            | "dd"
            | "details"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "tfoot"
            | "th"
            | "thead"
            | "tr"
            | "ul"
    )
}

impl ParsedHtml {
    pub fn source_encoding(&self) -> Option<&str> {
        self.source_encoding.as_deref()
    }

    pub fn build_dom(&self) -> Document {
        self.build_dom_checked().expect("parser produced an invalid DOM")
    }

    pub fn build_dom_checked(&self) -> Result<Document, DocumentBuildError> {
        build_dom_document(self)
    }

    pub fn head_metadata(&self) -> HeadMetadata {
        let ParsedMarkup::Html(document) = &self.document else {
            let ParsedMarkup::Xml(tree) = &self.document else { unreachable!("parsed markup variants are exhaustive") };
            return head_metadata_from_syntax_tree(tree);
        };
        let mut metadata = HeadMetadata::default();
        let Ok(selector) = scraper::Selector::parse("head title, link, style") else {
            return metadata;
        };
        for element in document.select(&selector) {
            match element.value().name() {
                "title" if metadata.title.is_none() => {
                    metadata.title = nonempty(element.text().collect::<String>());
                }
                "style" if style_type_is_css(element.value().attr("type")) => {
                    if let Some(css) = nonempty(element.text().collect::<String>()) {
                        metadata.stylesheets.push(StylesheetReference::Inline(css));
                    }
                }
                "link" if link_is_active_stylesheet(element.value().attr("rel")) && link_type_is_css(element.value().attr("type")) => {
                    if let Some(href) = element.value().attr("href").and_then(|href| nonempty(href.to_owned())) {
                        match element.value().attr("charset").and_then(|charset| nonempty(charset.to_owned())) {
                            Some(charset) => metadata.stylesheets.push(StylesheetReference::ExternalWithCharset { href, charset }),
                            None => metadata.stylesheets.push(StylesheetReference::External(href)),
                        }
                    }
                }
                _ => {}
            }
        }
        metadata
    }

    pub fn errors(&self) -> impl ExactSizeIterator<Item = &str> {
        self.errors.iter().map(String::as_str)
    }

    pub fn quirks_mode(&self) -> HtmlQuirksMode {
        match &self.document {
            ParsedMarkup::Html(document) => map_quirks_mode(document.quirks_mode),
            ParsedMarkup::Xml(_) => HtmlQuirksMode::NoQuirks,
        }
    }

    pub fn syntax_tree(&self) -> HtmlSyntaxTree {
        match &self.document {
            ParsedMarkup::Html(document) => syntax_tree(document),
            ParsedMarkup::Xml(document) => document.clone(),
        }
    }
}

impl ParsedHtmlFragment {
    pub fn errors(&self) -> impl ExactSizeIterator<Item = &str> {
        self.fragment.errors.iter().map(AsRef::as_ref)
    }

    pub fn syntax_tree(&self) -> HtmlSyntaxTree {
        let mut tree = syntax_tree(&self.fragment);
        if tree.nodes.len() == 1
            && let HtmlSyntaxNode::Element(root) = &mut tree.nodes[0]
            && root.local_name == "html"
            && root.namespace.as_deref() == Some("http://www.w3.org/1999/xhtml")
        {
            return HtmlSyntaxTree { nodes: std::mem::take(&mut root.children) };
        }
        tree
    }
}

fn html5ever_options(options: HtmlParserOptions) -> ParseOpts {
    ParseOpts {
        tokenizer: html5ever::tokenizer::TokenizerOpts { exact_errors: options.exact_errors, ..Default::default() },
        tree_builder: TreeBuilderOpts { exact_errors: options.exact_errors, scripting_enabled: options.scripting == HtmlScriptingMode::Enabled, ..Default::default() },
    }
}

fn context_namespace(namespace: HtmlNamespace) -> Namespace {
    match namespace {
        HtmlNamespace::Html => Namespace::from("http://www.w3.org/1999/xhtml"),
        HtmlNamespace::MathMl => Namespace::from("http://www.w3.org/1998/Math/MathML"),
        HtmlNamespace::Svg => Namespace::from("http://www.w3.org/2000/svg"),
    }
}

fn map_quirks_mode(mode: Html5everQuirksMode) -> HtmlQuirksMode {
    match mode {
        Html5everQuirksMode::NoQuirks => HtmlQuirksMode::NoQuirks,
        Html5everQuirksMode::LimitedQuirks => HtmlQuirksMode::LimitedQuirks,
        Html5everQuirksMode::Quirks => HtmlQuirksMode::Quirks,
    }
}

fn syntax_tree(html: &Html) -> HtmlSyntaxTree {
    HtmlSyntaxTree { nodes: syntax_children(html.tree.root()) }
}

fn syntax_children(parent: ego_tree::NodeRef<'_, Node>) -> Vec<HtmlSyntaxNode> {
    parent.children().filter_map(syntax_node).collect()
}

fn syntax_node(node: ego_tree::NodeRef<'_, Node>) -> Option<HtmlSyntaxNode> {
    match node.value() {
        Node::Document => None,
        Node::Fragment => Some(HtmlSyntaxNode::TemplateContents(syntax_children(node))),
        Node::Doctype(doctype) => Some(HtmlSyntaxNode::Doctype { name: doctype.name().to_owned(), public_id: doctype.public_id().to_owned(), system_id: doctype.system_id().to_owned() }),
        Node::Comment(comment) => Some(HtmlSyntaxNode::Comment(comment.to_string())),
        Node::Text(text) => Some(HtmlSyntaxNode::Text(text.to_string())),
        Node::ProcessingInstruction(pi) => Some(HtmlSyntaxNode::ProcessingInstruction { target: pi.target.to_string(), data: pi.data.to_string() }),
        Node::Element(element) => {
            let attributes = element
                .attrs
                .iter()
                .map(|(name, value)| HtmlSyntaxAttribute {
                    name: name.prefix.as_ref().map(|prefix| format!("{}:{}", prefix.as_ref(), name.local.as_ref())).unwrap_or_else(|| name.local.to_string()),
                    local_name: name.local.to_string(),
                    namespace: (!name.ns.is_empty()).then(|| name.ns.to_string()),
                    value: value.to_string(),
                })
                .collect();
            Some(HtmlSyntaxNode::Element(HtmlSyntaxElement { local_name: element.name.local.to_string(), namespace: (!element.name.ns.is_empty()).then(|| element.name.ns.to_string()), attributes, children: syntax_children(node) }))
        }
    }
}

fn parse_xml_syntax_tree(xml: &str) -> Result<HtmlSyntaxTree, XmlParseError> {
    let mut reader = NsReader::from_str(xml);
    {
        let config = reader.config_mut();
        config.allow_dangling_amp = false;
        config.allow_unmatched_ends = false;
        config.check_comments = true;
        config.check_end_names = true;
        config.expand_empty_elements = false;
        // XML permits whitespace between an end-tag name and `>`.
        config.trim_markup_names_in_closing_tags = true;
        config.trim_text(false);
    }
    let mut version = XmlVersion::Implicit1_0;
    let mut nodes = Vec::new();
    let mut elements = Vec::<HtmlSyntaxElement>::new();
    let mut root_seen = false;
    let mut doctype_seen = false;
    let mut declaration_seen = false;
    let mut content_seen_before_declaration = false;
    let mut xhtml_entities = false;

    loop {
        let event = reader.read_event().map_err(|error| xml_reader_error(&reader, error))?;
        match event {
            Event::Start(start) => {
                if elements.is_empty() {
                    if root_seen {
                        return Err(XmlParseError::new(reader.buffer_position(), "an XML document must have exactly one root element"));
                    }
                    root_seen = true;
                }
                elements.push(xml_syntax_element(&reader, &start, version, xhtml_entities)?);
            }
            Event::Empty(start) => {
                if elements.is_empty() {
                    if root_seen {
                        return Err(XmlParseError::new(reader.buffer_position(), "an XML document must have exactly one root element"));
                    }
                    root_seen = true;
                }
                let element = xml_syntax_element(&reader, &start, version, xhtml_entities)?;
                push_xml_node(&mut nodes, &mut elements, HtmlSyntaxNode::Element(element));
            }
            Event::End(_) => {
                let Some(element) = elements.pop() else {
                    return Err(XmlParseError::new(reader.buffer_position(), "closing element has no open element"));
                };
                push_xml_node(&mut nodes, &mut elements, HtmlSyntaxNode::Element(element));
            }
            Event::Text(text) => {
                let text = text.xml_content(version).map_err(|error| xml_reader_error(&reader, error))?;
                if elements.is_empty() {
                    if !declaration_seen {
                        content_seen_before_declaration = true;
                    }
                    if !text.trim().is_empty() {
                        return Err(XmlParseError::new(reader.buffer_position(), "character data is not allowed outside the root element"));
                    }
                } else {
                    push_xml_text(&mut elements, text.as_ref());
                }
            }
            Event::CData(text) => {
                if elements.is_empty() {
                    return Err(XmlParseError::new(reader.buffer_position(), "CDATA is not allowed outside the root element"));
                }
                let text = text.xml_content(version).map_err(|error| xml_reader_error(&reader, error))?;
                push_xml_text(&mut elements, text.as_ref());
            }
            Event::GeneralRef(reference) => {
                if elements.is_empty() {
                    return Err(XmlParseError::new(reader.buffer_position(), "entity references are not allowed outside the root element"));
                }
                let value = if let Some(character) = reference.resolve_char_ref().map_err(|error| xml_reader_error(&reader, error))? {
                    character.to_string()
                } else {
                    let name = reference.decode().map_err(|error| xml_reader_error(&reader, error))?;
                    resolve_xml_entity(name.as_ref(), xhtml_entities).ok_or_else(|| XmlParseError::new(reader.buffer_position(), format!("undefined entity '&{name};'")))?.to_owned()
                };
                push_xml_text(&mut elements, &value);
            }
            Event::Comment(comment) => {
                if !declaration_seen {
                    content_seen_before_declaration = true;
                }
                let comment = comment.xml_content(version).map_err(|error| xml_reader_error(&reader, error))?.into_owned();
                push_xml_node(&mut nodes, &mut elements, HtmlSyntaxNode::Comment(comment));
            }
            Event::PI(instruction) => {
                if !declaration_seen {
                    content_seen_before_declaration = true;
                }
                let target = utf8_xml_bytes(instruction.target(), reader.buffer_position(), "processing-instruction target")?.to_owned();
                let data = utf8_xml_bytes(instruction.content(), reader.buffer_position(), "processing-instruction data")?.trim_start().to_owned();
                push_xml_node(&mut nodes, &mut elements, HtmlSyntaxNode::ProcessingInstruction { target, data });
            }
            Event::DocType(doctype) => {
                if !declaration_seen {
                    content_seen_before_declaration = true;
                }
                if !elements.is_empty() || root_seen || doctype_seen {
                    return Err(XmlParseError::new(reader.buffer_position(), "DOCTYPE must occur at most once before the root element"));
                }
                doctype_seen = true;
                let raw = doctype.xml_content(version).map_err(|error| xml_reader_error(&reader, error))?.into_owned();
                xhtml_entities = is_xhtml_doctype(&raw);
                let name = raw.split_ascii_whitespace().next().unwrap_or_default().to_owned();
                nodes.push(HtmlSyntaxNode::Doctype { name, public_id: String::new(), system_id: String::new() });
            }
            Event::Decl(declaration) => {
                if declaration_seen || content_seen_before_declaration || root_seen || doctype_seen || !nodes.is_empty() {
                    return Err(XmlParseError::new(reader.buffer_position(), "the XML declaration must be the first document item"));
                }
                declaration_seen = true;
                let declared_version = declaration.version().map_err(|error| xml_reader_error(&reader, error))?;
                version = match declared_version.as_ref() {
                    b"1.0" => XmlVersion::Explicit1_0,
                    b"1.1" => XmlVersion::Explicit1_1,
                    unsupported => {
                        return Err(XmlParseError::new(reader.buffer_position(), format!("unsupported XML version '{}'", String::from_utf8_lossy(unsupported))));
                    }
                };
            }
            Event::Eof => break,
        }
    }

    if !elements.is_empty() {
        return Err(XmlParseError::new(reader.buffer_position(), "unclosed root element"));
    }
    if !root_seen {
        return Err(XmlParseError::new(reader.buffer_position(), "an XML document must have a root element"));
    }
    Ok(HtmlSyntaxTree { nodes })
}

fn xml_syntax_element(reader: &NsReader<&[u8]>, start: &BytesStart<'_>, version: XmlVersion, xhtml_entities: bool) -> Result<HtmlSyntaxElement, XmlParseError> {
    let offset = reader.buffer_position();
    utf8_xml_bytes(start.name().as_ref(), offset, "element name")?;
    let (namespace, local_name) = reader.resolver().resolve_element(start.name());
    let namespace = resolved_namespace(namespace, offset)?;
    let local_name = utf8_xml_bytes(local_name.as_ref(), offset, "element local name")?.to_owned();
    let mut attributes = Vec::new();
    let mut expanded_attribute_names = std::collections::HashSet::new();
    for attribute in start.attributes().with_checks(true) {
        let attribute = attribute.map_err(|error| XmlParseError::new(offset, error.to_string()))?;
        let raw_attribute_name = utf8_xml_bytes(attribute.key.as_ref(), offset, "attribute name")?.to_owned();
        let (attribute_namespace, attribute_local_name) = reader.resolver().resolve_attribute(attribute.key);
        let attribute_namespace = resolved_namespace(attribute_namespace, offset)?;
        let attribute_local_name = utf8_xml_bytes(attribute_local_name.as_ref(), offset, "attribute local name")?.to_owned();
        if !expanded_attribute_names.insert((attribute_namespace.clone(), attribute_local_name.clone())) {
            return Err(XmlParseError::new(offset, format!("duplicate expanded attribute name '{{{}}}{attribute_local_name}'", attribute_namespace.as_deref().unwrap_or_default())));
        }
        let value = attribute.normalized_value_with(version, 128, |name| resolve_xml_entity(name, xhtml_entities)).map_err(|error| XmlParseError::new(offset, error.to_string()))?.into_owned();
        attributes.push(HtmlSyntaxAttribute { name: raw_attribute_name, local_name: attribute_local_name, namespace: attribute_namespace, value });
    }
    Ok(HtmlSyntaxElement { local_name, namespace, attributes, children: Vec::new() })
}

fn resolved_namespace(namespace: ResolveResult<'_>, offset: u64) -> Result<Option<String>, XmlParseError> {
    match namespace {
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Bound(namespace) => Ok(Some(utf8_xml_bytes(namespace.as_ref(), offset, "namespace URI")?.to_owned())),
        ResolveResult::Unknown(prefix) => Err(XmlParseError::new(offset, format!("unknown namespace prefix '{}'", String::from_utf8_lossy(&prefix)))),
    }
}

fn resolve_xml_entity(name: &str, xhtml_entities: bool) -> Option<&'static str> {
    quick_xml::escape::resolve_xml_entity(name).or_else(|| xhtml_entities.then(|| quick_xml::escape::resolve_html5_entity(name)).flatten())
}

fn is_xhtml_doctype(doctype: &str) -> bool {
    let doctype = doctype.to_ascii_lowercase();
    doctype.split_ascii_whitespace().next() == Some("html") && (doctype.contains("-//w3c//dtd xhtml") || doctype.contains("xhtml1-") || doctype.contains("xhtml11"))
}

fn push_xml_node(nodes: &mut Vec<HtmlSyntaxNode>, elements: &mut [HtmlSyntaxElement], node: HtmlSyntaxNode) {
    if let Some(parent) = elements.last_mut() {
        parent.children.push(node);
    } else {
        nodes.push(node);
    }
}

fn push_xml_text(elements: &mut [HtmlSyntaxElement], text: &str) {
    let Some(parent) = elements.last_mut() else { return };
    if let Some(HtmlSyntaxNode::Text(existing)) = parent.children.last_mut() {
        existing.push_str(text);
    } else if !text.is_empty() {
        parent.children.push(HtmlSyntaxNode::Text(text.to_owned()));
    }
}

fn utf8_xml_bytes<'a>(bytes: &'a [u8], offset: u64, context: &str) -> Result<&'a str, XmlParseError> {
    std::str::from_utf8(bytes).map_err(|error| XmlParseError::new(offset, format!("invalid UTF-8 in {context}: {error}")))
}

fn xml_reader_error(reader: &NsReader<&[u8]>, error: impl fmt::Display) -> XmlParseError {
    XmlParseError::new(reader.error_position(), error.to_string())
}

/// Parse HTML directly into the renderer's framework-neutral DOM.
pub fn parse_dom_document(html: &str) -> Result<Document, DocumentBuildError> {
    build_dom_document(&parse_html_document(html))
}

/// Convert an already parsed scraper tree into the renderer's DOM.
/// Scraper types do not escape beyond this crate boundary.
pub fn build_dom_document(parsed: &ParsedHtml) -> Result<Document, DocumentBuildError> {
    if let ParsedMarkup::Xml(tree) = &parsed.document {
        return build_dom_from_syntax_tree(tree, DocumentMode::Xml);
    }
    let ParsedMarkup::Html(parsed_document) = &parsed.document else { unreachable!("XML documents returned above") };
    let has_explicit_doctype = parsed_document.tree.root().children().any(|node| matches!(node.value(), Node::Doctype(_)));
    (!has_explicit_doctype || parsed.quirks_mode() == HtmlQuirksMode::NoQuirks).then_some(()).expect("explicit quirks and limited-quirks HTML doctypes are unsupported");
    let mut document = DocumentBuilder::new();
    document.set_mode(DocumentMode::Html);
    if let Some(root) = select_document_root(parsed_document) {
        let root_data = build_dom_element_data(&mut document, &root);
        let (root_node, mut document) = document.append_dom_element(None, root_data)?;
        register_heading_toc(&mut document, root_node);
        build_dom_children(&mut document, root, root_node)?;
        return document.set_dom_root(root_node)?.finish();
    }
    document.finish_without_root()
}

fn build_dom_from_syntax_tree(tree: &HtmlSyntaxTree, mode: DocumentMode) -> Result<Document, DocumentBuildError> {
    let Some(root) = tree.nodes.iter().find_map(|node| match node {
        HtmlSyntaxNode::Element(element) => Some(element),
        _ => None,
    }) else {
        return DocumentBuilder::new().finish_without_root();
    };

    let mut document = DocumentBuilder::new();
    document.set_mode(mode);
    let root_data = build_dom_syntax_element_data(&mut document, root);
    let (root_node, mut document) = document.append_dom_element(None, root_data)?;
    build_dom_syntax_children(&mut document, &root.children, root_node)?;
    register_heading_toc(&mut document, root_node);
    document.set_dom_root(root_node)?.finish()
}

fn build_dom_syntax_children(document: &mut DocumentBuilder<Rootless>, children: &[HtmlSyntaxNode], parent: DomNodeId) -> Result<(), DocumentBuildError> {
    for child in children {
        match child {
            HtmlSyntaxNode::Element(element) => {
                let data = build_dom_syntax_element_data(document, element);
                let node = document.append_dom_element(Some(parent), data)?;
                build_dom_syntax_children(document, &element.children, node)?;
                register_heading_toc(document, node);
            }
            HtmlSyntaxNode::Text(text) if !text.is_empty() => {
                document.append_dom_text(parent, text.clone())?;
            }
            HtmlSyntaxNode::TemplateContents(children) => build_dom_syntax_children(document, children, parent)?,
            HtmlSyntaxNode::Doctype { .. } | HtmlSyntaxNode::Comment(_) | HtmlSyntaxNode::ProcessingInstruction { .. } | HtmlSyntaxNode::Text(_) => {}
        }
    }
    Ok(())
}

fn build_dom_syntax_element_data<State>(document: &mut DocumentBuilder<State>, element: &HtmlSyntaxElement) -> DomElementData {
    let tag = document.intern_string(&element.local_name);
    let namespace = element.namespace.as_deref().map(|namespace| document.intern_string(namespace));
    let attribute_value = |name: &str| element.attributes.iter().find(|attribute| attribute.name == name).map(|attribute| attribute.value.as_str());
    let id = attribute_value("id").map(|value| document.intern_string(value));
    let classes = document.intern_classes(attribute_value("class"));
    let source = attribute_value("src").or_else(|| attribute_value("href")).or_else(|| attribute_value("xlink:href"));
    let width = attribute_value("width").and_then(|value| value.parse().ok());
    let height = attribute_value("height").and_then(|value| value.parse().ok());
    let is_inline_svg = element.local_name == "svg" && element.namespace.as_deref() == Some("http://www.w3.org/2000/svg");
    let image_idx = if is_inline_svg {
        let bytes: Arc<[u8]> = serialize_svg_syntax_element(element).into_bytes().into();
        Some(document.push_image(ImageResource { source: ImageSource::Inline(bytes), width: 0, height: 0, width_attr: None, height_attr: None }))
    } else if matches!(element.local_name.as_str(), "img" | "image") {
        source.filter(|source| !source.trim().is_empty()).map(|source| document.push_image(ImageResource { source: ImageSource::Uri(source.trim().to_owned()), width: 0, height: 0, width_attr: width, height_attr: height }))
    } else {
        None
    };
    let attributes = element
        .attributes
        .iter()
        .map(|attribute| DomAttribute {
            name: document.intern_string(&attribute.name),
            local_name: document.intern_string(&attribute.local_name),
            namespace: attribute.namespace.as_deref().map(|namespace| document.intern_string(namespace)),
            value: document.intern_string(&attribute.value),
        })
        .collect();
    DomElementData { tag, namespace, attributes, attribute_cache: DomElementAttributeCache::new(id, classes), image_idx }
}

fn serialize_svg_syntax_element(element: &HtmlSyntaxElement) -> String {
    fn push_node(output: &mut String, node: &HtmlSyntaxNode) {
        match node {
            HtmlSyntaxNode::Element(element) => push_element(output, element),
            HtmlSyntaxNode::Text(text) => output.push_str(&quick_xml::escape::escape(text)),
            HtmlSyntaxNode::Comment(comment) => {
                output.push_str("<!--");
                output.push_str(comment);
                output.push_str("-->");
            }
            HtmlSyntaxNode::ProcessingInstruction { target, data } => {
                output.push_str("<?");
                output.push_str(target);
                output.push(' ');
                output.push_str(data);
                output.push_str("?>");
            }
            HtmlSyntaxNode::TemplateContents(children) => children.iter().for_each(|child| push_node(output, child)),
            HtmlSyntaxNode::Doctype { .. } => {}
        }
    }
    fn push_element(output: &mut String, element: &HtmlSyntaxElement) {
        output.push('<');
        output.push_str(&element.local_name);
        if element.local_name == "svg" && !element.attributes.iter().any(|attribute| attribute.name == "xmlns") {
            output.push_str(" xmlns=\"http://www.w3.org/2000/svg\"");
        }
        for attribute in &element.attributes {
            output.push(' ');
            output.push_str(&attribute.name);
            output.push_str("=\"");
            output.push_str(&quick_xml::escape::escape(&attribute.value));
            output.push('"');
        }
        output.push('>');
        element.children.iter().for_each(|child| push_node(output, child));
        output.push_str("</");
        output.push_str(&element.local_name);
        output.push('>');
    }

    let mut output = String::new();
    push_element(&mut output, element);
    output
}

fn nonempty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn head_metadata_from_syntax_tree(tree: &HtmlSyntaxTree) -> HeadMetadata {
    let mut metadata = HeadMetadata::default();
    collect_syntax_metadata(&tree.nodes, false, &mut metadata);
    metadata
}

fn collect_syntax_metadata(nodes: &[HtmlSyntaxNode], inside_head: bool, metadata: &mut HeadMetadata) {
    for node in nodes {
        let HtmlSyntaxNode::Element(element) = node else { continue };
        let inside_head = inside_head || element.local_name == "head";
        if inside_head && element.local_name == "title" && metadata.title.is_none() {
            metadata.title = nonempty(syntax_text_content(&element.children));
        } else if element.local_name == "style" && style_type_is_css(element.attributes.iter().find(|attribute| attribute.name == "type").map(|attribute| attribute.value.as_str())) {
            if let Some(css) = nonempty(syntax_text_content(&element.children)) {
                metadata.stylesheets.push(StylesheetReference::Inline(css));
            }
        } else if element.local_name == "link" {
            let attribute = |name: &str| element.attributes.iter().find(|attribute| attribute.name == name).map(|attribute| attribute.value.as_str());
            if link_is_active_stylesheet(attribute("rel"))
                && link_type_is_css(attribute("type"))
                && let Some(href) = attribute("href").and_then(|href| nonempty(href.to_owned()))
            {
                match attribute("charset").and_then(|charset| nonempty(charset.to_owned())) {
                    Some(charset) => metadata.stylesheets.push(StylesheetReference::ExternalWithCharset { href, charset }),
                    None => metadata.stylesheets.push(StylesheetReference::External(href)),
                }
            }
        }
        collect_syntax_metadata(&element.children, inside_head, metadata);
    }
}

fn syntax_text_content(nodes: &[HtmlSyntaxNode]) -> String {
    let mut content = String::new();
    for node in nodes {
        match node {
            HtmlSyntaxNode::Text(text) => content.push_str(text),
            HtmlSyntaxNode::Element(element) => content.push_str(&syntax_text_content(&element.children)),
            HtmlSyntaxNode::TemplateContents(children) => content.push_str(&syntax_text_content(children)),
            HtmlSyntaxNode::Doctype { .. } | HtmlSyntaxNode::Comment(_) | HtmlSyntaxNode::ProcessingInstruction { .. } => {}
        }
    }
    content
}

fn matches_ignore_ascii_case(value: &str, candidates: &[&str]) -> bool {
    candidates.iter().any(|candidate| value.eq_ignore_ascii_case(candidate))
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

fn select_document_root<'a>(document: &'a Html) -> Option<ElementRef<'a>> {
    for selector in ["html", "body", "svg"] {
        let selector = scraper::Selector::parse(selector).ok()?;
        if let Some(root) = document.select(&selector).next() {
            return Some(root);
        }
    }
    None
}

fn build_dom_element(document: &mut DocumentBuilder<Rootless>, element: ElementRef<'_>, parent: Option<DomNodeId>) -> Result<DomNodeId, DocumentBuildError> {
    let data = build_dom_element_data(document, &element);
    let node = document.append_dom_element(parent, data)?;
    for child in element.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(child) = ElementRef::wrap(child) {
                    build_dom_element(document, child, Some(node))?;
                }
            }
            Node::Text(text) if !text.is_empty() => {
                document.append_dom_text(node, text.to_string())?;
            }
            _ => {}
        }
    }
    register_heading_toc(document, node);
    Ok(node)
}

fn build_dom_children(document: &mut DocumentBuilder<Rootless>, element: ElementRef<'_>, parent: DomNodeId) -> Result<(), DocumentBuildError> {
    for child in element.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(child) = ElementRef::wrap(child) {
                    build_dom_element(document, child, Some(parent))?;
                }
            }
            Node::Text(text) if !text.is_empty() => {
                document.append_dom_text(parent, text.to_string())?;
            }
            _ => {}
        }
    }

    Ok(())
}

fn build_dom_element_data<State>(document: &mut DocumentBuilder<State>, element: &ElementRef<'_>) -> DomElementData {
    let tag_name = element.value().name();
    let tag = document.intern_string(tag_name);
    let namespace = (!element.value().name.ns.is_empty()).then(|| document.intern_string(element.value().name.ns.as_ref()));
    let id = element.value().attr("id").map(|value| document.intern_string(value));
    let classes = document.intern_classes(element.value().attr("class"));
    let source = element.value().attr("src").or_else(|| element.value().attr("href")).or_else(|| element.value().attr("xlink:href"));
    let width = element.value().attr("width").and_then(|value| value.parse().ok());
    let height = element.value().attr("height").and_then(|value| value.parse().ok());
    let is_inline_svg = tag_name == "svg" && element.value().name.ns.as_ref() == "http://www.w3.org/2000/svg";
    let image_idx = if is_inline_svg {
        let bytes: Arc<[u8]> = element.html().into_bytes().into();
        Some(document.push_image(ImageResource { source: ImageSource::Inline(bytes), width: 0, height: 0, width_attr: None, height_attr: None }))
    } else if matches!(tag_name, "img" | "image") {
        source.filter(|source| !source.trim().is_empty()).map(|source| document.push_image(ImageResource { source: ImageSource::Uri(source.trim().to_owned()), width: 0, height: 0, width_attr: width, height_attr: height }))
    } else {
        None
    };
    let attributes = element
        .value()
        .attrs
        .iter()
        .map(|(key, value)| {
            let name = key.prefix.as_ref().map(|prefix| format!("{}:{}", prefix.as_ref(), key.local.as_ref())).unwrap_or_else(|| key.local.as_ref().to_owned());
            DomAttribute {
                name: document.intern_string(&name),
                local_name: document.intern_string(key.local.as_ref()),
                namespace: (!key.ns.is_empty()).then(|| document.intern_string(key.ns.as_ref())),
                value: document.intern_string(value.as_ref()),
            }
        })
        .collect();
    DomElementData { tag, namespace, attributes, attribute_cache: DomElementAttributeCache::new(id, classes), image_idx }
}

fn register_heading_toc(document: &mut DocumentBuilder<Rootless>, node: DomNodeId) {
    let Some(element) = document.document().element_ref(node) else { return };
    let Some(level) = (match element.tag() {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }) else {
        return;
    };
    let existing_id = element.id().map(str::to_owned);
    let children: Vec<_> = element.children().collect();
    let mut title = String::new();
    for child in children {
        collect_text(document.document(), child, &mut title);
    }
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return;
    }
    let id = existing_id.unwrap_or_else(|| assign_generated_id(document, node));
    let title = document.intern_string(&title);
    let href = document.intern_string(&format!("#{id}"));
    document.push_toc_entry(DocumentTocEntry { level, title, href, node_idx: node.raw() });
}

fn collect_text(document: &Document, node: DomNodeId, output: &mut String) {
    match document.node_ref(node) {
        Some(NodeRef::Text(text)) => {
            if !output.is_empty() {
                output.push(' ');
            }
            output.push_str(text.text());
        }
        Some(NodeRef::Element(element)) => {
            for child in element.children() {
                collect_text(document, child, output);
            }
        }
        None => {}
    }
}

fn assign_generated_id<State>(document: &mut DocumentBuilder<State>, node: DomNodeId) -> String {
    let base = format!("heading-{}", node.raw());
    let mut candidate = base.clone();
    let mut suffix = 2;
    loop {
        let value = document.intern_string(&candidate);
        if !document.document().dom_id_in_use(value) {
            let name = document.intern_string("id");
            document.set_dom_id_attribute(node, value, name).expect("generated heading IDs should be valid");
            return candidate;
        }
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
}

fn normalize_self_closing_anchor_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < input.len() {
        let Some(rel_start) = input[i..].find("<a") else {
            out.push_str(&input[i..]);
            break;
        };
        let start = i + rel_start;
        out.push_str(&input[i..start]);

        let after_a = start + 2;
        let next = input[after_a..].chars().next();
        if !matches!(next, Some(c) if c.is_ascii_whitespace() || c == '>' || c == '/') {
            out.push_str("<a");
            i = after_a;
            continue;
        }

        let Some(rel_end) = input[after_a..].find('>') else {
            out.push_str(&input[start..]);
            break;
        };
        let end = after_a + rel_end;
        let trimmed = input[after_a..end].trim_end();
        if let Some(without_slash) = trimmed.strip_suffix('/') {
            out.push_str("<a");
            out.push_str(without_slash.trim_end());
            out.push_str("></a>");
        } else {
            out.push_str(&input[start..=end]);
        }
        i = end + 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{HtmlQuirksMode, MarkupSyntax, StylesheetReference, decode_html_bytes, parse_html_document, parse_xml_document, plain_text_from_fragment};
    use html_dom::{Document, DocumentMode, DomNodeId, ElementRef, ImageSource, NodeRef};

    fn child_element<'a>(document: &'a Document, parent: ElementRef<'a>, tag: &str) -> ElementRef<'a> {
        parent.children().find_map(|child| document.element_ref(child).filter(|element| element.tag() == tag)).unwrap_or_else(|| panic!("expected <{tag}> child of <{}>", parent.tag()))
    }

    fn body(document: &Document) -> ElementRef<'_> {
        let html = document.element_ref(document.dom_root().expect("parsed document root")).expect("root element");
        child_element(document, html, "body")
    }

    fn text_content(document: &Document, node: DomNodeId) -> String {
        match document.node_ref(node).expect("valid parsed node") {
            NodeRef::Text(text) => text.text().to_owned(),
            NodeRef::Element(element) => element.children().map(|child| text_content(document, child)).collect(),
        }
    }

    #[test]
    fn selects_xml_syntax_from_xml_family_extensions() {
        assert_eq!(MarkupSyntax::from_uri("chapter.xhtml"), MarkupSyntax::Xml);
        assert_eq!(MarkupSyntax::from_uri("reference.XHT?version=1"), MarkupSyntax::Xml);
        assert_eq!(MarkupSyntax::from_uri("image.svg#root"), MarkupSyntax::Xml);
        assert_eq!(MarkupSyntax::from_uri("chapter.html"), MarkupSyntax::Html);
    }

    #[test]
    fn html_byte_decoding_honors_meta_charset_before_lossy_utf8_conversion() {
        let mut bytes = b"<!doctype html><meta charset=windows-1251><p>".to_vec();
        bytes.push(0xe6);
        bytes.extend_from_slice(b"</p>");
        assert_eq!(decode_html_bytes(&bytes, None), "<!doctype html><meta charset=windows-1251><p>ж</p>");
    }

    #[test]
    fn html_byte_decoding_does_not_treat_comment_contents_as_a_meta_element() {
        let mut bytes = b"<!-- <meta charset=windows-1251> --><p>".to_vec();
        bytes.push(0xe6);
        bytes.extend_from_slice(b"</p>");
        assert!(decode_html_bytes(&bytes, None).contains('�'));
    }

    #[test]
    fn transport_charset_precedes_an_in_document_declaration() {
        let mut bytes = b"<meta charset=windows-1251><p>".to_vec();
        bytes.push(0xe9);
        bytes.extend_from_slice(b"</p>");
        assert!(decode_html_bytes(&bytes, Some("windows-1252")).contains('é'));
    }

    #[test]
    fn parses_xhtml_namespaces_cdata_and_namespaced_attributes() {
        let parsed = parse_xml_document(
            r#"<?xml version="1.0"?>
            <html xmlns="http://www.w3.org/1999/xhtml">
              <head><title>XML title</title><style><![CDATA[p > span { color: green; }]]></style></head>
              <body><svg xmlns="http://www.w3.org/2000/svg"><a xmlns:xlink="http://www.w3.org/1999/xlink" xlink:href="target.svg">link</a></svg></body>
            </html>"#,
        )
        .expect("well-formed XHTML");

        assert_eq!(parsed.head_metadata().title.as_deref(), Some("XML title"));
        assert_eq!(parsed.head_metadata().stylesheets, vec![StylesheetReference::Inline("p > span { color: green; }".to_owned())]);
        let document = parsed.build_dom();
        assert_eq!(document.mode(), DocumentMode::Xml);
        let html = document.element_ref(document.dom_root().unwrap()).unwrap();
        assert_eq!(html.tag(), "html");
        assert_eq!(html.namespace(), Some("http://www.w3.org/1999/xhtml"));
        let body = child_element(&document, html, "body");
        let svg = child_element(&document, body, "svg");
        assert_eq!(svg.namespace(), Some("http://www.w3.org/2000/svg"));
        let link = child_element(&document, svg, "a");
        assert_eq!(link.attr_expanded(Some("http://www.w3.org/1999/xlink"), "href"), Some("target.svg"));
        assert_eq!(text_content(&document, link.node_id()), "link");
    }

    #[test]
    fn registers_inline_svg_as_a_self_contained_image_resource() {
        let parsed = parse_xml_document(r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:svg="http://www.w3.org/2000/svg"><body><svg:svg height="50%"><svg:rect width="20" height="10" fill="blue"/></svg:svg></body></html>"#)
            .expect("well-formed XHTML with SVG");
        let document = parsed.build_dom();
        let svg = child_element(&document, body(&document), "svg");
        let image_idx = svg.image_idx().expect("inline SVG is replaced content");
        let image = document.image(image_idx).expect("registered SVG image");
        let ImageSource::Inline(bytes) = &image.source else { panic!("inline SVG must retain its serialized bytes") };
        let serialized = std::str::from_utf8(bytes).expect("serialized SVG is UTF-8");
        assert!(serialized.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(serialized.contains("<rect width=\"20\" height=\"10\" fill=\"blue\"></rect>"));
        assert_eq!(image.width_attr, None);
        assert_eq!(image.height_attr, None);
    }

    #[test]
    fn xml_parser_rejects_html_recovery_and_unknown_prefixes() {
        assert!(parse_xml_document("<html><p></html>").is_err());
        assert!(parse_xml_document("<one/><two/>").is_err());
        assert!(parse_xml_document("<root><x:item/></root>").is_err());
        assert!(parse_xml_document(" \n<?xml version=\"1.0\"?><root/>").is_err());
        assert!(parse_xml_document("<?xml version=\"2.0\"?><root/>").is_err());
        assert!(parse_xml_document("<root xmlns:a=\"urn:same\" xmlns:b=\"urn:same\" a:value=\"1\" b:value=\"2\"/>").is_err());
    }

    #[test]
    fn xml_names_and_metadata_are_case_sensitive() {
        let parsed = parse_xml_document(r#"<HTML xmlns="http://www.w3.org/1999/xhtml"><HEAD><TITLE>not an XHTML title</TITLE></HEAD><Body/></HTML>"#).unwrap();
        assert_eq!(parsed.head_metadata().title, None);
        let document = parsed.build_dom();
        let root = document.element_ref(document.dom_root().unwrap()).unwrap();
        assert_eq!(root.tag(), "HTML");
        assert_eq!(child_element(&document, root, "HEAD").tag(), "HEAD");
    }

    #[test]
    fn xml_end_tags_allow_spec_permitted_trailing_whitespace() {
        parse_xml_document("<root><child></child\n ></root\t>").expect("XML permits whitespace before an end tag's closing delimiter");
    }

    #[test]
    fn resolves_bounded_xhtml_entities_only_for_xhtml_doctypes() {
        let parsed = parse_xml_document(r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"><html xmlns="http://www.w3.org/1999/xhtml"><body>&nbsp;&copy;</body></html>"#).unwrap();
        let document = parsed.build_dom();
        let html = document.element_ref(document.dom_root().unwrap()).unwrap();
        let body = child_element(&document, html, "body");
        assert_eq!(text_content(&document, body.node_id()), "\u{a0}©");
        assert!(parse_xml_document("<root>&nbsp;</root>").is_err());
    }

    #[test]
    fn normalizes_self_closing_anchors() {
        let document = parse_html_document("<!doctype html><html><body><a id=\"target\"/><p>x</p></body></html>").build_dom();
        assert_eq!(document.nodes().filter(|(_, node)| matches!(node, NodeRef::Element(element) if element.tag() == "a" && element.id() == Some("target"))).count(), 1);
    }

    #[test]
    fn renderer_dom_accepts_documents_without_an_explicit_doctype() {
        let parsed = parse_html_document("<p>implicit standards mode</p>");
        assert_eq!(parsed.quirks_mode(), HtmlQuirksMode::Quirks, "the HTML parser still reports the specification-defined mode");
        let document = parsed.build_dom();
        assert_eq!(document.mode(), DocumentMode::Html);
    }

    #[test]
    #[should_panic(expected = "explicit quirks and limited-quirks HTML doctypes are unsupported")]
    fn renderer_dom_rejects_an_explicit_quirks_doctype() {
        let parsed = parse_html_document(r#"<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN"><p>explicit quirks</p>"#);
        assert_eq!(parsed.quirks_mode(), HtmlQuirksMode::Quirks);
        parsed.build_dom();
    }

    #[test]
    #[should_panic(expected = "explicit quirks and limited-quirks HTML doctypes are unsupported")]
    fn renderer_dom_rejects_an_explicit_limited_quirks_doctype() {
        let parsed = parse_html_document(r#"<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN" "http://www.w3.org/TR/html4/loose.dtd"><p>explicit limited quirks</p>"#);
        assert_eq!(parsed.quirks_mode(), HtmlQuirksMode::LimitedQuirks);
        parsed.build_dom();
    }

    #[test]
    fn extracts_compact_plain_text_from_html5_fragments() {
        let fragment = r#"<div xmlns="http://www.w3.org/1999/xhtml"><p>&alpha; &copy; one <em>two</em></p><p>three<br>four</p><script>ignored()</script>"#;
        assert_eq!(plain_text_from_fragment(fragment), "α © one two three four");
    }

    #[test]
    fn extracts_head_metadata_without_exposing_scraper() {
        let document = parse_html_document(
            r#"
            <html><head>
                <title> Example </title>
                <link rel="preload StyleSheet" type="Text/CSS; charset=utf-8" href="base.css?v=1">
                <link rel="alternate stylesheet" href="inactive.css">
                <style>p { color: blue; }</style>
                <style type="TeXt/CsS">p { background: green; }</style>
                <style type="text/cſs">p { color: red; }</style>
            </head><body></body></html>
        "#,
        );
        let metadata = document.head_metadata();
        assert_eq!(metadata.title.as_deref(), Some("Example"));
        assert_eq!(metadata.stylesheets, vec![StylesheetReference::External("base.css?v=1".to_owned()), StylesheetReference::Inline("p { color: blue; }".to_owned()), StylesheetReference::Inline("p { background: green; }".to_owned()),]);
    }

    #[test]
    fn wpt_block_start_tag_implicitly_closes_open_paragraph() {
        // Adapted from WPT html/syntax/parsing/resources/blocks.dat:
        // https://github.com/web-platform-tests/wpt/blob/master/html/syntax/parsing/resources/blocks.dat
        let document = parse_html_document("<!doctype html><p>foo<div>bar<p>baz").build_dom();
        let body = body(&document);
        let children: Vec<_> = body.children().collect();

        assert_eq!(children.len(), 2);
        assert_eq!(document.get_dom_tag(children[0]), Some("p"));
        assert_eq!(text_content(&document, children[0]), "foo");
        assert_eq!(document.get_dom_tag(children[1]), Some("div"));

        let div = document.element_ref(children[1]).expect("div element");
        let div_children: Vec<_> = div.children().collect();
        assert_eq!(div_children.len(), 2);
        assert!(matches!(document.node_ref(div_children[0]), Some(NodeRef::Text(text)) if text.text() == "bar"));
        assert_eq!(document.get_dom_tag(div_children[1]), Some("p"));
        assert_eq!(text_content(&document, div_children[1]), "baz");
    }

    #[test]
    fn wpt_adoption_agency_reparents_misnested_anchor_content() {
        // Adapted from WPT html/syntax/parsing/resources/adoption01.dat:
        // https://github.com/web-platform-tests/wpt/blob/master/html/syntax/parsing/resources/adoption01.dat
        let document = parse_html_document("<!doctype html><a>1<p>2</a>3</p>").build_dom();
        let body = body(&document);
        let children: Vec<_> = body.children().collect();

        assert_eq!(children.len(), 2);
        assert_eq!(document.get_dom_tag(children[0]), Some("a"));
        assert_eq!(text_content(&document, children[0]), "1");
        assert_eq!(document.get_dom_tag(children[1]), Some("p"));

        let paragraph = document.element_ref(children[1]).expect("paragraph element");
        let paragraph_children: Vec<_> = paragraph.children().collect();
        assert_eq!(paragraph_children.len(), 2);
        assert_eq!(document.get_dom_tag(paragraph_children[0]), Some("a"));
        assert_eq!(text_content(&document, paragraph_children[0]), "2");
        assert!(matches!(document.node_ref(paragraph_children[1]), Some(NodeRef::Text(text)) if text.text() == "3"));
    }

    #[test]
    fn wpt_character_references_follow_html_tokenization_rules() {
        // Representative cases adapted from WPT
        // html/syntax/parsing/resources/entities01.dat:
        // https://github.com/web-platform-tests/wpt/blob/master/html/syntax/parsing/resources/entities01.dat
        let cases = [("FOO&gt;BAR", "FOO>BAR"), ("I'm &notin; I tell you", "I'm ∉ I tell you"), ("FOO&#x41;BAR", "FOOABAR"), ("FOO&#x0080;ZOO", "FOO€ZOO"), ("FOO&#xD800;ZOO", "FOO�ZOO"), ("&ammmp;", "&ammmp;")];

        for (input, expected) in cases {
            let document = parse_html_document(&format!("<!doctype html>{input}")).build_dom();
            assert_eq!(text_content(&document, body(&document).node_id()), expected, "input {input:?}");
        }
    }
}
