use html_engine_model::shaping_contracts;
use html_style_model::open_type;
use crate::fonts::{PublicationFonts, native_slant, publication_faces_for_overrides};
use cssparser::{ParseError, Parser, ParserInput};
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::iter::{once, repeat_n};
use std::mem::take;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use web_time::Instant;

use gpui::{
    App, Font, FontFallbacks, FontId, FontWeight, Global, GlyphId as NativeGlyphId,
    Hsla, LineLayout, Pixels, Point, ShapedGlyph as NativeShapedGlyph,
    ShapedRun as NativeShapedRun, TextRun, Window, WindowTextSystem, font, px, rgba,
};
use html::style::ReaderStyleOverrides;
use html::text::FontFace;
use html_style_model::FontSynthesisStyle;
use html_style_model::TextDirection;
use html_style_model::is_cjk_character;
use html_style_model::OpenTypeFeature;
use html_style_model::OpenTypeFeatureKind;
use html_pipeline::engine::CapsSynthesis;
use html_engine_model::CaretCursor;
use html_engine_model::CaretPositions;
use html_pipeline::engine::FontFeatures;
use html::text::FontMetrics;
use html_pipeline::engine::FontRelativeMetrics;
use html_pipeline::engine::FontLineMetrics;
use html::text::FontSlant;
use html_pipeline::engine::GlyphId;
use html::text::GlyphHandle;
use html_pipeline::engine::TextRunId;
use html::text::GlyphRequest;
use html::text::GlyphResourceGeneration;
use html::text::GlyphShaper;
use html_pipeline::engine::CursiveJoining;
use html_pipeline::engine::NativeResourceCheckpoint;
use html::text::ShapeError;
use html::text::ShapedGlyph;
use html_pipeline::engine::RunFontMetrics;
use html_pipeline::engine::ShapedRun;
use html_pipeline::engine::ShapedTextRun;
use html_pipeline::engine::ShapingStyle;
use html_pipeline::engine::TextRunShapeRequest;
#[cfg(test)]
use html_engine_model::GlyphResourceStore;
use html_engine_model::{ShapedLine, shaping};
use html_fonts::LoadedFontFace;
use kurbo::Rect;
use smallvec::SmallVec;
use unicode_segmentation::UnicodeSegmentation;

mod bounds;
mod caps;

// Caps synthesis can mix font sizes inside the adapter's native display text.
// Engine requests themselves always contain one style and need no byte span.
#[derive(Clone, Debug)]
struct TextStyleSpan<'a> {
    byte_range: Range<usize>,
    style: ShapingStyle<'a>,
}

impl<'a> TextStyleSpan<'a> {
    fn new(byte_range: Range<usize>, style: ShapingStyle<'a>) -> Self {
        Self { byte_range, style }
    }

    fn byte_range(&self) -> Range<usize> {
        self.byte_range.clone()
    }
}

fn requested_font(family: String, weight: u16, slant: FontSlant, stretch: f32) -> Font {
    let mut selected = font(family);
    selected.weight = FontWeight(f32::from(weight));
    let (style, angle) = native_slant(slant);
    selected.style = style;
    selected.variations = gpui::FontVariations::new(stretch, angle).unwrap_or_default();
    selected
}

fn font_feature_flags(features: &FontFeatures<'_>) -> [bool; 25] {
    [
        features.glyph_composition_decomposition,
        features.required_ligatures,
        features.required_contextual_alternates,
        features.mark_positioning,
        features.mark_to_mark_positioning,
        features.kerning,
        features.standard_ligatures,
        features.contextual_ligatures,
        features.discretionary_ligatures,
        features.historical_ligatures,
        features.contextual_alternates,
        features.small_caps,
        features.capitals_to_small_caps,
        features.petite_caps,
        features.capitals_to_petite_caps,
        features.unicase,
        features.titling_capitals,
        features.lining_figures,
        features.oldstyle_figures,
        features.proportional_figures,
        features.tabular_figures,
        features.diagonal_fractions,
        features.stacked_fractions,
        features.ordinals,
        features.slashed_zero,
    ]
}

fn native_font_features(features: &FontFeatures<'_>) -> gpui::FontFeatures {
    fn encode(features: &FontFeatures<'_>) -> gpui::FontFeatures {
        let kinds = [
            OpenTypeFeatureKind::GlyphCompositionDecomposition,
            OpenTypeFeatureKind::RequiredLigatures,
            OpenTypeFeatureKind::RequiredContextualAlternates,
            OpenTypeFeatureKind::MarkPositioning,
            OpenTypeFeatureKind::MarkToMarkPositioning,
            OpenTypeFeatureKind::Kerning,
            OpenTypeFeatureKind::StandardLigatures,
            OpenTypeFeatureKind::ContextualLigatures,
            OpenTypeFeatureKind::DiscretionaryLigatures,
            OpenTypeFeatureKind::HistoricalLigatures,
            OpenTypeFeatureKind::ContextualAlternates,
            OpenTypeFeatureKind::SmallCaps,
            OpenTypeFeatureKind::CapitalsToSmallCaps,
            OpenTypeFeatureKind::PetiteCaps,
            OpenTypeFeatureKind::CapitalsToPetiteCaps,
            OpenTypeFeatureKind::Unicase,
            OpenTypeFeatureKind::TitlingCapitals,
            OpenTypeFeatureKind::LiningFigures,
            OpenTypeFeatureKind::OldstyleFigures,
            OpenTypeFeatureKind::ProportionalFigures,
            OpenTypeFeatureKind::TabularFigures,
            OpenTypeFeatureKind::DiagonalFractions,
            OpenTypeFeatureKind::StackedFractions,
            OpenTypeFeatureKind::Ordinals,
            OpenTypeFeatureKind::SlashedZero,
        ];
        let named = kinds
            .into_iter()
            .zip(font_feature_flags(features))
            .map(|(kind, enabled)| {
                (
                    String::from_utf8_lossy(&open_type::open_type_feature_kind_tag_bytes(kind)).into_owned(),
                    u32::from(enabled),
                )
            });
        let custom = features.custom_features.iter().map(|feature| {
            (
                String::from_utf8_lossy(&open_type::open_type_feature_tag_bytes(*(feature))).into_owned(),
                open_type::open_type_feature_value(*(feature)),
            )
        });
        gpui::FontFeatures::new(named.chain(custom).collect())
    }
    // Share the common translation across font families as well as runs.
    static DEFAULT: OnceLock<gpui::FontFeatures> = OnceLock::new();
    if *features == FontFeatures::DEFAULT {
        DEFAULT
            .get_or_init(|| encode(&FontFeatures::DEFAULT))
            .clone()
    } else {
        encode(features)
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct GlyphKey {
    ch: char,
    font_size: u32,
    font: Font,

}

#[derive(Clone)]
pub(crate) enum GpuiGlyph {
    Empty,
    Layout { layout: Arc<gpui::LineLayout> },
    // Keep the shaped run alive for retained glyph stores. Individual glyph
    // painting resolves its slices, offset, and color from that shared run.
    RunFragment { run: Arc<GpuiTextRun>, range: Range<u32> },
}

/// Immutable paint resources. The shaper copies on write while a frame retains them.
pub(crate) type GlyphStore = Rc<GlyphStorage>;

/// Backend paint resources addressed by the engine's dense character IDs.
/// A shaped run occupies one record covering all of its character IDs.
/// Individual-character resources remain available for fallback shaping.
#[derive(Clone, Default)]
pub(crate) struct GlyphStorage {
    records: Vec<GlyphRecord>,
    len: usize,
}

#[derive(Clone)]
struct GlyphRecord {
    start: usize,
    glyph: GpuiGlyph,
}

impl GlyphStorage {
    fn truncate(&mut self, len: usize) {
        self.records
            .truncate(self.records.partition_point(|record| record.start < len));
        self.len = len;
        if let Some(GlyphRecord {
            start,
            glyph: GpuiGlyph::RunFragment { range, .. },
        }) = self.records.last_mut()
        {
            range.end = range.start + (len - *start) as u32;
        }
    }
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

/// ASCII character boundaries coincide with byte offsets. Retain a range for
/// those runs instead of allocating an integer for every character boundary.
/// Other text uses two- or four-byte offsets when they cover the complete UTF-8
/// run including its end boundary, and machine-sized offsets for larger runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CharacterBytes<W = Arc<[usize]>, N = Arc<[u32]>, S = Arc<[u16]>> {
    Ascii { start: usize, end: usize },
    Utf8(W),
    Utf8Compact(N),
    Utf8Narrow(S),
}

type BorrowedCharacterBytes<'a> = CharacterBytes<&'a [usize], &'a [u32], &'a [u16]>;

impl CharacterBytes {
    pub(crate) fn new(text: &str) -> Self {
        if text.is_ascii() {
            Self::Ascii { start: 0, end: text.len() + 1 }
        } else {
            // An exact-size iterator lets Arc allocate the final slice directly,
            // without growing and then copying a temporary offset vector.
            let boundaries = text.chars().count() + 1;
            let mut offsets = text.char_indices();
            if u16::try_from(text.len()).is_ok() {
                // Include the end boundary in the checked range as well.
                Self::Utf8Narrow((0..boundaries).map(|_| offsets.next().map_or(text.len(), |(byte, _)| byte) as u16).collect())
            } else if u32::try_from(text.len()).is_ok() {
                // Every character boundary is at most the checked UTF-8 length.
                Self::Utf8Compact((0..boundaries).map(|_| offsets.next().map_or(text.len(), |(byte, _)| byte) as u32).collect())
            } else {
                Self::Utf8((0..boundaries).map(|_| offsets.next().map_or(text.len(), |(byte, _)| byte)).collect())
            }
        }
    }
}

impl<W: AsRef<[usize]>, N: AsRef<[u32]>, S: AsRef<[u16]>> CharacterBytes<W, N, S> {
    fn len(&self) -> usize {
        match self {
            Self::Ascii { start, end } => end - start,
            Self::Utf8(bytes) => bytes.as_ref().len(),
            Self::Utf8Compact(bytes) => bytes.as_ref().len(),
            Self::Utf8Narrow(bytes) => bytes.as_ref().len(),
        }
    }

    fn get(&self, character: usize) -> Option<usize> {
        match self {
            Self::Ascii { start, end } => (character < end - start).then(|| start + character),
            Self::Utf8(bytes) => bytes.as_ref().get(character).copied(),
            Self::Utf8Compact(bytes) => bytes.as_ref().get(character).map(|byte| *byte as usize),
            Self::Utf8Narrow(bytes) => bytes.as_ref().get(character).map(|byte| usize::from(*byte)),
        }
    }

    fn lower_bound(&self, byte: usize) -> usize {
        match self {
            Self::Ascii { start, end } => byte.saturating_sub(*start).min(end - start),
            Self::Utf8(bytes) => bytes.as_ref().partition_point(|offset| *offset < byte),
            Self::Utf8Compact(bytes) => bytes.as_ref().partition_point(|offset| (*offset as usize) < byte),
            Self::Utf8Narrow(bytes) => bytes.as_ref().partition_point(|offset| usize::from(*offset) < byte),
        }
    }

    fn character_at_or_before(&self, byte: usize) -> usize {
        match self {
            Self::Ascii { start, end } => byte.saturating_sub(*start).min((end - start).saturating_sub(1)),
            Self::Utf8(bytes) => bytes.as_ref().partition_point(|offset| *offset <= byte).saturating_sub(1),
            Self::Utf8Compact(bytes) => bytes.as_ref().partition_point(|offset| (*offset as usize) <= byte).saturating_sub(1),
            Self::Utf8Narrow(bytes) => bytes.as_ref().partition_point(|offset| usize::from(*offset) <= byte).saturating_sub(1),
        }
    }

    #[inline]
    fn find_from(&self, byte: usize, character: &mut usize) -> Option<usize> {
        if let Self::Ascii { start, end } = self {
            return (*start..*end).contains(&byte).then(|| byte - *start);
        }
        self.find_mapped_from(byte, character)
    }

    fn find_mapped_from(&self, byte: usize, character: &mut usize) -> Option<usize> {
        // Walk nearby boundaries, then use binary search for larger jumps or
        // visually reordered glyphs. The bounded walk avoids quadratic scans.
        for _ in 0..4 {
            match self.get(*character) {
                Some(offset) if offset == byte => return Some(*character),
                Some(offset) if offset < byte => *character += 1,
                _ => break,
            }
        }
        *character = self.lower_bound(byte);
        (self.get(*character) == Some(byte)).then_some(*character)
    }

    fn slice(&self, characters: Range<usize>) -> BorrowedCharacterBytes<'_> {
        assert!(characters.start <= characters.end && characters.end <= self.len());
        match self {
            Self::Ascii { start, .. } => CharacterBytes::Ascii { start: start + characters.start, end: start + characters.end },
            Self::Utf8(bytes) => CharacterBytes::Utf8(&bytes.as_ref()[characters]),
            Self::Utf8Compact(bytes) => CharacterBytes::Utf8Compact(&bytes.as_ref()[characters]),
            Self::Utf8Narrow(bytes) => CharacterBytes::Utf8Narrow(&bytes.as_ref()[characters]),
        }
    }
}

// Retained paint storage keeps native positions and flags unchanged.
// Large IDs or byte indices keep the original representation for the whole run.
#[derive(Clone, Copy)]
#[repr(align(8))]
struct CompactNativeGlyph {
    position: Point<Pixels>,
    index: u32,
    id: u16,
    is_emoji: bool,
}

// Two glyphs share one y coordinate in the owning run. The pair retains the
// native glyph's size and alignment so collection can reuse its allocation.
#[derive(Clone, Copy)]
#[repr(align(8))]
struct CompactNativeGlyphPair {
    x: [Pixels; 2],
    index: [u32; 2],
    id: [u16; 2],
    is_emoji: [bool; 2],
}

// Most paragraph runs fit 16-bit byte indices and share their emoji flag.
// Native alignment lets collection reuse the original glyph allocation.
#[derive(Clone, Copy)]
#[repr(align(8))]
struct NarrowNativeGlyph {
    x: Pixels,
    index: u16,
    id: u16,
}

// Eligible glyph positions and byte indices already exist in the run's
// caret/character tables. Two character-index/ID pairs share native alignment.
#[derive(Clone, Copy)]
#[repr(align(8))]
struct CaretNativeGlyphPair {
    character: [u16; 2],
    id: [u16; 2],
}

impl CaretNativeGlyphPair {
    fn new(first: (u16, u16), second: Option<(u16, u16)>) -> Self {
        let second = second.unwrap_or((0, 0));
        Self {
            character: [first.0, second.0],
            id: [first.1, second.1],
        }
    }
}

impl CompactNativeGlyphPair {
    fn new(first: NativeShapedGlyph, second: Option<NativeShapedGlyph>) -> Self {
        let mut pair = Self { x: [first.position.x, Pixels::ZERO], index: [first.index as u32, 0], id: [first.id.0 as u16, 0], is_emoji: [first.is_emoji, false] };
        if let Some(second) = second {
            pair.x[1] = second.position.x;
            pair.index[1] = second.index as u32;
            pair.id[1] = second.id.0 as u16;
            pair.is_emoji[1] = second.is_emoji;
        }
        pair
    }
}

enum NativeGlyphs {
    Horizontal { pairs: Box<[CompactNativeGlyphPair]>, y: Pixels, odd: bool },
    Compact(Box<[CompactNativeGlyph]>),
    Full(Box<[NativeShapedGlyph]>),
    NarrowHorizontal {
        glyphs: Box<[NarrowNativeGlyph]>,
        y: Pixels,
        is_emoji: bool,
    },
    CaretHorizontal {
        pairs: Box<[CaretNativeGlyphPair]>,
        y: Pixels,
        is_emoji: bool,
        odd: bool,
    },
}

impl NativeGlyphs {
    fn new(glyphs: Vec<NativeShapedGlyph>, bytes: &CharacterBytes, carets: &CaretPositions) -> Self {
        if !glyphs
            .iter()
            .all(|glyph| u32::try_from(glyph.index).is_ok() && u16::try_from(glyph.id.0).is_ok())
        {
            return Self::Full(glyphs.into_boxed_slice());
        }
        if let Some((y, is_emoji)) = glyphs.first().map(|glyph| (glyph.position.y, glyph.is_emoji))
            && glyphs.iter().all(|glyph| f32::from(glyph.position.y).to_bits() == f32::from(y).to_bits())
        {
            if glyphs.len() >= 2 && bytes.len() <= usize::from(u16::MAX) + 2 {
                let mut character_cursor = 0;
                let mut caret_cursor = carets.cursor();
                let matches_carets = glyphs.iter().all(|glyph| {
                    glyph.is_emoji == is_emoji
                        && bytes
                            .find_from(glyph.index, &mut character_cursor)
                            .is_some_and(|index| {
                                u16::try_from(index).is_ok()
                                    && caret_cursor.get(index).is_some_and(|x| {
                                        x.to_bits() == f32::from(glyph.position.x).to_bits()
                                    })
                            })
                });
                if matches_carets {
                    let len = glyphs.len();
                    let mut character_cursor = 0;
                    let mut first = None;
                    let pairs = glyphs
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, glyph)| {
                            let character = bytes
                                .find_from(glyph.index, &mut character_cursor)?
                                as u16;
                            let value = (character, glyph.id.0 as u16);
                            if let Some(first) = first.take() {
                                Some(CaretNativeGlyphPair::new(first, Some(value)))
                            } else if index + 1 == len {
                                Some(CaretNativeGlyphPair::new(value, None))
                            } else {
                                first = Some(value);
                                None
                            }
                        })
                        .collect();
                    return Self::CaretHorizontal {
                        pairs,
                        y,
                        is_emoji,
                        odd: len % 2 != 0,
                    };
                }
            }
            if glyphs
                .iter()
                .all(|glyph| u16::try_from(glyph.index).is_ok() && glyph.is_emoji == is_emoji)
            {
                let glyphs = glyphs
                    .into_iter()
                    .map(|glyph| NarrowNativeGlyph {
                        x: glyph.position.x,
                        index: glyph.index as u16,
                        id: glyph.id.0 as u16,
                    })
                    .collect();
                return Self::NarrowHorizontal {
                    glyphs,
                    y,
                    is_emoji,
                };
            }
            if glyphs.len() >= 4 {
                let len = glyphs.len();
                let mut first = None;
                let pairs = glyphs.into_iter().enumerate().filter_map(|(index, glyph)| {
                    if let Some(first) = first.take() {
                        Some(CompactNativeGlyphPair::new(first, Some(glyph)))
                    } else if index + 1 == len {
                        Some(CompactNativeGlyphPair::new(glyph, None))
                    } else {
                        first = Some(glyph);
                        None
                    }
                }).collect();
                return Self::Horizontal { pairs, y, odd: len % 2 != 0 };
            }
        }
        // Matching native alignment permits safe in-place Vec collection.
        Self::Compact(glyphs.into_iter().map(|glyph| CompactNativeGlyph {
            position: glyph.position,
            index: glyph.index as u32,
            id: glyph.id.0 as u16,
            is_emoji: glyph.is_emoji,
        }).collect())
    }

    fn len(&self) -> usize {
        match self {
            Self::Horizontal { pairs, odd, .. } => pairs.len() * 2 - usize::from(*odd),
            Self::Compact(glyphs) => glyphs.len(),
            Self::Full(glyphs) => glyphs.len(),
            Self::NarrowHorizontal { glyphs, .. } => glyphs.len(),
            Self::CaretHorizontal { pairs, odd, .. } => pairs.len() * 2 - usize::from(*odd),
        }
    }

    #[inline(always)]
    fn glyph(&self, index: usize, bytes: &CharacterBytes, carets: &mut CaretCursor<'_>) -> Option<NativeShapedGlyph> {
        Some(match self {
            Self::Horizontal { pairs, y, odd } => {
                // The final pair can contain one padding slot, which is never a glyph.
                assert!(index < pairs.len() * 2 - usize::from(*odd));
                let pair = &pairs[index / 2];
                let slot = index % 2;
                NativeShapedGlyph { id: NativeGlyphId(u32::from(pair.id[slot])), position: Point { x: pair.x[slot], y: *y }, index: pair.index[slot] as usize, is_emoji: pair.is_emoji[slot] }
            }
            Self::Compact(glyphs) => {
                let glyph = glyphs[index];
                NativeShapedGlyph { id: NativeGlyphId(u32::from(glyph.id)), position: glyph.position, index: glyph.index as usize, is_emoji: glyph.is_emoji }
            }
            Self::CaretHorizontal {
                pairs,
                y,
                is_emoji,
                odd,
            } => {
                assert!(index < pairs.len() * 2 - usize::from(*odd));
                let pair = &pairs[index / 2];
                let slot = index % 2;
                let character = usize::from(pair.character[slot]);
                NativeShapedGlyph {
                    id: NativeGlyphId(u32::from(pair.id[slot])),
                    position: Point {
                        x: px(carets.get(character)?),
                        y: *y,
                    },
                    index: bytes.get(character)?,
                    is_emoji: *is_emoji,
                }
            }
            Self::Full(glyphs) => glyphs[index].clone(),
            Self::NarrowHorizontal { glyphs, y, is_emoji } => {
                let glyph = glyphs[index];
                NativeShapedGlyph { id: NativeGlyphId(u32::from(glyph.id)), position: Point { x: glyph.x, y: *y }, index: usize::from(glyph.index), is_emoji: *is_emoji }
            }
        })
    }

    fn partition_point(
        &self,
        range: Range<usize>,
        bytes: &CharacterBytes,
        mut predicate: impl FnMut(usize) -> bool,
    ) -> usize {
        match self {
            Self::Horizontal { pairs, odd, .. } => {
                assert!(range.start <= range.end && range.end <= pairs.len() * 2 - usize::from(*odd));
                let start = range.start;
                let mut left = range.start;
                let mut right = range.end;
                while left < right {
                    let middle = left + (right - left) / 2;
                    if predicate(pairs[middle / 2].index[middle % 2] as usize) {
                        left = middle + 1;
                    } else {
                        right = middle;
                    }
                }
                left - start
            }
            Self::CaretHorizontal { pairs, odd, .. } => {
                assert!(
                    range.start <= range.end && range.end <= pairs.len() * 2 - usize::from(*odd)
                );
                let start = range.start;
                let mut left = range.start;
                let mut right = range.end;
                while left < right {
                    let middle = left + (right - left) / 2;
                    let character = usize::from(pairs[middle / 2].character[middle % 2]);
                    let Some(byte) = bytes.get(character) else {
                        return left - start;
                    };
                    if predicate(byte) {
                        left = middle + 1;
                    } else {
                        right = middle;
                    }
                }
                left - start
            }
            Self::Compact(glyphs) => glyphs[range].partition_point(|glyph| predicate(glyph.index as usize)),
            Self::Full(glyphs) => glyphs[range].partition_point(|glyph| predicate(glyph.index)),
            Self::NarrowHorizontal { glyphs, .. } => glyphs[range].partition_point(|glyph| predicate(usize::from(glyph.index))),
        }
    }
}

