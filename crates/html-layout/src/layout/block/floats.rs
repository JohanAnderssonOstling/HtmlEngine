#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FloatBand {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub side: FloatSide,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FloatContext {
    bands: Vec<FloatBand>,
    source_position_floor: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FloatPlacement {
    pub(crate) y: f64,
    pub(crate) left: f64,
    pub(crate) right: f64,
}

impl FloatContext {
    pub(crate) fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    pub(crate) fn band_count(&self) -> usize {
        self.bands.len()
    }

    pub(crate) fn max_bottom(&self) -> f64 {
        self.bands.iter().map(|band| band.bottom).fold(0.0, f64::max)
    }

    /// Records the hypothetical normal-flow position of the latest block in
    /// this formatting context. A descendant float in a collapse-through
    /// wrapper must not lose the adjoining margin that established that
    /// position.
    pub(crate) fn note_source_position(&mut self, y: f64) {
        self.source_position_floor = Some(self.source_position_floor.map_or(y, |floor| floor.max(y)));
    }

    pub(crate) fn source_position_at_or_after(&self, y: f64) -> f64 {
        self.source_position_floor.map_or(y, |floor| floor.max(y))
    }

    /// Finds the first vertical position at or below `start_y` where a float's
    /// complete margin box fits between the active left and right floats.
    pub(crate) fn place_margin_box(&self, start_y: f64, width: f64, height: f64, container_left: f64, container_right: f64) -> FloatPlacement {
        // A later float may not start above an earlier float, even when their
        // margin boxes would not overlap vertically.
        let start_y = self.source_position_floor.map_or(start_y, |floor| floor.max(start_y));
        let mut y = self.bands.iter().map(|band| band.top).fold(start_y, f64::max);
        let width = width.max(0.0);
        let height = height.max(0.0);

        loop {
            let (left, right) = self.available(y, height, container_left, container_right);
            // Keep the available span signed. Opposing floats can cross so
            // that `right < left`; even a zero-width box cannot occupy that
            // overlapping band and must retry below one of the floats.
            if width <= right - left {
                return FloatPlacement { y, left, right };
            }

            let bottom = y + height;
            let next_y = self.bands.iter().filter_map(|band| (band.bottom > y && band.top < bottom).then_some(band.bottom)).min_by(f64::total_cmp);
            let Some(next_y) = next_y.filter(|next_y| *next_y > y) else {
                // The float itself may be wider than its containing block. In
                // that case CSS permits overflow once no preceding float can
                // make additional room by ending below this position.
                return FloatPlacement { y, left, right };
            };
            y = next_y;
        }
    }

    pub(crate) fn add_band(&mut self, band: FloatBand) {
        // A zero-inline-size float excludes no horizontal space, but it still
        // has a float side and block extent that `clear` must honor.
        if band.right >= band.left && band.bottom > band.top {
            self.bands.push(band);
        }
    }

    pub(crate) fn available(&self, y: f64, height: f64, container_left: f64, container_right: f64) -> (f64, f64) {
        let bottom = y + height.max(0.0);
        let mut left = container_left;
        let mut right = container_right;
        for band in &self.bands {
            if band.bottom <= y || band.top >= bottom {
                continue;
            }
            match band.side {
                FloatSide::Left => left = left.max(band.right),
                FloatSide::Right => right = right.min(band.left),
            }
        }
        (left.min(container_right), right.max(container_left))
    }

    /// Finds the next vertical position where an in-flow atomic item can fit
    /// without overlapping an active float. Unlike a float placement this
    /// does not mutate the float context; it is used by the inline formatter
    /// to retry an otherwise-empty line below an exclusion.
    pub(crate) fn next_y_fitting(&self, mut y: f64, width: f64, height: f64, container_left: f64, container_right: f64) -> f64 {
        let width = width.max(0.0);
        let height = height.max(0.0);
        loop {
            let (left, right) = self.available(y, height, container_left, container_right);
            if width <= right - left {
                return y;
            }

            let bottom = y + height;
            let next_y = self.bands.iter().filter_map(|band| (band.bottom > y && band.top < bottom).then_some(band.bottom)).min_by(f64::total_cmp);
            let Some(next_y) = next_y.filter(|next_y| *next_y > y) else {
                return y;
            };
            y = next_y;
        }
    }

    pub(crate) fn clear_for(&self, y: f64, clear: Clear) -> f64 {
        match clear {
            Clear::None => y,
            Clear::Left => self.clear_to(y, ClearSide::Left),
            Clear::Right => self.clear_to(y, ClearSide::Right),
            Clear::Both => self.clear_to(y, ClearSide::Both),
        }
    }

