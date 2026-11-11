use crate::dom::{SourceAttribute, build_element_data};
use crate::metadata::document_title;
use crate::{HtmlQuirksMode, ParsedHtml, declared_source_encoding};
use html_dom::{
    Document, DocumentBuildError, DocumentBuilder, DocumentMode, DomElementData, DomNodeId,
    ImageSource, write_escaped_xml};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::mem;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmlParseError {
    offset: u64,
    line: usize,
    column: usize,
    message: String,
}

impl XmlParseError {
    fn new(offset: u64, message: impl Into<String>) -> Self {
        Self {
            offset,
            line: 1,
            column: 1,
            message: message.into(),
        }
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// One-based line and character column in the decoded source.
    pub fn line(&self) -> usize {
        self.line
    }

    pub fn column(&self) -> usize {
        self.column
    }
}

impl fmt::Display for XmlParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "XML parse error at line {}, column {} (byte {}): {}",
            self.line, self.column, self.offset, self.message
        )
    }
}

impl std::error::Error for XmlParseError {}
pub fn parse_xml_document(xml: &str) -> Result<ParsedHtml, XmlParseError> {
    let mut document = parse_xml_dom(xml).map_err(|mut error| {
        let prefix = &xml.as_bytes()[..(error.offset as usize).min(xml.len())];
        error.line = prefix.iter().filter(|&&byte| byte == b'\n').count() + 1;
        let line_start = prefix
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |index| index + 1);
        error.column = prefix[line_start..]
            .iter()
            .filter(|&&byte| byte & 0xc0 != 0x80)
            .count()
            + 1;
        error
    })?;
    document.set_title(document_title(&document));
    Ok(ParsedHtml {
        document: Ok(document),
        quirks_mode: HtmlQuirksMode::NoQuirks,
        errors: Vec::new(),
        source_encoding: declared_source_encoding(xml),
    })
}

