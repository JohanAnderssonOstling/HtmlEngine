//! Source addresses resolved directly against the current document's DOM.

use super::RenderAddressingView;

/// An owned document-relative source address. Publication/spine identifiers and
/// encodings such as EPUB CFI belong to the embedding application.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceLocation {
    pub path: Vec<usize>,
    pub utf16_offset: Option<usize>,
}

impl RenderAddressingView<'_> {
    /// First addressable content on a line, including images before its text.
    /// Generated content falls back to its owning source element.
    pub fn source_location_on_line(
        self,
        line: usize,
        glyph: Option<u32>,
    ) -> Option<SourceLocation> {
        let layout_line = self.doc.render_view(self.glyph_metrics).text().line(line)?;
        let glyph = glyph.filter(|glyph| layout_line.glyphs().contains(glyph));
        let mut source = glyph
            .and_then(|glyph| self.source_position_for_glyph(glyph, false))
            .map(|position| {
                let mut path = position
                    .elements()
                    .map(|step| step.step())
                    .collect::<Vec<_>>();
                path.push(position.text_step());
                SourceLocation {
                    path,
                    utf16_offset: Some(position.utf16_offset()),
                }
            });
        for image in self
            .doc
            .render_view(self.glyph_metrics)
            .fragments()
            .images_for_line(line)
            .iter()
        {
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
            let view = self.doc.render_view(self.glyph_metrics);
            let run = view
                .text()
                .text_runs()
                .find(|run| run.glyphs().contains(&glyph))?;
            std::iter::once(run.box_idx())
                .chain(view.boxes().ancestors(run.box_idx()))
                .find_map(|owner| self.element_location(owner))
        })
    }

    /// Document-space vertical coordinate for an element-only source address.
    /// Text addresses must be resolved through `resolve_source_position`.
    pub fn source_element_y(self, location: &SourceLocation) -> Option<f64> {
        if location.utf16_offset.is_some()
            || location
                .path
                .iter()
                .any(|step| *step == 0 || !step.is_multiple_of(2))
        {
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
            .get_dom_path_to_node(node)
            .into_iter()
            .skip(1)
            .map(|node| document.get_element_step(node))
            .collect::<Option<Vec<_>>>()?;
        Some(SourceLocation {
            path,
            utf16_offset: None,
        })
    }
}
