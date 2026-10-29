//! Typed document meaning, independent of reader interaction and pagination policy.

use super::DocumentRead;
use html_dom::{element_is_note_reference, element_is_note_target};
use std::iter::once;
use std::ops::Range;

/// Structural roles derived from markup and resolved layout. Roles may overlap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ContentRole {
    Heading,
    List,
    ListItem,
    CodeBlock,
    InlineCode,
    Figure,
    Caption,
    Quote,
    DefinitionTerm,
    DefinitionDescription,
    Table,
    TableCell,
    TableHeaderCell,
    Sidebar,
    MajorSection,
    CompactStructure,
    Statement,
    Label,
    Note,
    NoteReference,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContentSemantics {
    roles: u32,
    heading_level: Option<u8>,
}

impl ContentSemantics {
    pub fn has_role(self, role: ContentRole) -> bool {
        self.roles & (1 << role as u8) != 0
    }
    pub fn heading_level(self) -> Option<u8> {
        self.heading_level
    }
    fn insert(&mut self, role: ContentRole) {
        self.roles |= 1 << role as u8;
    }

    fn add_book_tokens<'a>(&mut self, tokens: impl IntoIterator<Item = &'a str>, aria: bool) {
        for token in tokens {
            let exact = if aria { doc_role(token) } else { Some(token) };
            if token_is(
                exact,
                &[
                    "chapter",
                    "part",
                    "appendix",
                    "prologue",
                    "epilogue",
                    "foreword",
                    "preface",
                    "introduction",
                    "conclusion",
                    "acknowledgments",
                    "afterword",
                    "bibliography",
                    "glossary",
                    "index",
                    "colophon",
                ],
            ) || (!aria
                && token_is(
                    exact,
                    &[
                        "titlepage",
                        "halftitlepage",
                        "copyright-page",
                        "frontmatter",
                        "bodymatter",
                        "backmatter",
                    ],
                ))
            {
                self.insert(ContentRole::MajorSection);
            }
            // Compact structures accept vocabulary prefixes; chapter/sidebar
            // roles above intentionally require exact tokens.
            let suffix = token.rsplit(':').next().unwrap_or_default();
            let kind = if aria { doc_role(suffix) } else { Some(suffix) };
            let statement = token_is(kind, &["example"])
                || (!aria && token_is(kind, &["theorem", "proof", "exercise"]));
            if statement {
                self.insert(ContentRole::Statement);
            }
            if statement
                || token_is(
                    kind,
                    &[
                        "epigraph",
                        "biblioentry",
                        "dedication",
                        "abstract",
                        "notice",
                        "pullquote",
                    ],
                )
                || (!aria && token_is(kind, &["poem", "verse", "stanza"]))
            {
                self.insert(ContentRole::CompactStructure);
            }
            if !aria && token_is(kind, &["label"]) {
                self.insert(ContentRole::Label);
            }
        }
    }
}

/// Semantic queries use source structure without exposing tag or attribute rules.
#[derive(Clone, Copy)]
pub struct RenderSemanticsView<'a> {
    pub(super) doc: DocumentRead<'a>,
    pub(super) glyph_metrics: &'a crate::layout_model::GlyphMetrics,
}

