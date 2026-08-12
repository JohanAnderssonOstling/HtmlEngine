use super::borders::CollapsedBorderGrid;
use super::columns::{TableCellPlacement, TableColumnLayout, span_extent};
use crate::layout::{LayoutEngine, OutputRanges, ResolvedBoxModel};
use crate::layout_model::{Children, LayoutMode};
use html_style_model::{UsedPreferredSize as PreferredSize, VerticalAlignValue};
use kurbo::{Point, Size, Vec2};

pub(super) struct PreparedTableCells {
    pub(super) baseline_offsets: Vec<Option<f64>>,
    pub(super) cell_heights: Vec<f64>,
    pub(super) row_baselines: Vec<Option<f64>>,
    pub(super) layouts: Vec<PreparedCellLayout>,
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
    placements: &[TableCellPlacement],
    row_count: usize,
    columns: &TableColumnLayout,
    table_content_pos: Point,
    v_spacing: f64,
    parent_content_height: Option<f64>,
    collapsed_grid: Option<&CollapsedBorderGrid>,
    definite_row_area_height: Option<f64>,
) -> PreparedTableCells {
    let mut heights = vec![0.0; placements.len()];
    let mut baseline_offsets = vec![None; placements.len()];
    let mut layouts = Vec::with_capacity(placements.len());
    for (index, placement) in placements.iter().enumerate() {
        let span_width = span_extent(
            &columns.starts,
            &columns.widths,
            placement.col,
            placement.colspan,
        );
        let provisional_point = Point::new(
            table_content_pos.x + columns.starts[placement.col],
            table_content_pos.y + v_spacing,
        );
        let used_borders = collapsed_grid.map(|grid| grid.cell_insets(*placement));
        let assign_final_height =
            cell_has_restricted_percentage_height_descendant(session, placement.cell_idx);
        let reusable = !assign_final_height
            && !cell_content_uses_parent_block_basis(session, placement.cell_idx);
        let layout_cell = |session: &mut LayoutEngine<'_, '_>| {
            session.without_fragmentation(|session| {
                session
                    .geometry
                    .set_point(placement.cell_idx, provisional_point);
                let mut request = crate::layout::BoxLayoutRequest::table_cell(
                    placement.cell_idx,
                    span_width,
                    parent_content_height,
                    used_borders,
                );
                if assign_final_height && let Some(row_area_height) = definite_row_area_height {
                    let provisional_span_height = row_area_height
                        * placement.rowspan.min(row_count) as f64
                        / row_count.max(1) as f64;
                    request = request.with_assigned_border_height(provisional_span_height);
                } else {
                    request = request.for_table_intrinsic_measurement();
                }
                let cell_layout = session.layout_box(request);
                let baseline = session.fragments.first_baseline_offset(
                    cell_layout.output.lines.clone(),
                    session.geometry.point(placement.cell_idx).y,
                );
                (cell_layout, baseline)
            })
        };
        let (cell_size, baseline) = if reusable {
            let (cell_layout, baseline) = layout_cell(session);
            let cell_style = session.reader.style(placement.cell_idx);
            let cell_box_model = used_borders.map_or_else(
                || ResolvedBoxModel::new(cell_style, span_width),
                |borders| ResolvedBoxModel::new(cell_style, span_width).with_used_borders(borders),
            );
            let natural_border_height = session.natural_content_height(placement.cell_idx)
                + cell_box_model.vertical_padding_border();
            let has_visible_content =
                cell_has_visible_content(session, placement.cell_idx, &cell_layout.output);
            let cell_size = cell_layout.size;
            layouts.push(PreparedCellLayout::Retained(RetainedCellLayout {
                output: cell_layout.output,
                provisional_point,
                natural_border_height,
                has_visible_content,
            }));
            (cell_size, baseline)
        } else {
            let ((cell_layout, baseline), _) =
                crate::layout::with_isolated_measurement(session, layout_cell);
            layouts.push(PreparedCellLayout::Relayout {
                assign_final_height,
            });
            (cell_layout.size, baseline)
        };
        heights[index] = cell_size.height;
        baseline_offsets[index] = baseline;
    }

    let mut row_baselines = vec![None::<f64>; row_count];
    for (index, placement) in placements
        .iter()
        .enumerate()
        .filter(|(_, placement)| placement.rowspan == 1)
    {
        if cell_uses_baseline_alignment(session, placement.cell_idx)
            && let Some(cell_baseline) = baseline_offsets[index]
        {
            row_baselines[placement.row] = Some(
                row_baselines[placement.row]
                    .map_or(cell_baseline, |current| current.max(cell_baseline)),
            );
        }
    }
    PreparedTableCells {
        baseline_offsets,
        cell_heights: heights,
        row_baselines,
        layouts,
    }
}

