//! Token construction, normalization, cached summaries, and token-owned metrics.
//!
//! This module is the source-to-token boundary. It does not choose line breaks
//! or publish fragments.

use super::hyphenation::DeferredHyphenationCache;
use super::justification::{SpaceGlueClass, classify_space};
use super::storage::{TokenBuffer, WidthBuffer};
use super::wrapping::KpPlan;
use super::*;
use crate::layout::LayoutEngine;
use crate::layout::inline_reader::TextCursor;
use crate::layout_model::WhitespaceWrapOverride;
use html_text::{is_mandatory_line_break, is_word_separator, suppresses_glyph, tracking_boundary_allowed};
use std::mem::{discriminant, size_of};
use unicode_bidi::UNICODE_VERSION;

/// Normal justification consumes at most the glue declared by the paragraph:
/// 50% space expansion or 33% contraction, followed by bounded cluster tracking.
/// A separate emergency pass may choose an underfull line; rendering still
/// observes the same space and tracking limits.
pub(super) const KP_TOLERANCE: f64 = 1.0;
pub(super) const HYPHEN_PENALTY: f64 = 50.0;

/// Keep line-breaking plans only when the token stream is substantial enough
/// to amortize their retained storage across later layouts.
pub(super) const INLINE_KP_CACHE_MIN_TOKENS: usize = 64;
pub(super) const INLINE_SUMMARY_BLOCK_TOKENS: usize = 16;

/// Derives a final line box from ascent, descent, and a minimum height.
///
/// Callers use this both for raw font metrics and for the adjusted metrics after
/// vertical-align offsets have been applied.
pub(super) fn compute_baseline(max_ascent: f64, max_descent: f64, min_height: f64) -> (f64, f64) {
    let content_height = max_ascent + max_descent;
    let line_height = content_height.max(min_height);
    let has_extents = max_ascent != 0.0 || max_descent != 0.0;
    let baseline = if has_extents {
        max_ascent + (line_height - content_height).max(0.0) / 2.0
    } else {
        line_height * 0.8
    };
    (line_height, baseline)
}

/// Converts a computed vertical-alignment value into the upward offset used by
/// the fragment writer.
#[cfg(any(test, feature = "testing"))]
pub(super) fn vertical_align_offset(
    token: &InlineToken,
    runs: &[InlineTokenMetrics],
    line_height: f64,
    baseline: f64,
) -> f64 {
    let metrics = token.run_metrics(runs);
    let ascent = metrics.ascent as f64;
    let descent = metrics.descent as f64;
    // Half-leading surrounds text glyphs inside their authored line-height.
    // Replaced and atomic inline boxes contribute their margin-box geometry
    // directly; treating the font line-height as their own leading can make a
    // tall box produce negative half-leading and inflate `vertical-align: top`
    // or `bottom` line boxes.
    let half_leading = match token.kind() {
        InlineTokenKind::Glyph { .. }
        | InlineTokenKind::Ellipsis { .. }
        | InlineTokenKind::Discretionary { .. } => {
            (f64::from(metrics.line_height) - (ascent + descent)) / 2.0
        }
        _ => 0.0,
    };
    vertical_align_metrics_offset(
        metrics.vertical_align.as_style_value(),
        ascent,
        descent,
        f64::from(metrics.line_height),
        metrics.font_size as f64,
        half_leading,
        line_height,
        baseline,
        ascent,
        descent,
    )
}

pub(super) fn vertical_align_metrics_offset(
    value: VerticalAlignValue,
    ascent: f64,
    descent: f64,
    own_line_height: f64,
    font_size: f64,
    half_leading: f64,
    line_height: f64,
    baseline: f64,
    parent_font_ascent: f64,
    parent_font_descent: f64,
) -> f64 {
    let middle = baseline - ascent + (ascent + descent - line_height) / 2.0;
    match value {
        VerticalAlignValue::Baseline => 0.0,
        VerticalAlignValue::Super => font_size * 0.5,
        VerticalAlignValue::Sub => -(font_size * 0.25),
        VerticalAlignValue::Length(px) => px as f64,
        // CSS resolves a percentage baseline shift against this element's own
        // line-height, not the final height of the containing line box.
        VerticalAlignValue::Percent(pct) => own_line_height * pct as f64,
        VerticalAlignValue::Calc {
            absolute_px,
            line_height_fraction,
            ..
        } => absolute_px as f64 + own_line_height * line_height_fraction as f64,
        VerticalAlignValue::Top => baseline - ascent - half_leading,
        VerticalAlignValue::Bottom => baseline + descent + half_leading - line_height,
        // Unlike `top` and `bottom`, these values use the content area of the
        // parent font. They must not follow growth of the final line box.
        VerticalAlignValue::TextTop => parent_font_ascent - ascent - half_leading,
        VerticalAlignValue::TextBottom => descent + half_leading - parent_font_descent,
        VerticalAlignValue::Middle => middle,
    }
}

/// Resolves the indent to apply to the current line.
///
/// This keeps the first-line and hanging-indent policy out of the main line
/// breaking loop.
pub(in crate::layout) fn indent_for_line(
    text_indent: f64,
    hanging: bool,
    is_first_line: bool,
) -> f64 {
    if hanging && is_first_line {
        0.0
    } else if hanging || is_first_line {
        text_indent
    } else {
        0.0
    }
}

/// Text directly owned by a block or table cell lives in an anonymous inline
/// whose `vertical-align` is the initial `baseline`.  In particular, a table
/// cell's own value aligns the cell within its row; it must not also shift the
/// cell's text inside the line box.  A real inline owner, on the other hand,
/// contributes its authored alignment to the complete inline subtree.
pub(super) fn text_vertical_align(doc: &LayoutEngine, box_idx: usize) -> VerticalAlignValue {
    if doc.reader.is_inline_box(box_idx) {
        doc.reader.style(box_idx).vertical_align()
    } else {
        VerticalAlignValue::Baseline
    }
}

pub(super) fn has_aligned_inline_ancestor(doc: &LayoutEngine, box_idx: usize) -> bool {
    // Source text is wrapped by a conceptual anonymous inline box. Its own
    // non-inherited `vertical-align` is therefore `baseline`; alignment on
    // the element owning the text is movement of the enclosing box.
    let mut current = Some(box_idx);
    while let Some(ancestor_idx) = current {
        let ancestor_style = doc.reader.style(ancestor_idx);
        if ancestor_style.display() != html_style_model::Display::Inline {
            break;
        }
        if !ancestor_style.vertical_align().is_initial()
            || ancestor_style.position() == html_style_model::PositionMode::Relative
        {
            return true;
        }
        current = doc.reader.get_parent(ancestor_idx);
    }
    false
}

#[derive(Clone, Copy, PartialEq)]
// Four-byte alignment keeps the cached width beside the packed header without
// rounding the stride back up to 16 bytes. Read the header by value so Rust can
// generate an unaligned load where the target requires one.
#[repr(C, packed(4))]
pub(super) struct InlineToken {
    header: u64,
    pub(super) width: f32,
}

// Construction and exceptional paragraphs retain the wide representation.
// Prepared ordinary paragraphs instead store one u32 and one f32 per token.
const _: () = assert!(size_of::<InlineToken>() <= 12);

/// Numeric alignment is resolved against the owner's font size and this run's
/// own line-height before interning. Only the remaining keywords depend on
/// the containing line or parent font during placement.
#[derive(Clone, Copy, Debug)]
pub(super) enum InlineVerticalAlign {
    Baseline,
    Top,
    Bottom,
    TextTop,
    TextBottom,
    Middle,
    Offset(f32),
    // Preserve their identity because line breaking keeps scripts attached to
    // the preceding token, unlike an arbitrary numeric baseline shift.
    Sub(f32),
    Super(f32),
}

impl InlineVerticalAlign {
    pub(super) fn resolve(value: VerticalAlignValue, line_height: f64, font_size: f32) -> Self {
        match value {
            VerticalAlignValue::Baseline => Self::Baseline,
            VerticalAlignValue::Top => Self::Top,
            VerticalAlignValue::Bottom => Self::Bottom,
            VerticalAlignValue::TextTop => Self::TextTop,
            VerticalAlignValue::TextBottom => Self::TextBottom,
            VerticalAlignValue::Middle => Self::Middle,
            VerticalAlignValue::Sub => Self::Sub(-(font_size * 0.25)),
            VerticalAlignValue::Super => Self::Super(font_size * 0.5),
            VerticalAlignValue::Length(px) => Self::Offset(px),
            VerticalAlignValue::Percent(fraction) => {
                Self::Offset((line_height * f64::from(fraction)) as f32)
            }
            VerticalAlignValue::Calc { absolute_px, line_height_fraction, .. } => {
                Self::Offset((f64::from(absolute_px)
                    + line_height * f64::from(line_height_fraction)) as f32)
            }
        }
    }

    /// Reuse the shared placement calculation without retaining the larger
    /// computed-style enum in each metrics entry.
    #[inline]
    pub(super) fn as_style_value(self) -> VerticalAlignValue {
        match self {
            Self::Baseline => VerticalAlignValue::Baseline,
            Self::Top => VerticalAlignValue::Top,
            Self::Bottom => VerticalAlignValue::Bottom,
            Self::TextTop => VerticalAlignValue::TextTop,
            Self::TextBottom => VerticalAlignValue::TextBottom,
            Self::Middle => VerticalAlignValue::Middle,
            Self::Offset(px) | Self::Sub(px) | Self::Super(px) => VerticalAlignValue::Length(px),
        }
    }
}

