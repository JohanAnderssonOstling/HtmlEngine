use super::columns::TableCellPlacement;
use crate::layout::{FragmentWriter, LayoutReader, PhysicalBorderEdge, UsedBorderInsets, border_pattern, physical_borders};
use crate::layout_model::{DecorationFragment, TableColumnGroupSpan, TableColumnTrack};
use html_style_model::{BorderStyle, UsedStyleView};
use kurbo::{Point, Rect};

#[derive(Clone, Copy, Debug)]
struct CollapsedBorder {
    style: BorderStyle,
    width: f64,
    color: u32,
    source_priority: u8,
    order: u32,
    visible: bool,
}

impl CollapsedBorder {
    fn from_edge(edge: PhysicalBorderEdge, source_priority: u8, order: u32, visible: bool) -> Option<Self> {
        (edge.style == BorderStyle::Hidden || (edge.style != BorderStyle::None && edge.width > 0.0)).then_some(Self { style: edge.style, width: edge.width, color: edge.color, source_priority, order, visible })
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
        self.width > current.width
            || (self.width == current.width
                && (self.style_priority() > current.style_priority()
                    || (self.style_priority() == current.style_priority() && (self.source_priority > current.source_priority || (self.source_priority == current.source_priority && self.order >= current.order)))))
    }
}

/// The authoritative collapsed-border result for one table. Horizontal edges
/// are indexed by `(row_boundary, column)` and vertical edges by
/// `(row, column_boundary)`.
pub(super) struct CollapsedBorderGrid {
    rows: usize,
    columns: usize,
    horizontal: Vec<Option<CollapsedBorder>>,
    vertical: Vec<Option<CollapsedBorder>>,
    next_order: u32,
}

impl CollapsedBorderGrid {
    pub(super) fn build(reader: &LayoutReader<'_>, table_box_idx: usize, row_boxes: &[u32], row_groups: &[u32], placements: &[TableCellPlacement], columns: &[TableColumnTrack], column_groups: &[TableColumnGroupSpan]) -> Self {
        let column_count = placements.iter().map(|placement| placement.col.saturating_add(placement.colspan)).max().unwrap_or(0);
        let mut grid = Self { rows: row_boxes.len(), columns: column_count, horizontal: vec![None; (row_boxes.len() + 1) * column_count], vertical: vec![None; row_boxes.len() * (column_count + 1)], next_order: 0 };

        // Lowest-precedence sources are registered first. The conflict
        // comparison still resolves width and style before source origin.
        grid.register_table(reader.style(table_box_idx), 1);
        for group in column_groups {
            grid.register_column_group(reader.used_style(group.style), group.start, group.span, 2);
        }
        for (column, track) in columns.iter().enumerate().take(column_count) {
            grid.register_column(reader.used_style(track.width_style), column, 3);
        }
        for &group_idx in row_groups {
            let group_rows = row_boxes.iter().enumerate().filter_map(|(row, &row_idx)| (reader.get_parent(row_idx as usize) == Some(group_idx as usize)).then_some(row)).collect::<Vec<_>>();
            if let (Some(&first), Some(&last)) = (group_rows.first(), group_rows.last()) {
                grid.register_row_group(reader.style(group_idx as usize), first, last + 1, 4);
            }
        }
        for (row, &row_idx) in row_boxes.iter().enumerate() {
            grid.register_row(reader.style(row_idx as usize), row, 5);
        }
        for placement in placements {
            grid.register_cell(reader.style(placement.cell_idx), *placement, 6);
        }
        grid
    }

    pub(super) fn outer_insets(&self) -> UsedBorderInsets {
        UsedBorderInsets { top: self.horizontal_boundary_max(0) * 0.5, right: self.vertical_boundary_max(self.columns) * 0.5, bottom: self.horizontal_boundary_max(self.rows) * 0.5, left: self.vertical_boundary_max(0) * 0.5 }
    }

    pub(super) fn cell_insets(&self, placement: TableCellPlacement) -> UsedBorderInsets {
        let row_end = placement.row.saturating_add(placement.rowspan).min(self.rows);
        let col_end = placement.col.saturating_add(placement.colspan).min(self.columns);
        UsedBorderInsets {
            top: (placement.col..col_end).filter_map(|col| self.horizontal_at(placement.row, col)).map(|edge| edge.width).fold(0.0, f64::max) * 0.5,
            right: (placement.row..row_end).filter_map(|row| self.vertical_at(row, col_end)).map(|edge| edge.width).fold(0.0, f64::max) * 0.5,
            bottom: (placement.col..col_end).filter_map(|col| self.horizontal_at(row_end, col)).map(|edge| edge.width).fold(0.0, f64::max) * 0.5,
            left: (placement.row..row_end).filter_map(|row| self.vertical_at(row, placement.col)).map(|edge| edge.width).fold(0.0, f64::max) * 0.5,
        }
    }

