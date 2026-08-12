use super::finite_f32;
use crate::layout::{LayoutEngine, measure_box_and_baselines_isolated, measure_replaced_content, preferred_aspect_ratio};
use taffy::geometry::Size as TaffySize;
use taffy::prelude::AvailableSpace;

type TaffyMeasureKey = (usize, u32, u32, u32, u32, u32, u32);

#[derive(Default)]
pub(crate) struct FlexGridState {
    measure_cache: rustc_data_structures::fx::FxHashMap<TaffyMeasureKey, MeasuredItem>,
    container_first_baselines: rustc_data_structures::fx::FxHashMap<usize, f64>,
}

#[derive(Clone, Copy)]
pub(super) struct MeasuredItem {
    pub(super) size: TaffySize<f32>,
    pub(super) first_baseline: Option<f64>,
}

impl FlexGridState {
    pub(crate) fn clear(&mut self) {
        self.measure_cache.clear();
        self.container_first_baselines.clear();
    }

    fn cached_measurement(&self, key: &TaffyMeasureKey) -> Option<MeasuredItem> {
        self.measure_cache.get(key).copied()
    }

    fn cache_measurement(&mut self, key: TaffyMeasureKey, measured: MeasuredItem) {
        self.measure_cache.insert(key, measured);
    }

    pub(crate) fn set_container_first_baseline(&mut self, box_idx: usize, baseline: Option<f64>) {
        if let Some(baseline) = baseline {
            self.container_first_baselines.insert(box_idx, baseline);
        } else {
            self.container_first_baselines.remove(&box_idx);
        }
    }

    pub(crate) fn container_first_baseline(&self, box_idx: usize) -> Option<f64> {
        self.container_first_baselines.get(&box_idx).copied()
    }
}

pub(super) fn measure_item_with_baseline(session: &mut LayoutEngine<'_, '_>, box_idx: usize, known: TaffySize<Option<f32>>, available: TaffySize<AvailableSpace>) -> MeasuredItem {
    let timing_started = session.start_timing();
    if let Some(intrinsic) = session.reader.image_intrinsic(box_idx) {
        let result = measure_replaced_content(intrinsic.size, preferred_aspect_ratio(session.reader.style(box_idx).aspect_ratio(), intrinsic.aspect_ratio), known, available);
        session.record_timing(|timings| timings.measure_flex_grid_item += timing_started.elapsed());
        return MeasuredItem { size: result, first_baseline: None };
    }
    let (available_width_tag, available_width_bits) = available_space_key(available.width);
    let (available_height_tag, available_height_bits) = available_space_key(available.height);
    let key = (box_idx, known.width.map(f32::to_bits).unwrap_or(u32::MAX), known.height.map(f32::to_bits).unwrap_or(u32::MAX), available_width_tag, available_width_bits, available_height_tag, available_height_bits);
    if let Some(measured) = session.flex_grid.cached_measurement(&key) {
        session.record_timing(|timings| timings.measure_flex_grid_item += timing_started.elapsed());
        return measured;
    }
    let style = session.reader.style(box_idx);
    let (outer_min_width, outer_max_width) = crate::layout::box_intrinsic_widths(session, box_idx);
    let horizontal_inset = style.get_horizontal_margin_padding(0.0) + style.border_left_width() as f64 + style.border_right_width() as f64;
    let mut min_width = (outer_min_width - horizontal_inset).max(0.0);
    let mut max_width = (outer_max_width - horizontal_inset).max(min_width);
    let vertical_inset = style.get_vertical_padding(0.0) + style.border_top_width() as f64 + style.border_bottom_width() as f64;
    let available_height = known
        .height
        .or(match available.height {
            AvailableSpace::Definite(value) => Some(value),
            AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
        })
        .map(|value| finite_f32((f64::from(value) - vertical_inset).max(0.0)));
    if let Some(width) = available_height.and_then(|height| crate::layout::percentage_height_image_width(session, box_idx, f64::from(height))) {
        min_width = width;
        max_width = width;
    }
    let width = known.width.map(f64::from).unwrap_or_else(|| match available.width {
        AvailableSpace::Definite(value) => max_width.min(f64::from(value).max(min_width)),
        AvailableSpace::MinContent => min_width,
        AvailableSpace::MaxContent => max_width,
    });
    let containing_width = width.max(0.0) + horizontal_inset;
    let (measured, first_baseline, _) = measure_box_and_baselines_isolated(session, box_idx, containing_width, known.height.map(f64::from));
    let resolved_vertical_inset = style.get_vertical_padding(containing_width) + style.border_top_width() as f64 + style.border_bottom_width() as f64;
    // Taffy's leaf measure callback returns content-box dimensions; Taffy
    // resolves and adds the item's padding itself. The isolated renderer pass
    // is still needed for block-size and baseline measurement, but its used
    // inline size may already have been reduced by percentage padding. Return
    // the intrinsic content width selected above so that padding is not
    // effectively subtracted here and then added by Taffy a second time.
    let result = TaffySize { width: known.width.unwrap_or_else(|| finite_f32(width)), height: known.height.unwrap_or_else(|| finite_f32((measured.height - resolved_vertical_inset).max(0.0))) };
    let measured = MeasuredItem { size: result, first_baseline };
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