impl PartialEq for InlineVerticalAlign {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Offset(a), Self::Offset(b))
            | (Self::Sub(a), Self::Sub(b))
            | (Self::Super(a), Self::Super(b)) => a.to_bits() == b.to_bits(),
            _ => discriminant(self) == discriminant(other),
        }
    }
}

const _: () = assert!(size_of::<InlineVerticalAlign>() <= 8);

#[derive(Clone, Copy, PartialEq)]
pub(super) struct InlineTokenMetrics {
    /// Prepared layout box that owns this run. Keeping ownership once per
    /// interned metrics run lets fragment publication recover inline ancestry
    /// without expanding the 12-byte dense token representation.
    pub(super) owner_box_idx: u32,
    pub(super) ascent: f32,
    pub(super) descent: f32,
    pub(super) line_height: f32,
    pub(super) font_size: f32,
    pub(super) vertical_align: InlineVerticalAlign,
    pub(super) white_space: WhiteSpace,
    pub(super) visible: bool,
    pub(super) placement_required: bool,
    // Directional controls retain source slots but own no glyph layout box.
    pub(super) contributes_line_metrics: bool,
}

const _: () = assert!(size_of::<InlineTokenMetrics>() <= 32);

impl InlineToken {
    #[inline]
    pub(super) fn metadata(&self) -> TokenMetadata {
        TokenMetadata {
            header: self.header,
        }
    }

    // The low word is shared by wide and SoA tokens: data 24, kind/glue 4,
    // break 2, shaping boundary 1, bidi continuation 1. Tags 10..14 are the
    // five space classes; tag 15 is a preserved tab. Compact reads need no
    // translation back into a different header layout.
    pub(super) const MAX_DATA: u32 = (1 << 24) - 1;
    const RUN_SHIFT: u32 = 32;
    const RUN_MASK: u32 = (1 << 18) - 1;
    pub(super) const MAX_RUN_IDX: u32 = Self::RUN_MASK - 1;
    const KIND_SHIFT: u32 = 24;
    const BREAK_SHIFT: u32 = 28;
    const CLUSTER_BOUNDARY_BIT: u64 = 1 << 30;
    // Bidi also joins Unicode Marks exposed as separate shaping clusters.
    // Wrapping retains the authoritative shaping boundary independently.
    const BIDI_CONTINUATION_BIT: u64 = 1 << 31;
    const WRAP_SHIFT: u32 = 50;
    const BIDI_SHIFT: u32 = 52;
    // Wide tokens can retain arbitrary combinations of kind, space, tab and
    // glue flags. The extended code uses the four high bits as well as the
    // low kind nibble; normal tokens keep these high bits zero.
    const EXTENDED_SHIFT: u32 = 60;
    const AUTHORED_BREAK_DATA_BIT: u32 = 1 << 2;

    pub(super) fn compact_header(&self) -> u32 {
        self.header as u32
    }
    #[inline]
    pub(super) fn from_compact(header: u32, width: f32, defaults: u64) -> Self {
        Self {
            header: u64::from(header)
                | (defaults & (((1u64 << Self::EXTENDED_SHIFT) - 1) & !u64::from(u32::MAX))),
            width,
        }
    }
    #[inline]
    fn code(&self) -> u32 {
        self.metadata().code()
    }

    fn flags(&self) -> (u32, bool, bool, SpaceGlueClass) {
        let code = self.code();
        match code {
            0..=9 => (code, false, false, SpaceGlueClass::Ordinary),
            10..=14 => (0, true, false, SpaceGlueClass::from_packed(code - 10)),
            15 => (0, false, true, SpaceGlueClass::Ordinary),
            _ => {
                let flags = (code - 16) / 10;
                (
                    (code - 16) % 10,
                    flags & 1 != 0,
                    flags & 2 != 0,
                    SpaceGlueClass::from_packed(flags >> 2),
                )
            }
        }
    }
    fn set_flags(&mut self, kind: u32, space: bool, tab: bool, glue: SpaceGlueClass) {
        let code = if !space && !tab && glue == SpaceGlueClass::Ordinary {
            kind
        } else if kind == 0 && space && !tab {
            10 + glue as u32
        } else if kind == 0 && !space && tab && glue == SpaceGlueClass::Ordinary {
            15
        } else {
            16 + kind + 10 * (u32::from(space) | (u32::from(tab) << 1) | ((glue as u32) << 2))
        };
        self.header = (self.header
            & !((15u64 << Self::KIND_SHIFT) | (15u64 << Self::EXTENDED_SHIFT)))
            | (u64::from(code & 15) << Self::KIND_SHIFT)
            | (u64::from(code >> 4) << Self::EXTENDED_SHIFT);
    }
    pub(super) fn mark_authored_break(&mut self) {
        debug_assert!(matches!(self.kind(), InlineTokenKind::Break { .. }));
        self.header |= u64::from(Self::AUTHORED_BREAK_DATA_BIT);
    }

    #[inline]
    pub(super) fn run_idx(&self) -> u32 {
        self.metadata().run_idx()
    }
    #[inline]
    pub(super) fn set_run_idx(&mut self, run_idx: u32) {
        assert!(
            run_idx <= Self::MAX_RUN_IDX,
            "inline token metrics index exceeds 18-bit capacity"
        );
        self.header = (self.header & !(u64::from(Self::RUN_MASK) << Self::RUN_SHIFT))
            | (u64::from(run_idx) << Self::RUN_SHIFT);
    }
    pub(super) fn bidi_level(&self) -> u8 {
        self.metadata().bidi_level()
    }
    pub(super) fn set_bidi_level(&mut self, level: u8) {
        self.header =
            (self.header & !(255u64 << Self::BIDI_SHIFT)) | (u64::from(level) << Self::BIDI_SHIFT);
    }
    #[inline]
    fn kind_parts(kind: InlineTokenKind) -> (u32, u32) {
        match kind {
            InlineTokenKind::Glyph { glyph_idx } => (0, glyph_idx),
            InlineTokenKind::Ellipsis { glyph } => (1, glyph),
            InlineTokenKind::Image { payload_idx } => (2, payload_idx),
            InlineTokenKind::AtomicBox { payload_idx } => (3, payload_idx),
            InlineTokenKind::Break { clear } => (4, clear as u32),
            InlineTokenKind::FloatAnchor { box_idx } => (5, box_idx),
            InlineTokenKind::InlineBoundary { payload_idx } => (6, payload_idx),
            InlineTokenKind::Discretionary { glyph } => (7, glyph),
            InlineTokenKind::Opportunity => (8, 0),
            InlineTokenKind::AbsoluteAnchor { box_idx } => (9, box_idx),
        }
    }

    fn set_kind(&mut self, kind: InlineTokenKind) {
        let (tag, data) = Self::kind_parts(kind);
        assert!(
            data <= Self::MAX_DATA,
            "inline token glyph/payload index exceeds 24-bit capacity"
        );
        let (_, space, tab, glue) = self.flags();
        self.set_flags(tag, space, tab, glue);
        self.header = (self.header & !u64::from(Self::MAX_DATA)) | u64::from(data);
    }
    #[inline]
    pub(super) fn new(
        kind: InlineTokenKind,
        width: f64,
        break_kind: BreakKind,
        wrap: TokenWrap,
        cluster_boundary_before: bool,
    ) -> Self {
        let (tag, data) = Self::kind_parts(kind);
        assert!(
            data <= Self::MAX_DATA,
            "inline token glyph/payload index exceeds 24-bit capacity"
        );
        Self {
            header: u64::from(data)
                | (u64::from(tag) << Self::KIND_SHIFT)
                | (u64::from(Self::RUN_MASK) << Self::RUN_SHIFT)
                | ((break_kind as u64) << Self::BREAK_SHIFT)
                | ((wrap as u64) << Self::WRAP_SHIFT)
                | if cluster_boundary_before {
                    Self::CLUSTER_BOUNDARY_BIT
                } else {
                    Self::BIDI_CONTINUATION_BIT
                },
            width: width as f32,
        }
    }
    #[inline]
    pub(super) fn kind(&self) -> InlineTokenKind {
        self.metadata().kind()
    }

    #[inline]
    pub(super) fn break_kind(&self) -> BreakKind {
        self.metadata().break_kind()
    }
    fn set_break_kind(&mut self, kind: BreakKind) {
        self.header =
            (self.header & !(3u64 << Self::BREAK_SHIFT)) | ((kind as u64) << Self::BREAK_SHIFT);
    }
    #[inline]
    pub(super) fn wrap(&self) -> TokenWrap {
        self.metadata().wrap()
    }
    #[inline]
    pub(super) fn is_cluster_boundary(&self) -> bool {
        self.metadata().is_cluster_boundary()
    }
    #[inline]
    pub(super) fn is_space(&self) -> bool {
        self.metadata().is_space()
    }
    pub(super) fn set_is_space(&mut self) {
        let (kind, _, tab, glue) = self.flags();
        self.set_flags(kind, true, tab, glue);
    }
    pub(super) fn set_is_tab(&mut self) {
        let (kind, space, _, glue) = self.flags();
        self.set_flags(kind, space, true, glue);
    }