type FontSpans = SmallVec<[GpuiFontSpan; 1]>;
type ColorSpans = SmallVec<[(Range<usize>, Hsla); 1]>;
type FontRuns = SmallVec<[TextRun; 1]>;

// Most runs cover one font, color and byte-order segment. Keep that metadata
// with the run; mixed styling or direction retains the complete shared arrays.
enum RunMetadata {
    Uniform { font_id: FontId, font_size: Pixels, color: Hsla, order: GlyphByteOrder },
    Detailed { font_spans: Arc<[GpuiFontSpan]>, glyph_segments: Arc<[GpuiGlyphSegment]>, colors: Box<[(Range<usize>, Hsla)]> },
}

impl RunMetadata {
    fn uniform(fonts: &[GpuiFontSpan], segments: &[GpuiGlyphSegment], colors: &[(Range<usize>, Hsla)], glyph_count: usize, byte_len: usize) -> Option<Self> {
        let ([font], [segment], [(range, color)]) = (fonts, segments, colors) else { return None };
        (font.glyphs == (0..glyph_count) && segment.glyphs == (0..glyph_count) && *range == (0..byte_len))
            .then_some(Self::Uniform { font_id: font.font_id, font_size: font.font_size, color: *color, order: segment.order })
    }

    fn new(
        glyphs: &[NativeShapedGlyph],
        font_spans: FontSpans,
        colors: ColorSpans,
        byte_len: usize,
    ) -> Self {
        let glyph_segments = build_glyph_segments(glyphs);
        Self::uniform(
            &font_spans,
            &glyph_segments,
            &colors,
            glyphs.len(),
            byte_len,
        )
        .unwrap_or_else(|| Self::Detailed {
            font_spans: Arc::from(font_spans.as_slice()),
            glyph_segments: Arc::from(glyph_segments.as_slice()),
            colors: colors.into_boxed_slice(),
        })
    }

    fn with_colors(&self, colors: Vec<(Range<usize>, Hsla)>, glyph_count: usize, byte_len: usize) -> Self {
        match self {
            Self::Uniform { font_id, font_size, order, .. } => {
                let fonts = [GpuiFontSpan { glyphs: 0..glyph_count, font_id: *font_id, font_size: *font_size }];
                let segments = [GpuiGlyphSegment { glyphs: 0..glyph_count, order: *order }];
                Self::uniform(&fonts, &segments, &colors, glyph_count, byte_len)
                    .unwrap_or_else(|| Self::Detailed { font_spans: Arc::from(fonts), glyph_segments: Arc::from(segments), colors: colors.into_boxed_slice() })
            }
            Self::Detailed { font_spans, glyph_segments, .. } => {
                Self::uniform(font_spans, glyph_segments, &colors, glyph_count, byte_len)
                    .unwrap_or_else(|| Self::Detailed { font_spans: font_spans.clone(), glyph_segments: glyph_segments.clone(), colors: colors.into_boxed_slice() })
            }
        }
    }

    fn font_spans_in_range(&self, range: Range<usize>, glyph_count: usize) -> impl Iterator<Item = GpuiFontSpan> + '_ {
        let (inline, spans): (Option<GpuiFontSpan>, &[GpuiFontSpan]) = match self {
            Self::Uniform { font_id, font_size, .. } => ((glyph_count > range.start).then_some(GpuiFontSpan { glyphs: 0..glyph_count, font_id: *font_id, font_size: *font_size }), &[]),
            Self::Detailed { font_spans, .. } => {
                let first = font_spans.partition_point(|span| span.glyphs.end <= range.start);
                (None, &font_spans[first..])
            }
        };
        inline.into_iter().chain(spans.iter().cloned()).take_while(move |span| span.glyphs.start < range.end)
    }

    fn glyph_segments(&self, glyph_count: usize) -> impl Iterator<Item = GpuiGlyphSegment> + '_ {
        let (inline, segments): (Option<GpuiGlyphSegment>, &[GpuiGlyphSegment]) = match self {
            Self::Uniform { order, .. } => (Some(GpuiGlyphSegment { glyphs: 0..glyph_count, order: *order }), &[]),
            Self::Detailed { glyph_segments, .. } => (None, glyph_segments),
        };
        inline.into_iter().chain(segments.iter().cloned())
    }

    fn color_spans(&self, byte_len: usize) -> impl Iterator<Item = (Range<usize>, Hsla)> + '_ {
        let (inline, colors): (Option<(Range<usize>, Hsla)>, &[(Range<usize>, Hsla)]) = match self {
            Self::Uniform { color, .. } => (Some((0..byte_len, *color)), &[]),
            Self::Detailed { colors, .. } => (None, colors),
        };
        inline.into_iter().chain(colors.iter().cloned())
    }
}

pub(crate) struct GpuiTextRun {
    // Retained scenes can share immutable glyphs while keeping separate colors.
    glyphs: Arc<NativeGlyphs>,
    metadata: RunMetadata,
    pub(crate) ascent: gpui::Pixels,
    pub(crate) descent: gpui::Pixels,
    character_bytes: CharacterBytes,
    pub(crate) caret_stops: CaretPositions,
    pub(crate) paint_bounds: Option<Rect>,
}

