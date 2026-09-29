use html_dom::{Document, DocumentBuildError};
use html5ever::driver::{self, ParseOpts};
use html5ever::tendril::TendrilSink;
use html5ever::tree_builder::{QuirksMode as Html5everQuirksMode, TreeBuilderOpts};
use html5ever::{LocalName, Namespace, QualName};
use scraper::{Html, Node};

use crate::metadata::{head_metadata_from_syntax_tree, nonempty};
use crate::{XmlParseError, parse_xml_document};

pub struct ParsedHtml {
    pub(crate) document: HtmlSyntaxTree,
    pub(crate) syntax: MarkupSyntax,
    pub(crate) quirks_mode: HtmlQuirksMode,
    pub(crate) errors: Vec<String>,
    pub(crate) source_encoding: Option<String>,
}

pub struct ParsedHtmlFragment {
    document: HtmlSyntaxTree,
    errors: Vec<String>,
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
            Some(extension)
                if ["xht", "xhtml", "xml", "svg"]
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate)) =>
            {
                Self::Xml
            }
            _ => Self::Html,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HtmlScriptingMode {
    #[default]
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HtmlParserOptions {
    pub scripting: HtmlScriptingMode,
    pub exact_errors: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtmlNamespace {
    Html,
    MathMl,
    Svg,
}

impl HtmlNamespace {
    fn uri(self) -> &'static str {
        match self {
            Self::Html => "http://www.w3.org/1999/xhtml",
            Self::MathMl => "http://www.w3.org/1998/Math/MathML",
            Self::Svg => "http://www.w3.org/2000/svg",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HtmlFragmentContext {
    pub namespace: HtmlNamespace,
    pub local_name: String,
}

impl HtmlFragmentContext {
    pub fn html(local_name: impl Into<String>) -> Self {
        Self {
            namespace: HtmlNamespace::Html,
            local_name: local_name.into(),
        }
    }

    pub fn svg(local_name: impl Into<String>) -> Self {
        Self {
            namespace: HtmlNamespace::Svg,
            local_name: local_name.into(),
        }
    }

    pub fn math_ml(local_name: impl Into<String>) -> Self {
        Self {
            namespace: HtmlNamespace::MathMl,
            local_name: local_name.into(),
        }
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
    Doctype {
        name: String,
        public_id: String,
        system_id: String,
    },
    Comment(String),
    Text(String),
    Element(HtmlSyntaxElement),
    TemplateContents(Vec<HtmlSyntaxNode>),
    ProcessingInstruction {
        target: String,
        data: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HtmlSyntaxElement {
    pub local_name: String,
    pub namespace: Option<String>,
    pub attributes: Vec<HtmlSyntaxAttribute>,
    pub children: Vec<HtmlSyntaxNode>,
}

impl HtmlSyntaxElement {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .map(|attribute| attribute.value.as_str())
    }
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

pub fn parse_html_document_with_options(html: &str, options: HtmlParserOptions) -> ParsedHtml {
    let parser = driver::parse_document(
        scraper::HtmlTreeSink::new(Html::new_document()),
        html5ever_options(options),
    );
    let document = parser.one(html);
    let errors = document.errors.iter().map(ToString::to_string).collect();
    let quirks_mode = map_quirks_mode(document.quirks_mode);
    ParsedHtml {
        document: syntax_tree(&document),
        syntax: MarkupSyntax::Html,
        quirks_mode,
        errors,
        source_encoding: declared_source_encoding(html),
    }
}

pub fn parse_document(source: &str, syntax: MarkupSyntax) -> Result<ParsedHtml, XmlParseError> {
    match syntax {
        MarkupSyntax::Html => Ok(parse_html_document(source)),
        MarkupSyntax::Xml => parse_xml_document(source),
    }
}

pub(crate) fn declared_source_encoding(source: &str) -> Option<String> {
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
            let end = value
                .find(|character: char| {
                    character.is_ascii_whitespace() || character == '>' || character == ';'
                })
                .unwrap_or(value.len());
            return nonempty(value[..end].to_owned());
        }
    }
    None
}

pub fn parse_html_fragment(
    html: &str,
    context: &HtmlFragmentContext,
    options: HtmlParserOptions,
) -> ParsedHtmlFragment {
    let context_name = QualName::new(
        None,
        context_namespace(context.namespace),
        LocalName::from(context.local_name.as_str()),
    );
    let parser = driver::parse_fragment(
        scraper::HtmlTreeSink::new(Html::new_fragment()),
        html5ever_options(options),
        context_name,
        Vec::new(),
        options.scripting == HtmlScriptingMode::Enabled,
    );
    let fragment = parser.one(html);
    let errors = fragment.errors.iter().map(ToString::to_string).collect();
    let mut document = syntax_tree(&fragment);
    if document.nodes.len() == 1
        && let HtmlSyntaxNode::Element(root) = &mut document.nodes[0]
        && root.local_name == "html"
        && root.namespace.as_deref() == Some("http://www.w3.org/1999/xhtml")
    {
        document.nodes = std::mem::take(&mut root.children);
    }
    ParsedHtmlFragment { document, errors }
}

impl ParsedHtml {
    pub fn source_encoding(&self) -> Option<&str> {
        self.source_encoding.as_deref()
    }

    pub fn build_dom(&self) -> Document {
        self.build_dom_checked()
            .expect("parser produced an invalid DOM")
    }

    pub fn build_dom_checked(&self) -> Result<Document, DocumentBuildError> {
        crate::build_dom_document(self)
    }

    pub fn head_metadata(&self) -> HeadMetadata {
        head_metadata_from_syntax_tree(&self.document)
    }

    pub fn errors(&self) -> impl ExactSizeIterator<Item = &str> {
        self.errors.iter().map(String::as_str)
    }

    pub fn quirks_mode(&self) -> HtmlQuirksMode {
        self.quirks_mode
    }

    pub fn syntax_tree(&self) -> HtmlSyntaxTree {
        self.document.clone()
    }
}

impl ParsedHtmlFragment {
    pub fn errors(&self) -> impl ExactSizeIterator<Item = &str> {
        self.errors.iter().map(String::as_str)
    }

    pub fn syntax_tree(&self) -> HtmlSyntaxTree {
        self.document.clone()
    }
}

fn html5ever_options(options: HtmlParserOptions) -> ParseOpts {
    ParseOpts {
        tokenizer: html5ever::tokenizer::TokenizerOpts {
            exact_errors: options.exact_errors,
            ..Default::default()
        },
        tree_builder: TreeBuilderOpts {
            exact_errors: options.exact_errors,
            scripting_enabled: options.scripting == HtmlScriptingMode::Enabled,
            ..Default::default()
        },
    }
}

fn context_namespace(namespace: HtmlNamespace) -> Namespace {
    Namespace::from(namespace.uri())
}

fn map_quirks_mode(mode: Html5everQuirksMode) -> HtmlQuirksMode {
    match mode {
        Html5everQuirksMode::NoQuirks => HtmlQuirksMode::NoQuirks,
        Html5everQuirksMode::LimitedQuirks => HtmlQuirksMode::LimitedQuirks,
        Html5everQuirksMode::Quirks => HtmlQuirksMode::Quirks,
    }
}

fn syntax_tree(html: &Html) -> HtmlSyntaxTree {
    HtmlSyntaxTree {
        nodes: syntax_children(html.tree.root()),
    }
}

fn syntax_children(parent: ego_tree::NodeRef<'_, Node>) -> Vec<HtmlSyntaxNode> {
    parent.children().filter_map(syntax_node).collect()
}

fn syntax_node(node: ego_tree::NodeRef<'_, Node>) -> Option<HtmlSyntaxNode> {
    match node.value() {
        Node::Document => None,
        Node::Fragment => Some(HtmlSyntaxNode::TemplateContents(syntax_children(node))),
        Node::Doctype(doctype) => Some(HtmlSyntaxNode::Doctype {
            name: doctype.name().to_owned(),
            public_id: doctype.public_id().to_owned(),
            system_id: doctype.system_id().to_owned(),
        }),
        Node::Comment(comment) => Some(HtmlSyntaxNode::Comment(comment.to_string())),
        Node::Text(text) => Some(HtmlSyntaxNode::Text(text.to_string())),
        Node::ProcessingInstruction(pi) => Some(HtmlSyntaxNode::ProcessingInstruction {
            target: pi.target.to_string(),
            data: pi.data.to_string(),
        }),
        Node::Element(element) => {
            let attributes = element
                .attrs
                .iter()
                .map(|(name, value)| HtmlSyntaxAttribute {
                    name: name
                        .prefix
                        .as_ref()
                        .map(|prefix| format!("{}:{}", prefix.as_ref(), name.local.as_ref()))
                        .unwrap_or_else(|| name.local.to_string()),
                    local_name: name.local.to_string(),
                    namespace: (!name.ns.is_empty()).then(|| name.ns.to_string()),
                    value: value.to_string(),
                })
                .collect();
            Some(HtmlSyntaxNode::Element(HtmlSyntaxElement {
                local_name: element.name.local.to_string(),
                namespace: (!element.name.ns.is_empty()).then(|| element.name.ns.to_string()),
                attributes,
                children: syntax_children(node),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HtmlQuirksMode, MarkupSyntax, StylesheetReference, parse_html_document, parse_xml_document,
    };
    use crate::{DocumentTextIndex, decode_html_bytes, plain_text_from_fragment};
    use html_dom::{Document, DocumentMode, DomNodeId, ElementRef, ImageSource, NodeRef};

    fn child_element<'a>(
        document: &'a Document,
        parent: ElementRef<'a>,
        tag: &str,
    ) -> ElementRef<'a> {
        parent
            .children()
            .find_map(|child| {
                document
                    .element_ref(child)
                    .filter(|element| element.tag() == tag)
            })
            .unwrap_or_else(|| panic!("expected <{tag}> child of <{}>", parent.tag()))
    }

    fn body(document: &Document) -> ElementRef<'_> {
        let html = document
            .element_ref(document.dom_root().expect("parsed document root"))
            .expect("root element");
        child_element(document, html, "body")
    }

    fn text_content(document: &Document, node: DomNodeId) -> String {
        match document.node_ref(node).expect("valid parsed node") {
            NodeRef::Text(text) => text.text().to_owned(),
            NodeRef::Element(element) => element
                .children()
                .map(|child| text_content(document, child))
                .collect(),
        }
    }

    #[test]
    fn selects_xml_syntax_from_xml_family_extensions() {
        assert_eq!(MarkupSyntax::from_uri("chapter.xhtml"), MarkupSyntax::Xml);
        assert_eq!(
            MarkupSyntax::from_uri("reference.XHT?version=1"),
            MarkupSyntax::Xml
        );
        assert_eq!(MarkupSyntax::from_uri("image.svg#root"), MarkupSyntax::Xml);
        assert_eq!(MarkupSyntax::from_uri("chapter.html"), MarkupSyntax::Html);
    }

    #[test]
    fn html_byte_decoding_honors_meta_charset_before_lossy_utf8_conversion() {
        let mut bytes = b"<!doctype html><meta charset=windows-1251><p>".to_vec();
        bytes.push(0xe6);
        bytes.extend_from_slice(b"</p>");
        assert_eq!(
            decode_html_bytes(&bytes, None),
            "<!doctype html><meta charset=windows-1251><p>ж</p>"
        );
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
        assert_eq!(
            parsed.head_metadata().stylesheets,
            vec![StylesheetReference::Inline(
                "p > span { color: green; }".to_owned()
            )]
        );
        let document = parsed.build_dom();
        assert_eq!(document.mode(), DocumentMode::Xml);
        let html = document.element_ref(document.dom_root().unwrap()).unwrap();
        assert_eq!(html.tag(), "html");
        assert_eq!(html.namespace(), Some("http://www.w3.org/1999/xhtml"));
        let body = child_element(&document, html, "body");
        let svg = child_element(&document, body, "svg");
        assert_eq!(svg.namespace(), Some("http://www.w3.org/2000/svg"));
        let link = child_element(&document, svg, "a");
        assert_eq!(
            link.attr_expanded(Some("http://www.w3.org/1999/xlink"), "href"),
            Some("target.svg")
        );
        assert_eq!(text_content(&document, link.node_id()), "link");
    }

    #[test]
    fn registers_inline_svg_with_serialized_bytes_for_the_resource_pipeline() {
        let parsed = parse_xml_document(r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:svg="http://www.w3.org/2000/svg"><body><svg:svg height="50%"><svg:rect width="20" height="10" fill="blue"/></svg:svg></body></html>"#)
            .expect("well-formed XHTML with SVG");
        let document = parsed.build_dom();
        let svg = child_element(&document, body(&document), "svg");
        let image_idx = svg.image_idx().expect("inline SVG is replaced content");
        let image = document.image(image_idx).expect("registered SVG image");
        let ImageSource::InlineSvg { bytes, base_uri } = &image.source else {
            panic!("inline SVG must retain its serialized bytes")
        };
        assert!(base_uri.is_empty(), "the resource pipeline supplies the effective base URI");
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
        assert!(
            parse_xml_document(
                "<root xmlns:a=\"urn:same\" xmlns:b=\"urn:same\" a:value=\"1\" b:value=\"2\"/>"
            )
            .is_err()
        );
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
        parse_xml_document("<root><child></child\n ></root\t>")
            .expect("XML permits whitespace before an end tag's closing delimiter");
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
    fn html_ignores_the_self_closing_flag_on_anchors() {
        let document = parse_html_document(
            "<!doctype html><html><body><p><a id=\"target\"/>After</p></body></html>",
        )
        .build_dom();
        let paragraph = child_element(&document, body(&document), "p");
        let anchor = child_element(&document, paragraph, "a");

        assert_eq!(anchor.id(), Some("target"));
        assert_eq!(text_content(&document, anchor.node_id()), "After");
    }

    #[test]
    fn renderer_dom_accepts_documents_without_an_explicit_doctype() {
        let parsed = parse_html_document("<p>implicit standards mode</p>");
        assert_eq!(
            parsed.quirks_mode(),
            HtmlQuirksMode::Quirks,
            "the HTML parser still reports the specification-defined mode"
        );
        let document = parsed.build_dom();
        assert_eq!(document.mode(), DocumentMode::Html);
    }

    #[test]
    fn renderer_dom_ignores_an_explicit_quirks_doctype() {
        let parsed = parse_html_document(
            r#"<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN"><p>explicit quirks</p>"#,
        );
        assert_eq!(parsed.quirks_mode(), HtmlQuirksMode::Quirks);
        let document = parsed.build_dom_checked().expect("legacy doctype should build a DOM");
        assert_eq!(document.mode(), DocumentMode::Html);
    }

    #[test]
    fn renderer_dom_ignores_an_explicit_limited_quirks_doctype() {
        let parsed = parse_html_document(
            r#"<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN" "http://www.w3.org/TR/html4/loose.dtd"><p>explicit limited quirks</p>"#,
        );
        assert_eq!(parsed.quirks_mode(), HtmlQuirksMode::LimitedQuirks);
        let document = parsed.build_dom_checked().expect("legacy doctype should build a DOM");
        assert_eq!(document.mode(), DocumentMode::Html);
    }

    #[test]
    fn extracts_compact_plain_text_from_html5_fragments() {
        let fragment = r#"<div xmlns="http://www.w3.org/1999/xhtml"><p>&alpha; &copy; one <em>two</em></p><p>three<br>four</p><script>ignored()</script>"#;
        assert_eq!(plain_text_from_fragment(fragment), "α © one two three four");
    }

    #[test]
    fn document_text_index_maps_collapsed_text_to_dom_positions() {
        let parsed = parse_html_document(
            "<html><body><p>one <em>two</em></p><p>three</p><script>ignored</script></body></html>",
        );
        let index = DocumentTextIndex::from_parsed(&parsed).expect("the parsed DOM is indexable");
        assert_eq!(index.text(), "one two three");

        let two = index
            .source_position_in(4..7)
            .expect("inline text has a source address");
        let three = index
            .source_position_in(8..13)
            .expect("block text has a source address");
        assert_ne!(two.element_steps(), three.element_steps());
        assert_eq!(two.utf16_offset(), 0);
        assert_eq!(three.utf16_offset(), 0);
        assert_eq!(
            index.source_run_count(),
            3,
            "one source run is retained per contiguous text fragment, not per character"
        );
    }

    #[test]
    fn document_text_index_storage_scales_with_runs_not_characters() {
        let body = "word ".repeat(2_500);
        let parsed = parse_html_document(&format!("<html><body><p>{body}</p></body></html>"));
        let index = DocumentTextIndex::from_parsed(&parsed).expect("the parsed DOM is indexable");

        assert_eq!(index.text().chars().count(), 12_499);
        assert_eq!(
            index.source_run_count(),
            2_500,
            "whitespace discontinuities remain independently addressable"
        );
        assert_eq!(
            index.source_path_count(),
            1,
            "all word runs share one interned DOM path"
        );
    }

    #[test]
    fn document_text_index_uses_utf16_offsets_across_cfi_text_chunks() {
        let parsed = parse_html_document("<html><body><p>😀<!-- split -->needle</p></body></html>");
        let index = DocumentTextIndex::from_parsed(&parsed).expect("the parsed DOM is indexable");
        assert_eq!(index.text(), "😀needle");

        let needle = index
            .source_position_in(1..7)
            .expect("the second source node is addressable");
        assert_eq!(
            needle.text_step(),
            1,
            "adjacent character-data nodes share one CFI step"
        );
        assert_eq!(
            needle.utf16_offset(),
            2,
            "the preceding astral character occupies two UTF-16 code units"
        );
    }

    #[test]
    fn document_text_index_excludes_semantic_note_subtrees() {
        let parsed = parse_html_document(
            "<html><body><p>before</p><aside epub:type='footnote'>epub hidden</aside><aside role='doc-endnote'>role hidden</aside><p>after</p></body></html>",
        );
        let index = DocumentTextIndex::from_parsed(&parsed).expect("the parsed DOM is indexable");

        assert_eq!(index.text(), "before after");
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
        assert_eq!(
            metadata.stylesheets,
            vec![
                StylesheetReference::External("base.css?v=1".to_owned()),
                StylesheetReference::Inline("p { color: blue; }".to_owned()),
                StylesheetReference::Inline("p { background: green; }".to_owned()),
            ]
        );
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
        assert!(
            matches!(document.node_ref(div_children[0]), Some(NodeRef::Text(text)) if text.text() == "bar")
        );
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

        let paragraph = document
            .element_ref(children[1])
            .expect("paragraph element");
        let paragraph_children: Vec<_> = paragraph.children().collect();
        assert_eq!(paragraph_children.len(), 2);
        assert_eq!(document.get_dom_tag(paragraph_children[0]), Some("a"));
        assert_eq!(text_content(&document, paragraph_children[0]), "2");
        assert!(
            matches!(document.node_ref(paragraph_children[1]), Some(NodeRef::Text(text)) if text.text() == "3")
        );
    }

    #[test]
    fn wpt_character_references_follow_html_tokenization_rules() {
        // Representative cases adapted from WPT
        // html/syntax/parsing/resources/entities01.dat:
        // https://github.com/web-platform-tests/wpt/blob/master/html/syntax/parsing/resources/entities01.dat
        let cases = [
            ("FOO&gt;BAR", "FOO>BAR"),
            ("I'm &notin; I tell you", "I'm ∉ I tell you"),
            ("FOO&#x41;BAR", "FOOABAR"),
            ("FOO&#x0080;ZOO", "FOO€ZOO"),
            ("FOO&#xD800;ZOO", "FOO�ZOO"),
            ("&ammmp;", "&ammmp;"),
        ];

        for (input, expected) in cases {
            let document = parse_html_document(&format!("<!doctype html>{input}")).build_dom();
            assert_eq!(
                text_content(&document, body(&document).node_id()),
                expected,
                "input {input:?}"
            );
        }
    }
}
