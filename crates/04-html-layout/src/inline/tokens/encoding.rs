//! Packed token headers and transient construction values.
use super::{BreakKind, InlineTokenMetrics, preserved_tab_metrics};
use crate::LayoutReader;
use crate::inline::boundaries;
use crate::inline::justification::SpaceGlueClass;
use html_engine_model::GlyphId;
use html_style_model::Clear;

#[derive(Clone, Copy, PartialEq)]
// Four-byte alignment keeps the cached width beside the packed header without
// rounding the stride back up to 16 bytes. Read the header by value so Rust can
// generate an unaligned load where the target requires one.
#[repr(C, packed(4))]
pub(in crate::inline) struct InlineToken {
    pub(in crate::inline) header: u64,
    pub(in crate::inline) width: f32,
}

// A transient construction/editing value; retained storage uses parallel u64/f32 columns.
const _: () = assert!(size_of::<InlineToken>() <= 12);

// Codes 16..215 encode flagged kinds; the next code holds a source reference.
const SOURCE_ITEM_CODE: u32 = 216;

impl InlineToken {
    #[inline]
    pub(in crate::inline) fn metadata(&self) -> TokenMetadata {
        TokenMetadata {
            header: self.header,
        }
    }

    // The low word stores data 24, kind/glue 4,
    // break 2, shaping boundary 1, bidi continuation 1. Tags 10..14 are the
    // five space classes; tag 15 is a preserved tab.
    pub(in crate::inline) const MAX_DATA: u32 = (1 << 24) - 1;
    pub(in crate::inline) const RUN_SHIFT: u32 = 32;
    pub(in crate::inline) const RUN_MASK: u32 = (1 << 18) - 1;
    pub(in crate::inline) const MAX_RUN_IDX: u32 = Self::RUN_MASK - 1;
    pub(in crate::inline) const KIND_SHIFT: u32 = 24;
    const BREAK_SHIFT: u32 = 28;
    const CLUSTER_BOUNDARY_BIT: u64 = 1 << 30;
    // Bidi also joins Unicode Marks exposed as separate shaping clusters.
    // Wrapping retains the authoritative shaping boundary independently.
    const BIDI_CONTINUATION_BIT: u64 = 1 << 31;
    pub(in crate::inline) const BOUNDARY_SHIFT: u32 = 50;
    pub(in crate::inline) const BOUNDARY_MASK: u64 = 7u64 << Self::BOUNDARY_SHIFT;
    // Headers retain arbitrary combinations of kind, space, tab and
    // glue flags. The extended code uses the four high bits as well as the
    // low kind nibble; normal tokens keep these high bits zero.
    pub(in crate::inline) const EXTENDED_SHIFT: u32 = 60;
    const AUTHORED_BREAK_DATA_BIT: u32 = 1 << 2;

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
            SOURCE_ITEM_CODE => (SOURCE_ITEM_CODE, false, false, SpaceGlueClass::Ordinary),
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
    pub(in crate::inline) fn mark_authored_break(&mut self) {
        debug_assert!(matches!(self.kind(), InlineTokenKind::Break { .. }));
        self.header |= u64::from(Self::AUTHORED_BREAK_DATA_BIT);
    }

    #[inline]
    pub(in crate::inline) fn run_idx(&self) -> u32 {
        self.metadata().run_idx()
    }
    #[inline]
    pub(in crate::inline) fn set_run_idx(&mut self, run_idx: u32) {
        assert!(
            run_idx <= Self::MAX_RUN_IDX,
            "inline token metrics index exceeds 18-bit capacity"
        );
        self.header = (self.header & !(u64::from(Self::RUN_MASK) << Self::RUN_SHIFT))
            | (u64::from(run_idx) << Self::RUN_SHIFT);
    }

    #[inline]
    fn kind_parts(kind: InlineTokenKind) -> (u32, u32) {
        match kind {
            InlineTokenKind::Glyph { glyph_idx } => (0, glyph_idx),
            InlineTokenKind::Ellipsis { glyph } => (1, glyph.0),
            InlineTokenKind::Image { payload_idx } => (2, payload_idx),
            InlineTokenKind::AtomicBox { payload_idx } => (3, payload_idx),
            InlineTokenKind::Break { clear } => (4, clear as u32),
            InlineTokenKind::FloatAnchor { box_idx } => (5, box_idx),
            InlineTokenKind::InlineBoundary { payload_idx } => (6, payload_idx),
            InlineTokenKind::Discretionary { glyph } => (7, glyph.0),
            InlineTokenKind::ZeroWidthBreak => (8, 0),
            InlineTokenKind::AbsoluteAnchor { box_idx } => (9, box_idx),
            InlineTokenKind::SourceItem { item_idx } => (SOURCE_ITEM_CODE, item_idx),
        }
    }

