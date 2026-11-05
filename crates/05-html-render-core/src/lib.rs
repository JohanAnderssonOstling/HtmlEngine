#[cfg(test)]
use html_engine_model::layout_state;
#[cfg(test)]
use html_style_model::computed_style;
#[cfg(test)]
use html_engine_model::shaping_contracts;
use html_pipeline::engine::geometry_queries;
use document::document_painter_paint_region;
use html_pipeline::engine::fragment_paint;
use html_pipeline::engine::{box_queries, text_paint};
mod document;
pub use document::{
    DocumentPainter, LinePlacement, PaintCandidates, PaintFragment, PaintProjection, ProjectedLine,
    RegionProjection,
};

pub use html_pipeline::engine::CornerRadii;
use html_pipeline::engine::{
    GlyphHandle, ImageId, LayoutConstraintError, LineId, PreparedTextRunFragment, RenderView,
    TextRunId,
};
use html_resources::DecodedImages;
use kurbo::{Point, Rect, Size};
use peniko::{Color, Image};
use std::ops::Range;
use std::sync::Arc;

fn pixel_snapper(scale: f64) -> impl Fn(f64) -> f64 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    move |value| (value * scale).round() / scale
}

pub trait Painter {
    /// Restricts subsequent paint operations until the matching `pop_clip`.
    /// Implementations must intersect nested clips and restore them on pop.
    fn push_clip(&mut self, rect: Rect);
    fn pop_clip(&mut self);
    fn fill_rect(&mut self, rect: Rect, color: Color);
    /// Optional rounded primitive. Existing adapters intentionally get a safe
    /// square fallback, so adding CSS radius support is not a breaking change.
    fn fill_rounded_rect(&mut self, rect: Rect, _radii: CornerRadii, color: Color) {
        self.fill_rect(rect, color);
    }
    /// Optional uniform solid rounded border primitive.
    fn stroke_rounded_rect(
        &mut self,
        rect: Rect,
        _radii: CornerRadii,
        width: f32,
        color: Color,
    ) {
        let width = (width as f64).max(0.0).min(rect.width().min(rect.height()));
        if width == 0.0 {
            return;
        }
        for edge in [
            Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + width),
            Rect::new(rect.x0, rect.y1 - width, rect.x1, rect.y1),
            Rect::new(rect.x0, rect.y0 + width, rect.x0 + width, rect.y1 - width),
            Rect::new(rect.x1 - width, rect.y0 + width, rect.x1, rect.y1 - width),
        ] {
            self.fill_rect(edge, color);
        }
    }
    /// Draw a native glyph at its baseline using the resolved RGBA foreground color.
    fn draw_glyph(&mut self, glyph: GlyphHandle, origin: Point, color: u32);
    /// True requires range-aware fragment drawing or a prepared-batch override.
    fn supports_text_runs(&self) -> bool {
        false
    }
    /// Draw a native run slice at its baseline. The default prepared-batch
    /// implementation uses this callback after translating line-relative offsets.
    fn draw_text_run_fragment(
        &mut self,
        _run: TextRunId,
        _range: Range<u32>,
        _origin: Point,
        _color: Option<u32>,
    ) {
        panic!("native text painters must implement range-aware fragments or prepared batches");
    }
    /// Draws layout-prepared, line-relative shaped-run slices.
    ///
    /// The default translates the retained fragments without allocating an
    /// intermediate display list. Backends may override this to submit the
    /// complete line as one native batch. Overrides must honor the supplied
    /// color, or each fragment's color when no line color is supplied.
    fn draw_prepared_text_run_batch(
        &mut self,
        fragments: &[PreparedTextRunFragment],
        line_origin: Point,
        scale: f64,
        color: Option<u32>,
    ) {
        let snap = pixel_snapper(scale);
        for fragment in fragments {
            let offset = fragment.offset;
            self.draw_text_run_fragment(
                fragment.run,
                fragment.range.clone(),
                Point::new(line_origin.x + offset.x, snap(line_origin.y + offset.y)),
                color.or(fragment.color),
            );
        }
    }
    fn draw_image(&mut self, image: &Image, hash: &[u8], rect: Rect);
    fn draw_svg(&mut self, _bytes: &[u8], _hash: &[u8], _intrinsic_size: (u32, u32), _rect: Rect) {}
    /// Inherited equation ink can change with the reader theme without
    /// regenerating geometry. Explicit colors in the SVG remain intact.
    fn draw_math_svg(
        &mut self,
        bytes: &[u8],
        hash: &[u8],
        intrinsic_size: (u32, u32),
        rect: Rect,
        _color: u32,
    ) {
        self.draw_svg(bytes, hash, intrinsic_size, rect);
    }
    fn draw_resource_image(&mut self, _image_idx: ImageId, _uri: Option<&str>, _rect: Rect) {}
}

#[derive(Default)]
pub struct RecordingPainter {
    pub clips: Vec<Rect>,
    pub clip_pops: usize,
    pub glyphs: Vec<(GlyphHandle, Point)>,
    pub glyph_colors: Vec<u32>,
    pub fills: Vec<(Rect, Color)>,
    pub images: Vec<Rect>,
    pub svgs: Vec<Rect>,
    pub resource_images: Vec<(ImageId, Option<String>, Rect)>,
    pub rounded_fills: Vec<(Rect, CornerRadii, Color)>,
    pub rounded_borders: Vec<(Rect, CornerRadii, f32, Color)>,
}

impl Painter for RecordingPainter {
    fn push_clip(&mut self, rect: Rect) {
        self.clips.push(rect);
    }

    fn pop_clip(&mut self) {
        self.clip_pops += 1;
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.fills.push((rect, color));
    }

    fn fill_rounded_rect(&mut self, rect: Rect, radii: CornerRadii, color: Color) {
        self.rounded_fills.push((rect, radii, color));
    }

    fn stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radii: CornerRadii,
        width: f32,
        color: Color,
    ) {
        self.rounded_borders.push((rect, radii, width, color));
    }

    fn draw_glyph(&mut self, glyph: GlyphHandle, origin: Point, color: u32) {
        self.glyphs.push((glyph, origin));
        self.glyph_colors.push(color);
    }

    fn draw_image(&mut self, _image: &Image, _hash: &[u8], rect: Rect) {
        self.images.push(rect);
    }

    fn draw_svg(&mut self, _bytes: &[u8], _hash: &[u8], _intrinsic_size: (u32, u32), rect: Rect) {
        self.svgs.push(rect);
    }

    fn draw_resource_image(&mut self, image_idx: ImageId, uri: Option<&str>, rect: Rect) {
        self.resource_images
            .push((image_idx, uri.map(str::to_owned), rect));
    }
}

enum RenderCommand {
    PushClip {
        rect: Rect,
    },
    PopClip,
    Fill {
        rect: Rect,
        color: Color,
    },
    RoundedFill {
        rect: Rect,
        radii: CornerRadii,
        color: Color,
    },
    RoundedBorder {
        rect: Rect,
        radii: CornerRadii,
        width: f32,
        color: Color,
    },
    Glyph {
        glyph: GlyphHandle,
        origin: Point,
        color: u32,
    },
    Image {
        image: Image,
        hash: Vec<u8>,
        rect: Rect,
    },
    Svg {
        bytes: Arc<[u8]>,
        hash: Vec<u8>,
        intrinsic_size: (u32, u32),
        rect: Rect,
        text_color: Option<u32>,
    },
    ResourceImage {
        image_idx: ImageId,
        uri: Option<String>,
        rect: Rect,
    },
}

#[derive(Clone)]
pub struct RenderScene {
    size: Size,
    content_height: f64,
    clipped: bool,
    commands: Arc<[RenderCommand]>,
    resources: Option<html_pipeline::engine::RenderResourceLease>,
}

impl RenderScene {
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn content_height(&self) -> f64 {
        self.content_height
    }

    /// Returns false without submitting commands after resource invalidation.
    pub fn paint(&self, painter: &mut impl Painter) -> bool {
        if !render_scene_is_valid(&self) {
            return false;
        }
        for command in self.commands.iter() {
            match command {
                RenderCommand::PushClip { rect } => painter.push_clip(*rect),
                RenderCommand::PopClip => painter.pop_clip(),
                RenderCommand::Fill { rect, color } => painter.fill_rect(*rect, *color),
                RenderCommand::RoundedFill { rect, radii, color } => {
                    painter.fill_rounded_rect(*rect, *radii, *color)
                }
                RenderCommand::RoundedBorder {
                    rect,
                    radii,
                    width,
                    color,
                } => painter.stroke_rounded_rect(*rect, *radii, *width, *color),
                RenderCommand::Glyph {
                    glyph,
                    origin,
                    color,
                } => painter.draw_glyph(*glyph, *origin, *color),
                RenderCommand::Image { image, hash, rect } => {
                    painter.draw_image(image, hash, *rect)
                }
                RenderCommand::Svg {
                    bytes,
                    hash,
                    intrinsic_size,
                    rect,
                    text_color,
                } => match text_color {
                    Some(color) => {
                        painter.draw_math_svg(bytes, hash, *intrinsic_size, *rect, *color)
                    }
                    None => painter.draw_svg(bytes, hash, *intrinsic_size, *rect),
                },
                RenderCommand::ResourceImage {
                    image_idx,
                    uri,
                    rect,
                } => painter.draw_resource_image(*image_idx, uri.as_deref(), *rect),
            }
        }
        true
    }
}

