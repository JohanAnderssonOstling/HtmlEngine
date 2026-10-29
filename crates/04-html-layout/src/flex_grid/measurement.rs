use super::finite_f32;
use crate::layout::{
    BoxLayoutRequest, LayoutEngine, ResolvedBoxModel, measure_box_isolated, preferred_aspect_ratio,
};
use kurbo::Size;
use rustc_data_structures::fx::FxHashMap;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::AvailableSpace;

type TaffyMeasureKey = (usize, u32, u32, u32, u32, u32, u32);

#[derive(Default)]
pub struct FlexGridState {
    measure_cache: FxHashMap<TaffyMeasureKey, MeasuredItem>,
}

#[derive(Clone, Copy)]
pub(super) struct MeasuredItem {
    pub(super) size: TaffySize<f32>,
    pub(super) first_baseline: Option<f64>,
}

impl FlexGridState {
    pub fn clear(&mut self) {
        self.measure_cache.clear();
    }

    fn cached_measurement(&self, key: &TaffyMeasureKey) -> Option<MeasuredItem> {
        self.measure_cache.get(key).copied()
    }

    fn cache_measurement(&mut self, key: TaffyMeasureKey, measured: MeasuredItem) {
        self.measure_cache.insert(key, measured);
    }
}

pub(super) fn measure_item_with_baseline(
    session: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    known: TaffySize<Option<f32>>,
    available: TaffySize<AvailableSpace>,
) -> MeasuredItem {
    let timing_started = session.start_timing();
    if let Some(size) = measure_replaced_item(session, box_idx, known, available) {
        session.record_timing(|timings| timings.measure_flex_grid_item += timing_started.elapsed());
        return MeasuredItem {
            size,
            first_baseline: None,
        };
    }
    let (available_width_tag, available_width_bits) = available_space_key(available.width);
    let (available_height_tag, available_height_bits) = available_space_key(available.height);
    let key = (
        box_idx,
        known.width.map(f32::to_bits).unwrap_or(u32::MAX),
        known.height.map(f32::to_bits).unwrap_or(u32::MAX),
        available_width_tag,
        available_width_bits,
        available_height_tag,
        available_height_bits,
    );
    if let Some(measured) = session.flex_grid.cached_measurement(&key) {
        session.record_timing(|timings| timings.measure_flex_grid_item += timing_started.elapsed());
        return measured;
    }
    let style = session.reader.style(box_idx);
    let ItemWidthBounds {
        minimum: min_width,
        maximum: max_width,
        horizontal_inset,
    } = item_width_bounds(session, box_idx, known.height, available.height);
    let width = known
        .width
        .map(f64::from)
        .unwrap_or_else(|| match available.width {
            AvailableSpace::Definite(value) => max_width.min(f64::from(value).max(min_width)),
            AvailableSpace::MinContent => min_width,
            AvailableSpace::MaxContent => max_width,
        });
    let containing_width = width.max(0.0) + horizontal_inset;
    let measured = measure_box_isolated(
        session,
        BoxLayoutRequest::normal(
            box_idx,
            containing_width.max(0.0),
            known.height.map(f64::from),
        ),
    );
    let resolved_vertical_inset = style.get_vertical_padding(containing_width)
        + style.border_top_width() as f64
        + style.border_bottom_width() as f64;
    // Taffy's leaf measure callback returns content-box dimensions; Taffy
    // resolves and adds the item's padding itself. The isolated renderer pass
    // is still needed for block-size and baseline measurement, but its used
    // inline size may already have been reduced by percentage padding. Return
    // the intrinsic content width selected above so that padding is not
    // effectively subtracted here and then added by Taffy a second time.
    let result = TaffySize {
        width: known.width.unwrap_or_else(|| finite_f32(width)),
        height: known.height.unwrap_or_else(|| {
            finite_f32((measured.size.height - resolved_vertical_inset).max(0.0))
        }),
    };
    let measured = MeasuredItem {
        size: result,
        first_baseline: measured.first_baseline,
    };
    session.flex_grid.cache_measurement(key, measured);
    session.record_timing(|timings| timings.measure_flex_grid_item += timing_started.elapsed());
    measured
}

fn available_space_key(value: AvailableSpace) -> (u32, u32) {
    match value {
        AvailableSpace::Definite(value) => (0, value.to_bits()),
        AvailableSpace::MinContent => (1, 0),
        AvailableSpace::MaxContent => (2, 0),
    }
}

