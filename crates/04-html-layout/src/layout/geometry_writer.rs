use crate::layout_model::BoxGeometry;
use kurbo::{Point, Rect, Size};

/// The only mutable access path to box geometry during layout.
pub(crate) struct GeometryWriter<'out> {
    geometry: &'out mut BoxGeometry,
}

impl<'out> GeometryWriter<'out> {
    pub(super) fn new(geometry: &'out mut BoxGeometry) -> Self {
        Self { geometry }
    }

    pub(crate) fn reset(&mut self, box_count: usize) {
        self.geometry.reset(box_count);
    }

    pub(crate) fn point(&self, box_idx: usize) -> Point {
        self.geometry.point(box_idx)
    }

    pub(crate) fn size(&self, box_idx: usize) -> Size {
        self.geometry.size(box_idx)
    }

    pub(crate) fn set_point(&mut self, box_idx: usize, point: Point) {
        self.geometry.set_point(box_idx, point);
    }

    pub(crate) fn set_size(&mut self, box_idx: usize, size: Size) {
        self.geometry.set_size(box_idx, size);
    }

    pub(super) fn len(&self) -> usize {
        self.geometry.len()
    }

    pub(crate) fn set_decoration_rect(&mut self, box_idx: usize, rect: Rect) {
        self.geometry.set_decoration_rect(box_idx, rect);
    }

    pub(crate) fn decoration_rect(&self, box_idx: usize) -> Option<Rect> {
        self.geometry.decoration_rect(box_idx)
    }

    pub(super) fn as_ref(&self) -> &BoxGeometry {
        self.geometry
    }

    pub(super) fn swap_storage(&mut self, scratch: &mut BoxGeometry) {
        std::mem::swap(self.geometry, scratch);
    }
}
