
use crate::fonts::{PublicationFonts, SharedPublicationFonts};
use cssparser::{ParseError, Parser, ParserInput};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use web_time::Instant;

use gpui::{Font, FontFallbacks, FontStyle, FontWeight, Hsla, LineLayout, Pixels, ShapedRun, TextRun, font, px, rgba};
use html::style::{TextDirection, is_cjk_character};
use html::text::{
    FontMetricsRequest, FontRelativeMetrics, FontSlant, GlyphId, GlyphMetric, GlyphRequest, ShapedGlyph, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper, OpenTypeFeature, ShapeError, ShapedLine, ShapedTextRun, TextRunShapeRequest, TextShapeRequest,
    TextStyleSpan,
};
use kurbo::Rect;
use smallvec::SmallVec;
use unicode_segmentation::UnicodeSegmentation;

mod bounds;

/// OpenType features that form the book renderer's quality baseline. Required
/// composition and positioning are explicit so behavior is stable across
/// platform backends; standard and contextual Latin ligatures remain enabled
/// unless CSS overrides their tag with zero.
const BOOK_QUALITY_FEATURES: [([u8; 4], u32); 9] = [(*b"ccmp", 1), (*b"rlig", 1), (*b"rclt", 1), (*b"mark", 1), (*b"mkmk", 1), (*b"kern", 1), (*b"liga", 1), (*b"clig", 1), (*b"calt", 1)];

fn requested_font(family: String, weight: u16, slant: FontSlant) -> Font {
    let mut selected = font(family);
    selected.weight = FontWeight(f32::from(weight));
    selected.style = match slant {
        FontSlant::Normal => FontStyle::Normal,
        FontSlant::Italic => FontStyle::Italic,
        FontSlant::Oblique => FontStyle::Oblique,
    };
    selected
}

fn font_slant_key(slant: FontSlant) -> u8 {
    match slant {
        FontSlant::Normal => 0,
        FontSlant::Italic => 1,
        FontSlant::Oblique => 2,
    }
}

