use crate::StylesheetReference;
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

pub(crate) fn document_title(document: &Document) -> Option<String> {
    fn text(document: &Document, node: DomNodeId, output: &mut String) {
        match document.node_ref(node) {
            NodeRef::Text(value) => output.push_str(value.text()),
            NodeRef::Element(element) => {
                for child in element.children() {
                    text(document, child, output);
                }
            }
        }
    }
    fn find(document: &Document, node: DomNodeId, inside_head: bool) -> Option<String> {
        let element = document.try_element_ref(node)?;
        let inside_head = inside_head || element.tag() == "head";
        if inside_head && element.tag() == "title" {
            let mut value = String::new();
            for child in element.children() {
                text(document, child, &mut value);
            }
            if let Some(value) = nonempty(value) {
                return Some(value);
            }
        }
        element
            .children()
            .find_map(|child| find(document, child, inside_head))
    }
    find(document, document.dom_root()?, false)
}

/// Collect active sheets in source order, retaining DOM owners for implicit scopes.
pub fn document_stylesheets(document: &Document) -> Vec<(StylesheetReference, Option<DomNodeId>)> {
    fn append_text(document: &Document, node: DomNodeId, text: &mut String) {
        match document.node_ref(node) {
            NodeRef::Text(value) => text.push_str(value.text()),
            NodeRef::Element(element) => element
                .children()
                .for_each(|child| append_text(document, child, text)),
        }
    }

    document
        .node_ids()
        .filter_map(|node| {
            let element = document.try_element_ref(node)?;
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
    use crate::{StylesheetReference, parse_xml_document};

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
