use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use web_time::Instant;

use gpui::{App, Bounds, ContentMask, Corners, Edges, Hsla, PaintGlyph, Pixels, RenderImage, SvgRenderer, Window, fill, point, px, rgba, size};
use gpui_image_eviction::track_image;
use html::layout::{PreparedTextRunFragment, ThemeAdaptation, UsedBorderRadii};
use html::render::{Painter, TextRunFragment};
use html::text::{GlyphId, TextRunId};
use image::Frame;
use kurbo::{Point, Rect};
use peniko::{Color, Image as PenikoImage};
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use smallvec::SmallVec;

use crate::math_svg::themed_math_svg;
use crate::raster_theme::{RasterClassifier, RasterKind, raster_buffer, raster_cache_key};
use crate::svg_theme::{themed_svg, themed_svg_cache_key};
use crate::text::{GlyphStorage, GlyphStore, GpuiGlyph, GpuiTextRun, RunStore};

#[cfg(test)]
mod glyph_fragment_tests {
    use super::*;
    use crate::text::GpuiRunGlyph;
    use gpui::{FontId, GlyphId as NativeGlyphId};
    use std::rc::Rc;

    fn run(text: &str, glyphs: &[(usize, f32)], carets: &[f32], colors: Vec<(Range<usize>, Hsla)>) -> Arc<GpuiTextRun> {
        let glyphs = glyphs
            .iter()
            .enumerate()
            .map(|(id, &(byte_index, x))| GpuiRunGlyph { font_id: FontId(0), glyph_id: NativeGlyphId(id as u32), position: point(px(x), px(0.0)), font_size: px(19.0), byte_index, is_emoji: false })
            .collect::<Vec<_>>();
        let bytes = text.char_indices().map(|(byte, _)| byte).chain([text.len()]).collect::<Vec<_>>();
        let mut run = GpuiTextRun::new(glyphs.into(), px(14.0), px(5.0), bytes.into(), colors.into());
        run.caret_stops = Arc::from(carets);
        Arc::new(run)
    }

    fn paint_characters(run: &Arc<GpuiTextRun>, characters: &[u32], color: Option<u32>) -> Vec<PaintCommand> {
        let mut glyphs = GlyphStorage::default();
        glyphs.push_run(run.clone(), run.caret_stops.len() - 1);
        // A retained glyph store must paint independently of the current run
        // store, which may already belong to a different document.
        let mut painter = GpuiCommandPainter::commands_only(Rc::new(glyphs), Rc::new(Vec::new()), Vec::new());
        for &character in characters {
            let origin = Point::new(100.0 + f64::from(run.caret_stops[character as usize]), 200.0);
            let color = color.unwrap_or_else(|| u32::from(run.character_color(character).to_rgb()));
            painter.draw_glyph(character, origin, color);
        }
        painter.into_commands()
    }

    fn fragment(commands: &[PaintCommand]) -> &GpuiTextRunFragment {
        let [PaintCommand::TextBatch { fragments }] = commands else { panic!("expected a text batch") };
        assert_eq!(fragments.len(), 1);
        &fragments[0]
    }

    #[test]
    fn deferred_character_painting_preserves_ligatures_combining_marks_and_visual_order() {
        let red = Hsla::from(rgba(0xff0000ff));
        for (run, character, expected) in [
            (run("office", &[(0, 0.0), (1, 10.0), (4, 40.0), (5, 50.0)], &[0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0], vec![(0..6, red)]), 1, vec![1..2]),
            (run("a\u{301}b", &[(0, 0.0), (0, 2.0), (3, 10.0)], &[0.0, 5.0, 10.0, 20.0], vec![(0..4, red)]), 0, vec![0..2]),
            (run("a\u{301}b", &[(0, 0.0), (0, 2.0), (3, 10.0)], &[0.0, 5.0, 10.0, 20.0], vec![(0..4, red)]), 1, vec![]),
            (run("aבc", &[(0, 0.0), (3, 20.0), (1, 10.0)], &[0.0, 10.0, 20.0, 30.0], vec![(0..4, red)]), 2, vec![1..2]),
        ] {
            let commands = paint_characters(&run, &[character], None);
            let fragment = fragment(&commands);
            assert!(Arc::ptr_eq(&fragment.run, &run));
            assert_eq!(fragment.range, character..character + 1);
            assert_eq!(fragment.origin, Point::new(100.0, 200.0));
            assert_eq!(fragment.glyph_ranges.as_slice(), expected);
            assert_eq!(fragment.color, Some(red));
        }
    }

