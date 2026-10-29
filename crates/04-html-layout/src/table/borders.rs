use super::columns::{TableCellPlacement, TableColumnLayout, row_group_range};
use crate::layout::paint_program::PaintStyleSource;
use crate::layout::{
    LayoutEngine, LayoutReader, PhysicalBorderEdge, UsedBorderInsets, border_pattern,
    physical_borders,
};
use crate::layout_model::{TableColumnGroupSpan, TableColumnTrack};
use html_style_model::{BorderStyle, TextDirection, UsedStyleView, Visibility};
use kurbo::{Point, Rect};
use std::{mem::swap, ops::Range};

#[derive(Clone, Copy, Debug)]
struct CollapsedBorder {
    style: BorderStyle,
    width: f64,
    source: PaintStyleSource,
    side: u8,
    source_priority: u8,
    order: u32,
    visible: bool,
}

impl CollapsedBorder {
    fn from_edge(
        edge: PhysicalBorderEdge,
        source_priority: u8,
        order: u32,
        visible: bool,
        source: PaintStyleSource,
    ) -> Option<Self> {
        (edge.style == BorderStyle::Hidden || (edge.style != BorderStyle::None && edge.width > 0.0))
            .then_some(Self {
                style: edge.style,
                width: edge.width,
                source,
                side: edge.side,
                source_priority,
                order,
                visible,
            })
    }

    fn style_priority(self) -> u8 {
        match self.style {
            BorderStyle::Hidden => u8::MAX,
            BorderStyle::Solid => 3,
            BorderStyle::Dashed => 2,
            BorderStyle::Dotted => 1,
            BorderStyle::Groove => 1,
            BorderStyle::Ridge => 2,
            BorderStyle::None => 0,
        }
    }

    fn wins_over(self, current: Self) -> bool {
        if self.style == BorderStyle::Hidden {
            return true;
        }
        if current.style == BorderStyle::Hidden {
            return false;
        }
        let precedence = |edge: Self| {
            (
                edge.width,
                edge.style_priority(),
                edge.source_priority,
                edge.order,
            )
        };
        precedence(self) >= precedence(current)
    }
}

/// The authoritative collapsed-border result for one table. Horizontal edges
/// are indexed by `(row_boundary, column)` and vertical edges by
/// `(row, column_boundary)`.
pub(super) struct CollapsedBorderGrid {
    rtl: bool,
    rows: usize,
    columns: usize,
    horizontal: Vec<Option<CollapsedBorder>>,
    vertical: Vec<Option<CollapsedBorder>>,
    next_order: u32,
}

impl CollapsedBorderGrid {
    pub(super) fn build(
        reader: &LayoutReader<'_>,
        table_box_idx: usize,
        row_boxes: &[u32],
        row_groups: &[u32],
        placements: &[TableCellPlacement],
        columns: &[TableColumnTrack],
        column_groups: &[TableColumnGroupSpan],
    ) -> Self {
        let column_count = placements
            .iter()
            .map(|placement| placement.col.saturating_add(placement.colspan))
            .max()
            .unwrap_or(0);
        let mut grid = Self {
            rtl: reader.style(table_box_idx).direction() == TextDirection::Rtl,
            rows: row_boxes.len(),
            columns: column_count,
            horizontal: vec![None; (row_boxes.len() + 1) * column_count],
            vertical: vec![None; row_boxes.len() * (column_count + 1)],
            next_order: 0,
        };

        // Lowest-precedence sources are registered first. The conflict
        // comparison still resolves width and style before source origin.
        grid.register_area(
            reader.style(table_box_idx),
            0..row_boxes.len(),
            0..column_count,
            1,
            PaintStyleSource::Box(table_box_idx as u32),
        );
        for group in column_groups {
            grid.register_area(
                reader.used_style(group.style),
                0..row_boxes.len(),
                group.start..group.start.saturating_add(group.span),
                2,
                PaintStyleSource::Style {
                    indices: group.style,
                    context: table_box_idx as u32,
                },
            );
        }
        for (column, track) in columns.iter().enumerate().take(column_count) {
            grid.register_area(
                reader.used_style(track.width_style),
                0..row_boxes.len(),
                column..column + 1,
                3,
                PaintStyleSource::Style {
                    indices: track.width_style,
                    context: table_box_idx as u32,
                },
            );
        }
        for &group_idx in row_groups {
            if let Some(range) = row_group_range(reader, row_boxes, group_idx as usize) {
                grid.register_area(
                    reader.style(group_idx as usize),
                    range,
                    0..column_count,
                    4,
                    PaintStyleSource::Box(group_idx),
                );
            }
        }
        for (row, &row_idx) in row_boxes.iter().enumerate() {
            grid.register_area(
                reader.style(row_idx as usize),
                row..row + 1,
                0..column_count,
                5,
                PaintStyleSource::Box(row_idx),
            );
        }
        for placement in placements {
            grid.register_area(
                reader.style(placement.cell_idx),
                placement.row..placement.row.saturating_add(placement.rowspan),
                placement.col..placement.col.saturating_add(placement.colspan),
                6,
                PaintStyleSource::Box(placement.cell_idx as u32),
            );
        }
        grid
    }

