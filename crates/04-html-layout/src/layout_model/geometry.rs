use html_dom::MemoryUsageReport;
use kurbo::{Point, Rect, Size};

/// Reusable used geometry indexed in lockstep with immutable box topology.
///
/// Preparation owns box identity and structure. A layout pass may only reset
/// and update this parallel store, so relayout cannot accidentally rewrite the
/// prepared tree.
#[derive(Clone, Default)]
pub(crate) struct BoxGeometry {
    points: Vec<Point>,
    sizes: Vec<Size>,
    decoration_rects: Vec<Option<Rect>>,
    lazy_reset: bool,
    generation: u32,
    generations: Vec<u32>,
}

impl BoxGeometry {
    pub(crate) fn lazy() -> Self {
        Self {
            lazy_reset: true,
            generation: 1,
            ..Self::default()
        }
    }

    pub(crate) fn uses_lazy_reset(&self) -> bool {
        self.lazy_reset
    }

    pub(crate) fn len(&self) -> usize {
        self.points.len()
    }

    pub(crate) fn reset(&mut self, box_count: usize) {
        self.points.resize(box_count, Point::ZERO);
        self.sizes.resize(box_count, Size::ZERO);
        self.decoration_rects.resize(box_count, None);
        if self.lazy_reset {
            self.generations.resize(box_count, 0);
            self.generation = self.generation.wrapping_add(1);
            if self.generation == 0 {
                self.generations.fill(0);
                self.generation = 1;
            }
        } else {
            self.points.fill(Point::ZERO);
            self.sizes.fill(Size::ZERO);
            self.decoration_rects.fill(None);
        }
    }

    #[inline]
    pub(crate) fn point(&self, box_idx: usize) -> Point {
        if self.is_current(box_idx) { self.points[box_idx] } else { Point::ZERO }
    }

    #[inline]
    pub(crate) fn size(&self, box_idx: usize) -> Size {
        if self.is_current(box_idx) { self.sizes[box_idx] } else { Size::ZERO }
    }

    #[inline]
    pub(crate) fn set_point(&mut self, box_idx: usize, point: Point) {
        self.touch(box_idx);
        self.points[box_idx] = point;
    }

    #[inline]
    pub(crate) fn set_size(&mut self, box_idx: usize, size: Size) {
        self.touch(box_idx);
        self.sizes[box_idx] = size;
    }

    #[inline]
    pub(crate) fn decoration_rect(&self, box_idx: usize) -> Option<Rect> {
        self.is_current(box_idx).then(|| self.decoration_rects[box_idx]).flatten()
    }

    #[inline]
    pub(crate) fn set_decoration_rect(&mut self, box_idx: usize, rect: Rect) {
        self.touch(box_idx);
        self.decoration_rects[box_idx] = Some(rect);
    }

    #[inline]
    fn is_current(&self, box_idx: usize) -> bool {
        !self.lazy_reset || self.generations[box_idx] == self.generation
    }

    #[inline]
    fn touch(&mut self, box_idx: usize) {
        if self.lazy_reset && self.generations[box_idx] != self.generation {
            self.points[box_idx] = Point::ZERO;
            self.sizes[box_idx] = Size::ZERO;
            self.decoration_rects[box_idx] = None;
            self.generations[box_idx] = self.generation;
        }
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<Point>(
            "BoxGeometry.points.storage",
            self.points.capacity(),
            self.points.len(),
        );
        report.add_slice_storage::<Size>(
            "BoxGeometry.sizes.storage",
            self.sizes.capacity(),
            self.sizes.len(),
        );
        report.add_slice_storage::<Option<Rect>>(
            "BoxGeometry.decoration_rects.storage",
            self.decoration_rects.capacity(),
            self.decoration_rects.len(),
        );
        report.add_slice_storage::<u32>(
            "BoxGeometry.generations.storage",
            self.generations.capacity(),
            self.generations.len(),
        );
        report
    }
}

#[cfg(test)]
mod tests {
    use super::BoxGeometry;
    use kurbo::{Point, Rect, Size};

    #[test]
    fn lazy_reset_hides_untouched_values_from_the_previous_measurement() {
        let mut geometry = BoxGeometry::lazy();
        geometry.reset(3);
        geometry.set_point(1, Point::new(4.0, 5.0));
        geometry.set_size(1, Size::new(6.0, 7.0));
        geometry.set_decoration_rect(1, Rect::new(1.0, 2.0, 3.0, 4.0));
        geometry.reset(3);
        assert_eq!(geometry.point(1), Point::ZERO);
        assert_eq!(geometry.size(1), Size::ZERO);
        assert_eq!(geometry.decoration_rect(1), None);
        geometry.set_size(1, Size::new(8.0, 9.0));
        assert_eq!(geometry.point(1), Point::ZERO);
        assert_eq!(geometry.size(1), Size::new(8.0, 9.0));
    }
}
