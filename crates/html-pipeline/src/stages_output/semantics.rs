//! Typed document meaning, independent of reader interaction and pagination policy.

use html_engine_model::layout_state;
use super::{BoxId, DocumentRead, RenderView};
use super::{box_queries, text_paint};
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
    pub fn content(self, box_id: BoxId) -> ContentSemantics {
        let box_idx = layout_state::box_id_index(box_id);
        let boxes = self.doc.render_view(self.glyph_metrics).boxes();
        let mut result = ContentSemantics::default();
        if box_queries::is_table(boxes, box_id) {
            result.insert(ContentRole::Table);
        }
        if box_queries::is_table_cell(boxes, box_id) {
            result.insert(ContentRole::TableCell);
        }
        let Some(element) = self.doc.element_for_box(box_idx) else {
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
        if tag.eq_ignore_ascii_case("code")
            && box_queries::render_box_view_is_block_container(boxes, box_id)
        {
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

/// Logical text ranges belonging to a box. Generated list markers are
/// included only when requested for semantic-selection highlighting.
pub fn render_semantics_view_text_ranges(
    value: RenderSemanticsView<'_>,
    box_id: BoxId,
    include_marker: bool,
) -> Vec<Range<u32>> {
    let view = value.doc.render_view(value.glyph_metrics);
    let mut ranges = text_paint::render_text_view_text_runs(view.text())
        .filter(|run| {
            render_semantics_view_contains(value, box_id, text_paint::render_text_run_box_id(&run))
        })
        .map(|run| run.glyphs())
        .collect::<Vec<_>>();
    if include_marker && let Some(marker) = box_queries::list_marker(view.boxes(), box_id) {
        ranges.extend(
            text_paint::marker_runs(view.text())
                .only_box(marker.marker_box())
                .map(|run| run.glyphs()),
        );
    }
    ranges.sort_by_key(|range| range.start);
    ranges
}

pub fn content_semantics_heading_level(value: ContentSemantics) -> Option<u8> {
    value.heading_level
}

pub fn render_semantics_view_has_role(
    value: RenderSemanticsView<'_>,
    box_id: BoxId,
    role: ContentRole,
) -> bool {
    value.content(box_id).has_role(role)
}

/// Whether `descendant` belongs to `ancestor`, including the box itself.
pub fn render_semantics_view_contains(
    value: RenderSemanticsView<'_>,
    ancestor_id: BoxId,
    descendant_id: BoxId,
) -> bool {
    let ancestor = layout_state::box_id_index(ancestor_id);
    let descendant = layout_state::box_id_index(descendant_id);
    let boxes = value.doc.render_view(value.glyph_metrics).boxes();
    ancestor < box_queries::len(boxes)
        && descendant < box_queries::len(boxes)
        && once(descendant_id)
            .chain(box_queries::ancestors(boxes, descendant_id))
            .any(|id| id == ancestor_id)
}

pub fn render_semantics_view_nearest(
    value: RenderSemanticsView<'_>,
    box_id: BoxId,
    role: ContentRole,
) -> Option<BoxId> {
    let boxes = value.doc.render_view(value.glyph_metrics).boxes();
    (layout_state::box_id_index(box_id) < box_queries::len(boxes)).then_some(())?;
    once(box_id)
        .chain(box_queries::ancestors(boxes, box_id))
        .find(|&index| render_semantics_view_has_role(value, index, role))
}

pub fn render_semantics_view_block_container<'a>(
    value: RenderSemanticsView<'a>,
    box_id: BoxId,
) -> Option<BoxId> {
    let boxes = value.doc.render_view(value.glyph_metrics).boxes();
    (layout_state::box_id_index(box_id) < box_queries::len(boxes)).then_some(())?;
    let mut ancestors = once(box_id).chain(box_queries::ancestors(boxes, box_id));
    ancestors.find(|&index| {
        box_queries::render_box_view_is_block_container(boxes, index)
            || box_queries::parent(boxes, index).is_none()
    })
}

pub fn render_view_semantics<'a>(value: RenderView<'a>) -> RenderSemanticsView<'a> {
    RenderSemanticsView {
        doc: value.doc,
        glyph_metrics: value.glyph_metrics,
    }
}
