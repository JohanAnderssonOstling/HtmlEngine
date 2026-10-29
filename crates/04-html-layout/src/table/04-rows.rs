use super::columns::TableColumnLayout;
use super::grid::PreparedTableGrid;
use crate::layout::{
    BoxLayoutRequest, BoxLayoutResult, LayoutEngine, MeasurementWorker, OutputRanges,
    ParallelBoxMeasurement, ResolvedBoxModel, UsedBorderInsets, install_parallel,
};
use crate::layout_model::{Children, InlineItemKind, LayoutMode};
use html_style_model::{UsedPreferredSize as PreferredSize, VerticalAlignValue};
use kurbo::{Point, Size, Vec2};
use rayon::prelude::*;

const PARALLEL_CELL_MIN_TASKS: usize = 8;
const PARALLEL_CELL_MIN_GLYPHS: usize = 4_096;

#[derive(Clone, Copy)]
struct ParallelCellTask {
    index: usize,
    point: Point,
    request: BoxLayoutRequest,
    glyph_cost: usize,
}

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
    Relayout { assign_final_height: bool },
}

pub(super) struct RetainedCellLayout {
    pub(super) output: OutputRanges,
    pub(super) provisional_point: Point,
    pub(super) natural_border_height: f64,
    pub(super) has_visible_content: bool,
}

