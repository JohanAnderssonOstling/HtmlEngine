#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FloatBand {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    pub side: FloatSide,
}

#[derive(Clone, Debug, Default)]
pub struct FloatContext {
    bands: Vec<FloatBand>,
    source_position_floor: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatPlacement {
    pub y: f64,
    pub left: f64,
    pub right: f64,
}

impl FloatContext {
    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    pub fn band_count(&self) -> usize {
        self.bands.len()
    }

    pub fn max_bottom(&self) -> f64 {
        self.bands
            .iter()
            .map(|band| band.bottom)
            .fold(0.0, f64::max)
    }

    /// Records the hypothetical normal-flow position of the latest block in
    /// this formatting context. A descendant float in a collapse-through
    /// wrapper must not lose the adjoining margin that established that
    /// position.
    pub fn note_source_position(&mut self, y: f64) {
        self.source_position_floor =
            Some(self.source_position_floor.map_or(y, |floor| floor.max(y)));
    }

    pub fn source_position_at_or_after(&self, y: f64) -> f64 {
        self.source_position_floor.map_or(y, |floor| floor.max(y))
    }

    /// Finds the first vertical position at or below `start_y` where a float's
    /// complete margin box fits between the active left and right floats.
    pub fn place_margin_box(
        &self,
        start_y: f64,
        width: f64,
        height: f64,
        container_left: f64,
        container_right: f64,
    ) -> FloatPlacement {
        // A later float may not start above an earlier float, even when their
        // margin boxes would not overlap vertically.
        let start_y = self
            .source_position_floor
            .map_or(start_y, |floor| floor.max(start_y));
        let y = self
            .bands
            .iter()
            .map(|band| band.top)
            .fold(start_y, f64::max);
        self.find_placement(y, width, height, container_left, container_right)
    }

    fn find_placement(
        &self,
        mut y: f64,
        width: f64,
        height: f64,
        container_left: f64,
        container_right: f64,
    ) -> FloatPlacement {
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
            let next_y = self
                .bands
                .iter()
                .filter_map(|band| (band.bottom > y && band.top < bottom).then_some(band.bottom))
                .min_by(f64::total_cmp);
            let Some(next_y) = next_y.filter(|next_y| *next_y > y) else {
                // An oversized item may overflow once no float can make
                // additional room by ending below this position.
                return FloatPlacement { y, left, right };
            };
            y = next_y;
        }
    }

    pub fn add_band(&mut self, band: FloatBand) {
        // A zero-inline-size float excludes no horizontal space, but it still
        // has a float side and block extent that `clear` must honor.
        if band.right >= band.left && band.bottom > band.top {
            self.bands.push(band);
        }
    }