fn parse_xml_dom(xml: &str) -> Result<Document, XmlParseError> {
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
    let mut document = DocumentBuilder::new();
    document.set_mode(DocumentMode::Xml);
    let mut root = None;
    let mut elements = Vec::<XmlFrame>::new();
    let mut captures = Vec::<ForeignCapture>::new();
    let mut root_seen = false;
    let mut doctype_seen = false;
    let mut declaration_seen = false;
    let mut content_seen_before_declaration = false;
    let mut xhtml_entities = false;

    loop {
        let event = reader
            .read_event()
            .map_err(|error| xml_reader_error(&reader, error))?;
        if let Event::Start(_) | Event::Empty(_) = &event
            && elements.is_empty()
        {
            if root_seen {
                return Err(XmlParseError::new(
                    reader.buffer_position(),
                    "an XML document must have exactly one root element",
                ));
            }
            root_seen = true;
        }
        let empty_element = matches!(&event, Event::Empty(_));
        let offset = reader.buffer_position();
        let dom_error = |error: DocumentBuildError| XmlParseError::new(offset, error.to_string());
        match event {
            Event::Start(start) | Event::Empty(start) => {
                flush_xml_text(&mut document, &mut elements, &mut captures)
                    .map_err(dom_error)?;
                let tag = xml_start_tag(&reader, &start, version, xhtml_entities)?;
                let data = tag.element_data(&mut document).map_err(dom_error)?;
                let foreign_image = tag.is_foreign_image().then_some(data.image_idx).flatten();
                let node = document.append_dom_element(elements.last().map(|frame| frame.node), data)
                    .map_err(dom_error)?;
                root.get_or_insert(node);
                let depth = elements.len() + 1;
                if !captures.is_empty() {
                    let inherited = elements
                        .last()
                        .and_then(|frame| frame.namespace.as_deref())
                        .unwrap_or("");
                    for capture in &mut captures {
                        tag.write_start_markup(&mut capture.markup, inherited);
                    }
                }
                if let Some(image) = foreign_image {
                    let mut markup = String::new();
                    tag.write_start_markup(&mut markup, "");
                    captures.push(ForeignCapture {
                        depth,
                        image,
                        markup,
                    });
                }
                if empty_element {
                    close_foreign_tag(&tag.local_name, &mut captures);
                    finish_foreign_capture(
                        &mut document,
                        &mut captures,
                        depth,
                    );
                } else {
                    elements.push(XmlFrame {
                        node,
                        name: tag.local_name,
                        namespace: tag.namespace,
                        text: String::new(),
                    });
                }
            }
            Event::End(_) => {
                flush_xml_text(&mut document, &mut elements, &mut captures)
                    .map_err(dom_error)?;
                let Some(frame) = elements.pop() else {
                    return Err(XmlParseError::new(
                        offset,
                        "closing element has no open element",
                    ));
                };
                close_foreign_tag(&frame.name, &mut captures);
                finish_foreign_capture(
                    &mut document,
                    &mut captures,
                    elements.len() + 1,
                );
            }
            Event::Text(text) => {
                let text = text
                    .xml_content(version)
                    .map_err(|error| xml_reader_error(&reader, error))?;
                if elements.is_empty() {
                    if !declaration_seen {
                        content_seen_before_declaration = true;
                    }
                    if !text.trim().is_empty() {
                        return Err(XmlParseError::new(
                            reader.buffer_position(),
                            "character data is not allowed outside the root element",
                        ));
                    }
                } else {
                    elements
                        .last_mut()
                        .expect("XML text parent")
                        .text
                        .push_str(text.as_ref());
                }
            }
            Event::CData(text) => {
                if elements.is_empty() {
                    return Err(XmlParseError::new(
                        reader.buffer_position(),
                        "CDATA is not allowed outside the root element",
                    ));
                }
                let text = text
                    .xml_content(version)
                    .map_err(|error| xml_reader_error(&reader, error))?;
                elements
                    .last_mut()
                    .expect("XML text parent")
                    .text
                    .push_str(text.as_ref());
            }
            Event::GeneralRef(reference) => {
                if elements.is_empty() {
                    return Err(XmlParseError::new(
                        reader.buffer_position(),
                        "entity references are not allowed outside the root element",
                    ));
                }
                let value = if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|error| xml_reader_error(&reader, error))?
                {
                    character.to_string()
                } else {
                    let name = reference
                        .decode()
                        .map_err(|error| xml_reader_error(&reader, error))?;
                    resolve_xml_entity(name.as_ref(), xhtml_entities)
                        .ok_or_else(|| {
                            XmlParseError::new(
                                reader.buffer_position(),
                                format!("undefined entity '&{name};'"),
                            )
                        })?
                        .to_owned()
                };
                elements
                    .last_mut()
                    .expect("XML text parent")
                    .text
                    .push_str(&value);
            }
            Event::Comment(comment) => {
                if !declaration_seen {
                    content_seen_before_declaration = true;
                }
                let comment = comment
                    .xml_content(version)
                    .map_err(|error| xml_reader_error(&reader, error))?
                    .into_owned();
                flush_xml_text(&mut document, &mut elements, &mut captures)
                    .map_err(dom_error)?;
                for capture in &mut captures {
                    capture.markup.push_str("<!--");
                    capture.markup.push_str(&comment);
                    capture.markup.push_str("-->");
                }
            }
            Event::PI(instruction) => {
                if !declaration_seen {
                    content_seen_before_declaration = true;
                }
                let target = utf8_xml_bytes(
                    instruction.target(),
                    reader.buffer_position(),
                    "processing-instruction target",
                )?
                .to_owned();
                let data = utf8_xml_bytes(
                    instruction.content(),
                    reader.buffer_position(),
                    "processing-instruction data",
                )?
                .trim_start()
                .to_owned();
                flush_xml_text(&mut document, &mut elements, &mut captures)
                    .map_err(dom_error)?;
                for capture in &mut captures {
                    capture.markup.push_str("<?");
                    capture.markup.push_str(&target);
                    capture.markup.push(' ');
                    capture.markup.push_str(&data);
                    capture.markup.push_str("?>");
                }
            }
            Event::DocType(doctype) => {
                if !declaration_seen {
                    content_seen_before_declaration = true;
                }
                if !elements.is_empty() || root_seen || doctype_seen {
                    return Err(XmlParseError::new(
                        reader.buffer_position(),
                        "DOCTYPE must occur at most once before the root element",
                    ));
                }
                doctype_seen = true;
                let raw = doctype
                    .xml_content(version)
                    .map_err(|error| xml_reader_error(&reader, error))?
                    .into_owned();
                xhtml_entities = is_xhtml_doctype(&raw);
            }
            Event::Decl(declaration) => {
                if declaration_seen || content_seen_before_declaration || root_seen || doctype_seen
                {
                    return Err(XmlParseError::new(
                        reader.buffer_position(),
                        "the XML declaration must be the first document item",
                    ));
                }
                declaration_seen = true;
                let declared_version = declaration
                    .version()
                    .map_err(|error| xml_reader_error(&reader, error))?;
                version = match declared_version.as_ref() {
                    b"1.0" => XmlVersion::Explicit1_0,
                    b"1.1" => XmlVersion::Explicit1_1,
                    unsupported => {
                        return Err(XmlParseError::new(
                            reader.buffer_position(),
                            format!(
                                "unsupported XML version '{}'",
                                String::from_utf8_lossy(unsupported)
                            ),
                        ));
                    }
                };
            }
            Event::Eof => break,
        }
    }

    if !elements.is_empty() {
        return Err(XmlParseError::new(
            reader.buffer_position(),
            "unclosed root element",
        ));
    }
    if !root_seen {
        return Err(XmlParseError::new(
            reader.buffer_position(),
            "an XML document must have a root element",
        ));
    }
    document.set_dom_root(root.expect("XML root node"))
        .and_then(|()| document.finish())
        .map_err(|error| XmlParseError::new(reader.buffer_position(), error.to_string()))
}

