use super::columns::TableCellPlacement;
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
    rows: &[u32],
    row_groups: &[u32],
    placements: &[TableCellPlacement],
    cell_heights: &[f64],
    spacing: f64,
    explicit_table_height: Option<f64>,
    caption_height: f64,
    outer_border_height: f64,
) -> Vec<f64> {
    let mut measures = initial_row_measures(session, rows, placements, cell_heights);

    let mut spanning = placements
        .iter()
        .copied()
        .zip(cell_heights.iter().copied())
        .filter_map(|(placement, minimum)| {
            let count = placement
                .rowspan
                .min(rows.len().saturating_sub(placement.row));
            (count > 1).then_some(RowspanRequirement {
                start: placement.row,
                count,
                minimum,
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
        let group_rows = rows
            .iter()
            .enumerate()
            .filter_map(|(row, &row_idx)| {
                (session.reader.get_parent(row_idx as usize) == Some(group_idx as usize))
                    .then_some(row)
            })
            .collect::<Vec<_>>();
        let (Some(&start), Some(&last)) = (group_rows.first(), group_rows.last()) else {
            continue;
        };
        distribute_excess(
            &mut measures,
            start,
            last - start + 1,
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
    cell_heights: &[f64],
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
            .max(cell_heights.get(index).copied().unwrap_or(0.0));
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
    let count = end - start;
    let indices = start..end;
    let total_size = rows[indices.clone()]
        .iter()
        .map(|row| row.block_size)
        .sum::<f64>();
    let mut extra = desired_size - spacing * count.saturating_sub(1) as f64 - total_size;
    if extra <= 0.0 {
        return;
    }

    if let Some(basis) = percentage_basis {
        let percentage_rows = indices
            .clone()
            .filter_map(|index| {
                let percent = rows[index].percent?;
                let deficit = (basis * percent - rows[index].block_size).max(0.0);
                (deficit > 0.0).then_some((index, deficit))
            })
            .collect::<Vec<_>>();
        let total_deficit = percentage_rows
            .iter()
            .map(|(_, deficit)| deficit)
            .sum::<f64>();
        if total_deficit > 0.0 {
            let distributed = extra.min(total_deficit);
            distribute_weighted(rows, &percentage_rows, distributed);
            extra -= distributed;
            if extra <= 0.0 {
                return;
            }
        }
    }

    let originating_rowspans = indices
        .clone()
        .filter(|&index| rowspan_distribution && index != start && rows[index].has_rowspan_start)
        .collect::<Vec<_>>();
    if !originating_rowspans.is_empty() {
        distribute_equal(rows, &originating_rowspans, extra);
        return;
    }

    let is_constrained =
        |row: &RowMeasure| row.constrained && (row.percent.is_none() || percentage_basis.is_some());
    let nonempty_auto = indices
        .clone()
        .filter(|&index| rows[index].block_size > 0.0 && !is_constrained(&rows[index]))
        .collect::<Vec<_>>();
    if !nonempty_auto.is_empty() {
        let weighted = nonempty_auto
            .iter()
            .map(|&index| (index, rows[index].block_size))
            .collect::<Vec<_>>();
        distribute_weighted(rows, &weighted, extra);
        return;
    }

    let empty = indices
        .clone()
        .filter(|&index| rows[index].block_size == 0.0)
        .collect::<Vec<_>>();
    let nonempty = indices
        .clone()
        .filter(|&index| rows[index].block_size > 0.0)
        .collect::<Vec<_>>();
    if rowspan_distribution && empty.len() == count {
        rows[*empty.last().expect("a nonempty span has a final row")].block_size += extra;
        return;
    }
    if !rowspan_distribution {
        let constrained_nonempty = nonempty
            .iter()
            .filter(|&&index| is_constrained(&rows[index]))
            .count();
        if empty.len() == count || empty.len() + constrained_nonempty == count {
            let unconstrained_empty = empty
                .iter()
                .copied()
                .filter(|&index| !is_constrained(&rows[index]))
                .collect::<Vec<_>>();
            let recipients = if unconstrained_empty.is_empty() {
                &empty
            } else {
                &unconstrained_empty
            };
            distribute_equal(rows, recipients, extra);
            return;
        }
    }

    if !nonempty.is_empty() {
        let weighted = nonempty
            .iter()
            .map(|&index| (index, rows[index].block_size))
            .collect::<Vec<_>>();
        distribute_weighted(rows, &weighted, extra);
    }
}

fn distribute_equal(rows: &mut [RowMeasure], recipients: &[usize], extra: f64) {
    if recipients.is_empty() || extra <= 0.0 {
        return;
    }
    let share = extra / recipients.len() as f64;
    for &index in recipients {
        rows[index].block_size += share;
    }
}

fn distribute_weighted(rows: &mut [RowMeasure], recipients: &[(usize, f64)], extra: f64) {
    if recipients.is_empty() || extra <= 0.0 {
        return;
    }
    let total_weight = recipients
        .iter()
        .map(|(_, weight)| weight.max(0.0))
        .sum::<f64>();
    if total_weight <= 0.0 {
        distribute_equal(
            rows,
            &recipients
                .iter()
                .map(|(index, _)| *index)
                .collect::<Vec<_>>(),
            extra,
        );
        return;
    }
    for &(index, weight) in recipients {
        rows[index].block_size += extra * weight.max(0.0) / total_weight;
    }
}

fn percentage_component(size: PreferredSize) -> Option<f64> {
    match size {
        PreferredSize::Percent(percent) => Some(percent.max(0.0) as f64),
        PreferredSize::Calc {
            absolute_px,
            percentage,
            ..
        } if absolute_px == 0.0 => Some(percentage.max(0.0) as f64),
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
