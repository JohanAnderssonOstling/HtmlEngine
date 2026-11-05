//! Source addresses resolved directly against the current document's DOM.

use crate::stages::fragment_paint;
use html_engine_model::layout_state;
use crate::stages::text_paint;
use super::box_queries;
use super::{BoxId, LineId, RenderAddressingView};
use html_dom::SourcePosition;
use std::iter::once;

/// An owned document-relative source address. Publication/spine identifiers and
/// encodings such as EPUB CFI belong to the embedding application.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceLocation {
    pub path: Vec<usize>,
    pub utf16_offset: Option<usize>,
}

impl From<SourcePosition<'_>> for SourceLocation {
    fn from(position: SourcePosition<'_>) -> Self {
        let path = position
            .elements()
            .map(|element| element.step())
            .chain(once(position.text_step()))
            .collect();
        Self {
            path,
            utf16_offset: Some(position.utf16_offset()),
        }
    }
}

/// First addressable content on a line, including images before its text.
/// Generated content falls back to its owning source element.
pub fn source_location_on_line(
    value: RenderAddressingView<'_>,
    line: LineId,
    glyph: Option<u32>,
) -> Option<SourceLocation> {
    let view = value.doc.render_view(value.glyph_metrics);
    let layout_line = view.text().line(line)?;
    let glyph = glyph.filter(|glyph| text_paint::render_line_glyphs(&layout_line).contains(glyph));
    let mut source = glyph
        .and_then(|glyph| value.source_position_for_glyph(glyph, false))
        .map(SourceLocation::from);
    for image in fragment_paint::render_fragment_view_images_for_line(view.fragments(), line).iter()
    {
        let Some(location) = value.element_location(image.box_idx) else {
            continue;
        };
        if source
            .as_ref()
            .is_none_or(|source| location.path < source.path)
        {
            source = Some(location);
        }
    }
    source.or_else(|| {
        let glyph = glyph?;
        let run = text_paint::render_text_view_text_runs(view.text())
            .find(|run| run.glyphs().contains(&glyph))?;
        once(text_paint::render_text_run_box_id(&run))
            .chain(box_queries::ancestors(
                view.boxes(),
                text_paint::render_text_run_box_id(&run),
            ))
            .find_map(|owner| value.element_location(layout_state::box_id_index(owner)))
    })
}

impl RenderAddressingView<'_> {
    fn element_location(self, box_idx: usize) -> Option<SourceLocation> {
        let document = self.doc.document();
        let node = document.node_id_from_raw(self.doc.box_dom_element_idx(box_idx)?)?;
        let path = document
            .element_source_path(node)?
            .into_iter()
            .map(|element| element.step())
            .collect();
        Some(SourceLocation {
            path,
            utf16_offset: None,
        })
    }
}

/// First stable carrier line for an element address. Unlike its y
/// coordinate, this retains identity when independent boxes overlap.
pub fn render_addressing_view_source_element_line(
    value: RenderAddressingView<'_>,
    location: &SourceLocation,
) -> Option<LineId> {
    if location.utf16_offset.is_some() || location.path.is_empty() {
        return None;
    }
    let node = value
        .doc
        .document()
        .find_dom_node_by_steps(&location.path)?
        .raw();
    let boxes = value.doc.render_view(value.glyph_metrics).boxes();
    (0..box_queries::len(boxes))
        .filter(|&index| value.doc.box_dom_element_idx(index) == Some(node))
        .map(|index| box_queries::content_line_range(boxes, layout_state::box_id_new(index)))
        .filter(|range| !range.is_empty())
        .map(|range| range.start)
        .min()
}

/// Document-space vertical coordinate for an element-only source address.
/// Text addresses must be resolved through `resolve_source_position`.
pub fn render_addressing_view_source_element_y(
    value: RenderAddressingView<'_>,
    location: &SourceLocation,
) -> Option<f64> {
    if location.utf16_offset.is_some() {
        return None;
    }
    if location.path.is_empty() {
        return Some(0.0);
    }
    let node = value
        .doc
        .document()
        .find_dom_node_by_steps(&location.path)?
        .raw();
    let boxes = value.doc.render_view(value.glyph_metrics).boxes();
    (0..box_queries::len(boxes))
        .filter(|&index| value.doc.box_dom_element_idx(index) == Some(node))
        .filter_map(|index| {
            box_queries::render_box_view_point(boxes, layout_state::box_id_new(index))
                .map(|point| point.y)
        })
        .min_by(f64::total_cmp)
}
