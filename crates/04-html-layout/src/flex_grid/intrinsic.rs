use super::item_sizing::ordered_children;
use super::measurement::MeasurementMode;
use super::sizing::{ContainerFacts, SizingPass};
use super::style::{
    grid_definite_inline_minimum_exceeds_track_limit, taffy_container_style, taffy_item_style,
};
use super::taffy_layout::TaffyLayoutTree;
use super::tracks::template_tracks_have_percentage_calc;
use super::{TaffyContainerKind, ensure_grid_algorithm_root, finite_f32};
use crate::{LayoutEngine, resolve_definite_outer_inline_size};
use html_style_model::UsedPreferredSize as PreferredSize;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::{AvailableSpace, Dimension, Style};

struct FlexIntrinsicItem {
    min_contribution: f64,
    max_contribution: f64,
    outer_flex_base: f64,
    outer_min: f64,
    outer_max: f64,
    flex_grow: f64,
}

/// Intrinsic contributions with explicit optional inline and block-size bases.
pub fn intrinsic_widths(
    engine: &mut LayoutEngine<'_, '_>,
    container_idx: usize,
    children: &[u32],
    kind: TaffyContainerKind,
    available_width: Option<f64>,
    definite_height: Option<f64>,
) -> (f64, f64) {
    let reader = engine.read.reader;
    let size_containment =
        kind == TaffyContainerKind::Grid && reader.style(container_idx).size_containment();
    let sizing_children = if size_containment { &[][..] } else { children };
    let definite_height = definite_height.or_else(|| {
        let used = reader.style(container_idx);
        let inline_basis = available_width.unwrap_or(0.0);
        let border_box_inset = if used.box_sizing() == html_style_model::BoxSizing::BorderBox {
            used.get_vertical_padding_border(inline_basis)
        } else {
            0.0
        };
        crate::resolve_vertical_size(used.height(), None, border_box_inset)
    });
    let container = ContainerFacts::new(&reader, container_idx, kind);
    if kind == TaffyContainerKind::Flex && container.flex_row() {
        return row_flex_intrinsic_widths(
            engine,
            children,
            container.wrap,
            reader.style(container_idx).column_gap().resolve(0.0),
        );
    }
    let ordered = ordered_children(engine, sizing_children);
    let probe = IntrinsicProbe {
        container_idx,
        children: &ordered,
        container,
        available_width,
    };
    let (mut min_width, max_width) = probe.widths(engine, definite_height);
    if kind == TaffyContainerKind::Grid {
        let fit_content_floor = sizing_children
            .iter()
            .filter(|&&child| {
                matches!(
                    reader.style(child as usize).width(),
                    PreferredSize::FitContent
                )
            })
            .map(|&child| crate::measurement::box_intrinsic_widths(engine, child as usize).0)
            .fold(0.0f64, f64::max);
        min_width = min_width.max(fit_content_floor);
    }
    (min_width, max_width.max(min_width))
}

struct IntrinsicProbe<'a> {
    container_idx: usize,
    children: &'a [u32],
    container: ContainerFacts,
    available_width: Option<f64>,
}

impl IntrinsicProbe<'_> {
    fn widths(
        &self,
        engine: &mut LayoutEngine<'_, '_>,
        definite_height: Option<f64>,
    ) -> (f64, f64) {
        let (root, children) = self.prepare(engine, None);
        let mut tree = TaffyLayoutTree::solve(
            MeasurementMode::Intrinsic(engine),
            self.container.kind,
            root,
            children,
            TaffySize {
                width: AvailableSpace::MinContent,
                height: AvailableSpace::MaxContent,
            },
        );
        let minimum = tree.root_layout().size.width.max(0.0) as f64;
        // With no definite block size, both probes use identical styles and
        // can reuse the entire adapter tree. A definite max-content block size
        // can change item styles and column wrapping, so prepare it separately.
        let Some(height) = definite_height else {
            tree.compute(TaffySize {
                width: AvailableSpace::MaxContent,
                height: AvailableSpace::MaxContent,
            });
            return (minimum, tree.root_layout().size.width.max(0.0) as f64);
        };
        drop(tree);
        let (root, children) = self.prepare(engine, Some(height));
        let tree = TaffyLayoutTree::solve(
            MeasurementMode::Intrinsic(engine),
            self.container.kind,
            root,
            children,
            TaffySize {
                width: AvailableSpace::MaxContent,
                height: AvailableSpace::Definite(finite_f32(height)),
            },
        );
        (minimum, tree.root_layout().size.width.max(0.0) as f64)
    }

    fn prepare(
        &self,
        engine: &mut LayoutEngine<'_, '_>,
        probe_height: Option<f64>,
    ) -> (Style, Vec<(Style, Option<u32>)>) {
        let mut child_styles: Vec<_> = self
            .children
            .iter()
            .map(|&child| {
                (
                    taffy_item_style(
                        engine,
                        child as usize,
                        SizingPass::Intrinsic,
                        probe_height,
                        self.container,
                        None,
                        None,
                        probe_height,
                    ),
                    Some(child),
                )
            })
            .collect();
        let has_indefinite_calc_track = self.container.kind == TaffyContainerKind::Grid
            && self.available_width.is_none()
            && template_tracks_have_percentage_calc(
                engine
                    .read
                    .reader
                    .style(self.container_idx)
                    .grid_template_columns(),
            );
        if !has_indefinite_calc_track {
            ensure_grid_algorithm_root(&mut child_styles, self.container.kind);
        }
        let floor_fixed_grid_column_maxima = grid_definite_inline_minimum_exceeds_track_limit(
            engine,
            self.container_idx,
            self.children,
            self.container.kind,
            0.0,
        );
        let mut root_style = taffy_container_style(
            engine,
            self.container_idx,
            SizingPass::Intrinsic,
            probe_height,
            None,
            None,
            self.container,
            floor_fixed_grid_column_maxima,
        );
        root_style.size.width = Dimension::auto();
        if self.container.kind == TaffyContainerKind::Grid {
            super::compatibility::adjust_intrinsic_grid_root(
                engine,
                self.container_idx,
                self.children,
                self.available_width,
                &mut root_style,
            );
        }
        (root_style, child_styles)
    }
}

