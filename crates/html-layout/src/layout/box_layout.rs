use crate::layout::LayoutEngine;
use crate::layout_model::{BoxType, InlineItemKind, LayoutMode, ListItemMarker};
use html_style_model::{BoxSizing, ListStylePosition, UsedPreferredSize as PreferredSize};
use kurbo::{Point, Size};
use std::time::Instant;

use super::box_constraints::{BoxLayoutRequest, constrain_content_width};
use super::box_positioning::finish_box_layout;
use super::box_sizing::ResolvedBoxSizing;
use super::formatting_context::{layout_flow_children, layout_inline_box, layout_taffy_container};
use crate::flex_grid::TaffyContainerKind;

#[must_use = "layout publishes subtree output ranges together with its size"]
pub(crate) struct BoxLayoutResult {
    pub(crate) size: Size,
    pub(crate) output: super::OutputRanges,
}

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

impl<'a, 'out> LayoutEngine<'a, 'out> {
    pub(crate) fn defer_absolute_box(&mut self, box_idx: usize, static_position: Point) {
        super::absolute_positioning::defer(self, box_idx, static_position);
    }

    pub(crate) fn layout_box(&mut self, request: BoxLayoutRequest) -> BoxLayoutResult {
        let resolved = self.resolve_box_sizing(request);
        self.layout_resolved_box(resolved)
    }

    pub(crate) fn layout_resolved_box(&mut self, resolved: ResolvedBoxSizing) -> BoxLayoutResult {
        let timing_started = Instant::now();
        let request = resolved.request;
        let output_cursor = self.fragments.output_cursor();
        let placement = resolve_box_placement(self, &resolved);
        // A formatting context owns descendant layout, including floats that
        // contribute to its automatic block size.
        let isolates_floats = self.reader.box_uses_float_context(request.box_idx);
        if isolates_floats {
            self.floats.push_context();
        }
        let mut content_size = layout_box_contents(self, &resolved, &placement);
        if isolates_floats {
            let float_height = self.floats.current_float_context().map(|context| (context.max_bottom() - placement.content_point.y).max(0.0)).unwrap_or(0.0);
            content_size.height = content_size.height.max(float_height);
            self.floats.pop_context();
        }
        self.margins.record_natural_content_height(request.box_idx, content_size.height);
        let size = resolve_used_border_size(&resolved, content_size);
        finish_box_layout(self, request.box_idx, size, output_cursor, request.available_width, request.parent_content_height);
        let output = self.fragments.output_since(output_cursor);
        self.record_timing(|t| t.layout_box_total += timing_started.elapsed());
        BoxLayoutResult { size, output }
    }
}

fn resolve_box_placement(engine: &LayoutEngine<'_, '_>, resolved: &ResolvedBoxSizing) -> BoxLayoutPlacement {
    let box_idx = resolved.request.box_idx;
    let box_point = engine.geometry.point(box_idx);
    let content_point = Point::new(box_point.x + resolved.box_model.padding_left + resolved.box_model.border_left, box_point.y + resolved.box_model.padding_top + resolved.box_model.border_top);
    let marker = engine.reader.list_marker(box_idx).map(|marker| {
        let width = measure_marker_width(engine, marker.marker_box as usize);
        ResolvedListMarker { marker, width, gap: 0.5 * engine.reader.style(box_idx).font_size() as f64, first_content_line: engine.fragments.state().line_output.lines.len() }
    });
    BoxLayoutPlacement { box_point, content_point, marker }
}

