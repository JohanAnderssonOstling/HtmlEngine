use crate::layout::LayoutEngine;
use crate::layout::replaced::{ReplacedSizeInput, resolve_replaced_content_size};
use crate::layout_model::LayoutMode;
use html_style_model::{BorderCollapseMode, BoxSizing, Float, PositionMode, UsedPreferredSize as PreferredSize, UsedStyleView};
use kurbo::Size;
use std::time::Instant;

use super::box_constraints::{BoxLayoutRequest, constrain_content_width, resolve_block_margin_left, resolve_content_width, resolve_vertical_min_size, resolve_vertical_size};
use super::box_model::{ResolvedBoxModel, UsedBorderInsets};

#[derive(Clone, Copy)]
pub(super) struct ResolvedHorizontalSizing {
    pub(super) preferred: PreferredSize,
    pub(super) min: PreferredSize,
    pub(super) max: PreferredSize,
    pub(super) content: f64,
    pub(super) margin_padding: f64,
    pub(super) border_box_inset: f64,
    suppresses_margins: bool,
    pub(super) shrink_to_fit: bool,
}

#[derive(Clone, Copy)]
pub(super) struct ResolvedVerticalSizing {
    authored_explicit: Option<f64>,
    pub(super) explicit: Option<f64>,
    pub(super) descendant_basis: Option<f64>,
    pub(super) min: Option<f64>,
    pub(super) max: Option<f64>,
}

#[derive(Clone, Copy)]
struct ResolvedWidthConstraints {
    preferred: PreferredSize,
    min: PreferredSize,
    max: PreferredSize,
    horizontal_margin: f64,
    margin_padding: f64,
    border_box_inset: f64,
    available_content: f64,
    smart_width: bool,
}

#[derive(Clone, Copy)]
struct ResolvedIntrinsicWidths {
    measured: Option<(f64, f64)>,
    shrink_to_fit: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct ResolvedHorizontalMargins {
    pub(crate) left: f64,
    pub(crate) total: f64,
}

/// Geometry-independent used sizing for one box.
///
/// A parent formatting context may inspect this value to place the box, then
/// pass the same value to `layout_resolved_box`; sizing is never guessed or
/// repeated by the parent.
#[derive(Clone)]
pub(crate) struct ResolvedBoxSizing {
    pub(super) request: BoxLayoutRequest,
    pub(super) layout_mode: LayoutMode,
    pub(super) box_model: ResolvedBoxModel,
    pub(super) box_sizing: BoxSizing,
    pub(super) horizontal: ResolvedHorizontalSizing,
    pub(super) vertical: ResolvedVerticalSizing,
    pub(super) used_aspect_ratio: Option<f64>,
    pub(super) replaced_size: Option<Size>,
    pub(super) is_float: bool,
    pub(super) fills_available_width: bool,
    horizontal_margins: ResolvedHorizontalMargins,
    pub(super) text_indent: f64,
}

impl ResolvedBoxSizing {
    pub(crate) fn box_idx(&self) -> usize {
        self.request.box_idx
    }

    pub(crate) fn available_width(&self) -> f64 {
        self.request.available_width
    }

    pub(crate) fn horizontal_margins(&self) -> ResolvedHorizontalMargins {
        self.horizontal_margins
    }

