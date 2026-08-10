//! Renderer-facing link, anchor, and source-correlation queries.

use super::text::text_runs;
use super::*;

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
        self.anchors.get(&id_idx).copied().map(|pos| RenderAnchorPosition::new(pos.y, pos.order))
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, RenderAnchorPosition)> + '_ {
        self.anchors.iter().map(|(&id_idx, &pos)| (id_idx, RenderAnchorPosition::new(pos.y, pos.order)))
    }
}

#[derive(Clone, Copy)]

pub struct RenderAddressingView<'a> {
    pub(super) doc: &'a LaidOutDocument,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceElementStep<'a> {
    step: usize,
    id: Option<&'a str>,
}

impl<'a> SourceElementStep<'a> {
    pub fn step(self) -> usize {
        self.step
    }

    pub fn id(self) -> Option<&'a str> {
        self.id
    }
}

/// Generic source position corresponding to a laid-out glyph boundary.
/// Publication-specific syntaxes such as EPUB CFI are built outside layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePosition<'a> {
    elements: Vec<SourceElementStep<'a>>,
    text_step: usize,
    utf16_offset: usize,
}

impl<'a> SourcePosition<'a> {
    pub fn elements(&self) -> impl ExactSizeIterator<Item = SourceElementStep<'a>> + '_ {
        self.elements.iter().copied()
    }

    pub fn text_step(&self) -> usize {
        self.text_step
    }

    pub fn utf16_offset(&self) -> usize {
        self.utf16_offset
    }
}

impl<'a> RenderAddressingView<'a> {
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
        self.doc.anchor_glyphs().iter().map(|(&id_idx, &glyph_idx)| (id_idx, glyph_idx))
    }

    pub fn glyph_range_for_anchor(self, id: &str) -> Option<Range<u32>> {
        let document = self.doc.document();
        let target = document.node_ids().find(|node| document.get_dom_id(*node) == Some(id))?;
        let mut start = u32::MAX;
        let mut end = 0u32;
        for run in text_runs(self.doc.shaped.inline_content.inline_items()) {
            let Some(mut node) = run.dom_text_node().and_then(|raw| document.node_id_from_raw(raw)) else {
                continue;
            };
            let mut belongs = node == target;
            while !belongs {
                let Some(parent) = document.get_dom_parent(node) else {
                    break;
                };
                node = parent;
                belongs = node == target;
            }
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

    pub fn source_position_for_glyph(self, glyph_idx: u32, after_glyph: bool) -> Option<SourcePosition<'a>> {
        let (text_node_idx, local_offset) = self.doc.get_dom_node_for_glyph(glyph_idx)?;
        let document = self.doc.document();
        let text_node = document.node_id_from_raw(text_node_idx)?;
        let parent = document.node_ref(text_node)?.parent()?;
        let text_step = document.get_text_node_step(text_node)?;
        let mut chunk_offset = 0usize;
        for node in document.find_text_nodes_by_step(parent, text_step) {
            if node == text_node {
                break;
            }
            chunk_offset += document.text_ref(node)?.text().encode_utf16().count();
        }
        let local_offset = if after_glyph { utf16_offset_after_character(document.text_ref(text_node)?.text(), local_offset)? } else { local_offset };
        let path = document.get_dom_path_to_node(parent);
        if path.is_empty() {
            return None;
        }
        let elements = path.into_iter().skip(1).map(|node| Some(SourceElementStep { step: document.get_element_step(node)?, id: document.get_dom_id(node) })).collect::<Option<Vec<_>>>()?;
        Some(SourcePosition { elements, text_step, utf16_offset: chunk_offset + local_offset })
    }

    /// Resolves a generic element/text path to a source glyph boundary.
    pub fn resolve_source_position(self, element_steps: &[usize], text_step: Option<usize>, utf16_offset: Option<usize>, allow_end: bool) -> Option<u32> {
        let document = self.doc.document();
        let parent = if element_steps.is_empty() { document.dom_root()? } else { document.find_dom_node_by_steps(element_steps)? };
        match text_step {
            Some(step) => find_glyph_in_text_chunk(self.doc, parent, step, utf16_offset, allow_end),
            None => find_glyph_in_node(self.doc, parent, utf16_offset, allow_end),
        }
    }
}

