use super::borders::CollapsedBorderGrid;
use super::columns::{TableCellPlacement, TableColumnLayout};
use crate::layout::{
    LayoutEngine, ResolvedBoxModel, UsedBorderInsets, resolve_definite_outer_inline_size,
};
use crate::layout_model::TableColumnWidthHint;
use html_style_model::{BoxSizing, UsedPreferredSize as PreferredSize};

struct ColumnMeasures {
    minimums: Vec<f64>,
    maximums: Vec<f64>,
    percentages: Vec<f64>,
    percentage_offsets: Vec<f64>,
    constrained: Vec<bool>,
}

#[derive(Clone, Copy)]
struct CellMeasures {
    minimum: f64,
    maximum: f64,
    percentage: f64,
    percentage_offset: f64,
    constrained: bool,
}

pub(super) fn resolve_table_columns(
    session: &LayoutEngine<'_, '_>,
    placements: &[TableCellPlacement],
    column_count: usize,
    hints: &[TableColumnWidthHint],
    content_width: f64,
    spacing: f64,
    collapsed_grid: Option<&CollapsedBorderGrid>,
    fixed_mode: bool,
) -> TableColumnLayout {
    let usable_width = (content_width - spacing * (column_count as f64 + 1.0)).max(0.0);
    let measures = column_measures(
        session,
        placements,
        column_count,
        hints,
        spacing,
        collapsed_grid,
        fixed_mode,
    );
    let widths = if fixed_mode {
        resolve_fixed_widths(&measures, usable_width.max(sum(&measures.minimums)))
    } else {
        resolve_measured_widths(&measures, usable_width, true)
    };
    let table_width = widths.iter().sum::<f64>() + spacing * (column_count as f64 + 1.0);
    let mut starts = Vec::with_capacity(widths.len());
    let mut cursor = spacing;
    for &width in &widths {
        starts.push(cursor);
        cursor += width + spacing;
    }
    TableColumnLayout {
        widths,
        starts,
        table_width,
    }
}

pub(super) fn intrinsic_column_widths(
    session: &LayoutEngine<'_, '_>,
    placements: &[TableCellPlacement],
    column_count: usize,
    hints: &[TableColumnWidthHint],
    spacing: f64,
    collapsed_grid: Option<&CollapsedBorderGrid>,
    fixed_mode: bool,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let measures = column_measures(
        session,
        placements,
        column_count,
        hints,
        spacing,
        collapsed_grid,
        fixed_mode,
    );
    (measures.minimums, measures.maximums, measures.percentages)
}

