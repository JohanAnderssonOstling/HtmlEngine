use super::tracks::{fixed_auto_track_limit, length_percentage_auto, line_names as grid_line_names, placement as grid_placement, template_track as grid_template_track, track_size as grid_track_size, used_length_percentage};
use super::{TaffyContainerKind, finite_f32};
use crate::layout::{
    LayoutEngine, ReplacedFlexAutoMinInput, ReplacedMainAxis, ResolvedBoxModel, clamp_replaced_definite_size_by_intrinsic_constraints, measure_box_isolated, resolve_replaced_flex_auto_min_main_size, resolve_replaced_intrinsic_constraint,
};
use html_style_model::{ContentAlignment, GridAutoFlow, ItemAlignment, LayoutStyle, OverflowMode, UsedPreferredSize as PreferredSize};
use taffy::geometry::{Line, Point as TaffyPoint, Rect, Size as TaffySize};
use taffy::prelude::{Dimension, LengthPercentage, Style};
use taffy::style::{
    AlignContent as TaffyAlignContent, AlignItems as TaffyAlignItems, BoxSizing as TaffyBoxSizing, Display as TaffyDisplay, FlexDirection as TaffyFlexDirection, FlexWrap as TaffyFlexWrap, GridAutoFlow as TaffyGridAutoFlow,
    GridTemplateArea as TaffyGridTemplateArea, Overflow as TaffyOverflow,
};

pub(super) fn dimension(value: PreferredSize) -> Dimension {
    match value {
        PreferredSize::Auto | PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => Dimension::auto(),
        // Stretch participates through flex/grid alignment; encoding it as
        // 100% would resolve an indefinite cross size against the container.
        PreferredSize::Stretch => Dimension::auto(),
        PreferredSize::Px(value) => Dimension::length(value.max(0.0)),
        PreferredSize::Percent(value) => Dimension::percent(value.max(0.0)),
        // Callers with a definite percentage basis resolve calc before this
        // fallback. With an indefinite basis, only the absolute term remains.
        PreferredSize::Calc { absolute_px, percentage_dependent: false, .. } => Dimension::length(absolute_px.max(0.0)),
        PreferredSize::Calc { percentage_dependent: true, .. } => Dimension::auto(),
        PreferredSize::Comparison { .. } if value.percentage_dependent() => Dimension::auto(),
        PreferredSize::Comparison { .. } => Dimension::length(html_style_model::resolve_used_preferred_size(value, 0.0, 0.0).max(0.0) as f32),
    }
}
pub(super) fn layout_style<'a>(session: &LayoutEngine<'a, '_>, box_idx: usize) -> &'a LayoutStyle {
    let indices = session.reader.box_style_indices(box_idx).unwrap_or_else(|| session.reader.styles().default_indices());
    session.reader.styles().layout_style(indices).expect("validated style handle")
}

