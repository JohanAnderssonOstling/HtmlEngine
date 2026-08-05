use std::ops::Range;

#[derive(Clone, Copy)]
pub(super) struct TableCellPlacement {
    pub(super) cell_idx: usize,
    pub(super) row: usize,
    pub(super) col: usize,
    pub(super) colspan: usize,
    pub(super) rowspan: usize,
}

impl TableCellPlacement {
    pub(super) fn column_range(self, column_count: usize) -> Option<Range<usize>> {
        let start = self.col.min(column_count);
        let end = self.col.saturating_add(self.colspan).min(column_count);
        (start < end).then_some(start..end)
    }
}

pub(super) struct TableGrid {
    pub(super) placements: Vec<TableCellPlacement>,
    pub(super) column_count: usize,
}

pub(super) struct TableColumnLayout {
    pub(super) widths: Vec<f64>,
    pub(super) starts: Vec<f64>,
    pub(super) table_width: f64,
}

pub(super) fn build_table_grid(rows: impl IntoIterator<Item = Vec<(usize, usize, usize)>>) -> TableGrid {
    let mut placements = Vec::new();
    let mut carry_rowspans: Vec<usize> = Vec::new();
    let mut max_columns = 0usize;

    for (row_idx, cells) in rows.into_iter().enumerate() {
        if row_idx > 0 {
            for span in &mut carry_rowspans {
                if *span > 0 {
                    *span -= 1;
                }
            }
            while matches!(carry_rowspans.last(), Some(0)) {
                carry_rowspans.pop();
            }
        }

        let mut col = 0usize;
        for (cell_idx, colspan, rowspan) in cells {
            while col < carry_rowspans.len() && carry_rowspans[col] > 0 {
                col += 1;
            }

            let colspan = colspan.max(1);
            let rowspan = rowspan.max(1);
            let end_col = col + colspan;
            if carry_rowspans.len() < end_col {
                carry_rowspans.resize(end_col, 0);
            }
            for covered_col in col..end_col {
                carry_rowspans[covered_col] = carry_rowspans[covered_col].max(rowspan);
            }
            placements.push(TableCellPlacement { cell_idx, row: row_idx, col, colspan, rowspan });
            col = end_col;
            max_columns = max_columns.max(end_col);
        }
    }

    TableGrid { placements, column_count: max_columns }
}

pub(super) fn resolve_column_widths(min_widths: &[f64], max_widths: &[f64], preferred_widths: Option<&[f64]>, preferred_priorities: Option<&[u8]>, target_width: f64) -> Vec<f64> {
    if min_widths.is_empty() {
        return Vec::new();
    }

    let min_widths: Vec<f64> = min_widths.iter().map(|w| w.max(0.0)).collect();
    let max_widths: Vec<f64> = max_widths.iter().enumerate().map(|(i, w)| w.max(min_widths[i])).collect();
    let target = target_width.max(0.0);
    let min_total = min_widths.iter().sum::<f64>();
    if target <= min_total {
        return min_widths;
    }

    let preferred_widths = if let Some(preferred) = preferred_widths {
        min_widths.iter().zip(&max_widths).enumerate().map(|(i, (min_w, max_w))| preferred.get(i).copied().unwrap_or(*min_w).clamp(*min_w, *max_w)).collect::<Vec<_>>()
    } else {
        min_widths.iter().zip(&max_widths).map(|(min_w, max_w)| min_w + 0.35 * (max_w - min_w)).collect()
    };
    let preferred_total = preferred_widths.iter().sum::<f64>();
    let max_total = max_widths.iter().sum::<f64>();
    let preferred_priorities = preferred_priorities.map(<[u8]>::to_vec).unwrap_or_else(|| vec![0; min_widths.len()]);

    if target <= preferred_total {
        let mut widths = min_widths.clone();
        let mut remaining = target - min_total;
        for priority in [2u8, 1, 0] {
            if remaining <= 0.0 {
                break;
            }
            remaining = distribute_toward_targets(&mut widths, &preferred_widths, remaining, |index| preferred_priorities[index] == priority, DistributionWeight::RemainingCapacity);
        }
        if remaining > 0.0 {
            let add = remaining / widths.len() as f64;
            for width in &mut widths {
                *width += add;
            }
        }
        return widths;
    }

    if target <= max_total {
        let mut widths = preferred_widths.clone();
        distribute_toward_targets(&mut widths, &max_widths, target - preferred_total, |_| true, DistributionWeight::Fixed(&preferred_widths));
        return widths;
    }

    let mut widths = max_widths;
    let auto_columns = (0..widths.len()).filter(|&index| preferred_priorities[index] == 0).collect::<Vec<_>>();
    let recipients = if auto_columns.is_empty() { (0..widths.len()).collect::<Vec<_>>() } else { auto_columns };
    let extra = (target - max_total) / recipients.len() as f64;
    for index in recipients {
        widths[index] += extra;
    }
    widths
}

