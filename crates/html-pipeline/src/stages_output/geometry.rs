//! Intrinsic text geometry. Page/column projection belongs to the embedding UI.

use html_engine_model::layout_state;
use crate::stages::fragment_paint;
use super::text_paint;
use super::{DocumentRead, LineId, RenderLine, RenderLineTextFragment, RenderView};
use kurbo::{Point, Rect};
use std::ops::Range;

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

    /// Horizontal caret coordinate relative to the layout line's origin.
    /// Affinity selects the appropriate fragment at bidi/inline boundaries.
    pub fn caret_x(self, line_idx: LineId, glyph: u32, affinity: CaretAffinity) -> Option<f64> {
        let text = self.doc.render_view(self.glyph_metrics).text();
        let line = text.line(line_idx)?;
        let position = glyph.clamp(line.start(), line.end());
        let fragment = fragment_at(
            text_paint::render_text_view_line_text_fragments(text, line_idx)
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
                .unwrap_or(text_paint::line_optical_offset_x(&line)),
        )
    }

    pub fn caret_span(self, line: LineId, range: Range<u32>) -> Option<(f64, f64)> {
        if range.start > range.end {
            return None;
        }
        let start = self.caret_x(line, range.start, CaretAffinity::Leading)?;
        let end = self.caret_x(line, range.end, CaretAffinity::Trailing)?;
        Some((start.min(end), start.max(end)))
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
        let mut placement =
            text_paint::line_placement(self.doc.render_view(self.glyph_metrics).text(), line.id())
                .expect("fragment belongs to a published line");
        let direction = if text_paint::fragment_is_rtl(&fragment) {
            -1.0
        } else {
            1.0
        };
        let origin = text_paint::line_optical_offset_x(&line)
            + text_paint::fragment_offset_x(&fragment)
            + if text_paint::fragment_is_rtl(&fragment) {
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

// Resolve shared logical boundaries using the requested caret affinity.
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

/// Closest logical caret on one line. `x` is relative to the line origin.
/// Shaping clusters, bidi, tracking, conditional hyphens, and justification
/// are interpreted here, so UI code need not reproduce spacing rules.
pub fn render_geometry_view_hit_test_line<'a>(
    value: RenderGeometryView<'a>,
    line_idx: LineId,
    x: f64,
) -> Option<u32> {
    if !x.is_finite() {
        return None;
    }
    let text = value.doc.render_view(value.glyph_metrics).text();
    let line = text.line(line_idx)?;
    let mut best = None;
    let mut distance = f64::INFINITY;
    for fragment in text_paint::render_text_view_line_text_fragments(text, line_idx)
        .into_iter()
        .flatten()
        .filter(|fragment| fragment.visible())
    {
        for (position, caret) in value.fragment_carets(&line, &fragment) {
            if text_paint::is_character_cluster_boundary(text, position) {
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

/// Last spatial traversal rank ending at or above the offset.
pub fn render_geometry_view_end_spatial_rank_at<'a>(
    value: RenderGeometryView<'a>,
    offset_y: f64,
    epsilon: f64,
) -> Option<usize> {
    let output = &value.doc.layout_state.line_output;
    let threshold = offset_y + epsilon;
    let lo = output.spatial_order.partition_point(|id| {
        let line = &output.lines[layout_state::line_id_index(*id)];
        line.point.y + line.height <= threshold
    });
    (0..lo).rev().find(|&rank| {
        render_geometry_view_line_vertical_bounds(value, output.spatial_order[rank])
            .is_some_and(|bounds| bounds.end <= threshold)
    })
}

/// Narrow document-space ink candidates without allocating or scanning
/// every image. Bounds include tall and overflowing image carriers.
pub fn render_geometry_view_lines_at_y<'a>(
    value: RenderGeometryView<'a>,
    y: f64,
) -> impl Iterator<Item = LineId> + 'a {
    let output = &value.doc.layout_state.line_output;
    let start = output.spatial_order.partition_point(|id| {
        let line = &output.lines[layout_state::line_id_index(*id)];
        line.point.y + line.height < y - output.hit_test_below
    });
    let end = output.spatial_order.partition_point(|id| {
        let line = &output.lines[layout_state::line_id_index(*id)];
        line.point.y + line.height <= y + output.hit_test_above
    });
    output.spatial_order[start..end.max(start)].iter().copied()
}

/// First spatial traversal rank extending below a document-space offset.
/// Use `text().spatial_order()[rank]` to obtain the stable line identity.
/// Returns the traversal length when no line extends below the offset.
pub fn render_geometry_view_start_spatial_rank_at<'a>(
    value: RenderGeometryView<'a>,
    offset_y: f64,
    epsilon: f64,
) -> usize {
    let view = value.doc.render_view(value.glyph_metrics);
    let output = &value.doc.layout_state.line_output;
    let threshold = offset_y + epsilon;
    let mut lo = output.spatial_order.partition_point(|id| {
        let line = &output.lines[layout_state::line_id_index(*id)];
        line.point.y + line.height <= threshold
    });
    for image in fragment_paint::render_fragment_view_images(view.fragments()).iter() {
        let id = layout_state::line_id_index(fragment_paint::render_image_fragment_line(&image));
        let Some(&rank) = output.spatial_order.ranks().get(id) else {
            continue;
        };
        let rank = rank as usize;
        if rank < lo {
            // The nominal line bottom is already at or above the threshold.
            // Inspect each image once instead of rescanning its whole line.
            let rect = fragment_paint::render_image_fragment_visible_rect(&image);
            if rect.width() > 0.0
                && rect.height() > 0.0
                && output
                    .lines
                    .get(id)
                    .is_some_and(|line| line.point.y + rect.y1 > threshold)
            {
                lo = rank;
            }
        }
    }
    lo
}

/// Intrinsic vertical extent of a line and its visible replaced content,
/// in document coordinates. Image offsets and object-fit clipping apply.
pub fn render_geometry_view_line_vertical_bounds<'a>(
    value: RenderGeometryView<'a>,
    line_idx: LineId,
) -> Option<Range<f64>> {
    let view = value.doc.render_view(value.glyph_metrics);
    let line = view.text().line(line_idx)?;
    let mut bounds = line.point().y..line.point().y + line.height();
    for image in
        fragment_paint::render_fragment_view_images_for_line(view.fragments(), line_idx).iter()
    {
        let rect = fragment_paint::render_image_fragment_visible_rect(&image);
        if rect.width() > 0.0 && rect.height() > 0.0 {
            bounds.start = bounds.start.min(line.point().y + rect.y0);
            bounds.end = bounds.end.max(line.point().y + rect.y1);
        }
    }
    Some(bounds)
}

