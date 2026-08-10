use super::tracks::{fixed_auto_track_limit, length_percentage_auto, line_names as grid_line_names, placement as grid_placement, template_track as grid_template_track, track_size as grid_track_size, used_length_percentage};
use super::{TaffyContainerKind, finite_f32};
use crate::layout::{
    LayoutEngine, ReplacedFlexAutoMinInput, ReplacedMainAxis, ResolvedBoxModel, clamp_replaced_definite_size_by_intrinsic_constraints, measure_box_isolated, resolve_replaced_flex_auto_min_main_size, resolve_replaced_intrinsic_constraint,
};
use html_style_model::{ContentAlignment, GridAutoFlow, ItemAlignment, LayoutStyle, OverflowMode, PositionMode, UsedPreferredSize as PreferredSize};
use taffy::geometry::{Line, Point as TaffyPoint, Rect, Size as TaffySize};
use taffy::prelude::{Dimension, LengthPercentage, Style};
use taffy::style::{
    AlignContent as TaffyAlignContent, AlignItems as TaffyAlignItems, BoxSizing as TaffyBoxSizing, Display as TaffyDisplay, FlexDirection as TaffyFlexDirection, FlexWrap as TaffyFlexWrap, GridAutoFlow as TaffyGridAutoFlow,
    GridTemplateArea as TaffyGridTemplateArea, GridTemplateAreas as TaffyGridTemplateAreas, Overflow as TaffyOverflow, Position as TaffyPosition,
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
    session.reader.layout_style(box_idx)
}

