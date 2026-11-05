//! One composed page used consistently for painting and pointer interaction.


use html_pipeline::engine::search_queries;
use html_pipeline::engine::fragment_paint;
use html_pipeline::engine::export_queries;
use html_pipeline::engine::box_queries;
use super::PagePositions;
use crate::document::ImageId;
use crate::geometry::{ContentHit, Geometry};
use crate::layout::{BoxId, LineId, RenderView};
use crate::render::{
    LinePlacement, OverlayPaintScratch, PaintCandidates, PaintFragment, PaintProjection, Painter,
    ProjectedLine, TextOverlay, paint_overlays,
};
use crate::selection::{
    SelectionController, SelectionExport, SelectionGeometry, SelectionHit, SelectionInput,
    SelectionLayout, SelectionOutcome, SelectionPaintScratch,
};
use crate::targets::{DocumentTarget, LinkTarget, ResolvedDestination};
use crate::search::{SourceTextMatches, TextPatternMatcher};
use html_engine_model::page_placements;
use kurbo::{Point, Rect, Vec2};
use peniko::Color;
use std::ops::Range;

/// A document and the engine-produced page that places its content. Pointer
/// coordinates and output geometry are page-local, before host display scaling.
#[derive(Clone, Copy)]
pub struct PageView<'a> {
    pub doc: RenderView<'a>,
    page: &'a PagePositions,
}

/// A visible reading band in page-local coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct PageReadingLine {
    pub glyphs: Range<u32>,
    pub bounds: Rect,
    pub column_bounds: Rect,
    pub column: i32,
    pub cell: Option<BoxId>,
}

struct BoxProjectionSpan {
    column: i32,
    translation: Vec2,
    top: f64,
    bottom: f64,
}

impl<'a> PageView<'a> {
    pub fn new(doc: RenderView<'a>, page: &'a PagePositions) -> Self {
        Self { doc, page }
    }