fn column_measures(
    session: &LayoutEngine<'_, '_>,
    placements: &[TableCellPlacement],
    column_count: usize,
    hints: &[TableColumnWidthHint],
    spacing: f64,
    collapsed_grid: Option<&CollapsedBorderGrid>,
    fixed_mode: bool,
) -> ColumnMeasures {
    let mut measures = ColumnMeasures {
        minimums: vec![0.0; column_count],
        maximums: vec![0.0; column_count],
        percentages: vec![0.0; column_count],
        percentage_offsets: vec![0.0; column_count],
        constrained: vec![false; column_count],
    };

    for (column, hint) in hints.iter().copied().enumerate().take(column_count) {
        let primary = session.reader.used_style(hint.style);
        let style = if matches!(primary.width(), PreferredSize::Auto) {
            hint.fallback_style
                .map(|style| session.reader.used_style(style))
                .unwrap_or(primary)
        } else {
            primary
        };
        apply_track_style(style, column, &mut measures, fixed_mode);
    }

    for &placement in placements
        .iter()
        .filter(|placement| placement.source_colspan == 1 && (!fixed_mode || placement.row == 0))
    {
        if placement.col >= column_count {
            continue;
        }
        let used_borders = collapsed_grid.map(|grid| grid.cell_insets(placement));
        let cell = cell_measures(session, placement.cell_idx, used_borders, fixed_mode);
        let column = placement.col;
        let track_is_constrained = measures.constrained[column];
        if !fixed_mode || !track_is_constrained {
            measures.minimums[column] = measures.minimums[column].max(cell.minimum);
        }
        if !track_is_constrained || (!fixed_mode && cell.constrained) {
            measures.maximums[column] = measures.maximums[column].max(cell.maximum);
        }
        measures.maximums[column] = measures.maximums[column].max(measures.minimums[column]);
        measures.percentages[column] = measures.percentages[column].max(cell.percentage);
        measures.percentage_offsets[column] =
            measures.percentage_offsets[column].max(cell.percentage_offset);
        measures.constrained[column] |= cell.constrained;
    }

    let mut spanning = placements
        .iter()
        .copied()
        .filter(|placement| placement.source_colspan > 1 && (!fixed_mode || placement.row == 0))
        .collect::<Vec<_>>();
    spanning.sort_by_key(|placement| placement.source_colspan);
    for placement in spanning {
        let Some(range) = placement.column_range(column_count) else {
            continue;
        };
        let internal_spacing = spacing * range.len().saturating_sub(1) as f64;
        let used_borders = collapsed_grid.map(|grid| grid.cell_insets(placement));
        let cell = cell_measures(session, placement.cell_idx, used_borders, fixed_mode);
        let required_minimum = (cell.minimum - internal_spacing).max(0.0);
        let required_maximum = (cell.maximum - internal_spacing).max(required_minimum);
        if fixed_mode {
            distribute_fixed_span(
                &mut measures,
                range.start,
                range.end,
                cell,
                required_minimum,
                required_maximum,
            );
        } else {
            distribute_auto_span(
                &mut measures,
                range.start,
                range.end,
                cell,
                required_minimum,
                required_maximum,
            );
        }
    }

    if !fixed_mode {
        let mut remaining_percentage = 1.0;
        for percentage in &mut measures.percentages {
            *percentage = percentage.clamp(0.0, remaining_percentage);
            remaining_percentage = (remaining_percentage - *percentage).max(0.0);
        }
    }
    for column in 0..column_count {
        measures.maximums[column] = measures.maximums[column].max(measures.minimums[column]);
    }

    if let Some(grid) = collapsed_grid {
        for column in 0..column_count {
            let left = (column > 0)
                .then(|| grid.vertical_boundary_max(column) * 0.5)
                .unwrap_or(0.0);
            let right = (column + 1 < column_count)
                .then(|| grid.vertical_boundary_max(column + 1) * 0.5)
                .unwrap_or(0.0);
            let border_floor = left + right;
            measures.minimums[column] = measures.minimums[column].max(border_floor);
            measures.maximums[column] = measures.maximums[column].max(measures.minimums[column]);
        }
    }
    measures
}

fn apply_track_style(
    style: html_style_model::UsedStyleView<'_>,
    column: usize,
    measures: &mut ColumnMeasures,
    fixed_mode: bool,
) {
    let minimum =
        resolve_definite_outer_inline_size(style.min_width(), style.box_sizing(), 0.0, 0.0)
            .unwrap_or(0.0);
    let maximum =
        resolve_definite_outer_inline_size(style.max_width(), style.box_sizing(), 0.0, 0.0)
            .unwrap_or(f64::INFINITY)
            .max(minimum);
    if let Some(width) =
        resolve_definite_outer_inline_size(style.width(), style.box_sizing(), 0.0, 0.0)
    {
        let width = width.clamp(minimum, maximum);
        if fixed_mode {
            measures.minimums[column] = measures.minimums[column].max(width);
        }
        measures.maximums[column] = measures.maximums[column].max(width);
        measures.constrained[column] = true;
    } else {
        measures.minimums[column] = measures.minimums[column].max(minimum);
    }
    if let Some(percentage) = percentage_component(style.width()) {
        measures.percentages[column] = measures.percentages[column].max(percentage);
    }
}