pub(super) fn taffy_container_style(
    session: &LayoutEngine<'_, '_>, box_idx: usize, content_width: f64, explicit_height: Option<f64>, min_height: Option<f64>, max_height: Option<f64>, kind: TaffyContainerKind, floor_fixed_grid_column_maxima: bool,
) -> Style {
    let source = layout_style(session, box_idx);
    let used = session.reader.style(box_idx);
    let grid_template_rows = used.grid_template_rows();
    let grid_template_columns = used.grid_template_columns();
    let grid_auto_rows = used.grid_auto_rows();
    let grid_auto_columns = used.grid_auto_columns();
    let mut style = Style {
        display: match kind {
            TaffyContainerKind::Flex => TaffyDisplay::Flex,
            TaffyContainerKind::Grid => TaffyDisplay::Grid,
        },
        size: TaffySize { width: Dimension::length(finite_f32(content_width)), height: explicit_height.map(|value| Dimension::length(finite_f32(value))).unwrap_or_else(Dimension::auto) },
        min_size: TaffySize { width: Dimension::auto(), height: min_height.map(|value| Dimension::length(finite_f32(value))).unwrap_or_else(Dimension::auto) },
        max_size: TaffySize { width: Dimension::auto(), height: max_height.map(|value| Dimension::length(finite_f32(value))).unwrap_or_else(Dimension::auto) },
        gap: TaffySize { width: used_length_percentage(session.reader.style(box_idx).column_gap(), content_width), height: used_length_percentage(session.reader.style(box_idx).row_gap(), content_width) },
        align_items: item_alignment(source.align_items, false),
        align_content: content_alignment(source.align_content, kind, false),
        justify_content: content_alignment(source.justify_content, kind, true),
        flex_direction: match source.flex_direction {
            html_style_model::FlexDirection::Row => TaffyFlexDirection::Row,
            html_style_model::FlexDirection::RowReverse => TaffyFlexDirection::RowReverse,
            html_style_model::FlexDirection::Column => TaffyFlexDirection::Column,
            html_style_model::FlexDirection::ColumnReverse => TaffyFlexDirection::ColumnReverse,
        },
        flex_wrap: match source.flex_wrap {
            html_style_model::FlexWrap::NoWrap => TaffyFlexWrap::NoWrap,
            html_style_model::FlexWrap::Wrap => TaffyFlexWrap::Wrap,
            html_style_model::FlexWrap::WrapReverse => TaffyFlexWrap::WrapReverse,
        },
        justify_items: item_alignment(source.justify_items, true),
        grid_template_rows: grid_template_rows.map(|track| grid_template_track(session.reader.styles(), &track, false)).collect(),
        grid_template_columns: grid_template_columns.map(|track| grid_template_track(session.reader.styles(), &track, floor_fixed_grid_column_maxima)).collect(),
        grid_template_row_names: grid_line_names(session.reader.styles(), &source.grid_template_row_names),
        grid_template_column_names: grid_line_names(session.reader.styles(), &source.grid_template_column_names),
        grid_template_areas: source
            .grid_template_areas
            .iter()
            .filter_map(|area| Some(TaffyGridTemplateArea { name: session.reader.styles().string(area.name)?.into(), row_start: area.row_start, row_end: area.row_end, column_start: area.column_start, column_end: area.column_end }))
            .collect(),
        grid_auto_rows: grid_auto_rows.map(|track| grid_track_size(&track, false)).collect(),
        grid_auto_columns: grid_auto_columns.map(|track| grid_track_size(&track, floor_fixed_grid_column_maxima)).collect(),
        grid_auto_flow: match source.grid_auto_flow {
            GridAutoFlow::Row => TaffyGridAutoFlow::Row,
            GridAutoFlow::Column => TaffyGridAutoFlow::Column,
            GridAutoFlow::RowDense => TaffyGridAutoFlow::RowDense,
            GridAutoFlow::ColumnDense => TaffyGridAutoFlow::ColumnDense,
        },
        ..Style::default()
    };
    if kind == TaffyContainerKind::Flex {
        style.justify_items = None;
    }
    style
}

pub(super) fn grid_definite_inline_minimum_exceeds_track_limit(session: &LayoutEngine<'_, '_>, container_idx: usize, children: &[u32], kind: TaffyContainerKind, containing_width: f64) -> bool {
    if kind != TaffyContainerKind::Grid {
        return false;
    }
    let Some(track_limit) = fixed_auto_track_limit(session.reader.style(container_idx).grid_template_columns(), containing_width) else {
        return false;
    };
    children.iter().any(|&child| {
        let style = session.reader.style(child as usize);
        let box_model = ResolvedBoxModel::new(style, containing_width);
        let margin = box_model.horizontal_margin();
        let padding_border = box_model.horizontal_padding_border();
        let outer = |width: f32| match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => width.max(0.0) as f64 + padding_border + margin,
            html_style_model::BoxSizing::BorderBox => (width.max(0.0) as f64).max(padding_border) + margin,
        };
        let preferred = match style.width() {
            PreferredSize::Px(width) => Some(outer(width)),
            _ => None,
        };
        let minimum = match style.min_width() {
            PreferredSize::Px(width) => Some(outer(width)),
            _ => None,
        };
        // Even an auto-sized empty item has a definite outer lower bound:
        // its padding, border, and non-auto margins. Taffy otherwise loses
        // that contribution when every spanned `minmax(auto, fixed)` track
        // has a zero maximum.
        let outer_insets = outer(0.0);
        preferred.into_iter().chain(minimum).chain(std::iter::once(outer_insets)).any(|contribution| contribution > track_limit + 0.01)
    })
}

