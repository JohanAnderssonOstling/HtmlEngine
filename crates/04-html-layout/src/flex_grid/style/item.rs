//! Translate item facts into dimensions, ratio constraints, and Taffy style.
use super::super::compatibility::{grid_percentage_stretch, intrinsic_column_ratio_basis};
use super::super::item_sizing::{
    flex_automatic_min_content_width, flex_replaced_automatic_minimum, taffy_item_width_dimension,
};
use super::super::sizing::{
    Axis, ContainerFacts, SizingPass, border_to_specified, content_to_specified,
    specified_to_content,
};
use super::super::tracks::{
    length_percentage_auto, placement as grid_placement, used_length_percentage,
};
use super::super::{TaffyContainerKind, finite_f32, length_dimension};
use super::conversion::{
    absolute_grid_inset, box_sizing, dimension, direction, item_alignment, overflow,
};
use crate::boxes::model::ResolvedBoxModel;
use crate::{
    LayoutEngine, clamp_replaced_definite_size_by_intrinsic_constraints,
    resolve_replaced_intrinsic_constraint,
};
use html_style_model::{
    ItemAlignment, LayoutStyle, PositionMode, UsedPreferredSize as PreferredSize, UsedStyleView,
};
use kurbo::Size;
use taffy::geometry::{Line, Point as TaffyPoint, Rect, Size as TaffySize};
use taffy::prelude::{Dimension, LengthPercentage, Style};
use taffy::style::Position as TaffyPosition;

pub(in crate::flex_grid) fn taffy_item_style(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    pass: SizingPass,
    containing_height: Option<f64>,
    container: ContainerFacts,
    resolved_flex_basis: Option<Dimension>,
    intrinsic_block_size: Option<f64>,
    stretch_height_basis: Option<f64>,
) -> Style {
    let style = engine.read.reader.style(box_idx);
    let box_model = engine.read.reader.box_model(box_idx, pass.width());
    let layout = engine.read.reader.layout_style(box_idx);
    let image_intrinsic = engine.read.reader.image_intrinsic(box_idx);
    let replaced_intrinsic = image_intrinsic.map(|intrinsic| intrinsic.size);
    let both_replaced_axes_auto = replaced_intrinsic.is_some()
        && matches!(style.width(), PreferredSize::Auto)
        && matches!(style.height(), PreferredSize::Auto);
    let aspect_ratio = if container.kind == TaffyContainerKind::Flex
        && replaced_intrinsic.is_some()
        && layout.flex_grow > 0.0
    {
        // Flexible replaced items transfer ratios during measurement, after flexing.
        None
    } else if style.aspect_ratio().uses_intrinsic() && image_intrinsic.is_some() {
        image_intrinsic
            .filter(|_| both_replaced_axes_auto)
            .and_then(|image| image.aspect_ratio)
            .map(finite_f32)
    } else {
        style.aspect_ratio().preferred()
    };
    let mut grid_stretch = grid_percentage_stretch(style, layout, container.kind);
    grid_stretch.width &=
        style.position() == PositionMode::Absolute || container.percentage_track_workaround;
    let item = ItemStyle {
        box_idx,
        pass,
        container,
        containing_height,
        stretch_height_basis,
        intrinsic_block_size,
        style,
        layout,
        box_model,
        replaced_intrinsic,
        aspect_ratio,
        grid_stretch,
    };
    let mut output = Style {
        size: TaffySize {
            width: item.width(engine),
            height: item.height(),
        },
        min_size: TaffySize {
            width: item.min_width(engine),
            height: item.min_height(engine),
        },
        max_size: TaffySize {
            width: item.inline_dimension(engine, item.style.max_width()),
            height: item.max_height(),
        },
        ..item.base_style(engine, resolved_flex_basis)
    };
    item.preserve_ratio(&mut output);
    output
}

struct ItemStyle<'a> {
    box_idx: usize,
    pass: SizingPass,
    container: ContainerFacts,
    containing_height: Option<f64>,
    stretch_height_basis: Option<f64>,
    intrinsic_block_size: Option<f64>,
    style: UsedStyleView<'a>,
    layout: &'a LayoutStyle,
    box_model: ResolvedBoxModel,
    replaced_intrinsic: Option<Size>,
    aspect_ratio: Option<f32>,
    grid_stretch: TaffySize<bool>,
}