    pub(super) fn emit(&self, fragments: &mut FragmentWriter<'_>, table_box_idx: usize, origin: Point, column_starts: &[f64], column_widths: &[f64], row_starts: &[f64], row_heights: &[f64]) {
        for boundary in 0..=self.rows {
            let Some(anchor) = boundary_anchor(row_starts, row_heights, boundary) else { continue };
            for column in 0..self.columns {
                let Some(edge) = self.horizontal_at(boundary, column) else { continue };
                let Some((&x, &width)) = column_starts.get(column).zip(column_widths.get(column)) else { continue };
                let start_join = self.vertical_joint_width(boundary, column) * 0.5;
                let end_join = self.vertical_joint_width(boundary, column + 1) * 0.5;
                if let Some(fragment) = edge_fragment(Rect::new(origin.x + x - start_join, origin.y + anchor - edge.width * 0.5, origin.x + x + width + end_join, origin.y + anchor + edge.width * 0.5), edge, true) {
                    fragments.push_block_decoration(table_box_idx as u32, fragment);
                }
            }
        }
        for row in 0..self.rows {
            let Some((&y, &height)) = row_starts.get(row).zip(row_heights.get(row)) else { continue };
            for boundary in 0..=self.columns {
                let Some(edge) = self.vertical_at(row, boundary) else { continue };
                let Some(anchor) = boundary_anchor(column_starts, column_widths, boundary) else { continue };
                let start_join = self.horizontal_joint_width(row, boundary) * 0.5;
                let end_join = self.horizontal_joint_width(row + 1, boundary) * 0.5;
                if let Some(fragment) = edge_fragment(Rect::new(origin.x + anchor - edge.width * 0.5, origin.y + y - start_join, origin.x + anchor + edge.width * 0.5, origin.y + y + height + end_join), edge, false) {
                    fragments.push_block_decoration(table_box_idx as u32, fragment);
                }
            }
        }
    }

    fn register_table(&mut self, style: UsedStyleView<'_>, priority: u8) {
        let borders = physical_borders(&style);
        let visible = style.visibility() == html_style_model::Visibility::Visible;
        self.register_horizontal(0, 0, self.columns, borders.top, priority, visible);
        self.register_horizontal(self.rows, 0, self.columns, borders.bottom, priority, visible);
        self.register_vertical(0, self.rows, 0, borders.left, priority, visible);
        self.register_vertical(0, self.rows, self.columns, borders.right, priority, visible);
    }

    fn register_row_group(&mut self, style: UsedStyleView<'_>, start: usize, end: usize, priority: u8) {
        let borders = physical_borders(&style);
        let visible = style.visibility() == html_style_model::Visibility::Visible;
        self.register_horizontal(start, 0, self.columns, borders.top, priority, visible);
        self.register_horizontal(end.min(self.rows), 0, self.columns, borders.bottom, priority, visible);
        self.register_vertical(start, end.min(self.rows), 0, borders.left, priority, visible);
        self.register_vertical(start, end.min(self.rows), self.columns, borders.right, priority, visible);
    }

    fn register_row(&mut self, style: UsedStyleView<'_>, row: usize, priority: u8) {
        let borders = physical_borders(&style);
        let visible = style.visibility() == html_style_model::Visibility::Visible;
        self.register_horizontal(row, 0, self.columns, borders.top, priority, visible);
        self.register_horizontal((row + 1).min(self.rows), 0, self.columns, borders.bottom, priority, visible);
        self.register_vertical(row, row + 1, 0, borders.left, priority, visible);
        self.register_vertical(row, row + 1, self.columns, borders.right, priority, visible);
    }

    fn register_column_group(&mut self, style: UsedStyleView<'_>, start: usize, span: usize, priority: u8) {
        let end = start.saturating_add(span).min(self.columns);
        let borders = physical_borders(&style);
        let visible = style.visibility() == html_style_model::Visibility::Visible;
        self.register_vertical(0, self.rows, start.min(self.columns), borders.left, priority, visible);
        self.register_vertical(0, self.rows, end, borders.right, priority, visible);
        self.register_horizontal(0, start, end, borders.top, priority, visible);
        self.register_horizontal(self.rows, start, end, borders.bottom, priority, visible);
    }