/// Whether the cell subtree consumes the block-size basis suppressed during
/// intrinsic table measurement. Such a cell must be rerun after row sizing;
/// all other cells can retain and translate their first pass.
fn cell_content_uses_parent_block_basis(session: &LayoutEngine<'_, '_>, cell_idx: usize) -> bool {
    fn size_uses_parent_block_basis(size: PreferredSize) -> bool {
        size.percentage_dependent() || matches!(size, PreferredSize::Stretch)
    }

    fn children_depend(session: &LayoutEngine<'_, '_>, children: &Children) -> bool {
        match children {
            Children::Blocks(children) => children
                .iter()
                .any(|&child| box_depends(session, child as usize)),
            Children::InlineItems(range) => range.clone().any(|item_idx| {
                session
                    .text
                    .inline_item(item_idx as usize)
                    .is_some_and(|item| {
                        matches!(
                            item.kind,
                            crate::layout_model::InlineItemKind::AtomicBox { box_idx }
                                if box_depends(session, box_idx as usize)
                        )
                    })
            }),
            Children::Empty => false,
        }
    }

    fn box_depends(session: &LayoutEngine<'_, '_>, box_idx: usize) -> bool {
        let style = session.reader.style(box_idx);
        if size_uses_parent_block_basis(style.height())
            || size_uses_parent_block_basis(style.min_height())
            || size_uses_parent_block_basis(style.max_height())
        {
            return true;
        }
        match session.reader.box_layout_mode(box_idx) {
            Some(LayoutMode::Block(block)) => children_depend(session, &block.children),
            Some(LayoutMode::TableCell(cell)) => children_depend(session, &cell.children),
            Some(LayoutMode::Flex(container)) => container
                .children
                .iter()
                .any(|&child| box_depends(session, child as usize)),
            Some(LayoutMode::Grid(container)) => container
                .children
                .iter()
                .any(|&child| box_depends(session, child as usize)),
            _ => false,
        }
    }

    match session.reader.box_layout_mode(cell_idx) {
        Some(LayoutMode::TableCell(cell)) => children_depend(session, &cell.children),
        _ => true,
    }
}

pub(super) fn cell_has_restricted_percentage_height_descendant(
    session: &LayoutEngine<'_, '_>,
    cell_idx: usize,
) -> bool {
    fn children_contain(session: &LayoutEngine<'_, '_>, children: &Children) -> bool {
        match children {
            Children::Blocks(children) => children
                .iter()
                .any(|&child| box_contains(session, child as usize)),
            Children::InlineItems(range) => range.clone().any(|item_idx| {
                session
                    .text
                    .inline_item(item_idx as usize)
                    .is_some_and(|item| match item.kind {
                        crate::layout_model::InlineItemKind::AtomicBox { box_idx } => {
                            box_contains(session, box_idx as usize)
                        }
                        _ => false,
                    })
            }),
            Children::Empty => false,
        }
    }

    fn box_contains(session: &LayoutEngine<'_, '_>, box_idx: usize) -> bool {
        let style = session.reader.style(box_idx);
        if style.height().percentage_dependent() && style.overflow_y().clips() {
            return true;
        }
        match session.reader.box_layout_mode(box_idx) {
            Some(LayoutMode::Block(block)) => children_contain(session, &block.children),
            Some(LayoutMode::TableCell(cell)) => children_contain(session, &cell.children),
            Some(LayoutMode::Flex(container)) => container
                .children
                .iter()
                .any(|&child| box_contains(session, child as usize)),
            Some(LayoutMode::Grid(container)) => container
                .children
                .iter()
                .any(|&child| box_contains(session, child as usize)),
            _ => false,
        }
    }

    match session.reader.box_layout_mode(cell_idx) {
        Some(LayoutMode::TableCell(cell)) => children_contain(session, &cell.children),
        _ => false,
    }
}

pub(crate) fn layout_table_row(
    session: &mut LayoutEngine<'_, '_>,
    _row_box_idx: usize,
    cells: Vec<u32>,
    out_of_flow: Vec<u32>,
    content_pos: Point,
    content_width: f64,
    parent_content_height: Option<f64>,
) -> Size {
    let timing_started = session.start_timing();
    if cells.is_empty() {
        for child in out_of_flow {
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
                    session.layout_box(crate::layout::BoxLayoutRequest::table_cell(
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
    for &cell_idx in &cells {
        let mut size = session.geometry.size(cell_idx as usize);
        size.height = size.height.max(row_height);
        session.geometry.set_size(cell_idx as usize, size);
    }
    for child in out_of_flow {
        session.defer_absolute_box(child as usize, content_pos);
    }
    session.record_timing(|timings| timings.layout_table_row += timing_started.elapsed());
    Size::new(content_width.max(0.0), row_height)
}

pub(super) fn cell_vertical_offset(
    session: &LayoutEngine<'_, '_>,
    cell_idx: usize,
    natural_height: f64,
    span_height: f64,
    row_baseline: Option<f64>,
    cell_baseline: Option<f64>,
) -> f64 {
    let extra = (span_height - natural_height).max(0.0);
    match session.reader.style(cell_idx).vertical_align() {
        VerticalAlignValue::Top => 0.0,
        VerticalAlignValue::Bottom | VerticalAlignValue::TextBottom => extra,
        VerticalAlignValue::Middle => extra * 0.5,
        VerticalAlignValue::Baseline
        | VerticalAlignValue::Sub
        | VerticalAlignValue::Super
        | VerticalAlignValue::Length(_)
        | VerticalAlignValue::Percent(_)
        | VerticalAlignValue::Calc { .. }
        | VerticalAlignValue::TextTop => match (row_baseline, cell_baseline) {
            (Some(row), Some(cell)) => (row - cell).max(0.0),
            _ => 0.0,
        },
    }
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

pub(super) fn cell_has_visible_content(
    session: &LayoutEngine<'_, '_>,
    cell_idx: usize,
    output: &crate::layout::OutputRanges,
) -> bool {
    if output.has_lines_or_images() {
        return true;
    }
    match session.reader.box_layout_mode(cell_idx) {
        Some(LayoutMode::TableCell(cell)) => !matches!(cell.children, Children::Empty),
        _ => false,
    }
}