    #[test]
    fn character_painting_reuses_utf8_colors_merges_fragments_and_honors_overrides() {
        let red = Hsla::from(rgba(0xff0000ff));
        let blue = Hsla::from(rgba(0x0000ffff));
        let run = run("aבc", &[(0, 0.0), (3, 20.0), (1, 10.0)], &[0.0, 10.0, 20.0, 30.0], vec![(0..1, red), (1..4, blue)]);
        assert_eq!(run.character_color(0), red);
        assert_eq!(run.character_color(1), blue);
        assert_eq!(run.character_color(2), blue);

        for color in [None, Some(0x00ff00ff)] {
            let commands = paint_characters(&run, &[0, 1, 2], color);
            let [PaintCommand::TextBatch { fragments }] = commands.as_slice() else { panic!("expected a text batch") };
            if let Some(color) = color {
                assert_eq!(fragments.len(), 1);
                assert_eq!(fragments[0].range, 0..3);
                assert_eq!(fragments[0].glyph_ranges.as_slice(), &[0..3]);
                assert_eq!(fragments[0].color, Some(Hsla::from(rgba(color))));
            } else {
                assert_eq!(fragments.len(), 2);
                assert_eq!(fragments[0].range, 0..1);
                assert_eq!(fragments[0].color, Some(red));
                assert_eq!(fragments[1].range, 1..3);
                assert_eq!(fragments[1].color, Some(blue));
            }
            assert!(fragments.iter().all(|fragment| fragment.origin == Point::new(100.0, 200.0)));
        }
    }
}

#[cfg(test)]
mod color_override_tests {
    use super::*;
    use crate::text::GpuiTextCache;
    use gpui::{EmptyView, TestAppContext, rgb};
    use html::render::{FragmentRenderOptions, FragmentRenderer};

    #[gpui::test]
    fn text_color_override_covers_authored_and_generated_text_without_recoloring_backgrounds(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let mut renderer = FragmentRenderer::new();
                let source =
                    "<style>p::before { content:'Intro'; color:orange !important; }</style><p style='background-color:#008000;color:red !important'>Body <span style='color:blue !important;font-size:24px;font-weight:bold'>Nested</span></p>";
                let rendered = renderer.render(source, &FragmentRenderOptions::new(300.0, None).unwrap(), &mut shaper).unwrap();
                for theme_color in [rgb(0xeeeeee).into(), rgb(0x222222).into()] {
                    let mut painter = GpuiCommandPainter::commands_only(shaper.store(), shaper.run_store(), Vec::new()).text_color(theme_color);
                    rendered.scene().paint(&mut painter);
                    let commands = painter.into_commands();
                    let mut text_fragments = 0;
                    for command in &commands {
                        match command {
                            PaintCommand::Glyph { color, .. } => {
                                assert_eq!(*color, theme_color);
                                text_fragments += 1;
                            }
                            PaintCommand::TextBatch { fragments } => {
                                for fragment in fragments.iter() {
                                    assert_eq!(fragment.color, Some(theme_color));
                                    text_fragments += 1;
                                }
                            }
                            _ => {}
                        }
                    }
                    assert!(text_fragments > 0);
                    assert!(commands.iter().any(|command| matches!(command, PaintCommand::Fill { color, .. } if u32::from(color.to_rgb()) == 0x008000ff)), "authored background colors must remain intact");
                }
            })
            .unwrap();
    }
}

const MAX_IMAGE_PIXELS: f64 = 4_194_304.0;
const MAX_IMAGE_EDGE: f64 = 4096.0;
const IMAGE_CACHE_BYTES: usize = 16 * 1024 * 1024;

// Bound rasterization independently of authored layout size.
fn raster_size(width: f64, height: f64) -> Option<(u32, u32)> {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let width = width.max(1.0);
    let height = height.max(1.0);
    let scale = (MAX_IMAGE_EDGE / width).min(MAX_IMAGE_EDGE / height).min(1.0);
    let width = width * scale;
    let height = height * scale;
    let scale = (MAX_IMAGE_PIXELS / (width * height)).sqrt().min(1.0);
    Some(((width * scale).floor().max(1.0) as u32, (height * scale).floor().max(1.0) as u32))
}