pub(super) fn taffy_item_style(session: &LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64, containing_height: Option<f64>, kind: TaffyContainerKind, resolved_flex_basis: Option<Dimension>) -> Style {
    let style = session.reader.style(box_idx);
    let box_model = ResolvedBoxModel::new(style, containing_width);
    let layout = layout_style(session, box_idx);
    let replaced_intrinsic = session.replaced.intrinsic_size(&session.reader, box_idx);
    let both_replaced_axes_auto = replaced_intrinsic.is_some() && matches!(style.width(), PreferredSize::Auto) && matches!(style.height(), PreferredSize::Auto);
    let authored_aspect_ratio = style.aspect_ratio();
    let intrinsic_aspect_ratio = replaced_intrinsic.and_then(|size| (both_replaced_axes_auto && size.height > 0.0).then_some(finite_f32(size.width / size.height)));
    let aspect_ratio = if kind == TaffyContainerKind::Flex && replaced_intrinsic.is_some() && layout.flex_grow > 0.0 {
        // A flexible replaced item may grow its main axis independently
        // of a constrained cross axis. Its measure callback transfers the
        // ratio only when one axis remains unresolved after flexing.
        None
    } else if authored_aspect_ratio.uses_intrinsic() {
        if replaced_intrinsic.is_some() { intrinsic_aspect_ratio } else { authored_aspect_ratio.preferred() }
    } else {
        authored_aspect_ratio.preferred()
    };
    let width = if grid_percentage_width_is_stretch_equivalent(session, box_idx, kind) { Dimension::auto() } else { taffy_item_width_dimension(session, box_idx, style.width(), containing_width) };
    let horizontal_main_axis = session.reader.get_parent(box_idx).map(|parent| matches!(layout_style(session, parent).flex_direction, html_style_model::FlexDirection::Row | html_style_model::FlexDirection::RowReverse)).unwrap_or(true);
    let mut min_width = if kind == TaffyContainerKind::Flex && horizontal_main_axis && matches!(style.min_width(), PreferredSize::Auto) {
        Dimension::length(finite_f32(flex_replaced_automatic_minimum(session, box_idx, containing_width, containing_height, true).unwrap_or_else(|| flex_automatic_min_content_width(session, box_idx, containing_width))))
    } else if kind == TaffyContainerKind::Grid && matches!(style.width(), PreferredSize::FitContent) && matches!(style.min_width(), PreferredSize::Auto) {
        // A fit-content preferred size is neither auto nor dependent on
        // the containing block. CSS Grid therefore uses the min-content
        // contribution, even when it exceeds a fixed track maximum.
        taffy_item_width_dimension(session, box_idx, PreferredSize::MinContent, containing_width)
    } else {
        taffy_item_width_dimension(session, box_idx, style.min_width(), containing_width)
    };
    if kind == TaffyContainerKind::Flex
        && horizontal_main_axis
        && matches!(style.min_width(), PreferredSize::Auto)
        && let Some(containing_height) = containing_height
    {
        let item_cross_inset = box_model.vertical_padding_border();
        if let Some(width) = session.replaced.percentage_height_width(&session.reader, box_idx, (containing_height - item_cross_inset).max(0.0)) {
            min_width = Dimension::length(finite_f32(width));
        }
    }
    let intrinsic_height_constraint = |value| replaced_intrinsic.and_then(|intrinsic| resolve_replaced_intrinsic_constraint(value, intrinsic.height, box_model.vertical_padding_border(), style.box_sizing()));
    let intrinsic_min_height = intrinsic_height_constraint(style.min_height());
    let intrinsic_max_height = intrinsic_height_constraint(style.max_height());
    let height = clamp_replaced_definite_size_by_intrinsic_constraints(style.height(), intrinsic_min_height, intrinsic_max_height).map(finite_f32).map(Dimension::length).unwrap_or_else(|| dimension(style.height()));
    let mut min_height = if let Some(value) = intrinsic_min_height {
        Dimension::length(finite_f32(value))
    } else if kind == TaffyContainerKind::Flex && !horizontal_main_axis && matches!(style.min_height(), PreferredSize::Auto) {
        flex_replaced_automatic_minimum(session, box_idx, containing_width, containing_height, false).map(finite_f32).map(Dimension::length).unwrap_or_else(Dimension::auto)
    } else {
        match style.min_height() {
            PreferredSize::Percent(_) if containing_height.is_none() => Dimension::length(0.0),
            value => dimension(value),
        }
    };
    let mut max_width = taffy_item_width_dimension(session, box_idx, style.max_width(), containing_width);
    let mut max_height = intrinsic_max_height.map(finite_f32).map(Dimension::length).unwrap_or_else(|| dimension(style.max_height()));
    let parent_alignment = session.reader.get_parent(box_idx).map(|parent| layout_style(session, parent).align_items).unwrap_or(ItemAlignment::Normal);
    let effective_alignment = if layout.align_self == ItemAlignment::Auto { parent_alignment } else { layout.align_self };
    let cross_auto_margins = if horizontal_main_axis { layout.margin_top_auto || layout.margin_bottom_auto } else { layout.margin_left_auto || layout.margin_right_auto };
    let flex_cross_stretches = kind == TaffyContainerKind::Flex && !cross_auto_margins && matches!(effective_alignment, ItemAlignment::Normal | ItemAlignment::Stretch);
    let preserve_ratio_in_constraints = kind != TaffyContainerKind::Flex || (!flex_cross_stretches && layout.flex_grow == 0.0);
    if both_replaced_axes_auto
        && preserve_ratio_in_constraints
        && let Some(intrinsic) = replaced_intrinsic
    {
        let horizontal_inset = box_model.horizontal_padding_border();
        let vertical_inset = box_model.vertical_padding_border();
        let constraint_to_content = |value: f32, inset: f64| match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => value.max(0.0) as f64,
            html_style_model::BoxSizing::BorderBox => (value.max(0.0) as f64 - inset).max(0.0),
        };
        let content_to_constraint = |value: f64, inset: f64| match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => value,
            html_style_model::BoxSizing::BorderBox => value + inset,
        };
        let min_w = match style.min_width() {
            PreferredSize::Auto => Some(0.0),
            PreferredSize::Px(value) => Some(constraint_to_content(value, horizontal_inset)),
            _ => None,
        };
        let min_h = match style.min_height() {
            PreferredSize::Auto => Some(0.0),
            PreferredSize::Px(value) => Some(constraint_to_content(value, vertical_inset)),
            _ => None,
        };
        let max_w = match style.max_width() {
            PreferredSize::Auto => Some(f64::INFINITY),
            PreferredSize::Px(value) => Some(constraint_to_content(value, horizontal_inset)),
            _ => None,
        };
        let max_h = match style.max_height() {
            PreferredSize::Auto => Some(f64::INFINITY),
            PreferredSize::Px(value) => Some(constraint_to_content(value, vertical_inset)),
            _ => None,
        };
        if let (Some(min_w), Some(min_h), Some(max_w), Some(max_h)) = (min_w, min_h, max_w, max_h)
            && intrinsic.width > 0.0
            && intrinsic.height > 0.0
        {
            let grow = (min_w / intrinsic.width).max(min_h / intrinsic.height).max(1.0);
            let shrink = (max_w / intrinsic.width).min(max_h / intrinsic.height).min(1.0);
            if grow <= 1.0 || shrink >= 1.0 {
                if grow > 1.0 {
                    min_width = Dimension::length(finite_f32(content_to_constraint(intrinsic.width * grow, horizontal_inset)));
                    min_height = Dimension::length(finite_f32(content_to_constraint(intrinsic.height * grow, vertical_inset)));
                } else if shrink < 1.0 {
                    max_width = Dimension::length(finite_f32(content_to_constraint(intrinsic.width * shrink, horizontal_inset)));
                    max_height = Dimension::length(finite_f32(content_to_constraint(intrinsic.height * shrink, vertical_inset)));
                }
            }
        }
    }
    Style {
        box_sizing: match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => TaffyBoxSizing::ContentBox,
            html_style_model::BoxSizing::BorderBox => TaffyBoxSizing::BorderBox,
        },
        margin: Rect {
            left: length_percentage_auto(style.margin_left(), layout.margin_left_auto, containing_width),
            right: length_percentage_auto(style.margin_right(), layout.margin_right_auto, containing_width),
            top: length_percentage_auto(style.margin_top(), layout.margin_top_auto, containing_width),
            bottom: length_percentage_auto(style.margin_bottom(), layout.margin_bottom_auto, containing_width),
        },
        padding: Rect {
            left: used_length_percentage(style.padding_left(), containing_width),
            right: used_length_percentage(style.padding_right(), containing_width),
            top: used_length_percentage(style.padding_top(), containing_width),
            bottom: used_length_percentage(style.padding_bottom(), containing_width),
        },
        border: Rect {
            left: LengthPercentage::length(style.border_left_width()),
            right: LengthPercentage::length(style.border_right_width()),
            top: LengthPercentage::length(style.border_top_width()),
            bottom: LengthPercentage::length(style.border_bottom_width()),
        },
        size: TaffySize { width, height },
        // A percentage minimum in an indefinite containing block computes
        // to zero. Passing the unresolved percentage through makes Taffy
        // substitute its automatic content minimum instead.
        min_size: TaffySize { width: min_width, height: min_height },
        max_size: TaffySize { width: max_width, height: max_height },
        aspect_ratio,
        overflow: TaffyPoint { x: overflow(style.overflow_x()), y: overflow(style.overflow_y()) },
        flex_grow: layout.flex_grow,
        flex_shrink: layout.flex_shrink,
        flex_basis: resolved_flex_basis.unwrap_or_else(|| dimension(style.flex_basis())),
        align_self: item_alignment(layout.align_self, false),
        justify_self: item_alignment(layout.justify_self, true),
        grid_row: Line { start: grid_placement(session.reader.styles(), layout.grid_row.start), end: grid_placement(session.reader.styles(), layout.grid_row.end) },
        grid_column: Line { start: grid_placement(session.reader.styles(), layout.grid_column.start), end: grid_placement(session.reader.styles(), layout.grid_column.end) },
        ..Style::default()
    }
}

