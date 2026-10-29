use crate::{
    HtmlFragmentContext, HtmlParserOptions, HtmlSyntaxNode, ParsedHtml, parse_html_fragment,
};
use html_dom::{
    Document, DocumentBuildError, DomNodeId, NodeRef, TextChildPosition, collapse_whitespace,
};
use std::collections::HashMap;
use std::ops::Range;

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
    paths: Vec<Box<[usize]>>,
}

struct SourceTextRun {
    text_start: usize,
    text_end: usize,
    byte_start: usize,
    byte_end: usize,
    path_index: usize,
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
            paths: Vec::new(),
            parent_paths: HashMap::new(),
            text_offsets: HashMap::new(),
            pending_space: false,
        };
        if let Some(root) = document.dom_root() {
            builder.visit(root);
        }
        Ok(Self { text: builder.text, runs: builder.runs, paths: builder.paths })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Number of Unicode scalar values in the collapsed text.
    pub fn character_count(&self) -> usize {
        self.runs.last().map_or(0, |run| run.text_end)
    }

    /// Borrow a character-indexed range without retaining a second text copy.
    /// Endpoints are located in the run index, then scanned within that run.
    pub fn text_range(&self, range: Range<usize>) -> Option<&str> {
        if range.start > range.end {
            return None;
        }
        self.text
            .get(self.byte_offset(range.start)?..self.byte_offset(range.end)?)
    }

    fn byte_offset(&self, position: usize) -> Option<usize> {
        if position == 0 {
            return Some(0);
        }
        // A collapsed separator is one ASCII space between source runs. Its
        // start is the preceding run's end; its end is the next run's start.
        let run = self
            .runs
            .get(self.runs.partition_point(|run| run.text_end < position))?;
        if position == run.text_end {
            return Some(run.byte_end);
        }
        let local = position.checked_sub(run.text_start)?;
        let text = &self.text[run.byte_start..run.byte_end];
        if text.len() == run.text_end - run.text_start {
            return Some(run.byte_start + local);
        }
        text.char_indices()
            .nth(local)
            .map(|(offset, _)| run.byte_start + offset)
    }

    /// Returns the first addressable source character in a matched interval.
    /// Inserted block-boundary spaces deliberately carry no source address.
    pub fn source_position_in(&self, range: Range<usize>) -> Option<SourceTextPosition> {
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
            element_steps: self.paths[run.path_index].to_vec(),
            text_step: run.text_step,
            utf16_offset: run.utf16_offset + source_offset,
        })
    }

    pub fn source_run_count(&self) -> usize {
        self.runs.len()
    }
    pub fn source_path_count(&self) -> usize {
        self.paths.len()
    }
}

struct TextIndexBuilder<'a> {
    document: &'a Document,
    text: String,
    text_chars: usize,
    runs: Vec<SourceTextRun>,
    paths: Vec<Box<[usize]>>,
    parent_paths: HashMap<DomNodeId, usize>,
    text_offsets: HashMap<DomNodeId, TextChildPosition>,
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
        if !self.text_offsets.contains_key(&node) {
            self.text_offsets.extend(
                self.document.text_child_positions(parent)
                    .map(|position| (position.node, position)),
            );
        }
        let Some(position) = self.text_offsets.get(&node) else {
            return;
        };
        let text_step = position.text_step;
        let current_chunk_offset = position.utf16_range.start;
        let path_index = if let Some(&index) = self.parent_paths.get(&parent) {
            index
        } else {
            let Some(path) = self.document.element_source_path(parent) else {
                return;
            };
            let index = self.paths.len();
            self.paths.push(path.into_iter().map(|element| element.step()).collect());
            self.parent_paths.insert(parent, index);
            index
        };
        let mut local_utf16 = 0usize;
        let mut run_start = None;
        for character in value.chars() {
            if character.is_whitespace() {
                self.finish_run(&mut run_start, path_index, text_step);
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
        self.finish_run(&mut run_start, path_index, text_step);
    }

    fn finish_run(
        &mut self,
        start: &mut Option<(usize, usize, usize)>,
        path_index: usize,
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
            path_index,
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
    collapse_whitespace(&text)
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

#[cfg(test)]
mod tests {
    use super::DocumentTextIndex;
    use crate::parse_html_document;

    #[test]
    fn character_ranges_cross_unicode_runs_and_collapsed_separators() {
        for source in [
            "",
            "<p> \n\t </p>",
            "<p>Aé😀e\u{301}</p>",
            "<p>a<!-- split -->b<em>é</em>c</p><p>d\u{a0}e</p>",
            "<p>😀 \n β</p><br><div>z<!-- split -->終</div>",
        ] {
            let index = DocumentTextIndex::from_parsed(&parse_html_document(source)).unwrap();
            let characters: Vec<_> = index.text().chars().collect();
            assert_eq!(index.character_count(), characters.len());
            for start in 0..=characters.len() {
                for end in start..=characters.len() {
                    let expected: String = characters[start..end].iter().collect();
                    assert_eq!(
                        index.text_range(start..end),
                        Some(expected.as_str()),
                        "{source}: {start}..{end}"
                    );
                }
            }
            assert_eq!(index.text_range(0..characters.len() + 1), None);
            assert_eq!(index.text_range(usize::MAX..usize::MAX), None);
            assert_eq!(index.text_range(1..0), None);
        }
    }

    #[test]
    fn cached_parent_paths_keep_sibling_chunks_and_nested_paths_distinct() {
        let parsed = parse_html_document(
            "<p>a<!-- split -->b<span>x</span>c<!-- split -->d</p><p>e<!-- split -->f</p>",
        );
        let index = DocumentTextIndex::from_parsed(&parsed).unwrap();
        assert_eq!(index.text(), "abxcd ef");
        assert_eq!(index.source_path_count(), 3);
        let positions =
            [0, 1, 2, 3, 4, 6, 7].map(|start| index.source_position_in(start..start + 1).unwrap());
        for i in [1, 3, 4] {
            assert_eq!(positions[0].element_steps(), positions[i].element_steps());
        }
        assert_ne!(positions[0].element_steps(), positions[2].element_steps());
        assert_ne!(positions[0].element_steps(), positions[5].element_steps());
        assert_eq!(positions[5].element_steps(), positions[6].element_steps());
        assert_eq!(
            positions
                .iter()
                .map(|position| (position.text_step(), position.utf16_offset()))
                .collect::<Vec<_>>(),
            [(1, 0), (1, 1), (1, 0), (3, 0), (3, 1), (1, 0), (1, 1)]
        );
    }
}