    pub fn available(
        &self,
        y: f64,
        height: f64,
        container_left: f64,
        container_right: f64,
    ) -> (f64, f64) {
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
    /// without overlapping an active float. In-flow items can fit above later
    /// floats; they do not inherit the source-order floor of float placement.
    pub fn next_y_fitting(
        &self,
        y: f64,
        width: f64,
        height: f64,
        container_left: f64,
        container_right: f64,
    ) -> f64 {
        self.find_placement(y, width, height, container_left, container_right)
            .y
    }

    pub fn clear_for(&self, y: f64, clear: Clear) -> f64 {
        if clear == Clear::None {
            return y;
        }
        let mut clear_y = y;
        for band in &self.bands {
            let affects_side = matches!(
                (clear, band.side),
                (Clear::Both, _)
                    | (Clear::Left, FloatSide::Left)
                    | (Clear::Right, FloatSide::Right)
            );
            if affects_side {
                clear_y = clear_y.max(band.bottom);
            }
        }
        clear_y
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatSide {
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

    pub fn current_float_context(&self) -> Option<&FloatContext> {
        self.stack.last()
    }

    pub fn current_float_context_mut(&mut self) -> Option<&mut FloatContext> {
        self.stack.last_mut()
    }

    pub fn push_context(&mut self) {
        self.stack.push(FloatContext::default());
    }

    pub fn pop_context(&mut self) {
        self.stack.pop();
    }

    pub fn push_hypothetical_clearance_floor(&mut self, y: f64) {
        self.hypothetical_clearance_floors.push(y);
    }

    pub fn pop_hypothetical_clearance_floor(&mut self) {
        self.hypothetical_clearance_floors.pop();
    }

    pub fn push_hypothetical_clearance_rewind(&mut self, offset: f64) {
        self.hypothetical_clearance_rewinds.push(offset);
    }

    pub fn pop_hypothetical_clearance_rewind(&mut self) {
        self.hypothetical_clearance_rewinds.pop();
    }

    fn hypothetical_clearance_floor(&self) -> Option<f64> {
        self.hypothetical_clearance_floors
            .iter()
            .copied()
            .max_by(f64::total_cmp)
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
    floats: &FloatState,
    origin_y: f64,
    y_offset: f64,
    pending_margin: MarginStrut,
    before_margin: MarginStrut,
    own_before_margin: f64,
    clear: Clear,
    margin_adjoins_parent_top: bool,
    has_adjoining_matching_float: bool,
) -> Option<Clearance> {
    if clear == Clear::None {
        return None;
    }
    let context = floats.current_float_context()?;
    let mut hypothetical_margin = pending_margin;
    hypothetical_margin.merge(before_margin);
    let unbounded_hypothetical_y = origin_y + y_offset + hypothetical_margin.resolve()
        - floats.hypothetical_clearance_rewind();
    let hypothetical_y = floats
        .hypothetical_clearance_floor()
        .map_or(unbounded_hypothetical_y, |floor| {
            floor.max(unbounded_hypothetical_y)
        });
    if margin_adjoins_parent_top {
        // Clearance separates the candidate's top margin from an adjoining
        // float source before that margin is collapsed. Consequently even a
        // very large positive margin can require negative clearance: the
        // border edge is placed at the float bottom, not at the hypothetical
        // margin-shifted position.
        let marginless_y = origin_y + y_offset;
        let separated_y = context.clear_for(marginless_y, clear);
        if separated_y > marginless_y
            && ((has_adjoining_matching_float && hypothetical_y > separated_y)
                || (own_before_margin.abs() > 0.001
                    && (hypothetical_y - separated_y).abs() <= 0.001))
        {
            return Some(Clearance {
                offset: separated_y - origin_y,
                hypothetical_y,
            });
        }
    }
    let cleared_y = context.clear_for(hypothetical_y, clear);
    (cleared_y > hypothetical_y).then_some(Clearance {
        offset: cleared_y - origin_y,
        hypothetical_y,
    })
}

/// Lay out, position and record either a block-flow float or an inline anchor.
/// Callers resolve their margin policy; preceding inline content contributes
/// its advance and line height, while block flow passes `Size::ZERO`.
pub(in crate::layout) fn layout_float(
    engine: &mut LayoutEngine<'_, '_>,
    resolved: ResolvedBoxSizing,
    origin: Point,
    margins: Insets,
    preceding_line: Size,
) -> Size {
    let box_idx = resolved.box_idx();
    let available_width = resolved.available_width();
    let side = engine.reader.style(box_idx).float();
    let initial_point = origin + Vec2::new(margins.x0, margins.y0);
    engine.geometry.set_point(box_idx, initial_point);
    let layout = engine.layout_resolved_box(resolved);
    let size = layout.size;

    let margin_width = size.width + margins.x0 + margins.x1;
    let margin_height = size.height + margins.y0 + margins.y1;
    let source_y = engine
        .floats
        .current_float_context()
        .map_or(origin.y, |context| {
            let candidate_y = context.source_position_at_or_after(origin.y);
            if preceding_line.width > 0.001 {
                let (left, right) = context.available(
                    candidate_y,
                    margin_height.max(1.0),
                    origin.x,
                    origin.x + available_width,
                );
                if preceding_line.width + margin_width > right - left + 0.001 {
                    return candidate_y + preceding_line.height;
                }
            }
            candidate_y
        });
    let placement = engine
        .floats
        .current_float_context()
        .map(|context| {
            context.place_margin_box(
                source_y,
                margin_width,
                margin_height,
                origin.x,
                origin.x + available_width,
            )
        })
        .unwrap_or(FloatPlacement {
            y: source_y,
            left: origin.x,
            right: origin.x + available_width,
        });
    let margin_y = placement.y;
    let final_x = if matches!(side, Float::Right) {
        placement.right - margin_width + margins.x0
    } else {
        placement.left + margins.x0
    };
    let final_y = margin_y + margins.y0;
    let final_point = Point::new(final_x, final_y);
    translate_laid_out_output(engine, &layout.output, final_point - initial_point);
    // Exclude the margin box: negative bottom margins let text resume before
    // the border-box bottom, as required for floated drop caps.
    if let Some(context) = engine.floats.current_float_context_mut() {
        context.add_band(FloatBand {
            left: final_x - margins.x0,
            right: final_x + size.width + margins.x1,
            top: margin_y,
            bottom: final_y + size.height + margins.y1,
            side: if matches!(side, Float::Right) {
                FloatSide::Right
            } else {
                FloatSide::Left
            },
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
        context.add_band(FloatBand {
            left: 0.0,
            right: 90.0,
            top: 0.0,
            bottom: 40.0,
            side: FloatSide::Left,
        });
        context.add_band(FloatBand {
            left: 150.0,
            right: 300.0,
            top: 0.0,
            bottom: 80.0,
            side: FloatSide::Right,
        });

        let placement = context.place_margin_box(0.0, 200.0, 20.0, 0.0, 300.0);

        assert_eq!(placement.y, 80.0);
        assert_eq!((placement.left, placement.right), (0.0, 300.0));
    }

    #[test]
    fn placement_checks_the_complete_float_height() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand {
            left: 0.0,
            right: 100.0,
            top: 30.0,
            bottom: 100.0,
            side: FloatSide::Left,
        });

        let placement = context.place_margin_box(0.0, 250.0, 80.0, 0.0, 300.0);

        assert_eq!(placement.y, 100.0);
    }

    #[test]
    fn placement_preserves_the_normal_flow_position_through_a_wrapper() {
        let mut context = FloatContext::default();
        context.note_source_position(30.0);

        let placement = context.place_margin_box(10.0, 20.0, 20.0, 0.0, 300.0);

        assert_eq!(placement.y, 30.0);
        assert_eq!(context.next_y_fitting(10.0, 20.0, 20.0, 0.0, 300.0), 10.0);
    }

    #[test]
    fn available_space_includes_side_by_side_floats_on_the_same_side() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand {
            left: 0.0,
            right: 40.0,
            top: 0.0,
            bottom: 40.0,
            side: FloatSide::Left,
        });
        context.add_band(FloatBand {
            left: 40.0,
            right: 80.0,
            top: 0.0,
            bottom: 40.0,
            side: FloatSide::Left,
        });

        assert_eq!(context.available(0.0, 20.0, 0.0, 300.0), (80.0, 300.0));
    }

    #[test]
    fn oversized_inline_item_can_find_space_below_a_float() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand {
            left: 0.0,
            right: 100.0,
            top: 0.0,
            bottom: 100.0,
            side: FloatSide::Left,
        });

        assert_eq!(context.next_y_fitting(0.0, 300.0, 100.0, 0.0, 300.0), 100.0);
    }

