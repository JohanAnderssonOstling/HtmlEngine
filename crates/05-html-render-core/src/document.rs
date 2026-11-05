//! Paint document content through a caller-supplied geometric projection.
//! CSS interpretation stays here; no pagination or scrolling policy lives here.

use html_pipeline::engine::export_queries;
use html_pipeline::engine::geometry_queries;
use html_resources::resources::images as image_resources;
use html_pipeline::engine::fragment_paint;
use html_pipeline::engine::text_paint;
use crate::{
    Painter, SceneImagePolicy, color_from_u32, paint_line_glyphs, paint_line_text_runs,
    paint_projected_decoration,
};
use html_pipeline::engine::{BoxId, LineId, RenderDecoration, RenderPaintItem, RenderView};
use html_resources::{DecodedImage, DecodedImages};
use kurbo::{Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinePlacement {
    pub line: LineId,
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

/// Source items that a projection may display. The engine includes line-owned
/// decorations, removes duplicate candidates and restores CSS paint order.
/// Empty lists select no items; `None` from [`PaintProjection::candidates`]
/// requests traversal of the complete document instead.
#[derive(Default)]
pub struct PaintCandidates {
    pub lines: Vec<LineId>,
    pub block_decorations: Vec<usize>,
}

/// A geometry-only contract. A source item may be omitted or placed more than
/// once (for example a repeated header). Implementations never choose CSS order.
/// Projections append their placements to `out`; the engine clears and reuses
/// the buffer between items.
pub trait PaintProjection {
    fn bounds(&self) -> Rect;
    fn project_line(&self, line: LineId, source: Point, out: &mut Vec<ProjectedLine>);
    fn project_rect(&self, source: Rect, line: Option<LineId>, out: &mut Vec<PaintFragment>);

    /// Projects a decoration with its retained source ownership. Region
    /// projections can use the ordinary rectangle mapping; fragmented pages
    /// may follow only the owner's placed content.
    fn project_decoration(
        &self,
        _owner: Option<BoxId>,
        source: Rect,
        line: Option<LineId>,
        out: &mut Vec<PaintFragment>,
    ) {
        self.project_rect(source, line, out);
    }

    /// Optional visibility candidates. The engine adds line-owned decorations
    /// and restores CSS order; candidates never dictate drawing order.
    fn candidates(&self, _view: RenderView<'_>) -> Option<PaintCandidates> {
        None
    }
}

/// Reusable projection output for one paint pass.
#[derive(Default)]
struct Placements {
    lines: Vec<ProjectedLine>,
    fragments: Vec<PaintFragment>,
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
    fn project_line(&self, _line: LineId, source: Point, out: &mut Vec<ProjectedLine>) {
        out.push(ProjectedLine {
            origin: source + (self.destination - self.source.origin()),
            clip: Some(self.bounds()),
        });
    }
    fn project_rect(&self, source: Rect, _line: Option<LineId>, out: &mut Vec<PaintFragment>) {
        let clipped = source.intersect(self.source);
        if clipped.width() > 0.0 && clipped.height() > 0.0 {
            out.push(PaintFragment {
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
    images: Option<DecodedImages<'a>>,
    /// Omit suppresses images. Otherwise decoded images are preferred and this
    /// policy controls resources that are not available yet.
    pub image_policy: SceneImagePolicy,
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
            image_policy: SceneImagePolicy::Reference,
            foreground: None,
            background: None,
            scale: 1.0,
        }
    }
    pub fn with_images(mut self, images: DecodedImages<'a>) -> Self {
        self.images = Some(images);
        self
    }

    pub fn paint(&self, projection: &impl PaintProjection, painter: &mut impl Painter) {
        if !text_paint::paint_resources_valid(self.view) {
            return;
        }
        let bounds = projection.bounds();
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }
        if let Some(color) = self
            .background
            .or_else(|| text_paint::render_view_canvas_background_color(&self.view))
        {
            painter.fill_rect(bounds, color_from_u32(color));
        }
        let fragments = self.view.fragments();
        let mut placements = Placements::default();
        if let Some(candidates) = projection.candidates(self.view) {
            for item in fragment_paint::paint_order_for(
                fragments,
                &candidates.lines,
                &candidates.block_decorations,
            ) {
                self.paint_item(item, projection, &mut placements, painter);
            }
        } else {
            for item in fragment_paint::paint_order(fragments) {
                self.paint_item(item, projection, &mut placements, painter);
            }
        }
    }

    fn paint_item(
        &self,
        item: RenderPaintItem,
        projection: &impl PaintProjection,
        placements: &mut Placements,
        painter: &mut impl Painter,
    ) {
        let bounds = projection.bounds();
        match item {
            RenderPaintItem::Decoration(decoration) => {
                self.paint_decoration(&decoration, projection, &mut placements.fragments, painter)
            }
            RenderPaintItem::Line(index) => {
                let Some(line) = self.view.text().line(index) else {
                    return;
                };
                placements.lines.clear();
                projection.project_line(index, line.point(), &mut placements.lines);
                for &placed in &placements.lines {
                    self.paint_line(index, placed, bounds, painter);
                }
            }
        }
    }

    fn paint_decoration(
        &self,
        decoration: &RenderDecoration,
        projection: &impl PaintProjection,
        fragments: &mut Vec<PaintFragment>,
        painter: &mut impl Painter,
    ) {
        let mut source = decoration.rect();
        if let Some(overflow) = fragment_paint::decoration_overflow_clip(&decoration) {
            source = overflow.apply_to(source);
        }
        if source.width() <= 0.0 || source.height() <= 0.0 {
            return;
        }
        let owner = fragment_paint::render_decoration_line(&decoration).or_else(|| {
            decoration
                .is_inline()
                .then(|| {
                    geometry_queries::render_geometry_view_line_intersecting_vertical_range(
                        geometry_queries::render_view_geometry(self.view),
                        source.y0,
                        source.y1,
                    )
                })
                .flatten()
        });
        if let Some(line) = owner.and_then(|index| self.view.text().line(index)) {
            source = source + kurbo::Vec2::new(text_paint::line_optical_offset_x(&line), 0.0);
        }
        fragments.clear();
        projection.project_decoration(decoration.owner_box(), source, owner, fragments);
        for &fragment in fragments.iter() {
            if fragment.source.width() <= 0.0 || fragment.source.height() <= 0.0 {
                continue;
            }
            let rect = Rect::from_origin_size(fragment.destination, fragment.source.size());
            let radii = fragment_paint::decoration_radii(&decoration).map(|mut radii| {
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
        }
    }

    fn paint_line(
        &self,
        index: LineId,
        placement: ProjectedLine,
        bounds: Rect,
        painter: &mut impl Painter,
    ) {
        let Some(line) = self.view.text().line(index) else {
            return;
        };
        let clip = placement.clip.map(|clip| clip.intersect(bounds));
        if let Some(clip) = clip {
            let vertical = geometry_queries::render_geometry_view_line_vertical_bounds(
                geometry_queries::render_view_geometry(self.view),
                index,
            );
            if vertical.is_some_and(|vertical| {
                let offset = placement.origin.y - line.point().y;
                vertical.end + offset <= clip.y0 || vertical.start + offset >= clip.y1
            }) {
                return;
            }
        }
        let overflow = text_paint::line_overflow_clip(self.view.text(), index).map(|overflow| {
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
        for image in
            fragment_paint::render_fragment_view_images_for_line(self.view.fragments(), index)
                .iter()
        {
            if matches!(self.image_policy, SceneImagePolicy::Omit) {
                continue;
            }
            let rect =
                fragment_paint::render_image_fragment_rect(&image) + placement.origin.to_vec2();
            let image_clip =
                fragment_paint::render_image_fragment_clip(&image) + placement.origin.to_vec2();
            let visible = fragment_paint::render_image_fragment_visible_rect(&image)
                + placement.origin.to_vec2();
            if visible.is_zero_area() {
                continue;
            }
            painter.push_clip(image_clip);
            match self
                .images
                .and_then(|images| image_resources::decoded_images_get(images, image.image()))
            {
                Some(DecodedImage::Raster { image, hash, .. }) => {
                    painter.draw_image(image, hash, rect)
                }
                Some(DecodedImage::Svg {
                    bytes,
                    hash,
                    width,
                    height,
                }) => {
                    let size = (*width, *height);
                    if export_queries::render_export_view_is_equation(
                        self.view.export(),
                        image.image(),
                    ) {
                        painter.draw_math_svg(
                            bytes,
                            hash,
                            size,
                            rect,
                            fragment_paint::image_text_color(&image),
                        );
                    } else {
                        painter.draw_svg(bytes, hash, size, rect);
                    }
                }
                None => match self.image_policy {
                    SceneImagePolicy::Omit => {}
                    SceneImagePolicy::Placeholder(color) => {
                        painter.fill_rect(visible, color_from_u32(color))
                    }
                    SceneImagePolicy::Reference => painter.draw_resource_image(
                        image.image(),
                        text_paint::render_view_image_uri(&self.view, image.image()),
                        rect,
                    ),
                },
            }
            painter.pop_clip();
        }
        let color = self
            .foreground
            .or_else(|| text_paint::line_paint_color(&line));
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

pub fn document_painter_paint_region<'a>(
    value: &DocumentPainter<'a>,
    source: Rect,
    destination: Point,
    painter: &mut impl Painter,
) {
    value.paint(
        &RegionProjection {
            source,
            destination,
        },
        painter,
    );
}
