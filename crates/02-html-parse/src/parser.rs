use crate::metadata::{document_stylesheets, nonempty};
use crate::{XmlParseError, parse_xml_document};
use html_dom as dom_values;
use html_dom::{Document, DocumentBuildError, media_type_essence, uri_path};

pub struct ParsedHtml {
    pub(crate) document: Result<Document, DocumentBuildError>,
    pub(crate) quirks_mode: HtmlQuirksMode,
    pub(crate) errors: Vec<String>,
    pub(crate) source_encoding: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MarkupSyntax {
    #[default]
    Html,
    /// Prefer XML semantics, recovering malformed XHTML with the HTML parser.
    Xhtml,
    Xml,
}

impl MarkupSyntax {
    /// Declared markup media types take precedence over the resource filename.
    /// Missing or unrecognized types fall back to the URI extension.
    pub fn from_resource(uri: &str, media_type: Option<&str>) -> Self {
        match media_type.map(media_type_essence) {
            Some(value) if value.eq_ignore_ascii_case("application/xhtml+xml") => Self::Xhtml,
            Some(value)
                if ["application/xml", "text/xml"]
                    .iter()
                    .any(|candidate| value.eq_ignore_ascii_case(candidate)) =>
            {
                Self::Xml
            }
            Some(value) if value.eq_ignore_ascii_case("text/html") => Self::Html,
            _ => Self::from_uri(uri),
        }
    }

    pub fn from_uri(uri: &str) -> Self {
        let path = uri_path(uri);
        match path.rsplit_once('.').map(|(_, extension)| extension) {
            Some(extension)
                if ["xht", "xhtml"]
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate)) =>
            {
                Self::Xhtml
            }
            Some(extension)
                if ["xml", "svg"]
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
pub enum HtmlQuirksMode {
    NoQuirks,
    LimitedQuirks,
    Quirks,
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

/// Parse HTML into our DOM, retaining parse diagnostics and source metadata.
pub fn parse_html_document(html: &str) -> ParsedHtml {
    parse_html_document_with_options(html, HtmlParserOptions::default())
}

pub fn parse_html_document_with_options(html: &str, options: HtmlParserOptions) -> ParsedHtml {
    crate::html::parse_document(html, options)
}

pub fn parse_document(source: &str, syntax: MarkupSyntax) -> Result<ParsedHtml, XmlParseError> {
    match syntax {
        MarkupSyntax::Html => Ok(parse_html_document(source)),
        MarkupSyntax::Xhtml => Ok(parse_xml_document(source).unwrap_or_else(|error| {
            let mut parsed = parse_html_document(source);
            parsed
                .errors
                .insert(0, format!("Recovered XHTML using HTML parsing: {error}"));
            parsed
        })),
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

impl ParsedHtml {
    pub fn source_encoding(&self) -> Option<&str> {
        self.source_encoding.as_deref()
    }

    /// Clone the parsed DOM when an additional owned copy is needed.
    pub fn build_dom(&self) -> Document {
        self.build_dom_checked()
            .expect("parser produced an invalid DOM")
    }

    /// Consume the parsed result and return its DOM without copying.
    pub fn into_dom(self) -> Document {
        self.into_dom_checked()
            .expect("parsed markup must form a valid DOM")
    }

    /// Return the DOM or a compact-storage capacity error.
    pub fn into_dom_checked(self) -> Result<Document, DocumentBuildError> {
        self.document
    }

    pub fn build_dom_checked(&self) -> Result<Document, DocumentBuildError> {
        crate::build_dom_document(self)
    }

    pub fn head_metadata(&self) -> HeadMetadata {
        let Ok(document) = &self.document else {
            return HeadMetadata::default();
        };
        HeadMetadata {
            title: document.title().map(str::to_owned),
            stylesheets: document_stylesheets(document)
                .into_iter()
                .map(|(sheet, _)| sheet)
                .collect(),
        }
    }

    /// Read the document title without collecting or copying stylesheet text.
    pub fn title(&self) -> Option<String> {
        self.document.as_ref().ok()?.title().map(str::to_owned)
    }

    pub fn errors(&self) -> impl ExactSizeIterator<Item = &str> {
        self.errors.iter().map(String::as_str)
    }

    pub fn quirks_mode(&self) -> HtmlQuirksMode {
        self.quirks_mode
    }
}
#[cfg(test)]
mod tests {
    use super::{
        HtmlQuirksMode, MarkupSyntax, StylesheetReference, parse_document, parse_html_document,
        parse_xml_document,
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
                    .try_element_ref(child)
                    .filter(|element| element.tag() == tag)
            })
            .unwrap_or_else(|| panic!("expected <{tag}> child of <{}>", parent.tag()))
    }

    fn body(document: &Document) -> ElementRef<'_> {
        let html = document
            .try_element_ref(document.dom_root().expect("parsed document root"))
            .expect("root element");
        child_element(document, html, "body")
    }

