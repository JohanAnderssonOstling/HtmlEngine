//! Renderer-facing link, anchor, and source-correlation queries.

use super::text::text_runs;
use super::*;
use crate::layout_model::GlyphMetrics;
use html_dom::{DomNodeId, element_is_note_reference};

pub struct RenderAnchorPosition {
    y: f64,
    order: u32,
}

impl RenderAnchorPosition {
    fn new(y: f64, order: u32) -> Self {
        Self { y, order }
    }

    pub fn y(&self) -> f64 {
        self.y
    }

    pub fn order(&self) -> u32 {
        self.order
    }
}

pub struct RenderAnchorPositions<'a> {
    anchors: &'a FxHashMap<u16, AnchorPosition>,
}

impl<'a> RenderAnchorPositions<'a> {
    fn new(anchors: &'a FxHashMap<u16, AnchorPosition>) -> Self {
        Self { anchors }
    }

    pub fn get(&self, id_idx: u16) -> Option<RenderAnchorPosition> {
        self.anchors
            .get(&id_idx)
            .copied()
            .map(|pos| RenderAnchorPosition::new(pos.y, pos.order))
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, RenderAnchorPosition)> + '_ {
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
    /// Link semantics for replaced content, including images inside inline
    /// ancestors that do not introduce their own layout box.
    pub fn link_for_box(self, box_idx: usize) -> Option<(&'a str, bool)> {
        let document = self.doc.document();
        let node = document.node_id_from_raw(self.doc.box_dom_element_idx(box_idx)?)?;
        std::iter::once(node)
            .chain(document.dom_ancestors(node))
            .find_map(|node| {
                let element = document.element_ref(node)?;
                if !element.tag().eq_ignore_ascii_case("a")
                    && !element.tag().eq_ignore_ascii_case("area")
                {
                    return None;
                }
                Some((element.attr("href")?, element_is_note_reference(element)))
            })
    }

    pub fn link_for_glyph(self, glyph_idx: u32) -> Option<u16> {
        self.doc.link_href_for_glyph(glyph_idx)
    }

    pub fn is_note_reference(self, glyph_idx: u32) -> bool {
        self.doc.glyph_is_note_reference(glyph_idx)
    }

    pub fn is_note_target(self, id: &str) -> bool {
        self.doc.target_is_note(id)
    }

    pub fn anchor_glyph(self, id_idx: u16) -> Option<u32> {
        self.doc.anchor_glyph(id_idx)
    }

    pub fn anchor_glyphs(self) -> impl Iterator<Item = (u16, u32)> + 'a {
        self.doc
            .anchor_glyphs()
            .iter()
            .map(|(&id_idx, &glyph_idx)| (id_idx, glyph_idx))
    }

    pub fn glyph_range_for_anchor(self, id: &str) -> Option<Range<u32>> {
        let document = self.doc.document();
        let target = document
            .node_ids()
            .find(|node| document.get_dom_id(*node) == Some(id))?;
        let mut start = u32::MAX;
        let mut end = 0u32;
        for run in text_runs(self.doc.inputs.text.content().inline_items()) {
            let Some(node) = run
                .dom_text_node()
                .and_then(|raw| document.node_id_from_raw(raw))
            else {
                continue;
            };
            let belongs = node == target
                || document
                    .dom_ancestors(node)
                    .any(|ancestor| ancestor == target);
            if belongs {
                let glyphs = run.glyphs();
                start = start.min(glyphs.start);
                end = end.max(glyphs.end);
            }
        }
        (start < end).then_some(start..end)
    }

    pub fn anchor_positions(self) -> RenderAnchorPositions<'a> {
        RenderAnchorPositions::new(&self.doc.layout_state.semantic_indexes.anchor_positions)
    }

    pub fn anchor_position(self, id_idx: u16) -> Option<RenderAnchorPosition> {
        self.anchor_positions().get(id_idx)
    }

    pub fn source_position_for_glyph(
        self,
        glyph_idx: u32,
        after_glyph: bool,
    ) -> Option<SourcePosition<'a>> {
        let (text_node_idx, local_offset) = self.doc.get_dom_node_for_glyph(glyph_idx)?;
        let document = self.doc.document();
        let text_node = document.node_id_from_raw(text_node_idx)?;
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
            Some(step) => find_glyph_in_text_chunk(
                self.doc,
                parent,
                step,
                utf16_offset,
                allow_end,
                self.glyph_metrics,
            ),
            None => find_glyph_in_node(
                self.doc,
                parent,
                utf16_offset,
                allow_end,
                self.glyph_metrics,
            ),
        }
    }
}

fn glyph_range_for_text_node(
    doc: DocumentRead<'_>,
    node: DomNodeId,
    glyph_metrics: &GlyphMetrics,
) -> Option<Range<u32>> {
    let mut runs = doc
        .render_view(glyph_metrics)
        .text()
        .text_runs()
        .filter(|run| run.dom_text_node() == Some(node.raw()))
        .map(|run| run.glyphs())
        .filter(|range| !range.is_empty());
    let mut range = runs.next()?;
    for run in runs {
        range.start = range.start.min(run.start);
        range.end = range.end.max(run.end);
    }
    Some(range)
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
    glyph_metrics: &GlyphMetrics,
) -> Option<u32> {
    let mut nodes = doc
        .document()
        .text_children_with_steps(parent)
        .filter(|(_, step)| *step == text_step)
        .peekable();
    let mut remaining = source_offset.unwrap_or(0);
    let mut last_end = None;
    while let Some((text, _)) = nodes.next() {
        let range = glyph_range_for_text_node(doc, text.node_id(), glyph_metrics);
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
    glyph_metrics: &GlyphMetrics,
) -> Option<u32> {
    match doc.document().node_ref(node)? {
        NodeRef::Text(text) => find_glyph_in_text_chunk(
            doc,
            text.parent()?,
            doc.document().get_text_node_step(node)?,
            source_offset,
            allow_end,
            glyph_metrics,
        ),
        NodeRef::Element(element) => element.children().find_map(|child| {
            find_glyph_in_node(doc, child, source_offset, allow_end, glyph_metrics)
        }),
    }
}