impl RenderSemanticsView<'_> {
    pub fn content(self, box_idx: usize) -> ContentSemantics {
        let boxes = self.doc.render_view(self.glyph_metrics).boxes();
        let mut result = ContentSemantics::default();
        if boxes.is_table(box_idx) {
            result.insert(ContentRole::Table);
        }
        if boxes.is_table_cell(box_idx) {
            result.insert(ContentRole::TableCell);
        }
        let Some(element) = self
            .doc
            .box_dom_element_idx(box_idx)
            .and_then(|raw| self.doc.document().node_id_from_raw(raw))
            .and_then(|node| self.doc.document().element_ref(node))
        else {
            return result;
        };
        let tag = element.tag();
        let epub_types = element.epub_types();
        let aria_roles = element.aria_roles();
        if let Some(level) = ["h1", "h2", "h3", "h4", "h5", "h6"]
            .iter()
            .position(|heading| tag.eq_ignore_ascii_case(heading))
        {
            result.heading_level = Some(level as u8 + 1);
            result.insert(ContentRole::Heading);
            result.insert(ContentRole::Label);
        }
        for (name, role) in [
            ("ul", ContentRole::List),
            ("ol", ContentRole::List),
            ("li", ContentRole::ListItem),
            ("pre", ContentRole::CodeBlock),
            ("code", ContentRole::InlineCode),
            ("figure", ContentRole::Figure),
            ("figcaption", ContentRole::Caption),
            ("blockquote", ContentRole::Quote),
            ("th", ContentRole::TableHeaderCell),
            ("dt", ContentRole::DefinitionTerm),
            ("dd", ContentRole::DefinitionDescription),
        ] {
            if tag.eq_ignore_ascii_case(name) {
                result.insert(role);
            }
        }
        if tag.eq_ignore_ascii_case("code") && boxes.is_block_container(box_idx) {
            result.insert(ContentRole::CodeBlock);
        }
        if element_is_note_target(element) {
            result.insert(ContentRole::Note);
        }
        if element_is_note_reference(element) {
            result.insert(ContentRole::NoteReference);
        }
        result.add_book_tokens(epub_types.clone(), false);
        result.add_book_tokens(aria_roles.clone(), true);
        if !result.has_role(ContentRole::Note)
            && (tag.eq_ignore_ascii_case("aside")
                || has_token(epub_types, "sidebar")
                || has_token(aria_roles, "complementary"))
        {
            result.insert(ContentRole::Sidebar);
        }
        result
    }

    pub fn has_role(self, box_idx: usize, role: ContentRole) -> bool {
        self.content(box_idx).has_role(role)
    }

    pub fn nearest(self, box_idx: usize, role: ContentRole) -> Option<usize> {
        let boxes = self.doc.render_view(self.glyph_metrics).boxes();
        (box_idx < boxes.len()).then_some(())?;
        once(box_idx)
            .chain(boxes.ancestors(box_idx))
            .find(|&index| self.has_role(index, role))
    }

    pub fn block_container(self, box_idx: usize) -> Option<usize> {
        let boxes = self.doc.render_view(self.glyph_metrics).boxes();
        (box_idx < boxes.len()).then_some(())?;
        let mut ancestors = once(box_idx).chain(boxes.ancestors(box_idx));
        ancestors.find(|&index| boxes.is_block_container(index) || boxes.parent(index).is_none())
    }

    /// Whether `descendant` belongs to `ancestor`, including the box itself.
    pub fn contains(self, ancestor: usize, descendant: usize) -> bool {
        let boxes = self.doc.render_view(self.glyph_metrics).boxes();
        ancestor < boxes.len()
            && descendant < boxes.len()
            && once(descendant)
                .chain(boxes.ancestors(descendant))
                .any(|index| index == ancestor)
    }

    /// Logical text ranges belonging to a box. Generated list markers are
    /// included only when requested for semantic-selection highlighting.
    pub fn text_ranges(self, box_idx: usize, include_marker: bool) -> Vec<Range<u32>> {
        let view = self.doc.render_view(self.glyph_metrics);
        let mut ranges = view
            .text()
            .text_runs()
            .filter(|run| self.contains(box_idx, run.box_idx()))
            .map(|run| run.glyphs())
            .collect::<Vec<_>>();
        if include_marker && let Some(marker) = view.boxes().list_marker(box_idx) {
            ranges.extend(
                view.text()
                    .marker_runs()
                    .only_box(marker.marker_box())
                    .map(|run| run.glyphs()),
            );
        }
        ranges.sort_by_key(|range| range.start);
        ranges
    }
}

fn has_token<'a>(tokens: impl IntoIterator<Item = &'a str>, expected: &str) -> bool {
    tokens
        .into_iter()
        .any(|token| token.eq_ignore_ascii_case(expected))
}

fn token_is(token: Option<&str>, candidates: &[&str]) -> bool {
    token.is_some_and(|token| {
        candidates
            .iter()
            .any(|candidate| token.eq_ignore_ascii_case(candidate))
    })
}

fn doc_role(token: &str) -> Option<&str> {
    token
        .get(..4)?
        .eq_ignore_ascii_case("doc-")
        .then(|| &token[4..])
}
