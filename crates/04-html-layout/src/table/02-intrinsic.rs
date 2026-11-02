use crate::layout::{LayoutEngine, ResolvedBoxModel};
use crate::layout_model::LayoutMode;

use super::captions::minimum_width as caption_minimum_width;
use super::column_sizing::{WidthMeasure, measure_table_columns};
use super::grid::{PreparedTableGrid, prepare_table_grid};

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TableIntrinsicSizes {
    pub minimum: f64,
    pub maximum: f64,
    pub grid_maximum: f64,
    pub full_width_percentage_grid: bool,
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

pub fn table_intrinsic_widths(session: &LayoutEngine<'_, '_>, table_box_idx: usize) -> (f64, f64) {
    let Some(LayoutMode::Table(table)) = session.reader.box_layout_mode(table_box_idx) else {
        return (0.0, 0.0);
    };
    let prepared = prepare_table_grid(session, table_box_idx, &table.rows);
    let measures = measure_table_columns(session, &prepared);
    let sizes = intrinsic_sizes_for_grid(session, table_box_idx, &prepared, &measures);
    (sizes.minimum, sizes.maximum)
}

pub(super) fn intrinsic_sizes_for_grid(
    session: &LayoutEngine<'_, '_>,
    table_box_idx: usize,
    prepared: &PreparedTableGrid,
    measures: &[WidthMeasure],
) -> TableIntrinsicSizes {
    let Some(LayoutMode::Table(table)) = session.reader.box_layout_mode(table_box_idx) else {
        return TableIntrinsicSizes::default();
    };
    let style = session.reader.style(table_box_idx);
    let grid = &prepared.grid;
    let caption_minimum =
        caption_minimum_width(session, &table.captions_top, &table.captions_bottom);
    let caption_grid_minimum =
        caption_minimum_grid_contribution(session, table_box_idx, caption_minimum);
    if grid.column_count == 0 {
        let minimum = caption_grid_minimum;
        return TableIntrinsicSizes {
            minimum,
            maximum: minimum,
            grid_maximum: 0.0,
            full_width_percentage_grid: false,
        };
    }

    let h_spacing = prepared.horizontal_spacing;
    let spacing = h_spacing * (grid.column_count as f64 + 1.0);
    // Intrinsic widths describe the complete used outer width, matching the
    // width contract consumed by final table layout.
    let outer_width = prepared.outer_borders.horizontal();
    let grid_minimum =
        measures.iter().map(|column| column.minimum).sum::<f64>() + spacing + outer_width;
    let direct_grid_maximum = measures.iter().map(|column| column.maximum).sum::<f64>();
    let mut inverse_percentage_maximum = direct_grid_maximum;
    let mut percentage_sum = 0.0f64;
    let mut non_percentage_maximum = 0.0f64;
    for column in measures {
        let percentage = column.percentage;
        if percentage > 0.0 {
            percentage_sum += percentage;
            inverse_percentage_maximum =
                inverse_percentage_maximum.max(column.maximum / percentage);
        } else {
            non_percentage_maximum += column.maximum;
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
        grid_maximum,
        full_width_percentage_grid: fills_available_grid,
    }
}