    pub(super) fn set_bidi_continuation(&mut self) {
        self.header |= Self::BIDI_CONTINUATION_BIT;
    }

    pub(super) fn set_space_glue_class(&mut self, class: SpaceGlueClass) {
        let (kind, space, tab, _) = self.flags();
        self.set_flags(kind, space, tab, class);
    }

    #[inline]
    pub(super) fn width(&self) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) {
            0.0
        } else {
            self.width as f64
        }
    }

    #[inline]
    pub(super) fn run_metrics<'a>(&self, runs: &'a [InlineTokenMetrics]) -> &'a InlineTokenMetrics {
        self.metadata().run_metrics(runs)
    }

    #[inline]
    pub(super) fn advance_at(
        &self,
        reader: &LayoutReader<'_>,
        runs: &[InlineTokenMetrics],
        position_from_block_start: f64,
    ) -> f64 {
        if !self.is_tab() {
            return self.width();
        }
        self.metadata()
            .tab_advance_at(reader, self.run_metrics(runs), position_from_block_start)
    }

    #[inline]
    pub(super) fn is_tab(&self) -> bool {
        self.metadata().is_tab()
    }
}

/// Read-only token information. Compact reads restore any shared bidi level;
/// widths and paragraph wrap defaults are accessed independently.
#[derive(Clone, Copy)]
pub(super) struct TokenMetadata {
    header: u64,
}
impl TokenMetadata {
    #[inline]
    pub(super) fn from_defaults(header: u64) -> Self {
        Self { header }
    }
    #[inline]
    pub(super) fn from_compact_with_bidi(header: u32, level: u8) -> Self {
        Self {
            header: u64::from(header) | (u64::from(level) << InlineToken::BIDI_SHIFT),
        }
    }
    #[inline]
    fn code(&self) -> u32 {
        ((self.header >> InlineToken::KIND_SHIFT) as u32 & 15)
            | (((self.header >> InlineToken::EXTENDED_SHIFT) as u32) << 4)
    }
    #[inline]
    fn kind_tag(&self) -> u32 {
        let code = self.code();
        match code {
            0..=9 => code,
            10..=15 => 0,
            _ => (code - 16) % 10,
        }
    }
    #[inline]
    fn data(&self) -> u32 {
        self.header as u32 & InlineToken::MAX_DATA
    }
    #[inline]
    pub(super) fn run_idx(&self) -> u32 {
        (self.header >> InlineToken::RUN_SHIFT) as u32 & InlineToken::RUN_MASK
    }
    #[inline]
    pub(super) fn bidi_level(&self) -> u8 {
        (self.header >> InlineToken::BIDI_SHIFT) as u8
    }
    #[inline]
    pub(super) fn kind(&self) -> InlineTokenKind {
        let data = self.data();
        match self.kind_tag() {
            0 => InlineTokenKind::Glyph { glyph_idx: data },
            1 => InlineTokenKind::Ellipsis { glyph: data },
            2 => InlineTokenKind::Image { payload_idx: data },
            3 => InlineTokenKind::AtomicBox { payload_idx: data },
            4 => InlineTokenKind::Break {
                clear: match data & 3 {
                    0 => html_style_model::Clear::None,
                    1 => html_style_model::Clear::Left,
                    2 => html_style_model::Clear::Right,
                    3 => html_style_model::Clear::Both,
                    _ => unreachable!("invalid packed clear value"),
                },
            },
            5 => InlineTokenKind::FloatAnchor { box_idx: data },
            6 => InlineTokenKind::InlineBoundary { payload_idx: data },
            7 => InlineTokenKind::Discretionary { glyph: data },
            8 => InlineTokenKind::Opportunity,
            9 => InlineTokenKind::AbsoluteAnchor { box_idx: data },
            _ => unreachable!("invalid packed inline-token kind"),
        }
    }
    #[inline]
    pub(super) fn break_kind(&self) -> BreakKind {
        match (self.header >> InlineToken::BREAK_SHIFT) & 3 {
            0 => BreakKind::None,
            1 => BreakKind::Soft,
            2 => BreakKind::Hard,
            3 => BreakKind::Discretionary,
            _ => unreachable!("invalid packed inline-token break kind"),
        }
    }
    /// A regular glyph with no break or extended space/tab flags. Shaping
    /// and bidi boundaries do not affect its advance within a fitting line.
    #[inline]
    pub(super) fn is_unbroken_glyph(&self) -> bool {
        let mask = (15u64 << InlineToken::EXTENDED_SHIFT)
            | (63u64 << InlineToken::KIND_SHIFT);
        self.header & mask == 0
    }
    #[inline]
    pub(super) fn wrap(&self) -> TokenWrap {
        match (self.header >> InlineToken::WRAP_SHIFT) & 3 {
            0 => TokenWrap::Normal,
            1 => TokenWrap::BreakWord,
            2 => TokenWrap::Anywhere,
            _ => unreachable!("invalid packed inline-token wrap kind"),
        }
    }
    #[inline]
    pub(super) fn is_cluster_boundary(&self) -> bool {
        self.header & InlineToken::CLUSTER_BOUNDARY_BIT != 0
    }
    #[inline]
    pub(super) fn continues_bidi_cluster(&self) -> bool {
        self.header & InlineToken::BIDI_CONTINUATION_BIT != 0
    }
    #[inline]
    pub(super) fn is_space(&self) -> bool {
        let code = self.code();
        if code < 16 {
            code.wrapping_sub(10) < 5
        } else {
            ((code - 16) / 10) & 1 != 0
        }
    }
    #[inline]
    pub(super) fn space_glue_class(&self) -> SpaceGlueClass {
        let code = self.code();
        if code < 16 {
            SpaceGlueClass::from_packed(code.wrapping_sub(10))
        } else {
            SpaceGlueClass::from_packed(((code - 16) / 10) >> 2)
        }
    }
    #[inline]
    pub(super) fn is_tab(&self) -> bool {
        if self.code() < 16 {
            self.code() == 15
        } else {
            ((self.code() - 16) / 10) & 2 != 0
        }
    }
    #[inline]
    pub(super) fn is_authored_break(&self) -> bool {
        self.kind_tag() == 4 && self.data() & InlineToken::AUTHORED_BREAK_DATA_BIT != 0
    }
    #[inline]
    pub(super) fn run_metrics<'a>(&self, runs: &'a [InlineTokenMetrics]) -> &'a InlineTokenMetrics {
        &runs[self.run_idx() as usize]
    }
    #[inline]
    pub(super) fn width(self, stored_width: f32) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) {
            0.0
        } else {
            f64::from(stored_width)
        }
    }
    #[inline]
    pub(super) fn discretionary_width(self, stored_width: f32) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) {
            f64::from(stored_width)
        } else {
            0.0
        }
    }
    #[inline]
    pub(super) fn advance_with_metrics(
        &self,
        stored_width: f32,
        reader: &LayoutReader<'_>,
        metrics: &InlineTokenMetrics,
        position_from_block_start: f64,
    ) -> f64 {
        if !self.is_tab() {
            return self.width(stored_width);
        }
        self.tab_advance_at(reader, metrics, position_from_block_start)
    }

    fn tab_advance_at(
        &self,
        reader: &LayoutReader<'_>,
        metrics: &InlineTokenMetrics,
        position_from_block_start: f64,
    ) -> f64 {
        let box_idx = metrics.owner_box_idx as usize;
        let (interval, minimum_advance) = preserved_tab_metrics(reader, box_idx);
        if interval <= 0.0 {
            return 0.0;
        }
        let remainder = position_from_block_start.rem_euclid(interval);
        let mut advance = if remainder <= f64::EPSILON {
            interval
        } else {
            interval - remainder
        };
        if advance < minimum_advance {
            advance += interval;
        }
        advance
    }
}
#[derive(Clone, Copy, PartialEq)]
pub(super) enum InlineTokenKind {
    Glyph {
        glyph_idx: u32,
    },
    Ellipsis {
        glyph: crate::GlyphId,
    },
    Image {
        payload_idx: u32,
    },
    AtomicBox {
        payload_idx: u32,
    },
    Break {
        clear: html_style_model::Clear,
    },
    FloatAnchor {
        box_idx: u32,
    },
    AbsoluteAnchor {
        box_idx: u32,
    },
    InlineBoundary {
        payload_idx: u32,
    },
    Discretionary {
        glyph: crate::GlyphId,
    },
    /// A zero-width soft wrapping opportunity after an atomic inline box.
    /// It uses discretionary break metadata, but unlike a discretionary
    /// hyphen it emits no glyph when selected.
    Opportunity,
}

