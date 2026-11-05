//! Renderer-facing link, anchor, and source-correlation queries.

use crate::stages::pagination_queries;
use crate::stages::source_queries;
use crate::stages::text_paint;
use html_engine_model::layout_state;
use crate::stages::export_queries;
use crate::stages::search_queries;
use super::text::{enclosing_text_range, text_runs};
use super::*;
use kurbo::Point;
use crate::layout_model::GlyphMetrics;
use html_dom::{DomNodeId, element_is_note_reference};
use html_text::InlineTextKind;

/// A borrowed link destination and its activation semantics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkTarget<'a> {
    pub href: &'a str,
    pub is_note_reference: bool,
}

/// Text and optional surrounding context borrowed from a host-owned annotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextQuote<'a> {
    pub exact: &'a str,
    pub prefix: Option<&'a str>,
    pub suffix: Option<&'a str>,
}

/// Addressable text, a position-only anchor, or a note available through scoped layout.
#[derive(Clone, Debug, PartialEq)]
pub enum ElementTarget {
    Text {
        glyphs: Range<u32>,
        y: Option<f64>,
        is_note: bool,
    },
    Position {
        y: f64,
        is_note: bool,
    },
    /// A note without published text or position, available through scoped layout.
    Note,
}

/// Document-local input for resolving navigation geometry.
pub enum DocumentTarget<'a> {
    /// The beginning of the group containing this pagination cursor.
    Cursor(usize),
    NextLine(usize),
    PreviousLine(usize),
    /// Restore the source line covering a document offset.
    Offset(f64),
    /// The last pagination cursor and its group's bottom, or document zero.
    End,
    Glyph(u32),
    Fraction(f64),
    Element(&'a str),
    Source(&'a super::SourceLocation),
}

/// A resolved carrier retains identity even when independent boxes share y.
#[derive(Clone, Copy, Debug)]
pub enum ResolvedDestination {
    Line {
        line: LineId,
        cursor: usize,
        point: Point,
        glyph: Option<u32>,
    },
    Position {
        y: f64,
    },
}

impl ElementTarget {
    /// Whether this is a note target, including notes rendered in normal flow.
    #[inline]
    pub fn is_note(&self) -> bool {
        match self {
            Self::Text { is_note, .. } | Self::Position { is_note, .. } => *is_note,
            Self::Note => true,
        }
    }
}

pub(crate) struct RenderAnchorPosition {
    y: f64,
    order: u32,
}

impl RenderAnchorPosition {
    fn new(y: f64, order: u32) -> Self {
        Self { y, order }
    }

    pub(crate) fn y(&self) -> f64 {
        self.y
    }

    pub(crate) fn order(&self) -> u32 {
        self.order
    }
}

pub(crate) struct RenderAnchorPositions<'a> {
    anchors: &'a FxHashMap<u16, AnchorPosition>,
}

impl<'a> RenderAnchorPositions<'a> {
    fn new(anchors: &'a FxHashMap<u16, AnchorPosition>) -> Self {
        Self { anchors }
    }

    pub(crate) fn get(&self, id_idx: u16) -> Option<RenderAnchorPosition> {
        self.anchors
            .get(&id_idx)
            .copied()
            .map(|pos| RenderAnchorPosition::new(pos.y, pos.order))
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (u16, RenderAnchorPosition)> + '_ {
        self.anchors
            .iter()
            .map(|(&id_idx, &pos)| (id_idx, RenderAnchorPosition::new(pos.y, pos.order)))
    }
}

#[derive(Clone, Copy)]

pub struct RenderAddressingView<'a> {
    pub(super) doc: DocumentRead<'a>,
    pub(super) glyph_metrics: &'a GlyphMetrics,
}

pub use html_dom::{SourceElementStep, SourcePosition};