    fn clear_to(&self, y: f64, side: ClearSide) -> f64 {
        let mut clear_y = y;
        for band in &self.bands {
            let affects_side = match side {
                ClearSide::Left => matches!(band.side, FloatSide::Left),
                ClearSide::Right => matches!(band.side, FloatSide::Right),
                ClearSide::Both => true,
            };
            if affects_side {
                clear_y = clear_y.max(band.bottom);
            }
        }
        clear_y
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClearSide {
    Left,
    Right,
    Both,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FloatSide {
    #[default]
    Left,
    Right,
}

#[derive(Default)]
pub(in crate::layout) struct FloatState {
    stack: Vec<FloatContext>,
    hypothetical_clearance_floors: Vec<f64>,
    hypothetical_clearance_rewinds: Vec<f64>,
}

impl FloatState {
    pub(in crate::layout) fn reset(&mut self) {
        self.stack.clear();
        self.hypothetical_clearance_floors.clear();
        self.hypothetical_clearance_rewinds.clear();
        self.push_context();
    }

    pub(in crate::layout) fn clear(&mut self) {
        self.stack.clear();
        self.hypothetical_clearance_floors.clear();
        self.hypothetical_clearance_rewinds.clear();
    }

    pub(crate) fn current_float_context(&self) -> Option<&FloatContext> {
        self.stack.last()
    }

    pub(crate) fn current_float_context_mut(&mut self) -> Option<&mut FloatContext> {
        self.stack.last_mut()
    }

    pub(crate) fn push_context(&mut self) {
        self.stack.push(FloatContext::default());
    }

    pub(crate) fn pop_context(&mut self) {
        self.stack.pop();
    }

    pub(crate) fn push_hypothetical_clearance_floor(&mut self, y: f64) {
        self.hypothetical_clearance_floors.push(y);
    }

    pub(crate) fn pop_hypothetical_clearance_floor(&mut self) {
        self.hypothetical_clearance_floors.pop();
    }

    pub(crate) fn push_hypothetical_clearance_rewind(&mut self, offset: f64) {
        self.hypothetical_clearance_rewinds.push(offset);
    }

    pub(crate) fn pop_hypothetical_clearance_rewind(&mut self) {
        self.hypothetical_clearance_rewinds.pop();
    }

    fn hypothetical_clearance_floor(&self) -> Option<f64> {
        self.hypothetical_clearance_floors.iter().copied().max_by(f64::total_cmp)
    }

    pub(super) fn hypothetical_clearance_rewind(&self) -> f64 {
        self.hypothetical_clearance_rewinds.iter().sum()
    }
}

/// Returns the new block-flow offset when `clear` introduces actual
/// clearance. The caller owns the adjoining-margin state that clearance
/// separates.
#[derive(Clone, Copy, Debug)]
pub(super) struct Clearance {
    pub(super) offset: f64,
    pub(super) hypothetical_y: f64,
}

pub(super) fn clearance_offset(
    floats: &FloatState, origin_y: f64, y_offset: f64, pending_margin: MarginStrut, before_margin: MarginStrut, own_before_margin: f64, clear: Clear, margin_adjoins_parent_top: bool, has_adjoining_matching_float: bool,
) -> Option<Clearance> {
    let side = match clear {
        Clear::Left => ClearSide::Left,
        Clear::Right => ClearSide::Right,
        Clear::Both => ClearSide::Both,
        Clear::None => return None,
    };
    let context = floats.current_float_context()?;
    let mut hypothetical_margin = pending_margin;
    hypothetical_margin.merge(before_margin);
    let unbounded_hypothetical_y = origin_y + y_offset + hypothetical_margin.resolve() - floats.hypothetical_clearance_rewind();
    let hypothetical_y = floats.hypothetical_clearance_floor().map_or(unbounded_hypothetical_y, |floor| floor.max(unbounded_hypothetical_y));
    if margin_adjoins_parent_top {
        // Clearance separates the candidate's top margin from an adjoining
        // float source before that margin is collapsed. Consequently even a
        // very large positive margin can require negative clearance: the
        // border edge is placed at the float bottom, not at the hypothetical
        // margin-shifted position.
        let marginless_y = origin_y + y_offset;
        let separated_y = context.clear_to(marginless_y, side);
        if separated_y > marginless_y && ((has_adjoining_matching_float && hypothetical_y > separated_y) || (own_before_margin.abs() > 0.001 && (hypothetical_y - separated_y).abs() <= 0.001)) {
            return Some(Clearance { offset: separated_y - origin_y, hypothetical_y });
        }
    }
    let cleared_y = context.clear_to(hypothetical_y, side);
    (cleared_y > hypothetical_y).then_some(Clearance { offset: cleared_y - origin_y, hypothetical_y })
}

/// Lays out and positions one float, translates its already-emitted subtree,
/// and records the resulting exclusion band.
pub(super) fn layout_float(
    engine: &mut LayoutEngine<'_, '_>, resolved: crate::layout::box_sizing::ResolvedBoxSizing, side: Float, origin: Point, flow_margin_y: f64, margin_left: f64, horizontal_margin: f64, margin_before: f64, margin_after: f64,
) -> Size {
    let box_idx = resolved.box_idx();
    let available_width = resolved.available_width();
    let initial_point = origin + Vec2::new(margin_left, flow_margin_y + margin_before);
    engine.geometry.set_point(box_idx, initial_point);
    let layout = engine.layout_resolved_box(resolved);
    let size = layout.size;

    let margin_width = size.width + horizontal_margin;
    let margin_height = size.height + margin_before + margin_after;
    let placement = engine.floats.current_float_context().map(|context| context.place_margin_box(origin.y + flow_margin_y, margin_width, margin_height, origin.x, origin.x + available_width)).unwrap_or(FloatPlacement {
        y: origin.y + flow_margin_y,
        left: origin.x,
        right: origin.x + available_width,
    });
    let margin_y = placement.y;
    let final_x = if matches!(side, Float::Right) { placement.right - margin_width + margin_left } else { placement.left + margin_left };
    let final_y = margin_y + margin_before;
    let final_point = Point::new(final_x, final_y);
    translate_laid_out_subtree_output(engine, box_idx, true, &layout.output, final_point - initial_point);
    if let Some(context) = engine.floats.current_float_context_mut() {
        context.add_band(FloatBand {
            left: final_x - margin_left,
            right: final_x + size.width + (horizontal_margin - margin_left),
            top: margin_y,
            bottom: final_y + size.height + margin_after,
            side: if matches!(side, Float::Right) { FloatSide::Right } else { FloatSide::Left },
        });
    }
    size
}

#[cfg(test)]
mod tests {
    use super::{FloatBand, FloatContext, FloatSide};

    #[test]
    fn placement_descends_through_every_blocking_float_band() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand { left: 0.0, right: 90.0, top: 0.0, bottom: 40.0, side: FloatSide::Left });
        context.add_band(FloatBand { left: 150.0, right: 300.0, top: 0.0, bottom: 80.0, side: FloatSide::Right });

        let placement = context.place_margin_box(0.0, 200.0, 20.0, 0.0, 300.0);

        assert_eq!(placement.y, 80.0);
        assert_eq!((placement.left, placement.right), (0.0, 300.0));
    }

    #[test]
    fn placement_checks_the_complete_float_height() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand { left: 0.0, right: 100.0, top: 30.0, bottom: 100.0, side: FloatSide::Left });