/// Large, uncommon replaced-element geometry lives outside the dense token
/// stream. Most documents now scan compact fixed-width tokens for every text
/// pass instead of dragging image-sized enum storage through cache lines.
#[derive(Clone, PartialEq)]
pub(super) enum ReplacedToken {
    Image {
        image_idx: u32,
        box_idx: u32,
        content_size: Size,
        border_size: Size,
        content_inset: Point,
        margin_left: f64,
        margin_top: f64,
        position_offset: Vec2,
        set_box_geometry: bool,
    },
    AtomicBox {
        box_idx: u32,
        border_size: Size,
        containing_width: f64,
        containing_height: Option<f64>,
        margin_left: f64,
        margin_top: f64,
    },
    InlineBoundary {
        box_idx: u32,
        margin_left: f64,
        left_inset: f64,
        inline_start: bool,
        inline_end: bool,
    },
}

/// Bidi facts shared by a summary block or selected line. Seed a nonempty
/// span with its first level before merging the remaining tokens or blocks.
#[derive(Clone, Copy, Default)]
pub(super) struct InlineBidiSummary {
    pub(super) has_odd_level: bool,
    // 255 is outside the valid bidi-level range and denotes mixed levels.
    uniform_level: u8,
}

impl InlineBidiSummary {
    #[inline]
    pub(super) fn from_level(level: u8) -> Self {
        Self {
            has_odd_level: level % 2 == 1,
            uniform_level: level,
        }
    }

    #[inline]
    pub(super) fn merge(&mut self, other: Self) {
        self.has_odd_level |= other.has_odd_level;
        if self.uniform_level != other.uniform_level {
            self.uniform_level = u8::MAX;
        }
    }

    #[inline]
    pub(super) fn uniform_level(self) -> Option<u8> {
        (self.uniform_level != u8::MAX).then_some(self.uniform_level)
    }

    #[inline]
    pub(super) fn is_uniform_at(self, level: u8) -> bool {
        self.uniform_level == level
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct InlineSummaryBlock {
    // A plain block covers consecutive glyphs, so its end follows from the
    // fixed block size rather than occupying another field and padding.
    pub(super) glyph_start: u32,
    pub(super) preserves_newlines: bool,
    pub(super) plain_text: bool,
    pub(super) bidi: InlineBidiSummary,
    pub(super) width: f64,
    pub(super) max_ascent: f64,
    pub(super) max_descent: f64,
}

// The uniform-level byte occupies existing padding, not another token array.
const _: () = assert!(size_of::<InlineSummaryBlock>() == 32);

impl InlineSummaryBlock {
    fn build(
        tokens: &[InlineToken],
        runs: &[InlineTokenMetrics],
        mut observe: impl FnMut(&InlineToken),
    ) -> Self {
        let first_level = tokens
            .first()
            .map_or(0, |token| token.metadata().bidi_level());
        let facts = tokens.iter().map(|token| {
            observe(token);
            (token.metadata(), token.width, token.run_idx())
        });
        Self::from_facts(first_level, facts, runs)
    }

    fn include_metrics(&mut self, metrics: &InlineTokenMetrics) {
        self.plain_text &= metrics.contributes_line_metrics
            && !metrics.placement_required
            && metrics.visible
            && matches!(metrics.vertical_align, InlineVerticalAlign::Baseline);
        self.preserves_newlines |= metrics.white_space.preserves_newlines();
        let ascent = metrics.ascent as f64;
        let descent = metrics.descent as f64;
        let half_leading = (f64::from(metrics.line_height) - (ascent + descent)) / 2.0;
        self.max_ascent = self.max_ascent.max(ascent + half_leading);
        self.max_descent = self.max_descent.max(descent + half_leading);
    }

    fn finish(mut self) -> Self {
        if !self.max_ascent.is_finite() {
            self.max_ascent = 0.0;
        }
        if !self.max_descent.is_finite() {
            self.max_descent = 0.0;
        }
        self
    }

    /// Direct compact construction already proves default LTR ordering.
    /// One association covers this entire block, so its glyph loop reads only
    /// the metadata and width columns; metrics contribute once afterward.
    fn from_shared_compact(headers: &[u32], widths: &[f32], metrics: &InlineTokenMetrics) -> Self {
        let mut summary = Self {
            glyph_start: u32::MAX,
            max_ascent: f64::NEG_INFINITY,
            max_descent: f64::NEG_INFINITY,
            plain_text: !headers.is_empty(),
            ..Self::default()
        };
        let mut expected_glyph = None;
        for (&header, &stored_width) in headers.iter().zip(widths) {
            let token = TokenMetadata::from_compact_with_bidi(header, 0);
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                summary.plain_text = false;
                continue;
            };
            summary.glyph_start = summary.glyph_start.min(glyph_idx);
            summary.plain_text &=
                expected_glyph.is_none_or(|expected| expected == glyph_idx) && !token.is_tab();
            expected_glyph = Some(glyph_idx + 1);
            // Preserve the established scalar f64 accumulation order.
            summary.width += token.width(stored_width);
        }
        // A block containing only discretionary/opportunity tokens contributes
        // no glyph metrics, just as in the general summary path.
        if summary.glyph_start != u32::MAX {
            summary.include_metrics(metrics);
        }
        summary.finish()
    }

    fn from_facts(
        first_level: u8,
        tokens: impl ExactSizeIterator<Item = (TokenMetadata, f32, u32)>,
        runs: &[InlineTokenMetrics],
    ) -> Self {
        let mut level_differences = 0;
        let mut summary = Self {
            glyph_start: u32::MAX,
            max_ascent: f64::NEG_INFINITY,
            max_descent: f64::NEG_INFINITY,
            plain_text: tokens.len() != 0,
            ..Self::default()
        };
        let mut expected_glyph = None;
        let mut previous_metrics = None;
        for (token, stored_width, index) in tokens {
            level_differences |= token.bidi_level() ^ first_level;
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                summary.plain_text = false;
                continue;
            };
            // Consecutive glyphs usually share metrics. Their eligibility and
            // extents contribute only once until the metrics index changes.
            if previous_metrics != Some(index) {
                summary.include_metrics(&runs[index as usize]);
                previous_metrics = Some(index);
            }
            summary.glyph_start = summary.glyph_start.min(glyph_idx);
            summary.plain_text &=
                expected_glyph.is_none_or(|expected| expected == glyph_idx) && !token.is_tab();
            expected_glyph = Some(glyph_idx + 1);
            summary.width += token.width(stored_width);
        }
        // Zero differences prove uniformity. Their low bit also records any
        // odd level when the first level is even, avoiding per-token branches.
        summary.bidi = InlineBidiSummary {
            has_odd_level: (first_level | level_differences) % 2 == 1,
            uniform_level: if level_differences == 0 {
                first_level
            } else {
                u8::MAX
            },
        };
        summary.finish()
    }
}

/// Paragraph properties collected with the summaries and retained with the
/// token plan. Reading them during composition never revisits token storage.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PreparedTokenFeatures {
    pub(super) has_tabs: bool,
    pub(super) preserves_spaces: bool,
    pub(super) has_float_anchors: bool,
    flags: u8,
}

const _: () = assert!(size_of::<PreparedTokenFeatures>() == 4);

impl PreparedTokenFeatures {
    const REQUIRES_WIDE: u8 = 1;
    const NO_BIDI_GLYPH_SEPARATORS: u8 = 1 << 1;
    const HAS_BREAKS: u8 = 1 << 2;
    const DEFERRED_HYPHENATION: u8 = 1 << 3;

    pub(super) fn deferred_hyphenation(self) -> bool {
        self.flags & Self::DEFERRED_HYPHENATION != 0
    }

    pub(super) fn set_deferred_hyphenation(&mut self, deferred: bool) {
        self.flags = (self.flags & !Self::DEFERRED_HYPHENATION)
            | (u8::from(deferred) * Self::DEFERRED_HYPHENATION);
    }

    /// False is unknown. Text emission proves this from source characters;
    /// metadata-only summary rebuilds cannot establish it on their own.
    #[inline]
    pub(super) fn no_bidi_glyph_separators(self) -> bool {
        self.flags & Self::NO_BIDI_GLYPH_SEPARATORS != 0
    }

    #[inline]
    pub(super) fn set_no_bidi_glyph_separators(&mut self, proven: bool) {
        self.flags = (self.flags & !Self::NO_BIDI_GLYPH_SEPARATORS)
            | (u8::from(proven) * Self::NO_BIDI_GLYPH_SEPARATORS);
    }

    #[inline]
    pub(super) fn no_bidi_separators(self) -> bool {
        self.flags & (Self::NO_BIDI_GLYPH_SEPARATORS | Self::HAS_BREAKS)
            == Self::NO_BIDI_GLYPH_SEPARATORS
    }
}

/// Unicode 16 B/S characters occur only among C0 controls, NEL and PARAGRAPH
/// SEPARATOR. Other data versions keep full classification until verified.
#[inline]
pub(super) fn may_be_bidi_separator(character: char) -> bool {
    !matches!(UNICODE_VERSION, (16, 0, 0))
        || character <= '\u{001f}'
        || matches!(character, '\u{0085}' | '\u{2029}')
}