impl<'a> RenderAddressingView<'a> {
    /// Resolve document facts together; the host chooses how to reveal them.
    pub fn destination(self, target: DocumentTarget<'_>) -> Option<ResolvedDestination> {
        let view = self.doc.render_view(self.glyph_metrics);
        let glyph_destination = |glyph| {
            let line = text_paint::render_text_view_line_for_glyph(view.text(), glyph)?;
            Some(ResolvedDestination::Line {
                line,
                cursor: pagination_queries::pagination_cursor(
                    pagination_queries::render_view_pagination(view),
                    line,
                )?,
                point: view.text().line(line)?.point(),
                glyph: Some(glyph),
            })
        };
        match target {
            DocumentTarget::Cursor(cursor) => {
                let position = pagination_queries::render_view_pagination(view)
                    .locate(PaginationTarget::Cursor(cursor))?;
                let group = position.group;
                Some(ResolvedDestination::Line {
                    line: position.line.id(),
                    cursor: group.cursors().start,
                    point: group.point(),
                    glyph: None,
                })
            }
            DocumentTarget::NextLine(cursor) => {
                let lines = pagination_queries::render_view_pagination(view);
                let next = lines.groups(cursor..lines.len()).nth(1)?;
                self.destination(DocumentTarget::Cursor(next.cursors().start))
            }
            DocumentTarget::PreviousLine(cursor) => {
                let lines = pagination_queries::render_view_pagination(view);
                if cursor >= lines.len() {
                    return None;
                }
                let previous = lines.groups(0..cursor.saturating_add(1)).rev().nth(1)?;
                self.destination(DocumentTarget::Cursor(previous.cursors().start))
            }
            DocumentTarget::Offset(y) => {
                if view.text().lines().is_empty() {
                    return None;
                }
                let cursor = pagination_queries::render_view_pagination(view).start_at(y, 0.1);
                let line = pagination_queries::render_view_pagination(view).source_line(cursor)?;
                let glyphs = line.glyphs();
                Some(ResolvedDestination::Line {
                    line: line.id(),
                    cursor,
                    point: line.point(),
                    glyph: (!glyphs.is_empty()).then_some(glyphs.start),
                })
            }
            DocumentTarget::End => {
                let lines = pagination_queries::render_view_pagination(view);
                let Some(position) = lines
                    .len()
                    .checked_sub(1)
                    .and_then(|cursor| lines.locate(PaginationTarget::Cursor(cursor)))
                else {
                    return Some(ResolvedDestination::Position { y: 0.0 });
                };
                let mut point = position.group.point();
                point.y += position.group.height();
                Some(ResolvedDestination::Line {
                    line: position.line.id(),
                    cursor: position.cursor,
                    point,
                    glyph: None,
                })
            }
            DocumentTarget::Fraction(fraction) => {
                let count = view.text().glyph_count();
                let glyph = ((fraction * count as f64) as u32).min(count.saturating_sub(1) as u32);
                glyph_destination(glyph)
            }
            DocumentTarget::Glyph(glyph) => glyph_destination(glyph),
            DocumentTarget::Element(id) => match self.element(id)? {
                ElementTarget::Text { glyphs, .. } => glyph_destination(glyphs.start),
                ElementTarget::Position { y, .. } => Some(ResolvedDestination::Position { y }),
                ElementTarget::Note => None,
            },
            DocumentTarget::Source(source) => {
                if let Some(line) =
                    source_queries::render_addressing_view_source_element_line(self, source)
                    && let Some(cursor) = pagination_queries::pagination_cursor(
                        pagination_queries::render_view_pagination(view),
                        line,
                    )
                {
                    return Some(ResolvedDestination::Line {
                        line,
                        cursor,
                        point: view.text().line(line)?.point(),
                        glyph: None,
                    });
                }
                source_queries::render_addressing_view_source_element_y(self, source)
                    .map(|y| ResolvedDestination::Position { y })
            }
        }
    }

    /// Resolve an authored ID to text, a position, or a note held out of flow.
    pub fn element(self, id: &str) -> Option<ElementTarget> {
        let view = self.doc.render_view(self.glyph_metrics);
        let key = self.doc.document().lookup_string(id)?;
        let glyphs = self.glyph_range_for_anchor(id);
        let y = self
            .anchor_position(key)
            .map(|position| position.y())
            .or_else(|| {
                let glyph = self.anchor_glyph(key)?;
                let line = text_paint::render_text_view_line_for_glyph(view.text(), glyph)?;
                Some(view.text().line(line)?.point().y)
            });
        let is_note = self.is_note_target(id);
        match (glyphs, y) {
            (Some(glyphs), y) => Some(ElementTarget::Text { glyphs, y, is_note }),
            (None, Some(y)) => Some(ElementTarget::Position { y, is_note }),
            (None, None) => is_note.then_some(ElementTarget::Note),
        }
    }

