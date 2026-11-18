//! Shared container facts and physical-axis sizing conversions.
use super::TaffyContainerKind;
use crate::LayoutReader;
use crate::boxes::model::ResolvedBoxModel;
use html_style_model::{
    BoxSizing, FlexDirection, FlexWrap, ItemAlignment, LayoutStyle, TextDirection,
};

#[derive(Clone, Copy)]
pub(super) struct ContainerFacts {
    pub kind: TaffyContainerKind,
    pub main_axis: Axis,
    pub rtl: bool,
    pub wrap: FlexWrap,
    pub align_items: ItemAlignment,
    pub percentage_track_workaround: bool,
}

impl ContainerFacts {
    pub fn new(reader: &LayoutReader<'_>, owner: usize, kind: TaffyContainerKind) -> Self {
        let layout = reader.layout_style(owner);
        Self {
            kind,
            main_axis: if matches!(
                layout.flex_direction,
                FlexDirection::Row | FlexDirection::RowReverse
            ) {
                Axis::Horizontal
            } else {
                Axis::Vertical
            },
            rtl: reader.style(owner).direction() == TextDirection::Rtl,
            wrap: layout.flex_wrap,
            align_items: layout.align_items,
            percentage_track_workaround: kind == TaffyContainerKind::Grid
                && super::compatibility::grid_percentage_track_workaround(reader.style(owner)),
        }
    }

    pub fn flex_row(self) -> bool {
        self.main_axis == Axis::Horizontal
    }

    pub fn alignment(self, item: &LayoutStyle) -> ItemAlignment {
        if item.align_self == ItemAlignment::Auto {
            self.align_items
        } else {
            item.align_self
        }
    }

    pub fn cross_axis_stretches(self, item: &LayoutStyle) -> bool {
        !self.main_axis.cross().auto_margin(item)
            && matches!(
                self.alignment(item),
                ItemAlignment::Normal | ItemAlignment::Stretch
            )
    }
}

/// Contained grids retain a definite root/gap width while resolving cyclic
/// tracks as intrinsic. Ordinary intrinsic probes have no inline-size basis.
#[derive(Clone, Copy)]
pub(super) enum SizingPass {
    Intrinsic,
    Final(f64),
    Contained(f64),
}

impl SizingPass {
    pub fn width(self) -> f64 {
        match self {
            Self::Intrinsic => 0.0,
            Self::Final(width) | Self::Contained(width) => width,
        }
    }

    pub fn is_intrinsic(self) -> bool {
        !matches!(self, Self::Final(_))
    }

    pub fn inline_basis(self) -> Option<f64> {
        match self {
            Self::Final(width) => Some(width),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    pub fn cross(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }

    pub fn select<T>(self, horizontal: T, vertical: T) -> T {
        match self {
            Self::Horizontal => horizontal,
            Self::Vertical => vertical,
        }
    }

    pub fn margin(self, model: ResolvedBoxModel) -> f64 {
        self.select(model.horizontal_margin(), model.vertical_margin())
    }

    pub fn inset(self, model: ResolvedBoxModel) -> f64 {
        self.select(
            model.horizontal_padding_border(),
            model.vertical_padding_border(),
        )
    }

    pub fn auto_margin(self, layout: &LayoutStyle) -> bool {
        self.select(
            layout.margin_left_auto || layout.margin_right_auto,
            layout.margin_top_auto || layout.margin_bottom_auto,
        )
    }

    pub fn stretch(
        self,
        basis: Option<f64>,
        model: ResolvedBoxModel,
        sizing: BoxSizing,
    ) -> Option<f64> {
        Some(border_to_specified(
            (basis? - self.margin(model)).max(self.inset(model)),
            self.inset(model),
            sizing,
        ))
    }
}

pub(super) fn border_to_specified(border: f64, inset: f64, sizing: BoxSizing) -> f64 {
    match sizing {
        BoxSizing::ContentBox => (border - inset).max(0.0),
        BoxSizing::BorderBox => border.max(inset),
    }
}

pub(super) fn content_to_specified(content: f64, inset: f64, sizing: BoxSizing) -> f64 {
    content
        + if sizing == BoxSizing::BorderBox {
            inset
        } else {
            0.0
        }
}

pub(super) fn specified_to_content(value: f64, inset: f64, sizing: BoxSizing) -> f64 {
    (value
        - if sizing == BoxSizing::BorderBox {
            inset
        } else {
            0.0
        })
    .max(0.0)
}
