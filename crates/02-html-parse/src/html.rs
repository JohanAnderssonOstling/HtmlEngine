//! Private html5ever/scraper adapter. The renderer consumes only our DOM.
use crate::dom::{ParsedDomDocument, SourceAttribute, build_element_data};
use crate::metadata::nonempty;
use crate::text::is_text_boundary_element;
use crate::{
    HtmlParserOptions, HtmlQuirksMode, HtmlScriptingMode, ParsedHtml, declared_source_encoding};
use ego_tree::NodeRef;
use html_dom::{
    Document, DocumentBuildError, DocumentBuilder, DomElementData, DomNodeId,
    write_escaped_xml};
use html5ever::driver::{self, ParseOpts};
use html5ever::tendril::TendrilSink;
use html5ever::tree_builder::{QuirksMode as Html5everQuirksMode, TreeBuilderOpts};
use html5ever::{LocalName, Namespace, QualName};
use scraper::{Html, Node};
use std::borrow::Cow;
use std::mem;

struct HtmlTree(Html);

pub(crate) fn parse_document(source: &str, options: HtmlParserOptions) -> ParsedHtml {
    let mut document = parse_native_document(source, options);
    let errors = mem::take(&mut document.errors)
        .into_iter()
        .map(Cow::into_owned)
        .collect();
    let quirks_mode = map_quirks_mode(document.quirks_mode);
    ParsedHtml {
        document: build_dom_with_title(HtmlTree(document)),
        quirks_mode,
        errors,
        source_encoding: declared_source_encoding(source),
    }
}

pub(crate) fn fragment_text(source: &str) -> String {
    let context = QualName::new(
        None,
        Namespace::from("http://www.w3.org/1999/xhtml"),
        LocalName::from("div"),
    );
    let document = driver::parse_fragment(
        scraper::HtmlTreeSink::new(Html::new_fragment()),
        html5ever_options(HtmlParserOptions::default()),
        context,
        Vec::new(),
        false,
    )
    .one(source);
    fn append(parent: NodeRef<'_, Node>, output: &mut String) {
        for child in parent.children() {
            match child.value() {
                Node::Text(text) => output.push_str(text),
                Node::Element(element) => {
                    let name = element.name.local.as_ref();
                    if matches!(name, "script" | "style" | "template" | "noscript" | "head") {
                        continue;
                    }
                    let boundary = is_text_boundary_element(name);
                    if boundary && !output.ends_with(char::is_whitespace) {
                        output.push(' ');
                    }
                    append(child, output);
                    if boundary && !output.ends_with(char::is_whitespace) {
                        output.push(' ');
                    }
                }
                _ => {}
            }
        }
    }
    let mut text = String::new();
    append(document.tree.root(), &mut text);
    text
}

pub(crate) fn parse_dom_document(source: &str) -> Result<ParsedDomDocument, DocumentBuildError> {
    let document = parse_native_document(source, HtmlParserOptions::default());
    let document = build_dom_with_title(HtmlTree(document))?;
    Ok(ParsedDomDocument {
        document,
        source_encoding: declared_source_encoding(source),
    })
}

fn parse_native_document(source: &str, options: HtmlParserOptions) -> Html {
    driver::parse_document(
        scraper::HtmlTreeSink::new(Html::new_document()),
        html5ever_options(options),
    )
    .one(source)
}

fn build_dom_with_title(tree: HtmlTree) -> Result<Document, DocumentBuildError> {
    let title = title(tree.0.tree.root(), false);
    let mut document = tree.build_dom()?;
    document.set_title(title);
    Ok(document)
}

impl HtmlTree {
    fn build_dom(&self) -> Result<Document, DocumentBuildError> {
        let Some(root) = self
            .0
            .tree
            .root()
            .children()
            .find(|node| node.value().is_element())
        else {
            return DocumentBuilder::new().finish();
        };
        let mut document = DocumentBuilder::new();
        let data = element_data(&mut document, root)?;
        let root_id = document.append_dom_element(None, data)?;
        append_children(&mut document, root, root_id)?;
        document.set_dom_root(root_id)?;
        document.finish()
    }
}

fn append_children(
    document: &mut DocumentBuilder,
    parent: NodeRef<'_, Node>,
    parent_id: DomNodeId,
) -> Result<(), DocumentBuildError> {
    for child in parent.children() {
        match child.value() {
            Node::Element(_) => {
                let data = element_data(document, child)?;
                let node = document.append_dom_element(Some(parent_id), data)?;
                append_children(document, child, node)?;
            }
            Node::Text(text) if !text.is_empty() => {
                document.append_dom_text(parent_id, text.to_string())?;
            }
            // Template contents live in fragment nodes, outside rendered children.
            _ => {}
        }
    }
    Ok(())
}

fn element_data(
    document: &mut DocumentBuilder,
    node: NodeRef<'_, Node>,
) -> Result<DomElementData, DocumentBuildError> {
    let element = node.value().as_element().expect("element node");
    build_element_data(
        document,
        element.name.local.as_ref(),
        (!element.name.ns.is_empty()).then(|| element.name.ns.as_ref()),
        element.attrs.iter().map(|(name, value)| SourceAttribute {
            name: qualified_name(name),
            local_name: name.local.as_ref(),
            namespace: (!name.ns.is_empty()).then(|| name.ns.as_ref()),
            value,
        }),
        || serialize_foreign_element(node),
    )
}

