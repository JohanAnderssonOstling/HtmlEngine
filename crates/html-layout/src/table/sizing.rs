use super::borders::CollapsedBorderGrid;
use super::columns::{TableCellPlacement, TableColumnLayout, distribute_deficit, distribute_surplus, resolve_column_widths};
use crate::layout::{LayoutEngine, ResolvedBoxModel, UsedBorderInsets};
use crate::layout_model::TableColumnWidthHint;
use html_style_model::{BoxSizing, UsedPreferredSize as PreferredSize, resolve_used_preferred_size};

pub(super) fn resolve_table_columns(
    session: &LayoutEngine<'_, '_>, placements: &[TableCellPlacement], column_count: usize, hints: &[TableColumnWidthHint], content_width: f64, spacing: f64, collapsed_grid: Option<&CollapsedBorderGrid>,
) -> TableColumnLayout {
    let usable_width = (content_width - spacing * (column_count as f64 + 1.0)).max(0.0);
    let (mut minimums, mut maximums) = intrinsic_column_widths(session, placements, column_count, hints, spacing, collapsed_grid);

    for &placement in placements {
        let Some(range) = placement.column_range(column_count) else { continue };
        let style = session.reader.style(placement.cell_idx);
        let borders = collapsed_grid.map(|grid| grid.cell_insets(placement));
        if let Some(hint) = resolve_width_hint(session, placement.cell_idx, style.min_width(), content_width, borders) {
            let current = minimums[range.clone()].iter().sum::<f64>();
            if current < hint {
                distribute_deficit(&mut minimums, range.start, range.end, hint - current);
            }
        }
        if let Some(hint) = resolve_width_hint(session, placement.cell_idx, style.max_width(), content_width, borders) {
            let current = maximums[range.clone()].iter().sum::<f64>();
            if current > hint {
                distribute_surplus(&mut maximums, &minimums, range.start, range.end, current - hint);
            }
        }
    }

    let mut preferred = minimums.iter().zip(&maximums).map(|(min, max)| min + 0.35 * (max - min)).collect::<Vec<_>>();
    let mut priorities = vec![0u8; column_count];
    for (column, hint) in hints.iter().copied().enumerate().take(column_count) {
        priorities[column] = match session.reader.used_style(hint.style).width() {
            PreferredSize::Percent(_) | PreferredSize::Calc { .. } | PreferredSize::Comparison { .. } => 2,
            PreferredSize::Px(_) | PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent | PreferredSize::Stretch => 1,
            PreferredSize::Auto => 0,
        };
    }
    for &placement in placements {
        let Some(range) = placement.column_range(column_count) else { continue };
        let borders = collapsed_grid.map(|grid| grid.cell_insets(placement));
        let Some(hint) = resolve_cell_width_hint(session, placement.cell_idx, session.reader.style(placement.cell_idx).width(), content_width, borders) else { continue };
        let current = preferred[range.clone()].iter().sum::<f64>();
        if current < hint {
            distribute_deficit(&mut preferred, range.start, range.end, hint - current);
        } else if current > hint {
            distribute_surplus(&mut preferred, &minimums, range.start, range.end, current - hint);
        }
        if placement.colspan == 1 {
            priorities[placement.col] = priorities[placement.col].max(preferred_box_width_priority(session, placement.cell_idx));
        }
    }

    for column in 0..column_count {
        maximums[column] = maximums[column].max(minimums[column]);
        preferred[column] = preferred[column].clamp(minimums[column], maximums[column]);
    }
    let widths = resolve_column_widths(&minimums, &maximums, Some(&preferred), Some(&priorities), usable_width);
    let table_width = widths.iter().sum::<f64>() + spacing * (column_count as f64 + 1.0);
    let mut starts = Vec::with_capacity(widths.len());
    let mut cursor = spacing;
    for &width in &widths {
        starts.push(cursor);
        cursor += width + spacing;
    }
    TableColumnLayout { widths, starts, table_width }
}