    fn document_geometry(&self) -> Geometry<'a> {
        Geometry::new(self.doc)
    }

    /// Projects document geometry into page columns and, when requested,
    /// replicated table headers. Painting and interaction share this mapping.
    fn project_rect(
        &self,
        source: Rect,
        line: Option<LineId>,
        include_repeated_headers: bool,
        out: &mut Vec<PaintFragment>,
    ) {
        if let Some(index) = line {
            if let (Some(screen), Some(line)) =
                (self.page.positions.get(index), self.doc.text().line(index))
            {
                out.push(PaintFragment {
                    source,
                    destination: source.origin() + (screen.point - line.point()),
                });
            }
        } else {
            let height = self.page.geometry.size.height;
            if height <= 0.0 {
                return;
            }
            let breaks = &self.page.offset_breaks;
            let start = self.page.paint_start_offset_y;
            let mut top = source.y0;
            let mut index = breaks.partition_point(|(at, _)| *at <= top);
            let mut offset = index.checked_sub(1).map_or(0.0, |index| breaks[index].1);
            while top < source.y1 {
                let bottom = breaks
                    .get(index)
                    .map_or(source.y1, |(at, _)| source.y1.min(*at));
                let from = top + offset - start;
                let to = bottom + offset - start;
                let first = (from / height).floor().max(0.0) as i32;
                let last = ((to - 0.0001) / height)
                    .floor()
                    .min(self.page.geometry.col_count - 1.0) as i32;
                for column in first..=last {
                    let column_top = f64::from(column) * height;
                    let y0 = from.max(column_top);
                    let y1 = to.min(column_top + height);
                    if y1 <= y0 {
                        continue;
                    }
                    let clipped = Rect::new(
                        source.x0,
                        y0 - offset + start,
                        source.x1,
                        y1 - offset + start,
                    );
                    out.push(PaintFragment {
                        source: clipped,
                        destination: Point::new(
                            page_placements::col_x(
                                &self.page.geometry,
                                f64::from(column),
                                source.x0,
                            ),
                            y0 - column_top,
                        ),
                    });
                }
                top = bottom;
                if let Some((at, next)) = breaks.get(index) {
                    if *at <= top {
                        offset = *next;
                        index += 1;
                    }
                }
            }
        }
        if include_repeated_headers {
            self.project_repeated_headers(source, out);
        }
    }

    /// Projects a box through its content and enclosing flow fragments.
    /// Independent positioned boxes retain separate page membership.
    fn project_box_rect(
        &self,
        source: Rect,
        owner: BoxId,
        include_repeated_headers: bool,
        out: &mut Vec<PaintFragment>,
    ) {
        let height = self.page.geometry.size.height;
        if height <= 0.0 {
            return;
        }
        if self.page.positions.is_empty() {
            self.project_rect(source, None, include_repeated_headers, out);
            return;
        }
        let boxes = self.doc.boxes();
        let mut distances = vec![usize::MAX; self.page.geometry.col_count.ceil().max(0.0) as usize];
        let mut candidates = Vec::new();
        for (index, screen) in self.page.positions.iter() {
            let Some(nearest) = distances.get_mut(screen.col_index as usize) else {
                continue;
            };
            let Some(distance) = box_queries::fragmentation_distance(boxes, owner, index) else {
                continue;
            };
            let Some(line) = self.doc.text().line(index) else {
                continue;
            };
            let top = line.point().y - screen.point.y;
            if source.y1 <= top || source.y0 >= top + height {
                continue;
            }
            *nearest = (*nearest).min(distance);
            candidates.push((line.point(), screen, distance));
        }
        let mut spans: Vec<BoxProjectionSpan> = Vec::new();
        for (point, screen, distance) in candidates {
            // Prefer the box's own content in each column, then the nearest
            // enclosing fragment. This also carries tall cells through
            // columns in which only another cell has text.
            if distance != distances[screen.col_index as usize] {
                continue;
            }
            let translation = screen.point - point;
            let mut top = -translation.y;
            let bottom = height - translation.y;
            if let Some(previous) = spans.last_mut()
                && previous.column == screen.col_index
            {
                if (previous.translation - translation).hypot() < 1e-6 {
                    continue;
                }
                // Keep two offsets within one column from painting the same
                // source band twice across a keep or rhythm adjustment.
                previous.bottom = previous.bottom.min(point.y);
                top = top.max(point.y);
            }
            spans.push(BoxProjectionSpan {
                column: screen.col_index,
                translation,
                top,
                bottom,
            });
        }
        for span in spans {
            let clipped = source.intersect(Rect::new(
                source.x0,
                span.top,
                source.x1,
                span.bottom.max(span.top),
            ));
            if clipped.width() > 0.0 && clipped.height() > 0.0 {
                out.push(PaintFragment {
                    source: clipped,
                    destination: clipped.origin() + span.translation,
                });
            }
        }
        if include_repeated_headers {
            self.project_repeated_headers(source, out);
        }
    }

    fn project_repeated_headers(&self, source: Rect, out: &mut Vec<PaintFragment>) {
        for header in &self.page.repeated_table_headers {
            let clipped = source.intersect(Rect::new(
                header.source_left,
                header.source_top,
                header.source_right,
                header.source_bottom,
            ));
            if clipped.width() > 0.0 && clipped.height() > 0.0 {
                out.push(PaintFragment {
                    source: clipped,
                    destination: Point::new(
                        page_placements::col_x(
                            &self.page.geometry,
                            header.col_index as f64,
                            clipped.x0,
                        ),
                        clipped.y0 - header.source_top,
                    ),
                });
            }
        }
    }

    fn placements(&self) -> impl Iterator<Item = LinePlacement> + '_ {
        self.page
            .positions
            .iter()
            .map(|(line, screen)| LinePlacement {
                line,
                origin: screen.point,
            })
    }

    // Select the page column before intrinsic hit testing.
    fn column_placements(&self, point: Point) -> impl Iterator<Item = LinePlacement> + '_ {
        let count = self.page.geometry.col_count as i32;
        let column = ((point.x / (self.page.geometry.col_width + self.page.geometry.col_gap))
            .floor() as i32)
            .clamp(0, (count - 1).max(0));
        self.page
            .positions
            .iter()
            .filter(move |(_, screen)| count > 0 && screen.col_index == column)
            .map(|(line, screen)| LinePlacement {
                line,
                origin: screen.point,
            })
    }

    pub fn glyph_range_bounds(&self, start: u32, end: u32) -> Option<Rect> {
        self.document_geometry()
            .selection_bounds_lines(start..end, self.placements())
    }

    pub fn hit_test_image(&self, point: Point) -> Option<ImageId> {
        self.document_geometry()
            .image_at_lines(point, self.column_placements(point))
    }

    /// Append the projected fragments of a box, including repeated headers.
    fn project_box(&self, box_idx: BoxId, out: &mut Vec<PaintFragment>) {
        let boxes = self.doc.boxes();
        let Some(rect) = boxes.bounds(box_idx) else {
            return;
        };
        if rect.width() <= 0.0 || rect.height() <= 0.0 || self.page.geometry.size.height <= 0.0 {
            return;
        }
        self.project_box_rect(rect, box_idx, true, out);
    }

    /// Whether any projected fragment of the box contains `point`. The
    /// fragment buffer is caller-owned so repeated tests reuse it.
    fn projected_box_contains(
        &self,
        box_idx: BoxId,
        point: Point,
        fragments: &mut Vec<PaintFragment>,
    ) -> bool {
        fragments.clear();
        self.project_box(box_idx, fragments);
        fragments.iter().any(|fragment| {
            Rect::from_origin_size(fragment.destination, fragment.source.size()).contains(point)
        })
    }

    pub fn link_at(&self, point: Point) -> Option<LinkTarget<'a>> {
        self.document_geometry()
            .link_at_lines(point, self.column_placements(point))
    }

    pub fn content_at(&self, point: Point) -> Option<ContentHit<'a>> {
        let mut fragments = Vec::new();
        self.document_geometry()
            .content_at_lines(point, self.column_placements(point), |index| {
                self.projected_box_contains(index, point, &mut fragments)
            })
    }
}