fn qualified_name(name: &QualName) -> Cow<'_, str> {
    match &name.prefix {
        Some(prefix) => Cow::Owned(format!("{}:{}", prefix.as_ref(), name.local.as_ref())),
        None => Cow::Borrowed(name.local.as_ref()),
    }
}

fn title(parent: NodeRef<'_, Node>, inside_head: bool) -> Option<String> {
    for child in parent.children() {
        let Node::Element(element) = child.value() else {
            continue;
        };
        let inside_head = inside_head || element.name.local.as_ref() == "head";
        if inside_head
            && element.name.local.as_ref() == "title"
            && let Some(title) = nonempty(text_content(child))
        {
            return Some(title);
        }
        if let Some(title) = title(child, inside_head) {
            return Some(title);
        }
    }
    None
}

fn text_content(parent: NodeRef<'_, Node>) -> String {
    fn append(parent: NodeRef<'_, Node>, output: &mut String) {
        for child in parent.children() {
            match child.value() {
                Node::Text(text) => output.push_str(text),
                Node::Element(_) | Node::Fragment => append(child, output),
                _ => {}
            }
        }
    }
    let mut output = String::new();
    append(parent, &mut output);
    output
}

fn serialize_foreign_element(root: NodeRef<'_, Node>) -> String {
    fn push_attribute(output: &mut String, name: &str, value: &str) {
        output.push(' ');
        output.push_str(name);
        output.push_str("=\"");
        write_escaped_xml(output, value);
        output.push('"');
    }
    fn prefix(name: &QualName) -> Option<&str> {
        name.prefix
            .as_ref()
            .map(|prefix| prefix.as_ref())
            .or_else(|| name.local.split_once(':').map(|(prefix, _)| prefix))
    }
    fn push_node(output: &mut String, node: NodeRef<'_, Node>, inherited_namespace: &str) {
        match node.value() {
            Node::Element(element) => {
                output.push('<');
                output.push_str(element.name.local.as_ref());
                let namespace = element.name.ns.as_ref();
                if namespace != inherited_namespace {
                    push_attribute(output, "xmlns", namespace);
                }
                let mut declared_prefixes = Vec::new();
                for (name, value) in &element.attrs {
                    let declared = if name
                        .prefix
                        .as_ref()
                        .is_some_and(|prefix| prefix.as_ref() == "xmlns")
                    {
                        Some(name.local.as_ref())
                    } else if name.prefix.is_none() {
                        name.local.strip_prefix("xmlns:")
                    } else {
                        None
                    };
                    let Some(declared) = declared else { continue };
                    let conflicting_use = element.attrs.iter().any(|(other, _)| {
                        prefix(other) == Some(declared)
                            && !other.ns.is_empty()
                            && other.ns.as_ref() != value.as_ref()
                    });
                    if !conflicting_use && !declared_prefixes.contains(&declared) {
                        push_attribute(output, &qualified_name(name), value);
                        declared_prefixes.push(declared);
                    }
                }
                for (name, _) in &element.attrs {
                    if let Some(prefix) = prefix(name)
                        && prefix != "xmlns"
                        && prefix != "xml"
                        && !declared_prefixes.contains(&prefix)
                        && !name.ns.is_empty()
                    {
                        push_attribute(output, &format!("xmlns:{prefix}"), name.ns.as_ref());
                        declared_prefixes.push(prefix);
                    }
                }
                for (name, value) in &element.attrs {
                    let name = qualified_name(name);
                    if name == "xmlns" || name.starts_with("xmlns:") {
                        continue;
                    }
                    push_attribute(output, &name, value);
                }
                output.push('>');
                for child in node.children() {
                    push_node(output, child, namespace);
                }
                output.push_str("</");
                output.push_str(element.name.local.as_ref());
                output.push('>');
            }
            Node::Text(text) => write_escaped_xml(output, text),
            Node::Comment(comment) => {
                output.push_str("<!--");
                output.push_str(comment);
                output.push_str("-->");
            }
            Node::ProcessingInstruction(pi) => {
                output.push_str("<?");
                output.push_str(&pi.target);
                output.push(' ');
                output.push_str(&pi.data);
                output.push_str("?>");
            }
            Node::Fragment => {
                for child in node.children() {
                    push_node(output, child, inherited_namespace);
                }
            }
            _ => {}
        }
    }
    let mut output = String::new();
    push_node(&mut output, root, "");
    output
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

fn map_quirks_mode(mode: Html5everQuirksMode) -> HtmlQuirksMode {
    match mode {
        Html5everQuirksMode::NoQuirks => HtmlQuirksMode::NoQuirks,
        Html5everQuirksMode::LimitedQuirks => HtmlQuirksMode::LimitedQuirks,
        Html5everQuirksMode::Quirks => HtmlQuirksMode::Quirks,
    }
}