#[derive(Default)]
pub(super) struct InlineTokens<'a> {
    pub(super) metadata: TokenBuffer<'a, u32>,
    pub(super) widths: WidthBuffer<'a>,
    pub(super) compact_defaults: Option<u64>,
    pub(super) dense: TokenBuffer<'a, InlineToken>,
    pub(super) runs: TokenBuffer<'a, InlineTokenMetrics>,
    pub(super) summary_blocks: TokenBuffer<'a, InlineSummaryBlock>,
    pub(super) replaced: TokenBuffer<'a, ReplacedToken>,
    pub(super) metrics_mode: MetricsMode,
    pub(super) metrics_ranges: TokenBuffer<'a, MetricsRange>,
    pub(super) features: PreparedTokenFeatures,
    pub(super) kp_plan: Option<KpPlan>,
    pub(super) deferred_hyphens: Option<Box<DeferredHyphenationCache>>,
}

impl<'a> InlineTokens<'a> {
    pub(super) fn span(&self) -> TokenSpan<'_> {
        if self.compact_defaults.is_some() {
            TokenSpan::Compact {
                source: self,
                start: 0,
                len: self.metadata.len() as u32,
            }
        } else {
            TokenSpan::Wide(&self.dense)
        }
    }
    pub(super) fn len(&self) -> usize {
        self.span().len()
    }
    pub(super) fn metadata_iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = TokenMetadata> + ExactSizeIterator {
        self.span().metadata_iter()
    }
    /// Construction has already established uniform wrap/LTR state and
    /// recorded metrics transitions while appending the two compact columns.
    fn finish_compact_metrics(&mut self) {
        if self.metrics_ranges.len() == 1 {
            let index = self.metrics_ranges[0].metrics;
            self.metrics_mode = MetricsMode::Uniform(index);
            self.compact_defaults = Some(u64::from(index) << InlineToken::RUN_SHIFT);
            self.metrics_ranges.clear();
        } else {
            self.metrics_mode = MetricsMode::Ranges;
            self.compact_defaults =
                Some(u64::from(InlineToken::RUN_MASK) << InlineToken::RUN_SHIFT);
        }
    }

    pub(super) fn expand_compact_for_build(&mut self, capacity: usize) {
        self.finish_compact_metrics();
        let mut dense = std::mem::take(&mut self.dense);
        dense.clear();
        dense.reserve(capacity.max(self.metadata.len()));
        self.span().append_to(dense.make_mut());
        self.dense = dense;
        // These columns were speculative construction storage. Release them
        // before continuing with wide tokens rather than retaining both forms.
        self.metadata = TokenBuffer::default();
        self.widths = WidthBuffer::default();
        self.compact_defaults = None;
        self.metrics_mode = MetricsMode::Unprepared;
        self.metrics_ranges.clear();
    }

    fn classify_compact_space_glue(&mut self, engine: &LayoutEngine<'_, '_>) {
        profile_scope!("tokens.classify_glue");
        fn context(
            engine: &LayoutEngine<'_, '_>,
            headers: &[u32],
            mut index: usize,
            direction: isize,
        ) -> Option<char> {
            loop {
                index = index.checked_add_signed(direction)?;
                match TokenMetadata::from_compact_with_bidi(*headers.get(index)?, 0).kind() {
                    InlineTokenKind::Glyph { glyph_idx } => {
                        return Some(engine.text.character(glyph_idx));
                    }
                    InlineTokenKind::Opportunity => {}
                    _ => return None,
                }
            }
        }
        let headers = self.metadata.make_mut();
        for index in 0..headers.len() {
            if !TokenMetadata::from_compact_with_bidi(headers[index], 0).is_space() {
                continue;
            }
            let previous = context(engine, headers, index, -1);
            let next = context(engine, headers, index, 1);
            let class = classify_space(previous, next);
            // Direct construction contains ordinary glyph spaces, whose five
            // glue classes occupy tags 10..14 in the low metadata word.
            headers[index] = (headers[index] & !(15 << InlineToken::KIND_SHIFT))
                | ((10 + class as u32) << InlineToken::KIND_SHIFT);
        }
    }

    fn rebuild_direct_summary_blocks(&mut self) {
        profile_scope!("tokens.summary_blocks");
        if self.metadata.is_empty() {
            self.compact_defaults = None;
            self.rebuild_summary_blocks();
            return;
        }
        if self.metadata.len() > InlineToken::MAX_DATA as usize {
            self.expand_compact_for_build(self.metadata.len());
            self.rebuild_summary_blocks();
            self.compact();
            return;
        }
        self.finish_compact_metrics();
        // Eligibility excludes preserved spaces and transparent non-text
        // tokens. Edge trimming therefore reads only contiguous metadata;
        // neither direction needs glyph or metrics lookups.
        let soft = |header: &u32| {
            TokenMetadata::from_compact_with_bidi(*header, 0).break_kind() == BreakKind::Soft
        };
        let leading = self
            .metadata
            .iter()
            .take_while(|header| soft(header))
            .count();
        let trailing = self.metadata[leading..]
            .iter()
            .rev()
            .take_while(|header| soft(header))
            .count();
        let end = self.widths.len();
        let widths = self.widths.make_mut();
        widths[..leading].fill(0.0);
        widths[end - trailing..].fill(0.0);
        self.features = PreparedTokenFeatures {
            flags: self.features.flags
                & (PreparedTokenFeatures::NO_BIDI_GLYPH_SEPARATORS
                    | PreparedTokenFeatures::DEFERRED_HYPHENATION),
            ..PreparedTokenFeatures::default()
        };
        if matches!(self.metrics_mode, MetricsMode::Uniform(_)) {
            self.build_direct_summaries::<false>();
        } else {
            self.build_direct_summaries::<true>();
        }
    }

    fn build_direct_summaries<const RANGED: bool>(&mut self) {
        let uniform = match self.metrics_mode {
            MetricsMode::Uniform(index) => index,
            _ => 0,
        };
        let mut range = 0;
        let ranges = &self.metrics_ranges;
        let runs = &self.runs;
        let summaries = self.summary_blocks.make_mut();
        summaries.clear();
        summaries.extend(
            self.metadata
                .chunks(INLINE_SUMMARY_BLOCK_TOKENS)
                .zip(self.widths.make_mut().chunks(INLINE_SUMMARY_BLOCK_TOKENS))
                .enumerate()
                .map(|(block, (headers, widths))| {
                    let start = block * INLINE_SUMMARY_BLOCK_TOKENS;
                    let shared = if RANGED {
                        while ranges[range].end as usize <= start {
                            range += 1;
                        }
                        let current = ranges[range];
                        (current.end as usize >= start + headers.len()).then_some(current.metrics)
                    } else {
                        Some(uniform)
                    };
                    if let Some(index) = shared {
                        InlineSummaryBlock::from_shared_compact(
                            headers,
                            widths,
                            &runs[index as usize],
                        )
                    } else {
                        let facts = headers.iter().zip(widths).enumerate().map(
                            |(offset, (&header, &width))| {
                                let index = if RANGED {
                                    while ranges[range].end as usize <= start + offset {
                                        range += 1;
                                    }
                                    ranges[range].metrics
                                } else {
                                    uniform
                                };
                                (
                                    TokenMetadata::from_compact_with_bidi(header, 0),
                                    width,
                                    index,
                                )
                            },
                        );
                        InlineSummaryBlock::from_facts(0, facts, runs)
                    }
                }),
        );
    }

    /// Uniform bidi and wrap values live in paragraph defaults. Mixed values,
    /// dense metrics associations and extended flag combinations stay wide.
    pub(super) fn compact(&mut self) {
        let Some(first) = self.dense.first() else {
            return;
        };
        let shared = first.header
            & (((1u64 << InlineToken::EXTENDED_SHIFT) - 1)
                & !((1u64 << InlineToken::WRAP_SHIFT) - 1));
        if !matches!(
            self.metrics_mode,
            MetricsMode::Uniform(_) | MetricsMode::Ranges
        ) || self.dense.len() > InlineToken::MAX_DATA as usize
            || self.features.flags & PreparedTokenFeatures::REQUIRES_WIDE != 0
        {
            return;
        }
        let index = match self.metrics_mode {
            MetricsMode::Uniform(index) => index,
            _ => InlineToken::RUN_MASK,
        };
        self.compact_defaults = Some((u64::from(index) << InlineToken::RUN_SHIFT) | shared);
        self.metadata.clear();
        self.widths.clear();
        // Acquire each owned destination once. Exact-length slice iterators
        // let Vec reserve once and write each contiguous column without a
        // Cow ownership check or a capacity check for every token.
        self.metadata
            .make_mut()
            .extend(self.dense.iter().map(InlineToken::compact_header));
        self.widths
            .make_mut()
            .extend(self.dense.iter().map(|token| token.width));
        self.dense.clear();
    }
    pub(super) fn metrics_view(&self) -> MetricsView<'_> {
        match self.metrics_mode {
            MetricsMode::Uniform(index) => MetricsView::Uniform(&self.runs[index as usize]),
            MetricsMode::Ranges => MetricsView::Ranges {
                source: self,
                offset: 0,
            },
            MetricsMode::Unprepared | MetricsMode::Dense(_) => MetricsView::direct(&self.runs),
        }
    }

    #[inline]
    pub(super) fn compact_bidi_level(&self) -> u8 {
        TokenMetadata::from_defaults(self.compact_defaults.expect("compact token defaults"))
            .bidi_level()
    }

    /// Transform one paragraph's dense tokens while moving its borrowed
    /// metric and payload views temporarily. Restore those views after emission.
    pub(super) fn take_paragraph(
        &mut self,
        range: Range<usize>,
        breaks: Option<(usize, usize)>,
    ) -> Self {
        let start = range.start;
        let dense = self
            .span()
            .slice(range)
            .to_vec()
            .into_iter()
            .enumerate()
            .filter(|(index, _)| {
                !breaks.is_some_and(|(a, b)| start + *index == a || start + *index == b)
            })
            .map(|(_, token)| token)
            .collect();
        let mut paragraph = Self {
            dense,
            runs: std::mem::take(&mut self.runs),
            replaced: std::mem::take(&mut self.replaced),
            features: PreparedTokenFeatures {
                flags: self.features.flags
                    & (PreparedTokenFeatures::NO_BIDI_GLYPH_SEPARATORS
                        | PreparedTokenFeatures::DEFERRED_HYPHENATION),
                ..PreparedTokenFeatures::default()
            },
            ..Self::default()
        };
        paragraph.rebuild_summary_blocks();
        paragraph.compact();
        paragraph
    }

    pub(super) fn restore_paragraph(&mut self, paragraph: Self) {
        self.runs = paragraph.runs;
        self.replaced = paragraph.replaced;
    }

    #[cfg(any(test, feature = "testing"))]
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            dense: Vec::with_capacity(capacity).into(),
            metadata: TokenBuffer::default(),
            widths: WidthBuffer::default(),
            compact_defaults: None,
            runs: TokenBuffer::default(),
            summary_blocks: Vec::with_capacity(capacity.div_ceil(INLINE_SUMMARY_BLOCK_TOKENS))
                .into(),
            replaced: TokenBuffer::default(),
            metrics_mode: MetricsMode::Unprepared,
            metrics_ranges: TokenBuffer::default(),
            features: PreparedTokenFeatures::default(),
            kp_plan: None,
            ..Self::default()
        }
    }

    pub(super) fn bind_metrics_in(
        runs: &mut TokenBuffer<'_, InlineTokenMetrics>,
        previous_run_idx: Option<u32>,
        token: &mut InlineToken,
        metrics: InlineTokenMetrics,
    ) -> u32 {
        if let Some(run_idx) = previous_run_idx
            && runs
                .get(run_idx as usize)
                .is_some_and(|previous| *previous == metrics)
        {
            token.set_run_idx(run_idx);
            return run_idx;
        }

        let run_idx = runs
            .iter()
            .position(|candidate| *candidate == metrics)
            .unwrap_or_else(|| {
                runs.push(metrics);
                runs.len() - 1
            });
        let run_idx = u32::try_from(run_idx).expect("inline run-metrics arena capacity exhausted");
        token.set_run_idx(run_idx);
        run_idx
    }

    pub(super) fn bind_metrics(
        &mut self,
        mut token: InlineToken,
        metrics: InlineTokenMetrics,
    ) -> InlineToken {
        let previous_run_idx = self.dense.last().map(|previous| previous.run_idx());
        Self::bind_metrics_in(&mut self.runs, previous_run_idx, &mut token, metrics);
        token
    }

    pub(super) fn push(&mut self, token: InlineToken, metrics: InlineTokenMetrics) {
        // Generic pushes do not supply source characters. Preserve the proof
        // only for synthetic/replaced tokens; text emission tracks glyphs.
        match token.kind() {
            InlineTokenKind::Glyph { .. } => {
                self.features.flags &= !PreparedTokenFeatures::NO_BIDI_GLYPH_SEPARATORS;
            }
            InlineTokenKind::Break { .. } => {
                self.features.flags |= PreparedTokenFeatures::HAS_BREAKS;
            }
            _ => {}
        }
        if self.compact_defaults.is_some() {
            self.dense = self.span().to_vec().into();
            self.metadata.clear();
            self.widths.clear();
            self.compact_defaults = None;
        }
        self.metrics_mode = MetricsMode::Unprepared;
        self.metrics_ranges.clear();
        let token = self.bind_metrics(token, metrics);
        self.dense.push(token);
    }

    fn push_replaced(
        &mut self,
        payload: ReplacedToken,
        kind: impl FnOnce(u32) -> InlineTokenKind,
        mut token: InlineToken,
        metrics: InlineTokenMetrics,
        allow_wrap: bool,
    ) {
        let opportunity = InlineToken::new(
            InlineTokenKind::Opportunity,
            0.0,
            BreakKind::Discretionary,
            TokenWrap::Normal,
            true,
        );
        if allow_wrap {
            self.push(opportunity, metrics);
        }
        let payload_idx = u32::try_from(self.replaced.len())
            .expect("inline replaced-token arena capacity exhausted");
        self.replaced.push(payload);
        token.set_kind(kind(payload_idx));
        self.push(token, metrics);
        if allow_wrap {
            self.push(opportunity, metrics);
        }
    }

    /// Resolve synthetic object opportunities against their actual logical
    /// neighbours, including styles outside transparent inline wrappers.
    fn resolve_atomic_break_opportunities(&mut self, engine: &LayoutEngine<'_, '_>) {
        let atomic = |token: InlineToken| {
            matches!(
                token.kind(),
                InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }
            )
        };
        if !self.dense.iter().copied().any(atomic) {
            return;
        }
        let neighbour = |token: InlineToken| {
            let character = match token.kind() {
                InlineTokenKind::Glyph { glyph_idx } => engine.text.character(glyph_idx),
                InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. } => '\u{fffc}',
                InlineTokenKind::Break { .. } => '\n',
                _ => return None,
            };
            Some((
                token.run_metrics(&self.runs).owner_box_idx as usize,
                character,
            ))
        };
        let mut resolved = Vec::with_capacity(self.dense.len());
        for (index, token) in self.dense.iter().copied().enumerate() {
            if matches!(token.kind(), InlineTokenKind::Opportunity) {
                let left = self.dense[..index]
                    .iter()
                    .rev()
                    .copied()
                    .find(|token| neighbour(*token).is_some());
                let right = self.dense[index + 1..]
                    .iter()
                    .copied()
                    .find(|token| neighbour(*token).is_some());
                if left.is_some_and(atomic) || right.is_some_and(atomic) {
                    let allowed = left
                        .and_then(neighbour)
                        .zip(right.and_then(neighbour))
                        .is_some_and(|(left, right)| {
                            left.1 != '\n'
                                && right.1 != '\n'
                                && html_text::atomic_boundary_wraps(
                                    engine.reader.styles(),
                                    engine.reader.layout_tree(),
                                    left,
                                    right,
                                )
                        });
                    if !allowed {
                        continue;
                    }
                }
            }
            resolved.push(token);
        }
        *self.dense.make_mut() = resolved;
    }

    pub(super) fn is_empty(&self) -> bool {
        self.span().is_empty()
    }

    /// Find edge whitespace during the forward summary pass. Transparent
    /// inline boundaries retain their geometry while adjacent collapsible
    /// spaces lose their advance. Only affected summary blocks are rebuilt.
    pub(super) fn rebuild_summary_blocks(&mut self) {
        profile_scope!("tokens.summary_blocks");
        let transparent_boundary = |token: &InlineToken| {
            matches!(
                token.kind(),
                InlineTokenKind::InlineBoundary { .. }
                    | InlineTokenKind::FloatAnchor { .. }
                    | InlineTokenKind::AbsoluteAnchor { .. }
            )
        };
        let mut leading = true;
        let mut leading_end = 0;
        let mut trailing_start = 0;
        let mut token_index = 0;
        let mut previous_metrics = None;
        let mut metrics_count = 0u32;
        let mut features = PreparedTokenFeatures {
            flags: self.features.flags
                & (PreparedTokenFeatures::NO_BIDI_GLYPH_SEPARATORS
                    | PreparedTokenFeatures::DEFERRED_HYPHENATION),
            ..PreparedTokenFeatures::default()
        };
        let shared = self.dense.first().map_or(0, |token| {
            (token.header >> InlineToken::WRAP_SHIFT)
                & ((1u64 << (InlineToken::EXTENDED_SHIFT - InlineToken::WRAP_SHIFT)) - 1)
        });
        // Eight bytes per boundary must amortize across at least eight tokens.
        // Highly fragmented plans retain direct indexing instead.
        let range_limit = self.dense.len() / 8;
        let metric_ranges = &mut self.metrics_ranges;
        metric_ranges.clear();
        let runs = &self.runs;
        let summaries = self.summary_blocks.make_mut();
        summaries.clear();
        summaries.extend(
            self.dense
                .chunks(INLINE_SUMMARY_BLOCK_TOKENS)
                .map(|tokens| {
                    InlineSummaryBlock::build(tokens, runs, |token| {
                        features.has_tabs |= token.is_tab();
                        features.flags |= u8::from(matches!(token.kind(), InlineTokenKind::Break { .. }))
                            * PreparedTokenFeatures::HAS_BREAKS;
                        features.has_float_anchors |=
                            matches!(token.kind(), InlineTokenKind::FloatAnchor { .. });
                        // Compare both shared fields, while requiring all
                        // extended flag bits above them to remain zero.
                        features.flags |= u8::from(token.header >> InlineToken::WRAP_SHIFT != shared)
                            * PreparedTokenFeatures::REQUIRES_WIDE;
                        let index = token.run_idx();
                        if previous_metrics != Some(index) {
                            features.preserves_spaces |=
                                runs[index as usize].white_space.preserves_spaces();
                            if let Some(previous) = previous_metrics {
                                if metrics_count as usize <= range_limit {
                                    metric_ranges.push(MetricsRange {
                                        end: token_index as u32,
                                        metrics: previous,
                                    });
                                }
                            }
                            metrics_count += 1;
                            previous_metrics = Some(index);
                        }
                        if token_is_collapsible_line_edge_space(token, runs) {
                            if leading {
                                leading_end = token_index + 1;
                            }
                        } else if !transparent_boundary(token) {
                            leading = false;
                            trailing_start = token_index + 1;
                        }
                        token_index += 1;
                    })
                }),
        );
        self.features = features;
        self.metrics_mode = match previous_metrics {
            Some(index) if metrics_count == 1 => MetricsMode::Uniform(index),
            Some(index) if metrics_count as usize <= range_limit => {
                metric_ranges.push(MetricsRange {
                    end: token_index as u32,
                    metrics: index,
                });
                MetricsMode::Ranges
            }
            _ => {
                metric_ranges.clear();
                MetricsMode::Dense(metrics_count)
            }
        };
        let ranges = [
            0..leading_end,
            trailing_start.max(leading_end)..self.dense.len(),
        ];
        if ranges.iter().all(Range::is_empty) {
            return;
        }
        let dense = self.dense.make_mut();
        for range in ranges {
            let mut changed = false;
            for token in &mut dense[range.clone()] {
                if token.break_kind() == BreakKind::Soft && token.width != 0.0 {
                    token.width = 0.0;
                    changed = true;
                }
            }
            if changed {
                for index in range.start / INLINE_SUMMARY_BLOCK_TOKENS
                    ..range.end.div_ceil(INLINE_SUMMARY_BLOCK_TOKENS)
                {
                    let start = index * INLINE_SUMMARY_BLOCK_TOKENS;
                    let end = (start + INLINE_SUMMARY_BLOCK_TOKENS).min(dense.len());
                    summaries[index] = InlineSummaryBlock::build(&dense[start..end], runs, |_| {});
                }
            }
        }
    }

    /// Stores the local punctuation context on each ordinary space. The token
    /// owns only this compact semantic classification; both Knuth--Plass and
    /// final placement resolve it through the shared glue policy.
    fn classify_space_glue(&mut self, engine: &LayoutEngine<'_, '_>) {
        profile_scope!("tokens.classify_glue");
        fn glyph_character(engine: &LayoutEngine<'_, '_>, token: &InlineToken) -> Option<char> {
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                return None;
            };
            Some(engine.text.character(glyph_idx))
        }

        fn context_character(
            engine: &LayoutEngine<'_, '_>,
            tokens: &[InlineToken],
            mut index: usize,
            direction: isize,
        ) -> Option<char> {
            loop {
                index = index.checked_add_signed(direction)?;
                let token = tokens.get(index)?;
                if let Some(character) = glyph_character(engine, token) {
                    return Some(character);
                }
                match token.kind() {
                    InlineTokenKind::InlineBoundary { .. }
                    | InlineTokenKind::FloatAnchor { .. }
                    | InlineTokenKind::AbsoluteAnchor { .. }
                    | InlineTokenKind::Opportunity => {}
                    _ => return None,
                }
            }
        }

        let dense = self.dense.make_mut();
        for index in 0..dense.len() {
            if !dense[index].is_space() {
                continue;
            }
            // Glue classes preserve token kinds and glyph identities, so
            // later spaces still observe the same neighboring characters.
            let previous = context_character(engine, dense, index, -1);
            let next = context_character(engine, dense, index, 1);
            dense[index].set_space_glue_class(classify_space(previous, next));
        }
    }

    /// Expand only Unicode paragraphs into the already-supported opportunity
    /// tokens. The ordinary Latin line-selection loops remain unchanged.
    fn insert_unicode_break_opportunities(&mut self, engine: &LayoutEngine<'_, '_>) {
        profile_scope!("tokens.unicode_breaks");
        let allow = |token: &InlineToken| match token.kind() {
            InlineTokenKind::Glyph { glyph_idx } =>
                engine.text.whitespace_wrap_before(glyph_idx as usize) == WhitespaceWrapOverride::UnicodeAllow,
            _ => false,
        };
        if !self.dense.iter().any(allow) { return; }
        let dense = self.dense.make_mut();
        let mut expanded: Vec<InlineToken> = Vec::with_capacity(dense.len());
        for mut token in dense.iter().copied() {
            if allow(&token) {
                token.set_break_kind(BreakKind::None);
                if token.is_cluster_boundary() {
                    let mut opportunity = InlineToken::new(
                        InlineTokenKind::Opportunity, 0.0, BreakKind::Discretionary, TokenWrap::Normal, true,
                    );
                    opportunity.set_run_idx(token.run_idx());
                    opportunity.set_bidi_level(token.bidi_level());
                    let mut position = expanded.len();
                    // Opening inline decoration travels with the first
                    // character rather than staying on the preceding line.
                    while let Some(previous) = position.checked_sub(1).and_then(|index| expanded.get(index)) {
                        let InlineTokenKind::InlineBoundary { payload_idx } = previous.kind() else { break };
                        if !matches!(self.replaced[payload_idx as usize], ReplacedToken::InlineBoundary { inline_start: true, inline_end: false, .. }) {
                            break;
                        }
                        position -= 1;
                    }
                    expanded.insert(position, opportunity);
                }
            }
            expanded.push(token);
        }
        *dense = expanded;
    }

    /// `pre-wrap` breaks after a complete sequence of preserved spaces, while
    /// `break-spaces` breaks after every preserved space. A normal soft token
    /// represents a break before discardable whitespace, so use explicit
    /// zero-width opportunities for these after-space rules.
    fn insert_preserved_space_break_opportunities(&mut self, engine: &LayoutEngine<'_, '_>) {
        profile_scope!("tokens.preserved_space_breaks");
        if !self.runs.iter().any(|run| matches!(run.white_space, WhiteSpace::PreWrap | WhiteSpace::BreakSpaces)) {
            return;
        }
        let dense = self.dense.make_mut();
        let mut expanded = Vec::with_capacity(dense.len());
        for (index, token) in dense.iter().copied().enumerate() {
            expanded.push(token);
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                continue;
            };
            let character = engine.text.character(glyph_idx);
            let white_space = token.run_metrics(&self.runs).white_space;
            if !matches!(character, ' ' | '\t')
                && !(white_space == WhiteSpace::BreakSpaces
                    && character == '\u{3000}')
            {
                continue;
            }
            let break_after = match white_space {
                WhiteSpace::BreakSpaces => true,
                WhiteSpace::PreWrap => dense[index + 1..]
                    .iter()
                    .find(|next| {
                        !matches!(
                            next.kind(),
                            InlineTokenKind::InlineBoundary { .. }
                                | InlineTokenKind::FloatAnchor { .. }
                                | InlineTokenKind::AbsoluteAnchor { .. }
                        )
                    })
                    .is_none_or(|next| {
                        let InlineTokenKind::Glyph { glyph_idx } = next.kind() else {
                            return true;
                        };
                        let next_character = engine.text.character(glyph_idx);
                        !matches!(next_character, ' ' | '\t')
                            || !next.run_metrics(&self.runs).white_space.preserves_spaces()
                    }),
                _ => false,
            };
            if break_after {
                let mut opportunity = InlineToken::new(
                    InlineTokenKind::Opportunity,
                    0.0,
                    BreakKind::Discretionary,
                    TokenWrap::Normal,
                    true,
                );
                opportunity.set_run_idx(token.run_idx());
                opportunity.set_bidi_level(token.bidi_level());
                expanded.push(opportunity);
            }
        }
        *dense = expanded;
    }

    pub(super) fn prepare_kp_plan(&mut self, punctuation_aware: bool) {
        profile_scope!("lines.prepare_kp_plan");
        if self.kp_plan.is_none() {
            self.kp_plan = Some(KpPlan::build(self.span(), &self.runs, punctuation_aware));
        }
    }

}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PreparedInlinePlanKey {
    pub(super) run_start: u32,
    pub(super) run_end: u32,
    pub(super) container_box_idx: u32,
    pub(super) high_quality_hyphenation: bool,
    pub(super) book_optimized: bool,
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::layout) enum BreakKind {
    None,
    Soft,
    Hard,
    Discretionary,
}

