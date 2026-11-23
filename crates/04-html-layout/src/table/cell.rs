//! Shared cell layout and content metrics used during preparation and publication.

use crate::{
    BoxLayoutRequest, BoxLayoutResult, LayoutEngine, OutputRanges, ResolvedBoxModel,
    UsedBorderInsets,
};
use html_engine_model::{Children, LayoutMode};
use kurbo::{Point, Size};

pub(super) struct RetainedCellLayout {
    pub(super) output: OutputRanges,
    pub(super) provisional_point: Point,
    pub(super) natural_border_height: f64,
    pub(super) has_visible_content: bool,
}

pub(super) fn layout_cell(
    engine: &mut LayoutEngine<'_, '_>,
    point: Point,
    request: BoxLayoutRequest,
) -> BoxLayoutResult {
    engine.geometry.set_point(request.box_idx(), point);
    engine.layout_box(request)
}

pub(super) fn retain_cell(
    engine: &mut LayoutEngine<'_, '_>,
    point: Point,
    request: BoxLayoutRequest,
    used_borders: Option<UsedBorderInsets>,
) -> (Size, Option<f64>, RetainedCellLayout) {
    let result = layout_cell(engine, point, request);
    let (natural_border_height, has_visible_content) = cell_content_metrics(
        engine,
        request.box_idx(),
        request.available_width,
        used_borders,
        &result,
    );
    (
        result.size,
        result.first_baseline,
        RetainedCellLayout {
            output: result.output,
            provisional_point: point,
            natural_border_height,
            has_visible_content,
        },
    )
}

fn cell_content_metrics(
    engine: &LayoutEngine<'_, '_>,
    cell_idx: usize,
    span_width: f64,
    used_borders: Option<UsedBorderInsets>,
    result: &BoxLayoutResult,
) -> (f64, bool) {
    let mut box_model = ResolvedBoxModel::new(engine.read.reader.style(cell_idx), span_width);
    if let Some(borders) = used_borders {
        box_model = box_model.with_used_borders(borders);
    }
    let natural_height = result.natural_content_size.height + box_model.vertical_padding_border();
    let has_visible_content = result.output.has_lines_or_images()
        || matches!(engine.read.reader.box_layout_mode(cell_idx), Some(LayoutMode::TableCell(cell))
            if !matches!(cell.children, Children::Empty));
    (natural_height, has_visible_content)
}