    fn text_content(document: &Document, node: DomNodeId) -> String {
        match document.node_ref(node) {
            NodeRef::Text(text) => text.text().to_owned(),
            NodeRef::Element(element) => element
                .children()
                .map(|child| text_content(document, child))
                .collect(),
        }
    }

    #[test]
    fn parsing_headings_preserves_author_ids_without_generating_anchors() {
        let parsed = parse_html_document("<h1>Intro</h1><h2 id='chapter-a'>Chapter A</h2>");
        for document in [parsed.build_dom(), parsed.into_dom()] {
            let body = body(&document);
            assert_eq!(child_element(&document, body, "h1").id(), None);
            assert_eq!(child_element(&document, body, "h2").id(), Some("chapter-a"));
        }
    }

    #[test]
    fn selects_xml_syntax_from_xml_family_extensions() {
        assert_eq!(MarkupSyntax::from_uri("chapter.xhtml"), MarkupSyntax::Xhtml);
        assert_eq!(
            MarkupSyntax::from_uri("reference.XHT?version=1"),
            MarkupSyntax::Xhtml
        );
        assert_eq!(MarkupSyntax::from_uri("image.svg#root"), MarkupSyntax::Xml);
        assert_eq!(MarkupSyntax::from_uri("chapter.html"), MarkupSyntax::Html);
    }

