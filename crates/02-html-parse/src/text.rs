use crate::{
    HtmlFragmentContext, HtmlParserOptions, HtmlSyntaxNode, ParsedHtml, parse_html_fragment,
};
use html_dom::{Document, DocumentBuildError, DomNodeId, NodeRef};
use std::collections::HashMap;
use std::sync::Arc;

/// A durable DOM address for one character in parse-level plain text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTextPosition {
    element_steps: Vec<usize>,
    text_step: usize,
    utf16_offset: usize,
}

impl SourceTextPosition {
    pub fn element_steps(&self) -> &[usize] {
        &self.element_steps
    }
    pub fn text_step(&self) -> usize {
        self.text_step
    }
    pub fn utf16_offset(&self) -> usize {
        self.utf16_offset
    }
}

/// Whitespace-collapsed document text paired with source addresses. It is
/// intentionally parse-only: whole-book search can build one document at a
/// time without style, layout, fonts, or images.
pub struct DocumentTextIndex {
    text: String,
    runs: Vec<SourceTextRun>,
    path_count: usize,
}

struct SourceTextRun {
    text_start: usize,
    text_end: usize,
    byte_start: usize,
    byte_end: usize,
    element_steps: Arc<[usize]>,
    text_step: usize,
    utf16_offset: usize,
}

impl DocumentTextIndex {
    pub fn from_parsed(parsed: &ParsedHtml) -> Result<Self, DocumentBuildError> {
        let document = parsed.build_dom_checked()?;
        let mut builder = TextIndexBuilder {
            document: &document,
            text: String::new(),
            text_chars: 0,
            runs: Vec::new(),
            paths: HashMap::new(),
            text_offsets: HashMap::new(),
            pending_space: false,
        };
        if let Some(root) = document.dom_root() {
            builder.visit(root);
        }
        Ok(Self {
            text: builder.text,
            runs: builder.runs,
            path_count: builder.paths.len(),
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the first addressable source character in a matched interval.
    /// Inserted block-boundary spaces deliberately carry no source address.
    pub fn source_position_in(&self, range: std::ops::Range<usize>) -> Option<SourceTextPosition> {
        let run_index = self.runs.partition_point(|run| run.text_end <= range.start);
        let run = self
            .runs
            .get(run_index)
            .filter(|run| run.text_start < range.end)?;
        let text_offset = range.start.max(run.text_start) - run.text_start;
        let source_offset = self.text[run.byte_start..run.byte_end]
            .chars()
            .take(text_offset)
            .map(char::len_utf16)
            .sum::<usize>();
        Some(SourceTextPosition {
            element_steps: run.element_steps.to_vec(),
            text_step: run.text_step,
            utf16_offset: run.utf16_offset + source_offset,
        })
    }

    pub fn source_run_count(&self) -> usize {
        self.runs.len()
    }
    pub fn source_path_count(&self) -> usize {
        self.path_count
    }
}

struct TextIndexBuilder<'a> {
    document: &'a Document,
    text: String,
    text_chars: usize,
    runs: Vec<SourceTextRun>,
    paths: HashMap<Vec<usize>, Arc<[usize]>>,
    text_offsets: HashMap<(DomNodeId, usize), usize>,
    pending_space: bool,
}

impl TextIndexBuilder<'_> {
    fn visit(&mut self, node: DomNodeId) {
        match self.document.node_ref(node) {
            Some(NodeRef::Text(text)) => self.push_text(node, text.text()),
            Some(NodeRef::Element(element)) => {
                let tag = element.tag();
                if matches!(tag, "script" | "style" | "template" | "noscript" | "head")
                    || html_dom::element_is_note_target(element)
                {
                    return;
                }
                let boundary = is_text_boundary_element(tag);
                if boundary {
                    self.pending_space = true;
                }
                for child in element.children() {
                    self.visit(child);
                }
                if boundary {
                    self.pending_space = true;
                }
            }
            None => {}
        }
    }

    fn push_text(&mut self, node: DomNodeId, value: &str) {
        let Some(parent) = self.document.node_ref(node).and_then(NodeRef::parent) else {
            return;
        };
        let Some(text_step) = self.document.get_text_node_step(node) else {
            return;
        };
        let chunk_offset = self.text_offsets.entry((parent, text_step)).or_default();
        let current_chunk_offset = *chunk_offset;
        *chunk_offset += value.encode_utf16().count();
        let path = self.document.get_dom_path_to_node(parent);
        let Some(element_steps) = path
            .into_iter()
            .skip(1)
            .map(|element| self.document.get_element_step(element))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let element_steps = self
            .paths
            .entry(element_steps.clone())
            .or_insert_with(|| Arc::from(element_steps))
            .clone();
        let mut local_utf16 = 0usize;
        let mut run_start = None;
        for character in value.chars() {
            if character.is_whitespace() {
                self.finish_run(&mut run_start, element_steps.clone(), text_step);
                self.pending_space = true;
            } else {
                if self.pending_space && !self.text.is_empty() {
                    self.text.push(' ');
                    self.text_chars += 1;
                }
                self.pending_space = false;
                run_start.get_or_insert((
                    self.text_chars,
                    self.text.len(),
                    current_chunk_offset + local_utf16,
                ));
                self.text.push(character);
                self.text_chars += 1;
            }
            local_utf16 += character.len_utf16();
        }
        self.finish_run(&mut run_start, element_steps, text_step);
    }

    fn finish_run(
        &mut self,
        start: &mut Option<(usize, usize, usize)>,
        element_steps: Arc<[usize]>,
        text_step: usize,
    ) {
        let Some((text_start, byte_start, utf16_offset)) = start.take() else {
            return;
        };
        self.runs.push(SourceTextRun {
            text_start,
            text_end: self.text_chars,
            byte_start,
            byte_end: self.text.len(),
            element_steps,
            text_step,
            utf16_offset,
        });
    }
}

pub fn plain_text_from_fragment(html: &str) -> String {
    let parsed = parse_html_fragment(
        html,
        &HtmlFragmentContext::html("div"),
        HtmlParserOptions::default(),
    );
    let mut text = String::new();
    append_plain_text(&parsed.syntax_tree().nodes, &mut text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn append_plain_text(nodes: &[HtmlSyntaxNode], output: &mut String) {
    for node in nodes {
        match node {
            HtmlSyntaxNode::Text(text) => output.push_str(text),
            HtmlSyntaxNode::Element(element)
                if matches!(
                    element.local_name.as_str(),
                    "script" | "style" | "template" | "noscript" | "head"
                ) => {}
            HtmlSyntaxNode::Element(element) => {
                let block = is_text_boundary_element(&element.local_name);
                if block && !output.ends_with(char::is_whitespace) {
                    output.push(' ');
                }
                append_plain_text(&element.children, output);
                if block && !output.ends_with(char::is_whitespace) {
                    output.push(' ');
                }
            }
            HtmlSyntaxNode::TemplateContents(_)
            | HtmlSyntaxNode::Doctype { .. }
            | HtmlSyntaxNode::Comment(_)
            | HtmlSyntaxNode::ProcessingInstruction { .. } => {}
        }
    }
}

fn is_text_boundary_element(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "br"
            | "dd"
            | "details"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "li"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "summary"
            | "table"
            | "tbody"
            | "td"
            | "tfoot"
            | "th"
            | "thead"
            | "tr"
            | "ul"
    )
}
