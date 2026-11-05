//! Document-local position facts shared by navigation and progress reporting.


use html_pipeline::engine::pagination_queries;
use super::{PageAnchor, PagePositions, PageRequest};
use crate::layout::{LineId, RenderView, SourceLocation};
use html_pipeline::engine::source_queries;

/// Position facts for one page origin. Source paths are resolved on demand so
/// callers querying only the glyph or progress allocate no source address.
pub struct DocumentPosition<'a> {
    pub cursor: usize,
    /// The first glyph on the origin's source line, independent of a preferred
    /// reading glyph. This remains available when its source address is missing.
    pub glyph: Option<u32>,
    pub document_fraction: f64,
    doc: RenderView<'a>,
    source_line: Option<(LineId, Option<u32>)>,
    empty_start: bool,
    origin: PageAnchor,
    preferred_glyph: Option<u32>,
}

/// Stable inputs for restoring a reading position after a document update.
pub struct ReflowAnchor {
    pub glyph: Option<u32>,
    pub source: Option<SourceLocation>,
}

impl DocumentPosition<'_> {
    /// Preserve leading content at document start; otherwise retain one source
    /// address and glyph fallback. The pending request determines whether document-start preservation applies.
    pub fn reflow_anchor(
        &self,
        page: Option<&PagePositions>,
        request: PageRequest,
    ) -> ReflowAnchor {
        let forward = matches!(
            request,
            PageRequest::StartingAt(_) | PageRequest::StartingAtOffset(_)
        );
        let first = self
            .doc
            .text()
            .line(LineId::new(0))
            .map(|line| line.start());
        let at_start = match (self.preferred_glyph, first) {
            (None, _) => true,
            (Some(anchor), Some(first)) => anchor == first,
            (Some(_), None) => false,
        };
        if at_start
            && forward
            && self.origin.offset_y <= 0.1
            && page.is_none_or(|page| page.start_line == 0)
        {
            ReflowAnchor {
                glyph: None,
                source: None,
            }
        } else {
            ReflowAnchor {
                glyph: self.preferred_glyph.or(self.glyph),
                source: self.source(),
            }
        }
    }

    /// Resolve a document-relative address, including image-only source lines
    /// and the intentional start address of a document without layout lines.
    pub fn source(&self) -> Option<SourceLocation> {
        if self.empty_start {
            return Some(SourceLocation::default());
        }
        let (line, glyph) = self.source_line?;
        source_queries::source_location_on_line(self.doc.addressing(), line, glyph)
    }
}

/// Resolve the origin's source line, glyph and document progress together.
/// `origin.cursor` takes precedence over its fallback offset. A preferred glyph
/// refines source addressing and progress without changing the page's glyph;
/// publication identity and the choice of preferred glyph belong to the host.
pub fn document_position<'a>(
    doc: RenderView<'a>,
    origin: PageAnchor,
    preferred_glyph: Option<u32>,
) -> DocumentPosition<'a> {
    let text = doc.text();
    let cursor = origin.cursor.unwrap_or_else(|| {
        pagination_queries::render_view_pagination(doc).start_at(origin.offset_y, 0.1)
    });
    let line = if text.lines().is_empty() {
        None
    } else {
        pagination_queries::render_view_pagination(doc).source_line(cursor)
    };
    let glyph = line.as_ref().map(|line| line.glyphs().start);
    let selected_glyph = preferred_glyph.or(glyph);
    let document_fraction =
        if let Some(glyph) = selected_glyph.filter(|_| text.glyph_count() > 0) {
            glyph as f64 / text.glyph_count().max(1) as f64
        } else {
            origin.offset_y
                / text
                    .lines()
                    .last()
                    .map(|line| line.point().y + line.height())
                    .unwrap_or(1.0)
                    .max(1.0)
        }
        .clamp(0.0, 1.0);
    let source_line = line.map(|line| {
        // An image-only line must not borrow the following text's address even
        // when both share a glyph boundary.
        (
            line.id(),
            selected_glyph.filter(|_| !line.glyphs().is_empty()),
        )
    });
    DocumentPosition {
        cursor,
        glyph,
        document_fraction,
        doc,
        source_line,
        empty_start: text.line_count() == 0 && origin.offset_y <= 0.1,
        origin,
        preferred_glyph,
    }
}