pub(super) fn token_is_collapsible_line_edge_space(
    token: &InlineToken,
    runs: &[InlineTokenMetrics],
) -> bool {
    token.break_kind() == BreakKind::Soft && !token.run_metrics(runs).white_space.preserves_spaces()
}

/// Mid-word wrapping allowed before this token, from `word-break` /
/// `overflow-wrap`. Unlike a `Soft` break the token is kept, not discarded.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenWrap {
    /// Break only at soft-break opportunities.
    Normal,
    /// `overflow-wrap: break-word`/`anywhere`: break mid-word only when the
    /// word cannot fit on a line by itself (no soft break on the line).
    BreakWord,
    /// `word-break: break-all`: break between any two characters, filling
    /// each line completely.
    Anywhere,
}

mod build;

pub(super) use build::{build_inline_tokens, cached_inline_plan_key};

/// Box supplying the space/ch metric used by number-valued `tab-size`.
/// CSS defines it on the nearest block container rather than the inline
/// span that happens to contain the tab.
fn tab_reference_box(reader: &LayoutReader<'_>, box_idx: usize) -> usize {
    let mut current = Some(box_idx);
    while let Some(idx) = current {
        let style = reader.style(idx);
        if style.display() != html_style_model::Display::Inline {
            return idx;
        }
        current = reader.get_parent(idx);
    }
    box_idx
}

