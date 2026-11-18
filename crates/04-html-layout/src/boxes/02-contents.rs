//! Dispatch one box into its formatting context.
use crate::block::layout_flow_children;
use crate::flex_grid::{TaffyContainerKind, layout_container};
use crate::inline::{ResolvedTextIndent, layout_inline_box};
use crate::{BoxLayoutRequest, ContentLayoutResult, LayoutEngine, ResolvedBoxSizing};
use html_engine_model::{InlineItemKind, LayoutMode, ListItemMarker};
use html_style_model::{ListStylePosition, TextDirection};
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

pub(crate) fn layout_box_contents(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
) -> ContentLayoutResult {
    let placement = resolve_box_placement(engine, resolved);
    if resolved.isolates_floats {
        engine.scratch.floats.push_context();
    }
    let mut result = dispatch_contents(engine, resolved, &placement);
    if let Some(marker) = placement.marker {
        place_list_marker(
            engine,
            marker,
            if engine.read.reader.style(resolved.box_idx()).direction() == TextDirection::Rtl {
                placement.content_point.x
                    + resolved.horizontal.content
                    + f64::from(resolved.box_model.padding_right + resolved.box_model.border_right)
            } else {
                placement.box_point.x
            },
            placement.content_point,
            resolved.horizontal.content,
        );
    }
    if resolved.isolates_floats {
        let float_height = (engine.scratch.floats.current_float_context().max_bottom()
            - placement.content_point.y)
            .max(0.0);
        result.size.height = result.size.height.max(float_height);
        engine.scratch.floats.pop_context();
    }
    result
}

fn resolve_box_placement(
    engine: &LayoutEngine<'_, '_>,
    resolved: &ResolvedBoxSizing,
) -> BoxLayoutPlacement {
    let box_idx = resolved.request.box_idx;
    let box_point = engine.geometry.point(box_idx);
    let insets = resolved.box_model.padding_border_insets();
    let content_point = Point::new(box_point.x + insets.left, box_point.y + insets.top);
    let marker = engine.read.reader.list_marker(box_idx).map(|marker| {
        let width = measure_marker_width(engine, marker.marker_box as usize);
        ResolvedListMarker {
            marker,
            width,
            gap: 0.5 * engine.read.reader.style(box_idx).font_size() as f64,
            first_content_line: engine.fragments.line_len(),
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
    let reader = engine.read.reader;
    match reader
        .box_layout_mode(box_idx)
        .expect("layout box should exist")
    {
        LayoutMode::Block(block) => layout_flow_children(
            engine,
            resolved,
            block,
            placement.content_point,
            resolved.text_indent + inside_marker_indent,
        ),
        LayoutMode::Table(_) => {
            let table = reader.box_table(box_idx).expect("table payload");
            crate::table::layout_table(engine, resolved, table, placement.content_point)
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
            resolved,
            &cell.children,
            placement.content_point,
            resolved.text_indent,
        ),
        LayoutMode::Flex(container) => layout_container(
            engine,
            resolved,
            placement.content_point,
            reader.layout_tree().child_slice(container),
            TaffyContainerKind::Flex,
        ),
        LayoutMode::Grid(container) => layout_container(
            engine,
            resolved,
            placement.content_point,
            reader.layout_tree().child_slice(container),
            TaffyContainerKind::Grid,
        ),
        LayoutMode::Inline(range) | LayoutMode::Anonymous(range) => layout_inline_box(
            engine,
            resolved,
            range.clone(),
            placement.content_point,
            ResolvedTextIndent {
                amount: if engine
                    .read
                    .reader
                    .get_parent(box_idx)
                    .is_some_and(|parent| engine.read.reader.is_flex_grid_box(parent))
                {
                    resolved.text_indent
                } else {
                    request.first_line_indent
                },
                hanging: false,
                each_line: false,
                first_line: true,
            },
        )
        .into(),
    }
}

fn measure_marker_width(engine: &LayoutEngine<'_, '_>, marker_box: usize) -> f64 {
    let Some(LayoutMode::Inline(range)) = engine.read.reader.box_layout_mode(marker_box) else {
        return 0.0;
    };
    let mut width = 0.0;
    for run_idx in range.clone() {
        let Some(run) = engine.read.text.inline_item(run_idx as usize) else {
            continue;
        };
        if let InlineItemKind::Marker { glyphs } = &run.kind {
            for glyph_idx in glyphs.clone() {
                if let Some(glyph_id) = engine.read.text.glyph(glyph_idx) {
                    width += engine.read.text.text_advance(
                        glyph_idx as usize,
                        engine.read.text.glyph_metric(glyph_id).advance(),
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
    let line_y = if first_content_line_idx < engine.fragments.line_len() {
        engine.fragments.line_point(first_content_line_idx).y
    } else {
        content_pos.y
    };
    let rtl = engine
        .read
        .reader
        .style(marker.marker_box as usize)
        .direction()
        == TextDirection::Rtl;
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
        engine
            .read
            .constraints
            .viewport_width()
            .max(marker_width + 1.0)
    };
    // Marker glyphs are positioned relative to the list-item border edge;
    // descendant or adjoining floats must not reflow that already-positioned
    // marker as though it were ordinary inline content.
    engine.scratch.floats.push_context();
    let _ = engine.layout_box(BoxLayoutRequest::normal(
        marker.marker_box as usize,
        available,
        None,
    ));
    engine.scratch.floats.pop_context();
}