pub(super) fn measure_intrinsic_item(
    session: &LayoutEngine<'_, '_>,
    box_idx: usize,
    known: TaffySize<Option<f32>>,
    available: TaffySize<AvailableSpace>,
) -> TaffySize<f32> {
    if let Some(size) = measure_replaced_item(session, box_idx, known, available) {
        return size;
    }
    let ItemWidthBounds {
        minimum: min_width,
        maximum: max_width,
        ..
    } = item_width_bounds(session, box_idx, known.height, available.height);
    let width = known.width.unwrap_or_else(|| match available.width {
        AvailableSpace::Definite(value) => value,
        AvailableSpace::MinContent => finite_f32(min_width),
        AvailableSpace::MaxContent => finite_f32(max_width),
    });
    TaffySize {
        width,
        height: known.height.unwrap_or(0.0),
    }
}

/// Shared inputs for intrinsic contribution queries and final item measurement.
/// Both stages resolve cyclic item insets against a zero inline-size basis.
pub(super) struct ItemWidthBounds {
    pub(super) minimum: f64,
    pub(super) maximum: f64,
    pub(super) horizontal_inset: f64,
}

pub(super) fn item_width_bounds(
    session: &LayoutEngine<'_, '_>,
    box_idx: usize,
    known_height: Option<f32>,
    available_height: AvailableSpace,
) -> ItemWidthBounds {
    let model = ResolvedBoxModel::new(session.reader.style(box_idx), 0.0).with_nonnegative_padding();
    let (outer_min_width, outer_max_width) = crate::layout::box_intrinsic_widths(session, box_idx);
    let horizontal_inset = model.horizontal_margin() + model.horizontal_padding_border();
    let mut min_width = (outer_min_width - horizontal_inset).max(0.0);
    let mut max_width = (outer_max_width - horizontal_inset).max(min_width);
    let vertical_inset = model.vertical_padding_border();
    let available_height = known_height
        .or(match available_height {
            AvailableSpace::Definite(value) => Some(value),
            AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
        })
        .map(|value| finite_f32((f64::from(value) - vertical_inset).max(0.0)));
    if let Some(width) = available_height.and_then(|height| {
        crate::layout::percentage_height_image_width(session, box_idx, f64::from(height))
    }) {
        min_width = width;
        max_width = width;
    }
    ItemWidthBounds {
        minimum: min_width,
        maximum: max_width,
        horizontal_inset,
    }
}

pub(super) fn measure_replaced_item(
    session: &LayoutEngine<'_, '_>,
    box_idx: usize,
    known: TaffySize<Option<f32>>,
    available: TaffySize<AvailableSpace>,
) -> Option<TaffySize<f32>> {
    let intrinsic = session.reader.image_intrinsic(box_idx)?;
    Some(measure_replaced_content(
        intrinsic.size,
        preferred_aspect_ratio(
            session.reader.style(box_idx).aspect_ratio(),
            intrinsic.aspect_ratio,
        ),
        known,
        available,
    ))
}

/// Taffy's known dimensions are border-box dimensions while its measure
/// callback returns content-box dimensions. When a known axis is present,
/// Taffy has already subtracted padding and borders from the corresponding
/// definite available space; use that content size for ratio transfer.
fn measure_replaced_content(
    intrinsic: Size,
    aspect_ratio: Option<f64>,
    known: TaffySize<Option<f32>>,
    available: TaffySize<AvailableSpace>,
) -> TaffySize<f32> {
    let known_content_width = known.width.map(|width| match available.width {
        AvailableSpace::Definite(content) => content.max(0.0),
        AvailableSpace::MinContent | AvailableSpace::MaxContent => width.max(0.0),
    });
    let known_content_height = known.height.map(|height| match available.height {
        AvailableSpace::Definite(content) => content.max(0.0),
        AvailableSpace::MinContent | AvailableSpace::MaxContent => height.max(0.0),
    });
    let ratio = aspect_ratio.filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    match (known_content_width, known_content_height, ratio) {
        (Some(width), Some(height), _) => TaffySize { width, height },
        (Some(width), None, Some(ratio)) => TaffySize {
            width,
            height: finite_f32(f64::from(width) / ratio),
        },
        (None, Some(height), Some(ratio)) => TaffySize {
            width: finite_f32(f64::from(height) * ratio),
            height,
        },
        (width, height, _) => TaffySize {
            width: width.unwrap_or_else(|| finite_f32(intrinsic.width)),
            height: height.unwrap_or_else(|| finite_f32(intrinsic.height)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taffy_measurement_uses_definite_content_box_space() {
        let measured = measure_replaced_content(
            Size::new(16.0, 16.0),
            Some(1.0),
            TaffySize {
                width: Some(30.0),
                height: None,
            },
            TaffySize {
                width: AvailableSpace::Definite(24.0),
                height: AvailableSpace::MaxContent,
            },
        );
        assert_eq!(
            measured,
            TaffySize {
                width: 24.0,
                height: 24.0
            }
        );
    }
}
