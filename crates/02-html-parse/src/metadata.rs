use crate::{HeadMetadata, HtmlSyntaxNode, HtmlSyntaxTree, StylesheetReference};

pub(crate) fn nonempty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
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
        } else if include_stylesheets
            && element.local_name == "style"
            && style_type_is_css(element.attribute("type"))
        {
            if let Some(css) = nonempty(syntax_text_content(&element.children)) {
                metadata.stylesheets.push(StylesheetReference::Inline(css));
            }
        } else if include_stylesheets
            && element.local_name == "link"
            && let Some(stylesheet) = StylesheetReference::from_link(
                element.attribute("rel"),
                element.attribute("type"),
                element.attribute("href"),
                element.attribute("charset"),
            )
        {
            metadata.stylesheets.push(stylesheet);
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
    let mut content = String::new();
    for node in nodes {
        match node {
            HtmlSyntaxNode::Text(text) => content.push_str(text),
            HtmlSyntaxNode::Element(element) => {
                content.push_str(&syntax_text_content(&element.children))
            }
            HtmlSyntaxNode::TemplateContents(children) => {
                content.push_str(&syntax_text_content(children))
            }
            HtmlSyntaxNode::Doctype { .. }
            | HtmlSyntaxNode::Comment(_)
            | HtmlSyntaxNode::ProcessingInstruction { .. } => {}
        }
    }
    content
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
    media_type.is_none_or(|media_type| {
        media_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("text/css")
    })
}

/// Whether a style element's type attribute permits CSS text.
pub fn style_type_is_css(media_type: Option<&str>) -> bool {
    media_type.is_none_or(|media_type| {
        media_type.trim().is_empty() || media_type.trim().eq_ignore_ascii_case("text/css")
    })
}
