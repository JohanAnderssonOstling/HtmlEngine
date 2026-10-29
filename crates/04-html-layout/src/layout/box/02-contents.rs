//! Dispatch one box into its formatting context.
use super::formatting_context::{layout_flow_children, layout_inline_box, layout_taffy_container};
use super::{BoxLayoutRequest, ContentLayoutResult, LayoutEngine, ResolvedBoxSizing};
use crate::flex_grid::TaffyContainerKind;
use crate::layout_model::{BoxType, InlineItemKind, LayoutMode, ListItemMarker};
use html_style_model::{ListStylePosition, TextDirection, UsedPreferredSize as PreferredSize};
use kurbo::Point;

#[derive(Clone, Copy)]
struct ResolvedListMarker {
    marker: ListItemMarker,
    width: f64,
    gap: f64,
    first_content_line: usize,
}

struct BoxLayoutPlacement {
    box_point: Point,
    content_point: Point,
    marker: Option<ResolvedListMarker>,
}

pub(super) fn layout_box_contents(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
) -> ContentLayoutResult {
    let placement = resolve_box_placement(engine, resolved);
    let box_idx = resolved.request.box_idx;
    let isolates_floats = engine.reader.box_uses_float_context(box_idx);
    if isolates_floats {
        engine.floats.push_context();
    }
    let mut result = dispatch_contents(engine, resolved, &placement);
    if isolates_floats {
        let float_height = engine
            .floats
            .current_float_context()
            .map(|context| (context.max_bottom() - placement.content_point.y).max(0.0))
            .unwrap_or(0.0);
        result.size.height = result.size.height.max(float_height);
        engine.floats.pop_context();
    }
    result
}

fn resolve_box_placement(
    engine: &LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
) -> BoxLayoutPlacement {
    let box_idx = resolved.request.box_idx;
    let box_point = engine.geometry.point(box_idx);
    let content_point = Point::new(
        box_point.x + resolved.box_model.padding_left + resolved.box_model.border_left,
        box_point.y + resolved.box_model.padding_top + resolved.box_model.border_top,
    );
    let marker = engine.reader.list_marker(box_idx).map(|marker| {
        let width = measure_marker_width(engine, marker.marker_box as usize);
        ResolvedListMarker {
            marker,
            width,
            gap: 0.5 * engine.reader.style(box_idx).font_size() as f64,
            first_content_line: engine.fragments.state().line_output.lines.len(),
        }
    });
    BoxLayoutPlacement {
        box_point,
        content_point,
        marker,
    }
}

fn dispatch_contents(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    placement: &BoxLayoutPlacement,
) -> ContentLayoutResult {
    let request = resolved.request;
    let box_idx = request.box_idx;
    let inside_marker_indent = placement
        .marker
        .filter(|marker| matches!(marker.marker.position, ListStylePosition::Inside))
        .map(|marker| marker.width + marker.gap)
        .unwrap_or(0.0);
    let reader = engine.reader;
    let content_size = match reader
        .box_layout_mode(box_idx)
        .expect("layout box should exist")
    {
        LayoutMode::Block(block) => layout_flow_children(
            engine,
            box_idx,
            &block.children,
            placement.content_point,
            resolved.horizontal.content,
            request.available_width,
            resolved.vertical.descendant_basis,
            resolved.text_indent + inside_marker_indent,
        ),
        LayoutMode::Table(table) => {
            crate::table::layout_table(engine, resolved, table.clone(), placement.content_point)
                .into()
        }
        LayoutMode::TableRow(row) => crate::table::layout_table_row(
            engine,
            &row.cells,
            &row.out_of_flow,
            placement.content_point,
            resolved.horizontal.content,
            request.parent_content_height,
        )
        .into(),
        LayoutMode::TableCell(cell) => layout_flow_children(
            engine,
            box_idx,
            &cell.children,
            placement.content_point,
            resolved.horizontal.content,
            request.available_width,
            resolved.vertical.descendant_basis,
            resolved.text_indent,
        ),
        LayoutMode::Flex(container) => layout_taffy_contents(
            engine,
            resolved,
            placement.content_point,
            &container.children,
            TaffyContainerKind::Flex,
        ),
        LayoutMode::Grid(container) => layout_taffy_contents(
            engine,
            resolved,
            placement.content_point,
            &container.children,
            TaffyContainerKind::Grid,
        ),
        BoxType::Inline(range) | BoxType::Anonymous(range) => layout_inline_box(
            engine,
            box_idx,
            range.clone(),
            placement.content_point,
            resolved.horizontal.content,
            resolved.vertical.descendant_basis,
            request.first_line_indent,
        )
        .into(),
    };

    if let Some(marker) = placement.marker {
        place_list_marker(
            engine,
            marker,
            if engine.reader.style(box_idx).direction() == TextDirection::Rtl {
                placement.content_point.x
                    + resolved.horizontal.content
                    + resolved.box_model.padding_right
                    + resolved.box_model.border_right
            } else {
                placement.box_point.x
            },
            placement.content_point,
            resolved.horizontal.content,
        );
    }
    content_size
}