/// How a recorded scene represents replaced images.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneImagePolicy {
    Omit,
    Placeholder(u32),
    Reference,
}

impl Default for SceneImagePolicy {
    fn default() -> Self {
        Self::Placeholder(0xe6e6e6ff)
    }
}

/// The region and image representation of a recorded scene.
#[derive(Clone, Copy, Debug)]
pub struct SceneOptions {
    viewport_width: f64,
    maximum_height: Option<f64>,
    image_policy: SceneImagePolicy,
}

impl SceneOptions {
    pub fn new(
        viewport_width: f64,
        maximum_height: Option<f64>,
    ) -> Result<Self, LayoutConstraintError> {
        if !viewport_width.is_finite() || viewport_width <= 0.0 {
            return Err(LayoutConstraintError::InvalidViewportWidth);
        }
        if maximum_height.is_some_and(|height| !height.is_finite() || height <= 0.0) {
            return Err(LayoutConstraintError::InvalidViewportHeight);
        }
        Ok(Self {
            viewport_width,
            maximum_height,
            image_policy: SceneImagePolicy::default(),
        })
    }

    pub fn with_image_policy(mut self, image_policy: SceneImagePolicy) -> Self {
        self.image_policy = image_policy;
        self
    }

    pub fn viewport_width(&self) -> f64 {
        self.viewport_width
    }

    pub fn maximum_height(&self) -> Option<f64> {
        self.maximum_height
    }

    pub fn image_policy(&self) -> SceneImagePolicy {
        self.image_policy
    }
}

pub(crate) fn paint_line_glyphs<'a>(
    document: impl Into<RenderView<'a>>,
    line_idx: LineId,
    origin: Point,
    scale: f64,
    color: Option<u32>,
    painter: &mut impl Painter,
) {
    let view = document.into();
    for fragment in text_paint::render_text_view_line_text_fragments(view.text(), line_idx)
        .into_iter()
        .flatten()
    {
        paint_line_glyph_fragment(view, line_idx, &fragment, origin, scale, color, painter);
    }
    paint_line_ellipsis(view, line_idx, origin, scale, color, painter);
}

/// Paints one sparse text fragment from a line that requires source-ordered
/// interleaving with inline decorations or replaced content.
pub(crate) fn paint_line_glyph_fragment<'a>(
    document: impl Into<RenderView<'a>>,
    line_idx: LineId,
    fragment: &html_pipeline::engine::RenderLineTextFragment,
    origin: Point,
    scale: f64,
    color: Option<u32>,
    painter: &mut impl Painter,
) {
    let document = document.into();
    if !text_paint::paint_resources_valid(document) || !fragment.visible() {
        return;
    }
    let text = document.text();
    let Some(line) = text.line(line_idx) else {
        return;
    };
    let snap = pixel_snapper(scale);
    let baseline_y = origin.y + text_paint::line_baseline(&line);
    let Some(mut placement) = text_paint::line_placement(text, line_idx) else {
        return;
    };
    let mut current_x = origin.x
        + text_paint::line_optical_offset_x(&line)
        + text_paint::fragment_offset_x(&fragment);
    for index in fragment.glyphs() {
        let Some(character) = placement.at(index) else {
            continue;
        };
        // Preserve fractional inline origins; only project the baseline onto
        // the device pixel grid, just as native prepared-run painting does.
        let draw_point = Point::new(current_x, snap(baseline_y - character.offset_y));
        if let Some(glyph) = character
            .paint_glyph
            .and_then(|glyph| text_paint::paint_handle(text, glyph))
        {
            let color = color
                .or_else(|| text_paint::paint_color_at(text, index))
                .expect("painted text has a resolved foreground color");
            painter.draw_glyph(glyph, draw_point, color);
        }
        current_x += character.advance;
    }
}

/// Paints a line from document-owned native shaping resources. Layout
/// placement adjustments are represented as positioned slices of those runs;
/// they never require abandoning retained shaping. Returns false only when the
/// resources are invalid, the painter has no native-run support, or shaping
/// coverage is incomplete.
pub(crate) fn paint_line_text_runs<'a>(
    document: impl Into<RenderView<'a>>,
    line_idx: LineId,
    origin: Point,
    scale: f64,
    color: Option<u32>,
    painter: &mut impl Painter,
) -> bool {
    let document = document.into();
    if !text_paint::paint_resources_valid(document) || !painter.supports_text_runs() {
        return false;
    }
    let Some(fragments) = text_paint::prepared_text_runs(document.text(), line_idx) else {
        return false;
    };
    painter.draw_prepared_text_run_batch(fragments, origin, scale, color);
    paint_line_ellipsis(document, line_idx, origin, scale, color, painter);
    true
}

/// Paints the synthetic overflow marker separately from selectable source
/// text. Native text-run painters can use this without falling back to drawing
/// the source line character by character.
pub(crate) fn paint_line_ellipsis<'a>(
    document: impl Into<RenderView<'a>>,
    line_idx: LineId,
    origin: Point,
    scale: f64,
    color: Option<u32>,
    painter: &mut impl Painter,
) {
    let document = document.into();
    if !text_paint::paint_resources_valid(document) {
        return;
    }
    let text = document.text();
    let Some(line) = text.line(line_idx) else {
        return;
    };
    let snap = pixel_snapper(scale);
    let baseline_y = origin.y + text_paint::line_baseline(&line);
    let mut paint = |glyph, offset: Point, source: Option<u32>| {
        if let Some(glyph) = text_paint::paint_handle(text, glyph) {
            let point = Point::new(
                // Synthetic text uses the same fractional inline origin as
                // ordinary glyphs and shaped runs. Snapping its x coordinate
                // changes the raster phase at discretionary hyphens.
                origin.x + text_paint::line_optical_offset_x(&line) + offset.x,
                snap(baseline_y - offset.y),
            );
            let color = color
                .or_else(|| source.and_then(|index| text_paint::paint_color_at(text, index)))
                .unwrap_or_else(|| {
                    box_queries::text_format(document.boxes(), text_paint::render_line_owner_box(&line)).color
                });
            painter.draw_glyph(glyph, point, color);
        }
    };
    if let Some(ellipsis) = text_paint::ellipsis_for_line(text, line_idx)
        && ellipsis.visible()
    {
        paint(ellipsis.glyph(), ellipsis.offset(), None);
    }
    if let Some(hyphen) = text_paint::hyphen_for_line(text, line_idx)
        && hyphen.visible()
    {
        paint(
            hyphen.glyph(),
            hyphen.offset(),
            Some(line.end().saturating_sub(1)),
        );
    }
}

fn build_fragment_view(
    view: RenderView<'_>,
    options: &SceneOptions,
    images: Option<DecodedImages<'_>>,
) -> RenderScene {
    let text = view.text();
    let fragments = view.fragments();
    let natural_height = text
        .lines()
        .iter()
        .filter_map(|line| {
            geometry_queries::render_geometry_view_line_vertical_bounds(
                geometry_queries::render_view_geometry(view),
                line.id(),
            )
        })
        .map(|bounds| bounds.end)
        .fold(0.0_f64, f64::max);
    let natural_height = fragment_paint::decorations(fragments)
        .iter()
        .map(|decoration| decoration.rect().y1)
        .fold(natural_height, f64::max);
    let height = options
        .maximum_height
        .map(|maximum| natural_height.min(maximum))
        .unwrap_or(natural_height);
    let clip = Rect::new(0.0, 0.0, options.viewport_width, height);
    let mut recorder = SceneRecorder::default();
    let mut painter = DocumentPainter::new(view);
    if let Some(images) = images {
        painter = painter.with_images(images);
    }
    painter.image_policy = options.image_policy;
    document_painter_paint_region(&painter, clip, Point::ORIGIN, &mut recorder);
    RenderScene {
        size: Size::new(options.viewport_width, height),
        content_height: natural_height,
        clipped: height < natural_height,
        commands: recorder.commands.into(),
        resources: text_paint::retain_paint_resources(view),
    }
}

fn color_from_u32(color: u32) -> Color {
    Color::rgba8(
        (color >> 24) as u8,
        (color >> 16) as u8,
        (color >> 8) as u8,
        color as u8,
    )
}