impl PaintProjection for PageView<'_> {
    fn candidates(&self, view: RenderView<'_>) -> Option<PaintCandidates> {
        let blocks = self.page.block_decoration_indices.as_deref()?;
        let mut candidates = PaintCandidates::default();
        candidates
            .lines
            .extend(self.page.positions.iter().map(|(line, _)| line));
        candidates.block_decorations.extend_from_slice(blocks);
        let mut header_decorations = Vec::new();
        for header in &self.page.repeated_table_headers {
            candidates.lines.extend(
                view.text()
                    .lines()
                    .iter()
                    .filter(|line| {
                        let source = line.point();
                        source.x >= header.source_left - 0.01
                            && source.x < header.source_right + 0.01
                            && source.y >= header.source_top - 0.01
                            && source.y < header.source_bottom - 0.01
                    })
                    .map(|line| line.id()),
            );
            fragment_paint::block_decoration_indices_intersecting(
                view.fragments(),
                header.source_top,
                header.source_bottom,
                &mut header_decorations,
            );
            candidates
                .block_decorations
                .extend_from_slice(&header_decorations);
        }
        Some(candidates)
    }
    fn bounds(&self) -> Rect {
        Rect::new(
            0.0,
            0.0,
            self.page.geometry.size.width.max(0.0),
            self.page.geometry.size.height.max(0.0),
        )
    }

    fn project_line(&self, line: LineId, source: Point, out: &mut Vec<ProjectedLine>) {
        if let Some(screen) = self.page.positions.get(line) {
            out.push(ProjectedLine {
                origin: screen.point,
                clip: None,
            });
        }
        for header in &self.page.repeated_table_headers {
            if source.x >= header.source_left - 0.01
                && source.x < header.source_right + 0.01
                && source.y >= header.source_top - 0.01
                && source.y < header.source_bottom - 0.01
            {
                out.push(ProjectedLine {
                    origin: Point::new(
                        page_placements::col_x(
                            &self.page.geometry,
                            header.col_index as f64,
                            source.x,
                        ),
                        source.y - header.source_top,
                    ),
                    clip: Some(Rect::new(
                        page_placements::col_x(
                            &self.page.geometry,
                            header.col_index as f64,
                            header.source_left,
                        ),
                        0.0,
                        page_placements::col_x(
                            &self.page.geometry,
                            header.col_index as f64,
                            header.source_right,
                        ),
                        header.source_bottom - header.source_top,
                    )),
                });
            }
        }
    }

    fn project_decoration(
        &self,
        owner: Option<BoxId>,
        source: Rect,
        line: Option<LineId>,
        out: &mut Vec<PaintFragment>,
    ) {
        if line.is_none()
            && let Some(owner) = owner
        {
            self.project_box_rect(source, owner, true, out);
        } else {
            self.project_rect(source, line, true, out);
        }
    }

    fn project_rect(&self, source: Rect, line: Option<LineId>, out: &mut Vec<PaintFragment>) {
        self.project_rect(source, line, true, out);
    }
}