fn cell_measures(
    session: &LayoutEngine<'_, '_>,
    cell_idx: usize,
    used_borders: Option<UsedBorderInsets>,
    fixed_mode: bool,
) -> CellMeasures {
    let style = session.reader.style(cell_idx);
    // Percentage cell padding is cyclic while column measures are computed,
    // so its intrinsic contribution is zero. It is resolved against the
    // final table width later when the cell contents are laid out.
    let box_model = used_borders.map_or_else(
        || ResolvedBoxModel::new(style, 0.0),
        |borders| ResolvedBoxModel::new(style, 0.0).with_used_borders(borders),
    );
    let extras = box_model.horizontal_padding_border();
    let (content_minimum, content_maximum) =
        crate::layout::box_content_intrinsic_widths(session, cell_idx);
    let lower =
        resolve_definite_outer_inline_size(style.min_width(), style.box_sizing(), extras, 0.0)
            .unwrap_or(0.0);
    let upper =
        resolve_definite_outer_inline_size(style.max_width(), style.box_sizing(), extras, 0.0)
            .unwrap_or(f64::INFINITY)
            .max(lower);
    let definite_width =
        resolve_definite_outer_inline_size(style.width(), style.box_sizing(), extras, 0.0)
            .map(|width| width.clamp(lower, upper));
    let percentage = percentage_component(style.width()).unwrap_or(0.0).max(0.0);
    let percentage_offset =
        if fixed_mode && percentage > 0.0 && matches!(style.box_sizing(), BoxSizing::ContentBox) {
            extras
        } else {
            0.0
        };

    let (minimum, maximum) = if fixed_mode {
        match definite_width {
            Some(width) => (width, width),
            None => (0.0, (content_maximum + extras).max(lower).min(upper)),
        }
    } else {
        let minimum = (content_minimum + extras).max(lower).min(upper);
        let natural_maximum = (content_maximum + extras).max(minimum).min(upper);
        let maximum = definite_width.map_or(natural_maximum, |width| width.max(minimum));
        (minimum, maximum)
    };

    CellMeasures {
        minimum,
        maximum: maximum.max(minimum),
        percentage,
        percentage_offset,
        constrained: definite_width.is_some(),
    }
}

fn percentage_component(value: PreferredSize) -> Option<f64> {
    match value {
        PreferredSize::Percent(percentage) => Some(percentage as f64),
        PreferredSize::Calc {
            absolute_px,
            percentage,
            ..
        } if absolute_px == 0.0 => Some(percentage as f64),
        _ => None,
    }
}

fn distribute_fixed_span(
    measures: &mut ColumnMeasures,
    start: usize,
    end: usize,
    cell: CellMeasures,
    required_minimum: f64,
    required_maximum: f64,
) {
    let count = end - start;
    let minimum = if cell.constrained {
        required_minimum / count as f64
    } else {
        0.0
    };
    let maximum = required_maximum / count as f64;
    let percentage = (cell.percentage > 0.0).then_some(cell.percentage / count as f64);
    for column in start..end {
        if measures.minimums[column] == 0.0 {
            measures.minimums[column] = minimum;
            measures.constrained[column] |= cell.constrained;
        }
        if measures.maximums[column] == 0.0 {
            measures.maximums[column] = maximum;
            measures.constrained[column] |= cell.constrained;
        }
        if measures.percentages[column] == 0.0 && !measures.constrained[column] {
            if let Some(percentage) = percentage {
                measures.percentages[column] = percentage;
                // A fixed-layout colspan distributes its percentage evenly,
                // but its own padding/border is not copied into every track.
                measures.percentage_offsets[column] = 0.0;
            }
        }
    }
}

fn distribute_spanning_percentage(
    measures: &mut ColumnMeasures,
    start: usize,
    end: usize,
    percentage: f64,
    offset: f64,
) {
    let residual = (percentage - measures.percentages[start..end].iter().sum::<f64>()).max(0.0);
    if residual > 0.0 {
        let recipients = (start..end)
            .filter(|&column| measures.percentages[column] == 0.0)
            .collect::<Vec<_>>();
        let weight_sum = recipients
            .iter()
            .map(|&column| measures.maximums[column])
            .sum::<f64>();
        for &column in &recipients {
            let share = if weight_sum > 0.0 {
                measures.maximums[column] / weight_sum
            } else {
                1.0 / recipients.len() as f64
            };
            measures.percentages[column] = residual * share;
        }
    }
    if offset > 0.0 {
        let add = offset / (end - start) as f64;
        for value in &mut measures.percentage_offsets[start..end] {
            *value = value.max(add);
        }
    }
}

