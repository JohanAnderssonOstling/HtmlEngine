use super::borders::CollapsedBorderGrid;
use super::columns::{TableCellPlacement, TableColumnLayout, span_extent};
use crate::layout::{LayoutEngine, resolve_vertical_size};
use crate::layout_model::{Children, LayoutMode};
use html_style_model::VerticalAlignValue;
use kurbo::{Point, Size, Vec2};
use std::time::Instant;

pub(super) struct TableCellMeasurements {
    pub(super) baseline_offsets: Vec<Option<f64>>,
    pub(super) row_heights: Vec<f64>,
    pub(super) row_baselines: Vec<Option<f64>>,
}

pub(super) fn distribute_row_growth(heights: &mut [f64], constrained: &[bool], candidates: &[usize], extra: f64) {
    if extra <= 0.0 || candidates.is_empty() {
        return;
    }
    let mut recipients = candidates.iter().copied().filter(|&row| !constrained[row]).collect::<Vec<_>>();
    if recipients.is_empty() {
        recipients.extend_from_slice(candidates);
    }
    let add = extra / recipients.len() as f64;
    for row in recipients {
        heights[row] += add;
    }
}

pub(super) fn resolve_row_heights(session: &LayoutEngine<'_, '_>, rows: &[u32], row_groups: &[u32], mut row_heights: Vec<f64>, v_spacing: f64, explicit_table_height: Option<f64>, caption_height: f64, outer_border_height: f64) -> Vec<f64> {
    let mut constrained = vec![false; rows.len()];
    for (row, &row_box_idx) in rows.iter().enumerate() {
        if let Some(height) = resolve_vertical_size(session.reader.style(row_box_idx as usize).height(), explicit_table_height, 0.0) {
            row_heights[row] = row_heights[row].max(height);
            constrained[row] = true;
        }
    }

    for &group_idx in row_groups {
        let Some(group_height) = resolve_vertical_size(session.reader.style(group_idx as usize).height(), explicit_table_height, 0.0) else {
            continue;
        };
        let group_rows = rows.iter().enumerate().filter_map(|(row, &row_idx)| (session.reader.get_parent(row_idx as usize) == Some(group_idx as usize)).then_some(row)).collect::<Vec<_>>();
        if group_rows.is_empty() {
            continue;
        }
        let current = group_rows.iter().map(|&row| row_heights[row]).sum::<f64>() + v_spacing * group_rows.len().saturating_sub(1) as f64;
        distribute_row_growth(&mut row_heights, &constrained, &group_rows, (group_height - current).max(0.0));
    }

    if let Some(explicit_height) = explicit_table_height {
        let target_grid_height = (explicit_height - caption_height).max(0.0);
        let natural_grid_height = row_heights.iter().sum::<f64>() + v_spacing * (rows.len() as f64 + 1.0) + outer_border_height;
        let all_rows = (0..row_heights.len()).collect::<Vec<_>>();
        distribute_row_growth(&mut row_heights, &constrained, &all_rows, (target_grid_height - natural_grid_height).max(0.0));
    }
    row_heights
}

pub(super) fn measure_cells(
    session: &mut LayoutEngine<'_, '_>, placements: &[TableCellPlacement], row_count: usize, columns: &TableColumnLayout, table_content_pos: Point, v_spacing: f64, parent_content_height: Option<f64>,
    collapsed_grid: Option<&CollapsedBorderGrid>,
) -> TableCellMeasurements {
    let mut heights = vec![0.0; placements.len()];
    let mut baseline_offsets = vec![None; placements.len()];
    for (index, placement) in placements.iter().enumerate() {
        let span_width = span_extent(&columns.starts, &columns.widths, placement.col, placement.colspan);
        let ((cell_size, baseline), _) = crate::layout::with_isolated_measurement(session, |session| {
            session.without_fragmentation(|session| {
                session.geometry.set_point(placement.cell_idx, Point::new(table_content_pos.x + columns.starts[placement.col], table_content_pos.y + v_spacing));
                let cell_layout = if let Some(grid) = collapsed_grid {
                    session.layout_box(crate::layout::BoxLayoutRequest::collapsed_table_cell(placement.cell_idx, span_width, parent_content_height, grid.cell_insets(*placement)))
                } else {
                    session.layout_box(crate::layout::BoxLayoutRequest::normal(placement.cell_idx, span_width, parent_content_height))
                };
                let baseline = session.fragments.first_baseline_offset(cell_layout.output.lines, session.geometry.point(placement.cell_idx).y);
                (cell_layout.size, baseline)
            })
        });
        heights[index] = cell_size.height;
        baseline_offsets[index] = baseline;
    }

    let mut row_heights = vec![0.0f64; row_count];
    let mut row_baselines = vec![None::<f64>; row_count];
    for (index, placement) in placements.iter().enumerate().filter(|(_, placement)| placement.rowspan == 1) {
        row_heights[placement.row] = row_heights[placement.row].max(heights[index]);
        if cell_uses_baseline_alignment(session, placement.cell_idx)
            && let Some(cell_baseline) = baseline_offsets[index]
        {
            row_baselines[placement.row] = Some(row_baselines[placement.row].map_or(cell_baseline, |current| current.max(cell_baseline)));
        }
    }
    for (index, placement) in placements.iter().enumerate().filter(|(_, placement)| placement.rowspan > 1) {
        let end_row = placement.row.saturating_add(placement.rowspan).min(row_count);
        let current = row_heights[placement.row..end_row].iter().sum::<f64>();
        let deficit = (heights[index] - current).max(0.0);
        if deficit > 0.0 {
            let add = deficit / (end_row - placement.row) as f64;
            for height in &mut row_heights[placement.row..end_row] {
                *height += add;
            }
        }
    }
    TableCellMeasurements { baseline_offsets, row_heights, row_baselines }
}

