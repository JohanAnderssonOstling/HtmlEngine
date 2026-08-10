use crate::{
    HtmlQuirksMode, HtmlSyntaxAttribute, HtmlSyntaxElement, HtmlSyntaxNode, HtmlSyntaxTree,
    MarkupSyntax, ParsedHtml, declared_source_encoding,
};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmlParseError {
    offset: u64,
    message: String,
}

impl XmlParseError {
    fn new(offset: u64, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
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
        write!(
            formatter,
            "XML parse error at byte {}: {}",
            self.offset, self.message
        )
    }
}

impl std::error::Error for XmlParseError {}
pub fn parse_xml_document(xml: &str) -> Result<ParsedHtml, XmlParseError> {
    let document = parse_xml_syntax_tree(xml)?;
    Ok(ParsedHtml {
        document,
        syntax: MarkupSyntax::Xml,
        quirks_mode: HtmlQuirksMode::NoQuirks,
        errors: Vec::new(),
        source_encoding: declared_source_encoding(xml),
    })
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
        let event = reader
            .read_event()
            .map_err(|error| xml_reader_error(&reader, error))?;
        match event {
            Event::Start(start) => {
                if elements.is_empty() {
                    if root_seen {
                        return Err(XmlParseError::new(
                            reader.buffer_position(),
                            "an XML document must have exactly one root element",
                        ));
                    }
                    root_seen = true;
                }
                elements.push(xml_syntax_element(
                    &reader,
                    &start,
                    version,
                    xhtml_entities,
                )?);
            }
            Event::Empty(start) => {
                if elements.is_empty() {
                    if root_seen {
                        return Err(XmlParseError::new(
                            reader.buffer_position(),
                            "an XML document must have exactly one root element",
                        ));
                    }
                    root_seen = true;
                }
                let element = xml_syntax_element(&reader, &start, version, xhtml_entities)?;
                push_xml_node(&mut nodes, &mut elements, HtmlSyntaxNode::Element(element));
            }
            Event::End(_) => {
                let Some(element) = elements.pop() else {
                    return Err(XmlParseError::new(
                        reader.buffer_position(),
                        "closing element has no open element",
                    ));
                };
                push_xml_node(&mut nodes, &mut elements, HtmlSyntaxNode::Element(element));
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
                    push_xml_text(&mut elements, text.as_ref());
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
                push_xml_text(&mut elements, text.as_ref());
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
                push_xml_text(&mut elements, &value);
            }
            Event::Comment(comment) => {
                if !declaration_seen {
                    content_seen_before_declaration = true;
                }
                let comment = comment
                    .xml_content(version)
                    .map_err(|error| xml_reader_error(&reader, error))?
                    .into_owned();
                push_xml_node(&mut nodes, &mut elements, HtmlSyntaxNode::Comment(comment));
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
                push_xml_node(
                    &mut nodes,
                    &mut elements,
                    HtmlSyntaxNode::ProcessingInstruction { target, data },
                );
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
                let name = raw
                    .split_ascii_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                nodes.push(HtmlSyntaxNode::Doctype {
                    name,
                    public_id: String::new(),
                    system_id: String::new(),
                });
            }
            Event::Decl(declaration) => {
                if declaration_seen
                    || content_seen_before_declaration
                    || root_seen
                    || doctype_seen
                    || !nodes.is_empty()
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
    Ok(HtmlSyntaxTree { nodes })
}

fn xml_syntax_element(
    reader: &NsReader<&[u8]>,
    start: &BytesStart<'_>,
    version: XmlVersion,
    xhtml_entities: bool,
) -> Result<HtmlSyntaxElement, XmlParseError> {
    let offset = reader.buffer_position();
    utf8_xml_bytes(start.name().as_ref(), offset, "element name")?;
    let (namespace, local_name) = reader.resolver().resolve_element(start.name());
    let namespace = resolved_namespace(namespace, offset)?;
    let local_name = utf8_xml_bytes(local_name.as_ref(), offset, "element local name")?.to_owned();
    let mut attributes = Vec::new();
    let mut expanded_attribute_names = std::collections::HashSet::new();
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
        attributes.push(HtmlSyntaxAttribute {
            name: raw_attribute_name,
            local_name: attribute_local_name,
            namespace: attribute_namespace,
            value,
        });
    }
    Ok(HtmlSyntaxElement {
        local_name,
        namespace,
        attributes,
        children: Vec::new(),
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

fn push_xml_node(
    nodes: &mut Vec<HtmlSyntaxNode>,
    elements: &mut [HtmlSyntaxElement],
    node: HtmlSyntaxNode,
) {
    if let Some(parent) = elements.last_mut() {
        parent.children.push(node);
    } else {
        nodes.push(node);
    }
}

fn push_xml_text(elements: &mut [HtmlSyntaxElement], text: &str) {
    let Some(parent) = elements.last_mut() else {
        return;
    };
    if let Some(HtmlSyntaxNode::Text(existing)) = parent.children.last_mut() {
        existing.push_str(text);
    } else if !text.is_empty() {
        parent.children.push(HtmlSyntaxNode::Text(text.to_owned()));
    }
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