pub(super) fn intrinsic_column_widths(
    session: &LayoutEngine<'_, '_>, placements: &[TableCellPlacement], column_count: usize, hints: &[TableColumnWidthHint], spacing: f64, collapsed_grid: Option<&CollapsedBorderGrid>,
) -> (Vec<f64>, Vec<f64>) {
    let mut minimums = vec![0.0f64; column_count];
    let mut maximums = vec![0.0f64; column_count];
    let mut definite_column_widths = vec![None; column_count];
    for (column, hint) in hints.iter().copied().enumerate().take(column_count) {
        let style = session.reader.used_style(hint.style);
        let width = resolve_used_preferred_size(style.width(), 0.0, 0.0).max(0.0);
        let min_width = resolve_used_preferred_size(style.min_width(), 0.0, 0.0).max(0.0);
        let max_width = resolve_used_preferred_size(style.max_width(), f64::INFINITY, 0.0).max(min_width);
        let constrained_width = width.max(min_width).min(max_width);
        if constrained_width > 0.0 {
            minimums[column] = constrained_width;
            maximums[column] = constrained_width;
            definite_column_widths[column] = Some(constrained_width);
        }
    }
    for placement in placements.iter().filter(|placement| placement.colspan == 1) {
        if placement.col >= column_count {
            continue;
        }
        // Each cell owns the half of every collapsed edge that lies inside
        // its grid area. The table adds the outward-facing halves of its
        // outer edges separately, so using the authored full cell borders
        // here would count those outer halves twice.
        let used_borders = collapsed_grid.map(|grid| grid.cell_insets(*placement));
        let (cell_min, cell_max) = constrained_cell_intrinsic_widths(session, placement.cell_idx, used_borders);
        minimums[placement.col] = minimums[placement.col].max(cell_min);
        maximums[placement.col] = maximums[placement.col].max(cell_max.max(cell_min));
    }
    for &placement in placements.iter().filter(|placement| placement.colspan > 1) {
        let Some(range) = placement.column_range(column_count) else { continue };
        let internal_spacing = spacing * (range.len() - 1) as f64;
        let used_borders = collapsed_grid.map(|grid| grid.cell_insets(placement));
        let (cell_min, cell_max) = constrained_cell_intrinsic_widths(session, placement.cell_idx, used_borders);
        let required_min = (cell_min - internal_spacing).max(0.0);
        let required_max = (cell_max.max(cell_min) - internal_spacing).max(required_min);
        let current_min = minimums[range.clone()].iter().sum::<f64>();
        if current_min < required_min {
            distribute_deficit(&mut minimums, range.start, range.end, required_min - current_min);
        }
        let current_max = maximums[range.clone()].iter().sum::<f64>();
        if current_max < required_max {
            distribute_deficit(&mut maximums, range.start, range.end, required_max - current_max);
        }
    }
    for column in 0..column_count {
        // A definite <col> width is the preferred width of that track. Cell
        // min-content can still force the track wider, but its max-content
        // width must not erase the authored preference and make an auto-width
        // table expand all the way to an unwrapped line.
        if definite_column_widths[column].is_some() {
            maximums[column] = minimums[column];
        }
        maximums[column] = maximums[column].max(minimums[column]);
    }
    if let Some(grid) = collapsed_grid {
        // An internal collapsed edge straddles its column boundary. Reserve
        // half of it in each adjacent track so an otherwise zero-width table
        // still contains the whole edge. Applying this symmetric floor after
        // content measurement avoids biasing equal-column surplus toward the
        // element that happened to author the winning border.
        for column in 0..column_count {
            let left = (column > 0).then(|| grid.vertical_boundary_max(column) * 0.5).unwrap_or(0.0);
            let right = (column + 1 < column_count).then(|| grid.vertical_boundary_max(column + 1) * 0.5).unwrap_or(0.0);
            let border_floor = left + right;
            minimums[column] = minimums[column].max(border_floor);
            maximums[column] = maximums[column].max(minimums[column]);
        }
    }
    (minimums, maximums)
}

