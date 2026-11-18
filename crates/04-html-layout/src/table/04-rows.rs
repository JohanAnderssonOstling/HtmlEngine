//! Prepare table cells and lay out standalone rows before final table row sizing.

use super::cell::{RetainedCellLayout, layout_cell, retain_cell};
use super::geometry::TableColumnLayout;
use super::s01_grid::PreparedTableGrid;
use crate::boxes::absolute_positioning;
use crate::{BoxLayoutRequest, LayoutEngine};
use html_engine_model::{InlineItemKind, LayoutMode};
use html_style_model::{UsedPreferredSize as PreferredSize, VerticalAlignValue};
use kurbo::{Point, Size, Vec2};

pub(super) struct PreparedTableCells {
    pub(super) cells: Vec<PreparedCell>,
    pub(super) row_baselines: Vec<Option<f64>>,
}

pub(super) struct PreparedCell {
    pub(super) height: f64,
    pub(super) baseline: Option<f64>,
    pub(super) layout: PreparedCellLayout,
}

pub(super) enum PreparedCellLayout {
    Retained(RetainedCellLayout),
    Relayout(CellHeightDependency),
}

pub(super) fn prepare_cells(
    engine: &mut LayoutEngine<'_, '_>,
    prepared: &PreparedTableGrid,
    columns: &TableColumnLayout,
    table_content_pos: Point,
    parent_content_height: Option<f64>,
    definite_row_area_height: Option<f64>,
) -> PreparedTableCells {
    let placements = &prepared.grid.placements;
    let v_spacing = prepared.vertical_spacing;
    let collapsed_grid = prepared.collapsed_borders.as_ref();
    let mut cells = Vec::with_capacity(placements.len());
    for placement in placements {
        let span_width = columns.span_width(placement.col, placement.colspan);
        let provisional_point = Point::new(
            table_content_pos.x + columns.physical_start(placement.col, placement.colspan),
            table_content_pos.y + v_spacing,
        );
        let used_borders = collapsed_grid.map(|grid| grid.cell_insets(*placement));
        let dependency = cell_height_dependency(engine, placement.cell_idx);
        let mut request = BoxLayoutRequest::table_cell(
            placement.cell_idx,
            span_width,
            parent_content_height,
            used_borders,
        );
        if dependency == CellHeightDependency::AssignedHeight
            && let Some(row_area_height) = definite_row_area_height
        {
            let provisional_span_height = row_area_height
                * placement.rowspan.min(prepared.rows.len()) as f64
                / prepared.rows.len().max(1) as f64;
            request = request.with_table_cell_height(provisional_span_height);
        } else {
            request = request.for_table_intrinsic_measurement();
        }
        let (cell_size, baseline, layout) = if dependency == CellHeightDependency::Independent {
            let (size, baseline, retained) =
                retain_cell(engine, provisional_point, request, used_borders);
            (size, baseline, PreparedCellLayout::Retained(retained))
        } else {
            let (size, baseline) = measure_cell_isolated(engine, provisional_point, request);
            (size, baseline, PreparedCellLayout::Relayout(dependency))
        };
        cells.push(PreparedCell {
            height: cell_size.height,
            baseline,
            layout,
        });
    }

    let mut row_baselines = vec![None::<f64>; prepared.rows.len()];
    for (placement, cell) in placements
        .iter()
        .zip(&cells)
        .filter(|(placement, _)| placement.rowspan == 1)
    {
        if cell_uses_baseline_alignment(engine, placement.cell_idx)
            && let Some(cell_baseline) = cell.baseline
        {
            row_baselines[placement.row] = Some(
                row_baselines[placement.row]
                    .map_or(cell_baseline, |current| current.max(cell_baseline)),
            );
        }
    }
    PreparedTableCells {
        cells,
        row_baselines,
    }
}