        let placement = context.place_margin_box(0.0, 250.0, 80.0, 0.0, 300.0);

        assert_eq!(placement.y, 100.0);
    }

    #[test]
    fn placement_preserves_the_normal_flow_position_through_a_wrapper() {
        let mut context = FloatContext::default();
        context.note_source_position(30.0);

        let placement = context.place_margin_box(10.0, 20.0, 20.0, 0.0, 300.0);

        assert_eq!(placement.y, 30.0);
    }

    #[test]
    fn available_space_includes_side_by_side_floats_on_the_same_side() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand { left: 0.0, right: 40.0, top: 0.0, bottom: 40.0, side: FloatSide::Left });
        context.add_band(FloatBand { left: 40.0, right: 80.0, top: 0.0, bottom: 40.0, side: FloatSide::Left });

        assert_eq!(context.available(0.0, 20.0, 0.0, 300.0), (80.0, 300.0));
    }

    #[test]
    fn oversized_inline_item_can_find_space_below_a_float() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand { left: 0.0, right: 100.0, top: 0.0, bottom: 100.0, side: FloatSide::Left });

        assert_eq!(context.next_y_fitting(0.0, 300.0, 100.0, 0.0, 300.0), 100.0);
    }

    #[test]
    fn zero_width_item_descends_below_crossed_float_edges() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand { left: 0.0, right: 250.0, top: 100.0, bottom: 200.0, side: FloatSide::Left });
        context.add_band(FloatBand { left: 150.0, right: 400.0, top: 0.0, bottom: 100.0, side: FloatSide::Right });

        assert_eq!(context.next_y_fitting(0.0, 0.0, 200.0, 0.0, 400.0), 100.0);
    }

    #[test]
    fn in_flow_item_can_use_space_above_a_later_lower_float() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand { left: 0.0, right: 80.0, top: 0.0, bottom: 20.0, side: FloatSide::Left });
        context.add_band(FloatBand { left: 80.0, right: 160.0, top: 0.0, bottom: 20.0, side: FloatSide::Left });
        context.add_band(FloatBand { left: 0.0, right: 80.0, top: 20.0, bottom: 40.0, side: FloatSide::Left });

        assert_eq!(context.next_y_fitting(0.0, 60.0, 20.0, 0.0, 224.0), 0.0, "normal-flow content is not subject to the source-order top constraint used when placing another float");
    }
}
use super::margins::MarginStrut;
use crate::layout::LayoutEngine;
use crate::layout::translate_laid_out_subtree_output;
use html_style_model::{Clear, Float};
use kurbo::{Point, Size, Vec2};