    #[test]
    fn zero_width_item_descends_below_crossed_float_edges() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand {
            left: 0.0,
            right: 250.0,
            top: 100.0,
            bottom: 200.0,
            side: FloatSide::Left,
        });
        context.add_band(FloatBand {
            left: 150.0,
            right: 400.0,
            top: 0.0,
            bottom: 100.0,
            side: FloatSide::Right,
        });

        assert_eq!(context.next_y_fitting(0.0, 0.0, 200.0, 0.0, 400.0), 100.0);
    }

    #[test]
    fn in_flow_item_can_use_space_above_a_later_lower_float() {
        let mut context = FloatContext::default();
        context.add_band(FloatBand {
            left: 0.0,
            right: 80.0,
            top: 0.0,
            bottom: 20.0,
            side: FloatSide::Left,
        });
        context.add_band(FloatBand {
            left: 80.0,
            right: 160.0,
            top: 0.0,
            bottom: 20.0,
            side: FloatSide::Left,
        });
        context.add_band(FloatBand {
            left: 0.0,
            right: 80.0,
            top: 20.0,
            bottom: 40.0,
            side: FloatSide::Left,
        });

        assert_eq!(
            context.next_y_fitting(0.0, 60.0, 20.0, 0.0, 224.0),
            0.0,
            "normal-flow content is not subject to the source-order top constraint used when placing another float"
        );
        assert_eq!(
            context.place_margin_box(0.0, 60.0, 20.0, 0.0, 224.0).y,
            20.0
        );
    }
}
use super::margins::MarginStrut;
use crate::layout::{LayoutEngine, ResolvedBoxSizing, translate_laid_out_output};
use html_style_model::{Clear, Float};
use kurbo::{Insets, Point, Size, Vec2};