fn layout_box_contents(engine: &mut LayoutEngine<'_, '_>, resolved: &ResolvedBoxSizing, placement: &BoxLayoutPlacement) -> Size {
    let request = resolved.request;
    let box_idx = request.box_idx;
    let inside_marker_indent = placement.marker.filter(|marker| matches!(marker.marker.position, ListStylePosition::Inside)).map(|marker| marker.width + marker.gap).unwrap_or(0.0);
    let content_size = match resolved.layout_mode.clone() {
        LayoutMode::Block(block) => {
            layout_flow_children(engine, box_idx, &block.children, placement.content_point, resolved.horizontal.content, request.available_width, resolved.vertical.descendant_basis, resolved.text_indent + inside_marker_indent)
        }
        LayoutMode::Table(table) => layout_table_contents(engine, resolved, placement.content_point, table.rows),
        LayoutMode::TableRow(row) => crate::table::layout_table_row(engine, box_idx, row.cells, row.out_of_flow, placement.content_point, resolved.horizontal.content, request.parent_content_height),
        LayoutMode::TableCell(cell) => layout_flow_children(engine, box_idx, &cell.children, placement.content_point, resolved.horizontal.content, request.available_width, resolved.vertical.descendant_basis, resolved.text_indent),
        LayoutMode::Flex(container) => layout_taffy_contents(engine, resolved, placement.content_point, &container.children, TaffyContainerKind::Flex),
        LayoutMode::Grid(container) => layout_taffy_contents(engine, resolved, placement.content_point, &container.children, TaffyContainerKind::Grid),
        BoxType::Inline(range) | BoxType::Anonymous(range) => layout_inline_box(engine, box_idx, range, placement.content_point, resolved.horizontal.content, request.parent_content_height, request.first_line_indent),
    };

    if let Some(marker) = placement.marker {
        place_list_marker(engine, marker.marker, marker.width, marker.gap, placement.box_point.x, placement.content_point, marker.first_content_line);
    }
    content_size
}

fn layout_table_contents(engine: &mut LayoutEngine<'_, '_>, resolved: &ResolvedBoxSizing, content_point: Point, rows: Vec<u32>) -> Size {
    let request = resolved.request;
    let uses_auto_intrinsic_width = request.assigned_border_size.and_then(|assigned| assigned.width).is_none() && matches!(resolved.horizontal.preferred, PreferredSize::Auto);
    let table_width = if uses_auto_intrinsic_width {
        let (intrinsic_min, mut intrinsic_max) = super::intrinsic_sizing::box_content_intrinsic_widths(engine, request.box_idx);
        if super::intrinsic_sizing::contains_full_width_percentage_table(engine, request.box_idx) {
            intrinsic_max = intrinsic_max.max(resolved.horizontal.content);
        }
        constrain_content_width(intrinsic_max.min(resolved.horizontal.content.max(intrinsic_min)), resolved.horizontal.min, resolved.horizontal.max, request.available_width, resolved.horizontal.border_box_inset)
    } else {
        resolved.horizontal.content
    };
    // Authored table height is a minimum for row distribution, after its
    // outer min/max constraints have been applied.
    let row_distribution_height = if request.assigned_border_size.and_then(|assigned| assigned.height).is_none() {
        resolved
            .vertical
            .explicit
            .map(|height| {
                let height = resolved.vertical.max.map_or(height, |maximum| height.min(maximum));
                resolved.vertical.min.map_or(height, |minimum| height.max(minimum))
            })
            .or(resolved.vertical.min)
    } else {
        resolved.vertical.explicit
    };
    crate::table::layout_table(engine, request.box_idx, rows, content_point, table_width, request.parent_content_height, row_distribution_height, uses_auto_intrinsic_width)
}

fn layout_taffy_contents(engine: &mut LayoutEngine<'_, '_>, resolved: &ResolvedBoxSizing, content_point: Point, children: &[u32], kind: TaffyContainerKind) -> Size {
    let request = resolved.request;
    layout_taffy_container(
        engine,
        request.box_idx,
        children,
        kind,
        content_point,
        resolved.horizontal.content,
        resolved.vertical.descendant_basis,
        resolved.vertical.min,
        resolved.vertical.max,
        resolved.is_float && matches!(resolved.horizontal.preferred, PreferredSize::Auto),
        request.available_width,
        resolved.horizontal.margin_padding + resolved.box_model.horizontal_border(),
        resolved.horizontal.min,
        resolved.horizontal.max,
        resolved.horizontal.border_box_inset,
    )
}