impl GpuiTextRun {
    pub(crate) fn new(
        glyphs: Vec<NativeShapedGlyph>,
        font_spans: FontSpans,
        ascent: gpui::Pixels,
        descent: gpui::Pixels,
        character_bytes: CharacterBytes,
        colors: ColorSpans,
        caret_stops: CaretPositions,
    ) -> Self {
        let byte_len = character_bytes
            .get(character_bytes.len().saturating_sub(1))
            .unwrap_or(0);
        let metadata = RunMetadata::new(&glyphs, font_spans, colors, byte_len);
        Self {
            glyphs: Arc::new(NativeGlyphs::new(glyphs, &character_bytes, &caret_stops)),
            metadata,
            ascent,
            descent,
            character_bytes,
            caret_stops,
            paint_bounds: None,
        }
    }

    #[inline(always)]
    fn native_glyph(&self, index: usize, carets: &mut CaretCursor<'_>) -> Option<NativeShapedGlyph> {
        self.glyphs
            .glyph(index, &self.character_bytes, carets)
    }

    #[inline]
    fn partition_glyphs(&self, range: Range<usize>, predicate: impl FnMut(usize) -> bool) -> usize {
        self.glyphs
            .partition_point(range, &self.character_bytes, predicate)
    }

    fn with_colors(&self, colors: Vec<(Range<usize>, Hsla)>) -> Self {
        Self {
            glyphs: Arc::clone(&self.glyphs),
            metadata: self.metadata.with_colors(colors, self.glyphs.len(), self.byte_len()),
            ascent: self.ascent,
            descent: self.descent,
            character_bytes: self.character_bytes.clone(),
            caret_stops: self.caret_stops.clone(),
            paint_bounds: self.paint_bounds,
        }
    }

    fn native_paint_bounds(glyphs: &[NativeShapedGlyph], font_spans: &[GpuiFontSpan], text_system: &WindowTextSystem) -> Option<Rect> {
        // A font's glyph boxes share the same extents. Their union depends
        // only on the extreme origins, available before glyph compression.
        // This avoids reconstructing every compact caret just for culling.
        let mut previous_font = None;
        let mut font_bounds = Rect::ZERO;
        let mut paint_bounds: Option<Rect> = None;
        for span in font_spans {
            let glyphs = glyphs.get(span.glyphs.clone())?;
            if glyphs.is_empty() {
                continue;
            }
            let key = (span.font_id, span.font_size);
            if previous_font != Some(key) {
                let bounds = text_system.bounding_box(span.font_id, span.font_size);
                font_bounds = Rect::new(f64::from(f32::from(bounds.left())), f64::from(f32::from(bounds.top())), f64::from(f32::from(bounds.right())), f64::from(f32::from(bounds.bottom())));
                previous_font = Some(key);
            }
            let mut x0 = f64::INFINITY;
            let mut y0 = f64::INFINITY;
            let mut x1 = f64::NEG_INFINITY;
            let mut y1 = f64::NEG_INFINITY;
            for glyph in glyphs {
                let x = f64::from(f32::from(glyph.position.x));
                let y = f64::from(f32::from(glyph.position.y));
                if !x.is_finite() || !y.is_finite() {
                    return None;
                }
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            let first = bounds::conservative_glyph_bounds(font_bounds, (x0, y0))?;
            let last = bounds::conservative_glyph_bounds(font_bounds, (x1, y1))?;
            let bounds = first.union(last);
            paint_bounds = Some(paint_bounds.map_or(bounds, |previous| previous.union(bounds)));
        }
        paint_bounds
    }

    pub(crate) fn iter_glyphs(&self) -> impl Iterator<Item = GpuiRunGlyph> + '_ {
        self.glyphs_in_range(0..self.glyphs.len())
    }

