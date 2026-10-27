//! Intrinsic text geometry. Page/column projection belongs to the embedding UI.

use super::{DocumentRead, RenderLine, RenderLineTextFragment};
use crate::ShapedLine;
use kurbo::Rect;
use std::ops::Range;

/// A backend-shaped fragment positioned within its logical layout line.
/// Applications may retain these for visible lines and pass them to queries;
/// an empty slice uses the engine's retained character geometry instead.
pub struct PositionedShapedLine {
    pub offset_x: f64,
    pub shaped: ShapedLine,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaretAffinity {
    Leading,
    Trailing,
}

#[derive(Clone, Copy)]
pub struct RenderGeometryView<'a> {
    pub(super) glyph_metrics: &'a crate::layout_model::GlyphMetrics,
    pub(super) doc: DocumentRead<'a>,
}

impl<'a> RenderGeometryView<'a> {
    /// Intrinsic vertical extent of a line and its visible replaced content,
    /// in document coordinates. Image offsets and object-fit clipping apply.
    pub fn line_vertical_bounds(self, line_idx: usize) -> Option<Range<f64>> {
        let view = self.doc.render_view(self.glyph_metrics);
        let line = view.text().line(line_idx)?;
        let mut bounds = line.point_y()..line.point_y() + line.height();
        for image in view.fragments().images_for_line(line_idx).iter() {
            let rect = Rect::from_origin_size(image.offset(), image.size()).intersect(image.clip());
            if rect.width() > 0.0 && rect.height() > 0.0 {
                bounds.start = bounds.start.min(line.point_y() + rect.y0);
                bounds.end = bounds.end.max(line.point_y() + rect.y1);
            }
        }
        Some(bounds)
    }

    /// First traversal line extending below a document-space offset. Lines
    /// are stored by nominal bottom edge; replaced-content overflow may extend
    /// an earlier line beyond that ordering.
    pub fn start_line_at(self, offset_y: f64, epsilon: f64) -> usize {
        let view = self.doc.render_view(self.glyph_metrics);
        let threshold = offset_y + epsilon;
        let mut lo = self
            .doc
            .layout_state
            .line_output
            .lines
            .partition_point(|line| line.point.y + line.height <= threshold);
        for image in view.fragments().images().iter() {
            let index = image.line_idx();
            if index < lo
                && self
                    .line_vertical_bounds(index)
                    .is_some_and(|bounds| bounds.end > threshold)
            {
                lo = index;
            }
        }
        lo
    }

    /// Last traversal line ending at or above the document-space offset.
    pub fn end_line_at(self, offset_y: f64, epsilon: f64) -> Option<usize> {
        let threshold = offset_y + epsilon;
        let lo = self
            .doc
            .layout_state
            .line_output
            .lines
            .partition_point(|line| line.point.y + line.height <= threshold);
        (0..lo).rev().find(|&index| {
            self.line_vertical_bounds(index)
                .is_some_and(|bounds| bounds.end <= threshold)
        })
    }

    /// First line intersecting a vertical decoration interval, including edge
    /// contact. Top edges need not be monotonic when line heights differ.
    pub fn line_intersecting_vertical_range(self, top: f64, bottom: f64) -> Option<usize> {
        if top > bottom {
            return None;
        }
        (0..self.doc.render_view(self.glyph_metrics).text().line_count()).find(|&index| {
            self.line_vertical_bounds(index)
                .is_some_and(|bounds| bounds.end >= top && bounds.start <= bottom)
        })
    }

    /// Horizontal caret coordinate relative to the layout line's origin.
    /// Affinity selects the appropriate fragment at bidi/inline boundaries.
    pub fn caret_x(
        self,
        line_idx: usize,
        glyph: u32,
        affinity: CaretAffinity,
        shaped: &[PositionedShapedLine],
    ) -> Option<f64> {
        let text = self.doc.render_view(self.glyph_metrics).text();
        let line = text.line(line_idx)?;
        let position = glyph.clamp(line.start(), line.end());
        let native = fragment_at(
            shaped
                .iter()
                .filter(|fragment| fragment.shaped.line_index == line_idx),
            |fragment| fragment.shaped.text_range.clone(),
            position,
            affinity,
        );
        if let Some(fragment) = native
            && let Some(x) = fragment.shaped.x_for_text_position(position)
        {
            return Some(line.optical_offset_x() + fragment.offset_x + x);
        }
        let fragment = fragment_at(
            text.line_text_fragments(line_idx)
                .into_iter()
                .flatten()
                .filter(|fragment| fragment.visible()),
            |fragment| fragment.glyphs(),
            position,
            affinity,
        );
        Some(
            fragment
                .and_then(|fragment| {
                    self.fragment_carets(&line, &fragment)
                        .find(|(glyph, _)| *glyph == position)
                        .map(|(_, x)| x)
                })
                .unwrap_or(line.optical_offset_x()),
        )
    }

    pub fn caret_span(
        self,
        line: usize,
        range: Range<u32>,
        shaped: &[PositionedShapedLine],
    ) -> Option<(f64, f64)> {
        if range.start > range.end {
            return None;
        }
        let start = self.caret_x(line, range.start, CaretAffinity::Leading, shaped)?;
        let end = self.caret_x(line, range.end, CaretAffinity::Trailing, shaped)?;
        Some((start.min(end), start.max(end)))
    }