#[derive(Default)]
pub struct ImageCache {
    entries: VecDeque<(Vec<u8>, Arc<RenderImage>, usize)>,
    bytes: usize,
    raster_classifier: RasterClassifier,
}

impl ImageCache {
    fn get(&mut self, key: &[u8]) -> Option<Arc<RenderImage>> {
        let index = self.entries.iter().position(|(stored, _, _)| stored == key)?;
        let entry = self.entries.remove(index)?;
        let image = entry.1.clone();
        self.entries.push_back(entry);
        Some(image)
    }

    fn insert(&mut self, key: Vec<u8>, image: Arc<RenderImage>) {
        let bytes = image.as_bytes(0).map_or(0, <[u8]>::len);
        if bytes > IMAGE_CACHE_BYTES {
            return;
        }
        while self.bytes + bytes > IMAGE_CACHE_BYTES || self.entries.len() >= 256 {
            if let Some((_, _, evicted)) = self.entries.pop_front() {
                self.bytes -= evicted;
            } else {
                break;
            }
        }
        self.bytes += bytes;
        self.entries.push_back((key, image, bytes));
    }
}

#[derive(Clone)]
pub enum PaintCommand {
    PushClip { rect: Rect },
    PopClip,
    Fill { rect: Rect, color: Hsla },
    RoundedFill { rect: Rect, radii: UsedBorderRadii, color: Hsla },
    RoundedBorder { rect: Rect, radii: UsedBorderRadii, width: f32, color: Hsla },
    Glyph { glyph: GlyphId, origin: Point, color: Hsla },
    TextBatch { fragments: Arc<Vec<GpuiTextRunFragment>> },
    Image { rect: Rect, image: Arc<RenderImage> },
}

#[derive(Clone)]
pub struct GpuiTextRunFragment {
    run: Arc<GpuiTextRun>,
    range: Range<u32>,
    glyph_ranges: SmallVec<[Range<usize>; 2]>,
    origin: Point,
    color: Option<Hsla>,
}

impl GpuiTextRunFragment {
    fn new(run: Arc<GpuiTextRun>, range: Range<u32>, origin: Point, color: Option<Hsla>) -> Self {
        let glyph_ranges = run.glyph_ranges(range.clone());
        Self { run, range, glyph_ranges, origin, color }
    }

    fn extend_to(&mut self, end: u32) {
        self.range.end = end;
        self.glyph_ranges = self.run.glyph_ranges(self.range.clone());
    }
}

pub struct GpuiCommandPainter<'a> {
    images: Option<&'a mut ImageCache>,
    glyphs: GlyphStore,
    text_runs: RunStore,
    text_color: Option<Hsla>,
    image_theme: Option<ThemeAdaptation>,
    pub(crate) commands: Vec<PaintCommand>,
}

impl<'a> GpuiCommandPainter<'a> {
    pub fn new(images: &'a mut ImageCache, glyphs: GlyphStore, text_runs: RunStore, mut commands: Vec<PaintCommand>) -> Self {
        commands.clear();
        Self { images: Some(images), glyphs, text_runs, text_color: None, image_theme: None, commands }
    }

    pub fn commands_only(glyphs: GlyphStore, text_runs: RunStore, mut commands: Vec<PaintCommand>) -> Self {
        commands.clear();
        Self { images: None, glyphs, text_runs, text_color: None, image_theme: None, commands }
    }

    /// Explicit text color override; non-text paint commands keep their colors.
    pub fn text_color(mut self, color: Hsla) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Reader palette for general SVG paint and confidently classified raster
    /// line art. Photographic pixels and explicit MathML colors are preserved.
    pub fn image_theme(mut self, theme: Option<ThemeAdaptation>) -> Self {
        self.image_theme = theme;
        self
    }