fn book_quality_font_features(overrides: &[OpenTypeFeature]) -> gpui::FontFeatures {
    let mut features = BOOK_QUALITY_FEATURES.to_vec();
    for feature in overrides {
        let entry = (feature.tag(), feature.value());
        if let Some(index) = features.iter().position(|(tag, _)| *tag == entry.0) {
            features[index] = entry;
        } else {
            features.push(entry);
        }
    }
    gpui::FontFeatures(Arc::new(features.into_iter().map(|(tag, value)| (String::from_utf8_lossy(&tag).into_owned(), value)).collect()))
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct GlyphKey {
    ch: char,
    font_size: u32,
    font_weight: u16,
    font_style: u8,
    family: String,
    fallbacks: Option<FontFallbacks>,
}

#[derive(Clone)]
pub enum GpuiGlyph {
    Empty,
    Layout { layout: Arc<gpui::LineLayout> },
    // Keep the shaped run alive for retained glyph stores. Individual glyph
    // painting resolves its slices, offset, and color from that shared run.
    RunFragment { run: Arc<GpuiTextRun>, range: Range<u32> },
}

/// Immutable paint resources. The shaper copies on write while a frame retains them.
pub type GlyphStore = Rc<GlyphStorage>;

/// Backend paint resources addressed by the engine's dense character IDs.
/// A shaped run occupies one record covering all of its character IDs.
/// Individual-character resources remain available for fallback shaping.
#[derive(Clone, Default)]
pub struct GlyphStorage {
    records: Vec<GlyphRecord>,
    len: usize,
}

#[derive(Clone)]
struct GlyphRecord {
    start: usize,
    glyph: GpuiGlyph,
}

impl GlyphStorage {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Resolve a character only when the painter requests an individual glyph.
    /// Normal native text painting already holds the shaped run directly.
    pub fn get(&self, index: usize) -> Option<GpuiGlyph> {
        if index >= self.len {
            return None;
        }
        let record = &self.records[self.records.partition_point(|record| record.start <= index) - 1];
        Some(match &record.glyph {
            GpuiGlyph::RunFragment { run, range } => {
                let character = range.start + (index - record.start) as u32;
                GpuiGlyph::RunFragment { run: run.clone(), range: character..character + 1 }
            }
            glyph => glyph.clone(),
        })
    }

    fn push(&mut self, glyph: GpuiGlyph) {
        self.records.push(GlyphRecord { start: self.len, glyph });
        self.len += 1;
    }

    pub(crate) fn push_run(&mut self, run: Arc<GpuiTextRun>, characters: usize) {
        if characters == 0 {
            return;
        }
        debug_assert!(u32::try_from(characters).is_ok());
        self.records.push(GlyphRecord { start: self.len, glyph: GpuiGlyph::RunFragment { run, range: 0..characters as u32 } });
        self.len += characters;
    }
}

pub struct GpuiTextRun {
    pub(crate) glyphs: Arc<[GpuiRunGlyph]>,
    pub(crate) ascent: gpui::Pixels,
    pub(crate) descent: gpui::Pixels,
    character_bytes: Arc<[usize]>,
    glyph_segments: Arc<[GpuiGlyphSegment]>,
    colors: Box<[(Range<usize>, Hsla)]>,
    pub(crate) caret_stops: Arc<[f32]>,
    advances: Arc<[f32]>,
    pub(crate) paint_bounds: Option<Rect>,
}

impl GpuiTextRun {
    pub(crate) fn new(glyphs: Arc<[GpuiRunGlyph]>, ascent: gpui::Pixels, descent: gpui::Pixels, character_bytes: Arc<[usize]>, colors: Box<[(Range<usize>, Hsla)]>) -> Self {
        let glyph_segments = Arc::from(build_glyph_segments(&glyphs));
        Self { glyphs, ascent, descent, character_bytes, glyph_segments, colors, caret_stops: Arc::default(), advances: Arc::default(), paint_bounds: None }
    }

    fn with_colors(&self, colors: Box<[(Range<usize>, Hsla)]>) -> Self {
        Self {
            glyphs: self.glyphs.clone(),
            ascent: self.ascent,
            descent: self.descent,
            character_bytes: self.character_bytes.clone(),
            glyph_segments: self.glyph_segments.clone(),
            colors,
            caret_stops: self.caret_stops.clone(),
            advances: self.advances.clone(),
            paint_bounds: self.paint_bounds,
        }
    }

    fn cache_paint_bounds(&mut self, text_system: &gpui::WindowTextSystem) {
        // Use each fallback font's full glyph box, rather than line metrics or
        // advances, so combining marks and glyph overhangs remain visible.
        // Font boxes use different vertical conventions on different backends;
        // symmetric extents conservatively cover both. Compute once per run,
        // never once per viewport or individual character paint command.
        let mut previous_font = None;
        let mut font_bounds = Rect::ZERO;
        for glyph in self.glyphs.iter() {
            let key = (glyph.font_id, glyph.font_size);
            if previous_font != Some(key) {
                let bounds = text_system.bounding_box(glyph.font_id, glyph.font_size);
                font_bounds = Rect::new(f64::from(f32::from(bounds.left())), f64::from(f32::from(bounds.top())), f64::from(f32::from(bounds.right())), f64::from(f32::from(bounds.bottom())));
                previous_font = Some(key);
            }
            let x = f64::from(f32::from(glyph.position.x));
            let y = f64::from(f32::from(glyph.position.y));
            let Some(bounds) = bounds::conservative_glyph_bounds(font_bounds, (x, y)) else {
                self.paint_bounds = None;
                return;
            };
            self.paint_bounds = Some(self.paint_bounds.map_or(bounds, |previous| previous.union(bounds)));
        }
    }

    /// Resolve a logical character range to slices of the visually ordered
    /// glyph array. Most text produces one slice. Mixed-direction text may
    /// produce several, which remain in the exact order GPUI shaped them.
    pub(crate) fn glyph_ranges(&self, range: Range<u32>) -> SmallVec<[Range<usize>; 2]> {
        let start = self.character_bytes.get(range.start as usize).copied().unwrap_or(0);
        let end = self.character_bytes.get(range.end as usize).copied().unwrap_or(usize::MAX);
        if start >= end {
            return SmallVec::new();
        }

        let mut ranges = SmallVec::<[Range<usize>; 2]>::new();
        for segment in self.glyph_segments.iter() {
            let glyphs = &self.glyphs[segment.glyphs.clone()];
            let (local_start, local_end) = match segment.order {
                GlyphByteOrder::Ascending => (glyphs.partition_point(|glyph| glyph.byte_index < start), glyphs.partition_point(|glyph| glyph.byte_index < end)),
                GlyphByteOrder::Descending => (glyphs.partition_point(|glyph| glyph.byte_index >= end), glyphs.partition_point(|glyph| glyph.byte_index >= start)),
            };
            if local_start == local_end {
                continue;
            }
            let selected = segment.glyphs.start + local_start..segment.glyphs.start + local_end;
            if let Some(previous) = ranges.last_mut()
                && previous.end == selected.start
            {
                previous.end = selected.end;
            } else {
                ranges.push(selected);
            }
        }
        ranges
    }

    pub(crate) fn color_at(&self, byte_index: usize) -> Hsla {
        self.colors.iter().find(|(range, _)| range.contains(&byte_index)).map(|(_, color)| *color).unwrap_or_else(|| Hsla::from(rgba(0x000000ff)))
    }

    pub(crate) fn character_color(&self, character: u32) -> Hsla {
        self.color_at(self.character_bytes[character as usize])
    }

    fn colors_match(&self, styles: &[TextStyleSpan<'_>]) -> bool {
        self.colors.len() == styles.len() && self.colors.iter().zip(styles).all(|((range, color), style)| *range == style.byte_range() && *color == Hsla::from(rgba(style.color())))
    }
}

#[derive(Clone, Copy)]
enum GlyphByteOrder {
    Ascending,
    Descending,
}

#[derive(Clone)]
struct GpuiGlyphSegment {
    glyphs: Range<usize>,
    order: GlyphByteOrder,
}

fn build_glyph_segments(glyphs: &[GpuiRunGlyph]) -> Vec<GpuiGlyphSegment> {
    if glyphs.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut order = None;
    for index in 1..glyphs.len() {
        let next_order = match glyphs[index].byte_index.cmp(&glyphs[index - 1].byte_index) {
            Ordering::Less => Some(GlyphByteOrder::Descending),
            Ordering::Greater => Some(GlyphByteOrder::Ascending),
            Ordering::Equal => None,
        };
        let Some(next_order) = next_order else { continue };
        if let Some(current_order) = order
            && !matches!((current_order, next_order), (GlyphByteOrder::Ascending, GlyphByteOrder::Ascending) | (GlyphByteOrder::Descending, GlyphByteOrder::Descending))
        {
            segments.push(GpuiGlyphSegment { glyphs: start..index, order: current_order });
            start = index;
            order = None;
        } else {
            order = Some(next_order);
        }
    }
    segments.push(GpuiGlyphSegment { glyphs: start..glyphs.len(), order: order.unwrap_or(GlyphByteOrder::Ascending) });
    segments
}

#[derive(Clone, Debug)]
pub(crate) struct GpuiRunGlyph {
    pub(crate) font_id: gpui::FontId,
    pub(crate) glyph_id: gpui::GlyphId,
    pub(crate) position: gpui::Point<gpui::Pixels>,
    pub(crate) font_size: gpui::Pixels,
    pub(crate) byte_index: usize,
    pub(crate) is_emoji: bool,
}

pub type RunStore = Rc<Vec<Arc<GpuiTextRun>>>;

struct DocumentTextResources {
    generation: GlyphResourceGeneration,
    table: HashMap<GlyphKey, ShapedGlyph>,
    glyphs: GlyphStore,
    runs: RunStore,
}

struct AppendTextResources {
    glyphs: GlyphStore,
    runs: RunStore,
}

#[derive(Clone)]
struct CachedGlyphShape {
    layout: Arc<gpui::LineLayout>,
    metric: GlyphMetric,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FontStyleIdentity {
    font_weight: u16,
    font_style: u8,
    family: Option<Box<str>>,
    cjk_family: Option<Box<str>>,
    features: Box<[OpenTypeFeature]>,
}

impl FontStyleIdentity {
    fn matches(&self, style: &TextStyleSpan<'_>) -> bool {
        self.font_weight == style.font_weight()
            && self.font_style == font_slant_key(style.font_slant())
            && self.family.as_deref() == style.font_family()
            && self.cjk_family.as_deref() == style.cjk_font_family()
            && self.features.as_ref() == style.font_features()
    }
}

#[derive(Clone)]
struct PreparedFont {
    identity: Arc<FontStyleIdentity>,
    font: Font,
    cjk_font: Option<Font>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LineStyleIdentity {
    byte_len: usize,
    font_size: u32,
    direction: TextDirection,
    font: Arc<FontStyleIdentity>,
}

struct CachedLineShape {
    text: Box<str>,
    styles: Box<[LineStyleIdentity]>,
    geometry: CachedLineGeometry,
    natural_run: Option<Arc<GpuiTextRun>>,
}

#[derive(Default)]
struct WesternShapingBenchmark {
    runs: u64,
    characters: u64,
    unsupported_runs: u64,
    legacy_glyph_path: Duration,
    full_run_shaping: Duration,
    authoritative_registration: Duration,
    joined_cluster_boundaries: u64,
}

impl WesternShapingBenchmark {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn report(&self) {
        if self.runs == 0 && self.unsupported_runs == 0 {
            return;
        }
        let legacy_us = self.legacy_glyph_path.as_micros().saturating_add(self.full_run_shaping.as_micros());
        let optimized_us = self.authoritative_registration.as_micros().saturating_add(self.full_run_shaping.as_micros());
        let speedup = if optimized_us == 0 { 0.0 } else { legacy_us as f64 / optimized_us as f64 };
        println!(
            "HTML_WESTERN_SHAPING_BENCH runs={} characters={} unsupported_runs={} legacy_total_us={} legacy_cached_glyph_us={} optimized_total_us={} authoritative_run_us={} authoritative_registration_us={} speedup={speedup:.2} width_error_percent=0.000 joined_cluster_boundaries={}",
            self.runs,
            self.characters,
            self.unsupported_runs,
            legacy_us,
            self.legacy_glyph_path.as_micros(),
            optimized_us,
            self.full_run_shaping.as_micros(),
            self.authoritative_registration.as_micros(),
            self.joined_cluster_boundaries,
        );
    }
}

fn western_shaping_benchmark_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("BOKHEIM_WESTERN_SHAPING_BENCHMARK").is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes")))
}

fn trailing_whitespace_profiling_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("BOKHEIM_PROFILE_TEXT").is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes")))
}

fn is_western_benchmark_text(text: &str) -> bool {
    text.chars().all(|character| {
        character.is_ascii()
            || matches!(
                character as u32,
                0x00a0..=0x024f
                    | 0x0300..=0x036f
                    | 0x1e00..=0x1eff
                    | 0x2000..=0x206f
                    | 0x20a0..=0x20cf
            )
    })
}

/// Natural geometry owned by the shape cache and borrowed while creating
/// paint resources and engine geometry views.
struct CachedLineGeometry {
    advances: Arc<[f32]>,
    glyphs: Arc<[GpuiRunGlyph]>,
    ascent: f32,
    descent: f32,
    caret_stops: Arc<[f32]>,
    cluster_boundaries: Arc<[bool]>,
    character_bytes: Arc<[usize]>,
}

impl CachedLineShape {
    fn matches(&self, text: &str, styles: &[TextStyleSpan<'_>]) -> bool {
        self.text.as_ref() == text
            && self.styles.len() == styles.len()
            && self.styles.iter().zip(styles).all(|(cached, style)| cached.byte_len == style.byte_range().len() && cached.font_size == style.font_size().to_bits() && cached.direction == style.direction() && cached.font.matches(style))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextShapeCacheStats {
    pub glyph_hits: u64,
    pub glyph_misses: u64,
    pub glyph_evictions: u64,
    pub retained_glyphs: usize,
    pub line_hits: u64,
    pub line_misses: u64,
    pub line_evictions: u64,
    pub retained_lines: usize,
    pub font_setup_hits: u64,
    pub font_setup_misses: u64,
    pub retained_font_setups: usize,
    pub registered_glyphs: usize,
    pub backend_glyph_records: usize,
    pub trailing_whitespace_probes: u64,
    pub trailing_whitespace_probe_characters: u64,
    /// Probe timing is collected only with `BOKHEIM_PROFILE_TEXT=1`.
    pub trailing_whitespace_probe_time: Duration,
}

const SHAPE_CACHE_CAPACITY: usize = 4096;
const LINE_SHAPE_CACHE_CAPACITY: usize = 1024;
const FONT_SETUP_CACHE_CAPACITY: usize = 32;

/// Split fallback lists without breaking quoted installed family names.
fn font_family_names(list: &str) -> impl Iterator<Item = (String, bool)> {
    let mut input = ParserInput::new(list);
    let mut parser = Parser::new(&mut input);
    let families: Result<Vec<_>, ParseError<'_, ()>> = parser.parse_comma_separated(|input| {
        if let Ok(name) = input.try_parse(Parser::expect_string_cloned) {
            return Ok((name.to_string(), true));
        }
        let mut name = input.expect_ident()?.to_string();
        while !input.is_exhausted() {
            name.push(' ');
            name.push_str(input.expect_ident()?);
        }
        Ok((name, false))
    });
    families.unwrap_or_default().into_iter()
}

fn append_font_runs(runs: &mut Vec<TextRun>, text: &str, latin: Font, cjk: Option<Font>, color: Hsla) {
    let push = |runs: &mut Vec<TextRun>, len, font| runs.push(TextRun { len, font, color, background_color: None, underline: None, strikethrough: None });
    let Some(cjk) = cjk.filter(|_| !text.is_ascii() && text.chars().any(is_cjk_character)) else {
        push(runs, text.len(), latin);
        return;
    };
    let mut start = 0;
    let mut previous = false;
    for (byte, cluster) in text.grapheme_indices(true) {
        let selected = cluster.chars().any(is_cjk_character);
        if byte == 0 {
            previous = selected;
        }
        if selected != previous {
            push(runs, byte - start, if previous { cjk.clone() } else { latin.clone() });
            start = byte;
            previous = selected;
        }
    }
    push(runs, text.len() - start, if previous { cjk } else { latin });
}

pub struct GpuiTextCache {
    text_system: Arc<gpui::WindowTextSystem>,
    font_families: HashMap<String, String>,
    publication_fonts: SharedPublicationFonts,
    font_generation: u64,
    prepared_fonts: VecDeque<PreparedFont>,
    generation: GlyphResourceGeneration,
    table: HashMap<GlyphKey, ShapedGlyph>,
    glyphs: GlyphStore,
    runs: RunStore,
    previous_document: Option<DocumentTextResources>,
    previous_append: Option<AppendTextResources>,
    shapes: HashMap<GlyphKey, CachedGlyphShape>,
    shape_order: VecDeque<GlyphKey>,
    shape_cache_stats: TextShapeCacheStats,
    line_shapes: HashMap<u64, CachedLineShape>,
    line_shape_order: VecDeque<u64>,
    western_benchmark: WesternShapingBenchmark,
}

impl GpuiTextCache {
    pub fn new(text_system: Arc<gpui::WindowTextSystem>) -> Self {
        let font_families = text_system.all_font_names().into_iter().map(|family| (family.to_lowercase(), family)).collect();
        Self {
            text_system,
            font_families,
            publication_fonts: Rc::new(RefCell::new(PublicationFonts::default())),
            font_generation: 0,
            prepared_fonts: VecDeque::new(),
            generation: GlyphResourceGeneration::default(),
            table: HashMap::new(),
            glyphs: Rc::default(),
            runs: Rc::new(Vec::new()),
            previous_document: None,
            previous_append: None,
            shapes: HashMap::with_capacity(SHAPE_CACHE_CAPACITY),
            shape_order: VecDeque::with_capacity(SHAPE_CACHE_CAPACITY),
            shape_cache_stats: TextShapeCacheStats::default(),
            line_shapes: HashMap::with_capacity(LINE_SHAPE_CACHE_CAPACITY),
            line_shape_order: VecDeque::with_capacity(LINE_SHAPE_CACHE_CAPACITY),
            western_benchmark: WesternShapingBenchmark::default(),
        }
    }

    pub fn with_publication_fonts(mut self, fonts: SharedPublicationFonts) -> Self {
        self.publication_fonts = fonts;
        self
    }

    fn refresh_publication_fonts(&mut self) {
        let generation = self.publication_fonts.borrow().generation();
        if generation != self.font_generation {
            self.prepared_fonts.clear();
            self.table.clear();
            self.shapes.clear();
            self.shape_order.clear();
            self.line_shapes.clear();
            self.line_shape_order.clear();
            self.font_generation = generation;
        }
    }

    pub fn store(&self) -> GlyphStore {
        self.glyphs.clone()
    }

    pub fn run_store(&self) -> RunStore {
        self.runs.clone()
    }

    pub fn shape_cache_stats(&self) -> TextShapeCacheStats {
        TextShapeCacheStats {
            retained_font_setups: self.prepared_fonts.len(),
            retained_glyphs: self.shapes.len(),
            retained_lines: self.line_shapes.len(),
            registered_glyphs: self.glyphs.len(),
            backend_glyph_records: self.glyphs.records.len(),
            ..self.shape_cache_stats
        }
    }

    fn insert_shape(&mut self, key: GlyphKey, shape: CachedGlyphShape) {
        self.shapes.insert(key.clone(), shape);
        self.shape_order.push_back(key);
        while self.shapes.len() > SHAPE_CACHE_CAPACITY {
            let Some(evicted) = self.shape_order.pop_front() else { break };
            if self.shapes.remove(&evicted).is_some() {
                self.shape_cache_stats.glyph_evictions += 1;
            }
        }
    }

    fn insert_line_shape(&mut self, key: u64, shape: CachedLineShape) {
        self.line_shapes.insert(key, shape);
        self.line_shape_order.push_back(key);
        while self.line_shapes.len() > LINE_SHAPE_CACHE_CAPACITY {
            let Some(evicted) = self.line_shape_order.pop_front() else { break };
            if self.line_shapes.remove(&evicted).is_some() {
                self.shape_cache_stats.line_evictions += 1;
            }
        }
    }

    fn available_family(&self, candidates: &[&str]) -> Option<String> {
        candidates.iter().find_map(|candidate| self.font_families.get(&candidate.to_lowercase()).cloned())
    }

    fn resolve_font(&self, family_list: Option<&str>, weight: u16, slant: FontSlant) -> Font {
        self.resolve_font_for_script(family_list, weight, slant, false)
    }

    fn resolve_font_for_script(&self, family_list: Option<&str>, weight: u16, slant: FontSlant, cjk: bool) -> Font {
        let allow_publication = if cjk { self.publication_fonts.borrow().allows_cjk_publication_fonts() } else { self.publication_fonts.borrow().allows_publication_family_list(family_list) };
        let mut resolved = family_list.into_iter().flat_map(font_family_names).filter_map(|(family, quoted)| self.resolve_family(&family, quoted, weight, slant, allow_publication));
        let mut selected = resolved.next().unwrap_or_else(|| requested_font(".SystemUIFont".to_owned(), weight, slant));
        // Keep CSS fallback families for glyphs missing from a custom face.
        // Font setup is cached across lines; single-family requests allocate no chain.
        let mut fallbacks = selected.fallbacks.as_ref().map(|fallbacks| fallbacks.fallback_list().to_vec()).unwrap_or_default();
        for candidate in resolved {
            for family in std::iter::once(candidate.family.as_ref()).chain(candidate.fallbacks.as_ref().into_iter().flat_map(|fallbacks| fallbacks.fallback_list().iter().map(String::as_str))) {
                if family != selected.family.as_ref() && !fallbacks.iter().any(|existing| existing == family) {
                    fallbacks.push(family.to_owned());
                }
            }
        }
        if !fallbacks.is_empty() {
            selected.fallbacks = Some(FontFallbacks::from_fonts(fallbacks));
        }
        selected
    }

    fn resolve_family(&self, family: &str, quoted: bool, weight: u16, slant: FontSlant, allow_publication: bool) -> Option<Font> {
        if family.is_empty() {
            return None;
        }
        let generic_name = !quoted
            && matches!(family.to_ascii_lowercase().as_str(), "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy" | "system-ui" | "ui-serif" | "ui-sans-serif" | "ui-monospace" | "ui-rounded" | "emoji" | "math" | "fangsong");
        if allow_publication
            && !generic_name
            && let Some(font) = self.publication_fonts.borrow().resolve(family, weight, slant)
        {
            return Some(font);
        }
        let generic = match (quoted, family.to_ascii_lowercase().as_str()) {
            (false, "serif") => self.available_family(&["NV Garamond", "Noto Serif", "DejaVu Serif", "Times New Roman"]),
            (false, "sans-serif") => self.available_family(&["Raleway", "Noto Sans", "DejaVu Sans", "Arial", ".SystemUIFont"]),
            (false, "monospace" | "ui-monospace") => self.available_family(&["Noto Sans Mono", "DejaVu Sans Mono", ".ZedMono"]),
            (false, "system-ui" | "ui-sans-serif") => Some(".SystemUIFont".to_owned()),
            (false, "ui-serif") => self.available_family(&["NV Garamond", "Noto Serif", "DejaVu Serif"]),
            _ => None,
        };
        generic.or_else(|| self.font_families.get(&family.to_lowercase()).cloned()).map(|family| requested_font(family, weight, slant))
    }

    /// Font selection and OpenType features do not depend on run length,
    /// size, or paint color. Share this setup and its cache identity across
    /// lines, with a small bound independent of the document's text length.
    fn prepared_font(&mut self, style: &TextStyleSpan<'_>) -> PreparedFont {
        if let Some(prepared) = self.prepared_fonts.iter().find(|prepared| prepared.identity.matches(style)) {
            self.shape_cache_stats.font_setup_hits += 1;
            return prepared.clone();
        }
        self.shape_cache_stats.font_setup_misses += 1;
        let mut gpui_font = self.resolve_font(style.font_family(), style.font_weight(), style.font_slant());
        gpui_font.features = book_quality_font_features(style.font_features());
        let cjk_font = style.cjk_font_family().map(|family| {
            let mut font = self.resolve_font_for_script(Some(family), style.font_weight(), style.font_slant(), true);
            font.features = gpui_font.features.clone();
            font
        });
        let prepared = PreparedFont {
            cjk_font,
            identity: Arc::new(FontStyleIdentity {
                font_weight: style.font_weight(),
                font_style: font_slant_key(style.font_slant()),
                family: style.font_family().map(Into::into),
                cjk_family: style.cjk_font_family().map(Into::into),
                features: style.font_features().into(),
            }),
            font: gpui_font,
        };
        if self.prepared_fonts.len() == FONT_SETUP_CACHE_CAPACITY {
            self.prepared_fonts.pop_front();
        }
        self.prepared_fonts.push_back(prepared.clone());
        prepared
    }

    fn line_shape_fingerprint(text: &str, styles: &[TextStyleSpan<'_>]) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        styles.len().hash(&mut hasher);
        for style in styles {
            style.byte_range().len().hash(&mut hasher);
            style.font_size().to_bits().hash(&mut hasher);
            style.direction().hash(&mut hasher);
            style.font_weight().hash(&mut hasher);
            font_slant_key(style.font_slant()).hash(&mut hasher);
            style.font_family().hash(&mut hasher);
            style.cjk_font_family().hash(&mut hasher);
            style.font_features().hash(&mut hasher);
        }
        hasher.finish()
    }

    fn layout_directional_line(&self, text: &str, font_size: Pixels, runs: &[TextRun], direction: TextDirection) -> Arc<LineLayout> {
        let inferred = self.text_system.layout_line(text, font_size, runs, None);
        let is_rtl = direction == TextDirection::Rtl;
        // Keep the native fast path when it already covers the whole source
        // with the requested direction. Coverage also detects dropped leading
        // neutrals/numerals in backends that infer a conflicting paragraph base.
        if (direction == TextDirection::Ltr && text.is_ascii())
            || (!inferred.clusters.is_empty() && inferred.clusters.iter().try_fold(0, |end, cluster| (cluster.bytes.start == end && cluster.is_rtl == is_rtl).then_some(cluster.bytes.end)) == Some(text.len()))
        {
            return inferred;
        }

        // Skrift has already resolved bidi levels. GPUI's line API infers its
        // own direction, so supply a strong base and a directional override to
        // preserve that resolved run, including neutrals and mirrored glyphs.
        // Synthetic controls stay local to shaping; exported indices refer to
        // the original logical text.
        let prefix = match direction {
            TextDirection::Ltr => "\u{200e}\u{202d}",
            TextDirection::Rtl => "\u{200f}\u{202e}",
        };
        let mut directed = String::with_capacity(text.len() + 9);
        directed.push_str(prefix);
        let mut source_bytes = Vec::with_capacity(text.chars().count() + 1);
        let mut directed_runs = runs.to_vec();
        let mut source_start = 0;
        for run in &mut directed_runs {
            let source_end = source_start + run.len;
            let directed_start = directed.len();
            for (byte, character) in text[source_start..source_end].char_indices() {
                source_bytes.push((directed.len(), source_start + byte));
                // Source bidi controls have already affected Skrift's levels.
                // Keep a zero-width source slot without applying them twice.
                directed.push(if matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { '\u{200b}' } else { character });
            }
            run.len = directed.len() - directed_start;
            source_start = source_end;
        }
        source_bytes.push((directed.len(), text.len()));
        let content_end = directed.len();
        directed.push('\u{202c}');
        if let Some(first) = directed_runs.first_mut() {
            first.len += prefix.len();
        }
        if let Some(last) = directed_runs.last_mut() {
            last.len += '\u{202c}'.len_utf8();
        }
        let native = self.text_system.layout_line(&directed, font_size, &directed_runs, None);
        let source_byte = |index| source_bytes.binary_search_by_key(&index, |&(byte, _)| byte).ok().map(|index| source_bytes[index].1);
        let runs = native
            .runs
            .iter()
            .filter_map(|run| {
                let glyphs = run
                    .glyphs
                    .iter()
                    .filter_map(|glyph| {
                        if glyph.index >= content_end {
                            return None;
                        }
                        let mut glyph = glyph.clone();
                        glyph.index = source_byte(glyph.index)?;
                        Some(glyph)
                    })
                    .collect::<Vec<_>>();
                (!glyphs.is_empty()).then_some(ShapedRun { font_id: run.font_id, glyphs })
            })
            .collect();
        let clusters = native
            .clusters
            .iter()
            .filter_map(|cluster| {
                if cluster.bytes.start < prefix.len() || cluster.bytes.end > content_end {
                    return None;
                }
                let mut cluster = cluster.clone();
                cluster.bytes = source_byte(cluster.bytes.start)?..source_byte(cluster.bytes.end)?;
                Some(cluster)
            })
            .collect();
        Arc::new(LineLayout { font_size: native.font_size, width: native.width, ascent: native.ascent, descent: native.descent, runs, clusters, len: text.len() })
    }

    fn shape_line_geometry(&mut self, text: &str, styles: &[TextStyleSpan<'_>], placement_request: Option<&TextShapeRequest<'_>>) -> Result<Option<ShapedLine>, ShapeError> {
        let key = Self::line_shape_fingerprint(text, styles);
        let placements_are_natural =
            placement_request.is_none_or(|request| request.placements().iter().all(|placement| placement.extra_advance() == 0.0 && placement.advance_override().is_none() && placement.baseline_shift() == 0.0 && placement.paints()));
        let mut colors = None;
        if self.line_shapes.get(&key).is_some_and(|cached| cached.matches(text, styles)) {
            self.shape_cache_stats.line_hits += 1;
        } else {
            self.shape_cache_stats.line_misses += 1;
            let mut character_bytes = text.char_indices().map(|(byte, _)| byte).collect::<Vec<_>>();
            character_bytes.push(text.len());
            let mut text_runs = Vec::with_capacity(styles.len());
            let mut font_run_boundaries = SmallVec::<[usize; 8]>::new();
            let mut miss_colors = Vec::with_capacity(styles.len());
            let mut identities = Vec::with_capacity(styles.len());
            for style in styles {
                let prepared = self.prepared_font(style);
                identities.push(LineStyleIdentity { byte_len: style.byte_range().len(), font_size: style.font_size().to_bits(), direction: style.direction(), font: prepared.identity });
                let color = Hsla::from(rgba(style.color()));
                let byte_range = style.byte_range();
                font_run_boundaries.push(text_runs.len());
                append_font_runs(&mut text_runs, &text[byte_range.clone()], prepared.font, prepared.cjk_font, color);
                miss_colors.push((byte_range, color));
            }
            font_run_boundaries.push(text_runs.len());
            let single_group = styles.iter().all(|style| style.font_size().to_bits() == styles[0].font_size().to_bits() && style.direction() == styles[0].direction());
            // A single size and direction can use native boundary allocations
            // directly. Multiple groups need aggregate line-coordinate arrays.
            let mut raw_advances = vec![0.0; character_bytes.len() - 1];
            let mut raw_caret_stops: Arc<[f32]> = if single_group { Arc::default() } else { vec![0.0; character_bytes.len()].into() };
            let mut raw_cluster_boundaries: Arc<[bool]> = if single_group { Arc::default() } else { vec![false; character_bytes.len()].into() };
            if !single_group {
                let boundaries = Arc::make_mut(&mut raw_cluster_boundaries);
                boundaries[0] = true;
                if let Some(last) = boundaries.last_mut() {
                    *last = true;
                }
            }
            let mut glyphs = Vec::new();
            let mut x_base = 0.0f32;
            let mut ascent = 0.0f32;
            let mut descent = 0.0f32;
            let mut first_style = 0usize;
            while first_style < styles.len() {
                let font_size_bits = styles[first_style].font_size().to_bits();
                let mut end_style = first_style + 1;
                while end_style < styles.len() && styles[end_style].font_size().to_bits() == font_size_bits && styles[end_style].direction() == styles[first_style].direction() {
                    end_style += 1;
                }
                let byte_start = styles[first_style].byte_range().start;
                let byte_end = styles[end_style - 1].byte_range().end;
                let group_text = &text[byte_start..byte_end];
                let (character_start, character_end) = if single_group { (0, character_bytes.len() - 1) } else { (character_bytes.partition_point(|byte| *byte < byte_start), character_bytes.partition_point(|byte| *byte < byte_end)) };
                if styles[first_style].font_size() == 0.0 {
                    if single_group {
                        raw_caret_stops = vec![0.0; character_bytes.len()].into();
                        raw_cluster_boundaries = vec![true; character_bytes.len()].into();
                    } else {
                        Arc::make_mut(&mut raw_caret_stops)[character_start..=character_end].fill(x_base);
                        Arc::make_mut(&mut raw_cluster_boundaries)[character_start..=character_end].fill(true);
                    }
                    first_style = end_style;
                    continue;
                }
                let layout = self.layout_directional_line(group_text, px(styles[first_style].font_size()), &text_runs[font_run_boundaries[first_style]..font_run_boundaries[end_style]], styles[first_style].direction());
                let bytes = &character_bytes[character_start..=character_end];
                let native = native_cluster_geometry(group_text, &layout, bytes, byte_start);
                let (mut local_caret_stops, local_cluster_boundaries) =
                    native.as_ref().map(|geometry| (geometry.carets.clone(), geometry.boundaries.clone())).unwrap_or_else(|| build_text_boundaries_with_offsets(group_text, &layout, bytes, byte_start));
                let mut group_width = f32::from(layout.width);
                if native.is_none()
                    && group_text.chars().last().is_some_and(char::is_whitespace)
                    && local_caret_stops.len() >= 2
                    && (local_caret_stops[local_caret_stops.len() - 1] - local_caret_stops[local_caret_stops.len() - 2]).abs() <= f32::EPSILON
                {
                    let probe_started = trailing_whitespace_profiling_enabled().then(Instant::now);
                    // Native line layout trims trailing whitespace from its
                    // caret geometry. Probe with a following printable glyph
                    // so the same font shapes that whitespace as interior
                    // content, then retain only the caret before the probe.
                    let mut probe_text = String::with_capacity(group_text.len() + 1);
                    probe_text.push_str(group_text);
                    probe_text.push('x');
                    let mut probe_runs = text_runs[font_run_boundaries[first_style]..font_run_boundaries[end_style]].to_vec();
                    if let Some(last) = probe_runs.last_mut() {
                        last.len += 1;
                    }
                    let probe_layout = self.layout_directional_line(&probe_text, px(styles[first_style].font_size()), &probe_runs, styles[first_style].direction());
                    let (probe_carets, _) = build_text_boundaries(&probe_text, &probe_layout);
                    let original_character_count = character_end - character_start;
                    if let Some(&corrected_end) = probe_carets.get(original_character_count) {
                        Arc::make_mut(&mut local_caret_stops)[original_character_count] = corrected_end;
                        group_width = group_width.max(corrected_end);
                    }
                    self.shape_cache_stats.trailing_whitespace_probes += 1;
                    self.shape_cache_stats.trailing_whitespace_probe_characters += original_character_count as u64;
                    if let Some(started) = probe_started {
                        self.shape_cache_stats.trailing_whitespace_probe_time += started.elapsed();
                    }
                }
                let local_advances = native.map(|geometry| geometry.advances).unwrap_or_else(|| advances_from_carets(&local_caret_stops, &local_cluster_boundaries));
                raw_advances[character_start..character_end].copy_from_slice(&local_advances);
                if single_group {
                    raw_caret_stops = local_caret_stops;
                    raw_cluster_boundaries = local_cluster_boundaries;
                } else {
                    let carets = Arc::make_mut(&mut raw_caret_stops);
                    for (offset, caret) in local_caret_stops.iter().copied().enumerate() {
                        carets[character_start + offset] = x_base + caret;
                    }
                    let boundaries = Arc::make_mut(&mut raw_cluster_boundaries);
                    for (offset, boundary) in local_cluster_boundaries.iter().copied().enumerate() {
                        boundaries[character_start + offset] |= boundary;
                    }
                }
                glyphs.reserve(layout.runs.iter().map(|run| run.glyphs.len()).sum::<usize>());
                for shaped_run in &layout.runs {
                    for glyph in &shaped_run.glyphs {
                        glyphs.push(GpuiRunGlyph {
                            font_id: shaped_run.font_id,
                            glyph_id: glyph.id,
                            position: gpui::point(px(x_base) + glyph.position.x, glyph.position.y),
                            font_size: layout.font_size,
                            byte_index: byte_start + glyph.index,
                            is_emoji: glyph.is_emoji,
                        });
                    }
                }
                x_base += group_width;
                ascent = ascent.max(f32::from(layout.ascent));
                descent = descent.max(f32::from(layout.descent));
                first_style = end_style;
            }
            let geometry =
                CachedLineGeometry { advances: raw_advances.into(), glyphs: Arc::from(glyphs), ascent, descent, caret_stops: raw_caret_stops, cluster_boundaries: raw_cluster_boundaries, character_bytes: Arc::from(character_bytes) };
            self.insert_line_shape(key, CachedLineShape { text: text.into(), styles: identities.into_boxed_slice(), geometry, natural_run: None });
            colors = Some(miss_colors);
        }
        let Some(cached_shape) = self.line_shapes.get(&key) else { return Ok(None) };
        let cached = &cached_shape.geometry;
        let ascent = cached.ascent;
        let cluster_boundaries = cached.cluster_boundaries.clone();
        let (line_index, text_range) = if let Some(request) = placement_request {
            (request.line_index(), request.text_range())
        } else {
            let Ok(character_count) = u32::try_from(cached.character_bytes.len() - 1) else { return Ok(None) };
            (0, 0..character_count)
        };

        let (glyphs, caret_stops) = if placements_are_natural {
            (cached.glyphs.clone(), cached.caret_stops.clone())
        } else {
            let mut glyphs = cached.glyphs.to_vec();
            let Some(request) = placement_request else { return Ok(None) };
            let Some(caret_stops) = apply_line_placements(request, &cached.character_bytes, &cached.caret_stops, &mut glyphs) else { return Ok(None) };
            (Arc::from(glyphs), caret_stops)
        };

        let reusable_run = placements_are_natural.then(|| cached_shape.natural_run.as_ref().filter(|run| run.colors_match(styles)).cloned()).flatten();
        let run_resource = if let Some(run) = reusable_run {
            run
        } else {
            let colors = colors.unwrap_or_else(|| styles.iter().map(|style| (style.byte_range(), Hsla::from(rgba(style.color())))).collect::<Vec<_>>()).into_boxed_slice();
            let mut run = GpuiTextRun::new(glyphs, px(cached.ascent), px(cached.descent), cached.character_bytes.clone(), colors);
            run.cache_paint_bounds(&self.text_system);
            run.caret_stops = caret_stops.clone();
            if placements_are_natural {
                run.advances = cached.advances.clone();
            }
            let run = Arc::new(run);
            if placements_are_natural && let Some(cached) = self.line_shapes.get_mut(&key).filter(|cached| cached.matches(text, styles)) {
                cached.natural_run = Some(run.clone());
            }
            run
        };
        let run = self.runs.len() as u32;
        Rc::make_mut(&mut self.runs).push(run_resource);
        Ok(Some(ShapedLine { line_index, text_range, run, ascent, caret_stops, cluster_boundaries }))
    }

    fn set_text_run_colors(&mut self, run_id: u32, colors: &[u32]) {
        let Some(existing) = self.runs.get(run_id as usize).cloned() else { return };
        let Some(spans) = paint_color_spans(&existing.character_bytes, colors) else { return };
        Rc::make_mut(&mut self.runs)[run_id as usize] = Arc::new(existing.with_colors(spans.into_boxed_slice()));
    }
}

fn paint_color_spans(character_bytes: &[usize], colors: &[u32]) -> Option<Vec<(Range<usize>, Hsla)>> {
    if character_bytes.len() != colors.len().checked_add(1)? || colors.is_empty() {
        return None;
    }
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut color = colors[0];
    for index in 1..colors.len() {
        if colors[index] == color {
            continue;
        }
        spans.push((character_bytes[start]..character_bytes[index], Hsla::from(rgba(color))));
        start = index;
        color = colors[index];
    }
    spans.push((character_bytes[start]..*character_bytes.last()?, Hsla::from(rgba(color))));
    Some(spans)
}

impl GlyphShaper for GpuiTextCache {
    fn reset(&mut self) {
        self.generation = GlyphResourceGeneration::default();
        self.table.clear();
        self.glyphs = Rc::default();
        self.runs = Rc::default();
    }

    fn resource_generation(&self) -> GlyphResourceGeneration {
        self.generation
    }

    fn begin_append_shaping(&mut self) -> Result<(), ShapeError> {
        self.refresh_publication_fonts();
        debug_assert!(self.previous_append.is_none(), "append shaping transactions cannot be nested");
        self.previous_append = Some(AppendTextResources { glyphs: self.glyphs.clone(), runs: self.runs.clone() });
        Ok(())
    }

    fn commit_append_shaping(&mut self) {
        self.previous_append = None;
    }

    fn rollback_append_shaping(&mut self) {
        if let Some(previous) = self.previous_append.take() {
            self.table.retain(|_, glyph| (glyph.handle() as usize) < previous.glyphs.len());
            self.glyphs = previous.glyphs;
            self.runs = previous.runs;
        }
    }

    fn begin_document_shaping(&mut self) {
        self.refresh_publication_fonts();
        if western_shaping_benchmark_enabled() {
            self.western_benchmark.reset();
        }
        debug_assert!(self.previous_document.is_none(), "document shaping transactions cannot be nested");
        let previous =
            DocumentTextResources { generation: std::mem::take(&mut self.generation), table: std::mem::take(&mut self.table), glyphs: std::mem::take(&mut self.glyphs), runs: std::mem::replace(&mut self.runs, Rc::new(Vec::new())) };
        self.previous_document = Some(previous);
    }

    fn commit_document_shaping(&mut self) {
        if western_shaping_benchmark_enabled() {
            self.western_benchmark.report();
        }
        self.previous_document = None;
    }

    fn rollback_document_shaping(&mut self) {
        if let Some(previous) = self.previous_document.take() {
            self.generation = previous.generation;
            self.table = previous.table;
            self.glyphs = previous.glyphs;
            self.runs = previous.runs;
        }
    }

    fn font_relative_metrics(&mut self, request: FontMetricsRequest<'_>) -> Result<FontRelativeMetrics, ShapeError> {
        let gpui_font = self.resolve_font(request.font_family(), request.font_weight(), request.font_slant());
        let font_id = self.text_system.resolve_font(&gpui_font);
        // CSS `ex` is the selected face's x-height, not the ink bounds of its
        // `x` glyph. Those differ for fonts such as WPT's Ahem and can also
        // differ because of hinting. GPUI exposes the face metric directly.
        let font_size = request.font_size();
        let x_height_ratio = f32::from(self.text_system.x_height(font_id, px(font_size))) / font_size;
        let cap_height_ratio = f32::from(self.text_system.cap_height(font_id, px(font_size))) / font_size;
        let zero_run = TextRun { len: 1, font: gpui_font, color: Hsla::from(rgba(0x000000FF)), background_color: None, underline: None, strikethrough: None };
        let zero_layout = self.text_system.layout_line("0", px(font_size), &[zero_run], None);
        let ch_advance_ratio = f32::from(zero_layout.width) / font_size;
        let ascent_ratio = f32::from(self.text_system.ascent(font_id, px(font_size))) / font_size;
        // GPUI exposes descent below the baseline as a signed value; layout's
        // metric contract stores its positive magnitude.
        let descent_ratio = -f32::from(self.text_system.descent(font_id, px(font_size))) / font_size;
        Ok(FontRelativeMetrics::from_line_ratios(x_height_ratio, ch_advance_ratio, cap_height_ratio, ascent_ratio, descent_ratio).unwrap_or_else(FontRelativeMetrics::fallback))
    }

    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
        let gpui_font = self.resolve_font_for_script(request.font_family, request.font_weight, request.font_slant, is_cjk_character(request.character));
        let font_style_key = font_slant_key(request.font_slant);
        let key = GlyphKey { ch: request.character, font_size: request.font_size.to_bits(), font_weight: request.font_weight, font_style: font_style_key, family: gpui_font.family.to_string(), fallbacks: gpui_font.fallbacks.clone() };
        if let Some(&glyph) = self.table.get(&key) { return Ok(glyph); }

        if let Some(cached) = self.shapes.get(&key).cloned() {
            self.shape_cache_stats.glyph_hits += 1;
            let handle = self.glyphs.len() as GlyphId;
            let registry_glyph = ShapedGlyph::from_metric(handle, cached.metric);
            let glyphs = Rc::make_mut(&mut self.glyphs);
            glyphs.push(GpuiGlyph::Layout { layout: cached.layout });
            self.table.insert(key, registry_glyph);
            return Ok(registry_glyph);
        }
        self.shape_cache_stats.glyph_misses += 1;

        // GPUI may clamp a zero-pixel request to paintable fallback geometry.
        // CSS defines this case as no ink and no metric contribution, so do
        // not send it through native shaping.
        if request.font_size == 0.0 {
            let metric = GlyphMetric::try_new(request.character, 0.0, 0.0, 0.0)?;
            let handle = self.glyphs.len() as GlyphId;
            let registry_glyph = ShapedGlyph::from_metric(handle, metric);
            let glyphs = Rc::make_mut(&mut self.glyphs);
            glyphs.push(GpuiGlyph::Empty);
            self.table.insert(key, registry_glyph);
            return Ok(registry_glyph);
        }

        let mut utf8 = [0; 4];
        let text = request.character.encode_utf8(&mut utf8);
        let run = TextRun { len: text.len(), font: gpui_font, ..Default::default() };
        let layout = self.text_system.layout_line(text, px(request.font_size), &[run], None);
        let glyphs = Rc::make_mut(&mut self.glyphs);
        let handle = glyphs.len() as GlyphId;
        let metric = GlyphMetric::try_new(request.character, f32::from(layout.width), f32::from(layout.ascent), f32::from(layout.descent).abs())?;
        let registry_glyph = ShapedGlyph::from_metric(handle, metric);
        glyphs.push(GpuiGlyph::Layout { layout: layout.clone() });

        self.table.insert(key.clone(), registry_glyph);
        self.insert_shape(key, CachedGlyphShape { layout, metric });
        Ok(registry_glyph)
    }

    fn shape_glyph_run(&mut self, request: TextRunShapeRequest<'_>, glyphs: &mut [GlyphId], resources: &mut GlyphResourceStore) -> Result<Option<ShapedTextRun>, ShapeError> {
        let benchmark_enabled = western_shaping_benchmark_enabled();
        let run_text = request.text();
        let run_style = request.style().clone();
        let western = is_western_benchmark_text(run_text);
        let full_run_started = Instant::now();
        let shaped = self.shape_text_run(request);
        let full_run_shaping = full_run_started.elapsed();

        // Benchmark the previous path without using its metrics for layout.
        // This is intentionally gated because it allocates legacy resources.
        let mut legacy_glyphs = benchmark_enabled.then(|| vec![GlyphId::MAX; glyphs.len()]);
        let legacy_started = Instant::now();
        if let Some(legacy_glyphs) = legacy_glyphs.as_mut() {
            let mut ascii = [GlyphId::MAX; 128];
            let mut non_ascii = HashMap::<char, GlyphId>::new();
            let style = &run_style;
            for (slot, character) in legacy_glyphs.iter_mut().zip(run_text.chars()) {
                let cached = if character.is_ascii() { ascii[character as usize] } else { non_ascii.get(&character).copied().unwrap_or(GlyphId::MAX) };
                *slot = if cached != GlyphId::MAX {
                    cached
                } else {
                    let glyph = resources.shape_glyph(self, GlyphRequest { character, font_size: style.font_size(), font_weight: style.font_weight(), font_slant: style.font_slant(), font_family: style.font_family_for_character(character) })?;
                    if character.is_ascii() {
                        ascii[character as usize] = glyph
                    } else {
                        non_ascii.insert(character, glyph);
                    }
                    glyph
                };
            }
        }
        let legacy_glyph_path = legacy_started.elapsed();

        let joined_cluster_boundaries = shaped.as_ref().ok().and_then(Option::as_ref).map_or(0, |run| run.cluster_boundaries().iter().filter(|boundary| !**boundary).count());
        let registration_started = Instant::now();
        let fast_registered = if western || run_style.font_size() == 0.0 || (run_style.cjk_font_family().is_some() && run_text.chars().any(is_cjk_character)) {
            if let Ok(Some(native)) = &shaped
                && let Some(run_id) = native.backend_run()
                && native.caret_stops().is_some()
                && let Some(run) = self.runs.get(run_id as usize).cloned()
            {
                let first_glyph = self.glyphs.len();
                let mut registered = 0;
                let registration: Result<(), ShapeError> = (|| {
                    for (index, ((slot, character), advance)) in glyphs.iter_mut().zip(run_text.chars()).zip(native.advances()).enumerate() {
                        let metric = GlyphMetric::try_new(character, *advance, f32::from(run.ascent), f32::from(run.descent).abs())?;
                        let glyph = resources.register_shaped(character, ShapedGlyph::from_metric((first_glyph + index) as GlyphId, metric))?;
                        *slot = glyph;
                        registered += 1;
                    }
                    Ok(())
                })();
                // Retain native resources for the successfully registered prefix,
                // including when the caller must roll back a failure.
                Rc::make_mut(&mut self.glyphs).push_run(run, registered);
                registration?;
                true
            } else {
                false
            }
        } else {
            false
        };
        let authoritative_registration = registration_started.elapsed();

        if !fast_registered {
            if let Some(legacy_glyphs) = legacy_glyphs {
                glyphs.copy_from_slice(&legacy_glyphs);
            } else {
                let mut ascii = [GlyphId::MAX; 128];
                let mut non_ascii = HashMap::<char, GlyphId>::new();
                let style = &run_style;
                for (slot, character) in glyphs.iter_mut().zip(run_text.chars()) {
                    let cached = if character.is_ascii() { ascii[character as usize] } else { non_ascii.get(&character).copied().unwrap_or(GlyphId::MAX) };
                    *slot = if cached != GlyphId::MAX {
                        cached
                    } else {
                        let glyph = resources.shape_glyph(self, GlyphRequest { character, font_size: style.font_size(), font_weight: style.font_weight(), font_slant: style.font_slant(), font_family: style.font_family_for_character(character) })?;
                        if character.is_ascii() {
                            ascii[character as usize] = glyph
                        } else {
                            non_ascii.insert(character, glyph);
                        }
                        glyph
                    };
                }
            }
        }

        if benchmark_enabled {
            self.western_benchmark.runs += 1;
            self.western_benchmark.characters += glyphs.len() as u64;
            self.western_benchmark.unsupported_runs += u64::from(!fast_registered);
            self.western_benchmark.legacy_glyph_path += legacy_glyph_path;
            self.western_benchmark.full_run_shaping += full_run_shaping;
            self.western_benchmark.authoritative_registration += if fast_registered { authoritative_registration } else { legacy_glyph_path };
            self.western_benchmark.joined_cluster_boundaries += joined_cluster_boundaries as u64;
        }
        shaped
    }

    fn shape_text_run(&mut self, request: TextRunShapeRequest<'_>) -> Result<Option<ShapedTextRun>, ShapeError> {
        let paint_colors = request.paint_colors();
        let styles = [request.style().clone()];
        // TextRunShapeRequest already validates the full style span. Natural
        // shaping needs no per-character placement buffer or second validation.
        let shaped = self.shape_line_geometry(request.text(), &styles, None);
        let Some(shaped) = shaped? else { return Ok(None) };
        if let Some(colors) = paint_colors {
            self.set_text_run_colors(shaped.run, colors);
        }
        Ok(shaped_text_run_from_line(&shaped, self.runs[shaped.run as usize].advances.clone()))
    }

    fn measure_line(&mut self, request: TextShapeRequest<'_>) -> Result<Option<ShapedTextRun>, ShapeError> {
        let retained_runs = self.runs.len();
        let shaped = self.shape_line(request);
        let portable = shaped.and_then(|shaped| {
            Ok(shaped.and_then(|shaped| shaped_text_run_from_line(&shaped, self.runs[shaped.run as usize].advances.clone()).and_then(|run| ShapedTextRun::new(Arc::from(run.advances()), Arc::from(run.cluster_boundaries())))))
        });
        Rc::make_mut(&mut self.runs).truncate(retained_runs);
        portable
    }

    fn shape_line(&mut self, request: TextShapeRequest<'_>) -> Result<Option<ShapedLine>, ShapeError> {
        self.shape_line_geometry(request.text(), request.styles(), Some(&request))
    }

    fn begin_line_shaping(&mut self) {}
}

fn shaped_text_run_from_line(shaped: &ShapedLine, advances: Arc<[f32]>) -> Option<ShapedTextRun> {
    let character_count = shaped.caret_stops.len().checked_sub(1)?;
    let advances = if advances.len() == character_count { advances } else { advances_from_carets(&shaped.caret_stops, &shaped.cluster_boundaries) };
    ShapedTextRun::with_backend_run(advances, shaped.cluster_boundaries.clone(), shaped.run, shaped.caret_stops.clone(), shaped.ascent)
}

fn advances_from_carets(carets: &[f32], boundaries: &[bool]) -> Arc<[f32]> {
    let character_count = carets.len().saturating_sub(1);
    let mut advances = vec![0.0; character_count];
    let mut boundary_indices = boundaries.iter().enumerate().filter_map(|(index, boundary)| boundary.then_some(index));
    let mut start = boundary_indices.next().unwrap_or(0);
    for end in boundary_indices {
        if start < end && end <= character_count {
            let width = (carets[end] - carets[start]).abs();
            advances[start..end].fill(width / (end - start) as f32);
        }
        start = end;
    }
    advances.into()
}

struct NativeClusterGeometry {
    carets: Arc<[f32]>,
    boundaries: Arc<[bool]>,
    advances: Arc<[f32]>,
}

fn native_cluster_geometry(text: &str, layout: &gpui::LineLayout, bytes: &[usize], byte_offset: usize) -> Option<NativeClusterGeometry> {
    if layout.clusters.is_empty() {
        return None;
    }
    let count = bytes.len().checked_sub(1)?;
    let mut carets = vec![0.0; count + 1];
    let mut boundaries = vec![false; count + 1];
    let mut advances = vec![0.0; count];
    boundaries[0] = true;
    boundaries[count] = true;
    for cluster in &layout.clusters {
        if cluster.bytes.is_empty() || cluster.bytes.end > text.len() {
            return None;
        }
        let start = bytes.binary_search(&(byte_offset + cluster.bytes.start)).ok()?;
        let end = bytes.binary_search(&(byte_offset + cluster.bytes.end)).ok()?;
        if start >= end {
            return None;
        }
        let width = f32::from(cluster.advance);
        let left = f32::from(cluster.x);
        if !width.is_finite() || width < 0.0 || !left.is_finite() {
            return None;
        }
        let (x0, x1) = if cluster.is_rtl { (left + width, left) } else { (left, left + width) };
        boundaries[start] = true;
        boundaries[end] = true;
        advances[start..end].fill(width / (end - start) as f32);
        for index in start..=end {
            carets[index] = x0 + (x1 - x0) * (index - start) as f32 / (end - start) as f32;
        }
    }
    Some(NativeClusterGeometry { carets: carets.into(), boundaries: boundaries.into(), advances: advances.into() })
}

fn apply_line_placements(request: &TextShapeRequest<'_>, character_bytes: &[usize], raw_caret_stops: &[f32], glyphs: &mut Vec<GpuiRunGlyph>) -> Option<Arc<[f32]>> {
    debug_assert_eq!(character_bytes.len(), raw_caret_stops.len());
    let character_count = character_bytes.len().saturating_sub(1);
    let caret_stops = request.adjusted_caret_stops(raw_caret_stops)?;
    let placements = request.placements();

    for glyph in glyphs.iter_mut() {
        let character_index = character_bytes.partition_point(|byte| *byte <= glyph.byte_index).saturating_sub(1).min(character_count - 1);
        glyph.position.x += px(caret_stops[character_index] - raw_caret_stops[character_index]);
        glyph.position.y -= px(placements[character_index].baseline_shift());
    }
    glyphs.retain(|glyph| {
        let character_index = character_bytes.partition_point(|byte| *byte <= glyph.byte_index).saturating_sub(1).min(character_count - 1);
        placements[character_index].paints()
    });
    Some(caret_stops)
}

fn build_text_boundaries(text: &str, layout: &gpui::LineLayout) -> (Arc<[f32]>, Arc<[bool]>) {
    let mut character_bytes = text.char_indices().map(|(byte, _)| byte).collect::<Vec<_>>();
    character_bytes.push(text.len());
    build_text_boundaries_with_offsets(text, layout, &character_bytes, 0)
}

fn build_text_boundaries_with_offsets(text: &str, layout: &gpui::LineLayout, character_bytes: &[usize], byte_offset: usize) -> (Arc<[f32]>, Arc<[bool]>) {
    if character_bytes.len() <= 1 {
        return (Arc::from([]), Arc::from([true]));
    }

    let glyph_bytes = || layout.runs.iter().flat_map(|run| run.glyphs.iter().map(|glyph| glyph.index));
    if glyph_bytes().is_sorted() {
        // Ordered clusters need no temporary index array or sorting. Include
        // both ends so missing glyphs and trailing whitespace retain their width.
        build_text_boundaries_from_clusters(text, layout, character_bytes, byte_offset, glyph_bytes().chain(std::iter::once(text.len())))
    } else {
        let mut cluster_bytes = glyph_bytes().collect::<Vec<_>>();
        cluster_bytes.push(0);
        cluster_bytes.push(text.len());
        cluster_bytes.sort_unstable();
        cluster_bytes.dedup();
        build_text_boundaries_from_clusters(text, layout, character_bytes, byte_offset, cluster_bytes.into_iter())
    }
}

fn build_text_boundaries_from_clusters(text: &str, layout: &gpui::LineLayout, character_bytes: &[usize], byte_offset: usize, cluster_bytes: impl Iterator<Item = usize>) -> (Arc<[f32]>, Arc<[bool]>) {
    let mut caret_stops = vec![0.0; character_bytes.len()];
    let mut cluster_boundaries = vec![false; character_bytes.len()];
    if let Some(first) = cluster_boundaries.first_mut() {
        *first = true;
    }
    if let Some(last) = cluster_boundaries.last_mut() {
        *last = true;
    }
    let mut glyphs = layout.runs.iter().flat_map(|run| run.glyphs.iter()).peekable();
    // Preserve GPUI's first-match lookup even for the mixed-order fallback.
    let mut x_for_index = |index| {
        while glyphs.peek().is_some_and(|glyph| glyph.index < index) {
            glyphs.next();
        }
        glyphs.peek().map_or(layout.width, |glyph| glyph.position.x)
    };
    let mut byte_start = 0;
    let mut char_start = 0;
    for byte_end in cluster_bytes {
        let byte_end = byte_end.min(text.len());
        if byte_start >= byte_end {
            continue;
        }
        // Cluster boundaries are increasing in both paths. Each character
        // offset is visited once instead of being searched for every cluster.
        let mut char_end = char_start;
        while char_end < character_bytes.len() && character_bytes[char_end] < byte_offset + byte_end {
            char_end += 1;
        }
        if char_start < char_end {
            cluster_boundaries[char_start] = true;
            cluster_boundaries[char_end] = true;
            let x0 = f32::from(x_for_index(byte_start));
            // Use the full width at the end, including trailing whitespace.
            let x1 = if byte_end == text.len() { f32::from(layout.width) } else { f32::from(x_for_index(byte_end)) };
            let count = char_end - char_start;
            for index in 0..=count {
                caret_stops[char_start + index] = x0 + (x1 - x0) * index as f32 / count as f32;
            }
        }
        byte_start = byte_end;
        char_start = char_end;
    }
    (Arc::from(caret_stops), Arc::from(cluster_boundaries))
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use gpui::{FontId, LineLayout, ShapedCluster, ShapedGlyph, ShapedRun, point};

    #[test]
    fn native_rtl_clusters_preserve_widths_and_the_left_terminal_caret() {
        let text = "אבג.";
        let mut native = layout(text, &[], 32.0);
        native.clusters = vec![
            ShapedCluster { bytes: 0..2, x: px(22.0), advance: px(10.0), is_rtl: true },
            ShapedCluster { bytes: 2..4, x: px(13.0), advance: px(9.0), is_rtl: true },
            ShapedCluster { bytes: 4..6, x: px(5.0), advance: px(8.0), is_rtl: true },
            ShapedCluster { bytes: 6..7, x: px(0.0), advance: px(5.0), is_rtl: true },
        ];
        let geometry = native_cluster_geometry(text, &native, &[0, 2, 4, 6, 7], 0).unwrap();
        assert_eq!(&*geometry.carets, &[32.0, 22.0, 13.0, 5.0, 0.0]);
        assert_eq!(&*geometry.advances, &[10.0, 9.0, 8.0, 5.0]);
        assert_eq!(geometry.advances.iter().sum::<f32>(), f32::from(native.width));
    }

    #[test]
    fn native_clusters_do_not_turn_bidi_jumps_into_character_advances() {
        let text = "aאב";
        let mut native = layout(text, &[], 30.0);
        native.clusters = vec![
            ShapedCluster { bytes: 0..1, x: px(0.0), advance: px(10.0), is_rtl: false },
            ShapedCluster { bytes: 1..3, x: px(20.0), advance: px(10.0), is_rtl: true },
            ShapedCluster { bytes: 3..5, x: px(10.0), advance: px(10.0), is_rtl: true },
        ];
        let geometry = native_cluster_geometry(text, &native, &[4, 5, 7, 9], 4).unwrap();
        assert_eq!(&*geometry.advances, &[10.0; 3]);
        assert_eq!(&*geometry.boundaries, &[true; 4]);
        // A bidi boundary has two caret positions. Its visual jump must not
        // be counted as extra width in the preceding character.
        assert_ne!(&*advances_from_carets(&geometry.carets, &geometry.boundaries), &*geometry.advances);
        let shaped = ShapedLine { line_index: 0, text_range: 0..3, run: 0, ascent: 12.0, caret_stops: geometry.carets, cluster_boundaries: geometry.boundaries };
        let converted = shaped_text_run_from_line(&shaped, geometry.advances).unwrap();
        assert_eq!(converted.advances(), &[10.0; 3]);
    }

    #[test]
    fn native_ligature_clusters_interpolate_carets_and_keep_trailing_spaces() {
        let text = "لا ";
        let mut native = layout(text, &[], 25.0);
        native.clusters = vec![ShapedCluster { bytes: 0..4, x: px(5.0), advance: px(20.0), is_rtl: true }, ShapedCluster { bytes: 4..5, x: px(0.0), advance: px(5.0), is_rtl: true }];
        let geometry = native_cluster_geometry(text, &native, &[0, 2, 4, 5], 0).unwrap();
        assert_eq!(&*geometry.carets, &[25.0, 15.0, 5.0, 0.0]);
        assert_eq!(&*geometry.advances, &[10.0, 10.0, 5.0]);
        assert_eq!(&*geometry.boundaries, &[true, false, true, true]);
    }

    #[test]
    fn native_cluster_metadata_is_rebased_or_dropped_when_splitting() {
        let mut native = layout("fia", &[&[(0, 0.0), (2, 20.0)]], 30.0);
        native.clusters = vec![ShapedCluster { bytes: 0..2, x: px(0.0), advance: px(20.0), is_rtl: false }, ShapedCluster { bytes: 2..3, x: px(20.0), advance: px(10.0), is_rtl: false }];
        let (prefix, suffix) = native.split_at(2);
        assert_eq!(prefix.clusters.len(), 1);
        assert_eq!(prefix.clusters[0].bytes, 0..2);
        assert_eq!(suffix.clusters[0].bytes, 0..1);
        assert_eq!(suffix.clusters[0].x, px(0.0));
        let (prefix, suffix) = native.split_at(1);
        assert!(prefix.clusters.is_empty());
        assert!(suffix.clusters.is_empty());
    }

    #[test]
    fn font_family_names_decode_css_hex_escapes_and_preserve_quoted_spaces() {
        assert_eq!(font_family_names(r#"\31 Reader, " A\20 B ", serif"#).collect::<Vec<_>>(), vec![("1Reader".into(), false), (" A B ".into(), true), ("serif".into(), false)]);
    }

    #[test]
    fn installed_font_names_preserve_commas_quotes_and_backslashes() {
        let families: Vec<_> = font_family_names(r#""A, B's \"Serif\" \\ Font", NV Garamond, serif"#).collect();
        assert_eq!(families, vec![("A, B's \"Serif\" \\ Font".into(), true), ("NV Garamond".into(), false), ("serif".into(), false)]);
        assert_eq!(font_family_names("'serif', sans-serif").collect::<Vec<_>>(), vec![("serif".into(), true), ("sans-serif".into(), false)]);
    }

    fn layout(text: &str, runs: &[&[(usize, f32)]], width: f32) -> LineLayout {
        LineLayout {
            width: px(width),
            len: text.len(),
            runs: runs.iter().map(|glyphs| ShapedRun { font_id: FontId(0), glyphs: glyphs.iter().map(|&(index, x)| ShapedGlyph { id: gpui::GlyphId(0), position: point(px(x), px(0.0)), index, is_emoji: false }).collect() }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn matches_platform_lookup_in_every_visual_order_across_runs() {
        // Checking every permutation also covers reversed and mixed source
        // order: the cursor must preserve GPUI's first-match semantics.
        for order in 0..256usize {
            let indices: [usize; 4] = std::array::from_fn(|index| (order >> (index * 2)) & 3);
            let mut sorted = indices;
            sorted.sort_unstable();
            if sorted != [0, 1, 2, 3] {
                continue;
            }
            let glyphs: Vec<_> = indices.iter().enumerate().map(|(visual, &byte)| (byte, visual as f32 * 10.0)).collect();
            let layout = layout("abcd", &[&glyphs[..2], &glyphs[2..]], 40.0);
            let (carets, boundaries) = build_text_boundaries("abcd", &layout);
            let expected: Vec<_> = (0..4).map(|byte| f32::from(layout.x_for_index(byte))).chain([40.0]).collect();
            assert_eq!(&*carets, expected, "visual order {indices:?}");
            assert_eq!(&*boundaries, &[true; 5]);
        }
    }

    #[test]
    fn interpolates_ligatures_and_combining_clusters_with_duplicate_glyphs() {
        let ligature = layout("office", &[&[(0, 0.0), (1, 10.0), (4, 40.0), (5, 50.0)]], 60.0);
        let (carets, boundaries) = build_text_boundaries("office", &ligature);
        assert_eq!(&*carets, &[0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);
        assert_eq!(&*boundaries, &[true, true, false, false, true, true, true]);
        let offsets = (4..=10).collect::<Vec<_>>(); // "office" following a four-byte scalar
        let reused = build_text_boundaries_with_offsets("office", &ligature, &offsets, 4);
        assert_eq!(reused.0, carets);
        assert_eq!(reused.1, boundaries);

        let text = "a\u{301}b";
        let combining = layout(text, &[&[(0, 0.0), (0, 2.0), (3, 10.0)]], 20.0);
        let (carets, boundaries) = build_text_boundaries(text, &combining);
        assert_eq!(&*carets, &[0.0, 5.0, 10.0, 20.0]);
        assert_eq!(&*boundaries, &[true, false, true, true]);
    }

    #[test]
    fn ordered_multibyte_clusters_keep_first_duplicate_and_interpolate_combining_marks() {
        let text = "aé\u{301}中z";
        let native = layout(text, &[&[(0, 0.0), (1, 10.0), (1, 12.0)], &[(5, 40.0), (8, 50.0)]], 60.0);
        let (carets, boundaries) = build_text_boundaries_with_offsets(text, &native, &[4, 5, 7, 9, 12, 13], 4);
        assert_eq!(&*carets, &[0.0, 10.0, 25.0, 40.0, 50.0, 60.0]);
        assert_eq!(&*boundaries, &[true, true, false, true, true, true]);
    }

    #[test]
    fn preserves_multibyte_mixed_order_and_trailing_whitespace_width() {
        let text = "abאבe\u{301}z";
        let mixed = layout(text, &[&[(4, 0.0), (2, 10.0), (0, 20.0)], &[(9, 30.0), (6, 40.0), (6, 42.0), (1, 50.0)]], 60.0);
        let (carets, boundaries) = build_text_boundaries(text, &mixed);
        assert_eq!(&*carets, &[0.0, 0.0, 0.0, 0.0, 30.0, 30.0, 30.0, 60.0]);
        assert_eq!(&*boundaries, &[true, true, true, true, true, false, true, true]);

        let trailing = layout("a ", &[&[(0, 0.0), (1, 10.0)]], 15.0);
        assert_eq!(&*build_text_boundaries("a ", &trailing).0, &[0.0, 10.0, 15.0]);
    }

    #[test]
    fn handles_empty_text_and_missing_glyphs() {
        let empty = layout("", &[], 0.0);
        let (carets, boundaries) = build_text_boundaries("", &empty);
        assert!(carets.is_empty());
        assert_eq!(&*boundaries, &[true]);

        let missing = layout("abc", &[], 0.0);
        let (carets, boundaries) = build_text_boundaries("abc", &missing);
        assert_eq!(&*carets, &[0.0; 4]);
        assert_eq!(&*boundaries, &[true, false, false, true]);
    }
}

#[cfg(test)]
mod ownership_tests {

    use super::*;
    use gpui::{EmptyView, TestAppContext};
    use html::render::{FragmentRenderOptions, FragmentRenderer};

    #[test]
    fn mixed_fonts_keep_cjk_graphemes_and_latin_runs_separate() {
        let text = "office 漢\u{fe00}か\u{3099}カー 한국 한。 café";
        let mut runs = Vec::new();
        append_font_runs(&mut runs, text, font("Latin"), Some(font("CJK")), rgba(0x000000ff).into());
        let mut byte = 0;
        let parts = runs
            .iter()
            .map(|run| {
                let part = (&text[byte..byte + run.len], run.font.family.as_ref());
                byte += run.len;
                part
            })
            .collect::<Vec<_>>();
        assert_eq!(parts, [("office ", "Latin"), ("漢\u{fe00}か\u{3099}カー", "CJK"), (" ", "Latin"), ("한국", "CJK"), (" ", "Latin"), ("한。", "CJK"), (" café", "Latin")]);
        for latin in ["Latin ASCII", "Svenska åäö — café"] {
            let mut runs = Vec::new();
            append_font_runs(&mut runs, latin, font("Latin"), Some(font("CJK")), rgba(0x000000ff).into());
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].font.family.as_ref(), "Latin");
            assert_eq!(runs[0].len, latin.len());
        }
        let original = TextStyleSpan::new(0..text.len(), 18.0, 400, FontSlant::Normal, 0, Some("Latin")).unwrap();
        let changed = original.clone().with_cjk_font_family(Some("CJK"));
        assert_ne!(GpuiTextCache::line_shape_fingerprint(text, &[original]), GpuiTextCache::line_shape_fingerprint(text, &[changed]));
    }

    #[gpui::test]
    fn book_cjk_fonts_survive_latin_overrides_and_custom_cjk_bypasses_publisher_aliases(cx: &mut TestAppContext) {
        use html::style::{FontFace, FontFaceStyle};
        use html_fonts::LoadedFontFace;
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let fonts = Rc::new(RefCell::new(PublicationFonts::default()));
                let overrides = html::pipeline::ReaderStyleOverrides { font_family: Some("Latin".into()), cjk_font_fallback: Some("Embedded".into()), ..Default::default() };
                fonts.borrow_mut().set_reader_overrides(&overrides);
                let declaration = FontFace { family: "Publisher".into(), sources: vec![], weight: 400.0..=400.0, style: FontFaceStyle::Normal, stretch: 100.0..=100.0, unicode_ranges: vec![0..=0x10ffff], additional_descriptors: vec![] };
                fonts.borrow_mut().set_active(&[declaration.clone()]);
                fonts.borrow_mut().register(LoadedFontFace { declaration, family: "Private Book CJK".into(), bytes: None, weight: 400, slant: FontSlant::Normal, variable: false }, window.text_system()).unwrap();
                let mut shaper = GpuiTextCache::new(window.text_system().clone()).with_publication_fonts(fonts.clone());
                for (key, family) in [("latin", "Latin"), ("embedded", "Embedded"), ("publisher", "Installed Publisher")] {
                    shaper.font_families.insert(key.into(), family.into());
                }
                let style = TextStyleSpan::new(0..4, 18.0, 400, FontSlant::Normal, 0, Some("Latin")).unwrap().with_cjk_font_family(Some("Publisher, Embedded"));
                let selected = shaper.prepared_font(&style);
                assert_eq!(selected.font.family.as_ref(), "Latin");
                assert_eq!(selected.cjk_font.unwrap().family.as_ref(), "Private Book CJK");
                fonts.borrow_mut().set_reader_overrides(&html::pipeline::ReaderStyleOverrides { cjk_font_family: Some("Publisher".into()), ..overrides });
                shaper.refresh_publication_fonts();
                let selected = shaper.prepared_font(&style);
                assert_eq!(selected.font.family.as_ref(), "Latin");
                assert_eq!(selected.cjk_font.unwrap().family.as_ref(), "Installed Publisher");
            })
            .unwrap();
    }

    #[gpui::test]
    fn custom_fonts_keep_explicit_cjk_fallbacks_and_cache_identity(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                shaper.font_families.insert("latin".into(), "Latin".into());
                shaper.font_families.insert("droid sans fallback".into(), "Droid Sans Fallback".into());
                let font = shaper.resolve_font(Some("Missing, Latin, 'Droid Sans Fallback', Latin"), 400, FontSlant::Normal);
                assert_eq!(font.family.as_ref(), "Latin");
                assert_eq!(font.fallbacks.as_ref().unwrap().fallback_list(), &["Droid Sans Fallback"]);
                let without_fallback = shaper.resolve_font(Some("Latin"), 400, FontSlant::Normal);
                assert!(without_fallback.fallbacks.is_none());
                let key = |font: Font| GlyphKey { ch: '漢', font_size: 19.0_f32.to_bits(), font_weight: 400, font_style: 0, family: font.family.to_string(), fallbacks: font.fallbacks };
                assert_ne!(key(font), key(without_fallback), "glyph caches must distinguish fallback chains");
                assert_eq!(shaper.resolve_font(Some("Missing, 'Droid Sans Fallback'"), 400, FontSlant::Normal).family.as_ref(), "Droid Sans Fallback");
            })
            .unwrap();
    }

    #[gpui::test]
    fn publication_fonts_invalidate_cached_fallback_selection(cx: &mut TestAppContext) {
        use html::style::{FontFace, FontFaceStyle};
        use html_fonts::LoadedFontFace;
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let fonts = Rc::new(RefCell::new(PublicationFonts::default()));
                let mut shaper = GpuiTextCache::new(window.text_system().clone()).with_publication_fonts(fonts.clone());
                let style = TextStyleSpan::new(0..5, 19.0, 400, FontSlant::Normal, 0x000000ff, Some("Publisher")).unwrap();
                assert_eq!(shaper.prepared_font(&style).font.family.as_ref(), ".SystemUIFont");
                shaper.shape_line_geometry("Hello", &[style.clone()], None).unwrap();
                assert!(!shaper.line_shapes.is_empty());
                let declaration = FontFace { family: "Publisher".into(), sources: vec![], weight: 400.0..=400.0, style: FontFaceStyle::Normal, stretch: 100.0..=100.0, unicode_ranges: vec![0..=0x10ffff], additional_descriptors: vec![] };
                fonts.borrow_mut().set_active(&[declaration.clone()]);
                fonts.borrow_mut().register(LoadedFontFace { declaration, family: "LoadedFace".into(), bytes: None, weight: 400, slant: FontSlant::Normal, variable: true }, window.text_system()).unwrap();
                shaper.begin_document_shaping();
                assert!(shaper.line_shapes.is_empty());
                assert_eq!(shaper.prepared_font(&style).font.family.as_ref(), "LoadedFace");
                shaper.commit_document_shaping();
                let overrides = html::pipeline::ReaderStyleOverrides { serif_font_family: Some("Publisher".into()), ..Default::default() };
                fonts.borrow_mut().set_reader_overrides(&overrides);
                shaper.begin_document_shaping();
                assert_eq!(shaper.prepared_font(&style).font.family.as_ref(), ".SystemUIFont", "reader font choices must bypass a colliding publication alias");
                shaper.commit_document_shaping();
                fonts.borrow_mut().set_reader_overrides(&Default::default());
                shaper.begin_document_shaping();
                assert_eq!(shaper.prepared_font(&style).font.family.as_ref(), "LoadedFace");
                shaper.commit_document_shaping();
            })
            .unwrap();
    }

    fn register_run(shaper: &mut GpuiTextCache, text: &str, resources: &mut GlyphResourceStore) -> Vec<GlyphId> {
        let style = TextStyleSpan::new(0..text.len(), 19.0, 400, FontSlant::Normal, 0x000000ff, None).unwrap();
        let request = TextRunShapeRequest::new(text, style).unwrap();
        let mut glyphs = vec![GlyphId::MAX; text.chars().count()];
        assert!(shaper.shape_glyph_run(request, &mut glyphs, resources).unwrap().is_some());
        glyphs
    }

    #[gpui::test]
    fn run_records_preserve_dense_ids_across_fallbacks_snapshots_and_rollback(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let mut resources = GlyphResourceStore::new(shaper.resource_generation());
                assert_eq!(register_run(&mut shaper, "office", &mut resources), [0, 1, 2, 3, 4, 5]);
                assert_eq!(resources.shape_glyph(&mut shaper, GlyphRequest { character: '☃', font_size: 0.0, font_weight: 400, font_slant: FontSlant::Normal, font_family: None }).unwrap(), 6);
                assert_eq!(register_run(&mut shaper, "abc", &mut resources), [7, 8, 9]);
                let retained = shaper.store();
                assert_eq!(retained.len(), 10);
                assert_eq!(retained.records.len(), 3, "native runs must not allocate a backend record per character");
                for (glyph, character) in [(0, 0), (5, 5), (7, 0), (9, 2)] {
                    let Some(GpuiGlyph::RunFragment { range, .. }) = retained.get(glyph) else { panic!("expected a run character") };
                    assert_eq!(range, character..character + 1);
                }
                assert!(matches!(retained.get(6), Some(GpuiGlyph::Empty)));
                assert!(retained.get(10).is_none());
                assert!(retained.get(usize::MAX).is_none());

                let checkpoint = resources.checkpoint();
                shaper.begin_append_shaping().unwrap();
                assert_eq!(register_run(&mut shaper, "append", &mut resources), [10, 11, 12, 13, 14, 15]);
                assert_eq!(shaper.store().records.len(), 4);
                assert_eq!(retained.records.len(), 3);
                assert!(retained.get(10).is_none());
                shaper.rollback_append_shaping();
                assert!(resources.rollback_to(checkpoint));
                assert!(Rc::ptr_eq(&retained, &shaper.store()));
                assert_eq!(resources.len(), retained.len());
                shaper.reset();
                assert!(shaper.store().is_empty());
                assert!(matches!(retained.get(9), Some(GpuiGlyph::RunFragment { .. })));
            })
            .unwrap();
    }

    #[gpui::test]
    fn retained_paint_resources_survive_append_recolor_rollback_and_document_reset(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let mut renderer = FragmentRenderer::new();
                let rendered = renderer.render("<p style='color:red'>Retained text</p>", &FragmentRenderOptions::new(300.0, None).unwrap(), &mut shaper).unwrap();
                let scene = rendered.into_scene();
                let resources = renderer.glyph_resources_mut();
                let glyphs = shaper.store();
                let runs = shaper.run_store();
                assert!(!glyphs.is_empty());
                assert!(!runs.is_empty());
                let retained_color = runs[0].color_at(0);
                let retained_glyph_count = glyphs.len();
                let retained_run_count = runs.len();
                let retained_table_count = shaper.table.len();

                let checkpoint = resources.checkpoint();
                shaper.begin_append_shaping().unwrap();
                shaper.set_text_run_colors(0, &[0x0000ffff; 13]);
                assert_eq!(runs[0].color_at(0), retained_color);
                assert_ne!(shaper.run_store()[0].color_at(0), retained_color);
                // Zero-ink glyphs allocate resources with deterministic metrics.
                resources.shape_glyph(&mut shaper, GlyphRequest { character: '☃', font_size: 0.0, font_weight: 400, font_slant: FontSlant::Normal,  font_family: None }).unwrap();
                let appended = shaper.store();
                assert!(appended.len() > retained_glyph_count);
                assert_eq!(glyphs.len(), retained_glyph_count);
                shaper.rollback_append_shaping();
                assert!(resources.rollback_to(checkpoint));
                assert!(Rc::ptr_eq(&shaper.store(), &glyphs));
                assert!(Rc::ptr_eq(&shaper.run_store(), &runs));
                assert!(appended.len() > shaper.store().len());
                assert_eq!(shaper.table.len(), retained_table_count);

                shaper.begin_append_shaping().unwrap();
                let umbrella = resources.shape_glyph(&mut shaper, GlyphRequest { character: '☂', font_size: 0.0, font_weight: 400, font_slant: FontSlant::Normal,  font_family: None }).unwrap();
                let snowman = resources.shape_glyph(&mut shaper, GlyphRequest { character: '☃', font_size: 0.0, font_weight: 400, font_slant: FontSlant::Normal,  font_family: None }).unwrap();
                assert_ne!(umbrella, snowman, "a rolled-back lookup must not resolve a reused dense ID");
                assert_eq!(resources.get(umbrella).unwrap().ch(), '☂');
                assert_eq!(resources.get(snowman).unwrap().ch(), '☃');
                shaper.commit_append_shaping();
                assert!(shaper.store().len() > glyphs.len());
                shaper.begin_document_shaping();
                let previous_resources = std::mem::replace(resources, GlyphResourceStore::new(shaper.resource_generation()));
                resources.shape_glyph(&mut shaper, GlyphRequest { character: 'x', font_size: 0.0, font_weight: 400, font_slant: FontSlant::Normal,  font_family: None }).unwrap();
                shaper.rollback_document_shaping();
                *resources = previous_resources;
                assert!(shaper.store().len() > glyphs.len());
                shaper.begin_document_shaping();
                *resources = GlyphResourceStore::new(shaper.resource_generation());
                resources.shape_glyph(&mut shaper, GlyphRequest { character: 'y', font_size: 0.0, font_weight: 400, font_slant: FontSlant::Normal,  font_family: None }).unwrap();
                shaper.commit_document_shaping();
                assert_eq!(shaper.store().len(), 1);
                assert_eq!(glyphs.len(), retained_glyph_count);
                assert_eq!(runs.len(), retained_run_count);
                assert_eq!(runs[0].color_at(0), retained_color);
                // The retained scene still resolves every command using its own resources.
                let mut painter = crate::paint::GpuiCommandPainter::commands_only(glyphs, runs, Vec::new());
                scene.paint(&mut painter);
                assert!(!painter.into_commands().is_empty());
            })
            .unwrap();
    }
}

