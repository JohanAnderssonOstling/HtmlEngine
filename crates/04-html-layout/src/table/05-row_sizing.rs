//! Resolve row heights from cell measurements, spanning cells, and authored heights.

use super::weighted_share;

use super::columns::percentage_component;
use super::grid::TableCellPlacement;
use super::s01_grid::PreparedTableGrid;
use super::s04_rows::PreparedCell;
use crate::{LayoutEngine, resolve_vertical_size};
use html_style_model::UsedPreferredSize as PreferredSize;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Default)]
struct RowMeasure {
    block_size: f64,
    percent: Option<f64>,
    constrained: bool,
    has_rowspan_start: bool,
}

#[derive(Clone, Copy, Debug)]
struct RowspanRequirement {
    start: usize,
    count: usize,
    minimum: f64,
}

pub(super) fn resolve_row_heights(
    engine: &LayoutEngine<'_, '_>,
    prepared: &PreparedTableGrid,
    cells: &[PreparedCell],
    explicit_table_height: Option<f64>,
    definite_row_area_height: Option<f64>,
) -> Vec<f64> {
    let placements = &prepared.grid.placements;
    let spacing = prepared.vertical_spacing;
    let mut measures = initial_row_measures(engine, prepared.rows, placements, cells);

    let mut spanning = placements
        .iter()
        .copied()
        .zip(cells)
        .filter_map(|(placement, cell)| {
            let count = placement
                .rowspan
                .min(prepared.rows.len().saturating_sub(placement.row));
            (count > 1).then_some(RowspanRequirement {
                start: placement.row,
                count,
                minimum: cell.height,
            })
        })
        .collect::<Vec<_>>();
    spanning.sort_by(compare_rowspan_requirements);
    for requirement in spanning {
        distribute_rowspan(
            &mut measures[requirement.start..requirement.end()],
            requirement.minimum,
            spacing,
        );
    }

    for (group_idx, range) in &prepared.row_groups {
        let Some(group_height) = resolve_vertical_size(
            engine.read.reader.style(*group_idx as usize).height(),
            explicit_table_height,
            0.0,
        ) else {
            continue;
        };
        distribute_sized_area(
            &mut measures[range.clone()],
            group_height,
            spacing,
            group_height,
        );
    }

    if let Some(row_area_height) = definite_row_area_height {
        distribute_sized_area(
            &mut measures,
            row_area_height,
            spacing,
            explicit_table_height.unwrap(),
        );
    }

    measures.into_iter().map(|row| row.block_size).collect()
}

impl RowspanRequirement {
    fn end(self) -> usize {
        self.start + self.count
    }
}

fn initial_row_measures(
    engine: &LayoutEngine<'_, '_>,
    rows: &[u32],
    placements: &[TableCellPlacement],
    cells: &[PreparedCell],
) -> Vec<RowMeasure> {
    let mut measures = vec![RowMeasure::default(); rows.len()];
    for (row, &row_box_idx) in rows.iter().enumerate() {
        let height = engine.read.reader.style(row_box_idx as usize).height();
        if let Some(minimum) = resolve_vertical_size(height, None, 0.0) {
            measures[row].block_size = minimum;
        }
        measures[row].percent = percentage_component(height).map(|percent| percent.max(0.0));
        measures[row].constrained = is_constrained_size(height);
    }

    for (index, placement) in placements.iter().enumerate() {
        if placement.row >= measures.len() {
            continue;
        }
        if placement.rowspan > 1 {
            measures[placement.row].has_rowspan_start = true;
            continue;
        }
        measures[placement.row].block_size = measures[placement.row]
            .block_size
            .max(cells.get(index).map_or(0.0, |cell| cell.height));
        let height = engine.read.reader.style(placement.cell_idx).height();
        measures[placement.row].percent = max_option(
            measures[placement.row].percent,
            percentage_component(height).map(|percent| percent.max(0.0)),
        );
        measures[placement.row].constrained |= is_constrained_size(height);
    }
    measures
}

fn compare_rowspan_requirements(left: &RowspanRequirement, right: &RowspanRequirement) -> Ordering {
    if left.start == right.start && left.count == right.count {
        return right.minimum.total_cmp(&left.minimum);
    }
    let left_enclosed = left.start >= right.start && left.end() <= right.end();
    let right_enclosed = right.start >= left.start && right.end() <= left.end();
    match (left_enclosed, right_enclosed) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => left.start.cmp(&right.start),
    }
}

fn row_surplus(rows: &[RowMeasure], desired_size: f64, spacing: f64) -> f64 {
    desired_size
        - spacing * rows.len().saturating_sub(1) as f64
        - rows.iter().map(|row| row.block_size).sum::<f64>()
}

fn distribute_rowspan(rows: &mut [RowMeasure], desired_size: f64, spacing: f64) {
    let extra = row_surplus(rows, desired_size, spacing);
    if rows.is_empty() || extra <= 0.0 {
        return;
    }
    if distribute_weighted(&mut rows[1..], extra, |row| {
        row.has_rowspan_start.then_some(1.0)
    }) || distribute_weighted(rows, extra, |row| {
        (row.block_size > 0.0 && (!row.constrained || row.percent.is_some()))
            .then_some(row.block_size)
    }) {
        return;
    }
    if rows.iter().all(|row| row.block_size == 0.0) {
        rows.last_mut().unwrap().block_size += extra;
    } else {
        distribute_weighted(rows, extra, |row| {
            (row.block_size > 0.0).then_some(row.block_size)
        });
    }
}