pub(super) fn distribute_deficit(widths: &mut [f64], start: usize, end: usize, deficit: f64) {
    if deficit <= 0.0 || start >= end {
        return;
    }
    let weight_sum = widths[start..end].iter().map(|width| width.max(1.0)).sum::<f64>();
    if weight_sum <= 0.0 {
        let add = deficit / (end - start) as f64;
        for width in &mut widths[start..end] {
            *width += add;
        }
        return;
    }
    for width in &mut widths[start..end] {
        *width += deficit * (width.max(1.0) / weight_sum);
    }
}

pub(super) fn distribute_surplus(widths: &mut [f64], mins: &[f64], start: usize, end: usize, surplus: f64) {
    if surplus <= 0.0 || start >= end {
        return;
    }
    let shrink_capacity = (start..end).map(|index| (widths[index] - mins[index]).max(0.0)).sum::<f64>();
    if shrink_capacity <= 0.0 {
        return;
    }
    let shrink = surplus.min(shrink_capacity);
    for index in start..end {
        let capacity = (widths[index] - mins[index]).max(0.0);
        if capacity > 0.0 {
            widths[index] -= shrink * (capacity / shrink_capacity);
        }
    }
}

enum DistributionWeight<'a> {
    Fixed(&'a [f64]),
    RemainingCapacity,
}

fn distribute_toward_targets(widths: &mut [f64], targets: &[f64], extra: f64, eligible: impl Fn(usize) -> bool, weight: DistributionWeight<'_>) -> f64 {
    if extra <= 0.0 || widths.is_empty() {
        return extra.max(0.0);
    }

    let mut remaining = extra;
    let epsilon = 0.01;
    while remaining > epsilon {
        let active_weight = (0..widths.len())
            .filter(|&index| eligible(index) && targets[index] > widths[index] + epsilon)
            .map(|index| match weight {
                DistributionWeight::Fixed(weights) => weights[index].max(1.0),
                DistributionWeight::RemainingCapacity => (targets[index] - widths[index]).max(1.0),
            })
            .sum::<f64>();
        if active_weight <= 0.0 {
            break;
        }

        let mut progressed = false;
        for index in 0..widths.len() {
            if !eligible(index) {
                continue;
            }
            let capacity = (targets[index] - widths[index]).max(0.0);
            if capacity <= epsilon {
                continue;
            }
            let item_weight = match weight {
                DistributionWeight::Fixed(weights) => weights[index].max(1.0),
                DistributionWeight::RemainingCapacity => capacity.max(1.0),
            };
            let add = (remaining * item_weight / active_weight).min(capacity);
            if add > epsilon {
                widths[index] += add;
                remaining -= add;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    remaining
}

pub(super) fn span_extent(starts: &[f64], sizes: &[f64], start: usize, span: usize) -> f64 {
    let start = start.min(sizes.len());
    let end = start.saturating_add(span).min(sizes.len());
    if start >= end {
        return 0.0;
    }
    let last = end - 1;
    (starts[last] + sizes[last] - starts[start]).max(0.0)
}