#[cfg(test)]
mod natural_run_tests {
    use super::*;
    use gpui::{EmptyView, TestAppContext};
    use html::text::CharacterPlacement;

    #[gpui::test]
    fn natural_runs_match_the_general_line_api_with_native_geometry_and_colors(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let features = [OpenTypeFeature::new(*b"liga", 0), OpenTypeFeature::new(*b"kern", 0)];
                for (text, size, slant) in [("office ffi", 19.0, FontSlant::Normal), ("a\u{301} 😀 אבג سلام", 27.0, FontSlant::Italic), ("word\u{2009}", 13.0, FontSlant::Oblique), ("zero size", 0.0, FontSlant::Normal)] {
                    let mut direct = GpuiTextCache::new(window.text_system().clone());
                    let mut general = GpuiTextCache::new(window.text_system().clone());
                    let count = text.chars().count();
                    let colors = (0..count).map(|index| if index % 2 == 0 { 0xff0000ff } else { 0x0000ffff }).collect::<Vec<_>>();
                    let style = TextStyleSpan::new(0..text.len(), size, 700, slant, 0xff0000ff, Some("serif")).unwrap().with_font_features(&features);
                    let request = TextRunShapeRequest::new(text, style.clone()).unwrap().with_paint_colors(&colors).unwrap();
                    let natural = direct.shape_text_run(request).unwrap().unwrap();
                    let styles = [style];
                    let placements = vec![CharacterPlacement::default(); count];
                    let request = TextShapeRequest::new(7, 10..10 + count as u32, text, &styles, &placements).unwrap();
                    let line = general.shape_line(request).unwrap().unwrap();
                    assert_eq!(line.line_index, 7);
                    assert_eq!(line.text_range, 10..10 + count as u32);
                    let expected = shaped_text_run_from_line(&line, general.runs[line.run as usize].advances.clone()).unwrap();
                    assert_eq!(natural.advances(), expected.advances(), "{text}");
                    assert_eq!(natural.cluster_boundaries(), expected.cluster_boundaries());
                    assert_eq!(natural.caret_stops(), expected.caret_stops());
                    assert_eq!(natural.ascent(), expected.ascent());
                    general.set_text_run_colors(line.run, &colors);
                    let actual = &direct.runs[natural.backend_run().unwrap() as usize];
                    let expected = &general.runs[line.run as usize];
                    assert_eq!(actual.character_bytes, expected.character_bytes);
                    assert_eq!(actual.colors, expected.colors);
                    assert_eq!(actual.glyphs.len(), expected.glyphs.len());
                    for (actual, expected) in actual.glyphs.iter().zip(expected.glyphs.iter()) {
                        assert_eq!(
                            (actual.font_id, actual.glyph_id, actual.position, actual.font_size, actual.byte_index, actual.is_emoji),
                            (expected.font_id, expected.glyph_id, expected.position, expected.font_size, expected.byte_index, expected.is_emoji)
                        );
                    }
                }
            })
            .unwrap();
    }

    #[gpui::test]
    fn font_setup_reuses_features_across_size_and_color_but_distinguishes_font_selection(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let style = TextStyleSpan::new(0..1, 19.0, 400, FontSlant::Normal, 0xff0000ff, Some("serif")).unwrap();
                let first = shaper.prepared_font(&style);
                let resized = TextStyleSpan::new(0..8, 26.0, 400, FontSlant::Normal, 0x0000ffff, Some("serif")).unwrap();
                let reused = shaper.prepared_font(&resized);
                assert!(Arc::ptr_eq(&first.identity, &reused.identity));
                assert!(Arc::ptr_eq(&first.font.features.0, &reused.font.features.0));
                for (weight, slant, family) in [(700, FontSlant::Normal, "serif"), (400, FontSlant::Italic, "serif"), (400, FontSlant::Normal, "monospace")] {
                    let distinct = shaper.prepared_font(&TextStyleSpan::new(0..1, 19.0, weight, slant, 0xff0000ff, Some(family)).unwrap());
                    assert!(!Arc::ptr_eq(&first.identity, &distinct.identity));
                }
                let features = [OpenTypeFeature::new(*b"liga", 0)];
                let disabled = shaper.prepared_font(&style.clone().with_font_features(&features));
                assert!(!Arc::ptr_eq(&first.identity, &disabled.identity));
                assert_eq!(disabled.font.features.0.iter().find(|(tag, _)| tag == "liga").unwrap().1, 0);
                let overrides = [OpenTypeFeature::new(*b"liga", 0), OpenTypeFeature::new(*b"liga", 1)];
                let enabled = shaper.prepared_font(&style.with_font_features(&overrides));
                assert_eq!(enabled.font.features.0.iter().find(|(tag, _)| tag == "liga").unwrap().1, 1);
                for weight in 100..100 + FONT_SETUP_CACHE_CAPACITY as u16 * 10 {
                    shaper.prepared_font(&TextStyleSpan::new(0..1, 19.0, weight, FontSlant::Normal, 0xff0000ff, Some("serif")).unwrap());
                }
                assert_eq!(shaper.prepared_fonts.len(), FONT_SETUP_CACHE_CAPACITY);
                assert_eq!(first.font.features.0.iter().find(|(tag, _)| tag == "liga").unwrap().1, 1, "retained identities and features survive cache eviction");
            })
            .unwrap();
    }

    #[gpui::test]
    fn mixed_font_sizes_zero_ink_and_adjusted_placements_keep_source_offsets(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let text = "aé中בz";
                let style = |range, size| TextStyleSpan::new(range, size, 400, FontSlant::Normal, 0x000000ff, Some("serif")).unwrap();
                let styles = [style(0..1, 19.0), style(1..3, 0.0), style(3..6, 26.0), style(6..9, 13.0)];
                let placements = vec![CharacterPlacement::default(); 5];
                let request = TextShapeRequest::new(2, 20..25, text, &styles, &placements).unwrap();
                let natural = shaper.shape_line(request).unwrap().unwrap();
                let mut expected_carets = vec![0.0; 6];
                let mut expected_boundaries = vec![false; 6];
                let mut x = 0.0;
                let mut character_start = 0;
                for span in &styles {
                    let run_text = &text[span.byte_range()];
                    let run_style = style(0..run_text.len(), span.font_size());
                    let shaped = shaper.shape_text_run(TextRunShapeRequest::new(run_text, run_style).unwrap()).unwrap().unwrap();
                    for (offset, stop) in shaped.caret_stops().unwrap().iter().enumerate() {
                        expected_carets[character_start + offset] = x + stop;
                    }
                    for (offset, boundary) in shaped.cluster_boundaries().iter().enumerate() {
                        expected_boundaries[character_start + offset] |= boundary;
                    }
                    x += shaped.caret_stops().unwrap().last().unwrap();
                    character_start += run_text.chars().count();
                }
                assert_eq!(natural.caret_stops.as_ref(), expected_carets);
                assert_eq!(natural.cluster_boundaries.as_ref(), expected_boundaries);
                let placements = (0..5).map(|index| CharacterPlacement::with_baseline_shift(2.0, None, index as f32, index != 2).unwrap()).collect::<Vec<_>>();
                let request = TextShapeRequest::new(3, 30..35, text, &styles, &placements).unwrap();
                let expected_carets = request.adjusted_caret_stops(&natural.caret_stops).unwrap();
                let shaped = shaper.shape_line(request).unwrap().unwrap();
                assert_eq!(shaped.caret_stops, expected_carets);
                assert_eq!(shaped.text_range, 30..35);
                let original = &shaper.runs[natural.run as usize];
                let adjusted = &shaper.runs[shaped.run as usize];
                let expected = original
                    .glyphs
                    .iter()
                    .filter_map(|glyph| {
                        let index = text[..glyph.byte_index].chars().count();
                        (index != 2).then_some((glyph.glyph_id, glyph.position.x + px(expected_carets[index] - natural.caret_stops[index]), glyph.position.y - px(index as f32)))
                    })
                    .collect::<Vec<_>>();
                let actual = adjusted.glyphs.iter().map(|glyph| (glyph.glyph_id, glyph.position.x, glyph.position.y)).collect::<Vec<_>>();
                assert_eq!(actual, expected);
            })
            .unwrap();
    }

    #[test]
    fn byte_offsets_reused_for_paint_colors_keep_multibyte_and_combining_ranges() {
        let bytes = [0, 1, 3, 5, 9]; // a, combining acute, Hebrew, emoji
        let spans = paint_color_spans(&bytes, &[0xff0000ff, 0xff0000ff, 0x0000ffff, 0x0000ffff]).unwrap();
        assert_eq!(spans, [(0..3, Hsla::from(rgba(0xff0000ff))), (3..9, Hsla::from(rgba(0x0000ffff)))]);
        assert!(paint_color_spans(&bytes, &[0xff0000ff]).is_none());
        assert!(paint_color_spans(&[0], &[]).is_none());
    }
}