    fn draw_themed_svg(&mut self, bytes: &[u8], hash: &[u8], _intrinsic_size: (u32, u32), rect: Rect, theme: ThemeAdaptation) {
        let Some((width, height)) = raster_size(rect.width(), rect.height()) else { return };
        let key = themed_svg_cache_key(hash, width, height, theme);
        let rasterize = || {
            let viewport = svg_with_viewport(bytes, width, height)?;
            let renderer = SvgRenderer::new(Arc::new(()));
            let parsed = renderer.parse_svg(&viewport).ok()?;
            // Resolve CSS and text once before adapting the paint. The
            // normalized document keeps masks and embedded image pixels out
            // of the color mapping, including colors shared through defs.
            let normalized = parsed.to_svg();
            themed_svg(normalized.as_bytes(), theme).and_then(|themed| renderer.render_single_frame(&themed, 1.0).ok()).or_else(|| renderer.render_parsed(&parsed, 1.0).ok())
        };
        let rendered = match self.images.as_deref_mut() {
            Some(images) => images.get(&key).or_else(|| {
                let rendered = rasterize()?;
                images.insert(key, rendered.clone());
                Some(rendered)
            }),
            None => rasterize(),
        };
        if let Some(image) = rendered {
            self.commands.push(PaintCommand::Image { rect, image });
        }
    }

    pub fn into_commands(self) -> Vec<PaintCommand> {
        self.commands
    }

    fn push_text_fragment(&mut self, fragment: GpuiTextRunFragment) {
        if let Some(PaintCommand::TextBatch { fragments }) = self.commands.last_mut() {
            let fragments = Arc::make_mut(fragments);
            if let Some(previous) = fragments.last_mut()
                && Arc::ptr_eq(&previous.run, &fragment.run)
                && previous.range.end == fragment.range.start
                && previous.origin == fragment.origin
                && previous.color == fragment.color
            {
                previous.extend_to(fragment.range.end);
                return;
            }
            fragments.push(fragment);
        } else {
            self.commands.push(PaintCommand::TextBatch { fragments: Arc::new(vec![fragment]) });
        }
    }

    fn push_text_fragments(&mut self, fragments: impl IntoIterator<Item = GpuiTextRunFragment>) {
        for fragment in fragments {
            self.push_text_fragment(fragment);
        }
    }

    fn push_glyph(&mut self, glyph: GlyphId, origin: Point, color: Hsla) {
        let native = self.glyphs.get(glyph as usize);
        match native {
            Some(GpuiGlyph::Empty) => {}
            Some(GpuiGlyph::RunFragment { run, range }) => {
                let natural_offset = run.caret_stops[range.start as usize].min(run.caret_stops[range.end as usize]);
                let origin = Point::new(origin.x - f64::from(natural_offset), origin.y);
                self.push_text_fragment(GpuiTextRunFragment::new(run, range, origin, Some(color)));
            }
            Some(GpuiGlyph::Layout { .. }) | None => self.commands.push(PaintCommand::Glyph { glyph, origin, color }),
        }
    }
}

fn gpui_color(color: Color) -> Hsla {
    Hsla::from(rgba(u32::from_be_bytes([color.r, color.g, color.b, color.a])))
}

fn render_image(image: &PenikoImage, target_width: u32, target_height: u32, theme: Option<ThemeAdaptation>) -> Option<Arc<RenderImage>> {
    let mut buffer = raster_buffer(image.data.data(), image.width, image.height, target_width, target_height, theme)?;
    for pixel in buffer.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(SmallVec::from_vec(vec![Frame::new(buffer)]))))
}

fn scaled_image_cache_key(hash: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(hash.len() + 9);
    key.push(b'r');
    key.extend_from_slice(&width.to_le_bytes());
    key.extend_from_slice(&height.to_le_bytes());
    key.extend_from_slice(hash);
    key
}

fn render_svg(bytes: &[u8], _intrinsic_size: (u32, u32), target_width: u32, target_height: u32) -> Option<Arc<RenderImage>> {
    let bytes = svg_with_viewport(bytes, target_width, target_height)?;
    SvgRenderer::new(Arc::new(())).render_single_frame(&bytes, 1.0).ok()
}

fn svg_with_viewport(bytes: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut reader = Reader::from_reader(bytes);
    let mut writer = Writer::new(Vec::with_capacity(bytes.len() + 32));
    let mut root_rewritten = false;
    loop {
        let event = reader.read_event().ok()?;
        let event = match event {
            Event::Start(start) if !root_rewritten && start.local_name().as_ref() == b"svg" => {
                root_rewritten = true;
                Event::Start(svg_start_with_viewport(&start, width, height)?)
            }
            Event::Empty(start) if !root_rewritten && start.local_name().as_ref() == b"svg" => {
                root_rewritten = true;
                Event::Empty(svg_start_with_viewport(&start, width, height)?)
            }
            Event::Eof => break,
            event => event.into_owned(),
        };
        writer.write_event(event).ok()?;
    }
    root_rewritten.then(|| writer.into_inner())
}