    pub(crate) fn glyphs_in_range(&self, range: Range<usize>) -> impl Iterator<Item = GpuiRunGlyph> + '_ {
        self.metadata.font_spans_in_range(range.clone(), self.glyphs.len())
            .flat_map(move |span| {
                let start = span.glyphs.start.max(range.start);
                let end = span.glyphs.end.min(range.end);
                let mut carets = self.caret_stops.cursor();
                (start..end).filter_map(move |index| {
                    let glyph = self.native_glyph(index, &mut carets)?;
                    Some(GpuiRunGlyph {
                        font_id: span.font_id,
                        font_size: span.font_size,
                        glyph_id: glyph.id,
                        position: glyph.position,
                        byte_index: glyph.index,
                        is_emoji: glyph.is_emoji,
                    })
                })
            })
    }

    /// Resolve a logical character range to slices of the visually ordered
    /// glyph array. Most text produces one slice. Mixed-direction text may
    /// produce several, which remain in the exact order GPUI shaped them.
    pub(crate) fn glyph_ranges(&self, range: Range<u32>) -> SmallVec<[Range<usize>; 2]> {
        let start = self.character_bytes.get(range.start as usize).unwrap_or(0);
        let end = self.character_bytes.get(range.end as usize).unwrap_or(usize::MAX);
        if start >= end {
            return SmallVec::new();
        }

        let mut ranges = SmallVec::<[Range<usize>; 2]>::new();
        for segment in self.metadata.glyph_segments(self.glyphs.len()) {
            let range = segment.glyphs.clone();
            let (local_start, local_end) = match segment.order {
                GlyphByteOrder::Ascending => (
                    self.partition_glyphs(range.clone(), |index| index < start),
                    self.partition_glyphs(range, |index| index < end),
                ),
                GlyphByteOrder::Descending => (
                    self.partition_glyphs(range.clone(), |index| index >= end),
                    self.partition_glyphs(range, |index| index >= start),
                ),
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

    fn byte_len(&self) -> usize {
        self.character_bytes.get(self.character_bytes.len().saturating_sub(1)).unwrap_or(0)
    }

    fn color_spans(&self) -> impl Iterator<Item = (Range<usize>, Hsla)> + '_ {
        self.metadata.color_spans(self.byte_len())
    }

    pub(crate) fn color_at(&self, byte_index: usize) -> Hsla {
        self.color_spans().find(|(range, _)| range.contains(&byte_index)).map(|(_, color)| color).unwrap_or_else(|| Hsla::from(rgba(0x000000ff)))
    }

    pub(crate) fn character_color(&self, character: u32) -> Hsla {
        self.character_bytes.get(character as usize).map(|byte| self.color_at(byte)).unwrap_or_else(|| Hsla::from(rgba(0x000000ff)))
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

fn build_glyph_segments(glyphs: &[NativeShapedGlyph]) -> SmallVec<[GpuiGlyphSegment; 1]> {
    if glyphs.is_empty() {
        return SmallVec::new();
    }

    let mut segments = SmallVec::new();
    let mut start = 0usize;
    let mut order = None;
    for index in 1..glyphs.len() {
        let next_order = match glyphs[index].index.cmp(&glyphs[index - 1].index) {
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

#[derive(Clone)]
pub(crate) struct GpuiFontSpan {
    pub(crate) glyphs: Range<usize>,
    pub(crate) font_id: FontId,
    pub(crate) font_size: Pixels,
}

pub(crate) type RunStore = Rc<Vec<Arc<GpuiTextRun>>>;

/// One immutable native-text snapshot, captured from a shaper in one operation.
/// Retained snapshots survive later shaping through copy-on-write storage.
#[derive(Clone)]
pub struct GpuiTextResources {
    pub(crate) generation: GlyphResourceGeneration,
    pub(crate) glyphs: GlyphStore,
    pub(crate) runs: RunStore,
}

impl GpuiTextResources {
    pub fn generation(&self) -> GlyphResourceGeneration {
        self.generation
    }
}

/// Lightweight native resources for replay. Recorded text batches already own
/// their runs, so this snapshot need not pin the shaper's run registry.
#[repr(transparent)]
#[derive(Clone)]
pub(crate) struct GpuiPaintResources(pub(crate) GlyphStore);

#[derive(Clone, Copy)]
struct CachedGlyph {
    handle: GlyphHandle,
    advance: f32,
    metrics: FontMetrics,
}

struct DocumentTextResources {
    generation: GlyphResourceGeneration,
    table: HashMap<GlyphKey, CachedGlyph>,
    glyphs: GlyphStore,
    runs: RunStore,
}

struct AppendTextResources {
    glyphs: usize,
    runs: usize,
    recolored: Vec<(usize, Arc<GpuiTextRun>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FontStyleIdentity {
    font_weight: u16,
    font_slant: FontSlant,
    synthesis_style: FontSynthesisStyle,
    font_stretch: u32,
    family: Option<Box<str>>,
    cjk_family: Option<Box<str>>,
    feature_flags: [bool; 25],
    custom_features: Box<[OpenTypeFeature]>,
}

impl FontStyleIdentity {
    fn matches(&self, style: &ShapingStyle<'_>, feature_flags: &[bool; 25]) -> bool {
        self.font_weight == style.font_weight
            && self.font_slant == style.font_slant
            && self.synthesis_style == style.font_synthesis_style
            && self.font_stretch == style.font_stretch.to_bits()
            && self.family.as_deref() == style.font_family
            && self.cjk_family.as_deref() == style.cjk_font_family
            && &self.feature_flags == feature_flags
            && self.custom_features.as_ref() == style.font_features.custom_features
    }
}

#[derive(Clone)]
struct PreparedFont {
    identity: Arc<FontStyleIdentity>,
    font: Font,
    cjk_font: Option<Font>,
}

fn trailing_whitespace_profiling_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("BOKHEIM_PROFILE_TEXT").is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes")))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextShapeCacheStats {
    pub glyph_shapes: u64,
    pub line_shapes: u64,
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

fn append_font_runs(runs: &mut FontRuns, text: &str, latin: Font, cjk: Option<Font>, color: Hsla) {
    let push = |runs: &mut FontRuns, len, font| runs.push(TextRun { len, font, color, background_color: None, underline: None, strikethrough: None });
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

/// Application font loading requested before family resolution or text shaping.
#[derive(Clone, Copy, Debug)]
pub enum FontLoadRequest<'a> {
    /// Make this family available. Return true when it is available to GPUI.
    Family(&'a str),
    /// Load fallbacks for this text. Return true if new fonts were registered.
    TextFallbacks(&'a str),
}

/// Host font-loading policy. The request determines the meaning of the result.
pub type FontLoader = Rc<dyn Fn(&WindowTextSystem, FontLoadRequest<'_>) -> bool>;

struct ApplicationFontLoader(FontLoader);
impl Global for ApplicationFontLoader {}

/// Share the host's bundled-font policy with book and continuous HTML views.
pub fn configure_font_loader(loader: FontLoader, cx: &mut App) {
    cx.set_global(ApplicationFontLoader(loader));
}

pub struct GpuiTextCache {
    font_loader: Option<FontLoader>,
    text_system: Arc<WindowTextSystem>,
    font_families: HashMap<String, String>,
    system_font_families_loaded: bool,
    publication_fonts: PublicationFonts,
    /// Set by selection or registration; cleared caches are rebuilt on the
    /// next shaping transaction.
    publication_fonts_changed: bool,
    prepared_fonts: VecDeque<PreparedFont>,
    native_caps: HashMap<(FontId, char, &'static str), bool>,
    generation: GlyphResourceGeneration,
    table: HashMap<GlyphKey, CachedGlyph>,
    glyphs: GlyphStore,
    runs: RunStore,
    previous_document: Option<DocumentTextResources>,
    previous_append: Option<AppendTextResources>,
    shape_cache_stats: TextShapeCacheStats,
}

impl GpuiTextCache {
    pub fn new(text_system: Arc<WindowTextSystem>) -> Self {
        let font_families = text_system.registered_font_names().into_iter().map(|family| (family.to_lowercase(), family)).collect();
        Self {
            text_system,
            font_loader: None,
            font_families,
            system_font_families_loaded: false,
            publication_fonts: PublicationFonts::default(),
            publication_fonts_changed: false,
            prepared_fonts: VecDeque::new(),
            native_caps: HashMap::new(),
            generation: GlyphResourceGeneration::default(),
            table: HashMap::new(),
            glyphs: Rc::default(),
            runs: Rc::new(Vec::new()),
            previous_document: None,
            previous_append: None,
            shape_cache_stats: TextShapeCacheStats::default(),
        }
    }

    pub fn for_window(window: &Window, cx: &App) -> Self {
        Self::new(window.text_system().clone())
            .with_font_loader(cx.try_global::<ApplicationFontLoader>().map(|loader| loader.0.clone()))
    }

    pub fn with_font_loader(mut self, loader: Option<FontLoader>) -> Self {
        self.font_loader = loader;
        self
    }

    fn ensure_text_fonts(&mut self, text: &str) {
        if self
            .font_loader
            .as_ref()
            .is_some_and(|loader| loader(&self.text_system, FontLoadRequest::TextFallbacks(text)))
        {
            for family in self.text_system.registered_font_names() {
                self.font_families.insert(family.to_lowercase(), family);
            }
            // A previously prepared fallback may refer to a different face.
            self.prepared_fonts.clear();
            self.native_caps.clear();
            self.table.clear();
        }
    }

    /// Registers a loaded publication face with the native text system.
    /// Returns whether the face was new; callers then refresh shaping.
    pub fn register_publication_font(&mut self, loaded: LoadedFontFace) -> Result<bool, String> {
        let registered = self.publication_fonts.register(loaded, &self.text_system)?;
        self.publication_fonts_changed |= registered;
        Ok(registered)
    }

    fn refresh_publication_fonts(&mut self) {
        if take(&mut self.publication_fonts_changed) {
            self.prepared_fonts.clear();
            self.native_caps.clear();
            self.table.clear();
        }
    }

    pub fn resources(&self) -> GpuiTextResources {
        GpuiTextResources {
            generation: self.generation,
            glyphs: self.glyphs.clone(),
            runs: self.runs.clone(),
        }
    }

    pub fn shape_cache_stats(&self) -> TextShapeCacheStats {
        TextShapeCacheStats { retained_font_setups: self.prepared_fonts.len(), registered_glyphs: self.glyphs.len(), backend_glyph_records: self.glyphs.records.len(), ..self.shape_cache_stats }
    }

    fn available_family(&mut self, candidates: &[&str]) -> Option<String> {
        candidates.iter().find_map(|candidate| self.family_name(candidate))
    }

    fn family_name(&mut self, family: &str) -> Option<String> {
        let key = family.to_lowercase();
        if let Some(family) = self.font_families.get(&key) {
            return Some(family.clone());
        }
        if self
            .font_loader
            .as_ref()
            .is_some_and(|loader| loader(&self.text_system, FontLoadRequest::Family(family)))
        {
            // Native backends resolve registered spelling, while CSS family
            // matching is case-insensitive. Refresh the canonical names.
            for registered in self.text_system.registered_font_names() {
                self.font_families.insert(registered.to_lowercase(), registered);
            }
            if let Some(registered) = self.font_families.get(&key) {
                return Some(registered.clone());
            }
        }
        if !self.system_font_families_loaded {
            for family in self.text_system.all_font_names() {
                self.font_families.insert(family.to_lowercase(), family);
            }
            self.system_font_families_loaded = true;
        }
        self.font_families.get(&key).cloned()
    }

    fn resolve_font(
        &mut self,
        family_list: Option<&str>,
        weight: u16,
        slant: FontSlant,
        stretch: f32,
        synthesis: FontSynthesisStyle,
    ) -> Font {
        self.resolve_font_for_script(family_list, weight, slant, false, stretch, synthesis)
    }

    fn resolve_font_for_script(
        &mut self,
        family_list: Option<&str>,
        weight: u16,
        slant: FontSlant,
        cjk: bool,
        stretch: f32,
        synthesis: FontSynthesisStyle,
    ) -> Font {
        let allow_publication = if cjk {
            self.publication_fonts.allows_cjk_publication_fonts()
        } else {
            self.publication_fonts
                .allows_publication_family_list(family_list)
        };
        let mut resolved = family_list
            .into_iter()
            .flat_map(font_family_names)
            .filter_map(|(family, quoted)| {
                self.resolve_family(
                    &family,
                    quoted,
                    weight,
                    slant,
                    allow_publication,
                    stretch,
                    synthesis,
                )
            });
        let mut selected = resolved
            .next()
            .unwrap_or_else(|| requested_font(".SystemUIFont".to_owned(), weight, slant, stretch));
        // Keep CSS fallback families for glyphs missing from a custom face.
        // Font setup is cached across lines; single-family requests allocate no chain.
        let mut fallbacks = selected.fallbacks.as_ref().map(|fallbacks| fallbacks.fallback_list().to_vec()).unwrap_or_default();
        for candidate in resolved {
            for family in once(candidate.family.as_ref()).chain(candidate.fallbacks.as_ref().into_iter().flat_map(|fallbacks| fallbacks.fallback_list().iter().map(String::as_str))) {
                if family != selected.family.as_ref() && !fallbacks.iter().any(|existing| existing == family) {
                    fallbacks.push(family.to_owned());
                }
            }
        }
        if !fallbacks.is_empty() {
            selected.fallbacks = Some(FontFallbacks::from_fonts(fallbacks));
        }
        if synthesis == FontSynthesisStyle::None {
            selected.variations = selected.variations.with_synthesis(false);
        }
        selected
    }

    fn resolve_family(
        &mut self,
        family: &str,
        quoted: bool,
        weight: u16,
        slant: FontSlant,
        allow_publication: bool,
        stretch: f32,
        synthesis: FontSynthesisStyle,
    ) -> Option<Font> {
        let generic_name = !quoted
            && matches!(family.to_ascii_lowercase().as_str(), "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy" | "system-ui" | "ui-serif" | "ui-sans-serif" | "ui-monospace" | "ui-rounded" | "emoji" | "math" | "fangsong");
        if allow_publication
            && !generic_name
            && let Some(font) = self
                .publication_fonts
                .resolve(family, weight, slant, stretch, synthesis)
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
        generic
            .or_else(|| self.family_name(family))
            .map(|family| requested_font(family, weight, slant, stretch))
    }

    /// Font selection and OpenType features do not depend on run length,
    /// size, or paint color. Share this setup and its cache identity across
    /// lines, with a small bound independent of the document's text length.
    fn prepared_font(&mut self, style: &ShapingStyle<'_>) -> PreparedFont {
        let feature_flags = font_feature_flags(style.font_features);
        if let Some(prepared) = self
            .prepared_fonts
            .iter()
            .find(|prepared| prepared.identity.matches(style, &feature_flags))
        {
            self.shape_cache_stats.font_setup_hits += 1;
            return prepared.clone();
        }
        self.shape_cache_stats.font_setup_misses += 1;
        let mut gpui_font = self.resolve_font(
            style.font_family,
            style.font_weight,
            style.font_slant,
            style.font_stretch,
            style.font_synthesis_style,
        );
        gpui_font.features = native_font_features(style.font_features);
        let cjk_font = style.cjk_font_family.map(|family| {
            let mut font = self.resolve_font_for_script(
                Some(family),
                style.font_weight,
                style.font_slant,
                true,
                style.font_stretch,
                style.font_synthesis_style,
            );
            font.features = gpui_font.features.clone();
            font
        });
        let prepared = PreparedFont {
            cjk_font,
            identity: Arc::new(FontStyleIdentity {
                font_weight: style.font_weight,
                font_slant: style.font_slant,
                synthesis_style: style.font_synthesis_style,
                font_stretch: style.font_stretch.to_bits(),
                family: style.font_family.map(Into::into),
                cjk_family: style.cjk_font_family.map(Into::into),
                feature_flags,
                custom_features: style.font_features.custom_features.into(),
            }),
            font: gpui_font,
        };
        if self.prepared_fonts.len() == FONT_SETUP_CACHE_CAPACITY {
            self.prepared_fonts.pop_front();
        }
        self.prepared_fonts.push_back(prepared.clone());
        prepared
    }

    fn release_shaping_scratch(&self) {
        if self.previous_document.is_none() && self.previous_append.is_none() {
            self.text_system.release_shaping_scratch();
        }
    }

    fn font_has_range_restrictions(&self, font: &Font) -> bool {
        let faces = &self.publication_fonts;
        once(font.family.as_ref()).chain(font.fallbacks.as_ref().into_iter()
            .flat_map(|fallbacks| fallbacks.fallback_list().iter().map(String::as_str)))
            .any(|family| faces.has_restricted_unicode_range(family))
    }

    /// Select the authored CSS subset before entering native fallback. Some
    /// backends fall straight back to a system font when the first subset has
    /// no glyph, bypassing a later publication face in the requested family list.
    fn range_filtered_font(&self, font: &Font, character: char) -> Font {
        if !self.font_has_range_restrictions(font) { return font.clone(); }
        let faces = &self.publication_fonts;
        let character = if character == '\t' { ' ' } else { character };
        let mut allowed = once(font.family.as_ref()).chain(font.fallbacks.as_ref().into_iter()
            .flat_map(|fallbacks| fallbacks.fallback_list().iter().map(String::as_str)))
            .filter(|family| faces.unicode_range_allows(family, character));
        let mut selected = font.clone();
        selected.family = allowed.next().unwrap_or(".SystemUIFont").to_owned().into();
        let fallbacks = allowed.map(str::to_owned).collect::<Vec<_>>();
        selected.fallbacks = (!fallbacks.is_empty()).then(|| FontFallbacks::from_fonts(fallbacks));
        selected
    }

    fn append_range_filtered_runs(&self, output: &mut FontRuns, text: &str, runs: FontRuns) {
        let mut offset = 0;
        for run in runs {
            let chunk = &text[offset..offset + run.len];
            offset += run.len;
            if !self.font_has_range_restrictions(&run.font) || chunk.is_empty() {
                output.push(run);
                continue;
            }
            let mut cache = HashMap::<char, Font>::new();
            let mut start = 0;
            let mut previous: Option<Font> = None;
            for (byte, character) in chunk.char_indices() {
                let font = cache.entry(character)
                    .or_insert_with(|| self.range_filtered_font(&run.font, character));
                if previous.as_ref().is_some_and(|previous| previous != font)
                    && let Some(previous) = previous.take()
                {
                    output.push(TextRun { len: byte - start, font: previous, ..run.clone() });
                    start = byte;
                }
                previous = Some(font.clone());
            }
            if let Some(font) = previous {
                output.push(TextRun { len: chunk.len() - start, font, ..run });
            }
        }
    }

    /// Native line bounds can include an unused face at the front of a
    /// fallback list. The engine supplies the primary-font strut separately;
    /// glyph metrics must describe only faces that actually supplied glyphs.
    fn retained_font_extents(&self, layout: &LineLayout) -> (Pixels, Pixels) {
        let used = || layout.runs.iter().filter(|run| !run.glyphs.is_empty());
        let ascent = used().map(|run| f32::from(self.text_system.ascent(run.font_id, layout.font_size)))
            .reduce(f32::max).map(px).unwrap_or(layout.ascent);
        let descent = used().map(|run| f32::from(self.text_system.descent(run.font_id, layout.font_size)).abs())
            .reduce(f32::max).map(px).unwrap_or(layout.descent);
        (ascent, descent)
    }

    fn layout_directional_line(&self, text: &str, font_size: Pixels, runs: &[TextRun], direction: TextDirection) -> LineLayout {
        // The document retains its own glyph and caret arrays. Holding these
        // native layouts in the frame cache would duplicate that geometry.
        let mut inferred = self.text_system.layout_line_uncached(text, font_size, runs);
        let is_rtl = direction == TextDirection::Rtl;
        // Keep the native fast path when it already covers the whole source
        // with the requested direction. Coverage also detects dropped leading
        // neutrals/numerals in backends that infer a conflicting paragraph base.
        if (direction == TextDirection::Ltr && text.is_ascii())
            || (!inferred.clusters.is_empty() && inferred.clusters.iter().try_fold(0, |end, cluster| (cluster.bytes.start == end && cluster.is_rtl == is_rtl).then_some(cluster.bytes.end)) == Some(text.len()))
        {
            let (ascent, descent) = self.retained_font_extents(&inferred);
            inferred.ascent = ascent;
            inferred.descent = descent;
            return inferred;
        }
        self.text_system.recycle_line_clusters(take(&mut inferred.clusters));
        drop(inferred);

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
        let mut native =
            self.text_system
                .layout_line_uncached(&directed, font_size, &directed_runs);
        let source_byte = |index| {
            source_bytes
                .binary_search_by_key(&index, |&(byte, _)| byte)
                .ok()
                .map(|index| source_bytes[index].1)
        };
        let runs: Vec<NativeShapedRun> = native
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
                (!glyphs.is_empty()).then_some(NativeShapedRun {
                    font_id: run.font_id,
                    glyphs,
                })
            })
            .collect();
        native.clusters.retain_mut(|cluster| {
            if cluster.bytes.start < prefix.len() || cluster.bytes.end > content_end {
                return false;
            }
            let (Some(start), Some(end)) = (source_byte(cluster.bytes.start), source_byte(cluster.bytes.end)) else { return false };
            cluster.bytes = start..end;
            true
        });
        let clusters = native.clusters;
        // Synthetic direction controls can select a fallback font even when
        // every authored glyph uses the requested face. Only retained source
        // runs contribute vertical metrics after those controls are removed.
        let ascent = runs.iter().map(|run| f32::from(self.text_system.ascent(run.font_id, font_size))).reduce(f32::max).map(px).unwrap_or(native.ascent);
        let descent = runs.iter().map(|run| f32::from(self.text_system.descent(run.font_id, font_size)).abs()).reduce(f32::max).map(px).unwrap_or(native.descent);
        LineLayout { font_size: native.font_size, width: native.width, ascent, descent, runs, clusters, len: text.len() }
    }

    fn layout_contextual_line(
        &self,
        text: &str,
        font_size: Pixels,
        runs: &[TextRun],
        direction: TextDirection,
        cursive_joining: CursiveJoining,
    ) -> LineLayout {
        // Layout resolves preserved tab stops. A same-length space keeps the
        // selected font's metrics without changing source byte indices.
        let tab_display;
        let text = if text.contains('\t') {
            tab_display = text.replace('\t', " ");
            tab_display.as_str()
        } else {
            text
        };
        if cursive_joining == CursiveJoining::Isolated {
            return self.layout_directional_line(text, font_size, runs, direction);
        }
        // GPUI's line API cannot supply surrounding text independently of a
        // font run. Solicit the neighboring cursive forms with zero-width
        // joiners, then export geometry in the original source coordinates.
        let prefix = if shaping_contracts::cursive_joining_connects_to_previous(cursive_joining) {
            '\u{200d}'.len_utf8()
        } else {
            0
        };
        let content_end = prefix + text.len();
        let mut contextual = String::with_capacity(text.len() + 6);
        if shaping_contracts::cursive_joining_connects_to_previous(cursive_joining) {
            contextual.push('\u{200d}');
        }
        contextual.push_str(text);
        if shaping_contracts::cursive_joining_connects_to_next(cursive_joining) {
            contextual.push('\u{200d}');
        }
        let mut contextual_runs = runs.to_vec();
        if let Some(first) = contextual_runs.first_mut() {
            first.len += prefix;
        }
        if let Some(last) = contextual_runs.last_mut() {
            last.len += contextual.len() - content_end;
        }
        let mut layout = self.layout_directional_line(&contextual, font_size, &contextual_runs, direction);
        for run in &mut layout.runs {
            run.glyphs.retain_mut(|glyph| {
                if glyph.index >= content_end {
                    return false;
                }
                glyph.index = glyph.index.saturating_sub(prefix);
                true
            });
        }
        layout.runs.retain(|run| !run.glyphs.is_empty());
        layout.clusters.retain_mut(|cluster| {
            let start = cluster.bytes.start.max(prefix);
            let end = cluster.bytes.end.min(content_end);
            if start >= end {
                return false;
            }
            cluster.bytes = start - prefix..end - prefix;
            true
        });
        layout.len = text.len();
        layout
    }

    fn shape_line_geometry(
        &mut self,
        text: &str,
        styles: &[TextStyleSpan<'_>],
    ) -> Result<Option<(ShapedLine, Arc<[f32]>)>, ShapeError> {
        self.ensure_text_fonts(text);
        if styles
            .iter()
            .any(|style| style.style.caps_synthesis != CapsSynthesis::None)
        {
            return self.shape_caps_line(text, styles);
        }
        self.shape_cache_stats.line_shapes += 1;
        let character_bytes = CharacterBytes::new(text);
        let mut text_runs = FontRuns::with_capacity(styles.len());
        let mut font_run_boundaries = SmallVec::<[usize; 8]>::new();
        let mut colors = ColorSpans::with_capacity(styles.len());
        for style in styles {
            let prepared = self.prepared_font(&(style).style);
            let color = Hsla::from(rgba(style.style.color));
            let byte_range = style.byte_range();
            font_run_boundaries.push(text_runs.len());
            let mut selected = FontRuns::new();
            append_font_runs(&mut selected, &text[byte_range.clone()], prepared.font, prepared.cjk_font, color);
            self.append_range_filtered_runs(&mut text_runs, &text[byte_range.clone()], selected);
            colors.push((byte_range, color));
        }
        font_run_boundaries.push(text_runs.len());
        let single_group = styles.iter().all(|style| {
            style.style.font_size.to_bits() == styles[0].style.font_size.to_bits()
                && style.style.direction == styles[0].style.direction
        });
        // A single size and direction can use native boundary allocations
        // directly. Multiple groups need aggregate line-coordinate arrays.
        let mut raw_advances: Arc<[f32]> = if single_group { Arc::default() } else { repeat_n(0.0, character_bytes.len() - 1).collect() };
        let mut raw_caret_stops: CaretPositions = if single_group { Arc::<[f32]>::default() } else { repeat_n(0.0, character_bytes.len()).collect() }.into();
        let mut raw_cluster_boundaries: Arc<[bool]> = if single_group { Arc::default() } else { repeat_n(false, character_bytes.len()).collect() };
        if !single_group {
            let boundaries = Arc::make_mut(&mut raw_cluster_boundaries);
            boundaries[0] = true;
            if let Some(last) = boundaries.last_mut() {
                *last = true;
            }
        }
        let mut glyphs = Vec::new();
        let mut font_spans = FontSpans::new();
        let mut x_base = 0.0f32;
        let mut ascent = 0.0f32;
        let mut descent = 0.0f32;
        let mut first_style = 0usize;
        while first_style < styles.len() {
            let font_size_bits = styles[first_style].style.font_size.to_bits();
            let mut end_style = first_style + 1;
            while end_style < styles.len()
                && styles[end_style].style.font_size.to_bits() == font_size_bits
                && styles[end_style].style.direction == styles[first_style].style.direction
            {
                end_style += 1;
            }
            let byte_start = styles[first_style].byte_range().start;
            let byte_end = styles[end_style - 1].byte_range().end;
            let group_text = &text[byte_start..byte_end];
            let (character_start, character_end) = if single_group {
                (0, character_bytes.len() - 1)
            } else {
                (
                    character_bytes.lower_bound(byte_start),
                    character_bytes.lower_bound(byte_end),
                )
            };
            if styles[first_style].style.font_size == 0.0 {
                if single_group {
                    raw_advances = repeat_n(0.0, character_bytes.len() - 1).collect();
                    raw_caret_stops = repeat_n(0.0, character_bytes.len())
                        .collect::<Arc<[f32]>>()
                        .into();
                    raw_cluster_boundaries = repeat_n(true, character_bytes.len()).collect();
                } else {
                    raw_caret_stops.make_dense()[character_start..=character_end].fill(x_base);
                    Arc::make_mut(&mut raw_cluster_boundaries)[character_start..=character_end].fill(true);
                }
                first_style = end_style;
                continue;
            }
            let layout = self.layout_contextual_line(
                group_text,
                px(styles[first_style].style.font_size),
                &text_runs[font_run_boundaries[first_style]..font_run_boundaries[end_style]],
                styles[first_style].style.direction,
                shaping::cursive_joining(
                    shaping_contracts::cursive_joining_connects_to_previous(styles[first_style]
                        .style
                        .cursive_joining),
                    shaping_contracts::cursive_joining_connects_to_next(styles[end_style - 1]
                        .style
                        .cursive_joining),
                ),
            );
            let bytes = character_bytes.slice(character_start..character_end + 1);
            let native =
                native_cluster_geometry(group_text, &layout, bytes, byte_start, single_group);
            let (mut local_caret_stops, local_cluster_boundaries) = native
                .as_ref()
                .map(|geometry| (geometry.carets.clone(), geometry.boundaries.clone()))
                .unwrap_or_else(|| {
                    let (carets, boundaries) =
                        build_text_boundaries_with_offsets(group_text, &layout, bytes, byte_start);
                    (carets.into(), boundaries)
                });
            let mut group_width = f32::from(layout.width);
            if native.is_none()
                && group_text.chars().last().is_some_and(char::is_whitespace)
                && local_caret_stops.len() >= 2
                && local_caret_stops.get(local_caret_stops.len() - 1)
                    .zip(local_caret_stops.get(local_caret_stops.len() - 2))
                    .is_some_and(|(last, previous)| (last - previous).abs() <= f32::EPSILON)
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
                let probe_layout = self.layout_directional_line(
                    &probe_text,
                    px(styles[first_style].style.font_size),
                    &probe_runs,
                    styles[first_style].style.direction,
                );
                let (probe_carets, _) = build_text_boundaries(&probe_text, &probe_layout);
                let original_character_count = character_end - character_start;
                if let Some(&corrected_end) = probe_carets.get(original_character_count) {
                    local_caret_stops.make_dense()[original_character_count] = corrected_end;
                    group_width = group_width.max(corrected_end);
                }
                self.text_system.recycle_line_clusters(probe_layout.clusters);
                self.shape_cache_stats.trailing_whitespace_probes += 1;
                self.shape_cache_stats.trailing_whitespace_probe_characters += original_character_count as u64;
                if let Some(started) = probe_started {
                    self.shape_cache_stats.trailing_whitespace_probe_time += started.elapsed();
                }
            }
            let local_advances = native.map(|geometry| geometry.advances).unwrap_or_else(|| advances_from_carets(&local_caret_stops.to_dense(), &local_cluster_boundaries));
            if single_group {
                raw_advances = local_advances;
                raw_caret_stops = local_caret_stops;
                raw_cluster_boundaries = local_cluster_boundaries;
            } else {
                Arc::make_mut(&mut raw_advances)[character_start..character_end].copy_from_slice(&local_advances);
                let carets = raw_caret_stops.make_dense();
                for (offset, caret) in local_caret_stops.iter().enumerate() {
                    carets[character_start + offset] = x_base + caret;
                }
                let boundaries = Arc::make_mut(&mut raw_cluster_boundaries);
                for (offset, boundary) in local_cluster_boundaries.iter().copied().enumerate() {
                    boundaries[character_start + offset] |= boundary;
                }
            }
            self.text_system.recycle_line_clusters(layout.clusters);
            for mut shaped_run in layout.runs {
                if shaped_run.glyphs.is_empty() {
                    continue;
                }
                let start = glyphs.len();
                for glyph in &mut shaped_run.glyphs {
                    glyph.position.x += px(x_base);
                    glyph.index += byte_start;
                }
                // Ordinary single-face runs keep the native allocation. More
                // groups append into the same compact glyph array.
                if glyphs.is_empty() {
                    glyphs = shaped_run.glyphs;
                } else {
                    glyphs.append(&mut shaped_run.glyphs);
                }
                if let Some(previous) = font_spans.last_mut()
                    && previous.font_id == shaped_run.font_id
                    && previous.font_size == layout.font_size
                {
                    previous.glyphs.end = glyphs.len();
                } else {
                    font_spans.push(GpuiFontSpan { glyphs: start..glyphs.len(), font_id: shaped_run.font_id, font_size: layout.font_size });
                }
            }
            x_base += group_width;
            ascent = ascent.max(f32::from(layout.ascent));
            descent = descent.max(f32::from(layout.descent));
            first_style = end_style;
        }
        let Ok(character_count) = u32::try_from(character_bytes.len() - 1) else {
            return Ok(None);
        };
        let caret_stops = CaretPositions::from_advances(&raw_advances, raw_caret_stops);
        // Layout and compact carets share the advances allocation; painting
        // reuses the same exact positions without copying native glyphs.
        let paint_bounds = GpuiTextRun::native_paint_bounds(&glyphs, &font_spans, &self.text_system);
        let mut run_resource = GpuiTextRun::new(glyphs, font_spans, px(ascent), px(descent), character_bytes, colors, caret_stops.clone());
        run_resource.paint_bounds = paint_bounds;
        let advances = raw_advances;
        let run = TextRunId(self.runs.len() as u32);
        Rc::make_mut(&mut self.runs).push(Arc::new(run_resource));
        Ok(Some((
            ShapedLine {
                line_index: 0,
                text_range: 0..character_count,
                run,
                ascent,
                caret_stops,
                cluster_boundaries: raw_cluster_boundaries,
            },
            advances,
        )))
    }

    fn run_font_metrics(&self, run: &GpuiTextRun, geometry: &ShapedTextRun) -> RunFontMetrics {
        if matches!(&run.metadata, RunMetadata::Uniform { .. }) {
            return RunFontMetrics::Uniform(FontMetrics {
                ascent: f32::from(run.ascent),
                descent: f32::from(run.descent).abs(),
            });
        }
        let mut spans = SmallVec::<[(Range<usize>, FontMetrics); 4]>::new();
        for span in run
            .metadata
            .font_spans_in_range(0..run.glyphs.len(), run.glyphs.len())
        {
            let metrics = FontMetrics {
                ascent: f32::from(self.text_system.ascent(span.font_id, span.font_size)),
                descent: f32::from(self.text_system.descent(span.font_id, span.font_size)).abs(),
            };
            spans.push((span.glyphs, metrics));
        }
        let Some((_, first)) = spans.first() else {
            return RunFontMetrics::Uniform(FontMetrics {
                ascent: f32::from(run.ascent),
                descent: f32::from(run.descent).abs(),
            });
        };
        if spans.iter().all(|(_, metrics)| metrics == first) {
            return RunFontMetrics::Uniform(*first);
        }
        // Accumulate once per cluster, then expand once into logical scalar
        // order. This also handles bidi order, ligatures and combining marks
        // without repeatedly walking long clusters for each native glyph.
        let boundaries = shaping_contracts::shaped_text_run_cluster_boundaries(&geometry)
            .iter()
            .enumerate()
            .filter_map(|(index, boundary)| boundary.then_some(index))
            .collect::<Vec<_>>();
        let mut clusters = vec![FontMetrics::default(); boundaries.len().saturating_sub(1)];
        let bytes = run.character_bytes.slice(0..run.character_bytes.len());
        for (range, metrics) in spans {
            for glyph in run.glyphs_in_range(range) {
                let character = bytes.character_at_or_before(glyph.byte_index);
                let cluster = boundaries
                    .partition_point(|boundary| *boundary <= character)
                    .saturating_sub(1);
                if let Some(vertical) = clusters.get_mut(cluster) {
                    *vertical = FontMetrics {
                        ascent: vertical.ascent.max(metrics.ascent),
                        descent: vertical.descent.max(metrics.descent),
                    };
                }
            }
        }
        let mut characters = Vec::with_capacity(shaping_contracts::shaped_text_run_advances(&geometry).len());
        for (range, metrics) in boundaries.windows(2).zip(clusters) {
            characters.extend(repeat_n(metrics, range[1] - range[0]));
        }
        RunFontMetrics::PerCharacter(characters.into())
    }

    fn set_text_run_colors(&mut self, run_id: TextRunId, colors: &[u32]) {
        let Some(existing) = self.runs.get(run_id.0 as usize) else {
            return;
        };
        let Some(spans) = paint_color_spans(
            existing
                .character_bytes
                .slice(0..existing.character_bytes.len()),
            colors,
        ) else {
            return;
        };
        let index = run_id.0 as usize;
        if let Some(previous) = &mut self.previous_append {
            if index < previous.runs
                && !previous.recolored.iter().any(|(saved, _)| *saved == index)
            {
                previous.recolored.push((index, existing.clone()));
            }
        }
        let entry = &mut Rc::make_mut(&mut self.runs)[index];
        if let Some(unique) = Arc::get_mut(entry) {
            unique.metadata = unique.metadata.with_colors(spans, unique.glyphs.len(), unique.byte_len());
        } else {
            // Retained scenes and rollback snapshots keep their original run.
            *entry = Arc::new(entry.with_colors(spans));
        }
    }
}

fn paint_color_spans(character_bytes: BorrowedCharacterBytes<'_>, colors: &[u32]) -> Option<Vec<(Range<usize>, Hsla)>> {
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
        spans.push((character_bytes.get(start)?..character_bytes.get(index)?, Hsla::from(rgba(color))));
        start = index;
        color = colors[index];
    }
    spans.push((character_bytes.get(start)?..character_bytes.get(character_bytes.len() - 1)?, Hsla::from(rgba(color))));
    Some(spans)
}

impl GlyphShaper for GpuiTextCache {
    /// Reader overrides suppress the publisher faces they replace. Unchanged
    /// selections keep the prepared font caches.
    fn select_publication_fonts(&mut self, faces: &[FontFace], overrides: &ReaderStyleOverrides) {
        let policy_changed = self.publication_fonts.set_reader_overrides(overrides);
        let active_changed = self
            .publication_fonts
            .set_active(publication_faces_for_overrides(faces, overrides));
        self.publication_fonts_changed |= policy_changed || active_changed;
    }

    fn resource_checkpoint(&self) -> Option<NativeResourceCheckpoint> {
        Some(NativeResourceCheckpoint {
            generation: self.generation,
            glyph_count: self.glyphs.len(),
            run_count: self.runs.len(),
        })
    }

    fn restore_resource_checkpoint(&mut self, checkpoint: NativeResourceCheckpoint) -> bool {
        let glyphs = checkpoint.glyph_count;
        let runs = checkpoint.run_count;
        if checkpoint.generation != self.generation
            || glyphs > self.glyphs.len()
            || runs > self.runs.len()
            || self.previous_document.is_some()
            || self.previous_append.is_some()
        {
            return false;
        }
        self.table
            .retain(|_, glyph| (glyph.handle.0 as usize) < glyphs);
        Rc::make_mut(&mut self.glyphs).truncate(glyphs);
        Rc::make_mut(&mut self.runs).truncate(runs);
        true
    }

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
        self.previous_append = Some(AppendTextResources {
            glyphs: self.glyphs.len(),
            runs: self.runs.len(),
            recolored: Vec::new(),
        });
        Ok(())
    }

    fn commit_append_shaping(&mut self) {
        self.previous_append = None;
        self.release_shaping_scratch();
    }

    fn rollback_append_shaping(&mut self) {
        if let Some(previous) = self.previous_append.take() {
            self.table
                .retain(|_, glyph| (glyph.handle.0 as usize) < previous.glyphs);
            if self.glyphs.len() != previous.glyphs {
                Rc::make_mut(&mut self.glyphs).truncate(previous.glyphs);
            }
            if self.runs.len() != previous.runs || !previous.recolored.is_empty() {
                let runs = Rc::make_mut(&mut self.runs);
                runs.truncate(previous.runs);
                for (index, run) in previous.recolored {
                    runs[index] = run;
                }
            }
        }
        self.release_shaping_scratch();
    }

    fn begin_document_shaping(&mut self) {
        self.refresh_publication_fonts();
        debug_assert!(
            self.previous_document.is_none(),
            "document shaping transactions cannot be nested"
        );
        let previous = DocumentTextResources {
            generation: take(&mut self.generation),
            table: take(&mut self.table),
            glyphs: take(&mut self.glyphs),
            runs: std::mem::replace(&mut self.runs, Rc::new(Vec::new())),
        };
        self.previous_document = Some(previous);
    }

    fn commit_document_shaping(&mut self) {
        self.previous_document = None;
        self.release_shaping_scratch();
    }

    fn rollback_document_shaping(&mut self) {
        if let Some(previous) = self.previous_document.take() {
            self.generation = previous.generation;
            self.table = previous.table;
            self.glyphs = previous.glyphs;
            self.runs = previous.runs;
        }
        self.release_shaping_scratch();
    }

    fn font_relative_metrics(
        &mut self,
        request: &ShapingStyle<'_>,
    ) -> Result<FontRelativeMetrics, ShapeError> {
        let gpui_font = self.resolve_font(
            request.font_family,
            request.font_weight,
            request.font_slant,
            request.font_stretch,
            request.font_synthesis_style,
        );
        // CSS Fonts 4 selects the first face whose unicode-range allows
        // U+0020, even when that face's cmap omits the space glyph itself.
        let mut metric_font = gpui_font.clone();
        metric_font.family = once(gpui_font.family.as_ref())
            .chain(gpui_font.fallbacks.as_ref().into_iter().flat_map(|fallbacks| {
                fallbacks.fallback_list().iter().map(String::as_str)
            }))
            .find(|family| self.publication_fonts.unicode_range_allows(family, ' '))
            .unwrap_or(".SystemUIFont")
            .to_owned()
            .into();
        let font_id = self.text_system.resolve_font(&metric_font);
        // CSS `ex` is the selected face's x-height, not the ink bounds of its
        // `x` glyph. Those differ for fonts such as WPT's Ahem and can also
        // differ because of hinting. GPUI exposes the face metric directly.
        let font_size = request.font_size;
        let fallback = FontRelativeMetrics::default();
        let x_height_ratio =
            f32::from(self.text_system.x_height(font_id, px(font_size))) / font_size;
        let x_height_ratio = if x_height_ratio != 0.0 {
            x_height_ratio
        } else {
            fallback.x_height_ratio
        };
        let cap_height_ratio =
            f32::from(self.text_system.cap_height(font_id, px(font_size))) / font_size;
        let cap_height_ratio = if cap_height_ratio != 0.0 {
            cap_height_ratio
        } else {
            fallback.cap_height_ratio
        };
        let zero_run = TextRun {
            len: 1,
            font: self.range_filtered_font(&gpui_font, '0'),
            color: Hsla::from(rgba(0x000000FF)),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let zero_layout =
            self.text_system
                .layout_line("0", px(font_size), &[zero_run.clone()], None);
        let ch_advance_ratio = f32::from(zero_layout.width) / font_size;
        // Spaces may be synthesized by native shaping when the primary face
        // lacks U+0020. Tabs instead need the actual space glyph from the CSS
        // fallback chain, respecting each face's declared unicode-range.
        let space_advance = once(gpui_font.family.as_ref())
            .chain(gpui_font.fallbacks.as_ref().into_iter().flat_map(|fallbacks| {
                fallbacks.fallback_list().iter().map(String::as_str)
            }))
            .filter(|family| self.publication_fonts.unicode_range_allows(family, ' '))
            .find_map(|family| {
                metric_font.family = family.to_owned().into();
                let font_id = self.text_system.resolve_font(&metric_font);
                self.text_system.advance(font_id, px(font_size), ' ').ok()
            })
            .map(|advance| advance.width)
            .unwrap_or_else(|| {
                // If no requested face supplies a space, let native fallback
                // choose one. Keep it inside the run to avoid trailing-space
                // trimming, and read its own boundary rather than subtracting
                // a zero shaped with a potentially different fallback face.
                let space_run = TextRun { len: 2, ..zero_run };
                self.text_system
                    .layout_line(" 0", px(font_size), &[space_run], None)
                    .x_for_index(1)
            });
        let space_advance_ratio = f32::from(space_advance) / font_size;
        let ascent_ratio = f32::from(self.text_system.ascent(font_id, px(font_size))) / font_size;
        // GPUI exposes descent below the baseline as a signed value; layout's
        // metric contract stores its positive magnitude.
        let descent_ratio = -f32::from(self.text_system.descent(font_id, px(font_size))) / font_size;
        // Older faces may omit x/cap height while still providing accurate
        // ascent and descent. Fill only the missing metrics above so inline
        // ancestor struts retain the selected face's baseline geometry.
        // Match screen font metrics: round ascent and descent independently,
        // rather than imposing a fixed multiple of the em size on every face.
        let normal_height =
            (ascent_ratio * font_size).round() + (descent_ratio * font_size).round();
        Ok(FontRelativeMetrics {
            x_height_ratio,
            ch_advance_ratio,
            space_advance_ratio,
            cap_height_ratio,
            line_metrics: Some(FontLineMetrics {
                ascent_ratio,
                descent_ratio,
            }),
            normal_line_height_ratio: (normal_height != 0.0).then_some(normal_height / font_size),
        })
    }

    fn shape_glyph(&mut self, request: GlyphRequest<'_>) -> Result<ShapedGlyph, ShapeError> {
        let mut utf8 = [0; 4];
        self.ensure_text_fonts(request.character.encode_utf8(&mut utf8));
        let gpui_font = self.resolve_font_for_script(
            shaping_contracts::shaping_style_font_family_for_character(
                &request.style,
                request.character,
            ),
            request.style.font_weight,
            request.style.font_slant,
            is_cjk_character(request.character),
            request.style.font_stretch,
            request.style.font_synthesis_style,
        );
        let gpui_font = self.range_filtered_font(&gpui_font, request.character);
        let key = GlyphKey {
            ch: request.character,
            font_size: request.style.font_size.to_bits(),
            font: gpui_font.clone(),
        };
        if let Some(&glyph) = self.table.get(&key) {
            return Ok(ShapedGlyph::new(glyph.handle, glyph.advance, glyph.metrics));
        }

        self.shape_cache_stats.glyph_shapes += 1;

        // GPUI may clamp a zero-pixel request to paintable fallback geometry.
        // CSS defines this case as no ink and no metric contribution, so do
        // not send it through native shaping.
        if request.style.font_size == 0.0 {
            let advance = 0.0;
            let metrics = FontMetrics {
                ascent: 0.0,
                descent: 0.0,
            };
            let handle = GlyphHandle(self.glyphs.len() as u32);
            let registry_glyph = ShapedGlyph::new(handle, advance, metrics);
            let glyphs = Rc::make_mut(&mut self.glyphs);
            glyphs.push(GpuiGlyph::Empty);
            self.table.insert(
                key,
                CachedGlyph {
                    handle,
                    advance,
                    metrics,
                },
            );
            return Ok(registry_glyph);
        }

        let mut utf8 = [0; 4];
        let display_character = if request.character == '\t' {
            ' '
        } else {
            request.character
        };
        let text = display_character.encode_utf8(&mut utf8);
        let run = TextRun {
            len: text.len(),
            font: gpui_font,
            ..Default::default()
        };
        let layout = self
            .text_system
            .layout_line(text, px(request.style.font_size), &[run], None);
        let (ascent, descent) = self.retained_font_extents(&layout);
        let glyphs = Rc::make_mut(&mut self.glyphs);
        let handle = GlyphHandle(glyphs.len() as u32);
        let advance = f32::from(layout.width);
        let metrics = FontMetrics {
            ascent: f32::from(ascent),
            descent: f32::from(descent).abs(),
        };
        let registry_glyph = ShapedGlyph::new(handle, advance, metrics);
        glyphs.push(GpuiGlyph::Layout { layout });

        self.table.insert(
            key,
            CachedGlyph {
                handle,
                advance,
                metrics,
            },
        );
        Ok(registry_glyph)
    }

    fn shape_run(
        &mut self,
        request: TextRunShapeRequest<'_>,
    ) -> Result<Option<ShapedRun>, ShapeError> {
        let paint_colors = shaping_contracts::text_run_shape_request_paint_colors(&request);
        // The engine sends one resolved style. Font preparation and native
        // feature translation are cached entirely inside this adapter.
        let styles = [TextStyleSpan::new(
            0..shaping_contracts::text_run_shape_request_text(&request).len(),
            *shaping_contracts::text_run_shape_request_style(&request),
        )];
        let Some((shaped, advances)) = self.shape_line_geometry(shaping_contracts::text_run_shape_request_text(&request), &styles)? else {
            return Ok(None);
        };
        if let Some(colors) = paint_colors {
            self.set_text_run_colors(shaped.run, colors);
        }
        let geometry = shaped_text_run_from_line(&shaped, advances);
        if let Some(run) = self.runs.get(shaped.run.0 as usize).cloned() {
            let first_handle = u32::try_from(self.glyphs.len())
                .map_err(|_| shaping_contracts::shape_error_registry_capacity_exhausted(self.glyphs.len()))?;
            let characters = shaping_contracts::shaped_text_run_advances(&geometry).len();
            let metrics = if shaping_contracts::text_run_shape_request_style(&request).caps_synthesis != CapsSynthesis::None {
                RunFontMetrics::Uniform(FontMetrics {
                    ascent: f32::from(run.ascent),
                    descent: f32::from(run.descent).abs(),
                })
            } else {
                self.run_font_metrics(&run, &geometry)
            };
            let result = shaping_contracts::shaped_run_painted(geometry, GlyphHandle(first_handle), metrics);
            Rc::make_mut(&mut self.glyphs).push_run(run, characters);
            return Ok(Some(result));
        }
        Ok(Some(shaping_contracts::shaped_run_geometry(geometry)))
    }
}

fn shaped_text_run_from_line(shaped: &ShapedLine, advances: Arc<[f32]>) -> ShapedTextRun {
    let character_count = shaped.caret_stops.len() - 1;
    let advances = if advances.len() == character_count {
        advances
    } else {
        advances_from_carets(&shaped.caret_stops.to_dense(), &shaped.cluster_boundaries)
    };
    shaping_contracts::shaped_text_run_with_backend_run(advances,
        shaped.cluster_boundaries.clone(),
        shaped.run,
        shaped.caret_stops.clone(),
        shaped.ascent,)
}

fn advances_from_carets(carets: &[f32], boundaries: &[bool]) -> Arc<[f32]> {
    let character_count = carets.len().saturating_sub(1);
    let mut advances: Arc<[f32]> = repeat_n(0.0, character_count).collect();
    let values = Arc::make_mut(&mut advances);
    let mut boundary_indices = boundaries.iter().enumerate().filter_map(|(index, boundary)| boundary.then_some(index));
    let mut start = boundary_indices.next().unwrap_or(0);
    for end in boundary_indices {
        if start < end && end <= character_count {
            let width = (carets[end] - carets[start]).abs();
            values[start..end].fill(width / (end - start) as f32);
        }
        start = end;
    }
    advances
}

struct NativeClusterGeometry {
    carets: CaretPositions,
    boundaries: Arc<[bool]>,
    advances: Arc<[f32]>,
}

fn native_cluster_geometry(text: &str, layout: &gpui::LineLayout, bytes: BorrowedCharacterBytes<'_>, byte_offset: usize, compact: bool) -> Option<NativeClusterGeometry> {
    if layout.clusters.is_empty() {
        return None;
    }
    let count = bytes.len().checked_sub(1)?;
    let mut boundaries: Arc<[bool]> = repeat_n(false, count + 1).collect();
    let boundary_values = Arc::make_mut(&mut boundaries);
    boundary_values[0] = true;
    boundary_values[count] = true;

    let compact = compact.then(|| CaretPositions::build_compact(count, |builder| {
        let mut next = 0;
        let mut position = 0.0;
        walk_native_cluster_geometry(text, layout, bytes, byte_offset, |event| {
            match event {
                NativeGeometryEvent::Cluster { start, end, position: start_position, is_rtl } => {
                    // Native clusters can be visually reordered or overlap in
                    // logical source order. Indexed dense writes preserve that
                    // behavior; the common ordered path streams each boundary.
                    if start != next || is_rtl {
                        return None;
                    }
                    boundary_values[start] = true;
                    boundary_values[end] = true;
                    // The next cluster owns the shared boundary, including
                    // any native rounding difference from the previous end.
                    position = start_position;
                }
                NativeGeometryEvent::Character { index, advance, end_position } => {
                    builder.push(position, advance)?;
                    position = end_position;
                    next = index + 1;
                }
            }
            Some(())
        })?;
        (next == count).then_some(position)
    })).flatten();
    if let Some((advances, carets)) = compact {
        return Some(NativeClusterGeometry { carets, boundaries, advances });
    }

    let mut carets: Arc<[f32]> = repeat_n(0.0, count + 1).collect();
    let mut advances: Arc<[f32]> = repeat_n(0.0, count).collect();
    let caret_values = Arc::make_mut(&mut carets);
    let advance_values = Arc::make_mut(&mut advances);
    walk_native_cluster_geometry(text, layout, bytes, byte_offset, |event| {
        match event {
            NativeGeometryEvent::Cluster { start, end, position, .. } => {
                boundary_values[start] = true;
                boundary_values[end] = true;
                caret_values[start] = position;
            }
            NativeGeometryEvent::Character { index, advance, end_position } => {
                advance_values[index] = advance;
                caret_values[index + 1] = end_position;
            }
        }
        Some(())
    })?;
    Some(NativeClusterGeometry { carets: carets.into(), boundaries, advances })
}

enum NativeGeometryEvent {
    Cluster { start: usize, end: usize, position: f32, is_rtl: bool },
    Character { index: usize, advance: f32, end_position: f32 },
}

fn walk_native_cluster_geometry(
    text: &str,
    layout: &gpui::LineLayout,
    bytes: BorrowedCharacterBytes<'_>,
    byte_offset: usize,
    mut emit: impl FnMut(NativeGeometryEvent) -> Option<()>,
) -> Option<()> {
    // Adjacent logical clusters share a boundary. Reuse that position for
    // ordered text; find_from still bounds its walk before searching bidi jumps.
    let mut boundary_cursor = 0;
    for cluster in &layout.clusters {
        if cluster.bytes.is_empty() || cluster.bytes.end > text.len() {
            return None;
        }
        let start = bytes.find_from(byte_offset + cluster.bytes.start, &mut boundary_cursor)?;
        let end = bytes.find_from(byte_offset + cluster.bytes.end, &mut boundary_cursor)?;
        if start >= end {
            return None;
        }
        let width = f32::from(cluster.advance);
        let left = f32::from(cluster.x);
        if !width.is_finite() || width < 0.0 || !left.is_finite() {
            return None;
        }
        let (x0, x1) = if cluster.is_rtl { (left + width, left) } else { (left, left + width) };
        emit(NativeGeometryEvent::Cluster { start, end, position: x0, is_rtl: cluster.is_rtl })?;
        if cluster.bytes.len() == 1 {
            // A one-byte cluster is one ASCII character, with no joiner or
            // variation selector to exclude. Keep the interpolated endpoint
            // arithmetic so native rounding stays identical to the full path.
            emit(NativeGeometryEvent::Character { index: start, advance: width, end_position: x0 + (x1 - x0) })?;
            continue;
        }
        let characters = text[cluster.bytes.clone()].chars();
        let zero_width = |character| {
            matches!(character,
            '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}'
                | '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}')
        };
        let visible_count = characters
            .clone()
            .filter(|&character| !zero_width(character))
            .count();
        let mut visible = 0;
        for (index, character) in (start..end).zip(characters) {
            // Joiners and variation selectors influence the cluster's glyphs
            // but own no part of its advance. Giving them a fraction of that
            // width shifts glyphs when a color span splits the cluster.
            let advance = if zero_width(character) || visible_count == 0 {
                0.0
            } else {
                visible += 1;
                width / visible_count as f32
            };
            let end_position = if visible_count == 0 {
                x0
            } else {
                x0 + (x1 - x0) * visible as f32 / visible_count as f32
            };
            emit(NativeGeometryEvent::Character { index, advance, end_position })?;
        }
    }
    Some(())
}

fn build_text_boundaries(text: &str, layout: &gpui::LineLayout) -> (Arc<[f32]>, Arc<[bool]>) {
    let character_bytes = CharacterBytes::new(text);
    build_text_boundaries_with_offsets(text, layout, character_bytes.slice(0..character_bytes.len()), 0)
}

fn build_text_boundaries_with_offsets(text: &str, layout: &gpui::LineLayout, character_bytes: BorrowedCharacterBytes<'_>, byte_offset: usize) -> (Arc<[f32]>, Arc<[bool]>) {
    if character_bytes.len() <= 1 {
        return (Arc::from([]), Arc::from([true]));
    }

    let glyph_bytes = || layout.runs.iter().flat_map(|run| run.glyphs.iter().map(|glyph| glyph.index));
    if glyph_bytes().is_sorted() {
        // Ordered clusters need no temporary index array or sorting. Include
        // both ends so missing glyphs and trailing whitespace retain their width.
        build_text_boundaries_from_clusters(text, layout, character_bytes, byte_offset, glyph_bytes().chain(once(text.len())))
    } else {
        let mut cluster_bytes = glyph_bytes().collect::<Vec<_>>();
        cluster_bytes.push(0);
        cluster_bytes.push(text.len());
        cluster_bytes.sort_unstable();
        cluster_bytes.dedup();
        build_text_boundaries_from_clusters(text, layout, character_bytes, byte_offset, cluster_bytes.into_iter())
    }
}

fn build_text_boundaries_from_clusters(text: &str, layout: &gpui::LineLayout, character_bytes: BorrowedCharacterBytes<'_>, byte_offset: usize, cluster_bytes: impl Iterator<Item = usize>) -> (Arc<[f32]>, Arc<[bool]>) {
    let mut caret_stops: Arc<[f32]> = repeat_n(0.0, character_bytes.len()).collect();
    let mut cluster_boundaries: Arc<[bool]> = repeat_n(false, character_bytes.len()).collect();
    let caret_values = Arc::make_mut(&mut caret_stops);
    let boundary_values = Arc::make_mut(&mut cluster_boundaries);
    if let Some(first) = boundary_values.first_mut() {
        *first = true;
    }
    if let Some(last) = boundary_values.last_mut() {
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
        while character_bytes.get(char_end).is_some_and(|byte| byte < byte_offset + byte_end) {
            char_end += 1;
        }
        if char_start < char_end {
            boundary_values[char_start] = true;
            boundary_values[char_end] = true;
            let x0 = f32::from(x_for_index(byte_start));
            // Use the full width at the end, including trailing whitespace.
            let x1 = if byte_end == text.len() { f32::from(layout.width) } else { f32::from(x_for_index(byte_end)) };
            let count = char_end - char_start;
            for index in 0..=count {
                caret_values[char_start + index] = x0 + (x1 - x0) * index as f32 / count as f32;
            }
        }
        byte_start = byte_end;
        char_start = char_end;
    }
    (caret_stops, cluster_boundaries)
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
        let geometry = native_cluster_geometry(text, &native, CharacterBytes::Utf8(&[0, 2, 4, 6, 7]), 0, true).unwrap();
        assert_eq!(geometry.carets.to_dense().as_ref(), &[32.0, 22.0, 13.0, 5.0, 0.0]);
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
        let geometry = native_cluster_geometry(text, &native, CharacterBytes::Utf8(&[4, 5, 7, 9]), 4, true).unwrap();
        assert_eq!(&*geometry.advances, &[10.0; 3]);
        assert_eq!(&*geometry.boundaries, &[true; 4]);
        // A bidi boundary has two caret positions. Its visual jump must not
        // be counted as extra width in the preceding character.
        assert_ne!(
            &*advances_from_carets(&geometry.carets.to_dense(), &geometry.boundaries),
            &*geometry.advances
        );
        let shaped = ShapedLine {
            line_index: 0,
            text_range: 0..3,
            run: TextRunId(0),
            ascent: 12.0,
            caret_stops: geometry.carets.into(),
            cluster_boundaries: geometry.boundaries,
        };
        let converted = shaped_text_run_from_line(&shaped, geometry.advances);
        assert_eq!(shaping_contracts::shaped_text_run_advances(&converted), &[10.0; 3]);
    }

    #[test]
    fn native_ligature_clusters_interpolate_carets_and_keep_trailing_spaces() {
        let text = "لا ";
        let mut native = layout(text, &[], 25.0);
        native.clusters = vec![ShapedCluster { bytes: 0..4, x: px(5.0), advance: px(20.0), is_rtl: true }, ShapedCluster { bytes: 4..5, x: px(0.0), advance: px(5.0), is_rtl: true }];
        let geometry = native_cluster_geometry(text, &native, CharacterBytes::Utf8(&[0, 2, 4, 5]), 0, true).unwrap();
        assert_eq!(geometry.carets.to_dense().as_ref(), &[25.0, 15.0, 5.0, 0.0]);
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
        let reused = build_text_boundaries_with_offsets("office", &ligature, CharacterBytes::Ascii { start: 4, end: 11 }, 4);
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
        let (carets, boundaries) = build_text_boundaries_with_offsets(text, &native, CharacterBytes::Utf8(&[4, 5, 7, 9, 12, 13]), 4);
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
    use html::render::SceneImagePolicy;
    use crate::fragment::load_fragment;
    use html_render_core::{self as render_core, SceneOptions};

    #[test]
    fn mixed_fonts_keep_cjk_graphemes_and_latin_runs_separate() {
        let text = "office 漢\u{fe00}か\u{3099}カー 한국 한。 café";
        let mut runs = FontRuns::new();
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
            let mut runs = FontRuns::new();
            append_font_runs(&mut runs, latin, font("Latin"), Some(font("CJK")), rgba(0x000000ff).into());
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].font.family.as_ref(), "Latin");
            assert_eq!(runs[0].len, latin.len());
        }
    }

    #[gpui::test]
    fn book_cjk_fonts_survive_latin_overrides_and_custom_cjk_bypasses_publisher_aliases(cx: &mut TestAppContext) {
        use html::text::FontFace;
        use html_style_model::FontFaceStyle;
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let overrides = html::style::ReaderStyleOverrides {
                    font_family: Some("Latin".into()),
                    cjk_font_fallback: Some("Embedded".into()),
                    ..Default::default()
                };
                let declaration = FontFace { family: "Publisher".into(), sources: vec![], weight: 400.0..=400.0, style: FontFaceStyle::Normal, stretch: 100.0..=100.0, unicode_ranges: vec![0..=0x10ffff], style_explicit: false, stretch_explicit: false, additional_descriptors: vec![] };
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                shaper.select_publication_fonts(&[declaration.clone()], &overrides);
                shaper
                    .register_publication_font(LoadedFontFace {
                        declaration: declaration.clone(),
                        family: "Private Book CJK".into(),
                        bytes: None,
                        weight: 400,
                        slant: FontSlant::Normal,
                        variable: false,
                    })
                    .unwrap();
                for (key, family) in [
                    ("latin", "Latin"),
                    ("embedded", "Embedded"),
                    ("publisher", "Installed Publisher"),
                ] {
                    shaper.font_families.insert(key.into(), family.into());
                }
                let mut style = TextStyleSpan::new(
                    0..4,
                    shaping::style(18.0, 400, FontSlant::Normal, 0, Some("Latin")),
                );
                style.style.cjk_font_family = Some("Publisher, Embedded");
                let selected = shaper.prepared_font(&(style).style);
                assert_eq!(selected.font.family.as_ref(), "Latin");
                assert_eq!(
                    selected.cjk_font.unwrap().family.as_ref(),
                    "Private Book CJK"
                );
                shaper.select_publication_fonts(
                    &[declaration],
                    &html::style::ReaderStyleOverrides {
                        cjk_font_family: Some("Publisher".into()),
                        ..overrides
                    },
                );
                shaper.refresh_publication_fonts();
                let selected = shaper.prepared_font(&(style).style);
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
                shaper
                    .font_families
                    .insert("droid sans fallback".into(), "Droid Sans Fallback".into());
                let font = shaper.resolve_font(
                    Some("Missing, Latin, 'Droid Sans Fallback', Latin"),
                    400,
                    FontSlant::Normal,
                    100.0,
                    FontSynthesisStyle::Auto,
                );
                assert_eq!(font.family.as_ref(), "Latin");
                assert_eq!(
                    font.fallbacks.as_ref().unwrap().fallback_list(),
                    &["Droid Sans Fallback"]
                );
                let without_fallback = shaper.resolve_font(
                    Some("Latin"),
                    400,
                    FontSlant::Normal,
                    100.0,
                    FontSynthesisStyle::Auto,
                );
                assert!(without_fallback.fallbacks.is_none());
                let key = |font: Font| GlyphKey {
                    ch: '漢',
                    font_size: 19.0_f32.to_bits(),
                    font,
                };
                assert_ne!(
                    key(font),
                    key(without_fallback),
                    "glyph caches must distinguish fallback chains"
                );
                assert_eq!(
                    shaper
                        .resolve_font(
                            Some("Missing, 'Droid Sans Fallback'"),
                            400,
                            FontSlant::Normal,
                            100.0,
                            FontSynthesisStyle::Auto
                        )
                        .family
                        .as_ref(),
                    "Droid Sans Fallback"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn publication_fonts_invalidate_cached_fallback_selection(cx: &mut TestAppContext) {
        use html::text::FontFace;
        use html_style_model::FontFaceStyle;
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let style = TextStyleSpan::new(
                    0..5,
                    shaping::style(19.0, 400, FontSlant::Normal, 0x000000ff, Some("Publisher")),
                );
                assert_eq!(
                    shaper.prepared_font(&(style).style).font.family.as_ref(),
                    ".SystemUIFont"
                );
                shaper
                    .shape_line_geometry("Hello", &[style.clone()])
                    .unwrap();
                assert!(!shaper.prepared_fonts.is_empty());
                let declaration = FontFace { family: "Publisher".into(), sources: vec![], weight: 400.0..=400.0, style: FontFaceStyle::Normal, stretch: 100.0..=100.0, unicode_ranges: vec![0..=0x10ffff], style_explicit: false, stretch_explicit: false, additional_descriptors: vec![] };
                let declarations = [declaration.clone()];
                shaper.select_publication_fonts(&declarations, &Default::default());
                shaper
                    .register_publication_font(LoadedFontFace {
                        declaration,
                        family: "LoadedFace".into(),
                        bytes: None,
                        weight: 400,
                        slant: FontSlant::Normal,
                        variable: true,
                    })
                    .unwrap();
                shaper.begin_document_shaping();
                assert!(shaper.prepared_fonts.is_empty());
                assert_eq!(
                    shaper.prepared_font(&(style).style).font.family.as_ref(),
                    "LoadedFace"
                );
                shaper.commit_document_shaping();
                let overrides = html::style::ReaderStyleOverrides {
                    serif_font_family: Some("Publisher".into()),
                    ..Default::default()
                };
                shaper.select_publication_fonts(&declarations, &overrides);
                shaper.begin_document_shaping();
                assert_eq!(
                    shaper.prepared_font(&(style).style).font.family.as_ref(),
                    ".SystemUIFont",
                    "reader font choices must bypass a colliding publication alias"
                );
                shaper.commit_document_shaping();
                shaper.select_publication_fonts(&declarations, &Default::default());
                shaper.begin_document_shaping();
                assert_eq!(
                    shaper.prepared_font(&(style).style).font.family.as_ref(),
                    "LoadedFace"
                );
                shaper.commit_document_shaping();
            })
            .unwrap();
    }

    fn register_run(
        shaper: &mut GpuiTextCache,
        text: &str,
        resources: &mut GlyphResourceStore,
    ) -> Vec<GlyphId> {
        let style = TextStyleSpan::new(
            0..text.len(),
            shaping::style(19.0, 400, FontSlant::Normal, 0x000000ff, None),
        );
        let request = shaping::run(text, (style).style, None);
        let mut glyphs = vec![GlyphId(u32::MAX); text.chars().count()];
        assert!(
            resources
                .shape_run(shaper, request, &mut glyphs)
                .unwrap()
                .is_some()
        );
        glyphs
    }

    #[gpui::test]
    fn run_records_preserve_dense_ids_across_fallbacks_snapshots_and_rollback(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let mut resources = GlyphResourceStore::new(shaper.resource_generation());
                assert_eq!(
                    register_run(&mut shaper, "office", &mut resources),
                    [0, 1, 2, 3, 4, 5].map(GlyphId)
                );
                assert_eq!(
                    resources
                        .shape_glyph(
                            &mut shaper,
                            shaping::glyph('☃', &{
                                let mut resolved_glyph_style =
                                    shaping::style(0.0, 400, FontSlant::Normal, 0, None);
                                resolved_glyph_style.font_stretch = 100.0;
                                resolved_glyph_style.font_synthesis_style =
                                    FontSynthesisStyle::Auto;
                                resolved_glyph_style
                            })
                        )
                        .unwrap(),
                    GlyphId(6)
                );
                assert_eq!(
                    register_run(&mut shaper, "abc", &mut resources),
                    [7, 8, 9].map(GlyphId)
                );
                let retained = shaper.glyphs.clone();
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
                assert_eq!(
                    register_run(&mut shaper, "append", &mut resources),
                    [10, 11, 12, 13, 14, 15].map(GlyphId)
                );
                assert_eq!(shaper.glyphs.clone().records.len(), 4);
                assert_eq!(retained.records.len(), 3);
                assert!(retained.get(10).is_none());
                shaper.rollback_append_shaping();
                assert!(resources.rollback_to(checkpoint));
                assert!(Rc::ptr_eq(&retained, &shaper.glyphs.clone()));
                assert_eq!(resources.len(), retained.len());
                shaper.reset();
                assert!(shaper.glyphs.clone().is_empty());
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
                let document = load_fragment("<p style='color:red'>Retained text</p>".to_owned(), 300.0, Vec::new(), &mut shaper).unwrap();
                let scene = render_core::render_scene_for_view(document.render_view().unwrap(), &SceneOptions::new(300.0, None).unwrap().with_image_policy(SceneImagePolicy::default()));
                let resources = &mut GlyphResourceStore::new(shaper.resource_generation());
                let GpuiTextResources {
                    generation,
                    glyphs,
                    runs,
                } = shaper.resources();
                assert!(!glyphs.is_empty());
                assert!(!runs.is_empty());
                let retained_color = runs[0].color_at(0);
                let retained_glyph_count = glyphs.len();
                let retained_run_count = runs.len();
                let retained_table_count = shaper.table.len();

                let checkpoint = resources.checkpoint();
                shaper.begin_append_shaping().unwrap();
                shaper.set_text_run_colors(TextRunId(0), &[0x0000ffff; 13]);
                assert_eq!(runs[0].color_at(0), retained_color);
                assert_ne!(shaper.runs.clone()[0].color_at(0), retained_color);
                // Zero-ink glyphs allocate resources with deterministic metrics.
                resources
                    .shape_glyph(
                        &mut shaper,
                        shaping::glyph('☃', &{
                            let mut resolved_glyph_style =
                                shaping::style(0.0, 400, FontSlant::Normal, 0, None);
                            resolved_glyph_style.font_stretch = 100.0;
                            resolved_glyph_style.font_synthesis_style = FontSynthesisStyle::Auto;
                            resolved_glyph_style
                        }),
                    )
                    .unwrap();
                let appended = shaper.glyphs.clone();
                assert!(appended.len() > retained_glyph_count);
                assert_eq!(glyphs.len(), retained_glyph_count);
                shaper.rollback_append_shaping();
                assert!(resources.rollback_to(checkpoint));
                assert!(Rc::ptr_eq(&shaper.glyphs.clone(), &glyphs));
                assert!(Rc::ptr_eq(&shaper.runs.clone(), &runs));
                assert!(appended.len() > shaper.glyphs.clone().len());
                assert_eq!(shaper.table.len(), retained_table_count);

                shaper.begin_append_shaping().unwrap();
                let umbrella = resources
                    .shape_glyph(
                        &mut shaper,
                        shaping::glyph('☂', &{
                            let mut resolved_glyph_style =
                                shaping::style(0.0, 400, FontSlant::Normal, 0, None);
                            resolved_glyph_style.font_stretch = 100.0;
                            resolved_glyph_style.font_synthesis_style = FontSynthesisStyle::Auto;
                            resolved_glyph_style
                        }),
                    )
                    .unwrap();
                let snowman = resources
                    .shape_glyph(
                        &mut shaper,
                        shaping::glyph('☃', &{
                            let mut resolved_glyph_style =
                                shaping::style(0.0, 400, FontSlant::Normal, 0, None);
                            resolved_glyph_style.font_stretch = 100.0;
                            resolved_glyph_style.font_synthesis_style = FontSynthesisStyle::Auto;
                            resolved_glyph_style
                        }),
                    )
                    .unwrap();
                assert_ne!(
                    umbrella, snowman,
                    "a rolled-back lookup must not resolve a reused dense ID"
                );
                assert_eq!(resources.get(umbrella).unwrap().ch(), '☂');
                assert_eq!(resources.get(snowman).unwrap().ch(), '☃');
                shaper.commit_append_shaping();
                assert!(shaper.glyphs.clone().len() > glyphs.len());
                shaper.begin_document_shaping();
                let previous_resources = std::mem::replace(
                    resources,
                    GlyphResourceStore::new(shaper.resource_generation()),
                );
                resources
                    .shape_glyph(
                        &mut shaper,
                        shaping::glyph('x', &{
                            let mut resolved_glyph_style =
                                shaping::style(0.0, 400, FontSlant::Normal, 0, None);
                            resolved_glyph_style.font_stretch = 100.0;
                            resolved_glyph_style.font_synthesis_style = FontSynthesisStyle::Auto;
                            resolved_glyph_style
                        }),
                    )
                    .unwrap();
                shaper.rollback_document_shaping();
                *resources = previous_resources;
                assert!(shaper.glyphs.clone().len() > glyphs.len());
                shaper.begin_document_shaping();
                *resources = GlyphResourceStore::new(shaper.resource_generation());
                resources
                    .shape_glyph(
                        &mut shaper,
                        shaping::glyph('y', &{
                            let mut resolved_glyph_style =
                                shaping::style(0.0, 400, FontSlant::Normal, 0, None);
                            resolved_glyph_style.font_stretch = 100.0;
                            resolved_glyph_style.font_synthesis_style = FontSynthesisStyle::Auto;
                            resolved_glyph_style
                        }),
                    )
                    .unwrap();
                shaper.commit_document_shaping();
                assert_eq!(shaper.glyphs.clone().len(), 1);
                assert_eq!(glyphs.len(), retained_glyph_count);
                assert_eq!(runs.len(), retained_run_count);
                assert_eq!(runs[0].color_at(0), retained_color);
                // The retained scene still resolves every command using its own resources.
                let mut painter = crate::paint::GpuiCommandPainter::commands_only(
                    GpuiTextResources {
                        generation,
                        glyphs,
                        runs,
                    },
                );
                scene.paint(&mut painter);
                assert!(!painter.finish().is_empty());
            })
            .unwrap();
    }
}

#[cfg(test)]
mod natural_run_tests {
    use super::*;
    use gpui::{EmptyView, TestAppContext};

    #[gpui::test]
    fn run_callback_preserves_native_geometry_and_colors(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let features = [
                    open_type::open_type_feature_new(OpenTypeFeatureKind::StandardLigatures, 0),
                    open_type::open_type_feature_new(OpenTypeFeatureKind::Kerning, 0),
                ];
                let mut feature_settings = features.to_vec();
                let features = shaping::font_features(&mut feature_settings, &features, false);
                for (text, size, slant) in [
                    ("office ffi", 19.0, FontSlant::Normal),
                    ("a\u{301} 😀 אבג سلام", 27.0, FontSlant::Italic),
                    ("word\u{2009}", 13.0, shaping_contracts::font_slant_oblique(14.0)),
                    ("zero size", 0.0, FontSlant::Normal),
                ] {
                    let mut direct = GpuiTextCache::new(window.text_system().clone());
                    let mut general = GpuiTextCache::new(window.text_system().clone());
                    let count = text.chars().count();
                    let colors = (0..count)
                        .map(|index| {
                            if index % 2 == 0 {
                                0xff0000ff
                            } else {
                                0x0000ffff
                            }
                        })
                        .collect::<Vec<_>>();
                    let mut style = TextStyleSpan::new(
                        0..text.len(),
                        shaping::style(size, 700, slant, 0xff0000ff, Some("serif")),
                    );
                    style.style.font_features = &features;
                    let request = shaping::run(text, (style.clone()).style, Some(&colors));
                    let natural = shaping_contracts::shaped_run_into_geometry(direct.shape_run(request).unwrap().unwrap());
                    let styles = [style];
                    let (line, advances) =
                        general.shape_line_geometry(text, &styles).unwrap().unwrap();
                    let expected = shaped_text_run_from_line(&line, advances);
                    assert_eq!(shaping_contracts::shaped_text_run_advances(&natural), shaping_contracts::shaped_text_run_advances(&expected), "{text}");
                    assert_eq!(shaping_contracts::shaped_text_run_cluster_boundaries(&natural), shaping_contracts::shaped_text_run_cluster_boundaries(&expected));
                    assert_eq!(shaping_contracts::shaped_text_run_caret_stops(&natural), shaping_contracts::shaped_text_run_caret_stops(&expected));
                    assert_eq!(shaping_contracts::shaped_text_run_ascent(&natural), shaping_contracts::shaped_text_run_ascent(&expected));
                    general.set_text_run_colors(line.run, &colors);
                    let actual = &direct.runs[shaping_contracts::shaped_text_run_backend_run(&natural).unwrap().0 as usize];
                    let expected = &general.runs[line.run.0 as usize];
                    assert_eq!(actual.character_bytes, expected.character_bytes);
                    assert!(actual.color_spans().eq(expected.color_spans()));
                    assert_eq!(actual.glyphs.len(), expected.glyphs.len());
                    for (actual, expected) in actual.iter_glyphs().zip(expected.iter_glyphs()) {
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
                let style = TextStyleSpan::new(
                    0..1,
                    shaping::style(19.0, 400, FontSlant::Normal, 0xff0000ff, Some("serif")),
                );
                let first = shaper.prepared_font(&(style).style);
                let resized = TextStyleSpan::new(
                    0..8,
                    shaping::style(26.0, 400, FontSlant::Normal, 0x0000ffff, Some("serif")),
                );
                let reused = shaper.prepared_font(&(resized).style);
                assert!(Arc::ptr_eq(&first.identity, &reused.identity));
                assert_eq!(first.font.features, reused.font.features);
                for (weight, slant, family) in [
                    (700, FontSlant::Normal, "serif"),
                    (400, FontSlant::Italic, "serif"),
                    (400, FontSlant::Normal, "monospace"),
                ] {
                    let distinct = shaper.prepared_font(
                        &(TextStyleSpan::new(
                            0..1,
                            shaping::style(19.0, weight, slant, 0xff0000ff, Some(family)),
                        ))
                        .style,
                    );
                    assert!(!Arc::ptr_eq(&first.identity, &distinct.identity));
                }
                let features = [open_type::open_type_feature_new(OpenTypeFeatureKind::StandardLigatures,
                    0,)];
                let mut disabled_style = style.clone();
                let mut feature_settings = features.to_vec();
                let resolved_features =
                    shaping::font_features(&mut feature_settings, &features, false);
                disabled_style.style.font_features = &resolved_features;
                let disabled = shaper.prepared_font(&(disabled_style).style);
                assert!(!Arc::ptr_eq(&first.identity, &disabled.identity));
                assert_eq!(
                    disabled
                        .font
                        .features
                        .tag_value_list()
                        .iter()
                        .find(|(tag, _)| tag == "liga")
                        .unwrap()
                        .1,
                    0
                );
                let overrides = [
                    open_type::open_type_feature_new(OpenTypeFeatureKind::StandardLigatures, 0),
                    open_type::open_type_feature_new(OpenTypeFeatureKind::StandardLigatures, 1),
                ];
                let mut enabled_style = style;
                let mut override_settings = overrides.to_vec();
                let resolved_overrides =
                    shaping::font_features(&mut override_settings, &overrides, false);
                enabled_style.style.font_features = &resolved_overrides;
                let enabled = shaper.prepared_font(&(enabled_style).style);
                assert_eq!(
                    enabled
                        .font
                        .features
                        .tag_value_list()
                        .iter()
                        .find(|(tag, _)| tag == "liga")
                        .unwrap()
                        .1,
                    1
                );
                for weight in 100..100 + FONT_SETUP_CACHE_CAPACITY as u16 * 10 {
                    shaper.prepared_font(
                        &(TextStyleSpan::new(
                            0..1,
                            shaping::style(
                                19.0,
                                weight,
                                FontSlant::Normal,
                                0xff0000ff,
                                Some("serif"),
                            ),
                        ))
                        .style,
                    );
                }
                assert_eq!(shaper.prepared_fonts.len(), FONT_SETUP_CACHE_CAPACITY);
                assert_eq!(
                    first
                        .font
                        .features
                        .tag_value_list()
                        .iter()
                        .find(|(tag, _)| tag == "liga")
                        .unwrap()
                        .1,
                    1,
                    "retained identities and features survive cache eviction"
                );
            })
            .unwrap();
    }

    #[test]
    fn byte_offsets_reused_for_paint_colors_keep_multibyte_and_combining_ranges() {
        let bytes = [0, 1, 3, 5, 9]; // a, combining acute, Hebrew, emoji
        let spans = paint_color_spans(CharacterBytes::Utf8(&bytes), &[0xff0000ff, 0xff0000ff, 0x0000ffff, 0x0000ffff]).unwrap();
        assert_eq!(spans, [(0..3, Hsla::from(rgba(0xff0000ff))), (3..9, Hsla::from(rgba(0x0000ffff)))]);
        assert!(paint_color_spans(CharacterBytes::Utf8(&bytes), &[0xff0000ff]).is_none());
        assert!(paint_color_spans(CharacterBytes::Ascii { start: 0, end: 1 }, &[]).is_none());
    }
}
