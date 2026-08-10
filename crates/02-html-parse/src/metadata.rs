use crate::{HeadMetadata, HtmlSyntaxNode, HtmlSyntaxTree, StylesheetReference};

pub(crate) fn nonempty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub(crate) fn head_metadata_from_syntax_tree(tree: &HtmlSyntaxTree) -> HeadMetadata {
    let mut metadata = HeadMetadata::default();
    collect_syntax_metadata(&tree.nodes, false, &mut metadata);
    metadata
}

fn collect_syntax_metadata(
    nodes: &[HtmlSyntaxNode],
    inside_head: bool,
    metadata: &mut HeadMetadata,
) {
    for node in nodes {
        let HtmlSyntaxNode::Element(element) = node else {
            continue;
        };
        let inside_head = inside_head || element.local_name == "head";
        if inside_head && element.local_name == "title" && metadata.title.is_none() {
            metadata.title = nonempty(syntax_text_content(&element.children));
        } else if element.local_name == "style" && style_type_is_css(element.attribute("type")) {
            if let Some(css) = nonempty(syntax_text_content(&element.children)) {
                metadata.stylesheets.push(StylesheetReference::Inline(css));
            }
        } else if element.local_name == "link"
            && link_is_active_stylesheet(element.attribute("rel"))
            && link_type_is_css(element.attribute("type"))
            && let Some(href) = element
                .attribute("href")
                .and_then(|href| nonempty(href.to_owned()))
        {
            match element
                .attribute("charset")
                .and_then(|charset| nonempty(charset.to_owned()))
            {
                Some(charset) => metadata
                    .stylesheets
                    .push(StylesheetReference::ExternalWithCharset { href, charset }),
                None => metadata
                    .stylesheets
                    .push(StylesheetReference::External(href)),
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
    media_type
        .map(|media_type| {
            media_type
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("text/css")
        })
        .unwrap_or(true)
}

fn style_type_is_css(media_type: Option<&str>) -> bool {
    media_type
        .map(|media_type| {
            media_type.trim().is_empty() || media_type.trim().eq_ignore_ascii_case("text/css")
        })
        .unwrap_or(true)
}