    pub(in crate::inline) fn set_kind(&mut self, kind: InlineTokenKind) {
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
    pub(in crate::inline) fn new(
        kind: InlineTokenKind,
        width: f64,
        break_kind: BreakKind,
        allows_emergency_wrap: bool,
        cluster_boundary_before: bool,
    ) -> Self {
        let (tag, data) = Self::kind_parts(kind);
        assert!(
            data <= Self::MAX_DATA,
            "inline token glyph/payload index exceeds 24-bit capacity"
        );
        Self {
            header: u64::from(data)
                | (u64::from(tag & 15) << Self::KIND_SHIFT)
                | (u64::from(tag >> 4) << Self::EXTENDED_SHIFT)
                | (u64::from(Self::RUN_MASK) << Self::RUN_SHIFT)
                | ((break_kind as u64) << Self::BREAK_SHIFT)
                | ((allows_emergency_wrap as u64) << Self::BOUNDARY_SHIFT)
                | if cluster_boundary_before {
                    Self::CLUSTER_BOUNDARY_BIT
                } else {
                    Self::BIDI_CONTINUATION_BIT
                },
            width: width as f32,
        }
    }
    #[inline]
    pub(in crate::inline) fn kind(&self) -> InlineTokenKind {
        self.metadata().kind()
    }

    #[inline]
    pub(in crate::inline) fn break_kind(&self) -> BreakKind {
        self.metadata().break_kind()
    }
    pub(in crate::inline) fn set_break_kind(&mut self, kind: BreakKind) {
        self.header =
            (self.header & !(3u64 << Self::BREAK_SHIFT)) | ((kind as u64) << Self::BREAK_SHIFT);
    }
    #[inline]
    pub(in crate::inline) fn boundary_flags(&self) -> u8 {
        self.metadata().boundary_flags()
    }

    pub(in crate::inline) fn set_boundary_flags(&mut self, flags: u8) {
        self.header =
            (self.header & !Self::BOUNDARY_MASK) | (u64::from(flags) << Self::BOUNDARY_SHIFT);
    }

    pub(in crate::inline) fn mark_break_before(&mut self) {
        self.set_boundary_flags(self.boundary_flags() | boundaries::NORMAL_BEFORE);
    }

    pub(in crate::inline) fn mark_break_after(&mut self) {
        self.set_boundary_flags(self.boundary_flags() | boundaries::NORMAL_AFTER);
    }

    #[inline]
    pub(in crate::inline) fn is_cluster_boundary(&self) -> bool {
        self.metadata().is_cluster_boundary()
    }
    #[inline]
    pub(in crate::inline) fn is_space(&self) -> bool {
        self.metadata().is_space()
    }
    pub(in crate::inline) fn set_is_space(&mut self) {
        let (kind, _, tab, glue) = self.flags();
        self.set_flags(kind, true, tab, glue);
    }
    pub(in crate::inline) fn set_is_tab(&mut self) {
        let (kind, space, _, glue) = self.flags();
        self.set_flags(kind, space, true, glue);
    }

    pub(in crate::inline) fn set_bidi_continuation(&mut self) {
        self.header |= Self::BIDI_CONTINUATION_BIT;
    }

    pub(in crate::inline) fn set_space_glue_class(&mut self, class: SpaceGlueClass) {
        let (kind, space, tab, _) = self.flags();
        self.set_flags(kind, space, tab, class);
    }

    #[inline]
    pub(in crate::inline) fn width(&self) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) {
            0.0
        } else {
            self.width as f64
        }
    }

    #[inline]
    pub(in crate::inline) fn run_metrics<'a>(
        &self,
        runs: &'a [InlineTokenMetrics],
    ) -> &'a InlineTokenMetrics {
        self.metadata().run_metrics(runs)
    }

    #[inline]
    pub(in crate::inline) fn advance_at(
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
    pub(in crate::inline) fn is_tab(&self) -> bool {
        self.metadata().is_tab()
    }
}

