use html_layout::{GlyphId, GlyphShaper, LaidOutDocument, TextRunId, UsedBorderRadii};
use html_pipeline::{FontEnvironmentRevision, ImageMetricsRevision, LayoutConstraints, PaintSettingsRevision, PipelineInputs, PipelineSession, ResourceRevision, SourceRevision, StyleEnvironment, StylesheetRevision};
use html_resources::ResourceProvider;
use kurbo::{Point, Rect, Size};
use peniko::{Color, Image};
use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::Arc;

pub trait Painter {
    /// Restricts subsequent paint operations until the matching `pop_clip`.
    /// Legacy adapters keep their previous behavior until they opt in.
    fn push_clip(&mut self, _rect: Rect) {}
    fn pop_clip(&mut self) {}
    fn fill_rect(&mut self, rect: Rect, color: Color);
    /// Optional rounded primitive. Existing adapters intentionally get a safe
    /// square fallback, so adding CSS radius support is not a breaking change.
    fn fill_rounded_rect(&mut self, rect: Rect, _radii: UsedBorderRadii, color: Color) {
        self.fill_rect(rect, color);
    }
    /// Optional uniform solid rounded border primitive.
    fn stroke_rounded_rect(&mut self, rect: Rect, _radii: UsedBorderRadii, width: f32, color: Color) {
        let width = (width as f64).max(0.0).min(rect.width().min(rect.height()));
        if width == 0.0 {
            return;
        }
        self.fill_rect(Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + width), color);
        self.fill_rect(Rect::new(rect.x0, rect.y1 - width, rect.x1, rect.y1), color);
        self.fill_rect(Rect::new(rect.x0, rect.y0 + width, rect.x0 + width, rect.y1 - width), color);
        self.fill_rect(Rect::new(rect.x1 - width, rect.y0 + width, rect.x1, rect.y1 - width), color);
    }
    fn draw_glyph(&mut self, glyph: GlyphId, origin: Point);
    fn draw_glyph_with_color(&mut self, glyph: GlyphId, origin: Point, _color: u32) {
        self.draw_glyph(glyph, origin);
    }
    fn supports_text_runs(&self) -> bool {
        false
    }
    fn draw_text_run(&mut self, _run: TextRunId, _origin: Point, _color: Option<u32>) {}
    fn draw_text_run_fragment(&mut self, run: TextRunId, _range: Range<u32>, origin: Point, color: Option<u32>) {
        self.draw_text_run(run, origin, color);
    }
    fn draw_image(&mut self, image: &Image, hash: &[u8], rect: Rect);
    fn draw_svg(&mut self, _bytes: &[u8], _hash: &[u8], _intrinsic_size: (u32, u32), _rect: Rect) {}
    fn draw_resource_image(&mut self, _image_idx: u32, _uri: Option<&str>, _rect: Rect) {}
}

#[derive(Default)]
pub struct RecordingPainter {
    pub clips: Vec<Rect>,
    pub clip_pops: usize,
    pub glyphs: Vec<(GlyphId, Point)>,
    pub fills: Vec<(Rect, Color)>,
    pub images: Vec<Rect>,
    pub svgs: Vec<Rect>,
    pub resource_images: Vec<(u32, Option<String>, Rect)>,
    pub rounded_fills: Vec<(Rect, UsedBorderRadii, Color)>,
    pub rounded_borders: Vec<(Rect, UsedBorderRadii, f32, Color)>,
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

    fn fill_rounded_rect(&mut self, rect: Rect, radii: UsedBorderRadii, color: Color) {
        self.rounded_fills.push((rect, radii, color));
    }

    fn stroke_rounded_rect(&mut self, rect: Rect, radii: UsedBorderRadii, width: f32, color: Color) {
        self.rounded_borders.push((rect, radii, width, color));
    }

    fn draw_glyph(&mut self, glyph: GlyphId, origin: Point) {
        self.glyphs.push((glyph, origin));
    }

    fn draw_image(&mut self, _image: &Image, _hash: &[u8], rect: Rect) {
        self.images.push(rect);
    }

    fn draw_svg(&mut self, _bytes: &[u8], _hash: &[u8], _intrinsic_size: (u32, u32), rect: Rect) {
        self.svgs.push(rect);
    }

    fn draw_resource_image(&mut self, image_idx: u32, uri: Option<&str>, rect: Rect) {
        self.resource_images.push((image_idx, uri.map(str::to_owned), rect));
    }
}

#[derive(Clone)]
pub enum RenderCommand {
    PushClip { rect: Rect },
    PopClip,
    Fill { rect: Rect, color: Color },
    RoundedFill { rect: Rect, radii: UsedBorderRadii, color: Color },
    RoundedBorder { rect: Rect, radii: UsedBorderRadii, width: f32, color: Color },
    Glyph { glyph: GlyphId, origin: Point, color: Option<u32> },
    Image { image: Image, hash: Vec<u8>, rect: Rect },
    Svg { bytes: Arc<[u8]>, hash: Vec<u8>, intrinsic_size: (u32, u32), rect: Rect },
    ResourceImage { image_idx: u32, uri: Option<String>, rect: Rect },
}

#[derive(Clone)]
pub struct RenderScene {
    size: Size,
    content_height: f64,
    clipped: bool,
    commands: Arc<[RenderCommand]>,
}

impl RenderScene {
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn content_height(&self) -> f64 {
        self.content_height
    }