    /// Reuse the host-decoded address when it still matches the quote.
    /// Otherwise locate the quoted text and context after source edits.
    pub fn resolve_range(
        self,
        preferred: Option<Range<u32>>,
        quote: TextQuote<'_>,
    ) -> Option<Range<u32>> {
        let view = self.doc.render_view(self.glyph_metrics);
        if let Some(range) = preferred
            && export_queries::render_export_view_text(view.export(), range.clone(), TextExportFormat::Plain).plain.as_deref() == Some(quote.exact)
        {
            return Some(range);
        }
        search_queries::render_text_view_locate_quote(view.text(), quote.exact, quote.prefix, quote.suffix)
    }

    /// Check published anchors without scanning text. A scoped-only note may
    /// still resolve through `element` when it has no published anchor.
    pub fn has_element(self, id: &str) -> bool {
        self.doc.document().lookup_string(id).is_some_and(|key| {
            self.anchor_glyph(key).is_some() || self.anchor_position(key).is_some()
        })
    }

    pub fn link_for_glyph(self, glyph_idx: u32) -> Option<LinkTarget<'a>> {
        self.doc.link_for_glyph(glyph_idx)
    }

    pub(crate) fn is_note_target(self, id: &str) -> bool {
        self.doc.target_is_note(id)
    }

    pub(crate) fn anchor_glyph(self, id_idx: u16) -> Option<u32> {
        self.doc.anchor_glyph(id_idx)
    }

    pub(crate) fn anchor_glyphs(self) -> impl Iterator<Item = (u16, u32)> + 'a {
        self.doc
            .anchor_glyphs()
            .iter()
            .map(|(&id_idx, &glyph_idx)| (id_idx, glyph_idx))
    }

    pub(crate) fn glyph_range_for_anchor(self, id: &str) -> Option<Range<u32>> {
        let document = self.doc.document();
        let target = document.element_by_id(id)?.node_id();
        text_runs(self.doc.inputs.text.content().inline_items())
            .filter_map(|run| {
                let node = document.node_id_from_raw(run.dom_text_node()?)?;
                let belongs = document.dom_contains(target, node);
                belongs.then(|| run.glyphs())
            })
            .reduce(enclosing_text_range)
            .filter(|range| !range.is_empty())
    }

    pub(crate) fn anchor_positions(self) -> RenderAnchorPositions<'a> {
        RenderAnchorPositions::new(&self.doc.layout_state.semantic_indexes.anchor_positions)
    }

    pub(crate) fn anchor_position(self, id_idx: u16) -> Option<RenderAnchorPosition> {
        self.anchor_positions().get(id_idx)
    }

    pub fn source_position_for_glyph(
        self,
        glyph_idx: u32,
        after_glyph: bool,
    ) -> Option<SourcePosition<'a>> {
        let local_offset = self.doc.glyph_source_offset(glyph_idx)? as usize;
        let run = self.doc.inputs.text.content().text_runs().find(|run| {
            run.kind == InlineTextKind::Text && run.glyphs.contains(&glyph_idx)
        })?;
        let document = self.doc.document();
        let text_node = document.node_id_from_raw(run.dom_text_node?)?;
        document.source_position_for_text_node(text_node, local_offset, after_glyph)
    }

    /// Resolves a generic element/text path to a source glyph boundary.
    /// Without a text step, uses the first addressable descendant in DOM order.
    /// Offsets are UTF-16 positions within that descendant's source text chunk.
    pub fn resolve_source_position(
        self,
        element_steps: &[usize],
        text_step: Option<usize>,
        utf16_offset: Option<usize>,
        allow_end: bool,
    ) -> Option<u32> {
        let document = self.doc.document();
        let parent = if element_steps.is_empty() {
            document.dom_root()?
        } else {
            document.find_dom_node_by_steps(element_steps)?
        };
        match text_step {
            Some(step) => find_glyph_in_text_chunk(self.doc, parent, step, utf16_offset, allow_end),
            None => find_glyph_in_node(self.doc, parent, utf16_offset, allow_end),
        }
    }
}

