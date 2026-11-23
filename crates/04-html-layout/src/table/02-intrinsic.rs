//! Measure intrinsic table widths, reusing prepared column measures when available.

use crate::LayoutEngine;

use super::s01_grid::{PreparedTableGrid, prepare_table_grid};
use super::s03_column_sizing::{WidthMeasure, measure_table_columns};

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TableIntrinsicSizes {
    pub minimum: f64,
    pub maximum: f64,
    pub grid_maximum: f64,
    pub full_width_percentage_grid: bool,
}

pub fn table_intrinsic_widths(
    engine: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
) -> (f64, f64) {
    let Some(table) = engine.read.reader.box_table(table_box_idx) else {
        return (0.0, 0.0);
    };
    let prepared = prepare_table_grid(engine, table_box_idx, table);
    let measures = measure_table_columns(engine, &prepared);
    let sizes = intrinsic_sizes_for_grid(engine, table_box_idx, &prepared, &measures);
    (sizes.minimum, sizes.maximum)
}

pub(super) fn intrinsic_sizes_for_grid(
    engine: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    prepared: &PreparedTableGrid,
    measures: &[WidthMeasure],
) -> TableIntrinsicSizes {
    let Some(table) = engine.read.reader.box_table(table_box_idx) else {
        return TableIntrinsicSizes::default();
    };
    let style = engine.read.reader.style(table_box_idx);
    let grid = &prepared.grid;
    let caption_minimum =
        caption_minimum_width(engine, &table.captions_top, &table.captions_bottom);
    let caption_grid_minimum =
        caption_minimum_grid_contribution(engine, table_box_idx, caption_minimum);

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
        std::iter::successors(engine.read.reader.get_parent(table_box_idx), |&box_idx| {
            engine.read.reader.get_parent(box_idx)
        })
        .any(|box_idx| engine.read.reader.is_table_cell_box(box_idx));
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

fn caption_minimum_width(
    engine: &mut LayoutEngine<'_, '_>,
    captions_top: &[u32],
    captions_bottom: &[u32],
) -> f64 {
    captions_top
        .iter()
        .chain(captions_bottom)
        .map(|&caption| crate::measurement::box_intrinsic_widths(engine, caption as usize).0)
        .fold(0.0, f64::max)
}

fn caption_minimum_grid_contribution(
    engine: &LayoutEngine<'_, '_>,
    table_box_idx: usize,
    caption_minimum: f64,
) -> f64 {
    let box_model = engine
        .read
        .reader
        .sizing_policy(table_box_idx)
        .adjust_box_model(engine.read.reader.box_model(table_box_idx, 0.0));
    let table_outer_extras = box_model.horizontal_margin() + box_model.horizontal_padding_border();
    (caption_minimum - table_outer_extras).max(0.0)
}