pub(super) fn taffy_container_style(
    session: &LayoutEngine<'_, '_>, box_idx: usize, content_width: f64, explicit_height: Option<f64>, min_height: Option<f64>, max_height: Option<f64>, kind: TaffyContainerKind, floor_fixed_grid_column_maxima: bool,
    intrinsic_inline_sizing: bool,
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
        grid_template_rows: grid_template_rows.map(|track| grid_template_track(session.reader.styles(), &track, false, explicit_height, intrinsic_inline_sizing)).collect(),
        grid_template_columns: grid_template_columns
            .map(|track| grid_template_track(session.reader.styles(), &track, floor_fixed_grid_column_maxima, (!intrinsic_inline_sizing).then_some(content_width), intrinsic_inline_sizing))
            .collect(),
        grid_template_row_names: grid_line_names(session.reader.styles(), &source.grid_template_row_names),
        grid_template_column_names: grid_line_names(session.reader.styles(), &source.grid_template_column_names),
        grid_template_areas: (source.grid_template_area_rows > 0 && source.grid_template_area_columns > 0).then(|| TaffyGridTemplateAreas {
            areas: source
                .grid_template_areas
                .iter()
                .filter_map(|area| Some(TaffyGridTemplateArea { name: session.reader.styles().string(area.name)?.into(), row_start: area.row_start, row_end: area.row_end, column_start: area.column_start, column_end: area.column_end }))
                .collect(),
            row_count: source.grid_template_area_rows,
            column_count: source.grid_template_area_columns,
        }),
        grid_auto_rows: grid_auto_rows.map(|track| grid_track_size(&track, false, explicit_height, intrinsic_inline_sizing)).collect(),
        grid_auto_columns: grid_auto_columns.map(|track| grid_track_size(&track, floor_fixed_grid_column_maxima, (!intrinsic_inline_sizing).then_some(content_width), intrinsic_inline_sizing)).collect(),
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
    let size_contained_intrinsic_minimum = containing_width <= 0.01
        && session.reader.style(container_idx).size_containment()
        && session.reader.style(container_idx).grid_template_columns().any(|component| match component {
            html_style_model::UsedGridTemplateTrack::Single(track) => intrinsic_minimum_has_definite_maximum(track),
            html_style_model::UsedGridTemplateTrack::Repeat { tracks, .. } => tracks.iter().any(intrinsic_minimum_has_definite_maximum),
        });
    if size_contained_intrinsic_minimum {
        return true;
    }
    let Some(track_limit) = fixed_auto_track_limit(session.reader.style(container_idx).grid_template_columns(), containing_width) else {
        return false;
    };
    children.iter().any(|&child| {
        let style = session.reader.style(child as usize);
        // Percentage item insets are cyclic at this track-sizing stage: their
        // definite lower-bound contribution is zero until the grid area is
        // known. Resolving them against the whole container would incorrectly
        // turn `minmax(auto, fixed)` into a fit-content track. A zero basis
        // retains fixed and calc-absolute terms without inventing a percentage
        // contribution from the container rather than the item grid area.
        let margin = style.get_horizontal_margin(0.0);
        let padding_border = style.get_horizontal_padding(0.0) + style.border_left_width() as f64 + style.border_right_width() as f64;
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

fn intrinsic_minimum_has_definite_maximum(track: html_style_model::UsedGridTrackSize) -> bool {
    matches!(
        track,
        html_style_model::UsedGridTrackSize::MinMax {
            min: html_style_model::UsedGridTrackBreadth::MinContent | html_style_model::UsedGridTrackBreadth::MaxContent,
            max: html_style_model::UsedGridTrackBreadth::Length(_),
        }
    )
}

pub(super) fn taffy_item_style(
    session: &LayoutEngine<'_, '_>, box_idx: usize, containing_width: f64, containing_height: Option<f64>, kind: TaffyContainerKind, resolved_flex_basis: Option<Dimension>, intrinsic_block_size: Option<f64>, stretch_height_basis: Option<f64>, intrinsic_inline_sizing: bool,
) -> Style {
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
    let horizontal_main_axis = session.reader.get_parent(box_idx).map(|parent| matches!(layout_style(session, parent).flex_direction, html_style_model::FlexDirection::Row | html_style_model::FlexDirection::RowReverse)).unwrap_or(true);
    let stretch_dimension = |basis: Option<f64>, horizontal: bool| {
        let basis = basis?;
        let (margin, padding_border) = if horizontal {
            (box_model.horizontal_margin(), box_model.horizontal_padding_border())
        } else {
            (box_model.vertical_margin(), box_model.vertical_padding_border())
        };
        let border_size = (basis - margin).max(padding_border);
        let specified = match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => (border_size - padding_border).max(0.0),
            html_style_model::BoxSizing::BorderBox => border_size,
        };
        Some(Dimension::length(finite_f32(specified)))
    };
    let intrinsic_block_dimension = || {
        intrinsic_block_size.map(|border_size| match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => (border_size - box_model.vertical_padding_border()).max(0.0),
            html_style_model::BoxSizing::BorderBox => border_size.max(box_model.vertical_padding_border()),
        })
        .map(finite_f32)
        .map(Dimension::length)
    };
    let taffy_handles_absolute_grid_item = kind == TaffyContainerKind::Grid
        && style.position() == PositionMode::Absolute
        && session.reader.get_parent(box_idx).is_some_and(|parent| matches!(session.reader.style(parent).width(), PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent));
    let taffy_handles_absolute_flex_item = kind == TaffyContainerKind::Flex && style.position() == PositionMode::Absolute;
    let taffy_handles_absolute_item = taffy_handles_absolute_grid_item || taffy_handles_absolute_flex_item;
    let percentage_width_is_grid_area_stretch = grid_percentage_width_is_stretch_equivalent(session, box_idx, kind)
        && (taffy_handles_absolute_grid_item || grid_percentage_width_needs_intrinsic_track_workaround(session, box_idx));
    let absolute_percentage_width_is_grid_area_stretch = taffy_handles_absolute_grid_item && percentage_width_is_grid_area_stretch;
    let percentage_height_is_grid_area_stretch = grid_percentage_height_is_stretch_equivalent(session, box_idx, kind);
    let width = if percentage_width_is_grid_area_stretch {
        Dimension::auto()
    } else if intrinsic_inline_sizing && style.width().percentage_dependent() {
        // A cyclic percentage cross size is auto while finding a flex
        // container's intrinsic inline size. Leaving it as a percentage lets
        // the eventual container width feed back into its own contribution.
        Dimension::auto()
    } else if kind == TaffyContainerKind::Flex && horizontal_main_axis && matches!(style.width(), PreferredSize::Stretch) {
        stretch_dimension(Some(containing_width), true).unwrap_or_else(Dimension::auto)
    } else if kind == TaffyContainerKind::Flex && !horizontal_main_axis && matches!(style.width(), PreferredSize::Stretch) && containing_width > 0.0 {
        let intrinsic_outer = crate::layout::box_intrinsic_widths(session, box_idx).0;
        // If this item's intrinsic contribution established the column flex
        // container's min-content width, `stretch` is definite during the
        // measure pass. Leaving it auto makes Taffy establish the line from
        // that border box and then add the same border once more while
        // stretching. A line made wider by another item stays auto so the
        // final cross-size can still enlarge this item.
        if (containing_width - intrinsic_outer).abs() <= 0.01 { stretch_dimension(Some(containing_width), true).unwrap_or_else(Dimension::auto) } else { Dimension::auto() }
    } else {
        taffy_item_width_dimension(session, box_idx, style.width(), containing_width, intrinsic_inline_sizing)
    };
    let mut min_width = if kind == TaffyContainerKind::Flex && !horizontal_main_axis && matches!(style.width(), PreferredSize::Stretch) {
        // A wrapped column flex line may grow wider than its container. The
        // stretch size is therefore a floor during line measurement, not a
        // definite preferred width.
        stretch_dimension(Some(containing_width), true).unwrap_or_else(Dimension::auto)
    } else if matches!(style.min_width(), PreferredSize::Stretch) {
        stretch_dimension(Some(containing_width), true).unwrap_or_else(Dimension::auto)
    } else if kind == TaffyContainerKind::Flex && horizontal_main_axis && matches!(style.min_width(), PreferredSize::Auto) {
        Dimension::length(finite_f32(flex_replaced_automatic_minimum(session, box_idx, containing_width, containing_height, true).unwrap_or_else(|| flex_automatic_min_content_width(session, box_idx, containing_width))))
    } else if kind == TaffyContainerKind::Grid && matches!(style.width(), PreferredSize::FitContent) && matches!(style.min_width(), PreferredSize::Auto) {
        // A fit-content preferred size is neither auto nor dependent on
        // the containing block. CSS Grid therefore uses the min-content
        // contribution, even when it exceeds a fixed track maximum.
        taffy_item_width_dimension(session, box_idx, PreferredSize::MinContent, containing_width, intrinsic_inline_sizing)
    } else {
        taffy_item_width_dimension(session, box_idx, style.min_width(), containing_width, intrinsic_inline_sizing)
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
    let height = if percentage_height_is_grid_area_stretch {
        Dimension::auto()
    } else if matches!(style.height(), PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent) {
        intrinsic_block_dimension().unwrap_or_else(Dimension::auto)
    } else if kind == TaffyContainerKind::Flex && !horizontal_main_axis && matches!(style.height(), PreferredSize::Stretch) {
        stretch_dimension(stretch_height_basis, false).unwrap_or_else(Dimension::auto)
    } else {
        clamp_replaced_definite_size_by_intrinsic_constraints(style.height(), intrinsic_min_height, intrinsic_max_height).map(finite_f32).map(Dimension::length).unwrap_or_else(|| dimension(style.height()))
    };
    let mut min_height = if let Some(value) = intrinsic_min_height {
        Dimension::length(finite_f32(value))
    } else if kind == TaffyContainerKind::Flex && !horizontal_main_axis && matches!(style.min_height(), PreferredSize::Auto) {
        if matches!(style.overflow_y(), OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto) {
            Dimension::length(0.0)
        } else {
            flex_replaced_automatic_minimum(session, box_idx, containing_width, containing_height, false)
                .map(finite_f32)
                .map(Dimension::length)
                .or_else(intrinsic_block_dimension)
                .unwrap_or_else(Dimension::auto)
        }
    } else {
        match style.min_height() {
            PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => intrinsic_block_dimension().unwrap_or_else(Dimension::auto),
            PreferredSize::Stretch => stretch_dimension(stretch_height_basis, false).unwrap_or_else(Dimension::auto),
            PreferredSize::Percent(_) if containing_height.is_none() => Dimension::length(0.0),
            value => dimension(value),
        }
    };
    let mut max_width = taffy_item_width_dimension(session, box_idx, style.max_width(), containing_width, intrinsic_inline_sizing);
    if matches!(style.max_width(), PreferredSize::Stretch) {
        max_width = stretch_dimension(Some(containing_width), true).unwrap_or_else(Dimension::auto);
    }
    let mut max_height = intrinsic_max_height.map(finite_f32).map(Dimension::length).unwrap_or_else(|| match style.max_height() {
        PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => intrinsic_block_dimension().unwrap_or_else(Dimension::auto),
        PreferredSize::Stretch => stretch_dimension(stretch_height_basis, false).unwrap_or_else(Dimension::auto),
        value => dimension(value),
    });
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
    let intrinsic_column_ratio_basis = (kind == TaffyContainerKind::Flex
        && intrinsic_inline_sizing
        && !horizontal_main_axis
        && matches!(style.flex_basis(), PreferredSize::Auto)
        && matches!(style.height(), PreferredSize::Auto)
        && style.width().percentage_dependent())
    .then(|| {
        let ratio = aspect_ratio?;
        let (_, max_content_width) = crate::layout::box_content_intrinsic_widths(session, box_idx);
        (ratio > 0.0).then(|| Dimension::length(finite_f32(max_content_width / f64::from(ratio))))
    })
    .flatten();
    Style {
        // Absolute grid items still belong in Taffy's local tree: Taffy
        // excludes them from track sizing, then resolves their containing
        // area from grid-row/grid-column during its positioning pass.
        position: if taffy_handles_absolute_item { TaffyPosition::Absolute } else { TaffyPosition::Relative },
        inset: Rect {
            left: absolute_grid_inset(style.inset_left(), containing_width, taffy_handles_absolute_item, absolute_percentage_width_is_grid_area_stretch),
            right: absolute_grid_inset(style.inset_right(), containing_width, taffy_handles_absolute_item, absolute_percentage_width_is_grid_area_stretch),
            top: absolute_grid_inset(style.inset_top(), containing_height.unwrap_or(containing_width), taffy_handles_absolute_item, percentage_height_is_grid_area_stretch),
            bottom: absolute_grid_inset(style.inset_bottom(), containing_height.unwrap_or(containing_width), taffy_handles_absolute_item, percentage_height_is_grid_area_stretch),
        },
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
        flex_basis: resolved_flex_basis.or(intrinsic_column_ratio_basis).unwrap_or_else(|| dimension(style.flex_basis())),
        // Taffy resolves percentage sizes of absolute grid items against the
        // whole container. A bare 100% size is exactly the grid-area stretch
        // size, so express that equivalence explicitly on the affected axis.
        align_self: if percentage_height_is_grid_area_stretch { item_alignment(ItemAlignment::Stretch, false) } else { item_alignment(layout.align_self, false) },
        justify_self: if percentage_width_is_grid_area_stretch { item_alignment(ItemAlignment::Stretch, true) } else { item_alignment(layout.justify_self, true) },
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
        return Some(taffy_item_width_dimension(session, box_idx, basis, containing_width, false));
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
        PreferredSize::Stretch => Some((containing_width - style.get_horizontal_margin(containing_width) - padding_border).max(0.0)),
        PreferredSize::Auto => None,
    };
    let mut automatic_min = preferred.map_or(content_min, |preferred| content_min.min(preferred));
    if let PreferredSize::Px(value) = style.max_width() {
        automatic_min = automatic_min.min(as_content_width(value as f64));
    } else if let PreferredSize::Percent(value) = style.max_width() {
        automatic_min = automatic_min.min(as_content_width(containing_width * value as f64));
    } else if matches!(style.max_width(), PreferredSize::Stretch) {
        automatic_min = automatic_min.min((containing_width - style.get_horizontal_margin(containing_width) - padding_border).max(0.0));
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

/// An otherwise unconstrained 100% grid item is equivalent to the grid
/// item's default stretch behavior when the adapter needs to avoid a cyclic
/// percentage contribution.
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

/// Taffy 0.13 resolves percentages correctly for fixed and flexible tracks.
/// A percentage item in `minmax(auto, <fixed>)` still feeds the unresolved
/// percentage back into the intrinsic minimum, however. Keep the old
/// stretch-equivalent encoding only for that remaining Taffy cycle.
fn grid_percentage_width_needs_intrinsic_track_workaround(session: &LayoutEngine<'_, '_>, box_idx: usize) -> bool {
    let Some(parent) = session.reader.get_parent(box_idx) else {
        return false;
    };
    session.reader.style(parent).grid_template_columns().any(|track| {
        matches!(
            track,
            html_style_model::UsedGridTemplateTrack::Single(html_style_model::UsedGridTrackSize::MinMax {
                min: html_style_model::UsedGridTrackBreadth::Auto,
                max: html_style_model::UsedGridTrackBreadth::Length(_),
            })
        )
    })
}

fn grid_percentage_height_is_stretch_equivalent(session: &LayoutEngine<'_, '_>, box_idx: usize, kind: TaffyContainerKind) -> bool {
    if kind != TaffyContainerKind::Grid {
        return false;
    }
    let style = session.reader.style(box_idx);
    let layout = layout_style(session, box_idx);
    matches!(style.height(), PreferredSize::Percent(value) if (value - 1.0).abs() <= f32::EPSILON)
        && matches!(style.min_height(), PreferredSize::Auto)
        && matches!(style.max_height(), PreferredSize::Auto)
        && !layout.margin_top_auto
        && !layout.margin_bottom_auto
        && style.margin_top().is_zero()
        && style.margin_bottom().is_zero()
        && style.padding_top().is_zero()
        && style.padding_bottom().is_zero()
        && style.border_top_width() == 0.0
        && style.border_bottom_width() == 0.0
}

fn optional_length_percentage_auto(value: Option<html_style_model::UsedLengthPct>, percentage_basis: f64) -> taffy::prelude::LengthPercentageAuto {
    value.map_or_else(taffy::prelude::LengthPercentageAuto::auto, |value| length_percentage_auto(value, false, percentage_basis))
}

fn absolute_grid_inset(value: Option<html_style_model::UsedLengthPct>, percentage_basis: f64, taffy_handles_absolute: bool, stretch_area: bool) -> taffy::prelude::LengthPercentageAuto {
    if !taffy_handles_absolute {
        return taffy::prelude::LengthPercentageAuto::auto();
    }
    if stretch_area && value.is_none() {
        taffy::prelude::LengthPercentageAuto::length(0.0)
    } else {
        optional_length_percentage_auto(value, percentage_basis)
    }
}

fn taffy_item_width_dimension(session: &LayoutEngine<'_, '_>, box_idx: usize, value: PreferredSize, containing_width: f64, intrinsic_inline_sizing: bool) -> Dimension {
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
        PreferredSize::FitContent if intrinsic_inline_sizing => max,
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
        ContentAlignment::Normal if kind == TaffyContainerKind::Flex && main_axis => TaffyAlignContent::FLEX_START,
        ContentAlignment::Normal => TaffyAlignContent::STRETCH,
        ContentAlignment::Start => TaffyAlignContent::START,
        ContentAlignment::End => TaffyAlignContent::END,
        ContentAlignment::FlexStart => TaffyAlignContent::FLEX_START,
        ContentAlignment::FlexEnd => TaffyAlignContent::FLEX_END,
        ContentAlignment::Center => TaffyAlignContent::CENTER,
        ContentAlignment::Stretch => TaffyAlignContent::STRETCH,
        ContentAlignment::SpaceBetween => TaffyAlignContent::SPACE_BETWEEN,
        ContentAlignment::SpaceAround => TaffyAlignContent::SPACE_AROUND,
        ContentAlignment::SpaceEvenly => TaffyAlignContent::SPACE_EVENLY,
    })
}

pub(super) fn item_alignment(value: ItemAlignment, _grid_inline_axis: bool) -> Option<TaffyAlignItems> {
    match value {
        ItemAlignment::Auto => None,
        ItemAlignment::Normal => Some(TaffyAlignItems::STRETCH),
        ItemAlignment::Start | ItemAlignment::SelfStart | ItemAlignment::Left => Some(TaffyAlignItems::START),
        ItemAlignment::End | ItemAlignment::SelfEnd | ItemAlignment::Right => Some(TaffyAlignItems::END),
        ItemAlignment::FlexStart => Some(TaffyAlignItems::FLEX_START),
        ItemAlignment::FlexEnd => Some(TaffyAlignItems::FLEX_END),
        ItemAlignment::Center => Some(TaffyAlignItems::CENTER),
        ItemAlignment::Stretch => Some(TaffyAlignItems::STRETCH),
        ItemAlignment::Baseline => Some(TaffyAlignItems::BASELINE),
    }
}
