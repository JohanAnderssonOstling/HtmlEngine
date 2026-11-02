use crate::{HeadMetadata, HtmlSyntaxNode, HtmlSyntaxTree, StylesheetReference};
use html_dom::{Document, DomNodeId, NodeRef, media_type_essence};

pub(crate) fn nonempty(mut value: String) -> Option<String> {
    value.truncate(value.trim_end().len());
    let start = value.len() - value.trim_start().len();
    value.drain(..start);
    (!value.is_empty()).then_some(value)
}

/// Share stylesheet policy while each tree keeps its native traversal.
fn stylesheet_for_element<'a>(
    namespace: Option<&str>,
    tag: &str,
    attribute: impl Fn(&str) -> Option<&'a str>,
    text: impl FnOnce() -> String,
) -> Option<StylesheetReference> {
    if namespace != Some("http://www.w3.org/1999/xhtml") {
        return None;
    }
    match tag {
        "style" if style_type_is_css(attribute("type")) => {
            nonempty(text()).map(StylesheetReference::Inline)
        }
        "link" => StylesheetReference::from_link(
            attribute("rel"),
            attribute("type"),
            attribute("href"),
            attribute("charset"),
        ),
        _ => None,
    }
}

pub(crate) fn head_metadata_from_syntax_tree(tree: &HtmlSyntaxTree) -> HeadMetadata {
    let mut metadata = HeadMetadata::default();
    collect_syntax_metadata(&tree.nodes, false, &mut metadata, true);
    metadata
}

pub(crate) fn title_from_syntax_tree(tree: &HtmlSyntaxTree) -> Option<String> {
    let mut metadata = HeadMetadata::default();
    collect_syntax_metadata(&tree.nodes, false, &mut metadata, false);
    metadata.title
}

fn collect_syntax_metadata(
    nodes: &[HtmlSyntaxNode],
    inside_head: bool,
    metadata: &mut HeadMetadata,
    include_stylesheets: bool,
) {
    for node in nodes {
        if !include_stylesheets && metadata.title.is_some() {
            break;
        }
        let HtmlSyntaxNode::Element(element) = node else {
            continue;
        };
        let inside_head = inside_head || element.local_name == "head";
        if inside_head && element.local_name == "title" && metadata.title.is_none() {
            metadata.title = nonempty(syntax_text_content(&element.children));
        } else if include_stylesheets {
            if let Some(sheet) = stylesheet_for_element(
                element.namespace.as_deref(),
                &element.local_name,
                |name| element.attribute(name),
                || syntax_text_content(&element.children),
            ) {
                metadata.stylesheets.push(sheet);
            }
        }
        collect_syntax_metadata(
            &element.children,
            inside_head,
            metadata,
            include_stylesheets,
        );
    }
}

fn syntax_text_content(nodes: &[HtmlSyntaxNode]) -> String {
    fn append(nodes: &[HtmlSyntaxNode], text: &mut String) {
        for node in nodes {
            match node {
                HtmlSyntaxNode::Text(value) => text.push_str(value),
                HtmlSyntaxNode::Element(element) => append(&element.children, text),
                HtmlSyntaxNode::TemplateContents(children) => append(children, text),
                _ => {}
            }
        }
    }

    let mut text = String::new();
    append(nodes, &mut text);
    text
}

/// Collect active sheets in source order, retaining DOM owners for implicit scopes.
pub fn document_stylesheets(document: &Document) -> Vec<(StylesheetReference, Option<DomNodeId>)> {
    fn append_text(document: &Document, node: DomNodeId, text: &mut String) {
        match document.node_ref(node) {
            Some(NodeRef::Text(value)) => text.push_str(value.text()),
            Some(NodeRef::Element(element)) => element
                .children()
                .for_each(|child| append_text(document, child, text)),
            None => {}
        }
    }

    document
        .node_ids()
        .filter_map(|node| {
            let element = document.element_ref(node)?;
            let sheet = stylesheet_for_element(
                element.namespace(),
                element.tag(),
                |name| element.attr(name),
                || {
                    let mut text = String::new();
                    element
                        .children()
                        .for_each(|child| append_text(document, child, &mut text));
                    text
                },
            )?;
            Some((sheet, element.parent().or(document.dom_root())))
        })
        .collect()
}

impl StylesheetReference {
    /// Classify a link's attributes as an active CSS stylesheet reference.
    pub fn from_link(
        rel: Option<&str>,
        media_type: Option<&str>,
        href: Option<&str>,
        charset: Option<&str>,
    ) -> Option<Self> {
        if !link_is_active_stylesheet(rel) || !link_type_is_css(media_type) {
            return None;
        }
        let href = href?.trim();
        if href.is_empty() {
            return None;
        }
        let href = href.to_owned();
        Some(
            match charset.map(str::trim).filter(|charset| !charset.is_empty()) {
                Some(charset) => Self::ExternalWithCharset {
                    href,
                    charset: charset.to_owned(),
                },
                None => Self::External(href),
            },
        )
    }
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
    media_type.is_none_or(|value| media_type_essence(value).eq_ignore_ascii_case("text/css"))
}

/// Whether a style element's type attribute permits CSS text.
pub fn style_type_is_css(media_type: Option<&str>) -> bool {
    media_type.is_none_or(|media_type| {
        media_type.trim().is_empty() || media_type.trim().eq_ignore_ascii_case("text/css")
    })
}

#[cfg(test)]
mod tests {
    use super::document_stylesheets;
    use crate::{StylesheetReference, parse_html_document, parse_xml_document};

    #[test]
    fn syntax_and_dom_share_stylesheet_order_and_exclusions() {
        let parsed = parse_html_document(
            r#"<html><head>
            <style> p {color:red} </style>
            <link rel="stylesheet" href="theme.css">
            <link rel="alternate stylesheet" href="inactive.css">
            <template><style>template {color:red}</style></template>
            </head><body><svg><style>foreign {color:red}</style></svg>
            <section id="scope"><style> p {color:green} </style></section>
            </body></html>"#,
        );
        let expected = vec![
            StylesheetReference::Inline("p {color:red}".into()),
            StylesheetReference::External("theme.css".into()),
            StylesheetReference::Inline("p {color:green}".into()),
        ];
        assert_eq!(parsed.head_metadata().stylesheets, expected);
        let document = parsed.into_dom();
        let sheets = document_stylesheets(&document);
        assert_eq!(
            sheets
                .iter()
                .map(|(sheet, _)| sheet.clone())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(document.get_dom_id(sheets[2].1.unwrap()), Some("scope"));
    }

    #[test]
    fn xml_stylesheets_require_html_namespace_and_exact_names() {
        let parsed = parse_xml_document(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head>
            <style>p {color:green}</style><STYLE>p {color:red}</STYLE>
            <style xmlns="urn:other">p {color:blue}</style>
            </head></html>"#,
        )
        .unwrap();
        let expected = vec![StylesheetReference::Inline("p {color:green}".into())];
        assert_eq!(parsed.head_metadata().stylesheets, expected);
        assert_eq!(
            document_stylesheets(&parsed.into_dom())
                .into_iter()
                .map(|(sheet, _)| sheet)
                .collect::<Vec<_>>(),
            expected
        );
    }
}