fn distribute_sized_area(rows: &mut [RowMeasure], desired_size: f64, spacing: f64, basis: f64) {
    let mut extra = row_surplus(rows, desired_size, spacing);
    if rows.is_empty() || extra <= 0.0 {
        return;
    }
    let deficit = |row: &RowMeasure| {
        let deficit = (basis * row.percent? - row.block_size).max(0.0);
        (deficit > 0.0).then_some(deficit)
    };
    let total_deficit = rows.iter().filter_map(deficit).sum::<f64>();
    if total_deficit > 0.0 {
        let distributed = extra.min(total_deficit);
        distribute_weighted(rows, distributed, deficit);
        extra -= distributed;
        if extra <= 0.0 {
            return;
        }
    }
    if distribute_weighted(rows, extra, |row| {
        (row.block_size > 0.0 && !row.constrained).then_some(row.block_size)
    }) {
        return;
    }
    if rows
        .iter()
        .all(|row| row.block_size == 0.0 || row.constrained)
    {
        let has_unconstrained_empty = rows
            .iter()
            .any(|row| row.block_size == 0.0 && !row.constrained);
        distribute_weighted(rows, extra, |row| {
            (row.block_size == 0.0 && (!has_unconstrained_empty || !row.constrained)).then_some(1.0)
        });
    } else {
        distribute_weighted(rows, extra, |row| {
            (row.block_size > 0.0).then_some(row.block_size)
        });
    }
}

/// `None` excludes a row; selected rows share the excess by nonnegative weight.
/// The selector reads each row before that row is changed, so both passes use
/// the same weights without allocating a separate recipient list.
fn distribute_weighted(
    rows: &mut [RowMeasure],
    extra: f64,
    weight: impl Fn(&RowMeasure) -> Option<f64>,
) -> bool {
    let mut count = 0;
    let total_weight = rows
        .iter()
        .filter_map(&weight)
        .map(|weight| {
            count += 1;
            weight.max(0.0)
        })
        .sum::<f64>();
    if count == 0 || extra <= 0.0 {
        return false;
    }
    for row in rows {
        if let Some(weight) = weight(row) {
            row.block_size += extra * weighted_share(weight.max(0.0), total_weight, count);
        }
    }
    true
}

fn is_constrained_size(size: PreferredSize) -> bool {
    matches!(
        size,
        PreferredSize::Px(_)
            | PreferredSize::Percent(_)
            | PreferredSize::Calc { .. }
            | PreferredSize::Comparison { .. }
    )
}

fn max_option(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RowMeasure, RowspanRequirement, compare_rowspan_requirements, distribute_rowspan,
        distribute_sized_area,
    };

    fn row(size: f64, constrained: bool) -> RowMeasure {
        RowMeasure {
            block_size: size,
            constrained,
            ..RowMeasure::default()
        }
    }

    fn sizes(rows: &[RowMeasure]) -> Vec<f64> {
        rows.iter().map(|row| row.block_size).collect()
    }

    #[test]
    fn rowspan_grows_nonempty_auto_rows_proportionally() {
        let mut rows = [row(45.0, false), row(15.0, false)];
        distribute_rowspan(&mut rows, 100.0, 0.0);
        assert_eq!(sizes(&rows), vec![75.0, 25.0]);
    }

    #[test]
    fn rowspan_preserves_constrained_rows_when_an_auto_row_can_grow() {
        let mut rows = [row(30.0, true), row(20.0, false)];
        distribute_rowspan(&mut rows, 100.0, 0.0);
        assert_eq!(sizes(&rows), vec![30.0, 70.0]);
    }

    #[test]
    fn rowspan_grows_constrained_rows_proportionally_as_fallback() {
        let mut rows = [row(20.0, true), row(20.0, true), row(40.0, true)];
        distribute_rowspan(&mut rows, 100.0, 0.0);
        assert_eq!(sizes(&rows), vec![25.0, 25.0, 50.0]);
    }

    #[test]
    fn rowspan_gives_an_all_empty_deficit_to_the_last_row() {
        let mut rows = [row(0.0, false), row(0.0, false), row(0.0, false)];
        distribute_rowspan(&mut rows, 99.0, 0.0);
        assert_eq!(sizes(&rows), vec![0.0, 0.0, 99.0]);
    }

    #[test]
    fn rowspan_internal_spacing_reduces_required_row_growth() {
        let mut rows = [
            row(0.0, false),
            row(0.0, false),
            row(0.0, false),
            row(0.0, false),
        ];
        distribute_rowspan(&mut rows, 60.0, 20.0);
        assert_eq!(sizes(&rows), vec![0.0; 4]);
    }

    #[test]
    fn percentage_row_growth_is_capped_by_available_table_surplus() {
        let mut rows = [
            RowMeasure {
                block_size: 20.0,
                percent: Some(0.3),
                constrained: true,
                ..RowMeasure::default()
            },
            row(340.0, false),
        ];
        distribute_sized_area(&mut rows, 460.0, 0.0, 460.0);
        assert_eq!(sizes(&rows), vec![120.0, 340.0]);
    }

    #[test]
    fn enclosed_rowspan_sorts_before_its_container() {
        let outer = RowspanRequirement {
            start: 0,
            count: 4,
            minimum: 50.0,
        };
        let inner = RowspanRequirement {
            start: 1,
            count: 2,
            minimum: 100.0,
        };
        assert_eq!(
            compare_rowspan_requirements(&inner, &outer),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_rowspan_requirements(&outer, &inner),
            std::cmp::Ordering::Greater
        );
    }
}
