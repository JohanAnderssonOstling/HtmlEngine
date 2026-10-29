//! Document geometry without a viewport, columns, or a window system.

use crate::layout::{CaretAffinity, PositionedShapedLine, RenderView};
use crate::selection::{SelectionGeometry, TableHit};
use crate::targets::{LinkTarget, Targets};
use kurbo::{Point, Rect};
use std::ops::Range;

/// A caller-chosen position for a document line. It can belong to a scrolled
/// surface, a page, a popup, or another arrangement of the same document.
pub use crate::render::LinePlacement;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextHit {
    pub line: usize,
    pub glyph: u32,
}

/// One clipped rectangle for an opaque caller-labelled logical range.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeRect<Id> {
    pub id: Id,
    pub line: usize,
    pub bounds: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageHit {
    pub image: u32,
    pub is_equation: bool,
}

/// Several identities can coincide: linked text or an image may lie in a cell.
/// Hosts choose which action to perform; hit testing never activates content.
#[derive(Clone, Debug)]
pub struct ContentHit<'a> {
    pub text: Option<TextHit>,
    pub link: Option<LinkTarget<'a>>,
    pub image: Option<ImageHit>,
    pub table: Option<TableHit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordHitPolicy {
    InkOnly,
    Nearest,
}

/// Intrinsic reading region, retaining cell identity for side-by-side content.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadingLineGeometry {
    pub glyphs: Range<u32>,
    pub bounds: Rect,
    pub cell: Option<usize>,
}

/// Intrinsic geometry, optionally refined by a host's native caret positions.
#[derive(Clone, Copy)]
pub struct Geometry<'a> {
    view: RenderView<'a>,
    shaped: &'a [PositionedShapedLine],
}

impl<'a> Geometry<'a> {
    pub fn range_geometry<Id: Clone>(
        self,
        ranges: impl IntoIterator<Item = (Id, Range<u32>)>,
    ) -> Vec<RangeRect<Id>> {
        self.range_geometry_lines(
            ranges,
            self.view.text().lines().iter().map(|line| LinePlacement {
                line: line.index(),
                origin: line.point(),
            }),
        )
    }