    fn register_column(&mut self, style: UsedStyleView<'_>, column: usize, priority: u8) {
        if column >= self.columns {
            return;
        }
        let borders = physical_borders(&style);
        let visible = style.visibility() == html_style_model::Visibility::Visible;
        self.register_vertical(0, self.rows, column, borders.left, priority, visible);
        self.register_vertical(0, self.rows, column + 1, borders.right, priority, visible);
        self.register_horizontal(0, column, column + 1, borders.top, priority, visible);
        self.register_horizontal(self.rows, column, column + 1, borders.bottom, priority, visible);
    }

    fn register_cell(&mut self, style: UsedStyleView<'_>, placement: TableCellPlacement, priority: u8) {
        let row_end = placement.row.saturating_add(placement.rowspan).min(self.rows);
        let col_end = placement.col.saturating_add(placement.colspan).min(self.columns);
        let borders = physical_borders(&style);
        let visible = style.visibility() == html_style_model::Visibility::Visible;
        self.register_horizontal(placement.row, placement.col, col_end, borders.top, priority, visible);
        self.register_horizontal(row_end, placement.col, col_end, borders.bottom, priority, visible);
        self.register_vertical(placement.row, row_end, placement.col, borders.left, priority, visible);
        self.register_vertical(placement.row, row_end, col_end, borders.right, priority, visible);
    }

    fn register_horizontal(&mut self, boundary: usize, start: usize, end: usize, edge: PhysicalBorderEdge, priority: u8, visible: bool) {
        let order = self.take_order();
        let Some(candidate) = CollapsedBorder::from_edge(edge, priority, order, visible) else { return };
        for column in start.min(self.columns)..end.min(self.columns) {
            let index = boundary.min(self.rows) * self.columns + column;
            register(&mut self.horizontal[index], candidate);
        }
    }

    fn register_vertical(&mut self, start: usize, end: usize, boundary: usize, edge: PhysicalBorderEdge, priority: u8, visible: bool) {
        let order = self.take_order();
        let Some(candidate) = CollapsedBorder::from_edge(edge, priority, order, visible) else { return };
        for row in start.min(self.rows)..end.min(self.rows) {
            let index = row * (self.columns + 1) + boundary.min(self.columns);
            register(&mut self.vertical[index], candidate);
        }
    }

    fn take_order(&mut self) -> u32 {
        let order = self.next_order;
        self.next_order = self.next_order.saturating_add(1);
        order
    }

    fn horizontal_at(&self, boundary: usize, column: usize) -> Option<CollapsedBorder> {
        (boundary <= self.rows && column < self.columns).then(|| self.horizontal[boundary * self.columns + column]).flatten()
    }

    fn vertical_at(&self, row: usize, boundary: usize) -> Option<CollapsedBorder> {
        (row < self.rows && boundary <= self.columns).then(|| self.vertical[row * (self.columns + 1) + boundary]).flatten()
    }

    fn horizontal_boundary_max(&self, boundary: usize) -> f64 {
        (0..self.columns).filter_map(|column| self.horizontal_at(boundary, column)).map(|edge| edge.width).fold(0.0, f64::max)
    }

    pub(super) fn vertical_boundary_max(&self, boundary: usize) -> f64 {
        (0..self.rows).filter_map(|row| self.vertical_at(row, boundary)).map(|edge| edge.width).fold(0.0, f64::max)
    }

    fn vertical_joint_width(&self, row_boundary: usize, column_boundary: usize) -> f64 {
        [row_boundary.checked_sub(1), (row_boundary < self.rows).then_some(row_boundary)].into_iter().flatten().filter_map(|row| self.vertical_at(row, column_boundary)).map(|edge| edge.width).fold(0.0, f64::max)
    }

    fn horizontal_joint_width(&self, row_boundary: usize, column_boundary: usize) -> f64 {
        [column_boundary.checked_sub(1), (column_boundary < self.columns).then_some(column_boundary)].into_iter().flatten().filter_map(|column| self.horizontal_at(row_boundary, column)).map(|edge| edge.width).fold(0.0, f64::max)
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
        starts.last().zip(sizes.last()).map(|(start, size)| start + size)
    } else {
        None
    }
}

fn edge_fragment(rect: Rect, edge: CollapsedBorder, horizontal: bool) -> Option<DecorationFragment> {
    if !edge.visible {
        return None;
    }
    if rect.width() <= 0.0 || rect.height() <= 0.0 || edge.color & 0xFF == 0 {
        return None;
    }
    let pattern = border_pattern(edge.style, horizontal)?;
    Some(DecorationFragment::patterned(rect, edge.color, false, false, pattern))
}