fn svg_start_with_viewport(start: &BytesStart<'_>, width: u32, height: u32) -> Option<BytesStart<'static>> {
    let name = std::str::from_utf8(start.name().as_ref()).ok()?.to_owned();
    let mut rewritten = BytesStart::new(name);
    for attribute in start.attributes().with_checks(false) {
        let attribute = attribute.ok()?;
        if !matches!(attribute.key.local_name().as_ref(), b"width" | b"height") {
            rewritten.push_attribute(attribute);
        }
    }
    let width = width.to_string();
    let height = height.to_string();
    rewritten.push_attribute(("width", width.as_str()));
    rewritten.push_attribute(("height", height.as_str()));
    Some(rewritten.into_owned())
}

impl Painter for GpuiCommandPainter<'_> {
    fn push_clip(&mut self, rect: Rect) {
        self.commands.push(PaintCommand::PushClip { rect });
    }

    fn pop_clip(&mut self) {
        self.commands.push(PaintCommand::PopClip);
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.commands.push(PaintCommand::Fill { rect, color: gpui_color(color) });
    }

    fn fill_rounded_rect(&mut self, rect: Rect, radii: UsedBorderRadii, color: Color) {
        self.commands.push(PaintCommand::RoundedFill { rect, radii, color: gpui_color(color) });
    }

    fn stroke_rounded_rect(&mut self, rect: Rect, radii: UsedBorderRadii, width: f32, color: Color) {
        self.commands.push(PaintCommand::RoundedBorder { rect, radii, width, color: gpui_color(color) });
    }

    

    fn draw_glyph(&mut self, glyph: GlyphId, origin: Point, color: u32) {
        self.push_glyph(glyph, origin, self.text_color.unwrap_or_else(|| Hsla::from(rgba(color))));
    }

    fn supports_text_runs(&self) -> bool {
        true
    }

    fn draw_text_run(&mut self, run: TextRunId, origin: Point, color: Option<u32>) {
        self.draw_text_run_fragment(run, 0..u32::MAX, origin, color);
    }

    fn draw_text_run_fragment(&mut self, run: TextRunId, range: Range<u32>, origin: Point, color: Option<u32>) {
        let Some(run) = self.text_runs.get(run as usize).cloned() else {
            debug_assert!(false, "renderer referenced a missing shaped text run");
            return;
        };
        self.push_text_fragment(GpuiTextRunFragment::new(run, range, origin, self.text_color.or_else(|| color.map(|color| Hsla::from(rgba(color))))));
    }

    fn draw_text_run_batch(&mut self, fragments: &[TextRunFragment]) {
        let runs = &self.text_runs;
        let mut batch = Vec::with_capacity(fragments.len());
        for fragment in fragments {
            let Some(run) = runs.get(fragment.run() as usize).cloned() else {
                debug_assert!(false, "renderer referenced a missing shaped text run");
                continue;
            };
            batch.push(GpuiTextRunFragment::new(run, fragment.range(), fragment.origin(), self.text_color.or_else(|| fragment.color().map(|color| Hsla::from(rgba(color))))));
        }
        self.push_text_fragments(batch);
    }

    fn draw_prepared_text_run_batch(&mut self, fragments: &[PreparedTextRunFragment], line_origin: Point, scale: f64, color: Option<u32>) {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        for fragment in fragments {
            let Some(run) = self.text_runs.get(fragment.run() as usize).cloned() else {
                debug_assert!(false, "renderer referenced a missing shaped text run");
                continue;
            };
            let offset = fragment.offset();
            // Snap the shared baseline, rather than each run's top edge.
            // Fallback fonts can give adjacent runs different ascents.
            let baseline = ((line_origin.y + offset.y) * scale).round() / scale;
            let origin = Point::new(line_origin.x + offset.x, baseline);
            self.push_text_fragment(GpuiTextRunFragment::new(run, fragment.range(), origin, self.text_color.or_else(|| color.or(fragment.color()).map(|color| Hsla::from(rgba(color))))));
        }
    }

    fn draw_image(&mut self, image: &PenikoImage, hash: &[u8], rect: Rect) {
        let Some(images) = self.images.as_deref_mut() else {
            debug_assert!(false, "an image was emitted into a command-only paint layer");
            return;
        };
        let Some((target_width, target_height)) = raster_size(rect.width(), rect.height()) else { return };
        let theme = self.image_theme.filter(|_| images.raster_classifier.classify(hash, image.data.data(), image.width, image.height) == RasterKind::NeutralLineArt);
        let cache_key = raster_cache_key(hash, target_width, target_height, theme);
        let rendered = images.get(&cache_key).or_else(|| {
            let rendered = render_image(image, target_width, target_height, theme)?;
            images.insert(cache_key, rendered.clone());
            Some(rendered)
        });
        if let Some(image) = rendered {
            self.commands.push(PaintCommand::Image { rect, image });
        }
    }

    fn draw_math_svg(&mut self, bytes: &[u8], hash: &[u8], intrinsic_size: (u32, u32), rect: Rect, color: u32) {
        let previous_color = self.text_color;
        self.text_color = Some(previous_color.unwrap_or_else(|| Hsla::from(rgba(color))));
        self.draw_svg(bytes, hash, intrinsic_size, rect);
        self.text_color = previous_color;
    }

    fn draw_svg(&mut self, bytes: &[u8], hash: &[u8], intrinsic_size: (u32, u32), rect: Rect) {
        if !bytes.starts_with(b"<svg data-html-mathml=\"true\"")
            && let Some(theme) = self.image_theme
        {
            self.draw_themed_svg(bytes, hash, intrinsic_size, rect, theme);
            return;
        }
        let Some((target_width, target_height)) = raster_size(rect.width(), rect.height()) else { return };
        let mut cache_key = scaled_image_cache_key(hash, target_width, target_height);
        cache_key[0] = b's';
        let theme_color = self.text_color.filter(|_| bytes.starts_with(b"<svg data-html-mathml=\"true\""));
        if let Some(color) = theme_color {
            cache_key.extend_from_slice(&u32::from(color.to_rgb()).to_be_bytes());
        }
        let rasterize = || {
            let themed = theme_color.and_then(|color| themed_math_svg(bytes, u32::from(color.to_rgb())));
            render_svg(themed.as_deref().unwrap_or(bytes), intrinsic_size, target_width, target_height)
        };
        let rendered = match self.images.as_deref_mut() {
            Some(images) => images.get(&cache_key).or_else(|| {
                let rendered = rasterize()?;
                images.insert(cache_key, rendered.clone());
                Some(rendered)
            }),
            None => rasterize(),
        };
        if let Some(image) = rendered {
            self.commands.push(PaintCommand::Image { rect, image });
        }
    }
}

