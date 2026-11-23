use super::geometry::TableGeometry;
use super::grid::TableCellPlacement;
use super::s01_grid::PreparedTableGrid;
use crate::LayoutEngine;
use crate::decorations::record_bound_background;
use crate::paint_program::PaintStyleSource;
use html_style_model::{StyleIndices, Visibility};
use kurbo::Rect;

pub(super) fn emit_table_layers(
    engine: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    geometry: &TableGeometry,
    prepared: &PreparedTableGrid,
) {
    let table_grid = Rect::from_origin_size(
        geometry.origin,
        (geometry.columns.table_width, geometry.height.max(0.0)),
    );

    if table_grid.height() <= 0.0 || table_grid.width() <= 0.0 {
        return;
    }

    // In the collapsed model the grid canvas is the table background area.
    // Emit it here, before column, row, cell, and border layers. Separate
    // tables retain their ordinary full border-box background path because
    // their padding and border-spacing also belong to the background area.
    emit_collapsed_table_background(engine, table_box_idx, table_grid);

    let mut styles = (0..prepared.grid.column_count)
        .map(|column| {
            let index = prepared
                .column_groups
                .partition_point(|group| group.start <= column)
                .checked_sub(1)?;
            let group = &prepared.column_groups[index];
            (column < group.start.saturating_add(group.span)).then_some((index, group.style))
        })
        .collect::<Vec<_>>();
    emit_table_column_background(engine, table_box_idx, prepared, geometry, &styles);
    styles.clear();
    styles.extend(prepared.columns.iter().enumerate().map(|(index, column)| {
        column
            .paints_background
            .then_some((index, column.width_style))
    }));
    emit_table_column_background(engine, table_box_idx, prepared, geometry, &styles);
}

pub(super) fn emit_table_row_layer_background(
    engine: &mut LayoutEngine<'_, '_>,
    owner_box_idx: usize,
    placements: &[TableCellPlacement],
    geometry: &TableGeometry,
) {
    for placement in placements {
        let rect = geometry.cell_rect(placement);
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            continue;
        }
        record_bound_background(
            engine,
            owner_box_idx,
            rect,
            PaintStyleSource::Box(owner_box_idx as u32),
        );
    }
}

fn emit_collapsed_table_background(
    engine: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    rect: Rect,
) {
    if rect.width() <= 0.0
        || rect.height() <= 0.0
        || !matches!(
            engine.read.reader.style(table_box_idx).border_collapse(),
            html_style_model::BorderCollapseMode::Collapse
        )
    {
        return;
    }
    record_bound_background(
        engine,
        table_box_idx,
        rect,
        PaintStyleSource::Box(table_box_idx as u32),
    );
}

fn emit_table_column_background(
    engine: &mut LayoutEngine<'_, '_>,
    table_box_idx: usize,
    prepared: &PreparedTableGrid,
    geometry: &TableGeometry,
    styles: &[Option<(usize, StyleIndices)>],
) {
    let mut pending = None::<(usize, StyleIndices, Rect)>;
    for cell in &prepared.grid.placements {
        let Some((part, style)) = styles.get(cell.col).copied().flatten() else {
            continue;
        };
        if engine.read.reader.used_style(style).visibility() == Visibility::Hidden {
            continue;
        }
        let rect = geometry.cell_rect(cell);
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            continue;
        }
        if let Some((previous_part, previous_style, previous)) = &mut pending {
            let adjacent = (previous.y0 == rect.y0
                && previous.y1 == rect.y1
                && (previous.x1 == rect.x0 || rect.x1 == previous.x0))
                || (previous.x0 == rect.x0
                    && previous.x1 == rect.x1
                    && (previous.y1 == rect.y0 || rect.y1 == previous.y0));
            if *previous_part == part && *previous_style == style && adjacent {
                *previous = previous.union(rect);
                continue;
            }
            record_bound_background(
                engine,
                table_box_idx,
                *previous,
                PaintStyleSource::Style {
                    indices: *previous_style,
                    context: table_box_idx as u32,
                },
            );
        }
        pending = Some((part, style, rect));
    }
    if let Some((_, style, rect)) = pending {
        record_bound_background(
            engine,
            table_box_idx,
            rect,
            PaintStyleSource::Style {
                indices: style,
                context: table_box_idx as u32,
            },
        );
    }
}