/// Read-only token information. Bidi levels remain in source text geometry;
/// widths are accessed independently.
#[derive(Clone, Copy)]
pub(in crate::inline) struct TokenMetadata {
    header: u64,
}
impl TokenMetadata {
    #[inline]
    pub(in crate::inline) fn from_header(header: u64) -> Self {
        Self { header }
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
            SOURCE_ITEM_CODE => SOURCE_ITEM_CODE,
            _ => (code - 16) % 10,
        }
    }
    #[inline]
    fn data(&self) -> u32 {
        self.header as u32 & InlineToken::MAX_DATA
    }
    #[inline]
    pub(in crate::inline) fn run_idx(&self) -> u32 {
        (self.header >> InlineToken::RUN_SHIFT) as u32 & InlineToken::RUN_MASK
    }
    #[inline]
    pub(in crate::inline) fn kind(&self) -> InlineTokenKind {
        let data = self.data();
        match self.kind_tag() {
            0 => InlineTokenKind::Glyph { glyph_idx: data },
            1 => InlineTokenKind::Ellipsis {
                glyph: GlyphId(data),
            },
            2 => InlineTokenKind::Image { payload_idx: data },
            3 => InlineTokenKind::AtomicBox { payload_idx: data },
            4 => InlineTokenKind::Break {
                clear: match data & 3 {
                    0 => Clear::None,
                    1 => Clear::Left,
                    2 => Clear::Right,
                    3 => Clear::Both,
                    _ => unreachable!("invalid packed clear value"),
                },
            },
            5 => InlineTokenKind::FloatAnchor { box_idx: data },
            6 => InlineTokenKind::InlineBoundary { payload_idx: data },
            7 => InlineTokenKind::Discretionary {
                glyph: GlyphId(data),
            },
            8 => InlineTokenKind::ZeroWidthBreak,
            9 => InlineTokenKind::AbsoluteAnchor { box_idx: data },
            SOURCE_ITEM_CODE => InlineTokenKind::SourceItem { item_idx: data },
            _ => unreachable!("invalid packed inline-token kind"),
        }
    }
    #[inline]
    pub(in crate::inline) fn break_kind(&self) -> BreakKind {
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
    pub(in crate::inline) fn is_unbroken_glyph(&self) -> bool {
        let mask = (15u64 << InlineToken::EXTENDED_SHIFT) | (63u64 << InlineToken::KIND_SHIFT);
        self.header & mask == 0
    }
    #[inline]
    pub(in crate::inline) fn boundary_flags(&self) -> u8 {
        ((self.header & InlineToken::BOUNDARY_MASK) >> InlineToken::BOUNDARY_SHIFT) as u8
    }
    #[inline]
    pub(in crate::inline) fn is_cluster_boundary(&self) -> bool {
        self.header & InlineToken::CLUSTER_BOUNDARY_BIT != 0
    }
    #[inline]
    pub(in crate::inline) fn continues_bidi_cluster(&self) -> bool {
        self.header & InlineToken::BIDI_CONTINUATION_BIT != 0
    }
    #[inline]
    pub(in crate::inline) fn is_space(&self) -> bool {
        let code = self.code();
        if code < 16 {
            code.wrapping_sub(10) < 5
        } else {
            ((code - 16) / 10) & 1 != 0
        }
    }
    #[inline]
    pub(in crate::inline) fn space_glue_class(&self) -> SpaceGlueClass {
        let code = self.code();
        if code == SOURCE_ITEM_CODE {
            return SpaceGlueClass::Ordinary;
        }
        if code < 16 {
            SpaceGlueClass::from_packed(code.wrapping_sub(10))
        } else {
            SpaceGlueClass::from_packed(((code - 16) / 10) >> 2)
        }
    }
    #[inline]
    pub(in crate::inline) fn is_tab(&self) -> bool {
        if self.code() < 16 {
            self.code() == 15
        } else {
            ((self.code() - 16) / 10) & 2 != 0
        }
    }
    #[inline]
    pub(in crate::inline) fn is_authored_break(&self) -> bool {
        self.kind_tag() == 4 && self.data() & InlineToken::AUTHORED_BREAK_DATA_BIT != 0
    }
    #[inline]
    pub(in crate::inline) fn run_metrics<'a>(
        &self,
        runs: &'a [InlineTokenMetrics],
    ) -> &'a InlineTokenMetrics {
        &runs[self.run_idx() as usize]
    }
    #[inline]
    pub(in crate::inline) fn width(self, stored_width: f32) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) {
            0.0
        } else {
            f64::from(stored_width)
        }
    }
    #[inline]
    pub(in crate::inline) fn discretionary_width(self, stored_width: f32) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) {
            f64::from(stored_width)
        } else {
            0.0
        }
    }
    #[inline]
    pub(in crate::inline) fn advance_with_metrics(
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
pub(in crate::inline) enum InlineTokenKind {
    /// Width-independent source reference, resolved before line composition.
    SourceItem {
        item_idx: u32,
    },
    Glyph {
        glyph_idx: u32,
    },
    Ellipsis {
        glyph: GlyphId,
    },
    Image {
        payload_idx: u32,
    },
    AtomicBox {
        payload_idx: u32,
    },
    Break {
        clear: Clear,
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
        glyph: GlyphId,
    },
    /// Authored U+200B control. Generated opportunities live on boundaries.
    ZeroWidthBreak,
}