fn constrained_cell_intrinsic_widths(session: &LayoutEngine<'_, '_>, cell_idx: usize, used_borders: Option<UsedBorderInsets>) -> (f64, f64) {
    let style = session.reader.style(cell_idx);
    let box_model = used_borders.map_or_else(|| ResolvedBoxModel::new(style, 0.0), |borders| ResolvedBoxModel::new(style, 0.0).with_used_borders(borders));
    let extras = box_model.horizontal_padding_border();
    let (content_min, content_max) = crate::layout::box_content_intrinsic_widths(session, cell_idx);
    let mut minimum = content_min + extras;
    let mut maximum = content_max + extras;
    if let PreferredSize::Px(width) = style.width() {
        let specified = match style.box_sizing() {
            BoxSizing::ContentBox => width.max(0.0) as f64 + extras,
            BoxSizing::BorderBox => (width.max(0.0) as f64).max(extras),
        };
        minimum = specified;
        maximum = specified;
    }
    let definite_bound = |value| match value {
        PreferredSize::Px(px) => Some(match style.box_sizing() {
            BoxSizing::ContentBox => (px as f64 + extras).max(0.0),
            BoxSizing::BorderBox => (px as f64).max(extras),
        }),
        _ => None,
    };
    let lower = definite_bound(style.min_width()).unwrap_or(0.0);
    let upper = definite_bound(style.max_width()).unwrap_or(f64::INFINITY).max(lower);
    minimum = minimum.clamp(lower, upper);
    maximum = maximum.clamp(minimum, upper);
    (minimum, maximum)
}

fn specified_width_extras(session: &LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64, used_borders: Option<UsedBorderInsets>) -> f64 {
    let style = session.reader.style(box_idx);
    let box_model = used_borders.map_or_else(|| ResolvedBoxModel::new(style, containing_width), |borders| ResolvedBoxModel::new(style, containing_width).with_used_borders(borders));
    match style.box_sizing() {
        BoxSizing::ContentBox if session.reader.is_table_cell_box(box_idx) => box_model.horizontal_padding_border(),
        BoxSizing::ContentBox => box_model.horizontal_noncontent(),
        BoxSizing::BorderBox if session.reader.is_table_cell_box(box_idx) => 0.0,
        BoxSizing::BorderBox => box_model.horizontal_margin(),
    }
}

fn resolve_width_hint(session: &LayoutEngine<'_, '_>, box_idx: usize, value: PreferredSize, table_width: f64, used_borders: Option<UsedBorderInsets>) -> Option<f64> {
    let extras = specified_width_extras(session, box_idx, table_width, used_borders);
    match value {
        PreferredSize::Auto => None,
        PreferredSize::Px(px) => Some((px as f64 + extras).max(0.0)),
        PreferredSize::Percent(percent) => Some((table_width * percent as f64 + extras).max(0.0)),
        PreferredSize::Calc { absolute_px, percentage, .. } => Some((absolute_px as f64 + table_width * percentage as f64 + extras).max(0.0)),
        PreferredSize::Comparison { .. } => Some((resolve_used_preferred_size(value, 0.0, table_width) + extras).max(0.0)),
        PreferredSize::MinContent => Some(crate::layout::box_intrinsic_widths(session, box_idx).0),
        PreferredSize::MaxContent => Some(crate::layout::box_intrinsic_widths(session, box_idx).1),
        PreferredSize::FitContent => {
            let (min, max) = crate::layout::box_intrinsic_widths(session, box_idx);
            Some(max.min(table_width.max(min)))
        }
        PreferredSize::Stretch => Some((table_width + extras).max(0.0)),
    }
}

fn resolve_cell_width_hint(session: &LayoutEngine<'_, '_>, box_idx: usize, value: PreferredSize, table_width: f64, used_borders: Option<UsedBorderInsets>) -> Option<f64> {
    // CSS table sizing treats a cell's width as a track hint only when it is
    // a length or a percentage. A genuinely mixed <length-percentage> calc
    // is neither, so it behaves like auto here. Pure calc expressions have
    // already been reduced to Px or Percent by the style boundary.
    if matches!(value, PreferredSize::Calc { absolute_px, percentage, .. } if absolute_px != 0.0 && percentage != 0.0) {
        return None;
    }
    resolve_width_hint(session, box_idx, value, table_width, used_borders)
}

fn preferred_box_width_priority(session: &LayoutEngine<'_, '_>, box_idx: usize) -> u8 {
    match session.reader.style(box_idx).width() {
        PreferredSize::Percent(_) => 2,
        PreferredSize::Calc { absolute_px, percentage, .. } if absolute_px != 0.0 && percentage != 0.0 => 0,
        PreferredSize::Calc { .. } => 2,
        PreferredSize::Comparison { .. } => 0,
        PreferredSize::Px(_) | PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent | PreferredSize::Stretch => 1,
        PreferredSize::Auto => 0,
    }
}