/// Taffy represents `flex-basis` as only auto, length, or percentage. CSS
/// intrinsic sizing keywords therefore have to be resolved at the adapter
/// boundary instead of being collapsed to `auto` (content/max-content).
pub(super) fn resolve_intrinsic_flex_basis(session: &mut LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64, horizontal_main_axis: bool) -> Option<Dimension> {
    let basis = session.reader.style(box_idx).flex_basis();
    if !matches!(basis, PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent) {
        return None;
    }
    if horizontal_main_axis {
        return Some(taffy_item_width_dimension(session, box_idx, basis, containing_width));
    }

    // Intrinsic block-axis sizes are measured with the item's allocated
    // cross size. This is observable when content wraps at that width.
    let measured = measure_box_isolated(session, box_idx, containing_width, None);
    let style = session.reader.style(box_idx);
    let vertical_inset = style.get_vertical_padding(containing_width) + style.border_top_width() as f64 + style.border_bottom_width() as f64;
    let basis = match style.box_sizing() {
        html_style_model::BoxSizing::ContentBox => (measured.height - vertical_inset).max(0.0),
        html_style_model::BoxSizing::BorderBox => measured.height.max(vertical_inset),
    };
    Some(Dimension::length(finite_f32(basis)))
}

/// Resolve the flex-item automatic minimum from the content-size
/// suggestion, capped by a definite preferred size. Feeding Taffy `auto`
/// here makes its generic measurement path treat a specified width as the
/// minimum itself, which prevents empty fixed-size items from shrinking
/// and gives percentage-sized tables no usable share of the flex line.
fn flex_automatic_min_content_width(session: &LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64) -> f64 {
    let style = session.reader.style(box_idx);
    if matches!(style.overflow_x(), OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto) {
        return 0.0;
    }

    let content_min = crate::layout::box_content_intrinsic_widths(session, box_idx).0.max(0.0);
    let padding_border = style.get_horizontal_padding(containing_width) + style.border_left_width() as f64 + style.border_right_width() as f64;
    let as_content_width = |outer_or_content: f64| match style.box_sizing() {
        html_style_model::BoxSizing::ContentBox => outer_or_content.max(0.0),
        html_style_model::BoxSizing::BorderBox => (outer_or_content - padding_border).max(0.0),
    };
    let preferred = match style.width() {
        PreferredSize::Px(value) => Some(as_content_width(value as f64)),
        PreferredSize::Percent(value) => Some(as_content_width(containing_width * value as f64)),
        PreferredSize::Calc { absolute_px, percentage, .. } => Some(as_content_width(absolute_px as f64 + containing_width * percentage as f64)),
        PreferredSize::Comparison { .. } => Some(as_content_width(html_style_model::resolve_used_preferred_size(style.width(), 0.0, containing_width))),
        PreferredSize::MinContent => Some(content_min),
        PreferredSize::MaxContent | PreferredSize::FitContent => Some(crate::layout::box_content_intrinsic_widths(session, box_idx).1.max(content_min)),
        PreferredSize::Stretch => Some(as_content_width(containing_width)),
        PreferredSize::Auto => None,
    };
    let mut automatic_min = preferred.map_or(content_min, |preferred| content_min.min(preferred));
    if let PreferredSize::Px(value) = style.max_width() {
        automatic_min = automatic_min.min(as_content_width(value as f64));
    } else if let PreferredSize::Percent(value) = style.max_width() {
        automatic_min = automatic_min.min(as_content_width(containing_width * value as f64));
    }
    automatic_min.max(0.0)
}

