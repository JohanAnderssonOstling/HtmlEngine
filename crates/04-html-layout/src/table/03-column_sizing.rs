//! Resolve column geometry and collect the width constraints used by the solvers.

use super::weighted_share;

use super::columns::{
    column_width_constraints, column_width_style, definite_width_constraints, percentage_component,
};
use super::geometry::TableColumnLayout;
use super::s01_grid::PreparedTableGrid;
use crate::{LayoutEngine, UsedBorderInsets};
use html_style_model::BoxSizing;

#[derive(Clone, Copy, Default)]
pub(super) struct WidthMeasure {
    pub(super) minimum: f64,
    pub(super) maximum: f64,
    pub(super) percentage: f64,
    percentage_offset: f64,
    constrained: bool,
}

pub(super) fn resolve_table_columns(
    measures: Vec<WidthMeasure>,
    content_width: f64,
    prepared: &PreparedTableGrid,
    fit_to_width: bool,
    rtl: bool,
) -> TableColumnLayout {
    let spacing = if fit_to_width {
        prepared
            .horizontal_spacing
            .min(content_width / (measures.len() as f64 + 1.0))
    } else {
        prepared.horizontal_spacing
    };
    let column_count = measures.len();
    let usable_width = (content_width - spacing * (column_count as f64 + 1.0)).max(0.0);
    let mut widths = if prepared.fixed_mode {
        resolve_fixed_widths(
            &measures,
            usable_width.max(measures.iter().map(|column| column.minimum).sum::<f64>()),
        )
    } else {
        resolve_measured_widths(&measures, usable_width, true)
    };
    // Resolve proportions normally, then keep the assigned tracks inside the
    // reading width. Cell layout consumes these widths and determines row height.
    let total = widths.iter().sum::<f64>();
    let scale = if fit_to_width && total > usable_width {
        usable_width / total
    } else {
        1.0
    };
    let mut starts = Vec::with_capacity(widths.len());
    let mut cursor = spacing
        + if rtl {
            prepared.outer_borders.right
        } else {
            prepared.outer_borders.left
        };
    for width in &mut widths {
        *width *= scale;
        starts.push(cursor);
        cursor += *width + spacing;
    }
    let table_width = cursor
        + if rtl {
            prepared.outer_borders.left
        } else {
            prepared.outer_borders.right
        };
    TableColumnLayout {
        rtl,
        spacing,
        widths,
        starts,
        table_width,
    }
}