    pub(crate) fn with_first_line_indent(mut self, first_line_indent: f64) -> Self {
        self.request.first_line_indent = first_line_indent;
        self
    }
}

impl<'a, 'out> LayoutEngine<'a, 'out> {
    pub(crate) fn resolve_box_sizing(&mut self, request: BoxLayoutRequest) -> ResolvedBoxSizing {
        let timing_started = Instant::now();
        let isolates_floats = self.reader.box_uses_float_context(request.box_idx);
        if isolates_floats {
            self.floats.push_context();
        }
        let resolved = resolve_box_sizing(self, request);
        if isolates_floats {
            self.floats.pop_context();
        }
        self.record_timing(|timings| timings.layout_box_total += timing_started.elapsed());
        resolved
    }
}

fn resolve_box_sizing(engine: &LayoutEngine<'_, '_>, request: BoxLayoutRequest) -> ResolvedBoxSizing {
    let box_idx = request.box_idx;
    let style = engine.reader.style(box_idx);
    let box_model = resolve_box_model(engine, request, style);
    let is_float = matches!(style.float(), Float::Left | Float::Right);
    let replaced_intrinsic = engine.replaced.intrinsic_size(&engine.reader, box_idx);
    let is_replaced = replaced_intrinsic.is_some();
    let authored_aspect_ratio = style.aspect_ratio();
    let intrinsic_aspect_ratio = replaced_intrinsic.and_then(|size| engine.replaced.intrinsic_ratio(&engine.reader, box_idx, size));
    let used_aspect_ratio = if authored_aspect_ratio.uses_intrinsic() { intrinsic_aspect_ratio.or_else(|| authored_aspect_ratio.preferred().map(f64::from)) } else { authored_aspect_ratio.preferred().map(f64::from) };
    let vertical = resolve_vertical_sizing(style, request, box_model);
    let (horizontal, replaced_size) = resolve_horizontal_sizing(engine, style, request, box_model, vertical, replaced_intrinsic, used_aspect_ratio, is_float);

    let layout_mode = engine.reader.box_layout_mode(box_idx).expect("layout box should exist").clone();
    let fills_available_width = !is_replaced && !is_float && (engine.reader.is_block_box(box_idx) || engine.reader.is_anonymous_box(box_idx) || matches!(style.display(), html_style_model::Display::Flex | html_style_model::Display::Grid));
    let horizontal_margins = resolve_horizontal_margins(engine, style, request, box_model, horizontal, replaced_size, fills_available_width, is_float);

    ResolvedBoxSizing {
        request,
        layout_mode,
        box_model,
        box_sizing: style.box_sizing(),
        horizontal,
        vertical,
        used_aspect_ratio,
        replaced_size,
        is_float,
        fills_available_width,
        horizontal_margins,
        text_indent: style.text_indent().resolve(horizontal.content),
    }
}

fn resolve_horizontal_margins(
    engine: &LayoutEngine<'_, '_>, style: UsedStyleView<'_>, request: BoxLayoutRequest, box_model: ResolvedBoxModel, horizontal: ResolvedHorizontalSizing, replaced_size: Option<Size>, fills_available_width: bool, is_float: bool,
) -> ResolvedHorizontalMargins {
    if horizontal.suppresses_margins {
        return ResolvedHorizontalMargins { left: 0.0, total: 0.0 };
    }

    let authored_left = style.margin_left().resolve(request.available_width);
    let border_width = resolved_border_width(box_model, horizontal, replaced_size, fills_available_width);
    let left = if is_float {
        authored_left
    } else if let Some(border_width) = border_width {
        let containing_direction = engine.reader.get_parent(request.box_idx).map(|parent_idx| engine.reader.style(parent_idx).direction()).unwrap_or(html_style_model::TextDirection::Ltr);
        resolve_block_margin_left(style, containing_direction, request.available_width, border_width)
    } else {
        authored_left
    };
    ResolvedHorizontalMargins { left, total: box_model.horizontal_margin() }
}

fn resolved_border_width(box_model: ResolvedBoxModel, horizontal: ResolvedHorizontalSizing, replaced_size: Option<Size>, fills_available_width: bool) -> Option<f64> {
    let content_width = match (replaced_size, horizontal.preferred) {
        (Some(size), _) => size.width,
        (None, PreferredSize::Auto) if fills_available_width => horizontal.content,
        (None, PreferredSize::Auto) => return None,
        (None, _) => horizontal.content,
    };
    Some(content_width + box_model.horizontal_padding() + box_model.horizontal_border())
}

fn resolve_box_model(engine: &LayoutEngine<'_, '_>, request: BoxLayoutRequest, style: UsedStyleView<'_>) -> ResolvedBoxModel {
    let mut box_model = ResolvedBoxModel::new(style, request.available_width);
    let collapsed_table =
        (engine.reader.is_table_box(request.box_idx) || matches!(style.display(), html_style_model::Display::Table | html_style_model::Display::InlineTable)) && matches!(style.border_collapse(), BorderCollapseMode::Collapse);
    if collapsed_table {
        // Collapsed table borders belong to the resolved edge grid, and table
        // padding has no used value in this model.
        box_model = box_model.without_padding();
    }
    if let Some(borders) = request.used_borders {
        box_model = box_model.with_used_borders(borders);
    } else if collapsed_table {
        box_model = box_model.with_used_borders(UsedBorderInsets::default());
    }
    box_model
}

fn resolve_vertical_sizing(style: UsedStyleView<'_>, request: BoxLayoutRequest, box_model: ResolvedBoxModel) -> ResolvedVerticalSizing {
    let padding_border = box_model.vertical_padding() + box_model.vertical_border();
    let border_box_inset = match style.box_sizing() {
        BoxSizing::ContentBox => 0.0,
        BoxSizing::BorderBox => padding_border,
    };
    let authored_explicit = resolve_vertical_size(style.height(), request.parent_content_height, border_box_inset);
    let assigned_content = request.assigned_border_size.and_then(|size| size.height.map(|height| (height - padding_border).max(0.0)));
    let explicit = assigned_content.or(authored_explicit);
    let min = resolve_vertical_min_size(style.min_height(), request.parent_content_height, border_box_inset);
    let max = resolve_vertical_size(style.max_height(), request.parent_content_height, border_box_inset).map(|maximum| maximum.max(min.unwrap_or(0.0)));
    let descendant_basis = match (request.assigned_border_size, assigned_content) {
        (Some(assigned), Some(height)) if assigned.height_is_definite => Some(height),
        _ => authored_explicit.map(|height| {
            let height = max.map_or(height, |maximum| height.min(maximum));
            min.map_or(height, |minimum| height.max(minimum))
        }),
    };
    ResolvedVerticalSizing { authored_explicit, explicit, descendant_basis, min, max }
}

fn resolve_horizontal_sizing(
    engine: &LayoutEngine<'_, '_>, style: UsedStyleView<'_>, request: BoxLayoutRequest, box_model: ResolvedBoxModel, vertical: ResolvedVerticalSizing, replaced_intrinsic: Option<Size>, aspect_ratio: Option<f64>, is_float: bool,
) -> (ResolvedHorizontalSizing, Option<Size>) {
    let intrinsic_widths = resolve_intrinsic_widths(engine, style, request, vertical, replaced_intrinsic, is_float);
    let constraints = resolve_width_constraints(engine, style, request, box_model, replaced_intrinsic, intrinsic_widths.measured);
    let replaced_size = resolve_replaced_size(style, request, box_model, constraints, replaced_intrinsic, aspect_ratio);
    let content = resolve_used_content_width(request, box_model, constraints, vertical, replaced_intrinsic, replaced_size, intrinsic_widths);
    (
        ResolvedHorizontalSizing {
            preferred: constraints.preferred,
            min: constraints.min,
            max: constraints.max,
            content,
            margin_padding: constraints.margin_padding,
            border_box_inset: constraints.border_box_inset,
            suppresses_margins: constraints.smart_width,
            shrink_to_fit: intrinsic_widths.shrink_to_fit,
        },
        replaced_size,
    )
}

fn resolve_intrinsic_widths(engine: &LayoutEngine<'_, '_>, style: UsedStyleView<'_>, request: BoxLayoutRequest, vertical: ResolvedVerticalSizing, replaced_intrinsic: Option<Size>, is_float: bool) -> ResolvedIntrinsicWidths {
    let is_table_cell = engine.reader.is_table_cell_box(request.box_idx);
    let preferred = if is_table_cell { PreferredSize::Auto } else { style.width() };
    let min = if is_table_cell { PreferredSize::Auto } else { style.min_width() };
    let max = if is_table_cell { PreferredSize::Auto } else { style.max_width() };
    let uses_intrinsic_keyword = replaced_intrinsic.is_none() && [preferred, min, max].into_iter().any(|size| matches!(size, PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent));
    let is_absolute_shrink_to_fit = style.position() == PositionMode::Absolute && !(style.inset_left().is_some() && style.inset_right().is_some());
    let shrink_to_fit = replaced_intrinsic.is_none() && request.assigned_border_size.and_then(|assigned| assigned.width).is_none() && (is_float || is_absolute_shrink_to_fit) && matches!(preferred, PreferredSize::Auto);
    let mut measured = (uses_intrinsic_keyword || shrink_to_fit).then(|| super::intrinsic_sizing::box_content_intrinsic_widths(engine, request.box_idx));
    // A definite float height makes a percentage-height replaced child
    // definite while the float's shrink-to-fit width is being measured. Its
    // intrinsic aspect ratio therefore contributes the transferred width,
    // rather than the child's unscaled intrinsic width.
    if shrink_to_fit && let Some(transferred_width) = vertical.authored_explicit.and_then(|height| engine.replaced.percentage_height_width(&engine.reader, request.box_idx, height)) {
        measured = Some((transferred_width, transferred_width));
    }
    ResolvedIntrinsicWidths { measured, shrink_to_fit }
}

fn resolve_width_constraints(
    engine: &LayoutEngine<'_, '_>, style: UsedStyleView<'_>, request: BoxLayoutRequest, box_model: ResolvedBoxModel, replaced_intrinsic: Option<Size>, intrinsic_widths: Option<(f64, f64)>,
) -> ResolvedWidthConstraints {
    let is_table_cell = engine.reader.is_table_cell_box(request.box_idx);
    let authored_width = if is_table_cell { PreferredSize::Auto } else { style.width() };
    let mut min = if is_table_cell { PreferredSize::Auto } else { style.min_width() };
    let mut max = if is_table_cell { PreferredSize::Auto } else { style.max_width() };
    // Table-cell margins compute normally but do not participate in its used
    // table geometry.
    let horizontal_margin = if is_table_cell { 0.0 } else { box_model.horizontal_margin() };
    let margin_padding = horizontal_margin + box_model.horizontal_padding();
    let horizontal_padding_border = box_model.horizontal_padding() + box_model.horizontal_border();
    let smart_width = replaced_intrinsic.and_then(|intrinsic| {
        engine.replaced.smart_standalone_image_width(
            &engine.reader,
            engine.config.image_sizing_policy(),
            request.box_idx,
            intrinsic,
            request.available_width,
            engine.config.viewport_height(),
            horizontal_padding_border,
            box_model.vertical_margin() + box_model.vertical_padding() + box_model.vertical_border(),
            request.assigned_border_size.and_then(|assigned| assigned.width).is_none() && engine.reader.is_only_block_child(request.box_idx),
        )
    });
    let mut preferred = smart_width.unwrap_or(authored_width);
    let border_box_inset = match style.box_sizing() {
        BoxSizing::ContentBox => 0.0,
        BoxSizing::BorderBox => horizontal_padding_border,
    };
    let auto_width_limit = request.auto_width_limit.unwrap_or(request.available_width);
    let available_content = (auto_width_limit - margin_padding - box_model.horizontal_border()).max(0.0);
    let has_intrinsic_width = |size| matches!(size, PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent);
    if replaced_intrinsic.is_none() && (has_intrinsic_width(preferred) || has_intrinsic_width(min) || has_intrinsic_width(max)) {
        let (intrinsic_min, intrinsic_max) = intrinsic_widths.expect("intrinsic width keywords require measured intrinsic widths");
        let resolve_intrinsic = |size| {
            let content = match size {
                PreferredSize::MinContent => intrinsic_min,
                PreferredSize::MaxContent => intrinsic_max,
                PreferredSize::FitContent => intrinsic_max.min(available_content.max(intrinsic_min)),
                PreferredSize::Stretch => unreachable!("stretch is not an intrinsic width"),
                _ => return size,
            };
            PreferredSize::Px((content + border_box_inset) as f32)
        };
        preferred = resolve_intrinsic(preferred);
        min = resolve_intrinsic(min);
        max = resolve_intrinsic(max);
    }
    ResolvedWidthConstraints { preferred, min, max, horizontal_margin, margin_padding, border_box_inset, available_content, smart_width: smart_width.is_some() }
}

fn resolve_replaced_size(style: UsedStyleView<'_>, request: BoxLayoutRequest, box_model: ResolvedBoxModel, constraints: ResolvedWidthConstraints, intrinsic: Option<Size>, aspect_ratio: Option<f64>) -> Option<Size> {
    intrinsic.filter(|_| request.assigned_border_size.is_none()).map(|intrinsic| {
        resolve_replaced_content_size(ReplacedSizeInput {
            intrinsic,
            aspect_ratio,
            width: constraints.preferred,
            height: style.height(),
            min_width: if constraints.smart_width { PreferredSize::Auto } else { constraints.min },
            min_height: style.min_height(),
            max_width: if constraints.smart_width { PreferredSize::Auto } else { constraints.max },
            max_height: style.max_height(),
            available_width: request.available_width,
            available_height: request.parent_content_height,
            horizontal_margin: if constraints.smart_width { 0.0 } else { constraints.horizontal_margin },
            vertical_margin: box_model.vertical_margin(),
            horizontal_padding_border: box_model.horizontal_padding() + box_model.horizontal_border(),
            vertical_padding_border: box_model.vertical_padding() + box_model.vertical_border(),
            box_sizing: style.box_sizing(),
        })
    })
}

fn resolve_used_content_width(
    request: BoxLayoutRequest, box_model: ResolvedBoxModel, constraints: ResolvedWidthConstraints, vertical: ResolvedVerticalSizing, replaced_intrinsic: Option<Size>, replaced_size: Option<Size>, intrinsic_widths: ResolvedIntrinsicWidths,
) -> f64 {
    let replaced_auto_width =
        replaced_intrinsic.map(|intrinsic| vertical.authored_explicit.and_then(|height| (intrinsic.height > 0.0).then_some(height * intrinsic.width / intrinsic.height)).unwrap_or(intrinsic.width).min(constraints.available_content));
    let horizontal_padding_border = box_model.horizontal_padding() + box_model.horizontal_border();
    let mut content = request
        .assigned_border_size
        .and_then(|size| size.width.map(|width| (width - horizontal_padding_border).max(0.0)))
        .or_else(|| replaced_size.map(|size| size.width))
        .unwrap_or_else(|| resolve_content_width(constraints.preferred, constraints.min, constraints.max, request.available_width, constraints.border_box_inset, replaced_auto_width.unwrap_or(constraints.available_content)));
    if intrinsic_widths.shrink_to_fit {
        let (intrinsic_min, intrinsic_max) = intrinsic_widths.measured.expect("shrink-to-fit width requires measured intrinsic widths");
        content = constrain_content_width(intrinsic_max.min(constraints.available_content.max(intrinsic_min)), constraints.min, constraints.max, request.available_width, constraints.border_box_inset);
    }
    content
}
