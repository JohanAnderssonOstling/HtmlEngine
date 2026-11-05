//! Document geometry without a viewport, columns, or a window system.

use html_engine_model::layout_state;
use html_pipeline::engine::address_queries;
use html_pipeline::engine::box_queries;
use html_pipeline::engine::export_queries;
use html_pipeline::engine::geometry_queries;
use html_pipeline::engine::search_queries;
use html_pipeline::engine::semantic_queries;
use html_pipeline::engine::fragment_paint;
use html_pipeline::engine::text_paint;
use crate::document::ImageId;
use crate::layout::{BoxId, CaretAffinity, LineId, RenderView};
use crate::selection::{Selection, TableCellRange, TableHit};
use crate::targets::LinkTarget;
use kurbo::{Point, Rect};
use std::ops::Range;

use crate::render::LinePlacement;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextHit {
    pub line: LineId,
    pub glyph: u32,
}

/// A caret and whole word resolved together in document coordinates.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WordHit {
    pub caret: u32,
    pub word: Range<u32>,
    pub position: Point,
}

/// One clipped rectangle for an opaque caller-labelled logical range.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeRect<Id> {
    pub id: Id,
    pub line: LineId,
    pub bounds: Rect,
}

/// A logical selection with caller-owned labels for its highlight styles.
/// Cells can use a different appearance from selected text.
pub(crate) struct SelectionHighlight<'a, Id> {
    pub selection: Selection<'a>,
    pub text: Id,
    pub cells: Id,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageHit {
    pub image: ImageId,
    pub is_equation: bool,
}

/// Several identities can coincide: linked text or an image may lie in a cell.
/// Hosts choose which action to perform; hit testing never activates content.
#[derive(Clone, Debug)]
pub struct ContentHit<'a> {
    pub text: Option<TextHit>,
    pub link: Option<LinkTarget<'a>>,
    pub image: Option<ImageHit>,
    /// Image under the point, including one covered by foreground text.
    pub underlying_image: Option<ImageHit>,
    pub table: Option<TableHit>,
}

/// The foreground ink at a point has one winner in CSS paint order. Image
/// ownership is retained only to resolve its enclosing link.
enum PaintedHit {
    Text(TextHit),
    Image { hit: ImageHit, box_idx: BoxId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WordHitPolicy {
    InkOnly,
    Nearest,
}

/// Intrinsic reading region, retaining cell identity for side-by-side content.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadingLineGeometry {
    pub glyphs: Range<u32>,
    pub bounds: Rect,
    pub cell: Option<BoxId>,
}

/// Intrinsic geometry from the engine's retained text measurements.
#[derive(Clone, Copy)]
pub(crate) struct Geometry<'a> {
    view: RenderView<'a>,
}