fn distribute_auto_span(
    measures: &mut ColumnMeasures,
    start: usize,
    end: usize,
    cell: CellMeasures,
    required_minimum: f64,
    required_maximum: f64,
) {
    distribute_spanning_percentage(
        measures,
        start,
        end,
        cell.percentage,
        cell.percentage_offset,
    );

    let mut span = ColumnMeasures {
        minimums: measures.minimums[start..end].to_vec(),
        maximums: measures.maximums[start..end].to_vec(),
        percentages: measures.percentages[start..end].to_vec(),
        percentage_offsets: measures.percentage_offsets[start..end].to_vec(),
        constrained: measures.constrained[start..end].to_vec(),
    };
    let minimums = resolve_measured_widths(&span, required_minimum, true);
    for (offset, minimum) in minimums.into_iter().enumerate() {
        let column = start + offset;
        measures.minimums[column] = measures.minimums[column].max(minimum);
        measures.maximums[column] = measures.maximums[column].max(measures.minimums[column]);
        span.minimums[offset] = measures.minimums[column];
        span.maximums[offset] = measures.maximums[column];
    }

    let maximums = resolve_measured_widths(&span, required_maximum, cell.constrained);
    for (offset, maximum) in maximums.into_iter().enumerate() {
        let column = start + offset;
        measures.maximums[column] = measures.maximums[column]
            .max(maximum)
            .max(measures.minimums[column]);
    }
}

fn resolve_measured_widths(
    measures: &ColumnMeasures,
    target: f64,
    treat_target_as_constrained: bool,
) -> Vec<f64> {
    if measures.minimums.is_empty() {
        return Vec::new();
    }
    let guess_minimum = measures.minimums.clone();
    let guess_percentage = (0..measures.minimums.len())
        .map(|column| {
            if measures.percentages[column] > 0.0 {
                measures.minimums[column].max(
                    target * measures.percentages[column] + measures.percentage_offsets[column],
                )
            } else {
                measures.minimums[column]
            }
        })
        .collect::<Vec<_>>();
    let guess_specified = (0..measures.minimums.len())
        .map(|column| {
            if measures.percentages[column] > 0.0 {
                guess_percentage[column]
            } else if measures.constrained[column] {
                measures.maximums[column]
            } else {
                measures.minimums[column]
            }
        })
        .collect::<Vec<_>>();
    let guess_maximum = (0..measures.minimums.len())
        .map(|column| {
            if measures.percentages[column] > 0.0 {
                guess_percentage[column]
            } else {
                measures.maximums[column].max(guess_specified[column])
            }
        })
        .collect::<Vec<_>>();

    if target <= sum(&guess_minimum) {
        return guess_minimum;
    }
    for (lower, upper) in [
        (&guess_minimum, &guess_percentage),
        (&guess_percentage, &guess_specified),
        (&guess_specified, &guess_maximum),
    ] {
        let lower_sum = sum(lower);
        let upper_sum = sum(upper);
        if target <= upper_sum && upper_sum > lower_sum {
            let progress = ((target - lower_sum) / (upper_sum - lower_sum)).clamp(0.0, 1.0);
            return lower
                .iter()
                .zip(upper)
                .map(|(lower, upper)| lower + progress * (upper - lower))
                .collect();
        }
    }

    let mut widths = guess_maximum;
    let excess = target - sum(&widths);
    distribute_excess_width(&mut widths, measures, excess, treat_target_as_constrained);
    widths
}