    pub(super) fn outer_insets(&self) -> UsedBorderInsets {
        UsedBorderInsets {
            top: self.horizontal_boundary_max(0) * 0.5,
            right: self.vertical_boundary_max(if self.rtl { 0 } else { self.columns }) * 0.5,
            bottom: self.horizontal_boundary_max(self.rows) * 0.5,
            left: self.vertical_boundary_max(if self.rtl { self.columns } else { 0 }) * 0.5,
        }
    }

    pub(super) fn cell_insets(&self, placement: TableCellPlacement) -> UsedBorderInsets {
        let row_end = placement
            .row
            .saturating_add(placement.rowspan)
            .min(self.rows);
        let col_end = placement
            .col
            .saturating_add(placement.colspan)
            .min(self.columns);
        UsedBorderInsets {
            top: (placement.col..col_end)
                .filter_map(|col| self.horizontal_at(placement.row, col))
                .map(|edge| edge.width)
                .fold(0.0, f64::max)
                * 0.5,
            right: (placement.row..row_end)
                .filter_map(|row| {
                    self.vertical_at(row, if self.rtl { placement.col } else { col_end })
                })
                .map(|edge| edge.width)
                .fold(0.0, f64::max)
                * 0.5,
            bottom: (placement.col..col_end)
                .filter_map(|col| self.horizontal_at(row_end, col))
                .map(|edge| edge.width)
                .fold(0.0, f64::max)
                * 0.5,
            left: (placement.row..row_end)
                .filter_map(|row| {
                    self.vertical_at(row, if self.rtl { col_end } else { placement.col })
                })
                .map(|edge| edge.width)
                .fold(0.0, f64::max)
                * 0.5,
        }
    }

    pub(super) fn emit(
        &self,
        session: &mut LayoutEngine<'_, '_>,
        table_box_idx: usize,
        origin: Point,
        columns: &TableColumnLayout,
        row_starts: &[f64],
        row_heights: &[f64],
    ) {
        let table_width = columns.table_width;
        let column_starts = &columns.starts;
        let column_widths = &columns.widths;
        for boundary in 0..=self.rows {
            let Some(anchor) = boundary_anchor(row_starts, row_heights, boundary) else {
                continue;
            };
            for column in 0..self.columns {
                let Some(edge) = self.horizontal_at(boundary, column) else {
                    continue;
                };
                let Some((&x, &width)) = column_starts.get(column).zip(column_widths.get(column))
                else {
                    continue;
                };
                let start_join = self.vertical_joint_width(boundary, column) * 0.5;
                let end_join = self.vertical_joint_width(boundary, column + 1) * 0.5;
                let (left, right) = if self.rtl {
                    (
                        table_width - x - width - end_join,
                        table_width - x + start_join,
                    )
                } else {
                    (x - start_join, x + width + end_join)
                };
                emit_edge(
                    session,
                    table_box_idx,
                    Rect::new(
                        origin.x + left,
                        origin.y + anchor - edge.width * 0.5,
                        origin.x + right,
                        origin.y + anchor + edge.width * 0.5,
                    ),
                    edge,
                    true,
                );
            }
        }
        for row in 0..self.rows {
            let Some((&y, &height)) = row_starts.get(row).zip(row_heights.get(row)) else {
                continue;
            };
            for boundary in 0..=self.columns {
                let Some(edge) = self.vertical_at(row, boundary) else {
                    continue;
                };
                let Some(anchor) = boundary_anchor(column_starts, column_widths, boundary) else {
                    continue;
                };
                let anchor = if self.rtl {
                    table_width - anchor
                } else {
                    anchor
                };
                let start_join = self.horizontal_joint_width(row, boundary) * 0.5;
                let end_join = self.horizontal_joint_width(row + 1, boundary) * 0.5;
                emit_edge(
                    session,
                    table_box_idx,
                    Rect::new(
                        origin.x + anchor - edge.width * 0.5,
                        origin.y + y - start_join,
                        origin.x + anchor + edge.width * 0.5,
                        origin.y + y + height + end_join,
                    ),
                    edge,
                    false,
                );
            }
        }
    }