impl<'a> PageView<'a> {
    /// Pointer data is resolved against the same column mapping used to paint.
    pub fn interact(
        &self,
        state: &mut SelectionController,
        input: SelectionInput,
    ) -> SelectionOutcome {
        let position = match &input {
            SelectionInput::PointerDown { position, .. }
            | SelectionInput::PointerMove { position, .. }
            | SelectionInput::PointerUp { position, .. }
            | SelectionInput::SelectAt { position, .. } => Some(*position),
            _ => None,
        };
        if let Some(position) = position {
            state.interact(
                SelectionLayout {
                    view: self.doc,
                    placements: self.column_placements(position),
                    projection: self,
                },
                input,
            )
        } else {
            state.interact(
                SelectionLayout {
                    view: self.doc,
                    placements: std::iter::empty(),
                    projection: self,
                },
                input,
            )
        }
    }

    pub fn selection_geometry(&self, state: &SelectionController) -> SelectionGeometry {
        state.geometry(SelectionLayout {
            view: self.doc,
            placements: self.placements(),
            projection: self,
        })
    }

    pub fn selection_hit(&self, state: &SelectionController, position: Point) -> SelectionHit {
        state.hit_test(
            SelectionLayout {
                view: self.doc,
                placements: self.placements(),
                projection: self,
            },
            position,
        )
    }

    pub fn export_selection(
        &self,
        state: &SelectionController,
        request: SelectionExport,
    ) -> Option<String> {
        state.export(
            SelectionLayout {
                view: self.doc,
                placements: std::iter::empty(),
                projection: self,
            },
            request,
        )
    }

    pub fn paint_selection(
        &self,
        state: &SelectionController,
        color: Color,
        painter: &mut impl Painter,
        scratch: &mut SelectionPaintScratch,
    ) {
        if self.page.last_line.is_none() {
            return;
        }
        state.paint(
            SelectionLayout {
                view: self.doc,
                placements: self.placements(),
                projection: self,
            },
            color,
            painter,
            scratch,
        );
    }

    pub fn paint_overlays(
        &self,
        overlays: impl IntoIterator<Item = TextOverlay>,
        painter: &mut impl Painter,
        scratch: &mut OverlayPaintScratch,
    ) {
        if self.page.last_line.is_none() {
            return;
        }
        paint_overlays(self.doc, overlays, self.placements(), painter, scratch);
    }

    /// Export the source text represented by this page's placed lines.
    pub fn text(&self) -> String {
        export_queries::text_for_lines(
            self.doc.export(),
            self.page.positions.iter().map(|(line, _)| line),
        )
        .unwrap_or_default()
    }

    /// Match patterns against visible source text without exposing line selection.
    pub fn match_text(&self, patterns: &TextPatternMatcher) -> SourceTextMatches<'a> {
        search_queries::match_lines(
            patterns,
            self.doc.text(),
            self.page.positions.iter().map(|(line, _)| line),
        )
    }

    pub fn reading_start(&self) -> u32 {
        let text = self.doc.text();
        self.page
            .positions
            .iter()
            .filter_map(|(line, _)| text.line(line).map(|line| line.start()))
            .min()
            .unwrap_or(0)
    }

    pub fn contains_glyph(&self, glyph: u32) -> bool {
        self.doc
            .addressing()
            .destination(DocumentTarget::Glyph(glyph))
            .is_some_and(|target| match target {
                ResolvedDestination::Line { line, .. } => self.page.positions.contains(line),
                ResolvedDestination::Position { .. } => false,
            })
    }

    pub fn reading_lines(&self) -> Vec<PageReadingLine> {
        let mut lines: Vec<_> = self
            .page
            .positions
            .iter()
            .filter_map(move |(index, screen)| {
                let line = self.doc.text().line(index)?;
                let column_x =
                    page_placements::col_x(&self.page.geometry, f64::from(screen.col_index), 0.0);
                let column_bounds = Rect::new(
                    column_x,
                    screen.point.y,
                    column_x + self.page.geometry.col_width,
                    screen.point.y + line.height(),
                );
                let natural = self.document_geometry().placed_reading_line(
                    LinePlacement {
                        line: index,
                        origin: screen.point,
                    },
                    column_bounds,
                )?;
                Some(PageReadingLine {
                    glyphs: natural.glyphs,
                    bounds: natural.bounds,
                    column_bounds,
                    column: screen.col_index,
                    cell: natural.cell,
                })
            })
            .collect();
        // Source order completes each table cell while retaining page columns.
        lines.sort_by_key(|line| (line.column, line.glyphs.start));
        lines
    }
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod fixtures;