/// Paint a semantic decoration after an outer projection (pagination,
/// columns, or clipping) has adjusted its rectangle and exposed corners.
/// CSS color, rounded fill, and border execution have one shared owner here.
pub(crate) fn paint_projected_decoration(
    painter: &mut impl Painter,
    decoration: &html_pipeline::engine::RenderDecoration,
    rect: Rect,
    radii: Option<CornerRadii>,
) {
    use html_pipeline::engine::RenderDecorationPattern;

    let color = color_from_u32(fragment_paint::decoration_color(&decoration));
    let (vertical, dash_units, gap_units) = match fragment_paint::decoration_pattern(&decoration) {
        RenderDecorationPattern::Solid => {
            paint_decoration_style(
                painter,
                rect,
                fragment_paint::decoration_color(&decoration),
                radii,
                decoration.border_width(),
            );
            return;
        }
        RenderDecorationPattern::DoubleHorizontal => {
            let thickness = rect.height() / 3.0;
            if thickness > 0.0 {
                for stripe in [
                    Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + thickness),
                    Rect::new(rect.x0, rect.y1 - thickness, rect.x1, rect.y1),
                ] {
                    painter.fill_rect(stripe, color);
                }
            }
            return;
        }
        RenderDecorationPattern::DottedHorizontal => (false, 1.0, 1.0),
        RenderDecorationPattern::DashedHorizontal => (false, 3.0, 2.0),
        RenderDecorationPattern::DottedVertical => (true, 1.0, 1.0),
        RenderDecorationPattern::DashedVertical => (true, 3.0, 2.0),
    };
    let (thickness, extent, mut position, limit) = if vertical {
        (rect.width(), rect.height(), rect.y0, rect.y1)
    } else {
        (rect.height(), rect.width(), rect.x0, rect.x1)
    };
    if thickness <= 0.0 || extent <= 0.0 {
        return;
    }
    let dash = thickness * dash_units;
    let gap = thickness * gap_units;
    while position < limit {
        let end = (position + dash).min(limit);
        let segment = if vertical {
            Rect::new(rect.x0, position, rect.x1, end)
        } else {
            Rect::new(position, rect.y0, end, rect.y1)
        };
        painter.fill_rect(segment, color);
        position = end + gap;
    }
}

/// Paints a resolved decoration after an outer projection (for example,
/// pagination) has adjusted which physical corners remain exposed.
pub(crate) fn paint_decoration_style(
    painter: &mut impl Painter,
    rect: Rect,
    color: u32,
    radii: Option<CornerRadii>,
    border_width: Option<f32>,
) {
    let color = color_from_u32(color);
    match (radii, border_width) {
        (Some(radii), Some(width)) => painter.stroke_rounded_rect(rect, radii, width, color),
        (Some(radii), None) => painter.fill_rounded_rect(rect, radii, color),
        (None, _) => painter.fill_rect(rect, color),
    }
}

#[derive(Default)]
struct SceneRecorder {
    commands: Vec<RenderCommand>,
}

impl Painter for SceneRecorder {
    fn push_clip(&mut self, rect: Rect) {
        self.commands.push(RenderCommand::PushClip { rect });
    }

    fn pop_clip(&mut self) {
        self.commands.push(RenderCommand::PopClip);
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.commands.push(RenderCommand::Fill { rect, color });
    }

    fn fill_rounded_rect(&mut self, rect: Rect, radii: CornerRadii, color: Color) {
        self.commands
            .push(RenderCommand::RoundedFill { rect, radii, color });
    }

