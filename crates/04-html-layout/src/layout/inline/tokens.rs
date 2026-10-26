//! Token construction, normalization, cached summaries, and token-owned metrics.
//!
//! This module is the source-to-token boundary. It does not choose line breaks
//! or publish fragments.

use super::justification::{SpaceGlueClass, classify_space};
use super::wrapping::KpPlan;
use super::storage::TokenBuffer;
use super::*;
use crate::layout::LayoutEngine;
use crate::layout_model::{LayoutMode, WhitespaceWrapOverride};
use std::mem::{discriminant, size_of};

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
#[cfg(test)]
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
            (token.line_height(runs) - (ascent + descent)) / 2.0
        }
        _ => 0.0,
    };
    vertical_align_metrics_offset(
        metrics.vertical_align.as_style_value(),
        ascent,
        descent,
        token.line_height(runs),
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

/// Returns the authored alignment of this inline-level box. `vertical-align`
/// is not inherited; parent movement is accumulated later from the retained
/// owner chain while constructing the line box.
pub(super) fn effective_vertical_align(doc: &LayoutEngine, box_idx: usize) -> VerticalAlignValue {
    let style = doc.reader.style(box_idx);
    style.vertical_align()
}

/// Text directly owned by a block or table cell lives in an anonymous inline
/// whose `vertical-align` is the initial `baseline`.  In particular, a table
/// cell's own value aligns the cell within its row; it must not also shift the
/// cell's text inside the line box.  A real inline owner, on the other hand,
/// contributes its authored alignment to the complete inline subtree.
pub(super) fn text_vertical_align(doc: &LayoutEngine, box_idx: usize) -> VerticalAlignValue {
    if matches!(
        doc.reader.box_layout_mode(box_idx),
        Some(LayoutMode::Inline(_))
    ) {
        effective_vertical_align(doc, box_idx)
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

// This is a performance invariant: the line-breaking passes walk one of these
// per character, often several times.
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
}

const _: () = assert!(size_of::<InlineTokenMetrics>() <= 32);

impl InlineToken {
    // Header: data[0..24], metrics index[24..42], metadata[42..64].
    pub(super) const MAX_DATA: u32 = (1 << 24) - 1;
    const RUN_SHIFT: u32 = 24;
    const RUN_MASK: u32 = (1 << 18) - 1;
    pub(super) const MAX_RUN_IDX: u32 = Self::RUN_MASK - 1;
    const METADATA_SHIFT: u32 = 42;
    const BIDI_SHIFT: u32 = 14;

    pub(super) const KIND_MASK: u32 = 0b1111;
    const BREAK_SHIFT: u32 = 4;
    const WRAP_SHIFT: u32 = 6;
    const CLUSTER_BOUNDARY_BIT: u32 = 1 << 8;
    const SPACE_GLUE_SHIFT: u32 = 9;
    const SPACE_GLUE_MASK: u32 = 0b111 << Self::SPACE_GLUE_SHIFT;
    const SPACE_BIT: u32 = 1 << 12;
    const TAB_BIT: u32 = 1 << 13;
    // Break payload uses two bits for `clear`; retain BR provenance here so
    // it does not consume metadata bits used by bidi levels.
    const AUTHORED_BREAK_DATA_BIT: u32 = 1 << 2;

    pub(super) fn mark_authored_break(&mut self) {
        debug_assert!(matches!(self.kind(), InlineTokenKind::Break { .. }));
        self.header |= u64::from(Self::AUTHORED_BREAK_DATA_BIT);
    }

    pub(super) fn is_authored_break(&self) -> bool {
        self.metadata() & Self::KIND_MASK == 4 && self.data() & Self::AUTHORED_BREAK_DATA_BIT != 0
    }

    #[inline]
    fn data(&self) -> u32 {
        self.header as u32 & Self::MAX_DATA
    }

    #[inline]
    fn metadata(&self) -> u32 {
        (self.header >> Self::METADATA_SHIFT) as u32
    }

    #[inline]
    fn set_metadata(&mut self, metadata: u32) {
        debug_assert!(metadata < (1 << 22));
        self.header = (self.header & ((1u64 << Self::METADATA_SHIFT) - 1))
            | (u64::from(metadata) << Self::METADATA_SHIFT);
    }

    #[inline]
    pub(super) fn run_idx(&self) -> u32 {
        (self.header >> Self::RUN_SHIFT) as u32 & Self::RUN_MASK
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
        (self.metadata() >> Self::BIDI_SHIFT) as u8
    }

    pub(super) fn set_bidi_level(&mut self, level: u8) {
        self.set_metadata(
            (self.metadata() & ((1 << Self::BIDI_SHIFT) - 1))
                | (u32::from(level) << Self::BIDI_SHIFT),
        );
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
        let (kind_tag, data) = Self::kind_parts(kind);
        assert!(
            data <= Self::MAX_DATA,
            "inline token glyph/payload index exceeds 24-bit capacity"
        );
        let mask = u64::from(Self::MAX_DATA) | (u64::from(Self::KIND_MASK) << Self::METADATA_SHIFT);
        self.header =
            (self.header & !mask) | u64::from(data) | (u64::from(kind_tag) << Self::METADATA_SHIFT);
    }

    #[inline]
    pub(super) fn new(
        kind: InlineTokenKind,
        width: f64,
        break_kind: BreakKind,
        wrap: TokenWrap,
        cluster_boundary_before: bool,
    ) -> Self {
        let (kind_tag, data) = Self::kind_parts(kind);
        assert!(
            data <= Self::MAX_DATA,
            "inline token glyph/payload index exceeds 24-bit capacity"
        );
        let metadata = kind_tag
            | ((break_kind as u32) << Self::BREAK_SHIFT)
            | ((wrap as u32) << Self::WRAP_SHIFT)
            | if cluster_boundary_before {
                Self::CLUSTER_BOUNDARY_BIT
            } else {
                0
            };
        Self {
            header: u64::from(data)
                | (u64::from(Self::RUN_MASK) << Self::RUN_SHIFT)
                | (u64::from(metadata) << Self::METADATA_SHIFT),
            width: width as f32,
        }
    }
    #[inline]
    pub(super) fn kind(&self) -> InlineTokenKind {
        let header = self.header;
        let data = header as u32 & Self::MAX_DATA;
        match (header >> Self::METADATA_SHIFT) as u32 & Self::KIND_MASK {
            0 => InlineTokenKind::Glyph {
                glyph_idx: data,
            },
            1 => InlineTokenKind::Ellipsis { glyph: data },
            2 => InlineTokenKind::Image {
                payload_idx: data,
            },
            3 => InlineTokenKind::AtomicBox {
                payload_idx: data,
            },
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
            6 => InlineTokenKind::InlineBoundary {
                payload_idx: data,
            },
            7 => InlineTokenKind::Discretionary { glyph: data },
            8 => InlineTokenKind::Opportunity,
            9 => InlineTokenKind::AbsoluteAnchor { box_idx: data },
            _ => unreachable!("invalid packed inline-token kind"),
        }
    }

    #[inline]
    pub(super) fn break_kind(&self) -> BreakKind {
        match (self.metadata() >> Self::BREAK_SHIFT) & 0b11 {
            0 => BreakKind::None,
            1 => BreakKind::Soft,
            2 => BreakKind::Hard,
            3 => BreakKind::Discretionary,
            _ => unreachable!("invalid packed inline-token break kind"),
        }
    }

    fn set_break_kind(&mut self, break_kind: BreakKind) {
        self.set_metadata((self.metadata() & !(0b11 << Self::BREAK_SHIFT))
            | ((break_kind as u32) << Self::BREAK_SHIFT));
    }

    #[inline]
    pub(super) fn wrap(&self) -> TokenWrap {
        match (self.metadata() >> Self::WRAP_SHIFT) & 0b11 {
            0 => TokenWrap::Normal,
            1 => TokenWrap::BreakWord,
            2 => TokenWrap::Anywhere,
            _ => unreachable!("invalid packed inline-token wrap kind"),
        }
    }

    #[inline]
    pub(super) fn is_cluster_boundary(&self) -> bool {
        self.metadata() & Self::CLUSTER_BOUNDARY_BIT != 0
    }

    #[inline]
    pub(super) fn is_space(&self) -> bool {
        self.metadata() & Self::SPACE_BIT != 0
    }

    pub(super) fn set_is_space(&mut self) {
        self.set_metadata(self.metadata() | Self::SPACE_BIT);
    }

    pub(super) fn set_is_tab(&mut self) {
        self.set_metadata(self.metadata() | Self::TAB_BIT);
    }

    #[inline]
    pub(super) fn space_glue_class(&self) -> SpaceGlueClass {
        SpaceGlueClass::from_packed(
            (self.metadata() & Self::SPACE_GLUE_MASK) >> Self::SPACE_GLUE_SHIFT,
        )
    }

    pub(super) fn set_space_glue_class(&mut self, class: SpaceGlueClass) {
        self.set_metadata(
            (self.metadata() & !Self::SPACE_GLUE_MASK) | ((class as u32) << Self::SPACE_GLUE_SHIFT));
    }

    #[inline]
    pub(super) fn width(&self) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) {
            0.0
        } else {
            self.width as f64
        }
    }

    pub(super) fn discretionary_width(&self) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) {
            self.width as f64
        } else {
            0.0
        }
    }

    #[inline]
    pub(super) fn run_metrics<'a>(&self, runs: &'a [InlineTokenMetrics]) -> &'a InlineTokenMetrics {
        &runs[self.run_idx() as usize]
    }

    #[inline]
    pub(super) fn line_height(&self, runs: &[InlineTokenMetrics]) -> f64 {
        f64::from(self.run_metrics(runs).line_height)
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
        self.tab_advance_at(reader, runs, position_from_block_start)
    }

    fn tab_advance_at(
        &self,
        reader: &LayoutReader<'_>,
        runs: &[InlineTokenMetrics],
        position_from_block_start: f64,
    ) -> f64 {
        let box_idx = self.run_metrics(runs).owner_box_idx as usize;
        let (interval, minimum_advance) = preserved_tab_metrics(
            reader.style(box_idx),
            tab_reference_style(reader, box_idx),
            reader.box_uses_ahem(box_idx),
        );
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

    #[inline]
    pub(super) fn is_tab(&self) -> bool {
        self.metadata() & Self::TAB_BIT != 0
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

#[derive(Clone, Copy, Default)]
pub(super) struct InlineSummaryBlock {
    // A plain block covers consecutive glyphs, so its end follows from the
    // fixed block size rather than occupying another field and padding.
    pub(super) glyph_start: u32,
    pub(super) preserves_newlines: bool,
    pub(super) plain_text: bool,
    pub(super) width: f64,
    pub(super) max_ascent: f64,
    pub(super) max_descent: f64,
}

impl InlineSummaryBlock {
    fn build(tokens: &[InlineToken], runs: &[InlineTokenMetrics]) -> Self {
        let mut summary = Self {
            glyph_start: u32::MAX,
            max_ascent: f64::NEG_INFINITY,
            max_descent: f64::NEG_INFINITY,
            plain_text: !tokens.is_empty(),
            ..Self::default()
        };
        let mut expected_glyph = None;
        for token in tokens {
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                summary.plain_text = false;
                continue;
            };
            let metrics = token.run_metrics(runs);
            summary.glyph_start = summary.glyph_start.min(glyph_idx);
            summary.plain_text &= expected_glyph.is_none_or(|expected| expected == glyph_idx)
                && !token.is_tab()
                && !metrics.placement_required
                && metrics.visible
                && matches!(metrics.vertical_align, InlineVerticalAlign::Baseline);
            expected_glyph = Some(glyph_idx + 1);
            summary.preserves_newlines |= metrics.white_space.preserves_newlines();
            summary.width += token.width();
            let ascent = metrics.ascent as f64;
            let descent = metrics.descent as f64;
            let half_leading = (f64::from(metrics.line_height) - (ascent + descent)) / 2.0;
            summary.max_ascent = summary.max_ascent.max(ascent + half_leading);
            summary.max_descent = summary.max_descent.max(descent + half_leading);
        }
        if !summary.max_ascent.is_finite() {
            summary.max_ascent = 0.0;
        }
        if !summary.max_descent.is_finite() {
            summary.max_descent = 0.0;
        }
        summary
    }
}

#[derive(Default)]
pub(super) struct InlineTokens {
    pub(super) dense: TokenBuffer<InlineToken>,
    pub(super) runs: TokenBuffer<InlineTokenMetrics>,
    pub(super) summary_blocks: TokenBuffer<InlineSummaryBlock>,
    pub(super) replaced: TokenBuffer<ReplacedToken>,
    pub(super) kp_plan: Option<KpPlan>,
}

impl InlineTokens {
    /// A temporary paragraph borrows the shared token arenas by ownership;
    /// only its dense token range is copied. Restore arenas after emission.
    pub(super) fn take_paragraph(&mut self, range: Range<usize>, breaks: Option<(usize, usize)>) -> Self {
        let mut paragraph = Self {
            dense: range.filter(|index| !breaks.is_some_and(|(a, b)| *index == a || *index == b))
                .map(|index| self.dense[index]).collect(),
            runs: std::mem::take(&mut self.runs),
            replaced: std::mem::take(&mut self.replaced),
            ..Self::default()
        };
        paragraph.collapse_edge_whitespace_through_boundaries();
        paragraph.rebuild_summary_blocks();
        paragraph
    }

    pub(super) fn restore_paragraph(&mut self, paragraph: Self) {
        self.runs = paragraph.runs;
        self.replaced = paragraph.replaced;
    }

    #[cfg(test)]
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            dense: Vec::with_capacity(capacity).into(),
            runs: TokenBuffer::default(),
            summary_blocks: Vec::with_capacity(
                capacity.div_ceil(INLINE_SUMMARY_BLOCK_TOKENS),
            ).into(),
            replaced: TokenBuffer::default(),
            kp_plan: None,
        }
    }

    pub(super) fn bind_metrics_in(
        runs: &mut TokenBuffer<InlineTokenMetrics>,
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
        let token = self.bind_metrics(token, metrics);
        self.dense.push(token);
    }

    fn push_replaced(
        &mut self,
        payload: ReplacedToken,
        kind: impl FnOnce(u32) -> InlineTokenKind,
        mut token: InlineToken,
        metrics: InlineTokenMetrics,
    ) {
        let payload_idx = u32::try_from(self.replaced.len())
            .expect("inline replaced-token arena capacity exhausted");
        self.replaced.push(payload);
        token.set_kind(kind(payload_idx));
        let token = self.bind_metrics(token, metrics);
        self.dense.push(token);
    }

    fn as_slice(&self) -> &[InlineToken] {
        &self.dense
    }

    pub(super) fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    pub(super) fn rebuild_summary_blocks(&mut self) {
        profile_scope!("tokens.summary_blocks");
        let summaries = self.summary_blocks.make_mut();
        summaries.clear();
        summaries.extend(
            self.dense
                .chunks(INLINE_SUMMARY_BLOCK_TOKENS)
                .map(|tokens| InlineSummaryBlock::build(tokens, &self.runs)),
        );
    }

    /// Collapsible indentation remains line-edge whitespace when inline
    /// borders, padding, or margins precede/follow it. Boundary tokens carry
    /// geometry but no text content, so walk through them while zeroing soft
    /// spaces at either paragraph edge.
    fn collapse_edge_whitespace_through_boundaries(&mut self) {
        profile_scope!("tokens.collapse_edges");
        let transparent_boundary = |token: &InlineToken| {
            matches!(
                token.kind(),
                InlineTokenKind::InlineBoundary { .. }
                    | InlineTokenKind::FloatAnchor { .. }
                    | InlineTokenKind::AbsoluteAnchor { .. }
            )
        };
        let mut collapsed = Vec::new();
        for (index, token) in self.dense.iter().enumerate() {
            if token_is_collapsible_line_edge_space(token, &self.runs) {
                collapsed.push(index);
            } else if !transparent_boundary(token) {
                break;
            }
        }
        for (index, token) in self.dense.iter().enumerate().rev() {
            if token_is_collapsible_line_edge_space(token, &self.runs) {
                collapsed.push(index);
            } else if !transparent_boundary(token) {
                break;
            }
        }
        let dense = self.dense.make_mut();
        for index in collapsed {
            dense[index].width = 0.0;
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
            Some(
                engine
                    .text
                    .glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default())
                    .ch(),
            )
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

        let classes = self
            .dense
            .iter()
            .enumerate()
            .filter(|(_, token)| token.is_space())
            .map(|(index, _)| {
                let previous = context_character(engine, &self.dense, index, -1);
                let next = context_character(engine, &self.dense, index, 1);
                (index, classify_space(previous, next))
            })
            .collect::<Vec<_>>();
        let dense = self.dense.make_mut();
        for (index, class) in classes {
            dense[index].set_space_glue_class(class);
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
            let character = engine
                .text
                .glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default())
                .ch();
            if !matches!(character, ' ' | '\t') {
                continue;
            }
            let white_space = token.run_metrics(&self.runs).white_space;
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
                        let next_character = engine
                            .text
                            .glyph_metric(
                                engine.text.glyph_at(glyph_idx as usize).unwrap_or_default(),
                            )
                            .ch();
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
            self.kp_plan = Some(KpPlan::build(self.as_slice(), &self.runs, punctuation_aware));
        }
    }

}

