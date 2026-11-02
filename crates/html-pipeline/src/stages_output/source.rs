//! Source addresses resolved directly against the current document's DOM.

use super::{LineId, RenderAddressingView};
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

impl RenderAddressingView<'_> {
    /// First addressable content on a line, including images before its text.
    /// Generated content falls back to its owning source element.
    pub fn source_location_on_line(
        self,
        line: usize,
        glyph: Option<u32>,
    ) -> Option<SourceLocation> {
        let view = self.doc.render_view(self.glyph_metrics);
        let layout_line = view.text().line(line)?;
        let glyph = glyph.filter(|glyph| layout_line.glyphs().contains(glyph));
        let mut source = glyph
            .and_then(|glyph| self.source_position_for_glyph(glyph, false))
            .map(SourceLocation::from);
        for image in view.fragments().images_for_line(line).iter() {
            let Some(location) = self.element_location(image.box_idx) else {
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
            let run = view
                .text()
                .text_runs()
                .find(|run| run.glyphs().contains(&glyph))?;
            once(run.box_idx())
                .chain(view.boxes().ancestors(run.box_idx()))
                .find_map(|owner| self.element_location(owner))
        })
    }

    /// First stable carrier line for an element address. Unlike its y
    /// coordinate, this retains identity when independent boxes overlap.
    pub fn source_element_line(self, location: &SourceLocation) -> Option<LineId> {
        if location.utf16_offset.is_some() || location.path.is_empty() {
            return None;
        }
        let node = self
            .doc
            .document()
            .find_dom_node_by_steps(&location.path)?
            .raw();
        let boxes = self.doc.render_view(self.glyph_metrics).boxes();
        (0..boxes.len())
            .filter(|&index| self.doc.box_dom_element_idx(index) == Some(node))
            .map(|index| boxes.content_line_range(index))
            .filter(|range| !range.is_empty())
            .map(|range| LineId::new(range.start))
            .min()
    }

    /// Document-space vertical coordinate for an element-only source address.
    /// Text addresses must be resolved through `resolve_source_position`.
    pub fn source_element_y(self, location: &SourceLocation) -> Option<f64> {
        if location.utf16_offset.is_some() {
            return None;
        }
        if location.path.is_empty() {
            return Some(0.0);
        }
        let node = self
            .doc
            .document()
            .find_dom_node_by_steps(&location.path)?
            .raw();
        let boxes = self.doc.render_view(self.glyph_metrics).boxes();
        (0..boxes.len())
            .filter(|&index| self.doc.box_dom_element_idx(index) == Some(node))
            .filter_map(|index| boxes.point(index).map(|point| point.y))
            .min_by(f64::total_cmp)
    }

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