fn resolve_used_border_size(resolved: &ResolvedBoxSizing, content_size: Size) -> Size {
    let request = resolved.request;
    let horizontal_padding = resolved.box_model.horizontal_padding();
    let vertical_padding = resolved.box_model.vertical_padding();
    let horizontal_border = resolved.box_model.horizontal_border();
    let vertical_border = resolved.box_model.vertical_border();
    let table_height_is_minimum = matches!(resolved.layout_mode, LayoutMode::Table(_) | LayoutMode::TableRow(_) | LayoutMode::TableCell(_));
    let ratio_auto_height = resolved.used_aspect_ratio.map(|ratio| match resolved.box_sizing {
        BoxSizing::ContentBox => resolved.horizontal.content / ratio,
        BoxSizing::BorderBox => ((resolved.horizontal.content + horizontal_padding + horizontal_border) / ratio - vertical_padding - vertical_border).max(0.0),
    });
    let mut content_height = match (resolved.replaced_size, resolved.vertical.explicit) {
        (Some(size), _) => size.height,
        (None, Some(explicit)) if table_height_is_minimum => content_size.height.max(explicit),
        (None, Some(explicit)) => explicit,
        (None, None) => ratio_auto_height.unwrap_or(content_size.height),
    };
    if resolved.replaced_size.is_none() {
        if let Some(maximum) = resolved.vertical.max {
            content_height = content_height.min(maximum);
        }
        if let Some(minimum) = resolved.vertical.min {
            content_height = content_height.max(minimum);
        }
    }
    content_height = content_height.max(0.0);

    let used_content_width = match (resolved.replaced_size, resolved.horizontal.preferred) {
        (Some(size), _) => size.width,
        (None, PreferredSize::Auto) if resolved.fills_available_width => resolved.horizontal.content,
        // Overflow does not enlarge a shrink-to-fit box. In particular, an
        // unbreakable text run may paint beyond a float constrained by
        // max-width while the float's used border width stays constrained.
        (None, PreferredSize::Auto) if resolved.horizontal.shrink_to_fit => resolved.horizontal.content,
        (None, PreferredSize::Auto) => content_size.width,
        (None, _) => resolved.horizontal.content,
    };
    let natural_width = used_content_width + horizontal_padding + horizontal_border;
    let natural_height = content_height + vertical_padding + vertical_border;
    request.assigned_border_size.map(|assigned| Size::new(assigned.width.unwrap_or(natural_width), assigned.height.unwrap_or(natural_height))).unwrap_or_else(|| Size::new(natural_width, natural_height))
}

fn measure_marker_width(engine: &LayoutEngine<'_, '_>, marker_box: usize) -> f64 {
    let Some(LayoutMode::Inline(range)) = engine.reader.box_layout_mode(marker_box) else {
        return 0.0;
    };
    let mut width = 0.0;
    for run_idx in range.clone() {
        let Some(run) = engine.text.inline_item(run_idx as usize) else { continue };
        if let InlineItemKind::Marker { glyphs } = &run.kind {
            for glyph_idx in glyphs.clone() {
                if let Some(glyph_id) = engine.text.glyph(glyph_idx) {
                    width += engine.text.text_advance(glyph_idx as usize, engine.text.glyph_metric(glyph_id).advance()) as f64;
                }
            }
        }
    }
    width
}

fn place_list_marker(engine: &mut LayoutEngine<'_, '_>, marker: ListItemMarker, marker_width: f64, marker_gap: f64, border_start_x: f64, content_pos: Point, first_content_line_idx: usize) {
    let line_y = engine.fragments.state().line_output.lines.get(first_content_line_idx).map(|line| line.point.y).unwrap_or(content_pos.y);
    let marker_x = match marker.position {
        ListStylePosition::Outside => border_start_x - marker_width - marker_gap,
        ListStylePosition::Inside => content_pos.x,
    };
    engine.geometry.set_point(marker.marker_box as usize, Point::new(marker_x, line_y));
    let available = engine.config.viewport_width().max(marker_width + 1.0);
    // Marker glyphs are positioned relative to the list-item border edge;
    // descendant or adjoining floats must not reflow that already-positioned
    // marker as though it were ordinary inline content.
    engine.floats.push_context();
    let _ = engine.layout_box(BoxLayoutRequest::normal(marker.marker_box as usize, available, None));
    engine.floats.pop_context();
}

#[cfg(test)]
#[path = "box_layout_tests.rs"]
mod tests;