/// Compute the content-based automatic minimum of a replaced flex item.
/// Taffy's generic leaf minimum starts with the raw intrinsic size, but CSS
/// first converts a definite (including stretched) cross size and cross
/// min/max constraints through the preferred aspect ratio.
fn flex_replaced_automatic_minimum(session: &LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64, containing_height: Option<f64>, horizontal_main_axis: bool) -> Option<f64> {
    let intrinsic = session.replaced.intrinsic_size(&session.reader, box_idx)?;
    let style = session.reader.style(box_idx);
    let layout = layout_style(session, box_idx);
    let scrollable_main = if horizontal_main_axis { style.overflow_x() } else { style.overflow_y() };
    if matches!(scrollable_main, OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto) {
        return Some(0.0);
    }

    let intrinsic_ratio = (intrinsic.height > 0.0).then_some(intrinsic.width / intrinsic.height);
    let authored_ratio = style.aspect_ratio();
    let ratio = if authored_ratio.uses_intrinsic() { intrinsic_ratio.or_else(|| authored_ratio.preferred().map(f64::from)) } else { authored_ratio.preferred().map(f64::from) }.filter(|ratio| ratio.is_finite() && *ratio > 0.0);

    let horizontal_padding_border = style.get_horizontal_padding(containing_width) + style.border_left_width() as f64 + style.border_right_width() as f64;
    let vertical_padding_border = style.get_vertical_padding(containing_width) + style.border_top_width() as f64 + style.border_bottom_width() as f64;
    let horizontal_margin = style.get_horizontal_margin(containing_width);
    let vertical_margin = style.margin_top().resolve(containing_width) + style.margin_bottom().resolve(containing_width);
    let (main_axis, main_size, cross_size, min_main_size, min_cross_size, max_main_size, max_cross_size, main_basis, cross_basis, main_margin, cross_margin, main_padding_border, cross_padding_border, cross_auto_margins) =
        if horizontal_main_axis {
            (
                ReplacedMainAxis::Horizontal,
                style.width(),
                style.height(),
                style.min_width(),
                style.min_height(),
                style.max_width(),
                style.max_height(),
                Some(containing_width),
                containing_height,
                horizontal_margin,
                vertical_margin,
                horizontal_padding_border,
                vertical_padding_border,
                layout.margin_top_auto || layout.margin_bottom_auto,
            )
        } else {
            (
                ReplacedMainAxis::Vertical,
                style.height(),
                style.width(),
                style.min_height(),
                style.min_width(),
                style.max_height(),
                style.max_width(),
                containing_height,
                Some(containing_width),
                vertical_margin,
                horizontal_margin,
                vertical_padding_border,
                horizontal_padding_border,
                layout.margin_left_auto || layout.margin_right_auto,
            )
        };
    let mut stretch_cross_size = false;
    if matches!(cross_size, PreferredSize::Auto) && !cross_auto_margins {
        let parent_alignment = session.reader.get_parent(box_idx).map(|parent| layout_style(session, parent).align_items).unwrap_or(ItemAlignment::Normal);
        let effective_alignment = if layout.align_self == ItemAlignment::Auto { parent_alignment } else { layout.align_self };
        if matches!(effective_alignment, ItemAlignment::Normal | ItemAlignment::Stretch) {
            stretch_cross_size = cross_basis.is_some();
        }
    }
    Some(resolve_replaced_flex_auto_min_main_size(ReplacedFlexAutoMinInput {
        intrinsic,
        aspect_ratio: ratio,
        main_axis,
        main_size,
        cross_size,
        min_main_size,
        min_cross_size,
        max_main_size,
        max_cross_size,
        main_basis,
        cross_basis,
        main_margin,
        cross_margin,
        main_padding_border,
        cross_padding_border,
        box_sizing: style.box_sizing(),
        stretch_cross_size,
    }))
}