pub fn paint_commands(commands: &[PaintCommand], glyph_store: &GlyphStore, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let started = Instant::now();
    let glyphs = glyph_store;
    let profile_enabled = std::env::var_os("BOKHEIM_PROFILE_RESIZE").is_some() || std::env::var_os("GPUI_KOBO_PROFILE").is_some();
    let mut profile = PaintProfile::default();
    paint_commands_inner(commands, &glyphs, bounds, window, cx, &mut profile, profile_enabled);
    let duration_ms = started.elapsed().as_millis();
    if profile_enabled {
        println!(
            "HTML_GPUI_PAINT_DETAIL commands={} text_run_commands={} text_run_fragments={} standalone_glyph_commands={} glyphs={} text_run_us={} fragment_us={} layout_us={} fill_us={} image_us={} total_us={}",
            commands.len(),
            profile.text_run_commands,
            profile.text_run_fragments,
            profile.standalone_glyph_commands,
            profile.glyphs,
            profile.text_run_us,
            profile.fragment_us,
            profile.layout_us,
            profile.fill_us,
            profile.image_us,
            started.elapsed().as_micros(),
        );
    }
    if duration_ms >= 10 {
        println!("HTML_GPUI_SCENE commands={} duration_ms={duration_ms}", commands.len());
    }
}

#[derive(Default)]
struct PaintProfile {
    text_run_commands: usize,
    text_run_fragments: usize,
    standalone_glyph_commands: usize,
    glyphs: usize,
    text_run_us: u128,
    fragment_us: u128,
    layout_us: u128,
    fill_us: u128,
    image_us: u128,
}