pub(super) fn measure_table_columns(
    engine: &mut LayoutEngine<'_, '_>,
    prepared: &PreparedTableGrid,
) -> Vec<WidthMeasure> {
    let placements = &prepared.grid.placements;
    let column_count = prepared.grid.column_count;
    let spacing = prepared.horizontal_spacing;
    let collapsed_grid = prepared.collapsed_borders.as_ref();
    let fixed_mode = prepared.fixed_mode;
    let mut measures = vec![WidthMeasure::default(); column_count];

    for (column, track) in prepared.columns.iter().enumerate().take(column_count) {
        let style = column_width_style(&engine.read.reader, track);
        let (minimum, width) = column_width_constraints(&engine.read.reader, track);
        let column = &mut measures[column];
        column.minimum = column.minimum.max(minimum);
        if let Some(width) = width {
            if fixed_mode {
                column.minimum = column.minimum.max(width);
            }
            column.maximum = column.maximum.max(width);
            column.constrained = true;
        }
        if let Some(percentage) = percentage_component(style.width()) {
            column.percentage = column.percentage.max(percentage);
        }
    }

    for &placement in placements
        .iter()
        .filter(|placement| placement.source_colspan == 1 && (!fixed_mode || placement.row == 0))
    {
        if placement.col >= column_count {
            continue;
        }
        let used_borders = collapsed_grid.map(|grid| grid.cell_insets(placement));
        let cell = cell_measures(engine, placement.cell_idx, used_borders, fixed_mode);
        let column = &mut measures[placement.col];
        let track_is_constrained = column.constrained;
        if !fixed_mode || !track_is_constrained {
            column.minimum = column.minimum.max(cell.minimum);
        }
        if !track_is_constrained || (!fixed_mode && cell.constrained) {
            column.maximum = column.maximum.max(cell.maximum);
        }
        column.maximum = column.maximum.max(column.minimum);
        column.percentage = column.percentage.max(cell.percentage);
        column.percentage_offset = column.percentage_offset.max(cell.percentage_offset);
        column.constrained |= cell.constrained;
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
        let cell = cell_measures(engine, placement.cell_idx, used_borders, fixed_mode);
        let required_minimum = (cell.minimum - internal_spacing).max(0.0);
        let required_maximum = (cell.maximum - internal_spacing).max(required_minimum);
        let span = &mut measures[range];
        if fixed_mode {
            distribute_fixed_span(span, cell, required_minimum, required_maximum);
        } else {
            distribute_auto_span(span, cell, required_minimum, required_maximum);
        }
    }

    if !fixed_mode {
        let mut remaining_percentage = 1.0;
        for column in &mut measures {
            column.percentage = column.percentage.clamp(0.0, remaining_percentage);
            remaining_percentage = (remaining_percentage - column.percentage).max(0.0);
        }
    }
    for (index, column) in measures.iter_mut().enumerate() {
        if let Some(grid) = collapsed_grid {
            let left = if index > 0 {
                grid.vertical_boundary_max(index) * 0.5
            } else {
                0.0
            };
            let right = if index + 1 < column_count {
                grid.vertical_boundary_max(index + 1) * 0.5
            } else {
                0.0
            };
            column.minimum = column.minimum.max(left + right);
        }
        column.maximum = column.maximum.max(column.minimum);
    }
    measures
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FixedColumnKind {
    Percentage,
    Fixed,
    Zero,
    Auto,
}

impl WidthMeasure {
    fn fixed_kind(&self) -> FixedColumnKind {
        match (self.percentage > 0.0, self.constrained, self.maximum) {
            (true, _, _) => FixedColumnKind::Percentage,
            (_, true, width) if width > 0.0 => FixedColumnKind::Fixed,
            (_, true, 0.0) => FixedColumnKind::Zero,
            _ => FixedColumnKind::Auto,
        }
    }

    /// Minimum, percentage, specified, and maximum guesses for auto layout.
    fn guesses(&self, target: f64) -> [f64; 4] {
        let percentage = if self.percentage > 0.0 {
            self.minimum
                .max(target * self.percentage + self.percentage_offset)
        } else {
            self.minimum
        };
        let specified = if self.percentage > 0.0 {
            percentage
        } else if self.constrained {
            self.maximum
        } else {
            self.minimum
        };
        let maximum = if self.percentage > 0.0 {
            percentage
        } else {
            self.maximum.max(specified)
        };
        [self.minimum, percentage, specified, maximum]
    }
}

fn cell_measures(
    engine: &mut LayoutEngine<'_, '_>,
    cell_idx: usize,
    used_borders: Option<UsedBorderInsets>,
    fixed_mode: bool,
) -> WidthMeasure {
    let style = engine.read.reader.style(cell_idx);
    // Percentage cell padding is cyclic while column measures are computed,
    // so its intrinsic contribution is zero. It is resolved against the
    // final table width later when the cell contents are laid out.
    let mut box_model = engine.read.reader.box_model(cell_idx, 0.0);
    if let Some(borders) = used_borders {
        box_model = box_model.with_used_borders(borders);
    }
    let extras = box_model.horizontal_padding_border();
    let (content_minimum, content_maximum) =
        crate::measurement::box_content_intrinsic_widths(engine, cell_idx, None);
    let (lower, upper, definite_width) = definite_width_constraints(style, extras);
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

    WidthMeasure {
        minimum,
        maximum,
        percentage,
        percentage_offset,
        constrained: definite_width.is_some(),
    }
}

fn distribute_fixed_span(
    measures: &mut [WidthMeasure],
    cell: WidthMeasure,
    required_minimum: f64,
    required_maximum: f64,
) {
    let count = measures.len();
    let minimum = if cell.constrained {
        required_minimum / count as f64
    } else {
        0.0
    };
    let maximum = required_maximum / count as f64;
    let percentage = (cell.percentage > 0.0).then_some(cell.percentage / count as f64);
    for column in measures {
        if column.minimum == 0.0 {
            column.minimum = minimum;
            column.constrained |= cell.constrained;
        }
        if column.maximum == 0.0 {
            column.maximum = maximum;
            column.constrained |= cell.constrained;
        }
        if column.percentage == 0.0
            && !column.constrained
            && let Some(percentage) = percentage
        {
            column.percentage = percentage;
            // A fixed-layout colspan distributes its percentage evenly,
            // but its own padding/border is not copied into every track.
            column.percentage_offset = 0.0;
        }
    }
}

fn distribute_spanning_percentage(measures: &mut [WidthMeasure], percentage: f64, offset: f64) {
    let residual =
        (percentage - measures.iter().map(|column| column.percentage).sum::<f64>()).max(0.0);
    if residual > 0.0 {
        let recipients = measures.iter().filter(|column| column.percentage == 0.0);
        let count = recipients.clone().count();
        let weight_sum = recipients.map(|column| column.maximum).sum::<f64>();
        for column in measures
            .iter_mut()
            .filter(|column| column.percentage == 0.0)
        {
            let share = weighted_share(column.maximum, weight_sum, count);
            column.percentage = residual * share;
        }
    }
    if offset > 0.0 {
        let add = offset / measures.len() as f64;
        for column in measures {
            column.percentage_offset = column.percentage_offset.max(add);
        }
    }
}

fn distribute_auto_span(
    span: &mut [WidthMeasure],
    cell: WidthMeasure,
    required_minimum: f64,
    required_maximum: f64,
) {
    distribute_spanning_percentage(span, cell.percentage, cell.percentage_offset);

    let minimums = resolve_measured_widths(span, required_minimum, true);
    for (column, minimum) in span.iter_mut().zip(minimums) {
        column.minimum = column.minimum.max(minimum);
        column.maximum = column.maximum.max(column.minimum);
    }

    let maximums = resolve_measured_widths(span, required_maximum, cell.constrained);
    for (column, maximum) in span.iter_mut().zip(maximums) {
        column.maximum = column.maximum.max(maximum);
    }
}

fn resolve_measured_widths(
    measures: &[WidthMeasure],
    target: f64,
    treat_target_as_constrained: bool,
) -> Vec<f64> {
    if measures.is_empty() {
        return Vec::new();
    }
    let mut totals = [0.0; 4];
    for column in measures {
        for (total, guess) in totals.iter_mut().zip(column.guesses(target)) {
            *total += guess;
        }
    }
    if target <= totals[0] {
        return measures.iter().map(|column| column.minimum).collect();
    }
    for lower in 0..3 {
        let upper = lower + 1;
        if target <= totals[upper] && totals[upper] > totals[lower] {
            let progress =
                ((target - totals[lower]) / (totals[upper] - totals[lower])).clamp(0.0, 1.0);
            return measures
                .iter()
                .map(|column| {
                    let guesses = column.guesses(target);
                    guesses[lower] + progress * (guesses[upper] - guesses[lower])
                })
                .collect();
        }
    }
    let mut widths = measures
        .iter()
        .map(|column| column.guesses(target)[3])
        .collect::<Vec<_>>();
    distribute_excess_width(
        &mut widths,
        measures,
        target - totals[3],
        treat_target_as_constrained,
    );
    widths
}

fn resolve_fixed_widths(measures: &[WidthMeasure], target: f64) -> Vec<f64> {
    use FixedColumnKind::{Auto, Fixed, Percentage, Zero};

    let count = measures.len();
    if count == 0 {
        return Vec::new();
    }
    let columns = |kind| (0..count).filter(move |&column| measures[column].fixed_kind() == kind);
    let mut counts = [0; 4];
    let mut total_percent = 0.0;
    let mut total_fixed = 0.0;
    let percentage_size =
        |column: usize| target * measures[column].percentage + measures[column].percentage_offset;
    for (index, column) in measures.iter().enumerate() {
        let kind = column.fixed_kind();
        counts[kind as usize] += 1;
        match kind {
            Percentage => total_percent += percentage_size(index),
            Fixed => total_fixed += column.maximum,
            _ => {}
        }
    }
    let mut widths = vec![0.0; count];
    let mut assigned = 0.0;

    let target_fixed = (target - total_percent).max(0.0);
    let scale = if ((total_fixed < target_fixed && counts[Auto as usize] == 0)
        || total_fixed > target)
        && total_fixed > 0.0
    {
        target_fixed / total_fixed
    } else {
        1.0
    };
    for column in columns(Fixed) {
        widths[column] = measures[column].maximum * scale;
        assigned += widths[column];
    }
    if assigned >= target {
        return widths;
    }

    let available = target - assigned;
    let scale_up = total_percent < available && counts[Auto as usize] == 0;
    let scale_down = total_percent > available;
    let scale = if (scale_up || scale_down) && total_percent > 0.0 {
        available / total_percent
    } else {
        1.0
    };
    for column in columns(Percentage) {
        widths[column] = percentage_size(column) * scale;
        assigned += widths[column];
    }

    let remaining = (target - assigned).max(0.0);
    let kind = if counts[Zero as usize] == count {
        Zero
    } else {
        Auto
    };
    let recipients = columns(kind);
    let recipient_count = counts[kind as usize];
    if recipient_count > 0 {
        let share = remaining / recipient_count as f64;
        for column in recipients {
            widths[column] = share;
        }
    } else if assigned < target {
        // With no auto columns, preserve the proportions of fixed columns
        // first, or percentage columns when there are no fixed ones.
        let kind = if total_fixed > 0.0 { Fixed } else { Percentage };
        let recipients = columns(kind);
        let recipient_count = counts[kind as usize];
        let weight_sum = recipients.clone().map(|column| widths[column]).sum::<f64>();
        for column in recipients {
            widths[column] +=
                (target - assigned) * weighted_share(widths[column], weight_sum, recipient_count);
        }
    }
    widths
}

fn distribute_excess_width(
    widths: &mut [f64],
    measures: &[WidthMeasure],
    extra: f64,
    treat_target_as_constrained: bool,
) {
    if extra <= 0.0 || widths.is_empty() {
        return;
    }
    let measures = &measures[..widths.len()];
    // Prefer nonempty auto columns, then empty auto columns, then constrained
    // columns when allowed, and finally percentage columns.
    let priority = |column: &WidthMeasure| {
        if column.percentage == 0.0 && !column.constrained {
            Some(if column.maximum > 0.0 { 0 } else { 1 })
        } else if column.percentage == 0.0 && treat_target_as_constrained {
            Some(2)
        } else if column.percentage > 0.0 {
            Some(3)
        } else {
            None
        }
    };
    let Some(group) = measures.iter().filter_map(priority).min() else {
        return;
    };
    let recipients = (0..widths.len()).filter(|&column| priority(&measures[column]) == Some(group));
    let count = recipients.clone().count();
    let weight = |column: usize| {
        if measures[column].percentage > 0.0 {
            measures[column].percentage
        } else {
            measures[column].maximum
        }
    };
    let weight_sum = recipients.clone().map(weight).sum::<f64>();
    for column in recipients {
        let share = weighted_share(weight(column), weight_sum, count);
        widths[column] += extra * share;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation_test_support::count_allocations;

    #[test]
    fn width_solvers_allocate_only_the_result_buffer() {
        let measures = [
            WidthMeasure {
                minimum: 10.0,
                maximum: 100.0,
                constrained: true,
                ..WidthMeasure::default()
            },
            WidthMeasure {
                minimum: 5.0,
                maximum: 40.0,
                percentage: 0.25,
                ..WidthMeasure::default()
            },
            WidthMeasure {
                constrained: true,
                ..WidthMeasure::default()
            },
            WidthMeasure {
                minimum: 20.0,
                maximum: 60.0,
                ..WidthMeasure::default()
            },
            WidthMeasure::default(),
        ];
        for target in [35.0, 38.0, 65.0, 180.0, 400.0] {
            let (auto, allocations) =
                count_allocations(|| resolve_measured_widths(&measures, target, true));
            assert_eq!(auto.len(), measures.len());
            assert_eq!(
                allocations, 1,
                "auto sizing must allocate only its result at {target}px"
            );
            let (fixed, allocations) =
                count_allocations(|| resolve_fixed_widths(&measures, target));
            assert_eq!(fixed.len(), measures.len());
            assert_eq!(
                allocations, 1,
                "fixed sizing must allocate only its result at {target}px"
            );
        }
    }
}