pub fn layout_table_row(
    engine: &mut LayoutEngine<'_, '_>,
    cells: &[u32],
    out_of_flow: &[u32],
    content_pos: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
) -> Size {
    if cells.is_empty() {
        for &child in out_of_flow {
            absolute_positioning::defer(engine, child as usize, content_pos);
        }
        return Size::ZERO;
    }
    let column_width = (content_width / cells.len() as f64).max(0.0);
    let mut row_height = 0.0f64;
    for (column, &cell_idx) in cells.iter().enumerate() {
        let cell_idx = cell_idx as usize;
        let point = content_pos + Vec2::new(column as f64 * column_width, 0.0);
        let request =
            BoxLayoutRequest::table_cell(cell_idx, column_width, parent_content_height, None);
        row_height = row_height.max(layout_cell(engine, point, request).size.height);
    }
    for &cell_idx in cells {
        let mut size = engine.geometry.size(cell_idx as usize);
        size.height = size.height.max(row_height);
        engine.geometry.set_size(cell_idx as usize, size);
    }
    for &child in out_of_flow {
        absolute_positioning::defer(engine, child as usize, content_pos);
    }
    Size::new(content_width.max(0.0), row_height)
}

fn measure_cell_isolated(
    engine: &mut LayoutEngine<'_, '_>,
    point: Point,
    request: BoxLayoutRequest,
) -> (Size, Option<f64>) {
    let layout = crate::measurement::with_isolated_measurement(engine, |engine| {
        layout_cell(engine, point, request)
    });
    (layout.size, layout.first_baseline)
}

/// Increasing requirements for the block-size basis of a cell's descendants.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CellHeightDependency {
    Independent,
    ParentBasis,
    AssignedHeight,
}

impl CellHeightDependency {
    fn strongest(dependencies: impl Iterator<Item = Self>) -> Self {
        let mut result = Self::Independent;
        for dependency in dependencies {
            result = result.max(dependency);
            if result == Self::AssignedHeight {
                break;
            }
        }
        result
    }
}

fn cell_height_dependency(
    engine: &mut LayoutEngine<'_, '_>,
    cell_idx: usize,
) -> CellHeightDependency {
    if let Some(&dependency) = engine.scratch.cell_height_dependencies.get(&cell_idx) {
        return dependency;
    }

    fn children_dependency(engine: &LayoutEngine<'_, '_>, box_idx: usize) -> CellHeightDependency {
        CellHeightDependency::strongest(
            engine
                .child_boxes(box_idx, |kind| match kind {
                    InlineItemKind::AtomicBox { box_idx } => Some(*box_idx),
                    _ => None,
                })
                .map(|child| box_dependency(engine, child)),
        )
    }

    fn box_dependency(engine: &LayoutEngine<'_, '_>, box_idx: usize) -> CellHeightDependency {
        let style = engine.read.reader.style(box_idx);
        if style.height().percentage_dependent() && style.overflow_y().clips() {
            return CellHeightDependency::AssignedHeight;
        }
        let own = if [style.height(), style.min_height(), style.max_height()]
            .into_iter()
            .any(|size| size.percentage_dependent() || matches!(size, PreferredSize::Stretch))
        {
            CellHeightDependency::ParentBasis
        } else {
            CellHeightDependency::Independent
        };
        let descendants = match engine.read.reader.box_layout_mode(box_idx) {
            Some(
                LayoutMode::Block(_)
                | LayoutMode::TableCell(_)
                | LayoutMode::Flex(_)
                | LayoutMode::Grid(_),
            ) => children_dependency(engine, box_idx),
            _ => CellHeightDependency::Independent,
        };
        own.max(descendants)
    }

    let dependency = match engine.read.reader.box_layout_mode(cell_idx) {
        Some(LayoutMode::TableCell(_)) => children_dependency(engine, cell_idx),
        _ => CellHeightDependency::ParentBasis,
    };
    engine
        .scratch
        .cell_height_dependencies
        .insert(cell_idx, dependency);
    dependency
}

fn cell_uses_baseline_alignment(engine: &LayoutEngine<'_, '_>, cell_idx: usize) -> bool {
    matches!(
        engine.read.reader.style(cell_idx).vertical_align(),
        VerticalAlignValue::Baseline
            | VerticalAlignValue::Sub
            | VerticalAlignValue::Super
            | VerticalAlignValue::Length(_)
            | VerticalAlignValue::Percent(_)
            | VerticalAlignValue::Calc { .. }
            | VerticalAlignValue::TextTop
    )
}
