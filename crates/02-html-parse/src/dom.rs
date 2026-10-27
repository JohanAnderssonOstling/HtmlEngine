use crate::{
    HtmlSyntaxElement, HtmlSyntaxNode, HtmlSyntaxTree, MarkupSyntax, ParsedHtml, parse_html_document,
};
use html_dom::{
    Document, DocumentBuildError, DocumentBuilder, DocumentMode, DomAttribute,
    DomElementAttributeCache, DomElementData, DomNodeId, ImageResource, ImageSource, Rootless,
};
use std::sync::Arc;

pub fn parse_dom_document(html: &str) -> Result<Document, DocumentBuildError> {
    build_dom_document_owned(parse_html_document(html))
}

/// Convert already parsed markup into the renderer's DOM.
pub fn build_dom_document(parsed: &ParsedHtml) -> Result<Document, DocumentBuildError> {
    let mode = match parsed.syntax {
        MarkupSyntax::Html => DocumentMode::Html,
        MarkupSyntax::Xml => DocumentMode::Xml,
    };
    build_dom_from_syntax_tree(&parsed.document, mode)
}

pub(crate) fn build_dom_document_owned(parsed: ParsedHtml) -> Result<Document, DocumentBuildError> {
    let mode = match parsed.syntax {
        MarkupSyntax::Html => DocumentMode::Html,
        MarkupSyntax::Xml => DocumentMode::Xml,
    };
    let Some(root) = parsed
        .document
        .nodes
        .into_iter()
        .find_map(|node| match node {
            HtmlSyntaxNode::Element(element) => Some(element),
            _ => None,
        })
    else {
        return DocumentBuilder::new().finish_without_root();
    };
    let mut document = DocumentBuilder::new();
    document.set_mode(mode);
    let root_data = build_dom_syntax_element_data(&mut document, &root);
    let (root_node, mut document) = document.append_dom_element(None, root_data)?;
    build_dom_owned_children(&mut document, root.children, root_node)?;
    document.set_dom_root(root_node)?.finish()
}