    fn stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radii: CornerRadii,
        width: f32,
        color: Color,
    ) {
        self.commands.push(RenderCommand::RoundedBorder {
            rect,
            radii,
            width,
            color,
        });
    }

    fn draw_glyph(&mut self, glyph: GlyphHandle, origin: Point, color: u32) {
        self.commands.push(RenderCommand::Glyph {
            glyph,
            origin,
            color,
        });
    }

    fn draw_image(&mut self, image: &Image, hash: &[u8], rect: Rect) {
        self.commands.push(RenderCommand::Image {
            image: image.clone(),
            hash: hash.to_vec(),
            rect,
        });
    }

    fn draw_svg(&mut self, bytes: &[u8], hash: &[u8], intrinsic_size: (u32, u32), rect: Rect) {
        self.commands.push(RenderCommand::Svg {
            bytes: Arc::from(bytes),
            hash: hash.to_vec(),
            intrinsic_size,
            rect,
            text_color: None,
        });
    }

    fn draw_math_svg(
        &mut self,
        bytes: &[u8],
        hash: &[u8],
        intrinsic_size: (u32, u32),
        rect: Rect,
        color: u32,
    ) {
        self.commands.push(RenderCommand::Svg {
            bytes: Arc::from(bytes),
            hash: hash.to_vec(),
            intrinsic_size,
            rect,
            text_color: Some(color),
        });
    }

    fn draw_resource_image(&mut self, image_idx: ImageId, uri: Option<&str>, rect: Rect) {
        self.commands.push(RenderCommand::ResourceImage {
            image_idx,
            uri: uri.map(str::to_owned),
            rect,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use html_pipeline::engine::{
        GlyphResourceStore, GlyphShaper, LaidOutDocument, LayoutConstraints,
    };
    use html_pipeline::{
        MarkupSyntax, MediaEnvironment, PipelineError, PipelineInputs, PipelineSession,
        StyleEnvironment,
    };
    use html_resources::ResourceProvider;
    use std::io;
    use html_pipeline::engine::GlyphResourceGeneration;
    use html_pipeline::engine::GlyphRequest;
    use html_pipeline::engine::FontMetrics;
    use html_pipeline::engine::ShapedGlyph;
    use html_pipeline::engine::{
        ShapeError, ShapedRun, ShapedTextRun, TextDirection, TextRunShapeRequest,
    };
    use std::collections::HashMap;

    struct DenyResourceProvider;

    impl ResourceProvider for DenyResourceProvider {
        fn read_bytes(&self, _uri: &str) -> io::Result<Vec<u8>> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "fragment external resources are disabled",
            ))
        }

        fn exists(&self, _uri: &str) -> bool {
            false
        }

        fn resolve(&self, _base: &str, href: &str) -> String {
            href.to_owned()
        }

        fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    /// Lays out one source string per call, replacing the previous fragment.
    struct FragmentRenderer {
        session: PipelineSession,
    }

    struct RenderedFragment<'a> {
        document: &'a LaidOutDocument<'a>,
        resources: &'a GlyphResourceStore,
        scene: RenderScene,
    }

    impl RenderedFragment<'_> {
        fn render_view(&self) -> RenderView<'_> {
            self.document.render_view(self.resources)
        }

        fn scene(&self) -> &RenderScene {
            &self.scene
        }

        fn into_scene(self) -> RenderScene {
            self.scene
        }
    }

    impl FragmentRenderer {
        fn new() -> Self {
            Self {
                session: PipelineSession::new(Arc::new(DenyResourceProvider)),
            }
        }

        fn render_view(&self) -> Option<RenderView<'_>> {
            self.session.render_view()
        }

        fn render(
            &mut self,
            source: &str,
            options: &SceneOptions,
            shaper: &mut impl GlyphShaper,
        ) -> Result<RenderedFragment<'_>, PipelineError> {
            let style_environment = StyleEnvironment {
                root_font_size: Default::default(),
                media: MediaEnvironment::screen(options.viewport_width(), options.maximum_height())
                    .expect("scene options are valid media dimensions"),
            };
            let layout = LayoutConstraints::new(options.viewport_width(), 16.0)?
                .with_viewport_height(options.maximum_height())?;
            let inputs = PipelineInputs::new(
                source.into(),
                MarkupSyntax::Html,
                "fragment.xhtml".into(),
                style_environment,
                layout,
            );
            self.session.attach(shaper).update(inputs)?;
            let document = self.session.document().expect("updated fragment is laid out");
            let resources = self.session.glyph_resources();
            let scene = render_scene_for_view(document.render_view(resources), options);
            Ok(RenderedFragment {
                document,
                resources,
                scene,
            })
        }
    }

    #[derive(Default)]
    struct TestShaper {
        glyphs: HashMap<(char, u32), GlyphHandle>,
        generation: GlyphResourceGeneration,
        append_checkpoint: Option<(HashMap<(char, u32), GlyphHandle>, GlyphResourceGeneration)>,
    }

    impl GlyphShaper for TestShaper {
        fn reset(&mut self) {
            self.glyphs.clear();
            self.generation = GlyphResourceGeneration::default();
        }

        fn resource_generation(&self) -> GlyphResourceGeneration {
            self.generation
        }

        fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
            assert!(self.append_checkpoint.is_none());
            self.append_checkpoint = Some((self.glyphs.clone(), self.generation.clone()));
            Ok(())
        }

        fn commit_append_shaping(&mut self) {
            self.append_checkpoint = None;
        }

        fn rollback_append_shaping(&mut self) {
            if let Some((glyphs, glyph_store)) = self.append_checkpoint.take() {
                self.glyphs = glyphs;
                self.generation = glyph_store;
            }
        }

        fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
            let key = (request.character, request.style.font_size.to_bits());
            let advance = request.style.font_size * 0.5;
            let metrics = FontMetrics {
                ascent: request.style.font_size * 0.75,
                descent: request.style.font_size * 0.25,
            };
            if let Some(glyph) = self.glyphs.get(&key) {
                return Ok(ShapedGlyph::new(*glyph, advance, metrics));
            }
            let glyph = GlyphHandle(self.glyphs.len() as u32);
            self.glyphs.insert(key, glyph);
            Ok(ShapedGlyph::new(glyph, advance, metrics))
        }
    }

    #[test]
    fn scalar_paint_uses_native_handles_and_baseline_coordinates() {
        use html_pipeline::engine::FontMetrics;
        #[derive(Default)]
        struct NativeHandles(GlyphResourceGeneration);
        impl GlyphShaper for NativeHandles {
            fn resource_generation(&self) -> GlyphResourceGeneration {
                self.0
            }
            fn reset(&mut self) {
                self.0 = GlyphResourceGeneration::default();
            }
            fn shape_glyph(
                &mut self,
                _request: GlyphRequest<'_>,
            ) -> Result<ShapedGlyph, ShapeError> {
                Ok(ShapedGlyph::new(
                    GlyphHandle(700),
                    8.0,
                    FontMetrics {
                        ascent: 12.0,
                        descent: 4.0,
                    },
                ))
            }
        }
        let mut renderer = FragmentRenderer::new();
        let mut shaper = NativeHandles::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer
            .render(
                "<p style='margin:0;line-height:30px'>x</p>",
                &options,
                &mut shaper,
            )
            .unwrap();
        let text = rendered.render_view().text();
        let line = text
            .lines()
            .iter()
            .find(|line| !text_paint::render_line_glyphs(&line).is_empty())
            .unwrap();
        assert_ne!(
            text_paint::glyph_at(text, line.start() as usize).map(|id| id.0),
            Some(700)
        );
        let mut painter = RecordingPainter::default();
        rendered.scene().paint(&mut painter);
        assert_eq!(painter.glyphs.len(), 1);
        let (handle, origin) = painter.glyphs[0];
        assert_eq!(handle, GlyphHandle(700));
        assert_eq!(
            origin.y,
            (line.point().y + text_paint::line_baseline(&line)).round()
        );
    }

    #[test]
    fn scalar_glyph_geometry_is_shared_across_authored_colors() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let rendered = renderer
            .render(
                "<p><span style='color:red'>x</span><span style='color:blue'>x</span></p>",
                &SceneOptions::new(200.0, None).unwrap(),
                &mut shaper,
            )
            .unwrap();
        let text = rendered.render_view().text();
        assert_eq!(text.glyph_count(), 2);
        assert_eq!(text_paint::glyph_at(text, 0), text_paint::glyph_at(text, 1));
        assert_eq!(text_paint::paint_color_at(text, 0), Some(0xff0000ff));
        assert_eq!(text_paint::paint_color_at(text, 1), Some(0x0000ffff));
        let mut painter = RecordingPainter::default();
        rendered.scene().paint(&mut painter);
        assert_eq!(painter.glyphs[0].0, painter.glyphs[1].0);
        assert_eq!(painter.glyph_colors, [0xff0000ff, 0x0000ffff]);
    }

    #[test]
    fn ellipsis_uses_the_container_foreground_color() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let rendered = renderer.render(
            "<p style='width:40px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis;color:green'><span style='color:red'>xxxxxxxxxx</span></p>",
            &SceneOptions::new(200.0, None).unwrap(), &mut shaper,
        ).unwrap();
        let handle = shaper
            .glyphs
            .iter()
            .find_map(|(&(character, _), &handle)| (character == '…').then_some(handle))
            .unwrap();
        assert!(rendered.scene().commands.as_ref().iter().any(|command|
            matches!(command, RenderCommand::Glyph { glyph, color, .. } if *glyph == handle && *color == 0x008000ff)));
    }

    #[test]
    fn standard_shaper_restores_native_resources_when_rendering_fails() {
        use html_pipeline::engine::GlyphWriter;
        use html_pipeline::engine::ResourceShaper;
        let mut shaper = ResourceShaper::new(|request, resources: &mut GlyphWriter<'_, char>| {
            if request.character == '!' {
                return Err(shaping_contracts::shape_error_registry_capacity_exhausted(0));
            }
            let metrics = FontMetrics {
                ascent: request.style.font_size * 0.75,
                descent: request.style.font_size * 0.25,
            };
            let handle = resources.insert(request.character)?;
            Ok(ShapedGlyph::new(
                handle,
                request.style.font_size * 0.5,
                metrics,
            ))
        });
        let mut renderer = FragmentRenderer::new();
        let options = SceneOptions::new(200.0, None).unwrap();
        // First-line refinement appends resources while replacement is pending.
        let scene = renderer
            .render(
                "<style>p::first-line {font-size:24px}</style><p>x</p>",
                &options,
                &mut shaper,
            )
            .unwrap()
            .into_scene();
        let generation = shaper.resource_generation();
        assert!(renderer.render("<p>y!</p>", &options, &mut shaper).is_err());
        assert_eq!(shaper.resource_generation(), generation);
        assert_eq!(renderer.render_view().unwrap().text().glyph_count(), 1);
        let mut painter = RecordingPainter::default();
        scene.paint(&mut painter);
        assert!(!painter.glyphs.is_empty());
        for (handle, _) in painter.glyphs {
            assert_eq!(shaper.resources().get(handle), Some(&'x'));
        }
        renderer.render("<p>z</p>", &options, &mut shaper).unwrap();
        assert_ne!(shaper.resource_generation(), generation);
        assert_eq!(shaper.resources().get(GlyphHandle(0)), Some(&'z'));
    }

    #[derive(Default)]
    struct NativeTestShaper {
        fallback: TestShaper,
        next_run: TextRunId,
        append_checkpoint_run: Option<TextRunId>,
    }

    impl GlyphShaper for NativeTestShaper {
        fn reset(&mut self) {
            self.fallback.reset();
            self.next_run = TextRunId(0);
        }

        fn resource_generation(&self) -> GlyphResourceGeneration {
            self.fallback.resource_generation()
        }

        fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
            self.fallback.begin_append_shaping()?;
            self.append_checkpoint_run = Some(self.next_run);
            Ok(())
        }

        fn commit_append_shaping(&mut self) {
            self.fallback.commit_append_shaping();
            self.append_checkpoint_run = None;
        }

        fn rollback_append_shaping(&mut self) {
            self.fallback.rollback_append_shaping();
            if let Some(next_run) = self.append_checkpoint_run.take() {
                self.next_run = next_run;
            }
        }

        fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
            self.fallback.shape_glyph(request)
        }

        fn shape_run(
            &mut self,
            request: TextRunShapeRequest<'_>,
        ) -> Result<Option<ShapedRun>, ShapeError> {
            let count = shaping_contracts::text_run_shape_request_text(&request).chars().count();
            let advances: Vec<f32> = shaping_contracts::text_run_shape_request_text(&request).chars().map(|character| {
                if matches!(character, '\u{00ad}' | '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { 0.0 } else { 8.0 }
            }).collect();
            let mut caret_stops = Vec::with_capacity(count + 1);
            let mut x = 0.0;
            caret_stops.push(x);
            for advance in &advances {
                x += advance;
                caret_stops.push(x);
            }
            if shaping_contracts::text_run_shape_request_style(&request).direction == TextDirection::Rtl {
                for stop in &mut caret_stops {
                    *stop = x - *stop;
                }
            }
            let run = self.next_run;
            self.next_run.0 += 1;
            Ok(Some(shaping_contracts::shaped_run_geometry(shaping_contracts::shaped_text_run_with_backend_run(Arc::from(advances),
                Arc::from(vec![true; count + 1]),
                run,
                caret_stops.into(),
                12.0,))))
        }
    }

    #[test]
    fn native_run_paint_uses_the_line_baseline() {
        #[derive(Default)]
        struct BaselinePainter(Vec<Point>);
        impl Painter for BaselinePainter {
            fn push_clip(&mut self, _: Rect) {}
            fn pop_clip(&mut self) {}
            fn fill_rect(&mut self, _: Rect, _: Color) {}
            fn draw_glyph(&mut self, _: GlyphHandle, _: Point, _color: u32) {
                panic!("expected native run painting");
            }
            fn supports_text_runs(&self) -> bool {
                true
            }
            fn draw_text_run_fragment(
                &mut self,
                _: TextRunId,
                _: Range<u32>,
                origin: Point,
                _: Option<u32>,
            ) {
                self.0.push(origin);
            }
            fn draw_image(&mut self, _: &Image, _: &[u8], _: Rect) {}
        }
        let mut renderer = FragmentRenderer::new();
        let mut shaper = NativeTestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer
            .render(
                "<p style='margin:0;line-height:30px'><span style='font-size:24px'>x</span>y</p>",
                &options,
                &mut shaper,
            )
            .unwrap();
        let view = rendered.render_view();
        let text = view.text();
        let line = text
            .lines()
            .iter()
            .find(|line| !text_paint::render_line_glyphs(&line).is_empty())
            .unwrap();
        let origin = Point::new(10.0, 20.25);
        let mut painter = BaselinePainter::default();
        assert!(paint_line_text_runs(
            view,
            line.id(),
            origin,
            2.0,
            None,
            &mut painter
        ));
        assert!(!painter.0.is_empty());
        let expected_y = ((origin.y + text_paint::line_baseline(&line)) * 2.0).round() / 2.0;
        assert!(painter.0.iter().all(|point| point.y == expected_y));
    }

    #[derive(Default)]
    struct PreparedBatchPainter {
        prepared_calls: usize,
        fragments: usize,
        scalar_glyphs: usize,
    }

    impl Painter for PreparedBatchPainter {
        fn push_clip(&mut self, _: Rect) {}
        fn pop_clip(&mut self) {}
        fn fill_rect(&mut self, _rect: Rect, _color: Color) {}

        fn draw_glyph(&mut self, _glyph: GlyphHandle, _origin: Point, _color: u32) {
            self.scalar_glyphs += 1;
        }

        fn supports_text_runs(&self) -> bool {
            true
        }

        fn draw_prepared_text_run_batch(
            &mut self,
            fragments: &[PreparedTextRunFragment],
            _line_origin: Point,
            _scale: f64,
            _color: Option<u32>,
        ) {
            self.prepared_calls += 1;
            self.fragments += fragments.len();
        }

        fn draw_image(&mut self, _image: &Image, _hash: &[u8], _rect: Rect) {}
    }

    #[derive(Default)]
    struct ColorPainter {
        native: bool,
        glyph_colors: Vec<Option<u32>>,
        run_colors: Vec<(Range<u32>, Option<u32>)>,
    }

    impl Painter for ColorPainter {
        fn push_clip(&mut self, _: Rect) {}
        fn pop_clip(&mut self) {}
        fn fill_rect(&mut self, _: Rect, _: Color) {}

        fn draw_glyph(&mut self, _: GlyphHandle, _: Point, color: u32) {
            self.glyph_colors.push(Some(color));
        }
        fn supports_text_runs(&self) -> bool {
            self.native
        }
        fn draw_text_run_fragment(
            &mut self,
            _: TextRunId,
            range: Range<u32>,
            _: Point,
            color: Option<u32>,
        ) {
            self.run_colors.push((range, color));
        }
        fn draw_image(&mut self, _: &Image, _: &[u8], _: Rect) {}
    }

    #[test]
    fn paint_color_updates_replay_native_and_scalar_text_without_shaping() {
        let mut factory = html_pipeline::DocumentFactory::new();
        let base =
            factory.parse_with_new_pipeline("<p><span>A</span><a href='#target'>A</a></p>", None);
        let mut shaper = NativeTestShaper::default();
        let constraints = html_pipeline::engine::LayoutConstraints::new(200.0, 16.0).unwrap();
        let mut document_resources = GlyphResourceStore::default();
        let mut document = base
            .shape_borrowed(&mut shaper, &mut document_resources)
            .unwrap()
            .layout(constraints, &document_resources)
            .expect("layout requires the shaper-aware API for first-line styles");
        let retained_color =
            text_paint::paint_color_at(document.render_view(&document_resources).text(), 1);
        let run_count = shaper.next_run;
        let preferences = html_pipeline::engine::ReaderStyleOverrides {
            foreground: Some(0x008000ff),
            link_foreground: Some(0xff0000ff),
            ..Default::default()
        };
        let painted = base.with_reader_paint_overrides(&preferences).unwrap();
        assert!(document.repaint_from(&painted, &document_resources));
        assert_eq!(shaper.next_run, run_count);
        let text = document.render_view(&document_resources).text();
        let line = text
            .lines()
            .iter()
            .find(|line| !text_paint::render_line_glyphs(&line).is_empty())
            .unwrap()
            .id();
        assert_eq!(text_paint::paint_color_at(text, 0), Some(0x008000ff));
        assert_eq!(text_paint::paint_color_at(text, 1), Some(0xff0000ff));
        assert_eq!(retained_color, Some(0x0000ffff));
        let mut native = ColorPainter {
            native: true,
            ..Default::default()
        };
        assert!(paint_line_text_runs(
            (&document).render_view(&document_resources),
            line,
            Point::ZERO,
            1.0,
            None,
            &mut native
        ));
        assert!(
            native
                .run_colors
                .iter()
                .any(|(_, color)| *color == Some(0x008000ff))
        );
        assert!(
            native
                .run_colors
                .iter()
                .any(|(_, color)| *color == Some(0xff0000ff))
        );
        assert!(native.glyph_colors.is_empty());
        let mut scalar = ColorPainter::default();
        paint_line_glyphs(
            (&document).render_view(&document_resources),
            line,
            Point::ZERO,
            1.0,
            None,
            &mut scalar,
        );
        assert_eq!(
            scalar.glyph_colors,
            vec![Some(0x008000ff), Some(0xff0000ff)]
        );
        let mut overridden = ColorPainter {
            native: true,
            ..Default::default()
        };
        paint_line_text_runs(
            (&document).render_view(&document_resources),
            line,
            Point::ZERO,
            1.0,
            Some(0x0000ffff),
            &mut overridden,
        );
        assert!(
            overridden
                .run_colors
                .iter()
                .all(|(_, color)| *color == Some(0x0000ffff))
        );
        // Restore publisher colors, including prepared native fragments.
        assert!(document.repaint_from(&base, &document_resources));
        let mut restored = ColorPainter {
            native: true,
            ..Default::default()
        };
        paint_line_text_runs(
            (&document).render_view(&document_resources),
            line,
            Point::ZERO,
            1.0,
            None,
            &mut restored,
        );
        assert!(restored.run_colors.iter().all(|(_, color)| color.is_none()));
    }

    #[test]
    fn initial_palette_and_relayout_publish_colored_native_fragments() {
        let mut factory = html_pipeline::DocumentFactory::new();
        let base =
            factory.parse_with_new_pipeline("<p><span>A</span><a href='#target'>A</a></p>", None);
        let preferences = html_pipeline::engine::ReaderStyleOverrides {
            foreground: Some(0x008000ff),
            link_foreground: Some(0xff0000ff),
            ..Default::default()
        };
        let painted = base.with_reader_paint_overrides(&preferences).unwrap();
        let mut shaper = NativeTestShaper::default();
        let constraints = html_pipeline::engine::LayoutConstraints::new(200.0, 16.0).unwrap();
        let mut document_resources = GlyphResourceStore::default();
        let mut document = painted
            .shape_borrowed(&mut shaper, &mut document_resources)
            .unwrap()
            .layout(constraints, &document_resources)
            .expect("layout requires the shaper-aware API for first-line styles");
        let run_count = shaper.next_run;
        for width in [200.0, 100.0, 240.0] {
            if width != 200.0 {
                document
                    .relayout(
                        html_pipeline::engine::LayoutConstraints::new(width, 16.0).unwrap(),
                        &document_resources,
                    )
                    .expect("layout requires the shaper-aware API for first-line styles");
            }
            let text = document.render_view(&document_resources).text();
            let colors: Vec<_> = text
                .lines()
                .iter()
                .flat_map(|line| {
                    text_paint::prepared_text_runs(text, line.id())
                        .expect("native coverage is published")
                })
                .map(|fragment| fragment.color)
                .collect();
            assert_eq!(colors, [Some(0x008000ff), Some(0xff0000ff)]);
            assert_eq!(
                shaper.next_run, run_count,
                "relayout reuses shaping resources"
            );
        }
    }

    #[test]
    fn paint_color_updates_preserve_overflow_clips_without_shaping() {
        let mut factory = html_pipeline::DocumentFactory::new();
        let base = factory.parse_with_new_pipeline("<div style='width:80px;height:20px;overflow:hidden;background-color:white'><p style='margin:0'>Clipped text retains its owner and geometry.</p></div>", None);
        let mut shaper = NativeTestShaper::default();
        let constraints = html_pipeline::engine::LayoutConstraints::new(120.0, 16.0).unwrap();
        let mut document_resources = GlyphResourceStore::default();
        let mut document = base
            .shape_borrowed(&mut shaper, &mut document_resources)
            .unwrap()
            .layout(constraints, &document_resources)
            .expect("layout requires the shaper-aware API for first-line styles");
        let clips: Vec<_> = text_paint::render_text_view_spatial_order(document
            .render_view(&document_resources)
            .text())
            .iter()
            .copied()
            .map(|index| {
                text_paint::line_overflow_clip(
                    document.render_view(&document_resources).text(),
                    index,
                )
            })
            .collect();
        assert!(clips.iter().any(Option::is_some));
        let run_count = shaper.next_run;
        let painted = base
            .with_reader_paint_overrides(&html_pipeline::engine::ReaderStyleOverrides {
                foreground: Some(0xeeeeeeff),
                ..Default::default()
            })
            .unwrap();
        assert!(document.repaint_from(&painted, &document_resources));
        let repainted_clips: Vec<_> = text_paint::render_text_view_spatial_order(document
            .render_view(&document_resources)
            .text())
            .iter()
            .copied()
            .map(|index| {
                text_paint::line_overflow_clip(
                    document.render_view(&document_resources).text(),
                    index,
                )
            })
            .collect();
        assert_eq!(clips, repainted_clips);
        assert_eq!(shaper.next_run, run_count);
        assert!(document.repaint_from(&base, &document_resources));
        let restored_clips: Vec<_> = text_paint::render_text_view_spatial_order(document
            .render_view(&document_resources)
            .text())
            .iter()
            .copied()
            .map(|index| {
                text_paint::line_overflow_clip(
                    document.render_view(&document_resources).text(),
                    index,
                )
            })
            .collect();
        assert_eq!(clips, restored_clips);
    }

    #[test]
    fn paint_updates_keep_first_letter_and_first_line_colors_after_resize() {
        let mut factory = html_pipeline::DocumentFactory::new();
        let base = factory.parse_with_new_pipeline(
            "<p style='color:green'>“Apple” text continues here and wraps.</p>",
            Some("p::first-letter {color:purple} p::first-line {color:blue}"),
        );
        let mut shaper = NativeTestShaper::default();
        let constraints = html_pipeline::engine::LayoutConstraints::new(120.0, 16.0).unwrap();
        let mut document_resources = GlyphResourceStore::default();
        let mut document = base
            .shape_borrowed(&mut shaper, &mut document_resources)
            .unwrap()
            .layout_with_metrics_and_shaper(
                constraints,
                &Default::default(),
                &mut shaper,
                &mut document_resources,
            )
            .unwrap();
        let theme = html_pipeline::engine::ThemeAdaptation {
            ink: 0xeeeeeeff,
            surface: 0x222222ff,
        };
        let painted = base
            .with_reader_paint_overrides(&html_pipeline::engine::ReaderStyleOverrides {
                theme: Some(theme),
                ..Default::default()
            })
            .unwrap();
        assert!(document.repaint_from(&painted, &document_resources));
        let text = document.render_view(&document_resources).text();
        assert_eq!(
            text_paint::paint_color_at(text, 0),
            Some(computed_style::theme_adaptation_adapt(theme, 0x800080ff))
        );
        assert_eq!(
            text_paint::paint_color_at(text, 1),
            Some(computed_style::theme_adaptation_adapt(theme, 0x800080ff))
        );
        assert_eq!(
            text_paint::line_paint_color(&text.line(LineId::new(0)).unwrap()),
            Some(computed_style::theme_adaptation_adapt(theme, 0x0000ffff))
        );
        document
            .relayout_with_metrics_and_shaper(
                html_pipeline::engine::LayoutConstraints::new(200.0, 16.0).unwrap(),
                &Default::default(),
                &mut shaper,
                &mut document_resources,
            )
            .unwrap();
        assert_eq!(
            text_paint::paint_color_at(document.render_view(&document_resources).text(), 0),
            Some(computed_style::theme_adaptation_adapt(theme, 0x800080ff))
        );
    }

    #[test]
    fn native_scalar_and_caret_positions_agree_around_unpainted_characters() {
        use html_pipeline::engine::CaretAffinity;

        for content in ["AB\u{00ad}CD EFGH", "AB\u{200e}CD EFGH"] {
            let mut factory = html_pipeline::DocumentFactory::new();
            let prepared = factory.parse_with_new_pipeline(
                &format!("<p>{content}</p>"),
                Some("body,p {margin:0} p {text-align:justify;hyphens:manual}"),
            );
            let mut shaper = NativeTestShaper::default();
            let mut document_resources = GlyphResourceStore::default();
            let document = prepared
                .shape_borrowed(&mut shaper, &mut document_resources)
                .unwrap()
                .layout(
                    html_pipeline::engine::LayoutConstraints::new(32.12, 16.0).unwrap(),
                    &document_resources,
                )
                .unwrap();
            let view = document.render_view(&document_resources);
            let text = view.text();
            let line = text.line(LineId::new(0)).unwrap();
            assert!(
                text_paint::line_letter_spacing(&line) > 0.0,
                "fixture must exercise justified tracking"
            );
            let mut scalar = RecordingPainter::default();
            paint_line_glyphs(view, LineId::new(0), Point::ZERO, 1.0, None, &mut scalar);
            let mut separate = RecordingPainter::default();
            for fragment in
                text_paint::render_text_view_line_text_fragments(text, LineId::new(0)).unwrap()
            {
                paint_line_glyph_fragment(
                    view,
                    LineId::new(0),
                    &fragment,
                    Point::ZERO,
                    1.0,
                    None,
                    &mut separate,
                );
            }
            assert_eq!(scalar.glyphs, separate.glyphs);
            for &(glyph, point) in &scalar.glyphs {
                let index = text_paint::render_line_glyphs(&line)
                    .find(|&index| {
                        text_paint::paint_glyph_at(text, index)
                            .and_then(|id| text_paint::paint_handle(text, id))
                            == Some(glyph)
                    })
                    .unwrap();
                let caret = geometry_queries::render_view_geometry(view)
                    .caret_x(LineId::new(0), index, CaretAffinity::Leading)
                    .unwrap();
                assert!(
                    (point.x - caret).abs() < 1.0e-6,
                    "scalar paint and caret disagree at {index}"
                );
            }
            let runs: Vec<_> =
                text_paint::authoritative_runs(text, text_paint::render_line_glyphs(&line))
                    .collect();
            for fragment in text_paint::prepared_text_runs(text, LineId::new(0))
                .expect("complete native coverage")
            {
                let run = runs
                    .iter()
                    .find(|run| {
                        run.run() == fragment.run && run.run_range().contains(&fragment.range.start)
                    })
                    .unwrap();
                let index = run.source_range().start + fragment.range.start - run.run_range().start;
                let natural = f64::from(run.natural_offset())
                    + (run.source_range().start..index)
                        .map(|index| f64::from(text_paint::character_advance(text, index).unwrap()))
                        .sum::<f64>();
                let caret = geometry_queries::render_view_geometry(view)
                    .caret_x(LineId::new(0), index, CaretAffinity::Leading)
                    .unwrap();
                assert!(
                    (fragment.offset.x + natural - caret).abs() < 1.0e-6,
                    "native paint and caret disagree at {index}"
                );
            }
        }
    }

    #[test]
    fn incomplete_native_coverage_uses_scalar_painting() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer
            .render("<p>fallback text</p>", &options, &mut shaper)
            .unwrap();
        let mut native = ColorPainter {
            native: true,
            ..Default::default()
        };
        assert!(!paint_line_text_runs(
            rendered.render_view(),
            LineId::new(0),
            Point::ZERO,
            1.0,
            None,
            &mut native
        ));
        assert!(native.run_colors.is_empty());
        let mut scalar = RecordingPainter::default();
        paint_line_glyphs(
            rendered.render_view(),
            LineId::new(0),
            Point::ZERO,
            1.0,
            None,
            &mut scalar,
        );
        assert!(!scalar.glyphs.is_empty());
    }

    #[test]
    fn native_line_paint_replays_layout_prepared_run_fragments() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = NativeTestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer
            .render("<p>prepared native text</p>", &options, &mut shaper)
            .unwrap();

        let text = rendered.render_view().text();
        let line_idx = text
            .lines()
            .iter()
            .find(|line| !text_paint::render_line_glyphs(&line).is_empty())
            .map(|line| line.id())
            .expect("fixture emits text");
        assert!(
            text_paint::prepared_text_runs(text, line_idx).is_some(),
            "layout retains authoritative run slices"
        );

        let mut painter = PreparedBatchPainter::default();
        assert!(paint_line_text_runs(
            rendered.render_view(),
            line_idx,
            Point::new(10.0, 20.0),
            1.0,
            None,
            &mut painter
        ));
        assert_eq!(painter.prepared_calls, 1);
        assert!(painter.fragments > 0);
        assert_eq!(
            painter.scalar_glyphs, 0,
            "native coverage never falls back to scalar glyph painting"
        );
    }

    #[test]
    fn rtl_scalar_paint_places_mirrored_brackets_and_hides_directional_controls() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer
            .render("<p dir='rtl'>(אבג)\u{200f}</p>", &options, &mut shaper)
            .unwrap();

        let text = rendered.render_view().text();
        let mut painter = RecordingPainter::default();
        paint_line_glyphs(
            rendered.render_view(),
            LineId::new(0),
            Point::ZERO,
            1.0,
            None,
            &mut painter,
        );
        painter.glyphs.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
        let actual: String = painter
            .glyphs
            .iter()
            .map(|(glyph, _)| {
                shaper
                    .glyphs
                    .iter()
                    .find_map(|(&(character, _), &handle)| (handle == *glyph).then_some(character))
                    .unwrap()
            })
            .collect();
        assert_eq!(actual, "(גבא)");
    }

    #[test]
    fn rtl_native_paint_uses_document_owned_directional_slices() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = NativeTestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer
            .render("<p dir='rtl'>אבג abc</p>", &options, &mut shaper)
            .unwrap();
        let mut painter = PreparedBatchPainter::default();
        assert!(paint_line_text_runs(
            rendered.render_view(),
            LineId::new(0),
            Point::ZERO,
            1.0,
            None,
            &mut painter
        ));
        assert_eq!(painter.prepared_calls, 1);
        assert!(painter.fragments >= 4);
        assert_eq!(painter.scalar_glyphs, 0);
    }

    #[derive(Default)]
    struct LegacyPainter {
        fills: usize,
    }

    impl Painter for LegacyPainter {
        fn push_clip(&mut self, _: Rect) {}
        fn pop_clip(&mut self) {}
        fn fill_rect(&mut self, _rect: Rect, _color: Color) {
            self.fills += 1;
        }

        fn draw_glyph(&mut self, _glyph: GlyphHandle, _origin: Point, _color: u32) {}

        fn draw_image(&mut self, _image: &Image, _hash: &[u8], _rect: Rect) {}
    }

    #[test]
    fn rounded_primitives_are_optional_for_painter_implementers() {
        let mut painter = LegacyPainter::default();
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let radii = CornerRadii {
            top_left: (4.0, 4.0),
            top_right: (4.0, 4.0),
            bottom_right: (4.0, 4.0),
            bottom_left: (4.0, 4.0),
        };
        painter.fill_rounded_rect(rect, radii, Color::BLACK);
        painter.stroke_rounded_rect(rect, radii, 2.0, Color::BLACK);
        assert_eq!(
            painter.fills, 5,
            "default methods preserve square fill/border behavior"
        );
    }

    #[test]
    fn rounded_background_and_uniform_border_reach_the_render_scene() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<div style='width:100px;height:30px;background:red;border:2px solid blue;border-radius:12px'>rounded</div>", &options, &mut shaper).unwrap();

        assert!(
            rendered
                .scene()
                .commands
                .as_ref()
                .iter()
                .any(|command| matches!(command, RenderCommand::RoundedFill { .. }))
        );
        assert!(rendered.scene().commands.as_ref().iter().any(
            |command| matches!(command, RenderCommand::RoundedBorder { width, .. } if *width == 2.0)
        ));
        let mut recording = RecordingPainter::default();
        rendered.scene().paint(&mut recording);
        assert!(!recording.rounded_fills.is_empty());
        assert!(!recording.rounded_borders.is_empty());
    }

    #[test]
    fn foreground_text_decoration_is_recorded_after_glyphs() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<span style='text-decoration-line:line-through;text-decoration-color:#ff0000'>marked</span>", &options, &mut shaper).unwrap();
        let commands = rendered.scene().commands.as_ref();
        let last_glyph = commands
            .iter()
            .rposition(|command| matches!(command, RenderCommand::Glyph { .. }))
            .expect("fixture emits glyphs");
        let foreground = commands.iter().position(|command| matches!(command, RenderCommand::Fill { color, .. } if color.r == 255 && color.g == 0 && color.b == 0)).expect("line-through emits a foreground decoration");
        assert!(
            foreground > last_glyph,
            "foreground decorations must paint after text"
        );
    }

    #[test]
    fn render_core_tessellates_semantic_dashed_decorations() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(240.0, None).unwrap();
        let rendered = renderer.render("<span style='text-decoration:line-through 2px dashed #ff0000'>patterned decoration</span>", &options, &mut shaper).unwrap();

        let semantic = fragment_paint::decorations(rendered.render_view().fragments())
            .iter()
            .filter(|decoration| fragment_paint::decoration_color(&decoration) == 0xff0000ff)
            .collect::<Vec<_>>();
        assert_eq!(
            semantic.len(),
            1,
            "layout publishes one semantic dashed span"
        );
        assert_eq!(
            fragment_paint::decoration_pattern(&semantic[0]),
            html_pipeline::engine::RenderDecorationPattern::DashedHorizontal
        );

        let painter_rects = rendered.scene().commands.as_ref().iter().filter(|command| matches!(command, RenderCommand::Fill { color, .. } if color.r == 255 && color.g == 0 && color.b == 0)).count();
        assert!(
            painter_rects > 1,
            "render core expands the semantic span into painter primitives"
        );
    }

    #[test]
    fn image_overflow_uses_line_identity_when_spatial_order_is_different() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<html><body style='margin:0;font-size:10px;line-height:10px'><div style='position:relative;top:80px'>A<img src='x' width='10' height='10' style='position:relative;top:60px;vertical-align:top'></div><div>B</div></body></html>", &options, &mut shaper).unwrap();
        let view = rendered.render_view();
        let text = view.text();
        assert_ne!(
            layout_state::line_id_index(text_paint::render_text_view_spatial_order(text)[0]),
            0
        );
        let nominal_bottom = text
            .lines()
            .iter()
            .map(|line| line.point().y + line.height())
            .fold(0.0, f64::max);
        let image = fragment_paint::render_fragment_view_images(view.fragments())
            .iter()
            .next()
            .unwrap();
        let image_bottom = geometry_queries::render_geometry_view_line_vertical_bounds(
            geometry_queries::render_view_geometry(view),
            fragment_paint::render_image_fragment_line(&image),
        )
        .unwrap()
        .end;
        assert!(
            image_bottom > nominal_bottom,
            "fixture must overflow its carrier line"
        );
        assert!(
            rendered.scene().size().height >= image_bottom,
            "image remains inside the scene despite non-spatial storage"
        );
    }

    #[test]
    fn renders_bounded_html_fragment_into_a_replayable_scene() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(160.0, Some(24.0)).unwrap();
        let rendered = renderer.render("<div style='background:#eee'><p>Hello <strong>fragment</strong></p><p>second line</p></div>", &options, &mut shaper).unwrap();
        assert!(rendered.scene().is_clipped());
        assert!(
            rendered
                .scene()
                .commands
                .as_ref()
                .iter()
                .any(|command| matches!(command, RenderCommand::Glyph { .. }))
        );
        let mut replay = RecordingPainter::default();
        rendered.scene().paint(&mut replay);
        assert!(!replay.glyphs.is_empty());
    }

    #[test]
    fn overflow_hidden_clips_descendant_backgrounds_and_records_text_masks() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<div style='height:40px;overflow:hidden'>inside<div style='height:20px;margin-top:40px;background:#ff0000'>outside</div></div>", &options, &mut shaper).unwrap();

        assert!(!rendered.scene().commands.as_ref().iter().any(|command| matches!(command, RenderCommand::Fill { color, .. } if color.r == 255 && color.g == 0 && color.b == 0)));
        assert!(
            rendered
                .scene()
                .commands
                .as_ref()
                .iter()
                .any(|command| matches!(command, RenderCommand::Glyph { .. })),
            "visible text exercises the overflow clip"
        );
        assert!(
            rendered
                .scene()
                .commands
                .as_ref()
                .iter()
                .any(|command| matches!(command, RenderCommand::PushClip { .. }))
        );
        assert_eq!(
            rendered
                .scene()
                .commands
                .as_ref()
                .iter()
                .filter(|command| matches!(command, RenderCommand::PushClip { .. }))
                .count(),
            rendered
                .scene()
                .commands
                .as_ref()
                .iter()
                .filter(|command| matches!(command, RenderCommand::PopClip))
                .count(),
        );
    }

    #[test]
    fn text_and_background_clips_preserve_the_visible_overflow_axis() {
        for (overflow, expected_background) in [
            (
                "overflow-x:clip;overflow-y:visible",
                Rect::new(0.0, 0.0, 40.0, 80.0),
            ),
            (
                "overflow-x:visible;overflow-y:clip",
                Rect::new(0.0, 0.0, 80.0, 40.0),
            ),
        ] {
            let mut renderer = FragmentRenderer::new();
            let mut shaper = TestShaper::default();
            let options = SceneOptions::new(200.0, None).unwrap();
            let source = format!(
                "<body style='margin:0'><div style='width:40px;height:40px;{overflow}'>\
                 <div style='width:80px;height:80px;line-height:80px;background:#ff0000'>X</div>\
                 </div></body>"
            );
            let rendered = renderer.render(&source, &options, &mut shaper).unwrap();
            let scene = rendered.scene();
            let background = scene
                .commands
                .as_ref()
                .iter()
                .find_map(|command| match command {
                    RenderCommand::Fill { rect, color }
                        if color.r == 255 && color.g == 0 && color.b == 0 =>
                    {
                        Some(*rect)
                    }
                    _ => None,
                })
                .expect("visible child background");
            assert_eq!(background, expected_background, "{overflow}");

            let expected_text_clip = if overflow.starts_with("overflow-x:clip") {
                Rect::new(0.0, 0.0, 40.0, render_scene_clip_rect(&scene).y1)
            } else {
                Rect::new(0.0, 0.0, render_scene_clip_rect(&scene).x1, 40.0)
            };
            assert!(scene.commands.as_ref().iter().any(|command| {
                matches!(command, RenderCommand::PushClip { rect } if *rect == expected_text_clip)
            }));
        }
    }

    #[test]
    fn renders_atom_xhtml_without_flattening_its_structure() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(240.0, None).unwrap();
        let rendered = renderer.render(r#"<div xmlns="http://www.w3.org/1999/xhtml"><p>This edition has <strong>images</strong>.</p><p>Title: Carmen</p></div>"#, &options, &mut shaper).unwrap();

        let view = rendered.render_view().text();
        let text = rendered
            .scene()
            .commands
            .as_ref()
            .iter()
            .filter_map(|command| match command {
                RenderCommand::Glyph { glyph, .. } => shaper
                    .glyphs
                    .iter()
                    .find_map(|(&(character, _), &handle)| (handle == *glyph).then_some(character)),
                _ => None,
            })
            .collect::<String>();
        assert!(text.contains("This edition has images."));
        assert!(text.contains("Title: Carmen"));
    }

    #[test]
    fn fallback_painting_preserves_inline_replaced_content_offsets() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<p style='margin:0;white-space:nowrap'>a<img src='missing.png' style='width:20px;height:10px'>b</p>", &options, &mut shaper).unwrap();

        let view = rendered.render_view().text();
        let positions = rendered
            .scene()
            .commands
            .as_ref()
            .iter()
            .filter_map(|command| match command {
                RenderCommand::Glyph { glyph, origin, .. } => Some((
                    shaper
                        .glyphs
                        .iter()
                        .find_map(|(&(character, _), &handle)| {
                            (handle == *glyph).then_some(character)
                        })?,
                    origin.x,
                )),
                _ => None,
            })
            .collect::<HashMap<_, _>>();

        let a = positions[&'a'];
        let b = positions[&'b'];
        assert!(
            b - a >= 28.0,
            "the second text fragment must begin after the first glyph and the 20px image"
        );
    }

    #[test]
    fn fallback_painting_preserves_fractional_inline_origins() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(200.0, None).unwrap();
        let rendered = renderer
            .render(
                "<span style='margin-left:0.5px'>X</span>",
                &options,
                &mut shaper,
            )
            .unwrap();
        let origin = rendered
            .scene()
            .commands
            .as_ref()
            .iter()
            .find_map(|command| match command {
                RenderCommand::Glyph { origin, .. } => Some(*origin),
                _ => None,
            })
            .expect("fixture emits a glyph");

        assert!(
            (origin.x.fract() - 0.5).abs() < 0.01,
            "fractional inline origin was lost: {}",
            origin.x
        );
    }

    #[test]
    fn visibility_hides_paint_but_allows_visible_descendants_and_preserves_advance() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(240.0, None).unwrap();
        let rendered = renderer.render("<p style='visibility:hidden;margin:0'>hidden <span style='visibility:visible;background:#123456'>shown</span> hidden</p><p style='margin:0'>after</p>", &options, &mut shaper).unwrap();

        let text = rendered.render_view().text();
        let painted = rendered
            .scene()
            .commands
            .as_ref()
            .iter()
            .filter_map(|command| match command {
                RenderCommand::Glyph { glyph, .. } => shaper
                    .glyphs
                    .iter()
                    .find_map(|(&(character, _), &handle)| (handle == *glyph).then_some(character)),
                _ => None,
            })
            .collect::<String>();

        assert_eq!(painted, "shownafter");
        assert!(rendered.scene().commands.as_ref().iter().any(|command| matches!(command, RenderCommand::Fill { color, .. } if color.r == 0x12 && color.g == 0x34 && color.b == 0x56)));
        let shown_x = rendered
            .scene()
            .commands
            .as_ref()
            .iter()
            .find_map(|command| match command {
                RenderCommand::Glyph { glyph, origin, .. }
                    if shaper.glyphs.iter().find_map(|(&(character, _), &handle)| {
                        (handle == *glyph).then_some(character)
                    }) == Some('s') =>
                {
                    Some(origin.x)
                }
                _ => None,
            })
            .expect("visible descendant glyph");
        assert!(
            shown_x > 40.0,
            "hidden prefix must still consume inline width: {shown_x}"
        );
    }

    #[test]
    fn hidden_replaced_image_keeps_its_inline_advance_without_painting() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(240.0, None)
            .unwrap()
            .with_image_policy(SceneImagePolicy::Reference);
        let rendered = renderer.render("<p style='margin:0'>a<img src='cover.png' style='visibility:hidden;width:20px;height:10px'>b</p>", &options, &mut shaper).unwrap();
        assert!(
            !rendered
                .scene()
                .commands
                .as_ref()
                .iter()
                .any(|command| matches!(command, RenderCommand::ResourceImage { .. }))
        );

        let text = rendered.render_view().text();
        let positions = rendered
            .scene()
            .commands
            .as_ref()
            .iter()
            .filter_map(|command| match command {
                RenderCommand::Glyph { glyph, origin, .. } => Some((
                    shaper
                        .glyphs
                        .iter()
                        .find_map(|(&(character, _), &handle)| {
                            (handle == *glyph).then_some(character)
                        })?,
                    origin.x,
                )),
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        assert!(
            positions[&'b'] - positions[&'a'] >= 28.0,
            "the hidden image must retain its 20px inline advance"
        );
    }

    #[test]
    fn same_fragment_reflows_when_its_target_width_changes() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let source = "<p>A deliberately long description that must wrap over several lines in the narrow target.</p>";
        let wide = renderer
            .render(
                source,
                &SceneOptions::new(480.0, None).unwrap(),
                &mut shaper,
            )
            .unwrap()
            .scene()
            .content_height();
        let narrow = renderer
            .render(
                source,
                &SceneOptions::new(100.0, None).unwrap(),
                &mut shaper,
            )
            .unwrap()
            .scene()
            .content_height();
        assert!(
            narrow > wide,
            "expected narrow fragment height {narrow} to exceed wide height {wide}"
        );
    }

    #[test]
    fn can_retain_image_references_for_a_target_adapter() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = SceneOptions::new(240.0, None)
            .unwrap()
            .with_image_policy(SceneImagePolicy::Reference);
        let rendered = renderer
            .render(
                r#"<p>Before</p><img src="cover.png" width="20" height="40"><p>After</p>"#,
                &options,
                &mut shaper,
            )
            .unwrap();
        assert!(rendered.scene().commands.as_ref().iter().any(|command| matches!(command, RenderCommand::ResourceImage { uri: Some(uri), .. } if uri == "cover.png")));
    }

    #[test]
    fn rejects_invalid_scene_constraints() {
        assert!(SceneOptions::new(0.0, None).is_err());
        assert!(SceneOptions::new(f64::INFINITY, None).is_err());
        assert!(SceneOptions::new(100.0, Some(f64::NAN)).is_err());
        assert!(SceneOptions::new(100.0, Some(0.0)).is_err());
    }
}

/// Build a scene from published geometry, retaining inherited styles,
/// document paint order and clipping, including independently laid-out scopes.
pub fn render_scene_for_view(view: RenderView<'_>, options: &SceneOptions) -> RenderScene {
    build_fragment_view(view, options, None)
}

pub fn render_scene_for_view_with_images(
    view: RenderView<'_>,
    options: &SceneOptions,
    images: DecodedImages<'_>,
) -> RenderScene {
    build_fragment_view(view, options, Some(images))
}

pub fn render_scene_clip_rect(value: &RenderScene) -> Rect {
    Rect::from_origin_size(Point::ORIGIN, value.size)
}

/// Whether the originating document still publishes these paint products.
pub fn render_scene_is_valid(value: &RenderScene) -> bool {
    value
        .resources
        .as_ref()
        .is_none_or(|lease| lease.is_valid())
}

pub fn render_scene_is_clipped(value: &RenderScene) -> bool {
    value.clipped
}
