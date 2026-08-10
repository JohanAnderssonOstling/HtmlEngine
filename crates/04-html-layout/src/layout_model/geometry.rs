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
}

impl BoxGeometry {
    pub(crate) fn reset(&mut self, box_count: usize) {
        self.points.resize(box_count, Point::ZERO);
        self.sizes.resize(box_count, Size::ZERO);
        self.decoration_rects.resize(box_count, None);
        self.points.fill(Point::ZERO);
        self.sizes.fill(Size::ZERO);
        self.decoration_rects.fill(None);
    }

    #[inline]
    pub(crate) fn point(&self, box_idx: usize) -> Point {
        self.points[box_idx]
    }

    #[inline]
    pub(crate) fn size(&self, box_idx: usize) -> Size {
        self.sizes[box_idx]
    }

    #[inline]
    pub(crate) fn set_point(&mut self, box_idx: usize, point: Point) {
        self.points[box_idx] = point;
    }

    #[inline]
    pub(crate) fn set_size(&mut self, box_idx: usize, size: Size) {
        self.sizes[box_idx] = size;
    }

    #[inline]
    pub(crate) fn decoration_rect(&self, box_idx: usize) -> Option<Rect> {
        self.decoration_rects[box_idx]
    }

    #[inline]
    pub(crate) fn set_decoration_rect(&mut self, box_idx: usize, rect: Rect) {
        self.decoration_rects[box_idx] = Some(rect);
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<Point>("BoxGeometry.points.storage", self.points.capacity(), self.points.len());
        report.add_slice_storage::<Size>("BoxGeometry.sizes.storage", self.sizes.capacity(), self.sizes.len());
        report.add_slice_storage::<Option<Rect>>("BoxGeometry.decoration_rects.storage", self.decoration_rects.capacity(), self.decoration_rects.len());
        report
    }
}