/// Canonical metrics for a preserved tab. Both intrinsic measurement and
/// final line placement use this helper so tab stops cannot diverge between
/// sizing and layout.
pub(in crate::layout) fn preserved_tab_metrics(
    reader: &LayoutReader<'_>,
    box_idx: usize,
) -> (f64, f64) {
    let reference_box = tab_reference_box(reader, box_idx);
    let style = reader.style(box_idx);
    let reference_style = reader.style(reference_box);
    let space_advance = reference_style.font_size() as f64
        * if reader.box_uses_ahem(reference_box) {
            1.0
        } else {
            f64::from(reader.font_metrics(reference_box).space_advance_ratio())
        };
    let ch_advance = reference_style.font_size() as f64
        * if reader.box_uses_ahem(reference_box) {
            1.0
        } else {
            f64::from(reader.font_metrics(reference_box).ch_advance_ratio())
        };
    let interval = match style.tab_size().kind() {
        TabSizeKind::Spaces => {
            style.tab_size().value() as f64
                * (space_advance
                    + reference_style.letter_spacing() as f64
                    + reference_style.word_spacing() as f64)
                    .max(0.0)
        }
        TabSizeKind::LengthPx => style.tab_size().value() as f64,
    };
    (
        interval.min(f32::MAX as f64),
        (ch_advance * 0.5).min(f32::MAX as f64),
    )
}