fn utf16_offset_after_character(text: &str, wanted: usize) -> Option<usize> {
    let mut offset = 0usize;
    for character in text.chars() {
        if offset == wanted {
            return Some(offset + character.len_utf16());
        }
        if offset > wanted {
            return None;
        }
        offset += character.len_utf16();
    }
    None
}

fn glyph_range_for_text_node(doc: &LaidOutDocument, node: html_dom::DomNodeId) -> Option<Range<u32>> {
    let mut runs = doc.render_view().text().text_runs().filter(|run| run.dom_text_node() == Some(node.raw())).map(|run| run.glyphs());
    let mut range = runs.next()?;
    for run in runs {
        range.start = range.start.min(run.start);
        range.end = range.end.max(run.end);
    }
    Some(range)
}

fn glyph_for_source_offset(doc: &LaidOutDocument, range: Range<u32>, source_offset: Option<usize>, allow_end: bool) -> u32 {
    let Some(source_offset) = source_offset else {
        return range.start;
    };
    for glyph_idx in range.clone() {
        if doc.glyph_source_offset(glyph_idx).is_some_and(|offset| offset as usize >= source_offset) {
            return glyph_idx;
        }
    }
    if allow_end { range.end } else { range.end.saturating_sub(1) }
}

fn find_glyph_in_text_chunk(doc: &LaidOutDocument, parent: html_dom::DomNodeId, text_step: usize, source_offset: Option<usize>, allow_end: bool) -> Option<u32> {
    let mut nodes = doc.document().find_text_nodes_by_step(parent, text_step).into_iter().peekable();
    let mut remaining = source_offset.unwrap_or(0);
    let mut last_end = None;
    while let Some(node) = nodes.next() {
        let text = doc.document().text_ref(node)?.text();
        let utf16_len = text.encode_utf16().count();
        let Some(range) = glyph_range_for_text_node(doc, node) else {
            remaining = remaining.saturating_sub(utf16_len);
            continue;
        };
        last_end = Some(range.end);
        if remaining < utf16_len || (remaining == utf16_len && (allow_end || nodes.peek().is_none())) {
            return Some(glyph_for_source_offset(doc, range, Some(remaining), allow_end));
        }
        remaining -= utf16_len;
    }
    last_end.map(|end| if allow_end { end } else { end.saturating_sub(1) })
}

fn find_glyph_in_node(doc: &LaidOutDocument, node: html_dom::DomNodeId, source_offset: Option<usize>, allow_end: bool) -> Option<u32> {
    match doc.document().node_ref(node)? {
        NodeRef::Text(_) => glyph_range_for_text_node(doc, node).map(|range| apply_source_offset(range, source_offset, allow_end)),
        NodeRef::Element(element) => {
            if let Some(glyph) = doc.render_view().text().text_runs().find_map(|run| {
                let box_node = doc.box_dom_element_idx(run.box_idx()).and_then(|raw| doc.document().node_id_from_raw(raw))?;
                (box_node == node).then(|| apply_source_offset(run.glyphs(), source_offset, allow_end))
            }) {
                return Some(glyph);
            }
            element.children().find_map(|child| find_glyph_in_node(doc, child, None, allow_end))
        }
    }
}

fn apply_source_offset(range: Range<u32>, source_offset: Option<usize>, allow_end: bool) -> u32 {
    let maximum = if allow_end { range.end } else { range.end.saturating_sub(1) };
    source_offset.map(|offset| (range.start + offset as u32).min(maximum)).unwrap_or(range.start)
}
