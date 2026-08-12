use crate::layout::{LayoutEngine, ResolvedBoxModel};
use crate::layout_model::LayoutMode;

use super::borders::CollapsedBorderGrid;
use super::captions::intrinsic_widths as caption_intrinsic_widths;
use super::grid::{effective_table_grid, table_grid};
use super::sizing::intrinsic_column_widths;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TableIntrinsicSizes {
    pub(crate) minimum: f64,
    pub(crate) maximum: f64,
    pub(crate) grid_minimum: f64,
    pub(crate) grid_maximum: f64,
    pub(crate) full_width_percentage_grid: bool,
}

fn caption_minimum_grid_contribution(
    session: &LayoutEngine<'_, '_>,
    table_box_idx: usize,
    caption_minimum: f64,
) -> f64 {
    let style = session.reader.style(table_box_idx);
    let mut box_model = ResolvedBoxModel::new(style, 0.0);
    if matches!(
        style.border_collapse(),
        html_style_model::BorderCollapseMode::Collapse
    ) {
        // Collapsed borders are already part of the grid intrinsic sizes.
        // Match `box_intrinsic_widths`, which therefore does not add the
        // authored table padding and borders around those sizes.
        box_model = box_model
            .without_padding()
            .with_used_borders(crate::layout::UsedBorderInsets::default());
    }
    let table_outer_extras = box_model.horizontal_margin() + box_model.horizontal_padding_border();
    (caption_minimum - table_outer_extras).max(0.0)
}

pub(crate) fn table_intrinsic_sizes(
    session: &LayoutEngine<'_, '_>,
    table_box_idx: usize,
) -> TableIntrinsicSizes {
    let Some(LayoutMode::Table(table)) = session.reader.box_layout_mode(table_box_idx) else {
        return TableIntrinsicSizes::default();
    };
    let style = session.reader.style(table_box_idx);
    let fixed_mode = matches!(
        style.table_layout(),
        html_style_model::TableLayoutMode::Fixed
    ) && !matches!(style.width(), html_style_model::UsedPreferredSize::Auto);
    let (grid, track_sources, logical_to_effective) = effective_table_grid(
        session,
        table_grid(session, &table.rows),
        &table.column_width_hints,
        fixed_mode,
    );
    if grid.column_count == 0 {
        let (caption_minimum, _) =
            caption_intrinsic_widths(session, &table.captions_top, &table.captions_bottom);
        let minimum = caption_minimum_grid_contribution(session, table_box_idx, caption_minimum);
        return TableIntrinsicSizes {
            minimum,
            maximum: minimum,
            grid_minimum: minimum,
            grid_maximum: 0.0,
            full_width_percentage_grid: false,
        };
    }

    let h_spacing = if matches!(
        style.border_collapse(),
        html_style_model::BorderCollapseMode::Collapse
    ) {
        0.0
    } else {
        style.border_spacing_horizontal().max(0.0) as f64
    };
    let effective_hints = track_sources
        .iter()
        .filter_map(|&source| table.column_width_hints.get(source).copied())
        .collect::<Vec<_>>();
    let effective_columns = track_sources
        .iter()
        .filter_map(|&source| table.columns.get(source).cloned())
        .collect::<Vec<_>>();
    let effective_column_groups = table
        .column_groups
        .iter()
        .filter_map(|group| {
            let logical_end = group
                .start
                .saturating_add(group.span)
                .min(logical_to_effective.len());
            if logical_end <= group.start {
                return None;
            }
            let start = logical_to_effective[group.start];
            let end = logical_to_effective[logical_end - 1] + 1;
            Some(crate::layout_model::TableColumnGroupSpan {
                style: group.style,
                start,
                span: end - start,
            })
        })
        .collect::<Vec<_>>();
    let collapsed_grid = matches!(
        style.border_collapse(),
        html_style_model::BorderCollapseMode::Collapse
    )
    .then(|| {
        CollapsedBorderGrid::build(
            &session.reader,
            table_box_idx,
            &table.rows,
            &table.row_groups,
            &grid.placements,
            &effective_columns,
            &effective_column_groups,
        )
    });
    let (min_widths, max_widths, percentages) = intrinsic_column_widths(
        session,
        &grid.placements,
        grid.column_count,
        &effective_hints,
        h_spacing,
        collapsed_grid.as_ref(),
        fixed_mode,
    );
    let spacing = h_spacing * (grid.column_count as f64 + 1.0);
    // Intrinsic widths describe the complete used outer width, matching the
    // width contract consumed by final table layout.
    let outer_width = collapsed_grid
        .as_ref()
        .map_or(0.0, |grid| grid.outer_insets().horizontal());
    let grid_minimum = min_widths.iter().sum::<f64>() + spacing + outer_width;
    let direct_grid_maximum = max_widths.iter().sum::<f64>();
    let mut inverse_percentage_maximum = direct_grid_maximum;
    let mut percentage_sum = 0.0f64;
    let mut non_percentage_maximum = 0.0f64;
    for column in 0..grid.column_count {
        let percentage = percentages[column];
        if percentage > 0.0 {
            percentage_sum += percentage;
            inverse_percentage_maximum =
                inverse_percentage_maximum.max(max_widths[column] / percentage);
        } else {
            non_percentage_maximum += max_widths[column];
        }
    }
    if percentage_sum > 0.0 && percentage_sum < 1.0 {
        inverse_percentage_maximum =
            inverse_percentage_maximum.max(non_percentage_maximum / (1.0 - percentage_sum));
    }
    // `max-content` deliberately uses the direct sum. Auto and fit-content
    // tables also honor the inverse constraint implied by percentage tracks.
    let nested_in_table_cell =
        std::iter::successors(session.reader.get_parent(table_box_idx), |&box_idx| {
            session.reader.get_parent(box_idx)
        })
        .any(|box_idx| session.reader.is_table_cell_box(box_idx));
    let grid_content_maximum = if matches!(
        style.width(),
        html_style_model::UsedPreferredSize::MaxContent
    ) || nested_in_table_cell
    {
        direct_grid_maximum
    } else {
        inverse_percentage_maximum
    };
    let grid_maximum = grid_content_maximum + spacing + outer_width;
    let (caption_minimum, _) =
        caption_intrinsic_widths(session, &table.captions_top, &table.captions_bottom);
    let caption_grid_minimum =
        caption_minimum_grid_contribution(session, table_box_idx, caption_minimum);
    let effective_grid_minimum = grid_minimum.max(caption_grid_minimum);
    // A caption's minimum inline-size is a lower bound for the grid. Its
    // max-content size does not by itself make an auto table wider.
    let minimum = effective_grid_minimum;
    let maximum = grid_maximum.max(minimum);
    // A percentage track in a table nested inside a cell is ignored for the
    // outer cell's intrinsic contribution, but final layout still resolves
    // it against the width allocated to that cell.
    let fills_available_grid =
        percentage_sum >= 1.0 || (nested_in_table_cell && percentage_sum > 0.0);
    TableIntrinsicSizes {
        minimum,
        maximum,
        grid_minimum: effective_grid_minimum,
        grid_maximum,
        full_width_percentage_grid: fills_available_grid,
    }
}

pub(crate) fn table_intrinsic_widths(
    session: &LayoutEngine<'_, '_>,
    table_box_idx: usize,
) -> (f64, f64) {
    let sizes = table_intrinsic_sizes(session, table_box_idx);
    (sizes.minimum, sizes.maximum)
}