    #[test]
    fn declared_markup_types_override_extensions_and_ignore_parameters() {
        for (uri, media_type, expected) in [
            (
                "chapter.html",
                Some(" Application/XHTML+XML ; charset=utf-8"),
                MarkupSyntax::Xhtml,
            ),
            (
                "chapter.xhtml",
                Some("TEXT/HTML; charset=utf-8"),
                MarkupSyntax::Html,
            ),
            ("chapter.html", Some("text/xml"), MarkupSyntax::Xml),
            ("chapter.xhtml?mode=html#part", None, MarkupSyntax::Xhtml),
            (
                "chapter.xhtml",
                Some("application/octet-stream"),
                MarkupSyntax::Xhtml,
            ),
            ("chapter.html", Some(""), MarkupSyntax::Html),
        ] {
            assert_eq!(
                MarkupSyntax::from_resource(uri, media_type),
                expected,
                "{uri}, {media_type:?}"
            );
        }
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
        let html = document.try_element_ref(document.dom_root().unwrap()).unwrap();
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
        let image_id = svg.image().expect("inline SVG is replaced content");
        let image = document.image(dom_values::image_id_raw(image_id)).expect("registered SVG image");
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
    fn xhtml_recovers_void_tags_and_entities_but_keeps_valid_xml_semantics() {
        let source = "<html xmlns='http://www.w3.org/1999/xhtml'><body><img src='cover.png'><p>Romeo<br>Julieta&nbsp;&copy;</p></body></html>";
        assert!(parse_document(source, MarkupSyntax::Xml).is_err());
        let parsed = parse_document(source, MarkupSyntax::Xhtml).unwrap();
        assert!(parsed.errors().any(|error| error.contains("Recovered XHTML") && error.contains("line 1, column")));
        let document = parsed.build_dom();
        assert_eq!(document.images().len(), 1);
        assert!(text_content(&document, body(&document).node_id()).contains("Julieta\u{a0}©"));

        let source = "<html xmlns='http://www.w3.org/1999/xhtml'><body><p><a id='page'/>After</p><svg xmlns='http://www.w3.org/2000/svg'><rect/></svg></body></html>";
        let parsed = parse_document(source, MarkupSyntax::Xhtml).unwrap();
        assert_eq!(parsed.errors().len(), 0);
        let document = parsed.build_dom();
        assert_eq!(document.mode(), DocumentMode::Xml);
        let paragraph = child_element(&document, body(&document), "p");
        assert!(child_element(&document, paragraph, "a").children().next().is_none());
        let svg = child_element(&document, body(&document), "svg");
        assert_eq!(svg.namespace(), Some("http://www.w3.org/2000/svg"));
    }

    #[test]
    fn xml_errors_report_line_and_unicode_character_column() {
        let source = "<html>\n<body>é</bad>\n</html>";
        let error = parse_xml_document(source).err().unwrap();
        assert_eq!(error.line(), 2);
        let prefix = &source[..error.offset() as usize];
        assert_eq!(error.column(), prefix.rsplit('\n').next().unwrap().chars().count() + 1);
        assert!(error.to_string().contains("line 2, column"));
        assert!(error.message().contains("bad"));
    }

    #[test]
    fn xml_names_and_metadata_are_case_sensitive() {
        let parsed = parse_xml_document(r#"<HTML xmlns="http://www.w3.org/1999/xhtml"><HEAD><TITLE>not an XHTML title</TITLE></HEAD><Body/></HTML>"#).unwrap();
        assert_eq!(parsed.head_metadata().title, None);
        assert_eq!(parsed.title(), None);
        let document = parsed.build_dom();
        let root = document.try_element_ref(document.dom_root().unwrap()).unwrap();
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
        let html = document.try_element_ref(document.dom_root().unwrap()).unwrap();
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
    fn title_query_skips_empty_titles_and_ignores_titles_outside_head() {
        let parsed = parse_xml_document(
            "<html><head><style>p { color: blue }</style><title> </title><title> Selected <em>title</em> </title></head><body><title>Ignored</title></body></html>",
        ).unwrap();
        assert_eq!(parsed.title().as_deref(), Some("Selected title"));
        assert_eq!(parsed.title(), parsed.head_metadata().title);
        let parsed = parse_xml_document(
            "<html><head><style>p { color: blue }</style></head><body><title>Ignored</title></body></html>",
        ).unwrap();
        assert_eq!(parsed.title(), None);
    }

    #[test]
    fn extracts_head_metadata_without_exposing_scraper() {
        let document = parse_html_document(
            r#"
            <html><head>
                <title> Example </title>
                <link rel="preload StyleSheet" type="Text/CSS; charset=utf-8" href="base.css?v=1">
                <link rel="alternate stylesheet" href="inactive.css">
                <link rel="stylesheet" type="text/plain" href="ignored.css">
                <link rel="stylesheet" href=" ">
                <link rel="stylesheet" href=" legacy.css " charset=" windows-1252 ">
                <style>p { color: blue; }</style>
                <style type="TeXt/CsS">p { background: green; }</style>
                <style type="text/cſs">p { color: red; }</style>
            </head><body></body></html>
        "#,
        );
        let metadata = document.head_metadata();
        assert_eq!(metadata.title.as_deref(), Some("Example"));
        assert_eq!(document.title(), metadata.title);
        assert_eq!(
            metadata.stylesheets,
            vec![
                StylesheetReference::External("base.css?v=1".to_owned()),
                StylesheetReference::ExternalWithCharset {
                    href: "legacy.css".to_owned(),
                    charset: "windows-1252".to_owned(),
                },
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
        assert_eq!(document.try_element_ref(children[0]).map(|element| element.tag()), Some("p"));
        assert_eq!(text_content(&document, children[0]), "foo");
        assert_eq!(document.try_element_ref(children[1]).map(|element| element.tag()), Some("div"));

        let div = document.try_element_ref(children[1]).expect("div element");
        let div_children: Vec<_> = div.children().collect();
        assert_eq!(div_children.len(), 2);
        assert!(
            matches!(document.node_ref(div_children[0]), NodeRef::Text(text) if text.text() == "bar")
        );
        assert_eq!(document.try_element_ref(div_children[1]).map(|element| element.tag()), Some("p"));
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
        assert_eq!(document.try_element_ref(children[0]).map(|element| element.tag()), Some("a"));
        assert_eq!(text_content(&document, children[0]), "1");
        assert_eq!(document.try_element_ref(children[1]).map(|element| element.tag()), Some("p"));

        let paragraph = document
            .try_element_ref(children[1])
            .expect("paragraph element");
        let paragraph_children: Vec<_> = paragraph.children().collect();
        assert_eq!(paragraph_children.len(), 2);
        assert_eq!(document.try_element_ref(paragraph_children[0]).map(|element| element.tag()), Some("a"));
        assert_eq!(text_content(&document, paragraph_children[0]), "2");
        assert!(
            matches!(document.node_ref(paragraph_children[1]), NodeRef::Text(text) if text.text() == "3")
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

/// Declared media type takes precedence over the resource filename.
pub fn markup_syntax_for_resource(
    provider: &dyn html_dom::ResourceProvider,
    uri: &str,
) -> MarkupSyntax {
    let media_type = provider
        .metadata(uri)
        .ok()
        .and_then(|metadata| metadata.media_type);
    MarkupSyntax::from_resource(uri, media_type.as_deref())
}