pub(crate) fn layout_table_row(session: &mut LayoutEngine<'_, '_>, _row_box_idx: usize, cells: Vec<u32>, out_of_flow: Vec<u32>, content_pos: Point, content_width: f64, parent_content_height: Option<f64>) -> Size {
    let timing_started = Instant::now();
    if cells.is_empty() {
        for child in out_of_flow {
            session.defer_absolute_box(child as usize, content_pos);
        }
        return Size::ZERO;
    }
    let column_width = (content_width / cells.len() as f64).max(0.0);
    let mut row_height = 0.0f64;
    for (column, &cell_idx) in cells.iter().enumerate() {
        let cell_idx = cell_idx as usize;
        session.geometry.set_point(cell_idx, content_pos + Vec2::new(column as f64 * column_width, 0.0));
        row_height = row_height.max(session.without_fragmentation(|session| session.layout_box(crate::layout::BoxLayoutRequest::normal(cell_idx, column_width, parent_content_height))).size.height);
    }
    for &cell_idx in &cells {
        let mut size = session.geometry.size(cell_idx as usize);
        size.height = size.height.max(row_height);
        session.geometry.set_size(cell_idx as usize, size);
    }
    for child in out_of_flow {
        session.defer_absolute_box(child as usize, content_pos);
    }
    session.record_timing(|timings| timings.layout_table_row += timing_started.elapsed());
    Size::new(content_width.max(0.0), row_height)
}

pub(super) fn cell_vertical_offset(session: &LayoutEngine<'_, '_>, cell_idx: usize, natural_height: f64, span_height: f64, row_baseline: Option<f64>, cell_baseline: Option<f64>) -> f64 {
    let extra = (span_height - natural_height).max(0.0);
    match session.reader.style(cell_idx).vertical_align() {
        VerticalAlignValue::Top => 0.0,
        VerticalAlignValue::Bottom | VerticalAlignValue::TextBottom => extra,
        VerticalAlignValue::Middle => extra * 0.5,
        VerticalAlignValue::Baseline | VerticalAlignValue::Sub | VerticalAlignValue::Super | VerticalAlignValue::Length(_) | VerticalAlignValue::Percent(_) | VerticalAlignValue::Calc { .. } | VerticalAlignValue::TextTop => {
            match (row_baseline, cell_baseline) {
                (Some(row), Some(cell)) => (row - cell).max(0.0),
                _ => 0.0,
            }
        }
    }
}

fn cell_uses_baseline_alignment(session: &LayoutEngine<'_, '_>, cell_idx: usize) -> bool {
    matches!(
        session.reader.style(cell_idx).vertical_align(),
        VerticalAlignValue::Baseline | VerticalAlignValue::Sub | VerticalAlignValue::Super | VerticalAlignValue::Length(_) | VerticalAlignValue::Percent(_) | VerticalAlignValue::Calc { .. } | VerticalAlignValue::TextTop
    )
}

pub(super) fn cell_has_visible_content(session: &LayoutEngine<'_, '_>, cell_idx: usize, output: &crate::layout::OutputRanges) -> bool {
    if output.has_lines_or_images() {
        return true;
    }
    match session.reader.box_layout_mode(cell_idx) {
        Some(LayoutMode::TableCell(cell)) => !matches!(cell.children, Children::Empty),
        _ => false,
    }
}
