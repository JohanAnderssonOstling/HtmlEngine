use crate::layout::LayoutEngine;
use crate::layout::replaced::{ReplacedSizeInput, preferred_aspect_ratio, resolve_replaced_content_size};
use crate::layout_model::LayoutMode;
use html_style_model::{BorderCollapseMode, BoxSizing, Float, ItemAlignment, OverflowMode, PositionMode, TextDirection, UsedPreferredSize as PreferredSize, UsedStyleView};
use kurbo::Size;
use std::time::Instant;

use super::box_constraints::{BoxLayoutRequest, constrain_content_width, resolve_block_margin_left, resolve_content_width, resolve_vertical_min_size_with_stretch_inset, resolve_vertical_size_with_stretch_inset};
use super::box_model::{ResolvedBoxModel, UsedBorderInsets};
use super::read_context::ImageIntrinsic;

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
    pub(super) intrinsic_min: bool,
    pub(super) intrinsic_max: bool,
    pub(super) ratio_auto_min_uses_content: bool,
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
    pub(super) inline_alignment: ItemAlignment,
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

/// Resolve normal-flow block alignment after width calculation. Alignment is
/// applied to the margin box, and automatic margins continue to take
/// precedence over `justify-self`.
pub(super) fn block_inline_alignment_offset(engine: &LayoutEngine<'_, '_>, resolved: &ResolvedBoxSizing) -> f64 {
    let request = resolved.request;
    let style = engine.reader.style(request.box_idx);
    if !engine.reader.is_block_box(request.box_idx) || resolved.is_float || style.position() == PositionMode::Absolute {
        return resolved.horizontal_margins.left;
    }

    let border_width = resolved.replaced_size.map_or(resolved.horizontal.content, |size| size.width) + resolved.box_model.horizontal_padding() + resolved.box_model.horizontal_border();
    let left = style.margin_left().resolve(request.available_width);
    let right = style.margin_right().resolve(request.available_width);
    let left_auto = style.margin_left_auto();
    let right_auto = style.margin_right_auto();
    if left_auto || right_auto {
        let remaining = request.available_width - border_width - if left_auto { 0.0 } else { left } - if right_auto { 0.0 } else { right };
        return match (left_auto, right_auto) {
            (true, true) if remaining > 0.0 => remaining / 2.0,
            (true, false) if remaining > 0.0 => remaining,
            _ => left,
        };
    }

    let free_space = request.available_width - border_width - left - right;
    let containing_direction = engine.reader.get_parent(request.box_idx).map(|parent| engine.reader.style(parent).direction()).unwrap_or(TextDirection::Ltr);
    let subject_direction = style.direction();
    let start_offset = |direction| if direction == TextDirection::Rtl { free_space } else { 0.0 };
    let end_offset = |direction| if direction == TextDirection::Rtl { 0.0 } else { free_space };
    let alignment_offset = match resolved.inline_alignment {
        ItemAlignment::Start | ItemAlignment::FlexStart => start_offset(containing_direction),
        ItemAlignment::End | ItemAlignment::FlexEnd => end_offset(containing_direction),
        ItemAlignment::SelfStart | ItemAlignment::Baseline => start_offset(subject_direction),
        ItemAlignment::SelfEnd => end_offset(subject_direction),
        ItemAlignment::Left => 0.0,
        ItemAlignment::Right => free_space,
        ItemAlignment::Center => free_space / 2.0,
        ItemAlignment::Auto | ItemAlignment::Normal | ItemAlignment::Stretch => return resolved.horizontal_margins.left,
    };
    left + alignment_offset
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
    let image_intrinsic = engine.reader.image_intrinsic(box_idx);
    let replaced_intrinsic = image_intrinsic.map(|intrinsic| intrinsic.size);
    let is_replaced = replaced_intrinsic.is_some();
    let inline_alignment = effective_inline_alignment(engine, box_idx);
    let authored_aspect_ratio = style.aspect_ratio();
    let used_aspect_ratio = preferred_aspect_ratio(authored_aspect_ratio, image_intrinsic.and_then(|intrinsic| intrinsic.ratio));
    let vertical = resolve_vertical_sizing(engine, style, request, box_model);
    let (horizontal, replaced_size) = resolve_horizontal_sizing(engine, style, request, box_model, vertical, image_intrinsic, used_aspect_ratio, is_float, inline_alignment);

    let layout_mode = engine.reader.box_layout_mode(box_idx).expect("layout box should exist").clone();
    let fills_available_width = !is_replaced
        && !is_float
        && !horizontal.shrink_to_fit
        && (engine.reader.is_block_box(box_idx) || engine.reader.is_anonymous_box(box_idx) || matches!(style.display(), html_style_model::Display::Flex | html_style_model::Display::Grid));
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
        inline_alignment,
        horizontal_margins,
        text_indent: style.text_indent().resolve(horizontal.content),
    }
}