fn xml_start_tag(
    reader: &NsReader<&[u8]>,
    start: &BytesStart<'_>,
    version: XmlVersion,
    xhtml_entities: bool,
) -> Result<XmlStartTag, XmlParseError> {
    let offset = reader.buffer_position();
    utf8_xml_bytes(start.name().as_ref(), offset, "element name")?;
    let (namespace, local_name) = reader.resolver().resolve_element(start.name());
    let namespace = resolved_namespace(namespace, offset)?;
    let local_name = utf8_xml_bytes(local_name.as_ref(), offset, "element local name")?.to_owned();
    let mut attributes = Vec::new();
    let mut expanded_attribute_names = HashSet::new();
    for attribute in start.attributes().with_checks(true) {
        let attribute = attribute.map_err(|error| XmlParseError::new(offset, error.to_string()))?;
        let raw_attribute_name =
            utf8_xml_bytes(attribute.key.as_ref(), offset, "attribute name")?.to_owned();
        let (attribute_namespace, attribute_local_name) =
            reader.resolver().resolve_attribute(attribute.key);
        let attribute_namespace = resolved_namespace(attribute_namespace, offset)?;
        let attribute_local_name = utf8_xml_bytes(
            attribute_local_name.as_ref(),
            offset,
            "attribute local name",
        )?
        .to_owned();
        if !expanded_attribute_names
            .insert((attribute_namespace.clone(), attribute_local_name.clone()))
        {
            return Err(XmlParseError::new(
                offset,
                format!(
                    "duplicate expanded attribute name '{{{}}}{attribute_local_name}'",
                    attribute_namespace.as_deref().unwrap_or_default()
                ),
            ));
        }
        let value = attribute
            .normalized_value_with(version, 128, |name| {
                resolve_xml_entity(name, xhtml_entities)
            })
            .map_err(|error| XmlParseError::new(offset, error.to_string()))?
            .into_owned();
        attributes.push(XmlAttribute {
            name: raw_attribute_name,
            local_name: attribute_local_name,
            namespace: attribute_namespace,
            value,
        });
    }
    Ok(XmlStartTag {
        local_name,
        namespace,
        attributes,
    })
}

fn resolved_namespace(
    namespace: ResolveResult<'_>,
    offset: u64,
) -> Result<Option<String>, XmlParseError> {
    match namespace {
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Bound(namespace) => Ok(Some(
            utf8_xml_bytes(namespace.as_ref(), offset, "namespace URI")?.to_owned(),
        )),
        ResolveResult::Unknown(prefix) => Err(XmlParseError::new(
            offset,
            format!(
                "unknown namespace prefix '{}'",
                String::from_utf8_lossy(&prefix)
            ),
        )),
    }
}

fn resolve_xml_entity(name: &str, xhtml_entities: bool) -> Option<&'static str> {
    quick_xml::escape::resolve_xml_entity(name).or_else(|| {
        xhtml_entities
            .then(|| quick_xml::escape::resolve_html5_entity(name))
            .flatten()
    })
}

fn is_xhtml_doctype(doctype: &str) -> bool {
    let doctype = doctype.to_ascii_lowercase();
    doctype.split_ascii_whitespace().next() == Some("html")
        && (doctype.contains("-//w3c//dtd xhtml")
            || doctype.contains("xhtml1-")
            || doctype.contains("xhtml11"))
}

fn utf8_xml_bytes<'a>(
    bytes: &'a [u8],
    offset: u64,
    context: &str,
) -> Result<&'a str, XmlParseError> {
    std::str::from_utf8(bytes)
        .map_err(|error| XmlParseError::new(offset, format!("invalid UTF-8 in {context}: {error}")))
}