pub(super) fn prepare_cells(
    session: &mut LayoutEngine<'_, '_>,
    prepared: &PreparedTableGrid,
    row_count: usize,
    columns: &TableColumnLayout,
    table_content_pos: Point,
    parent_content_height: Option<f64>,
    definite_row_area_height: Option<f64>,
) -> PreparedTableCells {
    let placements = &prepared.grid.placements;
    let v_spacing = prepared.vertical_spacing;
    let collapsed_grid = prepared.collapsed_borders.as_ref();
    let mut cells = Vec::with_capacity(placements.len());
    let parallel_workers = session.config.parallel_workers();
    let mut parallel_tasks = Vec::new();
    for (index, placement) in placements.iter().enumerate() {
        let span_width = columns.span_width(placement.col, placement.colspan);
        let provisional_point = Point::new(
            table_content_pos.x + columns.physical_start(placement.col, placement.colspan),
            table_content_pos.y + v_spacing,
        );
        let used_borders = collapsed_grid.map(|grid| grid.cell_insets(*placement));
        let dependency = cell_height_dependency(session, placement.cell_idx);
        let assign_final_height = dependency == CellHeightDependency::AssignedHeight;
        let reusable = dependency == CellHeightDependency::Independent;
        let mut request = BoxLayoutRequest::table_cell(
            placement.cell_idx,
            span_width,
            parent_content_height,
            used_borders,
        );
        if assign_final_height && let Some(row_area_height) = definite_row_area_height {
            let provisional_span_height =
                row_area_height * placement.rowspan.min(row_count) as f64 / row_count.max(1) as f64;
            request = request.with_assigned_border_height(provisional_span_height);
        } else {
            request = request.for_table_intrinsic_measurement();
        }
        let (cell_size, baseline, layout) = if reusable {
            let cell_layout = layout_cell(session, provisional_point, request);
            let baseline = cell_layout.first_baseline;
            let (natural_border_height, has_visible_content) = cell_content_metrics(
                session,
                placement.cell_idx,
                span_width,
                used_borders,
                &cell_layout,
            );
            let cell_size = cell_layout.size;
            let layout = PreparedCellLayout::Retained(RetainedCellLayout {
                output: cell_layout.output,
                provisional_point,
                natural_border_height,
                has_visible_content,
            });
            (cell_size, baseline, layout)
        } else {
            let (size, baseline) = if parallel_workers > 1 {
                parallel_tasks.push(ParallelCellTask {
                    index,
                    point: provisional_point,
                    request,
                    glyph_cost: cell_subtree_glyph_cost(session, placement.cell_idx),
                });
                (Size::ZERO, None)
            } else {
                measure_cell_isolated(session, provisional_point, request)
            };
            (
                size,
                baseline,
                PreparedCellLayout::Relayout {
                    assign_final_height,
                },
            )
        };
        cells.push(PreparedCell {
            height: cell_size.height,
            baseline,
            layout,
        });
    }

    measure_dependent_cells(session, &parallel_tasks, &mut cells);

    let mut row_baselines = vec![None::<f64>; row_count];
    for (placement, cell) in placements
        .iter()
        .zip(&cells)
        .filter(|(placement, _)| placement.rowspan == 1)
    {
        if cell_uses_baseline_alignment(session, placement.cell_idx)
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

fn measure_dependent_cells(
    session: &mut LayoutEngine<'_, '_>,
    tasks: &[ParallelCellTask],
    cells: &mut [PreparedCell],
) {
    let parallel_workers = session.config.parallel_workers();
    let glyph_cost = tasks.iter().map(|task| task.glyph_cost).sum::<usize>();
    if tasks.len() >= PARALLEL_CELL_MIN_TASKS && glyph_cost >= PARALLEL_CELL_MIN_GLYPHS {
        let context = session.measurement_context();
        let measurements = install_parallel(parallel_workers, || {
            tasks
                .par_iter()
                .map_init(MeasurementWorker::default, |worker, task| {
                    context.measure_table_cell(worker, task.point, task.request)
                })
                .collect::<Vec<ParallelBoxMeasurement>>()
        });
        for (task, measurement) in tasks.iter().zip(measurements) {
            cells[task.index].height = measurement.layout.size.height;
            cells[task.index].baseline = measurement.layout.first_baseline;
            session.retain_measurement_plans(measurement);
        }
    } else {
        for task in tasks {
            let (size, baseline) = measure_cell_isolated(session, task.point, task.request);
            cells[task.index].height = size.height;
            cells[task.index].baseline = baseline;
        }
    }
}

fn cell_subtree_glyph_cost(session: &LayoutEngine<'_, '_>, box_idx: usize) -> usize {
    fn children_cost(session: &LayoutEngine<'_, '_>, children: &Children) -> usize {
        match children {
            Children::Blocks(children) => children
                .iter()
                .map(|&child| box_cost(session, child as usize))
                .sum(),
            Children::InlineItems(range) => range
                .clone()
                .filter_map(|item_idx| session.text.inline_item(item_idx as usize))
                .map(|item| match &item.kind {
                    InlineItemKind::Text { glyphs } => glyphs.len(),
                    InlineItemKind::AtomicBox { box_idx } => box_cost(session, *box_idx as usize),
                    _ => 1,
                })
                .sum(),
            Children::Empty => 0,
        }
    }

    fn box_cost(session: &LayoutEngine<'_, '_>, box_idx: usize) -> usize {
        match session.reader.box_layout_mode(box_idx) {
            Some(LayoutMode::Block(block)) => children_cost(session, &block.children),
            Some(LayoutMode::TableCell(cell)) => children_cost(session, &cell.children),
            Some(LayoutMode::Flex(container)) => container
                .children
                .iter()
                .map(|&child| box_cost(session, child as usize))
                .sum(),
            Some(LayoutMode::Grid(container)) => container
                .children
                .iter()
                .map(|&child| box_cost(session, child as usize))
                .sum(),
            Some(LayoutMode::Inline(range) | LayoutMode::Anonymous(range)) => {
                children_cost(session, &Children::InlineItems(range.clone()))
            }
            _ => 1,
        }
    }

    box_cost(session, box_idx)
}

fn measure_cell_isolated(
    session: &mut LayoutEngine<'_, '_>,
    point: Point,
    request: BoxLayoutRequest,
) -> (Size, Option<f64>) {
    let layout = crate::layout::with_isolated_measurement(session, |session| {
        layout_cell(session, point, request)
    });
    (layout.size, layout.first_baseline)
}

fn layout_cell(
    session: &mut LayoutEngine<'_, '_>,
    point: Point,
    request: BoxLayoutRequest,
) -> BoxLayoutResult {
    session.without_fragmentation(|session| {
        session.geometry.set_point(request.box_idx(), point);
        session.layout_box(request)
    })
}

/// Increasing requirements for the block-size basis of a cell's descendants.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CellHeightDependency {
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

fn cell_height_dependency(session: &LayoutEngine<'_, '_>, cell_idx: usize) -> CellHeightDependency {
    fn children_dependency(
        session: &LayoutEngine<'_, '_>,
        children: &Children,
    ) -> CellHeightDependency {
        match children {
            Children::Blocks(children) => CellHeightDependency::strongest(
                children
                    .iter()
                    .map(|&child| box_dependency(session, child as usize)),
            ),
            Children::InlineItems(range) => {
                CellHeightDependency::strongest(range.clone().filter_map(|item_idx| {
                    let item = session.text.inline_item(item_idx as usize)?;
                    match item.kind {
                        InlineItemKind::AtomicBox { box_idx } => {
                            Some(box_dependency(session, box_idx as usize))
                        }
                        _ => None,
                    }
                }))
            }
            Children::Empty => CellHeightDependency::Independent,
        }
    }

    fn box_dependency(session: &LayoutEngine<'_, '_>, box_idx: usize) -> CellHeightDependency {
        let style = session.reader.style(box_idx);
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
        let descendants = match session.reader.box_layout_mode(box_idx) {
            Some(LayoutMode::Block(block)) => children_dependency(session, &block.children),
            Some(LayoutMode::TableCell(cell)) => children_dependency(session, &cell.children),
            Some(LayoutMode::Flex(container)) => CellHeightDependency::strongest(
                container
                    .children
                    .iter()
                    .map(|&child| box_dependency(session, child as usize)),
            ),
            Some(LayoutMode::Grid(container)) => CellHeightDependency::strongest(
                container
                    .children
                    .iter()
                    .map(|&child| box_dependency(session, child as usize)),
            ),
            _ => CellHeightDependency::Independent,
        };
        own.max(descendants)
    }

    match session.reader.box_layout_mode(cell_idx) {
        Some(LayoutMode::TableCell(cell)) => children_dependency(session, &cell.children),
        _ => CellHeightDependency::ParentBasis,
    }
}

pub fn layout_table_row(
    session: &mut LayoutEngine<'_, '_>,
    cells: &[u32],
    out_of_flow: &[u32],
    content_pos: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
) -> Size {
    let timing_started = session.start_timing();
    if cells.is_empty() {
        for &child in out_of_flow {
            session.defer_absolute_box(child as usize, content_pos);
        }
        return Size::ZERO;
    }
    let column_width = (content_width / cells.len() as f64).max(0.0);
    let mut row_height = 0.0f64;
    for (column, &cell_idx) in cells.iter().enumerate() {
        let cell_idx = cell_idx as usize;
        session.geometry.set_point(
            cell_idx,
            content_pos + Vec2::new(column as f64 * column_width, 0.0),
        );
        row_height = row_height.max(
            session
                .without_fragmentation(|session| {
                    session.layout_box(BoxLayoutRequest::table_cell(
                        cell_idx,
                        column_width,
                        parent_content_height,
                        None,
                    ))
                })
                .size
                .height,
        );
    }
    for &cell_idx in cells {
        let mut size = session.geometry.size(cell_idx as usize);
        size.height = size.height.max(row_height);
        session.geometry.set_size(cell_idx as usize, size);
    }
    for &child in out_of_flow {
        session.defer_absolute_box(child as usize, content_pos);
    }
    session.record_timing(|timings| timings.layout_table_row += timing_started.elapsed());
    Size::new(content_width.max(0.0), row_height)
}

fn cell_uses_baseline_alignment(session: &LayoutEngine<'_, '_>, cell_idx: usize) -> bool {
    matches!(
        session.reader.style(cell_idx).vertical_align(),
        VerticalAlignValue::Baseline
            | VerticalAlignValue::Sub
            | VerticalAlignValue::Super
            | VerticalAlignValue::Length(_)
            | VerticalAlignValue::Percent(_)
            | VerticalAlignValue::Calc { .. }
            | VerticalAlignValue::TextTop
    )
}

pub(super) fn cell_content_metrics(
    session: &LayoutEngine<'_, '_>,
    cell_idx: usize,
    span_width: f64,
    used_borders: Option<UsedBorderInsets>,
    result: &BoxLayoutResult,
) -> (f64, bool) {
    let mut box_model = ResolvedBoxModel::new(session.reader.style(cell_idx), span_width);
    if let Some(borders) = used_borders {
        box_model = box_model.with_used_borders(borders);
    }
    let natural_height = result.natural_content_size.height + box_model.vertical_padding_border();
    let has_visible_content = result.output.has_lines_or_images()
        || matches!(session.reader.box_layout_mode(cell_idx), Some(LayoutMode::TableCell(cell))
            if !matches!(cell.children, Children::Empty));
    (natural_height, has_visible_content)
}
