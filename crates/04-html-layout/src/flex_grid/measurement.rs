use super::finite_f32;
use crate::measurement::measure_box_isolated;
use crate::{BoxLayoutRequest, LayoutEngine, preferred_aspect_ratio};
use kurbo::Size;
use rustc_data_structures::fx::FxHashMap;
use taffy::geometry::Size as TaffySize;
use taffy::prelude::AvailableSpace;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TaffyMeasureKey {
    owner: usize,
    known_width: u32,
    known_height: u32,
    available_width: AvailableSpaceKey,
    available_height: AvailableSpaceKey,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum AvailableSpaceKey {
    Definite(u32),
    MinContent,
    MaxContent,
}

#[derive(Default)]
pub struct FlexGridState {
    measure_cache: FxHashMap<TaffyMeasureKey, MeasuredItem>,
}

impl FlexGridState {
    pub(crate) fn clear(&mut self) {
        self.measure_cache.clear();
    }
}

#[derive(Clone, Copy)]
pub(super) struct MeasuredItem {
    pub(super) size: TaffySize<f32>,
    pub(super) first_baseline: Option<f64>,
}

/// Both intrinsic probes and full measurements update explicitly owned caches.
pub(super) enum MeasurementMode<'engine, 'input, 'out> {
    Intrinsic(&'engine mut LayoutEngine<'input, 'out>),
    Full(&'engine mut LayoutEngine<'input, 'out>),
}

impl MeasurementMode<'_, '_, '_> {
    pub(super) fn measure(
        &mut self,
        box_idx: usize,
        known: TaffySize<Option<f32>>,
        available: TaffySize<AvailableSpace>,
    ) -> MeasuredItem {
        let intrinsic = matches!(self, Self::Intrinsic(_));
        let engine = match self {
            Self::Intrinsic(engine) | Self::Full(engine) => &mut **engine,
        };
        if let Some(intrinsic) = engine.read.reader.image_intrinsic(box_idx) {
            return MeasuredItem {
                size: measure_replaced_content(
                    intrinsic.size,
                    preferred_aspect_ratio(
                        engine.read.reader.style(box_idx).aspect_ratio(),
                        intrinsic.aspect_ratio,
                    ),
                    known,
                    available,
                ),
                first_baseline: None,
            };
        }
        let key = TaffyMeasureKey {
            owner: box_idx,
            known_width: known.width.map_or(u32::MAX, f32::to_bits),
            known_height: known.height.map_or(u32::MAX, f32::to_bits),
            available_width: available_space_key(available.width),
            available_height: available_space_key(available.height),
        };
        if !intrinsic && let Some(measured) = engine.scratch.flex_grid.measure_cache.get(&key) {
            return *measured;
        }
        let style = engine.read.reader.style(box_idx);
        let bounds = item_width_bounds(engine, box_idx, known.height, available.height);
        // Full layout clamps definite widths; intrinsic probes preserve them.
        let width = known
            .width
            .map(f64::from)
            .unwrap_or_else(|| match available.width {
                AvailableSpace::Definite(value) if !intrinsic => {
                    bounds.maximum.min(f64::from(value).max(bounds.minimum))
                }
                AvailableSpace::Definite(value) => f64::from(value),
                AvailableSpace::MinContent => bounds.minimum,
                AvailableSpace::MaxContent => bounds.maximum,
            });
        if intrinsic {
            return MeasuredItem {
                size: TaffySize {
                    width: known.width.unwrap_or_else(|| match available.width {
                        AvailableSpace::Definite(value) => value,
                        _ => finite_f32(width),
                    }),
                    height: known.height.unwrap_or(0.0),
                },
                first_baseline: None,
            };
        }
        let containing_width = width.max(0.0) + bounds.horizontal_inset;
        let measured = measure_box_isolated(
            engine,
            BoxLayoutRequest::normal(
                box_idx,
                containing_width.max(0.0),
                known.height.map(f64::from),
            ),
        );
        let resolved_vertical_inset = style.get_vertical_padding_border(containing_width);
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
        engine.scratch.flex_grid.measure_cache.insert(key, measured);
        measured
    }
}

/// Shared inputs for intrinsic contribution queries and final item measurement.
/// Both stages resolve cyclic item insets against a zero inline-size basis.
struct ItemWidthBounds {
    minimum: f64,
    maximum: f64,
    horizontal_inset: f64,
}

fn item_width_bounds(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    known_height: Option<f32>,
    available_height: AvailableSpace,
) -> ItemWidthBounds {
    let model = engine
        .read
        .reader
        .box_model(box_idx, 0.0)
        .with_nonnegative_padding();
    let (outer_min_width, outer_max_width) =
        crate::measurement::box_intrinsic_widths(engine, box_idx);
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
    // Book mode leaves the intrinsic bounds unchanged; the helper returns None.
    if let Some(width) = available_height.and_then(|height| {
        crate::measurement::percentage_height_image_width(engine, box_idx, f64::from(height))
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

fn available_space_key(value: AvailableSpace) -> AvailableSpaceKey {
    match value {
        AvailableSpace::Definite(value) => AvailableSpaceKey::Definite(value.to_bits()),
        AvailableSpace::MinContent => AvailableSpaceKey::MinContent,
        AvailableSpace::MaxContent => AvailableSpaceKey::MaxContent,
    }
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
    let known_content = known.zip_map(available, |known, available| {
        known.map(|size| match available {
            AvailableSpace::Definite(content) => content.max(0.0),
            AvailableSpace::MinContent | AvailableSpace::MaxContent => size.max(0.0),
        })
    });
    let ratio = aspect_ratio.filter(|ratio| ratio.is_finite() && *ratio > 0.0);
    match (known_content.width, known_content.height, ratio) {
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
