//! Translate container styles; item dimensions live in the item module.
mod conversion;
mod item;

pub(super) use super::compatibility::grid_definite_inline_minimum_exceeds_track_limit;
use super::sizing::{ContainerFacts, SizingPass};
use super::tracks::{
    line_names as grid_line_names, template_track as grid_template_track,
    track_size as grid_track_size, used_length_percentage,
};
use super::{TaffyContainerKind, length_dimension};
use crate::LayoutEngine;
pub(super) use conversion::dimension;
use conversion::{
    content_alignment, direction, flex_direction, flex_wrap, grid_auto_flow, item_alignment,
};
pub(super) use item::taffy_item_style;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::{Dimension, Style};
use taffy::style::{
    Display as TaffyDisplay, GridTemplateArea as TaffyGridTemplateArea,
    GridTemplateAreas as TaffyGridTemplateAreas,
};

pub(super) fn taffy_container_style(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    pass: SizingPass,
    explicit_height: Option<f64>,
    min_height: Option<f64>,
    max_height: Option<f64>,
    container: ContainerFacts,
    floor_fixed_grid_column_maxima: bool,
) -> Style {
    let source = engine.read.reader.layout_style(box_idx);
    let used = engine.read.reader.style(box_idx);

    let inline_main = container.kind == TaffyContainerKind::Grid || container.flex_row();
    let mut style = Style {
        direction: direction(used.direction()),
        display: match container.kind {
            TaffyContainerKind::Flex => TaffyDisplay::Flex,
            TaffyContainerKind::Grid => TaffyDisplay::Grid,
        },
        size: TaffySize {
            width: length_dimension(pass.width()),
            height: explicit_height.map_or_else(Dimension::auto, length_dimension),
        },
        min_size: TaffySize {
            width: Dimension::auto(),
            height: min_height.map_or_else(Dimension::auto, length_dimension),
        },
        max_size: TaffySize {
            width: Dimension::auto(),
            height: max_height.map_or_else(Dimension::auto, length_dimension),
        },
        gap: TaffySize {
            width: used_length_percentage(used.column_gap(), pass.width()),
            height: used_length_percentage(used.row_gap(), pass.width()),
        },
        align_items: item_alignment(source.align_items, container.rtl && !inline_main),
        align_content: content_alignment(
            source.align_content,
            container.kind,
            false,
            container.rtl && !inline_main,
        ),
        justify_content: content_alignment(
            source.justify_content,
            container.kind,
            true,
            container.rtl && inline_main,
        ),
        flex_direction: flex_direction(source.flex_direction),
        flex_wrap: flex_wrap(source.flex_wrap),
        justify_items: item_alignment(source.justify_items, container.rtl),
        grid_template_rows: used
            .grid_template_rows()
            .map(|track| {
                grid_template_track(
                    engine.read.reader.styles(),
                    &track,
                    false,
                    explicit_height,
                    pass,
                )
            })
            .collect(),
        grid_template_columns: used
            .grid_template_columns()
            .map(|track| {
                grid_template_track(
                    engine.read.reader.styles(),
                    &track,
                    floor_fixed_grid_column_maxima,
                    pass.inline_basis(),
                    pass,
                )
            })
            .collect(),
        grid_template_row_names: grid_line_names(
            engine.read.reader.styles(),
            &source.grid_template_row_names,
        ),
        grid_template_column_names: grid_line_names(
            engine.read.reader.styles(),
            &source.grid_template_column_names,
        ),
        grid_template_areas: (source.grid_template_area_rows > 0
            && source.grid_template_area_columns > 0)
            .then(|| TaffyGridTemplateAreas {
                areas: source
                    .grid_template_areas
                    .iter()
                    .filter_map(|area| {
                        Some(TaffyGridTemplateArea {
                            name: engine.read.reader.styles().string(area.name)?.into(),
                            row_start: area.row_start,
                            row_end: area.row_end,
                            column_start: area.column_start,
                            column_end: area.column_end,
                        })
                    })
                    .collect(),
                row_count: source.grid_template_area_rows,
                column_count: source.grid_template_area_columns,
            }),
        grid_auto_rows: used
            .grid_auto_rows()
            .map(|track| grid_track_size(&track, false, explicit_height, pass))
            .collect(),
        grid_auto_columns: used
            .grid_auto_columns()
            .map(|track| {
                grid_track_size(
                    &track,
                    floor_fixed_grid_column_maxima,
                    pass.inline_basis(),
                    pass,
                )
            })
            .collect(),
        grid_auto_flow: grid_auto_flow(source.grid_auto_flow),
        ..Style::default()
    };
    if container.kind == TaffyContainerKind::Flex {
        style.justify_items = None;
    }
    style
}