fn resolve_fixed_widths(measures: &ColumnMeasures, target: f64) -> Vec<f64> {
    let count = measures.minimums.len();
    if count == 0 {
        return Vec::new();
    }
    let is_percentage = |column: usize| measures.percentages[column] > 0.0;
    let is_zero = |column: usize| {
        !is_percentage(column) && measures.constrained[column] && measures.maximums[column] == 0.0
    };
    let is_fixed = |column: usize| {
        !is_percentage(column) && measures.constrained[column] && measures.maximums[column] > 0.0
    };
    let is_auto = |column: usize| !is_percentage(column) && !is_fixed(column) && !is_zero(column);

    let percent_columns = (0..count)
        .filter(|&column| is_percentage(column))
        .collect::<Vec<_>>();
    let fixed_columns = (0..count)
        .filter(|&column| is_fixed(column))
        .collect::<Vec<_>>();
    let zero_columns = (0..count)
        .filter(|&column| is_zero(column))
        .collect::<Vec<_>>();
    let auto_columns = (0..count)
        .filter(|&column| is_auto(column))
        .collect::<Vec<_>>();
    let percent_sizes = (0..count)
        .map(|column| target * measures.percentages[column] + measures.percentage_offsets[column])
        .collect::<Vec<_>>();
    let total_percent = percent_columns
        .iter()
        .map(|&column| percent_sizes[column])
        .sum::<f64>();
    let total_fixed = fixed_columns
        .iter()
        .map(|&column| measures.maximums[column])
        .sum::<f64>();
    let mut widths = vec![0.0; count];
    let mut assigned = 0.0;

    if !fixed_columns.is_empty() {
        let target_fixed = (target - total_percent).max(0.0);
        let scale =
            if (total_fixed < target_fixed && auto_columns.is_empty()) || total_fixed > target {
                if total_fixed > 0.0 {
                    target_fixed / total_fixed
                } else {
                    1.0
                }
            } else {
                1.0
            };
        for &column in &fixed_columns {
            widths[column] = measures.maximums[column] * scale;
            assigned += widths[column];
        }
    }
    if assigned >= target {
        return widths;
    }

    if !percent_columns.is_empty() {
        let available = target - assigned;
        let scale_up = total_percent < available && auto_columns.is_empty();
        let scale_down = total_percent > available;
        let scale = if (scale_up || scale_down) && total_percent > 0.0 {
            available / total_percent
        } else {
            1.0
        };
        for &column in &percent_columns {
            widths[column] = percent_sizes[column] * scale;
            assigned += widths[column];
        }
    }

    let remaining = (target - assigned).max(0.0);
    let recipients = if zero_columns.len() == count {
        &zero_columns
    } else {
        &auto_columns
    };
    if !recipients.is_empty() {
        let share = remaining / recipients.len() as f64;
        for &column in recipients {
            widths[column] = share;
        }
    } else if assigned < target {
        // Fixed or percentage columns consume any remainder when there are no
        // ordinary auto columns. Preserve their authored proportions.
        let recipients = if !fixed_columns.is_empty() {
            &fixed_columns
        } else {
            &percent_columns
        };
        let weight_sum = recipients.iter().map(|&column| widths[column]).sum::<f64>();
        for &column in recipients {
            widths[column] += (target - assigned)
                * if weight_sum > 0.0 {
                    widths[column] / weight_sum
                } else {
                    1.0 / recipients.len() as f64
                };
        }
    }
    widths
}

fn distribute_excess_width(
    widths: &mut [f64],
    measures: &ColumnMeasures,
    extra: f64,
    treat_target_as_constrained: bool,
) {
    if extra <= 0.0 || widths.is_empty() {
        return;
    }
    let nonempty_auto = (0..widths.len())
        .filter(|&column| {
            measures.percentages[column] == 0.0
                && !measures.constrained[column]
                && measures.maximums[column] > 0.0
        })
        .collect::<Vec<_>>();
    let auto = (0..widths.len())
        .filter(|&column| measures.percentages[column] == 0.0 && !measures.constrained[column])
        .collect::<Vec<_>>();
    let pixel = (0..widths.len())
        .filter(|&column| measures.percentages[column] == 0.0 && measures.constrained[column])
        .collect::<Vec<_>>();
    let percent = (0..widths.len())
        .filter(|&column| measures.percentages[column] > 0.0)
        .collect::<Vec<_>>();
    let recipients = if !nonempty_auto.is_empty() {
        nonempty_auto
    } else if !auto.is_empty() {
        auto
    } else if treat_target_as_constrained && !pixel.is_empty() {
        pixel
    } else {
        percent
    };
    if recipients.is_empty() {
        return;
    }
    let weight_sum = recipients
        .iter()
        .map(|&column| {
            if measures.percentages[column] > 0.0 {
                measures.percentages[column]
            } else {
                measures.maximums[column]
            }
        })
        .sum::<f64>();
    for column in recipients.iter().copied() {
        let weight = if measures.percentages[column] > 0.0 {
            measures.percentages[column]
        } else {
            measures.maximums[column]
        };
        let share = if weight_sum > 0.0 {
            weight / weight_sum
        } else {
            1.0 / recipients.len() as f64
        };
        widths[column] += extra * share;
    }
}

fn sum(values: &[f64]) -> f64 {
    values.iter().sum()
}