impl std::ops::Deref for InlineTokens {
    type Target = [InlineToken];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
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

pub(super) use build::build_inline_tokens;
pub(in crate::layout) use build::{inline_item_ownership_box, run_belongs_to_inline_context};

/// Style supplying the space/ch metric used by number-valued `tab-size`.
/// CSS defines it on the nearest block container rather than the inline
/// span that happens to contain the tab.
pub(in crate::layout) fn tab_reference_style<'input>(
    reader: &LayoutReader<'input>,
    box_idx: usize,
) -> html_style_model::UsedStyleView<'input> {
    let mut current = Some(box_idx);
    while let Some(idx) = current {
        let style = reader.style(idx);
        if style.display() != html_style_model::Display::Inline {
            return style;
        }
        current = reader.get_parent(idx);
    }
    reader.style(box_idx)
}

/// Canonical metrics for a preserved tab. Both intrinsic measurement and
/// final line placement use this helper so tab stops cannot diverge between
/// sizing and layout.
pub(in crate::layout) fn preserved_tab_metrics(
    style: html_style_model::UsedStyleView<'_>,
    reference_style: html_style_model::UsedStyleView<'_>,
    uses_ahem: bool,
) -> (f64, f64) {
    let ch_advance = reference_style.font_size() as f64 * if uses_ahem { 1.0 } else { 0.5 };
    let interval = match style.tab_size().kind() {
        TabSizeKind::Spaces => {
            style.tab_size().value() as f64
                * (ch_advance
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
    glyph_idx: u32,
    style: html_style_model::UsedStyleView<'_>,
    white_space: WhiteSpace,
) -> CanonicalTextUnit {
    let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
    let metric = engine.text.glyph_metric(glyph);
    canonical_text_unit_from_facts(
        engine,
        glyph_idx,
        metric.ch(),
        engine.text.text_advance(glyph_idx as usize, metric.advance()),
        style.font_size(),
        style.letter_spacing() as f64,
        style.word_spacing() as f64,
        style.word_spacing_factor() as f64,
        white_space,
    )
}

#[inline]
#[allow(clippy::too_many_arguments)]
pub(super) fn canonical_text_unit_from_facts(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    glyph_idx: u32,
    character: char,
    glyph_advance: f32,
    font_size: f32,
    letter_spacing: f64,
    word_spacing: f64,
    word_spacing_factor: f64,
    white_space: WhiteSpace,
) -> CanonicalTextUnit {
    let wrap_override = engine.text.whitespace_wrap_before(glyph_idx as usize);
    let break_kind = match wrap_override {
        WhitespaceWrapOverride::Style => break_kind(character, white_space),
        WhitespaceWrapOverride::Allow => BreakKind::Soft,
        WhitespaceWrapOverride::Suppress => BreakKind::None,
        WhitespaceWrapOverride::UnicodeAllow => BreakKind::Discretionary,
        WhitespaceWrapOverride::UnicodeSuppress => BreakKind::None,
    };
    let natural_advance = if matches!(character, '\u{00ad}' | '\u{200b}') || crate::shaping::is_bidi_control(character) || font_size <= 0.0 {
        0.0
    } else {
        let advance = glyph_advance as f64 + if character == ' ' { word_spacing } else { 0.0 };
        let advance = if matches!(character, ' ' | '\u{00a0}') { advance * word_spacing_factor } else { advance };
        advance + letter_spacing
    };
    CanonicalTextUnit {
        character,
        break_kind,
        natural_advance,
    }
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
    if character == '\n' && white_space.preserves_newlines() {
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