    pub fn is_clipped(&self) -> bool {
        self.clipped
    }

    pub fn clip_rect(&self) -> Rect {
        Rect::from_origin_size(Point::ORIGIN, self.size)
    }

    pub fn commands(&self) -> &[RenderCommand] {
        &self.commands
    }

    pub fn paint(&self, painter: &mut impl Painter) {
        for command in self.commands.iter() {
            match command {
                RenderCommand::PushClip { rect } => painter.push_clip(*rect),
                RenderCommand::PopClip => painter.pop_clip(),
                RenderCommand::Fill { rect, color } => painter.fill_rect(*rect, *color),
                RenderCommand::RoundedFill { rect, radii, color } => painter.fill_rounded_rect(*rect, *radii, *color),
                RenderCommand::RoundedBorder { rect, radii, width, color } => painter.stroke_rounded_rect(*rect, *radii, *width, *color),
                RenderCommand::Glyph { glyph, origin, color: Some(color) } => painter.draw_glyph_with_color(*glyph, *origin, *color),
                RenderCommand::Glyph { glyph, origin, color: None } => painter.draw_glyph(*glyph, *origin),
                RenderCommand::Image { image, hash, rect } => painter.draw_image(image, hash, *rect),
                RenderCommand::Svg { bytes, hash, intrinsic_size, rect } => painter.draw_svg(bytes, hash, *intrinsic_size, *rect),
                RenderCommand::ResourceImage { image_idx, uri, rect } => painter.draw_resource_image(*image_idx, uri.as_deref(), *rect),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FragmentImagePolicy {
    Omit,
    Placeholder(u32),
    Reference,
}

impl Default for FragmentImagePolicy {
    fn default() -> Self {
        Self::Placeholder(0xe6e6e6ff)
    }
}

#[derive(Clone, Debug)]
pub struct FragmentRenderOptions {
    viewport_width: f64,
    maximum_height: Option<f64>,
    line_height: f64,
    root_font_size: u32,
    base_uri: String,
    user_styles: Vec<String>,
    image_policy: FragmentImagePolicy,
}

impl FragmentRenderOptions {
    pub fn new(viewport_width: f64, maximum_height: Option<f64>) -> Result<Self, FragmentRenderError> {
        if !viewport_width.is_finite() || viewport_width <= 0.0 {
            return Err(FragmentRenderError::new("fragment viewport width must be finite and positive"));
        }
        if maximum_height.is_some_and(|height| !height.is_finite() || height <= 0.0) {
            return Err(FragmentRenderError::new("fragment maximum height must be finite and positive"));
        }
        Ok(Self { viewport_width, maximum_height, line_height: 16.0, root_font_size: 16, base_uri: "fragment.xhtml".to_owned(), user_styles: Vec::new(), image_policy: FragmentImagePolicy::default() })
    }

    pub fn with_typography(mut self, root_font_size: u32, line_height: f64) -> Result<Self, FragmentRenderError> {
        if root_font_size == 0 || !line_height.is_finite() || line_height <= 0.0 {
            return Err(FragmentRenderError::new("fragment typography must use a positive font size and line height"));
        }
        self.root_font_size = root_font_size;
        self.line_height = line_height;
        Ok(self)
    }

    pub fn with_viewport_width(mut self, viewport_width: f64) -> Result<Self, FragmentRenderError> {
        if !viewport_width.is_finite() || viewport_width <= 0.0 {
            return Err(FragmentRenderError::new("fragment viewport width must be finite and positive"));
        }
        self.viewport_width = viewport_width;
        Ok(self)
    }

    pub fn with_maximum_height(mut self, maximum_height: Option<f64>) -> Result<Self, FragmentRenderError> {
        if maximum_height.is_some_and(|height| !height.is_finite() || height <= 0.0) {
            return Err(FragmentRenderError::new("fragment maximum height must be finite and positive"));
        }
        self.maximum_height = maximum_height;
        Ok(self)
    }

    pub fn with_base_uri(mut self, base_uri: impl Into<String>) -> Result<Self, FragmentRenderError> {
        let base_uri = base_uri.into();
        if base_uri.trim().is_empty() || base_uri.chars().any(char::is_control) {
            return Err(FragmentRenderError::new("fragment base URI must be non-empty and contain no control characters"));
        }
        self.base_uri = base_uri;
        Ok(self)
    }

    pub fn with_user_styles(mut self, user_styles: Vec<String>) -> Self {
        self.user_styles = user_styles;
        self
    }

    pub fn with_image_policy(mut self, image_policy: FragmentImagePolicy) -> Self {
        self.image_policy = image_policy;
        self
    }

    pub fn viewport_width(&self) -> f64 {
        self.viewport_width
    }

    pub fn maximum_height(&self) -> Option<f64> {
        self.maximum_height
    }

    pub fn line_height(&self) -> f64 {
        self.line_height
    }

    pub fn root_font_size(&self) -> u32 {
        self.root_font_size
    }

    pub fn base_uri(&self) -> &str {
        &self.base_uri
    }

    pub fn user_styles(&self) -> &[String] {
        &self.user_styles
    }

    pub fn image_policy(&self) -> FragmentImagePolicy {
        self.image_policy
    }
}

pub struct RenderedFragment {
    document: LaidOutDocument,
    scene: RenderScene,
}

impl RenderedFragment {
    pub fn document(&self) -> &LaidOutDocument {
        &self.document
    }

    pub fn scene(&self) -> &RenderScene {
        &self.scene
    }

    pub fn into_parts(self) -> (LaidOutDocument, RenderScene) {
        (self.document, self.scene)
    }
}

pub struct FragmentRenderer {
    session: PipelineSession,
    resource_revision: ResourceRevision,
    stylesheet_revision: StylesheetRevision,
    font_revision: FontEnvironmentRevision,
    image_metrics_revision: ImageMetricsRevision,
}

impl Default for FragmentRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl FragmentRenderer {
    pub fn new() -> Self {
        Self::with_provider(Arc::new(DenyResourceProvider))
    }

    pub fn with_provider(provider: Arc<dyn ResourceProvider>) -> Self {
        Self {
            session: PipelineSession::new(provider),
            resource_revision: ResourceRevision::INITIAL,
            stylesheet_revision: StylesheetRevision::INITIAL,
            font_revision: FontEnvironmentRevision::INITIAL,
            image_metrics_revision: ImageMetricsRevision::INITIAL,
        }
    }

    pub fn invalidate_resources(&mut self) {
        self.resource_revision = self.resource_revision.next();
        self.stylesheet_revision = self.stylesheet_revision.next();
        self.image_metrics_revision = self.image_metrics_revision.next();
    }

    pub fn invalidate_fonts(&mut self) {
        self.font_revision = self.font_revision.next();
    }

    pub fn render(&mut self, source: &str, options: &FragmentRenderOptions, glyph_shaper: &mut impl GlyphShaper) -> Result<RenderedFragment, FragmentRenderError> {
        let inputs = PipelineInputs {
            source: source.to_owned(),
            markup_syntax: html_pipeline::MarkupSyntax::Html,
            user_styles: options.user_styles.clone(),
            reader_overrides: Default::default(),
            note_flow: Default::default(),
            source_revision: SourceRevision::INITIAL,
            base_uri: options.base_uri.clone(),
            resource_revision: self.resource_revision,
            stylesheet_revision: self.stylesheet_revision,
            style_environment: StyleEnvironment {
                root_font_size: options.root_font_size,
                media: html_pipeline::MediaEnvironment::screen(options.viewport_width, options.maximum_height).expect("validated fragment dimensions must form a media environment"),
                direction: 0,
            },
            font_environment: self.font_revision,
            image_metrics_revision: self.image_metrics_revision,
            layout: LayoutConstraints {
                viewport_width: options.viewport_width,
                viewport_height: options.maximum_height,
                line_height: options.line_height,
                image_sizing_policy: html_pipeline::ImageSizingPolicy::WebCompatible,
                text_composition_policy: html_pipeline::TextCompositionPolicy::WebCompatible,
            },
            image_metrics: Default::default(),
            paint: PaintSettingsRevision::INITIAL,
        };
        self.session.update(inputs, glyph_shaper).map_err(|error| FragmentRenderError::new(error.to_string()))?;
        let document = self.session.document().ok_or_else(|| FragmentRenderError::new("fragment pipeline produced no laid-out document"))?.clone();
        let scene = build_fragment_scene(&document, options);
        Ok(RenderedFragment { document, scene })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FragmentRenderError(String);

impl FragmentRenderError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for FragmentRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FragmentRenderError {}

pub fn paint_line_glyphs(document: &LaidOutDocument, line_idx: usize, origin: Point, scale: f64, color: Option<u32>, painter: &mut impl Painter) {
    let text = document.render_view().text();
    let Some(line) = text.line(line_idx) else { return };
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let snap = |value: f64| (value * scale).round() / scale;
    let baseline_y = origin.y + line.baseline();
    let offsets = text.line_glyph_offsets(line_idx);
    let advances = text.line_glyph_advances(line_idx);
    let mut offset_index = 0usize;
    let mut current_offset = offsets.as_ref().and_then(|runs| runs.get(0));
    let mut advance_index = 0usize;
    let mut current_advance = advances.as_ref().and_then(|runs| runs.get(0));
    for fragment in text.line_text_fragments(line_idx).into_iter().flatten() {
        let mut current_x = origin.x + line.optical_offset_x() + fragment.offset_x();
        for index in fragment.glyphs() {
            let glyph = text.glyph_at(index as usize).unwrap_or_default();
            let Some(metric) = text.glyph_metric(glyph) else { continue };
            while current_offset.as_ref().is_some_and(|run| index >= run.range().end) {
                offset_index += 1;
                current_offset = offsets.as_ref().and_then(|runs| runs.get(offset_index));
            }
            let offset = current_offset.as_ref().filter(|run| index >= run.range().start).map(|run| run.offset()).unwrap_or(0.0);
            while current_advance.as_ref().is_some_and(|run| index >= run.range().end) {
                advance_index += 1;
                current_advance = advances.as_ref().and_then(|runs| runs.get(advance_index));
            }
            let advance_override = current_advance.as_ref().filter(|run| index >= run.range().start).map(|run| run.advance());
            // Keep the shaped inline-axis origin fractional. Snapping every
            // glyph independently changes its phase at inline boundaries and
            // can make text disagree with adjacent CSS box edges.
            let draw_point = Point::new(current_x, snap(baseline_y - metric.baseline_offset() as f64 - offset));
            if advance_override.is_none() && metric.ch() != '\u{00ad}' {
                match color {
                    Some(color) => painter.draw_glyph_with_color(glyph, draw_point, color),
                    None => painter.draw_glyph(glyph, draw_point),
                }
            }
            current_x += advance_override.unwrap_or_else(|| {
                let tracking = if index + 1 < line.end() && text.is_character_cluster_boundary(index + 1) { line.letter_spacing() } else { 0.0 };
                text.character_advance(index).unwrap_or_else(|| metric.advance()) as f64 + tracking + if metric.ch() == ' ' { line.word_spacing() } else { 0.0 }
            });
        }
    }
    paint_line_ellipsis(document, line_idx, origin, scale, color, painter);
}

/// Paints one sparse text fragment from a line that requires source-ordered
/// interleaving with inline decorations or replaced content.
pub fn paint_line_glyph_fragment(document: &LaidOutDocument, line_idx: usize, fragment: &html_layout::RenderLineTextFragment, origin: Point, scale: f64, color: Option<u32>, painter: &mut impl Painter) {
    let text = document.render_view().text();
    let Some(line) = text.line(line_idx) else { return };
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let snap = |value: f64| (value * scale).round() / scale;
    let baseline_y = origin.y + line.baseline();
    let offsets = text.line_glyph_offsets(line_idx);
    let advances = text.line_glyph_advances(line_idx);
    let mut current_x = origin.x + line.optical_offset_x() + fragment.offset_x();
    for index in fragment.glyphs() {
        let glyph = text.glyph_at(index as usize).unwrap_or_default();
        let Some(metric) = text.glyph_metric(glyph) else { continue };
        let offset = offsets.as_ref().and_then(|runs| runs.iter().find(|run| index >= run.range().start && index < run.range().end)).map_or(0.0, |run| run.offset());
        let advance_override = advances.as_ref().and_then(|runs| runs.iter().find(|run| index >= run.range().start && index < run.range().end)).map(|run| run.advance());
        let draw_point = Point::new(current_x, snap(baseline_y - metric.baseline_offset() as f64 - offset));
        if advance_override.is_none() && metric.ch() != '\u{00ad}' {
            match color {
                Some(color) => painter.draw_glyph_with_color(glyph, draw_point, color),
                None => painter.draw_glyph(glyph, draw_point),
            }
        }
        current_x += advance_override.unwrap_or_else(|| {
            let tracking = if index + 1 < line.end() && text.is_character_cluster_boundary(index + 1) { line.letter_spacing() } else { 0.0 };
            text.character_advance(index).unwrap_or_else(|| metric.advance()) as f64 + tracking + if metric.ch() == ' ' { line.word_spacing() } else { 0.0 }
        });
    }
}

/// Paints a line from document-owned native shaping resources. Returns false
/// when the backend did not retain complete coverage or layout added placement
/// adjustments that require the portable glyph path.
pub fn paint_line_text_runs(document: &LaidOutDocument, line_idx: usize, origin: Point, scale: f64, color: Option<u32>, painter: &mut impl Painter) -> bool {
    if !painter.supports_text_runs() {
        return false;
    }
    let text = document.render_view().text();
    let Some(line) = text.line(line_idx) else { return false };
    if line.glyphs().any(|index| text.glyph_at(index as usize).and_then(|glyph| text.glyph_metric(glyph)).is_some_and(|metric| metric.ch() == '\u{00ad}')) {
        return false;
    }
    if line.letter_spacing() != 0.0 || line.word_spacing() != 0.0 || text.line_glyph_offsets(line_idx).is_some_and(|runs| runs.iter().next().is_some()) || text.line_glyph_advances(line_idx).is_some_and(|runs| runs.iter().next().is_some()) {
        return false;
    }
    let Some(fragments) = text.line_text_fragments(line_idx) else { return false };
    for fragment in text.line_text_fragments(line_idx).into_iter().flatten() {
        let mut expected = fragment.glyphs().start;
        for run in text.authoritative_runs(fragment.glyphs()) {
            if run.source_range().start != expected || run.placement_required() {
                return false;
            }
            expected = run.source_range().end;
        }
        if expected != fragment.glyphs().end {
            return false;
        }
    }
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let snap = |value: f64| (value * scale).round() / scale;
    for fragment in fragments {
        let mut x = origin.x + line.optical_offset_x() + fragment.offset_x();
        for run in text.authoritative_runs(fragment.glyphs()) {
            // Preserve the fractional line and inline offsets: independently
            // snapping every run changes glyph rasterization whenever an
            // inline boundary (including an atomic inline) splits otherwise
            // continuous text into multiple native runs.
            let run_origin = Point::new(x - f64::from(run.natural_offset()), snap(origin.y + line.baseline() - f64::from(run.ascent())));
            painter.draw_text_run_fragment(run.run(), run.run_range(), run_origin, color);
            for index in run.source_range() {
                x += f64::from(text.character_advance(index).unwrap_or(0.0));
            }
        }
    }
    paint_line_ellipsis(document, line_idx, origin, scale, color, painter);
    true
}

/// Paints the synthetic overflow marker separately from selectable source
/// text. Native text-run painters can use this without falling back to drawing
/// the source line character by character.
pub fn paint_line_ellipsis(document: &LaidOutDocument, line_idx: usize, origin: Point, scale: f64, color: Option<u32>, painter: &mut impl Painter) {
    let text = document.render_view().text();
    let Some(line) = text.line(line_idx) else { return };
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let snap = |value: f64| (value * scale).round() / scale;
    let baseline_y = origin.y + line.baseline();
    if let Some(ellipsis) = text.ellipsis_for_line(line_idx)
        && let Some(metric) = text.glyph_metric(ellipsis.glyph())
    {
        let offset = ellipsis.offset();
        let point = Point::new(snap(origin.x + line.optical_offset_x() + offset.x), snap(baseline_y - metric.baseline_offset() as f64 - offset.y));
        match color {
            Some(color) => painter.draw_glyph_with_color(ellipsis.glyph(), point, color),
            None => painter.draw_glyph(ellipsis.glyph(), point),
        }
    }
    if let Some(hyphen) = text.hyphen_for_line(line_idx)
        && let Some(metric) = text.glyph_metric(hyphen.glyph())
    {
        let offset = hyphen.offset();
        let point = Point::new(snap(origin.x + line.optical_offset_x() + offset.x), snap(baseline_y - metric.baseline_offset() as f64 - offset.y));
        match color {
            Some(color) => painter.draw_glyph_with_color(hyphen.glyph(), point, color),
            None => painter.draw_glyph(hyphen.glyph(), point),
        }
    }
}

impl RenderScene {
    /// Builds a scene for a document that is already laid out, for callers that
    /// produced one themselves rather than rendering a fragment from source.
    ///
    /// A reader laying out a note keeps the containing document's computed
    /// styles, so it cannot go through [`FragmentRenderer::render`], which
    /// re-renders from source in a session of its own. It still wants this:
    /// natural height over lines, decorations and images, content in paint
    /// order, and clipping to the height the caller can show.
    pub fn for_document(document: &LaidOutDocument, options: &FragmentRenderOptions) -> Self {
        build_fragment_scene(document, options)
    }
}

fn build_fragment_scene(document: &LaidOutDocument, options: &FragmentRenderOptions) -> RenderScene {
    let view = document.render_view();
    let text = view.text();
    let fragments = view.fragments();
    let mut natural_height = text.lines().iter().map(|line| line.point().y + line.height()).fold(0.0_f64, f64::max);
    natural_height = fragments.decorations().iter().map(|decoration| decoration.rect().y1).fold(natural_height, f64::max);
    for (line_idx, line) in text.lines().iter().enumerate() {
        natural_height = fragments.images_for_line(line_idx).iter().map(|image| line.point().y + image.clip().y1).fold(natural_height, f64::max);
    }
    let height = options.maximum_height.map(|maximum| natural_height.min(maximum)).unwrap_or(natural_height);
    let clip = Rect::new(0.0, 0.0, options.viewport_width, height);
    let mut recorder = SceneRecorder::default();
    paint_fragment_decorations(fragments, clip, false, &mut recorder);
    for line_idx in text.paint_order_indices().iter().filter_map(|&index| usize::try_from(index).ok()) {
        let Some(line) = text.line(line_idx) else { continue };
        if line.point().y >= height {
            continue;
        }
        let overflow_clip = text.line_overflow_clip(line_idx).map(|overflow| {
            let value = overflow.rect();
            Rect::new(if overflow.clips_x() { value.x0 } else { clip.x0 }, if overflow.clips_y() { value.y0 } else { clip.y0 }, if overflow.clips_x() { value.x1 } else { clip.x1 }, if overflow.clips_y() { value.y1 } else { clip.y1 })
                .intersect(clip)
        });
        if let Some(overflow_clip) = overflow_clip {
            recorder.push_clip(overflow_clip);
        }
        for image in fragments.images_for_line(line_idx).iter() {
            let rect = Rect::from_origin_size(line.point() + image.offset().to_vec2(), image.size());
            let image_clip = (image.clip() + line.point().to_vec2()).intersect(clip);
            if rect.intersect(image_clip).is_zero_area() {
                continue;
            }
            match options.image_policy {
                FragmentImagePolicy::Omit => {}
                FragmentImagePolicy::Placeholder(color) => recorder.fill_rect(rect.intersect(image_clip), color_from_u32(color)),
                FragmentImagePolicy::Reference if rect.intersect(image_clip) == rect => recorder.draw_resource_image(image.image_idx(), view.image_uri(image.image_idx()), rect),
                FragmentImagePolicy::Reference => {
                    recorder.push_clip(image_clip);
                    recorder.draw_resource_image(image.image_idx(), view.image_uri(image.image_idx()), rect);
                    recorder.pop_clip();
                }
            }
        }
        paint_line_glyphs(document, line_idx, line.point(), 1.0, None, &mut recorder);
        if overflow_clip.is_some() {
            recorder.pop_clip();
        }
    }
    paint_fragment_decorations(fragments, clip, true, &mut recorder);
    RenderScene { size: Size::new(options.viewport_width, height), content_height: natural_height, clipped: height < natural_height, commands: recorder.commands.into() }
}

fn paint_fragment_decorations(fragments: html_layout::RenderFragmentView<'_>, clip: Rect, foreground: bool, recorder: &mut SceneRecorder) {
    for decoration in fragments.decorations().iter().filter(|decoration| decoration.is_foreground() == foreground) {
        paint_scene_decoration(&decoration, clip, recorder);
    }
}

fn paint_scene_decoration(decoration: &html_layout::RenderDecoration, clip: Rect, recorder: &mut SceneRecorder) {
    let mut rect = decoration.rect().intersect(clip);
    if let Some(overflow) = decoration.overflow_clip() {
        let overflow_rect = overflow.rect();
        let x0 = if overflow.clips_x() { rect.x0.max(overflow_rect.x0) } else { rect.x0 };
        let x1 = if overflow.clips_x() { rect.x1.min(overflow_rect.x1) } else { rect.x1 };
        let y0 = if overflow.clips_y() { rect.y0.max(overflow_rect.y0) } else { rect.y0 };
        let y1 = if overflow.clips_y() { rect.y1.min(overflow_rect.y1) } else { rect.y1 };
        rect = Rect::new(x0, y0, x1, y1);
    }
    if rect.width() > 0.0 && rect.height() > 0.0 {
        paint_resolved_decoration(recorder, decoration, rect);
    }
}

fn color_from_u32(color: u32) -> Color {
    Color::rgba8((color >> 24) as u8, (color >> 16) as u8, (color >> 8) as u8, color as u8)
}

/// Converts one backend-neutral layout fragment into painter operations.
/// Pagination may provide a clipped/split rectangle, but CSS color, rounded
/// fill, and border execution have one shared owner here.
pub fn paint_resolved_decoration(painter: &mut impl Painter, decoration: &html_layout::RenderDecoration, rect: Rect) {
    paint_projected_decoration(painter, decoration, rect, decoration.radii());
}

/// Paint a semantic decoration after an outer projection (pagination,
/// columns, or clipping) has adjusted its rectangle and exposed corners.
pub fn paint_projected_decoration(painter: &mut impl Painter, decoration: &html_layout::RenderDecoration, rect: Rect, radii: Option<UsedBorderRadii>) {
    use html_layout::RenderDecorationPattern;

    let color = color_from_u32(decoration.color());
    match decoration.pattern() {
        RenderDecorationPattern::Solid => paint_decoration_style(painter, rect, decoration.color(), radii, decoration.border_width()),
        RenderDecorationPattern::DoubleHorizontal => {
            let thickness = rect.height() / 3.0;
            if thickness > 0.0 {
                painter.fill_rect(Rect::new(rect.x0, rect.y0, rect.x1, rect.y0 + thickness), color);
                painter.fill_rect(Rect::new(rect.x0, rect.y1 - thickness, rect.x1, rect.y1), color);
            }
        }
        RenderDecorationPattern::DottedHorizontal => paint_horizontal_pattern(painter, rect, color, 1.0, 1.0),
        RenderDecorationPattern::DashedHorizontal => paint_horizontal_pattern(painter, rect, color, 3.0, 2.0),
        RenderDecorationPattern::DottedVertical => paint_vertical_pattern(painter, rect, color, 1.0, 1.0),
        RenderDecorationPattern::DashedVertical => paint_vertical_pattern(painter, rect, color, 3.0, 2.0),
    }
}

/// Paints a resolved decoration after an outer projection (for example,
/// pagination) has adjusted which physical corners remain exposed.
pub fn paint_decoration_style(painter: &mut impl Painter, rect: Rect, color: u32, radii: Option<UsedBorderRadii>, border_width: Option<f32>) {
    let color = color_from_u32(color);
    match (radii, border_width) {
        (Some(radii), Some(width)) => painter.stroke_rounded_rect(rect, radii, width, color),
        (Some(radii), None) => painter.fill_rounded_rect(rect, radii, color),
        (None, _) => painter.fill_rect(rect, color),
    }
}

fn paint_horizontal_pattern(painter: &mut impl Painter, rect: Rect, color: Color, dash_units: f64, gap_units: f64) {
    let thickness = rect.height();
    if thickness <= 0.0 || rect.width() <= 0.0 {
        return;
    }
    let dash = thickness * dash_units;
    let gap = thickness * gap_units;
    let mut x = rect.x0;
    while x < rect.x1 {
        let end = (x + dash).min(rect.x1);
        painter.fill_rect(Rect::new(x, rect.y0, end, rect.y1), color);
        x = end + gap;
    }
}

fn paint_vertical_pattern(painter: &mut impl Painter, rect: Rect, color: Color, dash_units: f64, gap_units: f64) {
    let thickness = rect.width();
    if thickness <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let dash = thickness * dash_units;
    let gap = thickness * gap_units;
    let mut y = rect.y0;
    while y < rect.y1 {
        let end = (y + dash).min(rect.y1);
        painter.fill_rect(Rect::new(rect.x0, y, rect.x1, end), color);
        y = end + gap;
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

    fn fill_rounded_rect(&mut self, rect: Rect, radii: UsedBorderRadii, color: Color) {
        self.commands.push(RenderCommand::RoundedFill { rect, radii, color });
    }

    fn stroke_rounded_rect(&mut self, rect: Rect, radii: UsedBorderRadii, width: f32, color: Color) {
        self.commands.push(RenderCommand::RoundedBorder { rect, radii, width, color });
    }

    fn draw_glyph(&mut self, glyph: GlyphId, origin: Point) {
        self.commands.push(RenderCommand::Glyph { glyph, origin, color: None });
    }

    fn draw_glyph_with_color(&mut self, glyph: GlyphId, origin: Point, color: u32) {
        self.commands.push(RenderCommand::Glyph { glyph, origin, color: Some(color) });
    }

    fn draw_image(&mut self, image: &Image, hash: &[u8], rect: Rect) {
        self.commands.push(RenderCommand::Image { image: image.clone(), hash: hash.to_vec(), rect });
    }

    fn draw_svg(&mut self, bytes: &[u8], hash: &[u8], intrinsic_size: (u32, u32), rect: Rect) {
        self.commands.push(RenderCommand::Svg { bytes: Arc::from(bytes), hash: hash.to_vec(), intrinsic_size, rect });
    }

    fn draw_resource_image(&mut self, image_idx: u32, uri: Option<&str>, rect: Rect) {
        self.commands.push(RenderCommand::ResourceImage { image_idx, uri: uri.map(str::to_owned), rect });
    }
}

struct DenyResourceProvider;

impl ResourceProvider for DenyResourceProvider {
    fn read_bytes(&self, _uri: &str) -> io::Result<Vec<u8>> {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "fragment external resources are disabled"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use html_layout::{FontSlant, GlyphMetric, GlyphRegistry, ShapeError};
    use std::collections::HashMap;

    #[derive(Default)]
    struct TestShaper {
        glyphs: HashMap<(char, u32), GlyphId>,
    }

    impl GlyphShaper for TestShaper {
        fn reset(&mut self) {
            self.glyphs.clear();
        }

        fn shape_glyph<'a>(&mut self, glyph_metrics: &mut GlyphRegistry<'a>, character: char, font_size: f32, _font_weight: u16, _font_slant: FontSlant, _color: u32, _family: Option<&str>) -> Result<GlyphId, ShapeError> {
            let key = (character, font_size.to_bits());
            if let Some(glyph) = self.glyphs.get(&key) {
                return Ok(*glyph);
            }
            let metric = GlyphMetric::try_new(character, font_size * 0.5, font_size * 0.75, font_size * 0.25, font_size * 0.75).map_err(ShapeError::rejected_metric)?;
            let glyph = glyph_metrics.register(metric)?;
            self.glyphs.insert(key, glyph);
            Ok(glyph)
        }
    }

    #[derive(Default)]
    struct LegacyPainter {
        fills: usize,
    }

    impl Painter for LegacyPainter {
        fn fill_rect(&mut self, _rect: Rect, _color: Color) {
            self.fills += 1;
        }

        fn draw_glyph(&mut self, _glyph: GlyphId, _origin: Point) {}

        fn draw_image(&mut self, _image: &Image, _hash: &[u8], _rect: Rect) {}
    }

    #[test]
    fn rounded_primitives_are_optional_for_painter_implementers() {
        let mut painter = LegacyPainter::default();
        let rect = Rect::new(0.0, 0.0, 20.0, 20.0);
        let radii = UsedBorderRadii { top_left: (4.0, 4.0), top_right: (4.0, 4.0), bottom_right: (4.0, 4.0), bottom_left: (4.0, 4.0) };
        painter.fill_rounded_rect(rect, radii, Color::BLACK);
        painter.stroke_rounded_rect(rect, radii, 2.0, Color::BLACK);
        assert_eq!(painter.fills, 5, "default methods preserve square fill/border behavior");
    }

    #[test]
    fn rounded_background_and_uniform_border_reach_the_render_scene() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = FragmentRenderOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<div style='width:100px;height:30px;background:red;border:2px solid blue;border-radius:12px'>rounded</div>", &options, &mut shaper).unwrap();

        assert!(rendered.scene().commands().iter().any(|command| matches!(command, RenderCommand::RoundedFill { .. })));
        assert!(rendered.scene().commands().iter().any(|command| matches!(command, RenderCommand::RoundedBorder { width, .. } if *width == 2.0)));
        let mut recording = RecordingPainter::default();
        rendered.scene().paint(&mut recording);
        assert!(!recording.rounded_fills.is_empty());
        assert!(!recording.rounded_borders.is_empty());
    }

    #[test]
    fn foreground_text_decoration_is_recorded_after_glyphs() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = FragmentRenderOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<span style='text-decoration-line:line-through;text-decoration-color:#ff0000'>marked</span>", &options, &mut shaper).unwrap();
        let commands = rendered.scene().commands();
        let last_glyph = commands.iter().rposition(|command| matches!(command, RenderCommand::Glyph { .. })).expect("fixture emits glyphs");
        let foreground = commands.iter().position(|command| matches!(command, RenderCommand::Fill { color, .. } if color.r == 255 && color.g == 0 && color.b == 0)).expect("line-through emits a foreground decoration");
        assert!(foreground > last_glyph, "foreground decorations must paint after text");
    }

    #[test]
    fn render_core_tessellates_semantic_dashed_decorations() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = FragmentRenderOptions::new(240.0, None).unwrap();
        let rendered = renderer.render("<span style='text-decoration:line-through 2px dashed #ff0000'>patterned decoration</span>", &options, &mut shaper).unwrap();

        let semantic = rendered.document().render_view().fragments().decorations().iter().filter(|decoration| decoration.color() == 0xff0000ff).collect::<Vec<_>>();
        assert_eq!(semantic.len(), 1, "layout publishes one semantic dashed span");
        assert_eq!(semantic[0].pattern(), html_layout::RenderDecorationPattern::DashedHorizontal);

        let painter_rects = rendered.scene().commands().iter().filter(|command| matches!(command, RenderCommand::Fill { color, .. } if color.r == 255 && color.g == 0 && color.b == 0)).count();
        assert!(painter_rects > 1, "render core expands the semantic span into painter primitives");
    }

    #[test]
    fn renders_bounded_html_fragment_into_a_replayable_scene() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = FragmentRenderOptions::new(160.0, Some(24.0)).unwrap();
        let rendered = renderer.render("<div style='background:#eee'><p>Hello <strong>fragment</strong></p><p>second line</p></div>", &options, &mut shaper).unwrap();
        assert!(rendered.scene().is_clipped());
        assert!(rendered.scene().commands().iter().any(|command| matches!(command, RenderCommand::Glyph { .. })));
        let mut replay = RecordingPainter::default();
        rendered.scene().paint(&mut replay);
        assert!(!replay.glyphs.is_empty());
    }

    #[test]
    fn overflow_hidden_clips_descendant_backgrounds_and_records_text_masks() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = FragmentRenderOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<div style='height:40px;overflow:hidden'><div style='height:20px;margin-top:40px;background:#ff0000'>outside</div></div>", &options, &mut shaper).unwrap();

        assert!(!rendered.scene().commands().iter().any(|command| matches!(command, RenderCommand::Fill { color, .. } if color.r == 255 && color.g == 0 && color.b == 0)));
        assert!(rendered.scene().commands().iter().any(|command| matches!(command, RenderCommand::PushClip { .. })));
        assert_eq!(rendered.scene().commands().iter().filter(|command| matches!(command, RenderCommand::PushClip { .. })).count(), rendered.scene().commands().iter().filter(|command| matches!(command, RenderCommand::PopClip)).count(),);
    }

    #[test]
    fn renders_atom_xhtml_without_flattening_its_structure() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = FragmentRenderOptions::new(240.0, None).unwrap();
        let rendered = renderer.render(r#"<div xmlns="http://www.w3.org/1999/xhtml"><p>This edition has <strong>images</strong>.</p><p>Title: Carmen</p></div>"#, &options, &mut shaper).unwrap();
        let view = rendered.document().render_view().text();
        let text = rendered
            .scene()
            .commands()
            .iter()
            .filter_map(|command| match command {
                RenderCommand::Glyph { glyph, .. } => view.glyph_metric(*glyph).map(|metric| metric.ch()),
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
        let options = FragmentRenderOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<p style='margin:0;white-space:nowrap'>a<img src='missing.png' style='width:20px;height:10px'>b</p>", &options, &mut shaper).unwrap();
        let view = rendered.document().render_view().text();
        let positions = rendered
            .scene()
            .commands()
            .iter()
            .filter_map(|command| match command {
                RenderCommand::Glyph { glyph, origin, .. } => Some((view.glyph_metric(*glyph)?.ch(), origin.x)),
                _ => None,
            })
            .collect::<HashMap<_, _>>();

        let a = positions[&'a'];
        let b = positions[&'b'];
        assert!(b - a >= 28.0, "the second text fragment must begin after the first glyph and the 20px image");
    }

    #[test]
    fn fallback_painting_preserves_fractional_inline_origins() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = FragmentRenderOptions::new(200.0, None).unwrap();
        let rendered = renderer.render("<span style='margin-left:0.5px'>X</span>", &options, &mut shaper).unwrap();
        let origin = rendered
            .scene()
            .commands()
            .iter()
            .find_map(|command| match command {
                RenderCommand::Glyph { origin, .. } => Some(*origin),
                _ => None,
            })
            .expect("fixture emits a glyph");

        assert!((origin.x.fract() - 0.5).abs() < 0.01, "fractional inline origin was lost: {}", origin.x);
    }

    #[test]
    fn same_fragment_reflows_when_its_target_width_changes() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let source = "<p>A deliberately long description that must wrap over several lines in the narrow target.</p>";
        let wide = renderer.render(source, &FragmentRenderOptions::new(480.0, None).unwrap(), &mut shaper).unwrap().scene().content_height();
        let narrow = renderer.render(source, &FragmentRenderOptions::new(100.0, None).unwrap(), &mut shaper).unwrap().scene().content_height();
        assert!(narrow > wide, "expected narrow fragment height {narrow} to exceed wide height {wide}");
    }

    #[test]
    fn can_retain_image_references_for_a_target_adapter() {
        let mut renderer = FragmentRenderer::new();
        let mut shaper = TestShaper::default();
        let options = FragmentRenderOptions::new(240.0, None).unwrap().with_image_policy(FragmentImagePolicy::Reference);
        let rendered = renderer.render(r#"<p>Before</p><img src="cover.png" width="20" height="40"><p>After</p>"#, &options, &mut shaper).unwrap();
        assert!(rendered.scene().commands().iter().any(|command| matches!(command, RenderCommand::ResourceImage { uri: Some(uri), .. } if uri == "cover.png")));
    }

    #[test]
    fn rejects_invalid_fragment_constraints() {
        assert!(FragmentRenderOptions::new(0.0, None).is_err());
        assert!(FragmentRenderOptions::new(100.0, Some(f64::NAN)).is_err());
        assert!(FragmentRenderOptions::new(100.0, None).unwrap().with_viewport_width(f64::INFINITY).is_err());
        assert!(FragmentRenderOptions::new(100.0, None).unwrap().with_maximum_height(Some(0.0)).is_err());
        assert!(FragmentRenderOptions::new(100.0, None).unwrap().with_base_uri("\n").is_err());
    }
}
