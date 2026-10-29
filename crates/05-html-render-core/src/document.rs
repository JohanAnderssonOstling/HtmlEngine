//! Paint document content through a caller-supplied geometric projection.
//! CSS interpretation stays here; no pagination or scrolling policy lives here.

use crate::{
    FragmentImagePolicy, Painter, color_from_u32, paint_line_glyphs, paint_line_text_runs,
    paint_projected_decoration,
};
use html_pipeline::engine::{RenderDecoration, RenderPaintItem, RenderView};
use html_resources::{DecodedImage, ImagePipeline};
use kurbo::{Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinePlacement {
    pub line: usize,
    pub origin: Point,
}

/// A clipped source rectangle placed at a destination origin, without scaling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintFragment {
    pub source: Rect,
    pub destination: Point,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedLine {
    pub origin: Point,
    pub clip: Option<Rect>,
}

/// A geometry-only contract. A source item may be omitted or placed more than
/// once (for example a repeated header). Implementations never choose CSS order.
pub trait PaintProjection {
    fn bounds(&self) -> Rect;
    fn project_line(&self, line: usize, source: Point, visit: &mut dyn FnMut(ProjectedLine));
    fn project_rect(&self, source: Rect, line: Option<usize>, visit: &mut dyn FnMut(PaintFragment));

    /// Optional visibility candidates. The engine adds line-owned decorations
    /// and restores CSS order; candidates never dictate drawing order.
    fn candidates(
        &self,
        _view: RenderView<'_>,
        _lines: &mut Vec<usize>,
        _decorations: &mut Vec<usize>,
    ) -> bool {
        false
    }
}

/// One continuous document region, suitable for scrolling or standalone HTML.
#[derive(Clone, Copy, Debug)]
pub struct RegionProjection {
    pub source: Rect,
    pub destination: Point,
}

impl PaintProjection for RegionProjection {
    fn bounds(&self) -> Rect {
        Rect::from_origin_size(self.destination, self.source.size())
    }
    fn project_line(&self, _line: usize, source: Point, visit: &mut dyn FnMut(ProjectedLine)) {
        visit(ProjectedLine {
            origin: source + (self.destination - self.source.origin()),
            clip: Some(self.bounds()),
        });
    }
    fn project_rect(
        &self,
        source: Rect,
        _line: Option<usize>,
        visit: &mut dyn FnMut(PaintFragment),
    ) {
        let clipped = source.intersect(self.source);
        if clipped.width() > 0.0 && clipped.height() > 0.0 {
            visit(PaintFragment {
                source: clipped,
                destination: clipped.origin() + (self.destination - self.source.origin()),
            });
        }
    }
}

/// Shared HTML painting. Hosts supply geometry, optional decoded resources,
/// and explicit paint overrides; document styles remain authoritative otherwise.
pub struct DocumentPainter<'a> {
    view: RenderView<'a>,
    images: Option<&'a ImagePipeline>,
    /// Omit suppresses images. Otherwise decoded images are preferred and this
    /// policy controls resources that are not available yet.
    pub image_policy: FragmentImagePolicy,
    pub foreground: Option<u32>,
    pub background: Option<u32>,
    /// Device scale for baseline snapping, independent of geometric projection.
    pub scale: f64,
}

impl<'a> DocumentPainter<'a> {
    pub fn new(view: RenderView<'a>) -> Self {
        Self {
            view,
            images: None,
            image_policy: FragmentImagePolicy::Reference,
            foreground: None,
            background: None,
            scale: 1.0,
        }
    }
    pub fn with_images(mut self, images: &'a ImagePipeline) -> Self {
        self.images = Some(images);
        self
    }

    pub fn paint_region(&self, source: Rect, destination: Point, painter: &mut impl Painter) {
        self.paint(
            &RegionProjection {
                source,
                destination,
            },
            painter,
        );
    }