fn paint_commands_inner(commands: &[PaintCommand], glyphs: &GlyphStorage, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App, profile: &mut PaintProfile, profile_enabled: bool) {
    let mut index = 0usize;
    while index < commands.len() {
        let command = &commands[index];
        match command {
            PaintCommand::PushClip { rect } => {
                let mut depth = 1usize;
                let mut end = index + 1;
                while end < commands.len() && depth > 0 {
                    match &commands[end] {
                        PaintCommand::PushClip { .. } => depth += 1,
                        PaintCommand::PopClip => depth -= 1,
                        _ => {}
                    }
                    end += 1;
                }
                debug_assert_eq!(depth, 0, "paint clip commands must be balanced");
                let inner_end = end.saturating_sub(1);
                let clip_bounds = Bounds { origin: point(bounds.origin.x + px(rect.x0 as f32), bounds.origin.y + px(rect.y0 as f32)), size: size(px(rect.width().max(0.0) as f32), px(rect.height().max(0.0) as f32)) };
                window.with_content_mask(Some(ContentMask { bounds: clip_bounds }), |window| paint_commands_inner(&commands[index + 1..inner_end], glyphs, bounds, window, cx, profile, profile_enabled));
                index = end;
                continue;
            }
            PaintCommand::PopClip => {
                debug_assert!(false, "unmatched paint clip pop");
            }
            PaintCommand::Fill { rect, color } => {
                let operation_started = profile_enabled.then(Instant::now);
                let rect = Bounds { origin: point(bounds.origin.x + px(rect.x0 as f32), bounds.origin.y + px(rect.y0 as f32)), size: size(px(rect.width() as f32), px(rect.height() as f32)) };
                window.paint_quad(fill(rect, *color));
                if let Some(started) = operation_started {
                    profile.fill_us += started.elapsed().as_micros();
                }
            }
            PaintCommand::RoundedFill { rect, radii, color } => {
                let operation_started = profile_enabled.then(Instant::now);
                let rect = Bounds { origin: point(bounds.origin.x + px(rect.x0 as f32), bounds.origin.y + px(rect.y0 as f32)), size: size(px(rect.width() as f32), px(rect.height() as f32)) };
                window.paint_quad(fill(rect, *color).corner_radii(gpui_radii(*radii)));
                if let Some(started) = operation_started {
                    profile.fill_us += started.elapsed().as_micros();
                }
            }
            PaintCommand::RoundedBorder { rect, radii, width, color } => {
                let operation_started = profile_enabled.then(Instant::now);
                let rect = Bounds { origin: point(bounds.origin.x + px(rect.x0 as f32), bounds.origin.y + px(rect.y0 as f32)), size: size(px(rect.width() as f32), px(rect.height() as f32)) };
                let widths = Edges { top: px(*width), right: px(*width), bottom: px(*width), left: px(*width) };
                window.paint_quad(fill(rect, rgba(0)).corner_radii(gpui_radii(*radii)).border_widths(widths).border_color(*color));
                if let Some(started) = operation_started {
                    profile.fill_us += started.elapsed().as_micros();
                }
            }
            PaintCommand::Glyph { glyph, origin, color } => {
                profile.standalone_glyph_commands += 1;
                let Some(glyph) = glyphs.get(*glyph as usize) else {
                    debug_assert!(false, "paint command referenced a missing interned glyph");
                    continue;
                };
                match glyph {
                    GpuiGlyph::Empty => {}
                    GpuiGlyph::Layout { layout } => {
                        let operation_started = profile_enabled.then(Instant::now);
                        let origin = point(bounds.origin.x + px(origin.x as f32), bounds.origin.y + px(origin.y as f32));
                        for run in &layout.runs {
                            profile.glyphs += run.glyphs.len();
                            let paint_glyphs = run.glyphs.iter().map(|shaped_glyph| PaintGlyph {
                                origin: point(origin.x + shaped_glyph.position.x, origin.y + shaped_glyph.position.y),
                                font_id: run.font_id,
                                glyph_id: shaped_glyph.id,
                                font_size: layout.font_size,
                                color: *color,
                                is_emoji: shaped_glyph.is_emoji,
                            });
                            if let Err(error) = window.paint_glyphs(paint_glyphs) {
                                eprintln!("failed to paint GPUI glyph batch: {error:#}");
                            }
                        }
                        if let Some(started) = operation_started {
                            profile.layout_us += started.elapsed().as_micros();
                        }
                    }
                    GpuiGlyph::RunFragment { run, range } => {
                        let operation_started = profile_enabled.then(Instant::now);
                        let natural_offset = run.caret_stops[range.start as usize].min(run.caret_stops[range.end as usize]);
                        let glyph_ranges = run.glyph_ranges(range.clone());
                        let baseline = point(bounds.origin.x + px(origin.x as f32), bounds.origin.y + px(origin.y as f32));
                        let selected = glyph_ranges.iter().flat_map(|range| run.glyphs[range.clone()].iter());
                        let mut glyph_count = 0usize;
                        let paint_glyphs = selected
                            .map(|shaped_glyph| PaintGlyph {
                                origin: point(baseline.x + shaped_glyph.position.x - px(natural_offset), baseline.y + shaped_glyph.position.y),
                                font_id: shaped_glyph.font_id,
                                glyph_id: shaped_glyph.glyph_id,
                                font_size: shaped_glyph.font_size,
                                color: *color,
                                is_emoji: shaped_glyph.is_emoji,
                            })
                            .inspect(|_| glyph_count += 1);
                        if let Err(error) = window.paint_glyphs(paint_glyphs) {
                            eprintln!("failed to paint GPUI glyph batch: {error:#}");
                        }
                        profile.glyphs += glyph_count;
                        if let Some(started) = operation_started {
                            profile.fragment_us += started.elapsed().as_micros();
                        }
                    }
                }
            }
            PaintCommand::TextBatch { fragments } => {
                profile.text_run_commands += 1;
                profile.text_run_fragments += fragments.len();
                let operation_started = profile_enabled.then(Instant::now);
                let mut glyph_count = 0usize;
                let visible_fragments = fragments
                    .iter()
                    .filter(|fragment| {
                        let Some(ink) = fragment.run.paint_bounds else { return true };
                        let origin = point(bounds.origin.x + px(fragment.origin.x as f32) + px(ink.x0 as f32),
                                           bounds.origin.y + px(fragment.origin.y as f32) + px(ink.y0 as f32));
                        window.is_bounds_visible(Bounds::new(origin, size(px(ink.width() as f32), px(ink.height() as f32))))
                    })
                    .collect::<Vec<_>>();
                let paint_glyphs = visible_fragments.into_iter()
                    .flat_map(|fragment| {
                        let baseline = point(bounds.origin.x + px(fragment.origin.x as f32), bounds.origin.y + px(fragment.origin.y as f32));
                        fragment.glyph_ranges.iter().flat_map(|range| fragment.run.glyphs[range.clone()].iter()).map(move |glyph| PaintGlyph {
                            origin: point(baseline.x + glyph.position.x, baseline.y + glyph.position.y),
                            font_id: glyph.font_id,
                            glyph_id: glyph.glyph_id,
                            font_size: glyph.font_size,
                            color: fragment.color.unwrap_or_else(|| fragment.run.color_at(glyph.byte_index)),
                            is_emoji: glyph.is_emoji,
                        })
                    })
                    .inspect(|_| glyph_count += 1);
                if let Err(error) = window.paint_glyphs(paint_glyphs) {
                    eprintln!("failed to paint GPUI glyph batch: {error:#}");
                }
                profile.glyphs += glyph_count;
                if let Some(started) = operation_started {
                    profile.text_run_us += started.elapsed().as_micros();
                }
            }
            PaintCommand::Image { rect, image } => {
                let operation_started = profile_enabled.then(Instant::now);
                track_image(image, cx);
                let image_bounds = Bounds { origin: point(bounds.origin.x + px(rect.x0 as f32), bounds.origin.y + px(rect.y0 as f32)), size: size(px(rect.width() as f32), px(rect.height() as f32)) };
                let _ = window.paint_image(image_bounds, image_bounds, Corners::default(), image.clone(), 0, false);
                if let Some(started) = operation_started {
                    profile.image_us += started.elapsed().as_micros();
                }
            }
        }
        index += 1;
    }
}

fn gpui_radii(radii: UsedBorderRadii) -> Corners<Pixels> {
    // GPUI's quad primitive has circular rather than elliptical corners. The
    // smaller axis stays inside the CSS ellipse and avoids overpainting.
    Corners {
        top_left: px(radii.top_left.0.min(radii.top_left.1)),
        top_right: px(radii.top_right.0.min(radii.top_right.1)),
        bottom_right: px(radii.bottom_right.0.min(radii.bottom_right.1)),
        bottom_left: px(radii.bottom_left.0.min(radii.bottom_left.1)),
    }
}