    /// Register the four outer edges of any table part in logical grid coordinates.
    fn register_area(
        &mut self,
        style: UsedStyleView<'_>,
        rows: Range<usize>,
        columns: Range<usize>,
        priority: u8,
        source: PaintStyleSource,
    ) {
        let rows = rows.start.min(self.rows)..rows.end.min(self.rows);
        let columns = columns.start.min(self.columns)..columns.end.min(self.columns);
        let mut borders = physical_borders(&style);
        if self.rtl {
            swap(&mut borders.left, &mut borders.right);
        }
        let visible = style.visibility() == Visibility::Visible;
        for (boundary, edge) in [(rows.start, borders.top), (rows.end, borders.bottom)] {
            let Some(candidate) =
                CollapsedBorder::from_edge(edge, priority, self.take_order(), visible, source)
            else {
                continue;
            };
            for column in columns.clone() {
                let index = boundary * self.columns + column;
                register(&mut self.horizontal[index], candidate);
            }
        }
        for (boundary, edge) in [(columns.start, borders.left), (columns.end, borders.right)] {
            let Some(candidate) =
                CollapsedBorder::from_edge(edge, priority, self.take_order(), visible, source)
            else {
                continue;
            };
            for row in rows.clone() {
                let index = row * (self.columns + 1) + boundary;
                register(&mut self.vertical[index], candidate);
            }
        }
    }

    fn take_order(&mut self) -> u32 {
        let order = self.next_order;
        self.next_order = self.next_order.saturating_add(1);
        order
    }

    fn horizontal_at(&self, boundary: usize, column: usize) -> Option<CollapsedBorder> {
        (boundary <= self.rows && column < self.columns)
            .then(|| self.horizontal[boundary * self.columns + column])
            .flatten()
    }

    fn vertical_at(&self, row: usize, boundary: usize) -> Option<CollapsedBorder> {
        (row < self.rows && boundary <= self.columns)
            .then(|| self.vertical[row * (self.columns + 1) + boundary])
            .flatten()
    }

    fn horizontal_boundary_max(&self, boundary: usize) -> f64 {
        (0..self.columns)
            .filter_map(|column| self.horizontal_at(boundary, column))
            .map(|edge| edge.width)
            .fold(0.0, f64::max)
    }

    pub(super) fn vertical_boundary_max(&self, boundary: usize) -> f64 {
        (0..self.rows)
            .filter_map(|row| self.vertical_at(row, boundary))
            .map(|edge| edge.width)
            .fold(0.0, f64::max)
    }

    fn vertical_joint_width(&self, row_boundary: usize, column_boundary: usize) -> f64 {
        [
            row_boundary.checked_sub(1),
            (row_boundary < self.rows).then_some(row_boundary),
        ]
        .into_iter()
        .flatten()
        .filter_map(|row| self.vertical_at(row, column_boundary))
        .map(|edge| edge.width)
        .fold(0.0, f64::max)
    }

    fn horizontal_joint_width(&self, row_boundary: usize, column_boundary: usize) -> f64 {
        [
            column_boundary.checked_sub(1),
            (column_boundary < self.columns).then_some(column_boundary),
        ]
        .into_iter()
        .flatten()
        .filter_map(|column| self.horizontal_at(row_boundary, column))
        .map(|edge| edge.width)
        .fold(0.0, f64::max)
    }
}

fn register(slot: &mut Option<CollapsedBorder>, candidate: CollapsedBorder) {
    if slot.is_none_or(|current| candidate.wins_over(current)) {
        *slot = Some(candidate);
    }
}

fn boundary_anchor(starts: &[f64], sizes: &[f64], boundary: usize) -> Option<f64> {
    if boundary < starts.len() {
        starts.get(boundary).copied()
    } else if boundary == starts.len() {
        starts
            .last()
            .zip(sizes.last())
            .map(|(start, size)| start + size)
    } else {
        None
    }
}

fn emit_edge(
    session: &mut LayoutEngine<'_, '_>,
    owner: usize,
    rect: Rect,
    edge: CollapsedBorder,
    horizontal: bool,
) {
    if !edge.visible || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let Some(pattern) = border_pattern(edge.style, horizontal) else {
        return;
    };
    crate::layout::decorations::record_bound_border(
        session,
        owner,
        rect,
        edge.source,
        edge.side,
        pattern,
    );
}
