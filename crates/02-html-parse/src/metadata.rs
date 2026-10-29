use crate::{HeadMetadata, HtmlSyntaxNode, HtmlSyntaxTree, StylesheetReference};
use html_dom::{Document, DomNodeId, ElementRef, NodeRef, media_type_essence};

pub(crate) fn nonempty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// One traversal policy for parsed syntax and the canonical, possibly edited DOM.
#[derive(Clone, Copy)]
enum MetadataNode<'a> {
    Syntax(&'a HtmlSyntaxNode),
    Dom(&'a Document, DomNodeId),
}

impl<'a> MetadataNode<'a> {
    fn element(self) -> Option<MetadataElement<'a>> {
        match self {
            Self::Syntax(HtmlSyntaxNode::Element(element)) => {
                Some(MetadataElement::Syntax(element))
            }
            Self::Dom(document, node) => document.element_ref(node).map(MetadataElement::Dom),
            _ => None,
        }
    }

    fn visit_children(self, mut visit: impl FnMut(Self)) {
        match self {
            Self::Syntax(node) => {
                let children = match node {
                    HtmlSyntaxNode::Element(element) => &element.children,
                    HtmlSyntaxNode::TemplateContents(children) => children,
                    _ => return,
                };
                children.iter().for_each(|node| visit(Self::Syntax(node)));
            }
            Self::Dom(document, node) => {
                if let Some(element) = document.element_ref(node) {
                    element
                        .children()
                        .for_each(|node| visit(Self::Dom(document, node)));
                }
            }
        }
    }

    fn append_text(self, output: &mut String) {
        match self {
            Self::Syntax(HtmlSyntaxNode::Text(text)) => output.push_str(text),
            Self::Dom(document, node) => match document.node_ref(node) {
                Some(NodeRef::Text(text)) => output.push_str(text.text()),
                _ => self.visit_children(|child| child.append_text(output)),
            },
            _ => self.visit_children(|child| child.append_text(output)),
        }
    }
}

#[derive(Clone, Copy)]
enum MetadataElement<'a> {
    Syntax(&'a crate::HtmlSyntaxElement),
    Dom(ElementRef<'a>),
}

impl<'a> MetadataElement<'a> {
    fn is_named(self, tag: &str) -> bool {
        match self {
            Self::Syntax(element) => element.local_name == tag,
            Self::Dom(element) => element.tag() == tag,
        }
    }

    fn is_html(self, tag: &str) -> bool {
        let (namespace, name) = match self {
            Self::Syntax(element) => (element.namespace.as_deref(), element.local_name.as_str()),
            Self::Dom(element) => (element.namespace(), element.tag()),
        };
        namespace == Some("http://www.w3.org/1999/xhtml") && name == tag
    }

    fn attribute(self, name: &str) -> Option<&'a str> {
        match self {
            Self::Syntax(element) => element.attribute(name),
            Self::Dom(element) => element.attr(name),
        }
    }
}

#[derive(Default)]
struct Metadata {
    title: Option<String>,
    stylesheets: Vec<(StylesheetReference, Option<DomNodeId>)>,
}

impl Metadata {
    fn collect(
        &mut self,
        node: MetadataNode<'_>,
        inside_head: bool,
        include_stylesheets: bool,
        include_title: bool,
    ) {
        if !include_stylesheets && self.title.is_some() {
            return;
        }
        let Some(element) = node.element() else {
            return;
        };
        let inside_head = inside_head || element.is_named("head");
        let text_content = || {
            let mut text = String::new();
            node.visit_children(|child| child.append_text(&mut text));
            text
        };
        if include_title && inside_head && element.is_named("title") && self.title.is_none() {
            self.title = nonempty(text_content());
        } else if include_stylesheets {
            let stylesheet =
                if element.is_html("style") && style_type_is_css(element.attribute("type")) {
                    nonempty(text_content()).map(StylesheetReference::Inline)
                } else if element.is_html("link") {
                    StylesheetReference::from_link(
                        element.attribute("rel"),
                        element.attribute("type"),
                        element.attribute("href"),
                        element.attribute("charset"),
                    )
                } else {
                    None
                };
            if let Some(stylesheet) = stylesheet {
                let root = match node {
                    MetadataNode::Dom(document, id) => document
                        .element_ref(id)
                        .and_then(|element| element.parent())
                        .or(document.dom_root()),
                    MetadataNode::Syntax(_) => None,
                };
                self.stylesheets.push((stylesheet, root));
            }
        }
        node.visit_children(|child| {
            self.collect(child, inside_head, include_stylesheets, include_title)
        });
    }
}

pub(crate) fn head_metadata_from_syntax_tree(tree: &HtmlSyntaxTree) -> HeadMetadata {
    let mut metadata = Metadata::default();
    tree.nodes
        .iter()
        .for_each(|node| metadata.collect(MetadataNode::Syntax(node), false, true, true));
    HeadMetadata {
        title: metadata.title,
        stylesheets: metadata
            .stylesheets
            .into_iter()
            .map(|(sheet, _)| sheet)
            .collect(),
    }
}

pub(crate) fn title_from_syntax_tree(tree: &HtmlSyntaxTree) -> Option<String> {
    let mut metadata = Metadata::default();
    tree.nodes
        .iter()
        .for_each(|node| metadata.collect(MetadataNode::Syntax(node), false, false, true));
    metadata.title
}

/// Collect active sheets in source order, retaining DOM owners for implicit scopes.
pub fn document_stylesheets(document: &Document) -> Vec<(StylesheetReference, Option<DomNodeId>)> {
    let mut metadata = Metadata::default();
    if let Some(root) = document.dom_root() {
        metadata.collect(MetadataNode::Dom(document, root), false, true, false);
    }
    metadata.stylesheets
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