/// The text facts shared by intrinsic sizing and final line construction.
/// This is computed on demand rather than stored beside each glyph, keeping
/// the dense inline token at its cache-friendly fixed size.
#[derive(Clone, Copy)]
pub(in crate::layout) struct CanonicalTextUnit {
    pub(in crate::layout) character: char,
    pub(in crate::layout) break_kind: BreakKind,
    pub(in crate::layout) natural_advance: f64,
}

pub(in crate::layout) fn canonical_text_unit(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    cursor: &mut TextCursor<'_>,
    glyph_idx: u32,
    box_idx: usize,
    run_end: u32,
    style: html_style_model::UsedStyleView<'_>,
    white_space: WhiteSpace,
) -> (CanonicalTextUnit, bool) {
    let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
    let (fallback_advance, _) = cursor.glyph_dimensions(glyph).expect("registered glyph metric");
    let (advance, cluster_boundary) = cursor.character_geometry(glyph_idx as usize, fallback_advance);
    (
        canonical_text_unit_from_facts(
            engine,
            glyph_idx,
            box_idx,
            run_end,
            engine.text.character(glyph_idx),
            advance,
            style.font_size(),
            style.letter_spacing() as f64,
            style.word_spacing() as f64,
            style.word_spacing_factor() as f64,
            white_space,
        ),
        cluster_boundary,
    )
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn canonical_text_unit_from_facts(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    glyph_idx: u32,
    box_idx: usize,
    run_end: u32,
    character: char,
    glyph_advance: f32,
    font_size: f32,
    letter_spacing: f64,
    word_spacing: f64,
    word_spacing_factor: f64,
    white_space: WhiteSpace,
) -> CanonicalTextUnit {
    let wrap_override = engine.text.whitespace_wrap_before(glyph_idx as usize);
    let break_kind = if is_mandatory_line_break(character) { BreakKind::Hard } else { match wrap_override {
        WhitespaceWrapOverride::Style => break_kind(character, white_space),
        WhitespaceWrapOverride::Allow => BreakKind::Soft,
        WhitespaceWrapOverride::Suppress => BreakKind::None,
        WhitespaceWrapOverride::UnicodeAllow => BreakKind::Discretionary,
        WhitespaceWrapOverride::UnicodeSuppress => BreakKind::None,
    }};
    let natural_advance =
        if suppresses_glyph(character) || character == '\u{200b}' || font_size <= 0.0 {
            0.0
        } else {
            let advance = glyph_advance as f64
                + if glyph_advance > 0.0 && is_word_separator(character) {
                    word_spacing
                } else {
                    0.0
                };
            let advance = if matches!(character, ' ' | '\u{00a0}') {
                advance * word_spacing_factor
            } else {
                advance
            };
            let mut resolved_tracking = 0.0;
            let tracking = (letter_spacing != 0.0 || glyph_idx + 1 >= run_end
                || engine.text.bidi_level(glyph_idx) != engine.text.bidi_level(glyph_idx + 1)) && {
                // Format controls keep source positions and shaping effects, but
                // do not create extra typographic units. Test the boundary before
                // the next visible unit, including after a ZWJ attached to this
                // character; an emoji joining sequence still has no such boundary.
                let mut next = glyph_idx as usize + 1;
                while engine
                    .text
                    .content()
                    .character_at(next)
                    .is_some_and(|character| suppresses_glyph(character) && character != '\u{00ad}')
                {
                    next += 1;
                }
                resolved_tracking = tracking_spacing_after(engine, box_idx, run_end, glyph_idx, next, letter_spacing);
                resolved_tracking != 0.0 && engine.text.is_cluster_boundary(next)
                    && tracking_boundary_allowed(
                        character,
                        engine.text.content().character_at(next).unwrap_or('\0'),
                    )
            };
            advance + if tracking { resolved_tracking } else { 0.0 }
        };
    CanonicalTextUnit {
        character,
        break_kind,
        // Token storage uses f32 widths. Intrinsic sizing must accumulate
        // those same values, otherwise a shrink-to-fit line can overflow its
        // own max-content width solely through per-token rounding.
        natural_advance: f64::from(natural_advance as f32),
    }
}

/// A gap between inline fragments belongs to their common containing
/// element. A bidi level change also fragments every intervening inline,
/// even when a narrow line later brings the pieces next to each other.
fn tracking_spacing_after(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    owner: usize,
    run_end: u32,
    glyph: u32,
    next: usize,
    default: f64,
) -> f64 {
    let fragmented = engine.text.bidi_level(glyph) != engine.text.bidi_level(next as u32);
    if next < run_end as usize && !fragmented { return default; }
    let Some(other) = engine.text.content().text_runs()
        .find(|run| run.glyphs.contains(&(next as u32)))
        .map(|run| run.box_idx as usize) else { return 0.0; };
    let context = |mut node| {
        while engine.reader.is_inline_box(node) {
            let Some(parent) = engine.reader.get_parent(node) else { break; };
            node = parent;
        }
        node
    };
    if context(owner) != context(other) { return 0.0; }
    let next_character = engine.text.character(next as u32);
    if next_character == '\n' || is_mandatory_line_break(next_character) { return 0.0; }
    let mut left = owner;
    let mut right = other;
    let depth = |mut node| {
        let mut depth = 0;
        while let Some(parent) = engine.reader.get_parent(node) {
            depth += 1; node = parent;
        }
        depth
    };
    let left_depth = depth(left);
    let right_depth = depth(right);
    for _ in right_depth..left_depth {
        let Some(parent) = engine.reader.get_parent(left) else { return default; };
        left = parent;
    }
    for _ in left_depth..right_depth {
        let Some(parent) = engine.reader.get_parent(right) else { return default; };
        right = parent;
    }
    while left != right {
        let (Some(a), Some(b)) = (engine.reader.get_parent(left), engine.reader.get_parent(right)) else { return default; };
        left = a; right = b;
    }
    if fragmented {
        while engine.reader.is_inline_box(left) {
            let Some(parent) = engine.reader.get_parent(left) else { break; };
            left = parent;
        }
    }
    f64::from(engine.reader.style(left).letter_spacing())
}

/// Measures a token span as a fully placed line without mutating the document.
///
/// This computes glyph span, line metrics, justification, and per-token
/// placements so the later write phase can emit fragments without
/// reinterpreting the token span.

/// Classifies a glyph as a hard break, soft break, or non-break.
///
/// This replaces the old pair of `breakable` and `hard_break` booleans.
pub(in crate::layout) fn break_kind(character: char, white_space: WhiteSpace) -> BreakKind {
    if is_mandatory_line_break(character) || (character == '\n' && white_space.preserves_newlines()) {
        return BreakKind::Hard;
    }
    if character == '\u{00A0}' || !white_space.allows_wrap() {
        return BreakKind::None;
    }
    if white_space.preserves_spaces() && matches!(character, ' ' | '\t') {
        return BreakKind::None;
    }
    match character {
        ' ' | '\t' | '\n' => BreakKind::Soft,
        '\u{200b}' => BreakKind::Discretionary,
        _ => BreakKind::None,
    }
}