/// Taffy currently resolves a percentage grid-item width against the
/// entire grid container while sizing tracks. For an otherwise
/// unconstrained 100% item with no horizontal box-model additions, that
/// width is equivalent to the grid item's default stretch behavior. Using
/// stretch keeps the percentage from inflating every fractional track.
fn grid_percentage_width_is_stretch_equivalent(session: &LayoutEngine<'_, '_>, box_idx: usize, kind: TaffyContainerKind) -> bool {
    if kind != TaffyContainerKind::Grid {
        return false;
    }
    let style = session.reader.style(box_idx);
    let layout = layout_style(session, box_idx);
    matches!(style.width(), PreferredSize::Percent(value) if (value - 1.0).abs() <= f32::EPSILON)
        && matches!(style.min_width(), PreferredSize::Auto)
        && matches!(style.max_width(), PreferredSize::Auto)
        && !layout.margin_left_auto
        && !layout.margin_right_auto
        && style.margin_left().is_zero()
        && style.margin_right().is_zero()
        && style.padding_left().is_zero()
        && style.padding_right().is_zero()
        && style.border_left_width() == 0.0
        && style.border_right_width() == 0.0
}

fn taffy_item_width_dimension(session: &LayoutEngine<'_, '_>, box_idx: usize, value: PreferredSize, containing_width: f64) -> Dimension {
    if let PreferredSize::Calc { absolute_px, percentage, .. } = value {
        return Dimension::length(finite_f32((absolute_px as f64 + containing_width * percentage as f64).max(0.0)));
    }
    if !matches!(value, PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent) {
        return dimension(value);
    }
    let style = session.reader.style(box_idx);
    let (min, max) = crate::layout::box_content_intrinsic_widths(session, box_idx);
    let available = (containing_width - style.get_horizontal_margin_padding(containing_width) - style.border_left_width() as f64 - style.border_right_width() as f64).max(0.0);
    let content = match value {
        PreferredSize::MinContent => min,
        PreferredSize::MaxContent => max,
        PreferredSize::FitContent => max.min(available.max(min)),
        _ => unreachable!(),
    };
    let border_box_inset = match style.box_sizing() {
        html_style_model::BoxSizing::ContentBox => 0.0,
        html_style_model::BoxSizing::BorderBox => style.get_horizontal_padding(containing_width) + style.border_left_width() as f64 + style.border_right_width() as f64,
    };
    Dimension::length(finite_f32(content + border_box_inset))
}