/// First line in spatial order intersecting the interval, including edges.
pub fn render_geometry_view_line_intersecting_vertical_range<'a>(
    value: RenderGeometryView<'a>,
    top: f64,
    bottom: f64,
) -> Option<LineId> {
    if top > bottom {
        return None;
    }
    value
        .doc
        .layout_state
        .line_output
        .spatial_order
        .iter()
        .copied()
        .find(|id| {
            render_geometry_view_line_vertical_bounds(value, *id)
                .is_some_and(|bounds| bounds.end >= top && bounds.start <= bottom)
        })
}

/// Overflow-clipped rectangles in document coordinates, one per visible
/// text fragment.
/// They are independent of the application's viewport and pagination.
pub fn render_geometry_view_line_selection_rects<'a>(
    value: RenderGeometryView<'a>,
    line_idx: LineId,
    range: Range<u32>,
) -> impl Iterator<Item = Rect> + 'a {
    let text = value.doc.render_view(value.glyph_metrics).text();
    let line = text.line(line_idx);
    let clip = text_paint::line_overflow_clip(text, line_idx);
    text_paint::render_text_view_line_text_fragments(text, line_idx)
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
            let (x0, x1) = value.caret_span(line_idx, selected)?;
            let rect = Rect::new(
                line.point().x + x0,
                line.point().y,
                line.point().x + x1,
                line.point().y + line.height(),
            );
            let rect = clip.map_or(rect, |clip| clip.apply_to(rect));
            (rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
        })
}

pub fn render_view_geometry<'a>(value: RenderView<'a>) -> RenderGeometryView<'a> {
    RenderGeometryView {
        doc: value.doc,
        glyph_metrics: value.glyph_metrics,
    }
}

/// Selection bounds for explicit line placements, using intrinsic overflow clips.
pub fn selection_bounds_lines<'a>(
    value: RenderGeometryView<'a>,
    range: Range<u32>,
    placements: impl IntoIterator<Item = (LineId, Point)>,
) -> Option<Rect> {
    if range.start >= range.end {
        return None;
    }
    let text = value.doc.render_view(value.glyph_metrics).text();
    placements
        .into_iter()
        .flat_map(|(line, origin)| {
            let source = text.line(line).map_or(Point::ZERO, |line| line.point());
            let delta = origin - source;
            let clip = text_paint::line_overflow_clip(text, line);
            render_geometry_view_line_selection_rects(value, line, range.clone())
                .map(move |rect| clip.map_or(rect, |clip| clip.apply_to(rect)))
                .filter(|rect| rect.width() > 0.0 && rect.height() > 0.0)
                .map(move |rect| rect + delta)
        })
        .reduce(|left, right| left.union(right))
        .filter(|rect| rect.width() > 0.0 || rect.height() > 0.0)
}

/// Visible logical intersections on one source line, without merging fragments.
pub fn text_intersections<'a>(
    value: RenderGeometryView<'a>,
    line: LineId,
    range: Range<u32>,
) -> impl Iterator<Item = Range<u32>> + 'a {
    let text = value.doc.render_view(value.glyph_metrics).text();
    text_paint::render_text_view_line_text_fragments(text, line)
        .into_iter()
        .flatten()
        .filter_map(move |fragment| {
            let glyphs = fragment.glyphs();
            let start = range.start.max(glyphs.start);
            let end = range.end.min(glyphs.end);
            (fragment.visible() && start < end).then_some(start..end)
        })
}