    pub fn paint(&self, projection: &impl PaintProjection, painter: &mut impl Painter) {
        let bounds = projection.bounds();
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }
        if let Some(color) = self
            .background
            .or_else(|| self.view.canvas_background_color())
        {
            painter.fill_rect(bounds, color_from_u32(color));
        }
        let mut lines = Vec::new();
        let mut decorations = Vec::new();
        if projection.candidates(self.view, &mut lines, &mut decorations) {
            for item in self.view.fragments().paint_order_for(&lines, &decorations) {
                self.paint_item(item, projection, painter);
            }
        } else {
            for item in self.view.fragments().paint_order() {
                self.paint_item(item, projection, painter);
            }
        }
    }

    fn paint_item(
        &self,
        item: RenderPaintItem,
        projection: &impl PaintProjection,
        painter: &mut impl Painter,
    ) {
        let bounds = projection.bounds();
        match item {
            RenderPaintItem::Decoration(decoration) => {
                self.paint_decoration(&decoration, projection, painter)
            }
            RenderPaintItem::Line(index) => {
                let Some(line) = self.view.text().line(index) else {
                    return;
                };
                projection.project_line(index, line.point(), &mut |placed| {
                    self.paint_line(index, placed, bounds, painter)
                });
            }
        }
    }

    fn paint_decoration(
        &self,
        decoration: &RenderDecoration,
        projection: &impl PaintProjection,
        painter: &mut impl Painter,
    ) {
        let mut source = decoration.rect();
        if let Some(overflow) = decoration.overflow_clip() {
            source = overflow.apply_to(source);
        }
        if source.width() <= 0.0 || source.height() <= 0.0 {
            return;
        }
        let owner = decoration.line_idx().or_else(|| {
            decoration
                .is_inline()
                .then(|| {
                    self.view
                        .geometry()
                        .line_intersecting_vertical_range(source.y0, source.y1)
                })
                .flatten()
                .and_then(|rank| {
                    self.view
                        .text()
                        .spatial_order()
                        .get(rank)
                        .map(|id| id.index())
                })
        });
        if let Some(line) = owner.and_then(|index| self.view.text().line(index)) {
            source = source + kurbo::Vec2::new(line.optical_offset_x(), 0.0);
        }
        projection.project_rect(source, owner, &mut |fragment| {
            if fragment.source.width() <= 0.0 || fragment.source.height() <= 0.0 {
                return;
            }
            let rect = Rect::from_origin_size(fragment.destination, fragment.source.size());
            let radii = decoration.radii().map(|mut radii| {
                if fragment.source.x0 > source.x0 {
                    radii.top_left = (0.0, 0.0);
                    radii.bottom_left = (0.0, 0.0);
                }
                if fragment.source.x1 < source.x1 {
                    radii.top_right = (0.0, 0.0);
                    radii.bottom_right = (0.0, 0.0);
                }
                if fragment.source.y0 > source.y0 {
                    radii.top_left = (0.0, 0.0);
                    radii.top_right = (0.0, 0.0);
                }
                if fragment.source.y1 < source.y1 {
                    radii.bottom_left = (0.0, 0.0);
                    radii.bottom_right = (0.0, 0.0);
                }
                radii
            });
            paint_projected_decoration(painter, decoration, rect, radii);
        });
    }

    fn paint_line(
        &self,
        index: usize,
        placement: ProjectedLine,
        bounds: Rect,
        painter: &mut impl Painter,
    ) {
        let Some(line) = self.view.text().line(index) else {
            return;
        };
        let clip = placement.clip.map(|clip| clip.intersect(bounds));
        if let Some(clip) = clip {
            let vertical = self.view.geometry().line_vertical_bounds(index);
            if vertical.is_some_and(|vertical| {
                let offset = placement.origin.y - line.point_y();
                vertical.end + offset <= clip.y0 || vertical.start + offset >= clip.y1
            }) {
                return;
            }
        }
        let overflow = self.view.text().line_overflow_clip(index).map(|overflow| {
            let delta = placement.origin - line.point();
            overflow.apply_to(bounds - delta) + delta
        });
        let clip = match (clip, overflow) {
            (Some(a), Some(b)) => Some(a.intersect(b)),
            (a, b) => a.or(b),
        };
        if clip.is_some_and(|clip| clip.width() <= 0.0 || clip.height() <= 0.0) {
            return;
        }
        if let Some(clip) = clip {
            painter.push_clip(clip);
        }
        for image in self.view.fragments().images_for_line(index).iter() {
            if matches!(self.image_policy, FragmentImagePolicy::Omit) {
                continue;
            }
            let rect = image.rect() + placement.origin.to_vec2();
            let image_clip = image.clip() + placement.origin.to_vec2();
            let visible = image.visible_rect() + placement.origin.to_vec2();
            if visible.is_zero_area() {
                continue;
            }
            painter.push_clip(image_clip);
            match self
                .images
                .and_then(|images| images.get_decoded(image.image_idx()))
            {
                Some(DecodedImage::Raster { image, hash, .. }) => {
                    painter.draw_image(image, hash, rect)
                }
                Some(DecodedImage::Svg {
                    bytes,
                    hash,
                    width,
                    height,
                }) if self.view.export().is_equation(image.image_idx()) => {
                    painter.draw_math_svg(bytes, hash, (*width, *height), rect, image.text_color())
                }
                Some(DecodedImage::Svg {
                    bytes,
                    hash,
                    width,
                    height,
                }) => painter.draw_svg(bytes, hash, (*width, *height), rect),
                None => match self.image_policy {
                    FragmentImagePolicy::Omit => {}
                    FragmentImagePolicy::Placeholder(color) => {
                        painter.fill_rect(visible, color_from_u32(color))
                    }
                    FragmentImagePolicy::Reference => painter.draw_resource_image(
                        image.image_idx(),
                        self.view.image_uri(image.image_idx()),
                        rect,
                    ),
                },
            }
            painter.pop_clip();
        }
        let color = self.foreground.or_else(|| line.paint_color());
        if !paint_line_text_runs(
            self.view,
            index,
            placement.origin,
            self.scale,
            color,
            painter,
        ) {
            paint_line_glyphs(
                self.view,
                index,
                placement.origin,
                self.scale,
                color,
                painter,
            );
        }
        if clip.is_some() {
            painter.pop_clip();
        }
    }
}