pub(super) fn overflow(value: OverflowMode) -> TaffyOverflow {
    match value {
        OverflowMode::Visible => TaffyOverflow::Visible,
        OverflowMode::Clip => TaffyOverflow::Clip,
        OverflowMode::Hidden | OverflowMode::Auto => TaffyOverflow::Hidden,
        OverflowMode::Scroll => TaffyOverflow::Scroll,
    }
}

pub(super) fn content_alignment(value: ContentAlignment, kind: TaffyContainerKind, main_axis: bool) -> Option<TaffyAlignContent> {
    Some(match value {
        ContentAlignment::Normal if kind == TaffyContainerKind::Flex && main_axis => TaffyAlignContent::FlexStart,
        ContentAlignment::Normal => TaffyAlignContent::Stretch,
        ContentAlignment::Start => TaffyAlignContent::Start,
        ContentAlignment::End => TaffyAlignContent::End,
        ContentAlignment::FlexStart => TaffyAlignContent::FlexStart,
        ContentAlignment::FlexEnd => TaffyAlignContent::FlexEnd,
        ContentAlignment::Center => TaffyAlignContent::Center,
        ContentAlignment::Stretch => TaffyAlignContent::Stretch,
        ContentAlignment::SpaceBetween => TaffyAlignContent::SpaceBetween,
        ContentAlignment::SpaceAround => TaffyAlignContent::SpaceAround,
        ContentAlignment::SpaceEvenly => TaffyAlignContent::SpaceEvenly,
    })
}

pub(super) fn item_alignment(value: ItemAlignment, _grid_inline_axis: bool) -> Option<TaffyAlignItems> {
    match value {
        ItemAlignment::Auto => None,
        ItemAlignment::Normal => Some(TaffyAlignItems::Stretch),
        ItemAlignment::Start | ItemAlignment::SelfStart => Some(TaffyAlignItems::Start),
        ItemAlignment::End | ItemAlignment::SelfEnd => Some(TaffyAlignItems::End),
        ItemAlignment::FlexStart => Some(TaffyAlignItems::FlexStart),
        ItemAlignment::FlexEnd => Some(TaffyAlignItems::FlexEnd),
        ItemAlignment::Center => Some(TaffyAlignItems::Center),
        ItemAlignment::Stretch => Some(TaffyAlignItems::Stretch),
        ItemAlignment::Baseline => Some(TaffyAlignItems::Baseline),
    }
}