impl ItemStyle<'_> {
    fn width(&self, engine: &mut LayoutEngine<'_, '_>) -> Dimension {
        if self.grid_stretch.width
            || (self.pass.is_intrinsic() && self.style.width().percentage_dependent())
        {
            // A cyclic percentage cross size is auto while finding a flex
            // container's intrinsic inline size. Leaving it as a percentage lets
            // the eventual container width feed back into its own contribution.
            Dimension::auto()
        } else if self.container.kind == TaffyContainerKind::Flex
            && matches!(self.style.width(), PreferredSize::Stretch)
        {
            // A column item stretches here only when its intrinsic contribution
            // established the line width; otherwise final cross-axis layout stretches it.
            let stretch = self.container.flex_row()
                || (self.pass.width() > 0.0
                    && (self.pass.width()
                        - crate::measurement::box_intrinsic_widths(engine, self.box_idx).0)
                        .abs()
                        <= 0.01);
            if stretch {
                self.inline_dimension(engine, PreferredSize::Stretch)
            } else {
                Dimension::auto()
            }
        } else {
            taffy_item_width_dimension(engine, self.box_idx, self.style.width(), self.pass)
        }
    }
    fn min_width(&self, engine: &mut LayoutEngine<'_, '_>) -> Dimension {
        if (self.container.kind == TaffyContainerKind::Flex
            && !self.container.flex_row()
            && matches!(self.style.width(), PreferredSize::Stretch))
            || matches!(self.style.min_width(), PreferredSize::Stretch)
        {
            // A wrapped column flex line may grow wider than its container. The
            // stretch size is therefore a floor during line measurement, not a
            // definite preferred width.
            self.stretch_dimension(Some(self.pass.width()), Axis::Horizontal)
                .unwrap_or_else(Dimension::auto)
        } else if self.container.kind == TaffyContainerKind::Flex
            && self.container.flex_row()
            && matches!(self.style.min_width(), PreferredSize::Auto)
        {
            if let Some(height_basis) = self.containing_height {
                let item_cross_inset = self.box_model.vertical_padding_border();
                // The helper disables this automatic-minimum contribution in book mode.
                if let Some(width) = crate::measurement::percentage_height_image_width(
                    engine,
                    self.box_idx,
                    (height_basis - item_cross_inset).max(0.0),
                ) {
                    return length_dimension(width);
                }
            }
            length_dimension(
                flex_replaced_automatic_minimum(
                    engine,
                    self.box_idx,
                    self.pass.width(),
                    self.containing_height,
                    self.container,
                )
                .unwrap_or_else(|| {
                    flex_automatic_min_content_width(engine, self.box_idx, self.pass.width())
                }),
            )
        } else if self.container.kind == TaffyContainerKind::Grid
            && matches!(self.style.width(), PreferredSize::FitContent)
            && matches!(self.style.min_width(), PreferredSize::Auto)
        {
            // A fit-content preferred size is neither auto nor dependent on
            // the containing block. CSS Grid therefore uses the min-content
            // contribution, even when it exceeds a fixed track maximum.
            taffy_item_width_dimension(engine, self.box_idx, PreferredSize::MinContent, self.pass)
        } else {
            taffy_item_width_dimension(engine, self.box_idx, self.style.min_width(), self.pass)
        }
    }
    fn height(&self) -> Dimension {
        if self.grid_stretch.height {
            Dimension::auto()
        } else if self.style.height().is_intrinsic_keyword()
            || (self.container.kind == TaffyContainerKind::Flex
                && !self.container.flex_row()
                && matches!(self.style.height(), PreferredSize::Stretch))
        {
            self.block_dimension(self.style.height())
        } else {
            clamp_replaced_definite_size_by_intrinsic_constraints(
                self.style.height(),
                self.intrinsic_height_constraint(self.style.min_height()),
                self.intrinsic_height_constraint(self.style.max_height()),
            )
            .map(length_dimension)
            .unwrap_or_else(|| dimension(self.style.height()))
        }
    }
    fn min_height(&self, engine: &mut LayoutEngine<'_, '_>) -> Dimension {
        if let Some(value) = self.intrinsic_height_constraint(self.style.min_height()) {
            length_dimension(value)
        } else if self.container.kind == TaffyContainerKind::Flex
            && !self.container.flex_row()
            && matches!(self.style.min_height(), PreferredSize::Auto)
        {
            if self.style.overflow_y().is_scrollable() {
                Dimension::length(0.0)
            } else {
                flex_replaced_automatic_minimum(
                    engine,
                    self.box_idx,
                    self.pass.width(),
                    self.containing_height,
                    self.container,
                )
                .map(length_dimension)
                .or_else(|| self.intrinsic_block_dimension())
                .unwrap_or_else(Dimension::auto)
            }
        } else if matches!(self.style.min_height(), PreferredSize::Percent(_))
            && self.containing_height.is_none()
        {
            Dimension::length(0.0)
        } else {
            self.block_dimension(self.style.min_height())
        }
    }
    fn inline_dimension(
        &self,
        engine: &mut LayoutEngine<'_, '_>,
        value: PreferredSize,
    ) -> Dimension {
        if matches!(value, PreferredSize::Stretch) {
            self.stretch_dimension(Some(self.pass.width()), Axis::Horizontal)
                .unwrap_or_else(Dimension::auto)
        } else {
            taffy_item_width_dimension(engine, self.box_idx, value, self.pass)
        }
    }
    fn max_height(&self) -> Dimension {
        self.intrinsic_height_constraint(self.style.max_height())
            .map(length_dimension)
            .unwrap_or_else(|| self.block_dimension(self.style.max_height()))
    }

    fn block_dimension(&self, value: PreferredSize) -> Dimension {
        match value {
            PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent => {
                self.intrinsic_block_dimension()
                    .unwrap_or_else(Dimension::auto)
            }
            PreferredSize::Stretch => self
                .stretch_dimension(self.stretch_height_basis, Axis::Vertical)
                .unwrap_or_else(Dimension::auto),
            _ => dimension(value),
        }
    }

    fn preserve_ratio(&self, output: &mut Style) {
        let flex_cross_stretches = self.container.kind == TaffyContainerKind::Flex
            && self.container.cross_axis_stretches(self.layout);
        let preserve_ratio_in_constraints = self.container.kind != TaffyContainerKind::Flex
            || (!flex_cross_stretches && self.layout.flex_grow == 0.0);
        if matches!(self.style.width(), PreferredSize::Auto)
            && matches!(self.style.height(), PreferredSize::Auto)
            && preserve_ratio_in_constraints
            && let Some(intrinsic) = self.replaced_intrinsic
        {
            let horizontal_inset = self.box_model.horizontal_padding_border();
            let vertical_inset = self.box_model.vertical_padding_border();
            let constraint_to_content = |value: PreferredSize, inset: f64, auto: f64| match value {
                PreferredSize::Auto => Some(auto),
                PreferredSize::Px(value) => Some(specified_to_content(
                    value.max(0.0) as f64,
                    inset,
                    self.style.box_sizing(),
                )),
                _ => None,
            };
            let min_w = constraint_to_content(self.style.min_width(), horizontal_inset, 0.0);
            let min_h = constraint_to_content(self.style.min_height(), vertical_inset, 0.0);
            let max_w =
                constraint_to_content(self.style.max_width(), horizontal_inset, f64::INFINITY);
            let max_h =
                constraint_to_content(self.style.max_height(), vertical_inset, f64::INFINITY);
            if let (Some(min_w), Some(min_h), Some(max_w), Some(max_h)) =
                (min_w, min_h, max_w, max_h)
                && intrinsic.width > 0.0
                && intrinsic.height > 0.0
            {
                let grow = (min_w / intrinsic.width)
                    .max(min_h / intrinsic.height)
                    .max(1.0);
                let shrink = (max_w / intrinsic.width)
                    .min(max_h / intrinsic.height)
                    .min(1.0);
                let (scale, target) = if grow > 1.0 && shrink >= 1.0 {
                    (grow, &mut output.min_size)
                } else if grow <= 1.0 && shrink < 1.0 {
                    (shrink, &mut output.max_size)
                } else {
                    return;
                };
                *target = TaffySize {
                    width: intrinsic.width,
                    height: intrinsic.height,
                }
                .zip_map(
                    TaffySize {
                        width: horizontal_inset,
                        height: vertical_inset,
                    },
                    |size, inset| {
                        length_dimension(content_to_specified(
                            size * scale,
                            inset,
                            self.style.box_sizing(),
                        ))
                    },
                );
            }
        }
    }
    fn base_style(
        &self,
        engine: &mut LayoutEngine<'_, '_>,
        resolved_flex_basis: Option<Dimension>,
    ) -> Style {
        let intrinsic_column_ratio_basis = intrinsic_column_ratio_basis(
            engine,
            self.box_idx,
            self.container,
            self.pass,
            self.aspect_ratio,
        );
        Style {
            direction: direction(self.style.direction()),
            // Absolute grid items still belong in Taffy's local tree: Taffy
            // excludes them from track sizing, then resolves their containing
            // area from grid-row/grid-column during its positioning pass.
            position: if self.style.position() == PositionMode::Absolute {
                TaffyPosition::Absolute
            } else {
                TaffyPosition::Relative
            },
            inset: Rect {
                left: (self.style.inset_left(), Axis::Horizontal),
                right: (self.style.inset_right(), Axis::Horizontal),
                top: (self.style.inset_top(), Axis::Vertical),
                bottom: (self.style.inset_bottom(), Axis::Vertical),
            }
            .map(|(value, axis)| {
                absolute_grid_inset(
                    value,
                    axis.select(
                        self.pass.width(),
                        self.containing_height.unwrap_or(self.pass.width()),
                    ),
                    self.style.position() == PositionMode::Absolute,
                    axis.select(self.grid_stretch.width, self.grid_stretch.height),
                )
            }),
            box_sizing: box_sizing(self.style.box_sizing()),
            margin: Rect {
                left: (self.style.margin_left(), self.layout.margin_left_auto),
                right: (self.style.margin_right(), self.layout.margin_right_auto),
                top: (self.style.margin_top(), self.layout.margin_top_auto),
                bottom: (self.style.margin_bottom(), self.layout.margin_bottom_auto),
            }
            .map(|(value, auto)| length_percentage_auto(value, auto, self.pass.width())),
            padding: Rect {
                left: self.style.padding_left(),
                right: self.style.padding_right(),
                top: self.style.padding_top(),
                bottom: self.style.padding_bottom(),
            }
            .map(|value| used_length_percentage(value, self.pass.width())),
            border: Rect {
                left: self.style.border_left_width(),
                right: self.style.border_right_width(),
                top: self.style.border_top_width(),
                bottom: self.style.border_bottom_width(),
            }
            .map(LengthPercentage::length),
            aspect_ratio: self.aspect_ratio,
            overflow: TaffyPoint {
                x: overflow(self.style.overflow_x()),
                y: overflow(self.style.overflow_y()),
            },
            flex_grow: self.layout.flex_grow,
            flex_shrink: self.layout.flex_shrink,
            flex_basis: resolved_flex_basis
                .or(intrinsic_column_ratio_basis)
                .unwrap_or_else(|| dimension(self.style.flex_basis())),
            // Taffy resolves percentage sizes of absolute grid items against the
            // whole container. A bare 100% size is exactly the grid-area stretch
            // size, so express that equivalence explicitly on the affected axis.
            align_self: if self.grid_stretch.height {
                item_alignment(ItemAlignment::Stretch, false)
            } else {
                item_alignment(
                    self.layout.align_self,
                    self.container.rtl
                        && self.container.kind == TaffyContainerKind::Flex
                        && !self.container.flex_row(),
                )
            },
            justify_self: if self.grid_stretch.width {
                item_alignment(ItemAlignment::Stretch, true)
            } else {
                item_alignment(self.layout.justify_self, self.container.rtl)
            },
            grid_row: Line {
                start: grid_placement(engine.read.reader.styles(), self.layout.grid_row.start),
                end: grid_placement(engine.read.reader.styles(), self.layout.grid_row.end),
            },
            grid_column: Line {
                start: grid_placement(engine.read.reader.styles(), self.layout.grid_column.start),
                end: grid_placement(engine.read.reader.styles(), self.layout.grid_column.end),
            },
            ..Style::default()
        }
    }
    fn stretch_dimension(&self, basis: Option<f64>, axis: Axis) -> Option<Dimension> {
        axis.stretch(basis, self.box_model, self.style.box_sizing())
            .map(length_dimension)
    }

    fn intrinsic_block_dimension(&self) -> Option<Dimension> {
        self.intrinsic_block_size.map(|size| {
            length_dimension(border_to_specified(
                size,
                self.box_model.vertical_padding_border(),
                self.style.box_sizing(),
            ))
        })
    }

    fn intrinsic_height_constraint(&self, value: PreferredSize) -> Option<f64> {
        self.replaced_intrinsic.and_then(|intrinsic| {
            resolve_replaced_intrinsic_constraint(
                value,
                intrinsic.height,
                self.box_model.vertical_padding_border(),
                self.style.box_sizing(),
            )
        })
    }
}