fn effective_inline_alignment(engine: &LayoutEngine<'_, '_>, box_idx: usize) -> ItemAlignment {
    let alignment = engine.reader.layout_style(box_idx).justify_self;
    let alignment = if alignment == ItemAlignment::Auto {
        engine.reader.get_parent(box_idx).map(|parent| engine.reader.layout_style(parent).justify_items).unwrap_or(ItemAlignment::Normal)
    } else {
        alignment
    };
    if alignment == ItemAlignment::Auto { ItemAlignment::Normal } else { alignment }
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

fn resolve_vertical_sizing(engine: &LayoutEngine<'_, '_>, style: UsedStyleView<'_>, request: BoxLayoutRequest, box_model: ResolvedBoxModel) -> ResolvedVerticalSizing {
    let padding_border = box_model.vertical_padding() + box_model.vertical_border();
    let border_box_inset = match style.box_sizing() {
        BoxSizing::ContentBox => 0.0,
        BoxSizing::BorderBox => padding_border,
    };
    let stretch_inset = super::block::stretch_margin_inset(&engine.reader, request.box_idx, request.available_width) + padding_border;
    let authored_explicit = (!request.intrinsic_block_measurement)
        .then(|| resolve_vertical_size_with_stretch_inset(style.height(), request.parent_content_height, border_box_inset, stretch_inset))
        .flatten();
    let assigned_content = request.assigned_border_size.and_then(|size| size.height.map(|height| (height - padding_border).max(0.0)));
    let explicit = assigned_content.or(authored_explicit);
    let min = (!request.intrinsic_block_measurement)
        .then(|| resolve_vertical_min_size_with_stretch_inset(style.min_height(), request.parent_content_height, border_box_inset, stretch_inset))
        .flatten();
    let max = (!request.intrinsic_block_measurement)
        .then(|| resolve_vertical_size_with_stretch_inset(style.max_height(), request.parent_content_height, border_box_inset, stretch_inset))
        .flatten()
        .map(|maximum| maximum.max(min.unwrap_or(0.0)));
    let intrinsic_min = !request.intrinsic_block_measurement && matches!(style.min_height(), PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent);
    let intrinsic_max = !request.intrinsic_block_measurement && matches!(style.max_height(), PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent);
    // A preferred aspect ratio can turn an automatic block size into a
    // transferred size. Its automatic minimum remains content-based unless
    // this axis is a scroll container; `clip` deliberately is not scrollable.
    let ratio_auto_min_uses_content = matches!(style.height(), PreferredSize::Auto)
        && matches!(style.min_height(), PreferredSize::Auto)
        && !matches!(style.overflow_y(), OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto);
    let descendant_basis = match (request.assigned_border_size, assigned_content) {
        (Some(assigned), Some(height)) if assigned.height_is_definite => Some(height),
        _ if request.suppress_authored_height_basis => None,
        _ => authored_explicit.map(|height| {
            let height = max.map_or(height, |maximum| height.min(maximum));
            min.map_or(height, |minimum| height.max(minimum))
        }),
    };
    ResolvedVerticalSizing { authored_explicit, explicit, descendant_basis, min, max, intrinsic_min, intrinsic_max, ratio_auto_min_uses_content }
}

fn resolve_horizontal_sizing(
    engine: &LayoutEngine<'_, '_>, style: UsedStyleView<'_>, request: BoxLayoutRequest, box_model: ResolvedBoxModel, vertical: ResolvedVerticalSizing, image_intrinsic: Option<ImageIntrinsic>, aspect_ratio: Option<f64>, is_float: bool, inline_alignment: ItemAlignment,
) -> (ResolvedHorizontalSizing, Option<Size>) {
    let replaced_intrinsic = image_intrinsic.map(|intrinsic| intrinsic.size);
    let intrinsic_widths = resolve_intrinsic_widths(engine, style, request, vertical, replaced_intrinsic, is_float, inline_alignment);
    let constraints = resolve_width_constraints(engine, style, request, box_model, image_intrinsic, intrinsic_widths.measured);
    let replaced_size = resolve_replaced_size(style, request, box_model, constraints, replaced_intrinsic, aspect_ratio);
    let ratio_stretch_width = (replaced_intrinsic.is_none() && matches!(style.width(), PreferredSize::Auto) && matches!(style.height(), PreferredSize::Stretch))
        .then(|| {
            let height = vertical.authored_explicit?;
            let ratio = aspect_ratio?;
            Some(match style.box_sizing() {
                BoxSizing::ContentBox => height * ratio,
                BoxSizing::BorderBox => {
                    let vertical_border_box = height + box_model.vertical_padding_border();
                    (vertical_border_box * ratio - box_model.horizontal_padding_border()).max(0.0)
                }
            })
        })
        .flatten();
    let content = resolve_used_content_width(request, box_model, constraints, vertical, replaced_intrinsic, replaced_size, intrinsic_widths, ratio_stretch_width);
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

fn resolve_intrinsic_widths(
    engine: &LayoutEngine<'_, '_>,
    style: UsedStyleView<'_>,
    request: BoxLayoutRequest,
    vertical: ResolvedVerticalSizing,
    replaced_intrinsic: Option<Size>,
    is_float: bool,
    inline_alignment: ItemAlignment,
) -> ResolvedIntrinsicWidths {
    let is_table_cell = engine.reader.is_table_cell_box(request.box_idx);
    let preferred = if is_table_cell { PreferredSize::Auto } else { style.width() };
    let min = if is_table_cell { PreferredSize::Auto } else { style.min_width() };
    let max = if is_table_cell { PreferredSize::Auto } else { style.max_width() };
    let uses_intrinsic_keyword = replaced_intrinsic.is_none() && [preferred, min, max].into_iter().any(|size| matches!(size, PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent));
    let is_absolute_shrink_to_fit = style.position() == PositionMode::Absolute && !(style.inset_left().is_some() && style.inset_right().is_some());
    let is_alignment_shrink_to_fit = engine.reader.is_block_box(request.box_idx)
        && style.position() != PositionMode::Absolute
        && !matches!(inline_alignment, ItemAlignment::Auto | ItemAlignment::Normal | ItemAlignment::Stretch);
    let shrink_to_fit = replaced_intrinsic.is_none()
        && request.assigned_border_size.and_then(|assigned| assigned.width).is_none()
        && (is_float || is_absolute_shrink_to_fit || is_alignment_shrink_to_fit)
        && matches!(preferred, PreferredSize::Auto);
    let mut measured = if shrink_to_fit {
        let model = ResolvedBoxModel::new(style, request.available_width);
        let available_content = (request.auto_width_limit.unwrap_or(request.available_width) - model.horizontal_margin() - model.horizontal_padding_border()).max(0.0);
        Some(super::intrinsic_sizing::box_content_intrinsic_widths_with_available(engine, request.box_idx, available_content))
    } else if uses_intrinsic_keyword
        && (style.max_width().percentage_dependent()
            || (style.box_sizing() == BoxSizing::BorderBox && (style.padding_left().has_percentage() || style.padding_right().has_percentage())))
    {
        Some(super::intrinsic_sizing::box_content_intrinsic_widths_with_available(engine, request.box_idx, request.available_width))
    } else {
        uses_intrinsic_keyword.then(|| super::intrinsic_sizing::box_content_intrinsic_widths(engine, request.box_idx))
    };
    // A definite float height makes a percentage-height replaced child
    // definite while the float's shrink-to-fit width is being measured. Its
    // intrinsic aspect ratio therefore contributes the transferred width,
    // rather than the child's unscaled intrinsic width.
    if shrink_to_fit && let Some(transferred_width) = vertical.authored_explicit.and_then(|height| crate::layout::percentage_height_image_width(engine, request.box_idx, height)) {
        measured = Some((transferred_width, transferred_width));
    }
    ResolvedIntrinsicWidths { measured, shrink_to_fit }
}

fn resolve_width_constraints(
    engine: &LayoutEngine<'_, '_>, style: UsedStyleView<'_>, request: BoxLayoutRequest, box_model: ResolvedBoxModel, image_intrinsic: Option<ImageIntrinsic>, intrinsic_widths: Option<(f64, f64)>,
) -> ResolvedWidthConstraints {
    let replaced_intrinsic = image_intrinsic.map(|intrinsic| intrinsic.size);
    let is_table_cell = engine.reader.is_table_cell_box(request.box_idx);
    let authored_width = if is_table_cell { PreferredSize::Auto } else { style.width() };
    let mut min = if is_table_cell { PreferredSize::Auto } else { style.min_width() };
    let mut max = if is_table_cell { PreferredSize::Auto } else { style.max_width() };
    // Table-cell margins compute normally but do not participate in its used
    // table geometry.
    let horizontal_margin = if is_table_cell { 0.0 } else { box_model.horizontal_margin() };
    let margin_padding = horizontal_margin + box_model.horizontal_padding();
    let horizontal_padding_border = box_model.horizontal_padding() + box_model.horizontal_border();
    let smart_width = image_intrinsic.and_then(|intrinsic| {
        engine.reader.smart_standalone_image_width(
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
    let parent_owns_item_sizing = engine.reader.get_parent(request.box_idx).is_some_and(|parent| matches!(engine.reader.box_layout_mode(parent), Some(LayoutMode::Flex(_) | LayoutMode::Grid(_))));
    let resolve_stretch = |size| match size {
        // Stretch sizes the margin box to the available space. Store a
        // specified size that resolves back to the already box-model-adjusted
        // content size in the common width constraint path.
        PreferredSize::Stretch if !parent_owns_item_sizing => PreferredSize::Px((available_content + border_box_inset) as f32),
        _ => size,
    };
    preferred = resolve_stretch(preferred);
    min = resolve_stretch(min);
    max = resolve_stretch(max);
    ResolvedWidthConstraints { preferred, min, max, horizontal_margin, margin_padding, border_box_inset, available_content, smart_width: smart_width.is_some() }
}

fn resolve_replaced_size(style: UsedStyleView<'_>, request: BoxLayoutRequest, box_model: ResolvedBoxModel, constraints: ResolvedWidthConstraints, intrinsic: Option<Size>, aspect_ratio: Option<f64>) -> Option<Size> {
    intrinsic.filter(|_| request.assigned_border_size.is_none()).map(|intrinsic| {
        resolve_replaced_content_size(
            ReplacedSizeInput::from_style(style, intrinsic, aspect_ratio, request.available_width, request.parent_content_height)
                .with_box_model(
                    if constraints.smart_width { 0.0 } else { constraints.horizontal_margin },
                    box_model.vertical_margin(),
                    box_model.horizontal_padding() + box_model.horizontal_border(),
                    box_model.vertical_padding() + box_model.vertical_border(),
                )
                .with_width_constraints(
                    constraints.preferred,
                    if constraints.smart_width { PreferredSize::Auto } else { constraints.min },
                    if constraints.smart_width { PreferredSize::Auto } else { constraints.max },
                ),
        )
    })
}

fn resolve_used_content_width(
    request: BoxLayoutRequest, box_model: ResolvedBoxModel, constraints: ResolvedWidthConstraints, vertical: ResolvedVerticalSizing, replaced_intrinsic: Option<Size>, replaced_size: Option<Size>, intrinsic_widths: ResolvedIntrinsicWidths, ratio_stretch_width: Option<f64>,
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
    if let Some(ratio_width) = ratio_stretch_width {
        content = constrain_content_width(ratio_width, constraints.min, constraints.max, request.available_width, constraints.border_box_inset);
    }
    content
}