fn build_dom_owned_children(
    document: &mut DocumentBuilder<Rootless>,
    children: Vec<HtmlSyntaxNode>,
    parent: DomNodeId,
) -> Result<(), DocumentBuildError> {
    for child in children {
        match child {
            HtmlSyntaxNode::Element(element) => {
                let data = build_dom_syntax_element_data(document, &element);
                let node = document.append_dom_element(Some(parent), data)?;
                build_dom_owned_children(document, element.children, node)?;
            }
            HtmlSyntaxNode::Text(text) if !text.is_empty() => {
                document.append_dom_text(parent, text)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn build_dom_from_syntax_tree(
    tree: &HtmlSyntaxTree,
    mode: DocumentMode,
) -> Result<Document, DocumentBuildError> {
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
    document.set_dom_root(root_node)?.finish()
}

fn build_dom_syntax_children(
    document: &mut DocumentBuilder<Rootless>,
    children: &[HtmlSyntaxNode],
    parent: DomNodeId,
) -> Result<(), DocumentBuildError> {
    for child in children {
        match child {
            HtmlSyntaxNode::Element(element) => {
                let data = build_dom_syntax_element_data(document, element);
                let node = document.append_dom_element(Some(parent), data)?;
                build_dom_syntax_children(document, &element.children, node)?;
            }
            HtmlSyntaxNode::Text(text) if !text.is_empty() => {
                document.append_dom_text(parent, text.clone())?;
            }
            // `html-dom` has no document-fragment node. Keep template contents
            // out of the rendered child list, matching the former scraper-tree
            // conversion path.
            HtmlSyntaxNode::TemplateContents(_)
            | HtmlSyntaxNode::Doctype { .. }
            | HtmlSyntaxNode::Comment(_)
            | HtmlSyntaxNode::ProcessingInstruction { .. }
            | HtmlSyntaxNode::Text(_) => {}
        }
    }
    Ok(())
}

fn build_dom_syntax_element_data<State>(
    document: &mut DocumentBuilder<State>,
    element: &HtmlSyntaxElement,
) -> DomElementData {
    let tag = document.intern_string(&element.local_name);
    let namespace = element
        .namespace
        .as_deref()
        .map(|namespace| document.intern_string(namespace));
    let id = element
        .attribute("id")
        .map(|value| document.intern_string(value));
    let classes = document.intern_classes(element.attribute("class"));
    let source = element
        .attribute("src")
        .or_else(|| element.attribute("href"))
        .or_else(|| element.attribute("xlink:href"));
    let width = element
        .attribute("width")
        .and_then(|value| value.parse().ok());
    let height = element
        .attribute("height")
        .and_then(|value| value.parse().ok());
    let is_inline_svg = element.local_name == "svg"
        && element.namespace.as_deref() == Some("http://www.w3.org/2000/svg");
    let is_mathml = element.local_name == "math"
        && element.namespace.as_deref() == Some("http://www.w3.org/1998/Math/MathML");
    let image_idx = if is_mathml {
        Some(document.push_image(ImageResource {
            source: ImageSource::MathMl {
                markup: serialize_foreign_syntax_element(element).into(),
                svg: Arc::from([]),
                baseline: 1.0,
            },
            width: 0,
            height: 0,
            width_attr: None,
            height_attr: None,
        }))
    } else if is_inline_svg {
        let bytes: Arc<[u8]> = serialize_foreign_syntax_element(element).into_bytes().into();
        Some(document.push_image(ImageResource {
            source: ImageSource::InlineSvg {
                bytes,
                base_uri: String::new(),
            },
            width: 0,
            height: 0,
            width_attr: None,
            height_attr: None,
        }))
    } else if matches!(element.local_name.as_str(), "img" | "image") {
        source
            .filter(|source| !source.trim().is_empty())
            .map(|source| {
                document.push_image(ImageResource {
                    source: ImageSource::Uri(source.trim().to_owned()),
                    width: 0,
                    height: 0,
                    width_attr: width,
                    height_attr: height,
                })
            })
    } else {
        None
    };
    let attributes = element
        .attributes
        .iter()
        .map(|attribute| DomAttribute {
            name: document.intern_string(&attribute.name),
            local_name: document.intern_string(&attribute.local_name),
            namespace: attribute
                .namespace
                .as_deref()
                .map(|namespace| document.intern_string(namespace)),
            value: document.intern_string(&attribute.value),
        })
        .collect();
    DomElementData {
        tag,
        namespace,
        attributes,
        attribute_cache: DomElementAttributeCache::new(id, classes),
        image_idx,
    }
}

fn serialize_foreign_syntax_element(element: &HtmlSyntaxElement) -> String {
    fn push_attribute(output: &mut String, name: &str, value: &str) {
        output.push(' ');
        output.push_str(name);
        output.push_str("=\"");
        output.push_str(&quick_xml::escape::escape(value));
        output.push('"');
    }

    fn push_node(output: &mut String, node: &HtmlSyntaxNode, default_namespace: &str) {
        match node {
            HtmlSyntaxNode::Element(element) => {
                push_element(output, element, default_namespace)
            }
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
            HtmlSyntaxNode::TemplateContents(children) => {
                children
                    .iter()
                    .for_each(|child| push_node(output, child, default_namespace))
            }
            HtmlSyntaxNode::Doctype { .. } => {}
        }
    }
    fn push_element(output: &mut String, element: &HtmlSyntaxElement, inherited_namespace: &str) {
        output.push('<');
        output.push_str(&element.local_name);
        let namespace = element.namespace.as_deref().unwrap_or("");
        if namespace != inherited_namespace {
            push_attribute(output, "xmlns", namespace);
        }

        let mut declared_prefixes = Vec::new();
        for attribute in &element.attributes {
            let Some(prefix) = attribute.name.strip_prefix("xmlns:") else {
                continue;
            };
            let conflicting_use = element.attributes.iter().any(|other| {
                other.name.split_once(':').is_some_and(|(used_prefix, _)| {
                    used_prefix == prefix
                        && other.namespace.as_deref().is_some_and(|uri| uri != attribute.value)
                })
            });
            if !conflicting_use && !declared_prefixes.contains(&prefix) {
                push_attribute(output, &attribute.name, &attribute.value);
                declared_prefixes.push(prefix);
            }
        }
        for attribute in &element.attributes {
            if let Some((prefix, _)) = attribute.name.split_once(':')
                && prefix != "xmlns"
                && prefix != "xml"
                && !declared_prefixes.contains(&prefix)
                && let Some(uri) = attribute.namespace.as_deref()
            {
                push_attribute(output, &format!("xmlns:{prefix}"), uri);
                declared_prefixes.push(prefix);
            }
        }
        for attribute in &element.attributes {
            if attribute.name == "xmlns" || attribute.name.starts_with("xmlns:") {
                continue;
            }
            push_attribute(output, &attribute.name, &attribute.value);
        }
        output.push('>');
        element
            .children
            .iter()
            .for_each(|child| push_node(output, child, namespace));
        output.push_str("</");
        output.push_str(&element.local_name);
        output.push('>');
    }

    let mut output = String::new();
    push_element(&mut output, element, "");
    output
}