impl<'a> Geometry<'a> {
    fn point_placements(self, point: Point) -> impl Iterator<Item = LinePlacement> + 'a {
        geometry_queries::render_geometry_view_lines_at_y(
            geometry_queries::render_view_geometry(self.view),
            point.y,
        )
        .filter_map(move |id| {
            let line = self.view.text().line(id)?;
            Some(LinePlacement {
                line: id,
                origin: line.point(),
            })
        })
    }

    fn paint_rank(self, line: LineId) -> u32 {
        fragment_paint::line_paint_rank(self.view.fragments(), line).unwrap_or(0)
    }

    fn painted_hit(
        self,
        point: Point,
        placements: &[LinePlacement],
    ) -> (Option<PaintedHit>, Option<ImageHit>) {
        let text = self.ink_hit(point, placements);
        let image = self.image_hit_at_lines(point, placements.iter().copied());
        let foreground = match (text, image) {
            // Images paint before text on a shared carrier line.
            (Some(text), Some((_, _, image_line)))
                if self.paint_rank(text.line) >= self.paint_rank(image_line) =>
            {
                Some(PaintedHit::Text(text))
            }
            (_, Some((hit, box_idx, _))) => Some(PaintedHit::Image { hit, box_idx }),
            (Some(text), None) => Some(PaintedHit::Text(text)),
            (None, None) => None,
        };
        (foreground, image.map(|(hit, _, _)| hit))
    }

    fn line_placements(self) -> impl Iterator<Item = LinePlacement> + 'a {
        self.view.text().lines().iter().map(|line| LinePlacement {
            line: line.id(),
            origin: line.point(),
        })
    }

    fn box_contains(self, index: BoxId, point: Point) -> bool {
        let boxes = self.view.boxes();
        box_queries::render_box_view_point(boxes, index)
            .zip(box_queries::render_box_view_size(boxes, index))
            .is_some_and(|(origin, size)| Rect::from_origin_size(origin, size).contains(point))
    }

    /// Visit caller placements once, indexing range intersections before native
    /// caret/clip queries. Output preserves placement order, then input range
    /// order, including duplicate ranges and repeated placements. Labels carry
    /// no engine interpretation or appearance.
    pub(crate) fn range_geometry_lines<Id: Clone>(
        self,
        ranges: impl IntoIterator<Item = (Id, Range<u32>)>,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Vec<RangeRect<Id>> {
        let mut result = Vec::new();
        self.visit_range_intersections(ranges, placements, |id, range, placement| {
            result.extend(
                self.placed_selection_rects(placement, range.clone())
                    .map(|bounds| RangeRect {
                        id: id.clone(),
                        line: placement.line,
                        bounds,
                    }),
            );
        });
        result
    }

    fn visit_range_intersections<Id>(
        self,
        ranges: impl IntoIterator<Item = (Id, Range<u32>)>,
        placements: impl IntoIterator<Item = LinePlacement>,
        mut visit: impl FnMut(&Id, &Range<u32>, LinePlacement),
    ) {
        let mut ranges: Vec<_> = ranges
            .into_iter()
            .enumerate()
            .filter(|(_, (_, range))| range.start < range.end)
            .collect();
        ranges.sort_by_key(|(order, (_, range))| (range.start, *order));
        let mut max_end = 0;
        let prefix_end: Vec<_> = ranges
            .iter()
            .map(|(_, (_, range))| {
                max_end = max_end.max(range.end);
                max_end
            })
            .collect();
        let mut candidates = Vec::new();
        for placement in placements {
            let Some(line) = self.view.text().line(placement.line) else {
                continue;
            };
            let stop = ranges.partition_point(|(_, (_, range))| range.start < line.end());
            let start = prefix_end[..stop].partition_point(|end| *end <= line.start());
            candidates.clear();
            candidates.extend(
                ranges[start..stop]
                    .iter()
                    .filter(|(_, (_, range))| range.end > line.start()),
            );
            candidates.sort_by_key(|(order, _)| *order);
            for (_, (id, range)) in &candidates {
                visit(id, range, placement);
            }
        }
    }

    /// Resolve one overlay pass through caller-supplied line placement,
    /// appending labelled text rectangles to `rects` and highlighted table
    /// cell boxes to `cells`. Text preserves placement order, then input range
    /// order; selection text follows the other ranges. Explicit table cells
    /// precede cells promoted by semantic selection. Hosts project cell boxes
    /// and paint them after the text; no colors are assigned here.
    pub(crate) fn highlights<Id: Clone>(
        self,
        ranges: impl IntoIterator<Item = (Id, Range<u32>)>,
        placements: impl IntoIterator<Item = LinePlacement>,
        rects: &mut Vec<(Id, Rect)>,
    ) {
        let mut ranges = ranges.into_iter().peekable();
        if ranges.peek().is_none() {
            return;
        }
        self.visit_range_intersections(ranges, placements, |id, range, placement| {
            rects.extend(
                self.placed_selection_rects(placement, range.clone())
                    .map(|bounds| (id.clone(), bounds)),
            );
        });
    }

    pub(crate) fn collect_highlights<Id: Clone>(
        self,
        ranges: impl IntoIterator<Item = (Id, Range<u32>)>,
        selection: Option<SelectionHighlight<'a, Id>>,
        table: Option<(Id, TableCellRange)>,
        placements: impl IntoIterator<Item = LinePlacement>,
        rects: &mut Vec<(Id, Rect)>,
        cells: &mut Vec<(Id, BoxId)>,
    ) {
        let selection = selection.map(|highlight| {
            (
                highlight.selection.geometry_plan(),
                highlight.text,
                highlight.cells,
            )
        });
        let mut ranges = ranges.into_iter().peekable();
        if ranges.peek().is_none()
            && selection
                .as_ref()
                .is_none_or(|(plan, _, _)| plan.text_ranges.len() <= 1)
        {
            // One selection interval is written directly, including standalone
            // notes, without allocating a range index.
            if let Some((plan, text, _)) = &selection {
                if let Some(range) = plan.text_ranges.first() {
                    for placement in placements {
                        let Some(line) = self.view.text().line(placement.line) else {
                            continue;
                        };
                        if range.start >= line.end() || range.end <= line.start() {
                            continue;
                        }
                        rects.extend(
                            self.placed_selection_rects(placement, range.clone())
                                .map(|bounds| (text.clone(), bounds)),
                        );
                    }
                }
            }
        } else {
            let selected_ranges = selection.iter().flat_map(|(plan, text, _)| {
                plan.text_ranges
                    .iter()
                    .cloned()
                    .map(move |range| (text.clone(), range))
            });
            self.visit_range_intersections(
                ranges.chain(selected_ranges),
                placements,
                |id, range, placement| {
                    rects.extend(
                        self.placed_selection_rects(placement, range.clone())
                            .map(|bounds| (id.clone(), bounds)),
                    );
                },
            );
        }
        if let Some((id, range)) = table {
            if let Some(table) =
                box_queries::render_box_view_table(self.view.boxes(), range.table_box())
            {
                cells.extend(range.cells(&table).map(|cell| (id.clone(), cell.box_id())));
            }
        }
        if let Some((plan, _, cell_id)) = selection {
            cells.extend(
                plan.cell_boxes
                    .into_iter()
                    .map(|index| (cell_id.clone(), index)),
            );
        }
    }

    pub(crate) fn new(view: RenderView<'a>) -> Self {
        Self { view }
    }

    pub(crate) fn content_at(self, point: Point) -> Option<ContentHit<'a>> {
        self.content_at_lines(point, self.point_placements(point), |index| {
            self.box_contains(index, point)
        })
    }

    /// Placements and projected cell containment are supplied by the host.
    /// Text/link hits require visible ink bounds, unlike nearest-caret selection.
    pub(crate) fn content_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
        contains_box: impl FnMut(BoxId) -> bool,
    ) -> Option<ContentHit<'a>> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        let placements: Vec<_> = placements.into_iter().collect();
        let (hit, underlying_image) = self.painted_hit(point, &placements);
        let targets = self.view.addressing();
        let (text, image, link) = match hit {
            Some(PaintedHit::Text(text)) => (Some(text), None, targets.link_for_glyph(text.glyph)),
            Some(PaintedHit::Image { hit, box_idx }) => (
                None,
                Some(hit),
                address_queries::render_addressing_view_link_for_box(targets, box_idx),
            ),
            None => (None, None, None),
        };
        let table = self.table_at_where(contains_box);
        (text.is_some() || image.is_some() || table.is_some()).then_some(ContentHit {
            text,
            link,
            image,
            underlying_image,
            table,
        })
    }

    /// Link-only hover queries avoid traversing unrelated table cells.
    pub(crate) fn link_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Option<LinkTarget<'a>> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        let placements: Vec<_> = placements.into_iter().collect();
        let targets = self.view.addressing();
        match self.painted_hit(point, &placements).0 {
            Some(PaintedHit::Text(text)) => targets.link_for_glyph(text.glyph),
            Some(PaintedHit::Image { box_idx, .. }) => {
                address_queries::render_addressing_view_link_for_box(targets, box_idx)
            }
            None => None,
        }
    }

    fn ink_hit(self, point: Point, placements: &[LinePlacement]) -> Option<TextHit> {
        self.hit_test_lines(point, placements.iter().copied())
            .and_then(|hit| {
                [Some(hit.glyph), hit.glyph.checked_sub(1)]
                    .into_iter()
                    .flatten()
                    .find_map(|glyph| {
                        self.selection_bounds_lines(
                            glyph..glyph.saturating_add(1),
                            placements.iter().copied(),
                        )
                        .is_some_and(|bounds| bounds.contains(point))
                        .then_some(TextHit {
                            line: hit.line,
                            glyph,
                        })
                    })
            })
    }

    pub(crate) fn word_hit_at(self, point: Point, policy: WordHitPolicy) -> Option<WordHit> {
        self.word_hit_at_lines(point, self.line_placements(), policy)
    }

    /// Resolve caret affinity and word geometry in one hit test. Nearest-word
    /// fallback is restricted to the hit line supplied by the caller.
    pub(crate) fn word_hit_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
        policy: WordHitPolicy,
    ) -> Option<WordHit> {
        let placements: Vec<_> = placements.into_iter().collect();
        let hit = self.hit_test_lines(point, placements.iter().copied())?;
        let make_hit = |word| WordHit {
            caret: hit.glyph,
            word,
            position: point,
        };
        for glyph in [Some(hit.glyph), hit.glyph.checked_sub(1)]
            .into_iter()
            .flatten()
        {
            if self
                .selection_bounds_lines(glyph..glyph.saturating_add(1), placements.iter().copied())
                .is_some_and(|bounds| bounds.contains(point))
            {
                if let Some(word) =
                    search_queries::render_text_view_word_range_at(self.view.text(), glyph)
                {
                    return Some(make_hit(word));
                }
                break;
            }
        }
        if policy == WordHitPolicy::InkOnly {
            return None;
        }
        search_queries::render_text_view_word_ranges_in(
            self.view.text(),
            text_paint::render_line_glyphs(&self.view.text().line(hit.line)?),
        )
        .filter_map(|word| {
            let bounds = self.selection_bounds_lines(
                word.clone(),
                placements
                    .iter()
                    .copied()
                    .filter(|placement| placement.line == hit.line),
            )?;
            let dx = (bounds.x0 - point.x).max(point.x - bounds.x1).max(0.0);
            let dy = (bounds.y0 - point.y).max(point.y - bounds.y1).max(0.0);
            Some((word, dx * dx + dy * dy))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(word, _)| make_hit(word))
    }

    /// Apply intrinsic clipping and find the containing cell/block. The caller
    /// supplies a fallback horizontal band when no owning box has a width.
    pub(crate) fn placed_reading_line(
        self,
        placement: LinePlacement,
        fallback: Rect,
    ) -> Option<ReadingLineGeometry> {
        let text = self.view.text();
        let boxes = self.view.boxes();
        let line = text.line(placement.line)?;
        if line.height() <= 0.0 {
            return None;
        }
        let delta = placement.origin - line.point();
        let clip = text_paint::line_overflow_clip(text, placement.line).map(|clip| {
            clip.apply_to(Rect::new(
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::INFINITY,
            )) + delta
        });
        let clipped = |rect: Rect| clip.map_or(rect, |clip| rect.intersect(clip));
        let glyphs = text_paint::render_text_view_line_text_fragments(text, placement.line)?
            .filter(|fragment| fragment.visible())
            .map(|fragment| fragment.glyphs())
            .filter(|range| !range.is_empty())
            .filter(|range| {
                let Some((left, right)) = geometry_queries::render_view_geometry(self.view)
                    .caret_span(placement.line, range.clone())
                else {
                    return false;
                };
                let rect = clipped(Rect::new(
                    placement.origin.x + left,
                    placement.origin.y,
                    placement.origin.x + right,
                    placement.origin.y + line.height(),
                ));
                rect.width() > 0.0 && rect.height() > 0.0
            })
            .reduce(|left, right| left.start.min(right.start)..left.end.max(right.end))?;
        let owner = text_paint::render_line_owner_box(&line);
        let cell = std::iter::once(owner)
            .chain(box_queries::ancestors(boxes, owner))
            .find(|&index| box_queries::is_table_cell(boxes, index));
        let region = cell.unwrap_or(owner);
        let (x, width) = box_queries::render_box_view_point(boxes, region)
            .zip(box_queries::render_box_view_size(boxes, region))
            .filter(|(_, size)| size.width > 0.0)
            .map(|(origin, size)| (origin.x + delta.x, size.width))
            .unwrap_or((fallback.x0, fallback.width()));
        let bounds = clipped(Rect::new(
            x,
            placement.origin.y,
            x + width,
            placement.origin.y + line.height(),
        ));
        (bounds.width() > 0.0 && bounds.height() > 0.0).then_some(ReadingLineGeometry {
            glyphs,
            bounds,
            cell,
        })
    }

    pub(crate) fn selection_contains_lines(
        self,
        selection: &Selection<'a>,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
        mut contains_box: impl FnMut(BoxId) -> bool,
    ) -> bool {
        if !point.x.is_finite() || !point.y.is_finite() {
            return false;
        }
        let selection = selection.geometry_plan();
        if selection
            .cell_boxes
            .iter()
            .any(|&index| contains_box(index))
        {
            return true;
        }
        placements.into_iter().any(|placement| {
            selection.text_ranges.iter().any(|range| {
                self.placed_selection_rects(placement, range.clone())
                    .any(|rect| rect.contains(point))
            })
        })
    }

    pub(crate) fn line_selection_rects(
        self,
        line: LineId,
        range: Range<u32>,
    ) -> impl Iterator<Item = Rect> + 'a {
        let clip = text_paint::line_overflow_clip(self.view.text(), line);
        geometry_queries::render_geometry_view_line_selection_rects(
            geometry_queries::render_view_geometry(self.view),
            line,
            range,
        )
        .map(move |rect| clip.map_or(rect, |clip| clip.apply_to(rect)))
        .filter(|rect| rect.width() > 0.0 && rect.height() > 0.0)
    }

    pub(crate) fn placed_selection_rects(
        self,
        placement: LinePlacement,
        range: Range<u32>,
    ) -> impl Iterator<Item = Rect> + 'a {
        let source = self
            .view
            .text()
            .line(placement.line)
            .map_or(Point::ZERO, |line| line.point());
        let delta = placement.origin - source;
        self.line_selection_rects(placement.line, range)
            .map(move |rect| rect + delta)
    }

    pub(crate) fn selection_rects(self, range: Range<u32>) -> Vec<Rect> {
        self.view
            .text()
            .lines()
            .iter()
            .flat_map(|line| self.line_selection_rects(line.id(), range.clone()))
            .collect()
    }

    pub(crate) fn selection_bounds(self, range: Range<u32>) -> Option<Rect> {
        self.selection_bounds_lines(range, self.line_placements())
    }

    pub(crate) fn selection_bounds_lines(
        self,
        range: Range<u32>,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Option<Rect> {
        geometry_queries::selection_bounds_lines(
            geometry_queries::render_view_geometry(self.view),
            range,
            placements
                .into_iter()
                .map(|placement| (placement.line, placement.origin)),
        )
    }

    pub(crate) fn text_intersections(
        self,
        line: LineId,
        range: Range<u32>,
    ) -> impl Iterator<Item = Range<u32>> + 'a {
        geometry_queries::text_intersections(
            geometry_queries::render_view_geometry(self.view),
            line,
            range,
        )
    }

    pub(crate) fn selection_endpoints_lines(
        self,
        range: Range<u32>,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> (Option<Point>, Option<Point>) {
        let (mut leading, mut trailing) = (None, None);
        if range.start >= range.end {
            return (leading, trailing);
        }
        for placement in placements {
            let Some(line) = self.view.text().line(placement.line) else {
                continue;
            };
            if self
                .text_intersections(placement.line, range.start..range.start + 1)
                .next()
                .is_some()
            {
                leading = geometry_queries::render_view_geometry(self.view)
                    .caret_x(placement.line, range.start, CaretAffinity::Leading)
                    .map(|x| {
                        Point::new(placement.origin.x + x, placement.origin.y + line.height())
                    });
            }
            if self
                .text_intersections(placement.line, range.end - 1..range.end)
                .next()
                .is_some()
            {
                trailing = geometry_queries::render_view_geometry(self.view)
                    .caret_x(placement.line, range.end, CaretAffinity::Trailing)
                    .map(|x| {
                        Point::new(placement.origin.x + x, placement.origin.y + line.height())
                    });
            }
        }
        (leading, trailing)
    }

    pub(crate) fn hit_test(self, point: Point) -> Option<TextHit> {
        self.hit_test_lines(point, self.line_placements())
    }

    /// The host chooses the candidate lines, for example only those in the
    /// column receiving a click. Nearest-line and bidi/cluster rules are shared.
    pub(crate) fn hit_test_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Option<TextHit> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        let mut best = None;
        let mut distance = (f64::INFINITY, f64::INFINITY);
        for placement in placements {
            let Some(line) = self.view.text().line(placement.line) else {
                continue;
            };
            if text_paint::render_line_glyphs(&line).is_empty() {
                continue;
            }
            let nearest = self
                .placed_selection_rects(placement, text_paint::render_line_glyphs(&line))
                .filter(|rect| rect.width() > 0.0 && rect.height() > 0.0)
                .map(|rect| {
                    let dy = (rect.y0 - point.y).max(point.y - rect.y1).max(0.0);
                    let dx = (rect.x0 - point.x).max(point.x - rect.x1).max(0.0);
                    ((dy, dx), point.x.clamp(rect.x0, rect.x1))
                })
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((candidate, x)) = nearest {
                if candidate < distance
                    || (candidate == distance
                        && best.is_some_and(|(previous, _): (LinePlacement, f64)| {
                            self.paint_rank(placement.line) > self.paint_rank(previous.line)
                        }))
                {
                    distance = candidate;
                    best = Some((placement, x));
                }
            }
        }
        let (placement, x) = best?;
        let glyph = geometry_queries::render_geometry_view_hit_test_line(
            geometry_queries::render_view_geometry(self.view),
            placement.line,
            x - placement.origin.x,
        )?;
        Some(TextHit {
            line: placement.line,
            glyph,
        })
    }

    /// Select the whole text block containing the nearest visible hit.
    pub(crate) fn block_range_at(self, point: Point) -> Option<Range<u32>> {
        let hit = self.hit_test(point)?;
        let text = self.view.text();
        let owner = text_paint::render_line_owner_box(&text.line(hit.line)?);
        text.lines()
            .iter()
            .filter(|line| text_paint::render_line_owner_box(&line) == owner)
            .filter_map(|line| {
                self.text_intersections(line.id(), text_paint::render_line_glyphs(&line))
                    .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
            })
            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
    }

    /// The host tests its projected cell rectangles; the engine owns traversal
    /// and span interpretation. This also supports cells split across regions.
    pub(crate) fn table_at_where(
        self,
        mut contains_box: impl FnMut(BoxId) -> bool,
    ) -> Option<TableHit> {
        let boxes = self.view.boxes();
        let mut hit: Option<(TableHit, BoxId)> = None;
        for table_box in (0..box_queries::len(boxes))
            .map(layout_state::box_id_new)
            .filter(|&index| box_queries::is_table(boxes, index))
        {
            let Some(table) = box_queries::render_box_view_table(boxes, table_box) else {
                continue;
            };
            for cell in table.cells() {
                if contains_box(cell.box_id()) {
                    if hit.as_ref().is_none_or(|previous| {
                        semantic_queries::render_semantics_view_contains(
                            semantic_queries::render_view_semantics(self.view),
                            previous.1,
                            cell.box_id(),
                        )
                    }) {
                        hit = Some((TableHit::from_cell(table_box, cell), cell.box_id()));
                    }
                }
            }
        }
        hit.map(|(hit, _)| hit)
    }

    pub(crate) fn image_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Option<ImageId> {
        self.image_hit_at_lines(point, placements)
            .map(|(hit, _, _)| hit.image)
    }

    fn image_hit_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Option<(ImageHit, BoxId, LineId)> {
        let mut best = None;
        for placement in placements {
            let Some(line) = self.view.text().line(placement.line) else {
                continue;
            };
            let delta = placement.origin - line.point();
            let clip = text_paint::line_overflow_clip(self.view.text(), placement.line);
            for image in fragment_paint::render_fragment_view_images_for_line(
                self.view.fragments(),
                placement.line,
            )
            .iter()
            {
                let visible = fragment_paint::render_image_fragment_visible_rect(&image)
                    + placement.origin.to_vec2();
                let visible = clip.map_or(visible, |clip| clip.apply_to(visible - delta) + delta);
                if visible.width() > 0.0 && visible.height() > 0.0 && visible.contains(point) {
                    let index = image.image();
                    if best.is_some_and(|(_, _, line)| {
                        self.paint_rank(line) > self.paint_rank(placement.line)
                    }) {
                        continue;
                    }
                    best = Some((
                        ImageHit {
                            image: index,
                            is_equation: export_queries::render_export_view_is_equation(
                                self.view.export(),
                                index,
                            ),
                        },
                        fragment_paint::render_image_fragment_box_id(&image),
                        placement.line,
                    ));
                }
            }
        }
        best
    }
}