    /// Visit caller placements once, indexing range intersections before native
    /// caret/clip queries. Output preserves placement order, then input range
    /// order, including duplicate ranges and repeated placements. Labels carry
    /// no engine interpretation or appearance.
    pub fn range_geometry_lines<Id: Clone>(
        self,
        ranges: impl IntoIterator<Item = (Id, Range<u32>)>,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Vec<RangeRect<Id>> {
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
        let mut result = Vec::new();
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
                result.extend(self.placed_selection_rects(placement, range.clone()).map(
                    |bounds| RangeRect {
                        id: id.clone(),
                        line: placement.line,
                        bounds,
                    },
                ));
            }
        }
        result
    }

    pub fn new(view: RenderView<'a>) -> Self {
        Self { view, shaped: &[] }
    }

    pub fn with_shaped_lines(mut self, shaped: &'a [PositionedShapedLine]) -> Self {
        self.shaped = shaped;
        self
    }

    pub fn content_at(self, point: Point) -> Option<ContentHit<'a>> {
        let boxes = self.view.boxes();
        self.content_at_lines(
            point,
            self.view.text().lines().iter().map(|line| LinePlacement {
                line: line.index(),
                origin: line.point(),
            }),
            |index| {
                boxes
                    .point(index)
                    .zip(boxes.size(index))
                    .is_some_and(|(origin, size)| {
                        Rect::from_origin_size(origin, size).contains(point)
                    })
            },
        )
    }

    /// Placements and projected cell containment are supplied by the host.
    /// Text/link hits require visible ink bounds, unlike nearest-caret selection.
    pub fn content_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
        contains_box: impl FnMut(usize) -> bool,
    ) -> Option<ContentHit<'a>> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        let placements: Vec<_> = placements.into_iter().collect();
        let text = self.ink_hit(point, &placements);
        let image_hit = self.image_hit_at_lines(point, placements);
        let targets = Targets::new(self.view);
        let link = image_hit
            .and_then(|(_, box_idx)| targets.box_link(box_idx))
            .or_else(|| text.and_then(|hit| targets.link(hit.glyph)));
        let image = image_hit.map(|(image, _)| image);
        let table = self.table_at_where(contains_box);
        (text.is_some() || image.is_some() || table.is_some()).then_some(ContentHit {
            text,
            link,
            image,
            table,
        })
    }

    pub fn link_at(self, point: Point) -> Option<LinkTarget<'a>> {
        self.link_at_lines(
            point,
            self.view.text().lines().iter().map(|line| LinePlacement {
                line: line.index(),
                origin: line.point(),
            }),
        )
    }

    /// Link-only hover queries avoid traversing unrelated table cells.
    pub fn link_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Option<LinkTarget<'a>> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        let placements: Vec<_> = placements.into_iter().collect();
        let targets = Targets::new(self.view);
        self.image_hit_at_lines(point, placements.iter().copied())
            .and_then(|(_, box_idx)| targets.box_link(box_idx))
            .or_else(|| {
                self.ink_hit(point, &placements)
                    .and_then(|hit| targets.link(hit.glyph))
            })
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

    pub fn word_at(self, point: Point, policy: WordHitPolicy) -> Option<Range<u32>> {
        self.word_at_lines(
            point,
            self.view.text().lines().iter().map(|line| LinePlacement {
                line: line.index(),
                origin: line.point(),
            }),
            policy,
        )
    }

    /// Resolve caret affinity to the word actually under the pointer. Only the
    /// caller's candidate lines can supply a word, including at region edges.
    pub fn word_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
        policy: WordHitPolicy,
    ) -> Option<Range<u32>> {
        let placements: Vec<_> = placements.into_iter().collect();
        let hit = self.hit_test_lines(point, placements.iter().copied())?;
        let candidates = [Some(hit.glyph), hit.glyph.checked_sub(1)];
        for glyph in candidates.into_iter().flatten() {
            if self
                .selection_bounds_lines(glyph..glyph.saturating_add(1), placements.iter().copied())
                .is_some_and(|bounds| bounds.contains(point))
            {
                return self.view.text().word_range_at(glyph);
            }
        }
        if policy == WordHitPolicy::InkOnly {
            return None;
        }
        candidates.into_iter().flatten().find_map(|glyph| {
            self.selection_bounds_lines(
                glyph..glyph.saturating_add(1),
                placements.iter().copied(),
            )?;
            self.view.text().word_range_at(glyph)
        })
    }

    pub fn reading_line(self, index: usize) -> Option<ReadingLineGeometry> {
        let line = self.view.text().line(index)?;
        let (left, right) = self.caret_span(index, line.glyphs())?;
        self.placed_reading_line(
            LinePlacement {
                line: index,
                origin: line.point(),
            },
            Rect::new(
                line.point().x + left,
                line.point().y,
                line.point().x + right,
                line.point().y + line.height(),
            ),
        )
    }

    /// Apply intrinsic clipping and find the containing cell/block. The caller
    /// supplies a fallback horizontal band when no owning box has a width.
    pub fn placed_reading_line(
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
        let clip = text.line_overflow_clip(placement.line).map(|clip| {
            clip.apply_to(Rect::new(
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::INFINITY,
            )) + delta
        });
        let clipped = |rect: Rect| clip.map_or(rect, |clip| rect.intersect(clip));
        let glyphs = text
            .line_text_fragments(placement.line)?
            .filter(|fragment| fragment.visible())
            .map(|fragment| fragment.glyphs())
            .filter(|range| !range.is_empty())
            .filter(|range| {
                let Some((left, right)) = self.caret_span(placement.line, range.clone()) else {
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
        let owner = line.owner_box_idx();
        let cell = std::iter::once(owner)
            .chain(boxes.ancestors(owner))
            .find(|&index| boxes.is_table_cell(index));
        let region = cell.unwrap_or(owner);
        let (x, width) = boxes
            .point(region)
            .zip(boxes.size(region))
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

    pub fn selection_contains(self, selection: &SelectionGeometry, point: Point) -> bool {
        let boxes = self.view.boxes();
        self.selection_contains_lines(
            selection,
            point,
            self.view.text().lines().iter().map(|line| LinePlacement {
                line: line.index(),
                origin: line.point(),
            }),
            |index| {
                boxes
                    .point(index)
                    .zip(boxes.size(index))
                    .is_some_and(|(origin, size)| {
                        Rect::from_origin_size(origin, size).contains(point)
                    })
            },
        )
    }

    pub fn selection_contains_lines(
        self,
        selection: &SelectionGeometry,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
        mut contains_box: impl FnMut(usize) -> bool,
    ) -> bool {
        if !point.x.is_finite() || !point.y.is_finite() {
            return false;
        }
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

    pub fn caret_x(self, line: usize, glyph: u32, affinity: CaretAffinity) -> Option<f64> {
        self.view
            .geometry()
            .caret_x(line, glyph, affinity, self.shaped)
    }

    pub fn caret_span(self, line: usize, range: Range<u32>) -> Option<(f64, f64)> {
        self.view.geometry().caret_span(line, range, self.shaped)
    }

    pub fn line_selection_rects(
        self,
        line: usize,
        range: Range<u32>,
    ) -> impl Iterator<Item = Rect> + 'a {
        let clip = self.view.text().line_overflow_clip(line);
        self.view
            .geometry()
            .line_selection_rects(line, range, self.shaped)
            .map(move |rect| clip.map_or(rect, |clip| clip.apply_to(rect)))
            .filter(|rect| rect.width() > 0.0 && rect.height() > 0.0)
    }

    pub fn placed_selection_rects(
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

    pub fn selection_rects(self, range: Range<u32>) -> Vec<Rect> {
        self.view
            .text()
            .lines()
            .iter()
            .flat_map(|line| self.line_selection_rects(line.index(), range.clone()))
            .collect()
    }

    pub fn selection_bounds(self, range: Range<u32>) -> Option<Rect> {
        self.selection_bounds_lines(
            range,
            self.view.text().lines().iter().map(|line| LinePlacement {
                line: line.index(),
                origin: line.point(),
            }),
        )
    }

    pub fn selection_bounds_lines(
        self,
        range: Range<u32>,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Option<Rect> {
        if range.start >= range.end {
            return None;
        }
        placements
            .into_iter()
            .flat_map(|placement| self.placed_selection_rects(placement, range.clone()))
            .reduce(|left, right| left.union(right))
            .filter(|rect| rect.width() > 0.0 || rect.height() > 0.0)
    }

    pub fn text_intersections(
        self,
        line: usize,
        range: Range<u32>,
    ) -> impl Iterator<Item = Range<u32>> + 'a {
        self.view
            .text()
            .line_text_fragments(line)
            .into_iter()
            .flatten()
            .filter_map(move |fragment| {
                let glyphs = fragment.glyphs();
                let start = range.start.max(glyphs.start);
                let end = range.end.min(glyphs.end);
                (fragment.visible() && start < end).then_some(start..end)
            })
    }

    pub fn selection_endpoints(self, range: Range<u32>) -> (Option<Point>, Option<Point>) {
        self.selection_endpoints_lines(
            range,
            self.view.text().lines().iter().map(|line| LinePlacement {
                line: line.index(),
                origin: line.point(),
            }),
        )
    }

    pub fn selection_endpoints_lines(
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
                leading = self
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
                trailing = self
                    .caret_x(placement.line, range.end, CaretAffinity::Trailing)
                    .map(|x| {
                        Point::new(placement.origin.x + x, placement.origin.y + line.height())
                    });
            }
        }
        (leading, trailing)
    }

    pub fn hit_test(self, point: Point) -> Option<TextHit> {
        self.hit_test_lines(
            point,
            self.view.text().lines().iter().map(|line| LinePlacement {
                line: line.index(),
                origin: line.point(),
            }),
        )
    }

    /// The host chooses the candidate lines, for example only those in the
    /// column receiving a click. Nearest-line and bidi/cluster rules are shared.
    pub fn hit_test_lines(
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
            if line.glyphs().is_empty() {
                continue;
            }
            let nearest = self
                .placed_selection_rects(placement, line.glyphs())
                .filter(|rect| rect.width() > 0.0 && rect.height() > 0.0)
                .map(|rect| {
                    let dy = (rect.y0 - point.y).max(point.y - rect.y1).max(0.0);
                    let dx = (rect.x0 - point.x).max(point.x - rect.x1).max(0.0);
                    ((dy, dx), point.x.clamp(rect.x0, rect.x1))
                })
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((candidate, x)) = nearest {
                if candidate < distance {
                    distance = candidate;
                    best = Some((placement, x));
                }
            }
        }
        let (placement, x) = best?;
        let glyph = self.view.geometry().hit_test_line(
            placement.line,
            x - placement.origin.x,
            self.shaped,
        )?;
        Some(TextHit {
            line: placement.line,
            glyph,
        })
    }

    /// Select the whole text block containing the nearest visible hit.
    pub fn block_range_at(self, point: Point) -> Option<Range<u32>> {
        let hit = self.hit_test(point)?;
        let text = self.view.text();
        let owner = text.line(hit.line)?.owner_box_idx();
        text.lines()
            .iter()
            .filter(|line| line.owner_box_idx() == owner)
            .filter_map(|line| {
                self.text_intersections(line.index(), line.glyphs())
                    .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
            })
            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
    }

    pub fn image_at(self, point: Point) -> Option<u32> {
        self.image_at_lines(
            point,
            self.view.text().lines().iter().map(|line| LinePlacement {
                line: line.index(),
                origin: line.point(),
            }),
        )
    }

    /// Find a table cell in document coordinates, including its spanned rows
    /// and columns. Table export is available through `view.boxes().table()`.
    pub fn table_at(self, point: Point) -> Option<TableHit> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        let boxes = self.view.boxes();
        self.table_at_where(|box_idx| {
            let (Some(origin), Some(size)) = (boxes.point(box_idx), boxes.size(box_idx)) else {
                return false;
            };
            Rect::from_origin_size(origin, size).contains(point)
        })
    }

    /// The host tests its projected cell rectangles; the engine owns traversal
    /// and span interpretation. This also supports cells split across regions.
    pub fn table_at_where(self, mut contains_box: impl FnMut(usize) -> bool) -> Option<TableHit> {
        let boxes = self.view.boxes();
        for table_box in (0..boxes.len()).filter(|&index| boxes.is_table(index)) {
            let Some(table) = boxes.table(table_box) else {
                continue;
            };
            for cell in table.cells() {
                if contains_box(cell.box_idx()) {
                    return Some(TableHit::from_cell(table_box, cell));
                }
            }
        }
        None
    }

    pub fn image_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Option<u32> {
        self.image_hit_at_lines(point, placements)
            .map(|(hit, _)| hit.image)
    }

    fn image_hit_at_lines(
        self,
        point: Point,
        placements: impl IntoIterator<Item = LinePlacement>,
    ) -> Option<(ImageHit, usize)> {
        for placement in placements {
            let Some(line) = self.view.text().line(placement.line) else {
                continue;
            };
            let delta = placement.origin - line.point();
            let clip = self.view.text().line_overflow_clip(placement.line);
            for image in self.view.fragments().images_for_line(placement.line).iter() {
                let visible = image.visible_rect() + placement.origin.to_vec2();
                let visible = clip.map_or(visible, |clip| clip.apply_to(visible - delta) + delta);
                if visible.width() > 0.0 && visible.height() > 0.0 && visible.contains(point) {
                    let index = image.image_idx();
                    return Some((
                        ImageHit {
                            image: index,
                            is_equation: self.view.export().is_equation(index),
                        },
                        image.box_idx(),
                    ));
                }
            }
        }
        None
    }
}