fn xml_reader_error(reader: &NsReader<&[u8]>, error: impl fmt::Display) -> XmlParseError {
    XmlParseError::new(reader.error_position(), error.to_string())
}

// Only open elements and their pending text are retained during streaming XML parsing.
struct XmlFrame {
    node: DomNodeId,
    name: String,
    namespace: Option<String>,
    text: String,
}

struct ForeignCapture {
    depth: usize,
    image: u32,
    markup: String,
}

struct XmlAttribute {
    name: String,
    local_name: String,
    namespace: Option<String>,
    value: String,
}

struct XmlStartTag {
    local_name: String,
    namespace: Option<String>,
    attributes: Vec<XmlAttribute>,
}

impl XmlStartTag {
    fn element_data(
        &self,
        document: &mut DocumentBuilder,
    ) -> Result<DomElementData, DocumentBuildError> {
        build_element_data(
            document,
            &self.local_name,
            self.namespace.as_deref(),
            self.attributes.iter().map(|attribute| SourceAttribute {
                name: Cow::Borrowed(&attribute.name),
                local_name: &attribute.local_name,
                namespace: attribute.namespace.as_deref(),
                value: &attribute.value,
            }),
            String::new,
        )
    }

    fn is_foreign_image(&self) -> bool {
        (self.local_name == "svg"
            && self.namespace.as_deref() == Some("http://www.w3.org/2000/svg"))
            || (self.local_name == "math"
                && self.namespace.as_deref() == Some("http://www.w3.org/1998/Math/MathML"))
    }

    fn write_start_markup(&self, output: &mut String, inherited_namespace: &str) {
        fn attribute(output: &mut String, name: &str, value: &str) {
            output.push(' ');
            output.push_str(name);
            output.push_str("=\"");
            write_escaped_xml(output, value);
            output.push('"');
        }
        output.push('<');
        output.push_str(&self.local_name);
        let namespace = self.namespace.as_deref().unwrap_or("");
        if namespace != inherited_namespace {
            attribute(output, "xmlns", namespace);
        }
        let mut declared_prefixes = Vec::new();
        for entry in &self.attributes {
            let Some(prefix) = entry.name.strip_prefix("xmlns:") else {
                continue;
            };
            let conflict = self.attributes.iter().any(|other| {
                other.name.split_once(':').is_some_and(|(used, _)| {
                    used == prefix
                        && other
                            .namespace
                            .as_deref()
                            .is_some_and(|uri| uri != entry.value)
                })
            });
            if !conflict && !declared_prefixes.contains(&prefix) {
                attribute(output, &entry.name, &entry.value);
                declared_prefixes.push(prefix);
            }
        }
        for entry in &self.attributes {
            if let Some((prefix, _)) = entry.name.split_once(':')
                && prefix != "xmlns"
                && prefix != "xml"
                && !declared_prefixes.contains(&prefix)
                && let Some(namespace) = entry.namespace.as_deref()
            {
                attribute(output, &format!("xmlns:{prefix}"), namespace);
                declared_prefixes.push(prefix);
            }
        }
        for entry in &self.attributes {
            if entry.name == "xmlns" || entry.name.starts_with("xmlns:") {
                continue;
            }
            attribute(output, &entry.name, &entry.value);
        }
        output.push('>');
    }
}

fn flush_xml_text(
    document: &mut DocumentBuilder,
    elements: &mut [XmlFrame],
    captures: &mut [ForeignCapture],
) -> Result<(), DocumentBuildError> {
    if let Some(frame) = elements.last_mut()
        && !frame.text.is_empty()
    {
        let text = mem::take(&mut frame.text);
        for capture in captures {
            write_escaped_xml(&mut capture.markup, &text);
        }
        document.append_dom_text(frame.node, text)?;
    }
    Ok(())
}

fn close_foreign_tag(name: &str, captures: &mut [ForeignCapture]) {
    for capture in captures {
        capture.markup.push_str("</");
        capture.markup.push_str(name);
        capture.markup.push('>');
    }
}

fn finish_foreign_capture(
    document: &mut DocumentBuilder,
    captures: &mut Vec<ForeignCapture>,
    depth: usize,
) {
    if captures
        .last()
        .is_some_and(|capture| capture.depth == depth)
    {
        let capture = captures.pop().expect("foreign XML capture");
        match &mut document.images_mut()[capture.image as usize].source {
            ImageSource::InlineSvg { bytes, .. } => *bytes = capture.markup.into_bytes().into(),
            ImageSource::MathMl { markup, .. } => *markup = capture.markup.into(),
            _ => unreachable!("foreign XML capture must belong to SVG or MathML"),
        }
    }
}