fn row_flex_intrinsic_widths(
    engine: &mut LayoutEngine<'_, '_>,
    children: &[u32],
    flex_wrap: html_style_model::FlexWrap,
    gap: f64,
) -> (f64, f64) {
    let items: Vec<_> = children
        .iter()
        .map(|&child| flex_intrinsic_item(engine, child as usize))
        .collect();
    if items.is_empty() {
        return (0.0, 0.0);
    }
    let line_gaps = gap.max(0.0) * items.len().saturating_sub(1) as f64;
    let max_width = flex_intrinsic_line_width(&items, false) + line_gaps;
    let min_width = if matches!(flex_wrap, html_style_model::FlexWrap::NoWrap) {
        flex_intrinsic_line_width(&items, true) + line_gaps
    } else {
        items
            .iter()
            .map(|item| item.min_contribution)
            .fold(0.0f64, f64::max)
    };
    (min_width.max(0.0), max_width.max(min_width))
}

fn flex_intrinsic_item(engine: &mut LayoutEngine<'_, '_>, box_idx: usize) -> FlexIntrinsicItem {
    let style = engine.read.reader.style(box_idx);
    let layout = engine.read.reader.layout_style(box_idx);
    let margin = style.get_horizontal_margin(0.0);
    let padding_border = style.get_horizontal_padding_border(0.0);
    let outer_inset = margin + padding_border;
    let (content_min, content_max) =
        crate::measurement::box_content_intrinsic_widths(engine, box_idx, None);
    let raw_min = (content_min + outer_inset).max(0.0);
    let raw_max = (content_max + outer_inset).max(raw_min);
    let outer_width = |value| match value {
        PreferredSize::MinContent => Some(raw_min),
        PreferredSize::Content | PreferredSize::MaxContent | PreferredSize::FitContent => {
            Some(raw_max)
        }
        _ => resolve_definite_outer_inline_size(value, style.box_sizing(), padding_border, margin),
    };
    let outer_preferred = outer_width(style.width());
    let outer_flex_base = match style.flex_basis() {
        PreferredSize::Auto => outer_preferred,
        value => outer_width(value),
    }
    .unwrap_or(raw_max);
    let automatic_min = if style.overflow_x().is_scrollable() {
        margin
    } else {
        outer_preferred.map_or(raw_min, |preferred| raw_min.min(preferred))
    };
    let outer_min = match style.min_width() {
        PreferredSize::Auto => automatic_min,
        value => outer_width(value).unwrap_or(0.0),
    };
    let outer_max = match style.max_width() {
        PreferredSize::Auto | PreferredSize::Percent(_) | PreferredSize::Stretch => f64::INFINITY,
        value => outer_width(value).unwrap_or(f64::INFINITY),
    }
    .max(outer_min);
    let constraint_dependent_basis = style.flex_basis().percentage_dependent()
        || matches!(
            style.flex_basis(),
            PreferredSize::Content | PreferredSize::FitContent | PreferredSize::Stretch
        );
    let contribution = |raw: f64, min_content: bool| {
        let mut value = outer_preferred.map_or(raw, |preferred| raw.max(preferred));
        if layout.flex_grow == 0.0 {
            value = value.min(outer_flex_base);
        }
        if layout.flex_shrink == 0.0 && !(min_content && constraint_dependent_basis) {
            value = value.max(outer_flex_base);
        }
        value.clamp(outer_min, outer_max)
    };
    FlexIntrinsicItem {
        min_contribution: contribution(raw_min, true),
        max_contribution: contribution(raw_max, false),
        outer_flex_base,
        outer_min,
        outer_max,
        flex_grow: layout.flex_grow as f64,
    }
}

fn flex_intrinsic_line_width(items: &[FlexIntrinsicItem], min_content: bool) -> f64 {
    let mut compatible_width = 0.0;
    let mut chosen_fraction = f64::NEG_INFINITY;
    let mut grow_sum = 0.0;
    for item in items {
        let contribution = if min_content {
            item.min_contribution
        } else {
            item.max_contribution
        };
        compatible_width += contribution;
        grow_sum += item.flex_grow;
        let free_space = contribution - item.outer_flex_base;
        let fraction = if free_space > 0.0 {
            if item.flex_grow >= 1.0 {
                free_space / item.flex_grow
            } else {
                free_space * item.flex_grow
            }
        } else {
            free_space
        };
        chosen_fraction = chosen_fraction.max(fraction);
    }
    if chosen_fraction <= 0.0 || !chosen_fraction.is_finite() {
        return compatible_width;
    }
    let chosen_fraction = if grow_sum > 0.0 && grow_sum < 1.0 {
        chosen_fraction / grow_sum
    } else {
        chosen_fraction
    };
    items
        .iter()
        .map(|item| {
            (item.outer_flex_base + item.flex_grow * chosen_fraction)
                .clamp(item.outer_min, item.outer_max)
        })
        .sum()
}