fn layout_taffy_contents(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
    content_point: Point,
    children: &[u32],
    kind: TaffyContainerKind,
) -> ContentLayoutResult {
    let request = resolved.request;
    let (min_height, max_height) = if kind == TaffyContainerKind::Flex
        && (resolved.vertical.min.is_intrinsic() || resolved.vertical.max.is_intrinsic())
    {
        // An intrinsic min/max block-size is a constraint on the flex
        // container's natural block-size. Measuring the final pass at the
        // authored height first would make `height: 0; min-height: max-content`
        // appear to have a zero intrinsic minimum (and similarly preserve an
        // oversized authored height through an intrinsic maximum).
        //
        // Probe an auto-height Flex formatting context in disposable output,
        // then feed that natural size back to Taffy as the root min/max
        // constraint. Keeping it as a constraint rather than an exact height
        // also preserves the definiteness rules used for descendant
        // percentage heights.
        let container_layout = engine.reader.layout_style(request.box_idx);
        let wrapped_column = matches!(
            container_layout.flex_direction,
            html_style_model::FlexDirection::Column
                | html_style_model::FlexDirection::ColumnReverse
        ) && !matches!(
            container_layout.flex_wrap,
            html_style_model::FlexWrap::NoWrap
        );
        let natural_height = if wrapped_column && resolved.vertical.min.is_intrinsic() {
            let mut maximum = 0.0f64;
            for &child in children {
                let child_idx = child as usize;
                if engine.reader.style(child_idx).position()
                    == html_style_model::PositionMode::Absolute
                {
                    continue;
                }
                let size = crate::layout::measure_box_isolated(
                    engine,
                    BoxLayoutRequest::normal(child_idx, resolved.horizontal.content.max(0.0), None),
                )
                .size_with_floats();
                let style = engine.reader.style(child_idx);
                let outer_height = size.height
                    + style.margin_top().resolve(resolved.horizontal.content)
                    + style.margin_bottom().resolve(resolved.horizontal.content);
                maximum = maximum.max(outer_height);
            }
            maximum
        } else {
            let natural = super::with_isolated_measurement(engine, |engine| {
                crate::flex_grid::layout(
                    engine,
                    request.box_idx,
                    children,
                    Point::ZERO,
                    resolved.horizontal.content,
                    None,
                    None,
                    None,
                    kind,
                )
            });
            natural.size.height.max(0.0)
        };
        (
            resolved.vertical.min.resolve(Some(natural_height)),
            resolved.vertical.max.resolve(Some(natural_height)),
        )
    } else {
        (
            resolved.vertical.min.resolve(None),
            resolved.vertical.max.resolve(None),
        )
    };
    layout_taffy_container(
        engine,
        request.box_idx,
        children,
        kind,
        content_point,
        resolved.horizontal.content,
        resolved.vertical.descendant_basis,
        min_height,
        max_height,
        resolved.is_float && matches!(resolved.horizontal.preferred, PreferredSize::Auto),
        request.available_width,
        resolved.horizontal.margin_padding + resolved.box_model.horizontal_border(),
        resolved.horizontal.min,
        resolved.horizontal.max,
        resolved.horizontal.border_box_inset,
    )
}

fn measure_marker_width(engine: &LayoutEngine<'_, '_>, marker_box: usize) -> f64 {
    let Some(LayoutMode::Inline(range)) = engine.reader.box_layout_mode(marker_box) else {
        return 0.0;
    };
    let mut width = 0.0;
    for run_idx in range.clone() {
        let Some(run) = engine.text.inline_item(run_idx as usize) else {
            continue;
        };
        if let InlineItemKind::Marker { glyphs } = &run.kind {
            for glyph_idx in glyphs.clone() {
                if let Some(glyph_id) = engine.text.glyph(glyph_idx) {
                    width += engine.text.text_advance(
                        glyph_idx as usize,
                        engine.text.glyph_metric(glyph_id).advance(),
                    ) as f64;
                }
            }
        }
    }
    width
}

fn place_list_marker(
    engine: &mut LayoutEngine<'_, '_>,
    marker: ResolvedListMarker,
    border_start_x: f64,
    content_pos: Point,
    content_width: f64,
) {
    let ResolvedListMarker {
        marker,
        width: marker_width,
        gap: marker_gap,
        first_content_line: first_content_line_idx,
    } = marker;
    let line_y = if first_content_line_idx < engine.fragments.state().line_output.lines.len() {
        engine.fragments.line_point(first_content_line_idx).y
    } else {
        content_pos.y
    };
    let rtl = engine.reader.style(marker.marker_box as usize).direction() == TextDirection::Rtl;
    let marker_x = match (marker.position, rtl) {
        (ListStylePosition::Outside, true) => border_start_x + marker_gap,
        (ListStylePosition::Outside, false) => border_start_x - marker_width - marker_gap,
        (ListStylePosition::Inside, true) => content_pos.x + content_width - marker_width,
        (ListStylePosition::Inside, false) => content_pos.x,
    };
    engine
        .geometry
        .set_point(marker.marker_box as usize, Point::new(marker_x, line_y));
    let available = if rtl {
        marker_width.max(1.0)
    } else {
        engine.config.viewport_width().max(marker_width + 1.0)
    };
    // Marker glyphs are positioned relative to the list-item border edge;
    // descendant or adjoining floats must not reflow that already-positioned
    // marker as though it were ordinary inline content.
    engine.floats.push_context();
    let _ = engine.layout_box(BoxLayoutRequest::normal(
        marker.marker_box as usize,
        available,
        None,
    ));
    engine.floats.pop_context();
}
