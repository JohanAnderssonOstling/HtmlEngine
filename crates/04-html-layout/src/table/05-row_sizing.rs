use super::columns::{TableCellPlacement, row_group_range};
use super::grid::PreparedTableGrid;
use super::rows::PreparedCell;
use crate::layout::{LayoutEngine, resolve_vertical_size};
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

impl RowspanRequirement {
    fn end(self) -> usize {
        self.start + self.count
    }
}

pub(super) fn resolve_row_heights(
    session: &LayoutEngine<'_, '_>,
    prepared: &PreparedTableGrid,
    rows: &[u32],
    row_groups: &[u32],
    cells: &[PreparedCell],
    explicit_table_height: Option<f64>,
    caption_height: f64,
) -> Vec<f64> {
    let placements = &prepared.grid.placements;
    let spacing = prepared.vertical_spacing;
    let outer_border_height = prepared.outer_borders.vertical();
    let mut measures = initial_row_measures(session, rows, placements, cells);

    let mut spanning = placements
        .iter()
        .copied()
        .zip(cells)
        .filter_map(|(placement, cell)| {
            let count = placement
                .rowspan
                .min(rows.len().saturating_sub(placement.row));
            (count > 1).then_some(RowspanRequirement {
                start: placement.row,
                count,
                minimum: cell.height,
            })
        })
        .collect::<Vec<_>>();
    spanning.sort_by(compare_rowspan_requirements);
    for requirement in spanning {
        distribute_excess(
            &mut measures,
            requirement.start,
            requirement.count,
            requirement.minimum,
            spacing,
            None,
            true,
        );
    }

    for &group_idx in row_groups {
        let Some(group_height) = resolve_vertical_size(
            session.reader.style(group_idx as usize).height(),
            explicit_table_height,
            0.0,
        ) else {
            continue;
        };
        let Some(range) = row_group_range(&session.reader, rows, group_idx as usize) else {
            continue;
        };
        distribute_excess(
            &mut measures,
            range.start,
            range.len(),
            group_height,
            spacing,
            Some(group_height),
            false,
        );
    }

    if let Some(explicit_height) = explicit_table_height {
        let target_grid_height = (explicit_height - caption_height).max(0.0);
        let row_area_height = (target_grid_height - outer_border_height - 2.0 * spacing).max(0.0);
        distribute_excess(
            &mut measures,
            0,
            rows.len(),
            row_area_height,
            spacing,
            Some(explicit_height),
            false,
        );
    }

    measures.into_iter().map(|row| row.block_size).collect()
}

fn initial_row_measures(
    session: &LayoutEngine<'_, '_>,
    rows: &[u32],
    placements: &[TableCellPlacement],
    cells: &[PreparedCell],
) -> Vec<RowMeasure> {
    let mut measures = vec![RowMeasure::default(); rows.len()];
    for (row, &row_box_idx) in rows.iter().enumerate() {
        let height = session.reader.style(row_box_idx as usize).height();
        if let Some(minimum) = resolve_vertical_size(height, None, 0.0) {
            measures[row].block_size = minimum;
        }
        measures[row].percent = percentage_component(height);
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
        let height = session.reader.style(placement.cell_idx).height();
        measures[placement.row].percent = max_option(
            measures[placement.row].percent,
            percentage_component(height),
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

fn distribute_excess(
    rows: &mut [RowMeasure],
    start: usize,
    count: usize,
    desired_size: f64,
    spacing: f64,
    percentage_basis: Option<f64>,
    rowspan_distribution: bool,
) {
    if count == 0 || start >= rows.len() {
        return;
    }
    let end = start.saturating_add(count).min(rows.len());
    let rows = &mut rows[start..end];
    let count = end - start;
    let total_size = rows.iter().map(|row| row.block_size).sum::<f64>();
    let mut extra = desired_size - spacing * count.saturating_sub(1) as f64 - total_size;
    if extra <= 0.0 {
        return;
    }

    if let Some(basis) = percentage_basis {
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
    }

    if rowspan_distribution
        && distribute_weighted(&mut rows[1..], extra, |row| {
            row.has_rowspan_start.then_some(1.0)
        })
    {
        return;
    }

    let is_constrained =
        |row: &RowMeasure| row.constrained && (row.percent.is_none() || percentage_basis.is_some());
    if distribute_weighted(rows, extra, |row| {
        (row.block_size > 0.0 && !is_constrained(row)).then_some(row.block_size)
    }) {
        return;
    }

    let empty_count = rows.iter().filter(|row| row.block_size == 0.0).count();
    if rowspan_distribution && empty_count == count {
        rows[count - 1].block_size += extra;
        return;
    }
    if !rowspan_distribution {
        let constrained_nonempty = rows
            .iter()
            .filter(|row| row.block_size > 0.0 && is_constrained(row))
            .count();
        if empty_count == count || empty_count + constrained_nonempty == count {
            let has_unconstrained_empty = rows
                .iter()
                .any(|row| row.block_size == 0.0 && !is_constrained(row));
            distribute_weighted(rows, extra, |row| {
                (row.block_size == 0.0 && (!has_unconstrained_empty || !is_constrained(row)))
                    .then_some(1.0)
            });
            return;
        }
    }

    distribute_weighted(rows, extra, |row| {
        (row.block_size > 0.0).then_some(row.block_size)
    });
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
            row.block_size += if total_weight <= 0.0 {
                extra / count as f64
            } else {
                extra * weight.max(0.0) / total_weight
            };
        }
    }
    true
}

fn percentage_component(size: PreferredSize) -> Option<f64> {
    match size {
        PreferredSize::Percent(percent) => Some(percent.max(0.0) as f64),
        PreferredSize::Calc {
            absolute_px: 0.0,
            percentage,
            ..
        } => Some(percentage.max(0.0) as f64),
        _ => None,
    }
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
    use super::{RowMeasure, RowspanRequirement, compare_rowspan_requirements, distribute_excess};

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
        distribute_excess(&mut rows, 0, 2, 100.0, 0.0, None, true);
        assert_eq!(sizes(&rows), vec![75.0, 25.0]);
    }

    #[test]
    fn rowspan_preserves_constrained_rows_when_an_auto_row_can_grow() {
        let mut rows = [row(30.0, true), row(20.0, false)];
        distribute_excess(&mut rows, 0, 2, 100.0, 0.0, None, true);
        assert_eq!(sizes(&rows), vec![30.0, 70.0]);
    }

    #[test]
    fn rowspan_grows_constrained_rows_proportionally_as_fallback() {
        let mut rows = [row(20.0, true), row(20.0, true), row(40.0, true)];
        distribute_excess(&mut rows, 0, 3, 100.0, 0.0, None, true);
        assert_eq!(sizes(&rows), vec![25.0, 25.0, 50.0]);
    }

    #[test]
    fn rowspan_gives_an_all_empty_deficit_to_the_last_row() {
        let mut rows = [row(0.0, false), row(0.0, false), row(0.0, false)];
        distribute_excess(&mut rows, 0, 3, 99.0, 0.0, None, true);
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
        distribute_excess(&mut rows, 0, 4, 60.0, 20.0, None, true);
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
        distribute_excess(&mut rows, 0, 2, 460.0, 0.0, Some(460.0), false);
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