    /// Closest logical caret on one line. `x` is relative to the line origin.
    /// Shaping clusters, bidi, tracking, conditional hyphens, and justification
    /// are interpreted here, so UI code need not reproduce spacing rules.
    pub fn hit_test_line(
        self,
        line_idx: usize,
        x: f64,
        shaped: &[PositionedShapedLine],
    ) -> Option<u32> {
        if !x.is_finite() {
            return None;
        }
        let text = self.doc.render_view(self.glyph_metrics).text();
        let line = text.line(line_idx)?;
        let conditional = line.glyphs().any(|index| {
            self.doc.inputs.text.content().character_at(index as usize) == Some('\u{00ad}' as u32)
        });
        let overrides = text
            .line_glyph_advances(line_idx)
            .is_some_and(|runs| !runs.is_empty());
        if line.letter_spacing() == 0.0 && line.word_spacing() == 0.0 && !conditional && !overrides
        {
            let optical = line.optical_offset_x();
            let carets = shaped
                .iter()
                .filter(|fragment| fragment.shaped.line_index == line_idx)
                .flat_map(|fragment| {
                    fragment
                        .shaped
                        .caret_stops
                        .iter()
                        .zip(fragment.shaped.cluster_boundaries.iter())
                        .enumerate()
                        .filter_map(move |(offset, (&stop, &boundary))| {
                            let position = fragment
                                .shaped
                                .text_range
                                .start
                                .saturating_add(offset as u32);
                            let caret = optical + fragment.offset_x + f64::from(stop);
                            (boundary
                                && position <= fragment.shaped.text_range.end
                                && caret.is_finite())
                            .then_some((position, caret))
                        })
                });
            if let Some((position, _)) = carets
                .min_by(|(_, left), (_, right)| (x - left).abs().total_cmp(&(x - right).abs()))
            {
                return Some(position);
            }
        }
        let mut best = None;
        let mut distance = f64::INFINITY;
        for fragment in text
            .line_text_fragments(line_idx)
            .into_iter()
            .flatten()
            .filter(|fragment| fragment.visible())
        {
            for (position, caret) in self.fragment_carets(&line, &fragment) {
                if text.is_character_cluster_boundary(position) {
                    let delta = (x - caret).abs();
                    if delta <= distance {
                        distance = delta;
                        best = Some(position);
                    }
                }
            }
        }
        best.or(Some(line.start()))
    }

    /// Rectangles in document coordinates, one per visible text fragment.
    /// They are independent of the application's viewport and pagination.
    pub fn line_selection_rects<'b>(
        self,
        line_idx: usize,
        range: Range<u32>,
        shaped: &'b [PositionedShapedLine],
    ) -> impl Iterator<Item = Rect> + 'b
    where
        'a: 'b,
    {
        let text = self.doc.render_view(self.glyph_metrics).text();
        let line = text.line(line_idx);
        text.line_text_fragments(line_idx)
            .into_iter()
            .flatten()
            .filter_map(move |fragment| {
                if !fragment.visible() || range.start >= range.end {
                    return None;
                }
                let selected =
                    range.start.max(fragment.glyphs().start)..range.end.min(fragment.glyphs().end);
                if selected.start >= selected.end {
                    return None;
                }
                let line = line.as_ref()?;
                let (x0, x1) = self.caret_span(line_idx, selected, shaped)?;
                Some(Rect::new(
                    line.point_x() + x0,
                    line.point_y(),
                    line.point_x() + x1,
                    line.point_y() + line.height(),
                ))
            })
    }

    fn fragment_carets<'b>(
        self,
        line: &'b RenderLine,
        fragment: &RenderLineTextFragment,
    ) -> impl Iterator<Item = (u32, f64)> + 'b
    where
        'a: 'b,
    {
        let range = fragment.glyphs();
        let mut placement = self
            .doc
            .render_view(self.glyph_metrics)
            .text()
            .line_placement(line.index())
            .expect("fragment belongs to a published line");
        let direction = if fragment.is_rtl() { -1.0 } else { 1.0 };
        let origin = line.optical_offset_x()
            + fragment.offset_x()
            + if fragment.is_rtl() {
                range
                    .clone()
                    .map(|index| {
                        placement
                            .at(index)
                            .map_or(0.0, |character| character.advance)
                    })
                    .sum()
            } else {
                0.0
            };
        (range.start..=range.end).scan(origin, move |x, position| {
            let caret = *x;
            if position < range.end {
                *x += direction
                    * placement
                        .at(position)
                        .map_or(0.0, |character| character.advance);
            }
            Some((position, caret))
        })
    }
}

// Resolve shared boundaries identically for native and retained fragments.
fn fragment_at<T>(
    fragments: impl Iterator<Item = T>,
    glyphs: impl Fn(&T) -> Range<u32>,
    position: u32,
    affinity: CaretAffinity,
) -> Option<T> {
    let mut adjacent = None;
    for fragment in fragments {
        let range = glyphs(&fragment);
        let leading = range.start == position;
        let trailing = range.start < position && position <= range.end;
        if match affinity {
            CaretAffinity::Leading => leading,
            CaretAffinity::Trailing => trailing,
        } {
            return Some(fragment);
        }
        if adjacent.is_none() && (leading || trailing) {
            adjacent = Some(fragment);
        }
    }
    adjacent
}