fn glyph_range_for_text_node(doc: DocumentRead<'_>, node: DomNodeId) -> Option<Range<u32>> {
    text_runs(doc.inputs.text.content().inline_items())
        .filter(|run| run.dom_text_node() == Some(node.raw()))
        .map(|run| run.glyphs())
        .filter(|range| !range.is_empty())
        .reduce(enclosing_text_range)
}

fn glyph_for_source_offset(
    doc: DocumentRead<'_>,
    range: Range<u32>,
    source_offset: Option<usize>,
    allow_end: bool,
) -> u32 {
    let Some(source_offset) = source_offset else {
        return range.start;
    };
    for glyph_idx in range.clone() {
        if doc
            .glyph_source_offset(glyph_idx)
            .is_some_and(|offset| offset as usize >= source_offset)
        {
            return glyph_idx;
        }
    }
    if allow_end {
        range.end
    } else {
        range.end.saturating_sub(1)
    }
}

fn find_glyph_in_text_chunk(
    doc: DocumentRead<'_>,
    parent: DomNodeId,
    text_step: usize,
    source_offset: Option<usize>,
    allow_end: bool,
) -> Option<u32> {
    let mut nodes = doc
        .document()
        .text_children_with_steps(parent)
        .filter(|(_, step)| *step == text_step)
        .peekable();
    let mut remaining = source_offset.unwrap_or(0);
    let mut last_end = None;
    while let Some((text, _)) = nodes.next() {
        let range = glyph_range_for_text_node(doc, text.node_id());
        if remaining == 0 {
            if let Some(range) = range {
                return Some(range.start);
            }
            continue;
        }
        let utf16_len = text.text().encode_utf16().count();
        let Some(range) = range else {
            remaining = remaining.saturating_sub(utf16_len);
            continue;
        };
        last_end = Some(range.end);
        if remaining < utf16_len
            || (remaining == utf16_len && (allow_end || nodes.peek().is_none()))
        {
            return Some(glyph_for_source_offset(
                doc,
                range,
                Some(remaining),
                allow_end,
            ));
        }
        remaining -= utf16_len;
    }
    last_end.map(|end| {
        if allow_end {
            end
        } else {
            end.saturating_sub(1)
        }
    })
}

fn find_glyph_in_node(
    doc: DocumentRead<'_>,
    node: DomNodeId,
    source_offset: Option<usize>,
    allow_end: bool,
) -> Option<u32> {
    match doc.document().node_ref(node)? {
        NodeRef::Text(text) => find_glyph_in_text_chunk(
            doc,
            text.parent()?,
            doc.document().get_text_node_step(node)?,
            source_offset,
            allow_end,
        ),
        NodeRef::Element(element) => element
            .children()
            .find_map(|child| find_glyph_in_node(doc, child, source_offset, allow_end)),
    }
}

/// Link semantics for replaced content, including images inside inline
/// ancestors that do not introduce their own layout box.
pub fn render_addressing_view_link_for_box<'a>(
    value: RenderAddressingView<'a>,
    box_id: BoxId,
) -> Option<LinkTarget<'a>> {
    let box_idx = layout_state::box_id_index(box_id);
    let document = value.doc.document();
    let node = document.node_id_from_raw(value.doc.box_dom_element_idx(box_idx)?)?;
    std::iter::once(node)
        .chain(document.dom_ancestors(node))
        .find_map(|node| {
            let element = document.element_ref(node)?;
            if !element.tag().eq_ignore_ascii_case("a")
                && !element.tag().eq_ignore_ascii_case("area")
            {
                return None;
            }
            Some(LinkTarget {
                href: element.attr("href")?,
                is_note_reference: element_is_note_reference(element),
            })
        })
}
