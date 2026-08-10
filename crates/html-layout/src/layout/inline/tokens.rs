//! Token construction, normalization, cached summaries, and token-owned metrics.
//!
//! This module is the source-to-token boundary. It does not choose line breaks
//! or publish fragments.

use super::justification::{SpaceGlueClass, classify_space};
use super::wrapping::KpPlan;
use super::*;
use crate::layout::LayoutEngine;
use crate::layout_model::LayoutMode;
use unicode_segmentation::UnicodeSegmentation;

/// Normal justification consumes at most the glue declared by the paragraph:
/// 50% space expansion or 33% contraction. A separate emergency pass may
/// choose an underfull line, but rendering still observes these limits.
pub(super) const KP_TOLERANCE: f64 = 1.0;
pub(super) const HYPHEN_PENALTY: f64 = 50.0;

/// Sharing tiny plans costs more allocation and hashing than rebuilding them.
/// Retain only text contexts large enough to amortize their shared buffers.
pub(super) const INLINE_TOKEN_CACHE_MIN_GLYPHS: usize = 64;
pub(super) const INLINE_SUMMARY_BLOCK_TOKENS: usize = 16;

/// Derives a final line box from ascent, descent, and a minimum height.
///
/// Callers use this both for raw font metrics and for the adjusted metrics after
/// vertical-align offsets have been applied.
pub(super) fn compute_baseline(max_ascent: f64, max_descent: f64, min_height: f64) -> (f64, f64) {
    let content_height = max_ascent + max_descent;
    let line_height = content_height.max(min_height);
    let has_extents = max_ascent != 0.0 || max_descent != 0.0;
    let baseline = if has_extents { max_ascent + (line_height - content_height).max(0.0) / 2.0 } else { line_height * 0.8 };
    (line_height, baseline)
}

/// Converts a computed vertical-alignment value into the upward offset used by
/// the fragment writer.
pub(super) fn vertical_align_offset(token: &InlineToken, runs: &[InlineTokenMetrics], line_height: f64, baseline: f64) -> f64 {
    let metrics = token.run_metrics(runs);
    let ascent = metrics.ascent as f64;
    let descent = metrics.descent as f64;
    // Half-leading surrounds text glyphs inside their authored line-height.
    // Replaced and atomic inline boxes contribute their margin-box geometry
    // directly; treating the font line-height as their own leading can make a
    // tall box produce negative half-leading and inflate `vertical-align: top`
    // or `bottom` line boxes.
    let half_leading = match token.kind() {
        InlineTokenKind::Glyph { .. } | InlineTokenKind::Ellipsis { .. } | InlineTokenKind::Discretionary { .. } => (token.line_height(runs) - (ascent + descent)) / 2.0,
        _ => 0.0,
    };
    vertical_align_metrics_offset(metrics.vertical_align, ascent, descent, token.line_height(runs), metrics.font_size as f64, half_leading, line_height, baseline, ascent, descent)
}

pub(super) fn vertical_align_metrics_offset(
    value: VerticalAlignValue, ascent: f64, descent: f64, own_line_height: f64, font_size: f64, half_leading: f64, line_height: f64, baseline: f64, parent_font_ascent: f64, parent_font_descent: f64,
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
        VerticalAlignValue::Calc { absolute_px, line_height_fraction, .. } => absolute_px as f64 + own_line_height * line_height_fraction as f64,
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
pub(in crate::layout) fn indent_for_line(text_indent: f64, hanging: bool, is_first_line: bool) -> f64 {
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
fn text_vertical_align(doc: &LayoutEngine, box_idx: usize) -> VerticalAlignValue {
    if matches!(doc.reader.box_layout_mode(box_idx), Some(LayoutMode::Inline(_))) { effective_vertical_align(doc, box_idx) } else { VerticalAlignValue::Baseline }
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
        if !ancestor_style.vertical_align().is_initial() || ancestor_style.position() == html_style_model::PositionMode::Relative {
            return true;
        }
        current = doc.reader.get_parent(ancestor_idx);
    }
    false
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct InlineToken {
    pub(super) data: u32,
    pub(super) metadata: u32,
    pub(super) run_idx: u32,
    pub(super) width: f32,
}

// This is a performance invariant: the line-breaking passes walk one of these
// per character, often several times.
const _: () = assert!(std::mem::size_of::<InlineToken>() <= 16);

#[derive(Clone, Copy, PartialEq)]
pub(super) struct InlineTokenMetrics {
    /// Prepared layout box that owns this run. Keeping ownership once per
    /// interned metrics run lets fragment publication recover inline ancestry
    /// without expanding the 16-byte dense token representation.
    pub(super) owner_box_idx: u32,
    pub(super) ascent: f32,
    pub(super) descent: f32,
    pub(super) line_height: f64,
    pub(super) tab_interval: f64,
    pub(super) tab_min_advance: f64,
    pub(super) font_size: f32,
    pub(super) vertical_align: VerticalAlignValue,
    pub(super) white_space: WhiteSpace,
    pub(super) placement_required: bool,
}

impl InlineToken {
    pub(super) const KIND_MASK: u32 = 0b1111;
    const BREAK_SHIFT: u32 = 4;
    const WRAP_SHIFT: u32 = 6;
    const CLUSTER_BOUNDARY_BIT: u32 = 1 << 8;
    const SPACE_GLUE_SHIFT: u32 = 9;
    const SPACE_GLUE_MASK: u32 = 0b111 << Self::SPACE_GLUE_SHIFT;

    pub(super) fn new(kind: InlineTokenKind, width: f64, break_kind: BreakKind, wrap: TokenWrap, cluster_boundary_before: bool) -> Self {
        let (kind_tag, data) = match kind {
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
        };
        let metadata = kind_tag | ((break_kind as u32) << Self::BREAK_SHIFT) | ((wrap as u32) << Self::WRAP_SHIFT) | if cluster_boundary_before { Self::CLUSTER_BOUNDARY_BIT } else { 0 };
        Self { data, metadata, run_idx: u32::MAX, width: width as f32 }
    }

    #[inline]
    pub(super) fn kind(&self) -> InlineTokenKind {
        match self.metadata & Self::KIND_MASK {
            0 => InlineTokenKind::Glyph { glyph_idx: self.data },
            1 => InlineTokenKind::Ellipsis { glyph: self.data },
            2 => InlineTokenKind::Image { payload_idx: self.data },
            3 => InlineTokenKind::AtomicBox { payload_idx: self.data },
            4 => InlineTokenKind::Break {
                clear: match self.data {
                    0 => html_style_model::Clear::None,
                    1 => html_style_model::Clear::Left,
                    2 => html_style_model::Clear::Right,
                    3 => html_style_model::Clear::Both,
                    _ => unreachable!("invalid packed clear value"),
                },
            },
            5 => InlineTokenKind::FloatAnchor { box_idx: self.data },
            6 => InlineTokenKind::InlineBoundary { payload_idx: self.data },
            7 => InlineTokenKind::Discretionary { glyph: self.data },
            8 => InlineTokenKind::Opportunity,
            9 => InlineTokenKind::AbsoluteAnchor { box_idx: self.data },
            _ => unreachable!("invalid packed inline-token kind"),
        }
    }

    #[inline]
    pub(super) fn break_kind(&self) -> BreakKind {
        match (self.metadata >> Self::BREAK_SHIFT) & 0b11 {
            0 => BreakKind::None,
            1 => BreakKind::Soft,
            2 => BreakKind::Hard,
            3 => BreakKind::Discretionary,
            _ => unreachable!("invalid packed inline-token break kind"),
        }
    }

    fn set_break_kind(&mut self, break_kind: BreakKind) {
        self.metadata = (self.metadata & !(0b11 << Self::BREAK_SHIFT)) | ((break_kind as u32) << Self::BREAK_SHIFT);
    }

    #[inline]
    pub(super) fn wrap(&self) -> TokenWrap {
        match (self.metadata >> Self::WRAP_SHIFT) & 0b11 {
            0 => TokenWrap::Normal,
            1 => TokenWrap::BreakWord,
            2 => TokenWrap::Anywhere,
            _ => unreachable!("invalid packed inline-token wrap kind"),
        }
    }

    #[inline]
    pub(super) fn is_cluster_boundary(&self) -> bool {
        self.metadata & Self::CLUSTER_BOUNDARY_BIT != 0
    }

    #[inline]
    pub(super) fn space_glue_class(&self) -> SpaceGlueClass {
        SpaceGlueClass::from_packed((self.metadata & Self::SPACE_GLUE_MASK) >> Self::SPACE_GLUE_SHIFT)
    }

    pub(super) fn set_space_glue_class(&mut self, class: SpaceGlueClass) {
        self.metadata = (self.metadata & !Self::SPACE_GLUE_MASK) | ((class as u32) << Self::SPACE_GLUE_SHIFT);
    }

    #[inline]
    pub(super) fn width(&self) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) { 0.0 } else { self.width as f64 }
    }

    pub(super) fn discretionary_width(&self) -> f64 {
        if matches!(self.kind(), InlineTokenKind::Discretionary { .. }) { self.width as f64 } else { 0.0 }
    }

    #[inline]
    pub(super) fn run_metrics<'a>(&self, runs: &'a [InlineTokenMetrics]) -> &'a InlineTokenMetrics {
        &runs[self.run_idx as usize]
    }

    #[inline]
    pub(super) fn line_height(&self, runs: &[InlineTokenMetrics]) -> f64 {
        self.run_metrics(runs).line_height
    }

    #[inline]
    pub(super) fn advance_at(&self, runs: &[InlineTokenMetrics], position_from_block_start: f64) -> f64 {
        let metrics = self.run_metrics(runs);
        if metrics.tab_interval < 0.0 {
            return self.width();
        }
        let interval = metrics.tab_interval;
        if interval <= 0.0 {
            return 0.0;
        }
        let remainder = position_from_block_start.rem_euclid(interval);
        let mut advance = if remainder <= f64::EPSILON { interval } else { interval - remainder };
        if advance < metrics.tab_min_advance {
            advance += interval;
        }
        advance
    }

    #[inline]
    pub(super) fn is_tab(&self, runs: &[InlineTokenMetrics]) -> bool {
        self.run_metrics(runs).tab_interval >= 0.0
    }
}

#[derive(Clone, Copy)]
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
#[derive(Clone)]
pub(super) enum ReplacedToken {
    Image { image_idx: u32, box_idx: u32, content_size: Size, border_size: Size, content_inset: Point, margin_left: f64, margin_top: f64, position_offset: Vec2, set_box_geometry: bool },
    AtomicBox { box_idx: u32, border_size: Size, containing_width: f64, containing_height: Option<f64>, margin_left: f64, margin_top: f64 },
    InlineBoundary { box_idx: u32, margin_left: f64, left_inset: f64, inline_start: bool, inline_end: bool },
}

#[derive(Clone, Copy, Default)]
pub(super) struct InlineSummaryBlock {
    pub(super) glyph_start: u32,
    pub(super) glyph_end: u32,
    pub(super) width: f64,
    pub(super) max_ascent: f64,
    pub(super) max_descent: f64,
    pub(super) preserves_newlines: bool,
    pub(super) plain_text: bool,
}

impl InlineSummaryBlock {
    fn build(tokens: &[InlineToken], runs: &[InlineTokenMetrics]) -> Self {
        let mut summary = Self { glyph_start: u32::MAX, max_ascent: f64::NEG_INFINITY, max_descent: f64::NEG_INFINITY, plain_text: !tokens.is_empty(), ..Self::default() };
        let mut expected_glyph = None;
        for token in tokens {
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else {
                summary.plain_text = false;
                continue;
            };
            let metrics = token.run_metrics(runs);
            summary.glyph_start = summary.glyph_start.min(glyph_idx);
            summary.glyph_end = summary.glyph_end.max(glyph_idx + 1);
            summary.plain_text &= expected_glyph.is_none_or(|expected| expected == glyph_idx) && metrics.tab_interval < 0.0 && !metrics.placement_required && matches!(metrics.vertical_align, VerticalAlignValue::Baseline);
            expected_glyph = Some(glyph_idx + 1);
            summary.preserves_newlines |= metrics.white_space.preserves_newlines();
            summary.width += token.width();
            let ascent = metrics.ascent as f64;
            let descent = metrics.descent as f64;
            let half_leading = (metrics.line_height - (ascent + descent)) / 2.0;
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

#[derive(Clone)]
pub(super) enum InlineStorage<T: Clone> {
    Owned(Vec<T>),
    Shared(std::sync::Arc<Vec<T>>),
}

impl<T: Clone> Default for InlineStorage<T> {
    fn default() -> Self {
        Self::Owned(Vec::new())
    }
}

impl<T: Clone> InlineStorage<T> {
    fn with_capacity(capacity: usize) -> Self {
        Self::Owned(Vec::with_capacity(capacity))
    }

    fn make_mut(&mut self) -> &mut Vec<T> {
        match self {
            Self::Owned(values) => values,
            Self::Shared(values) => std::sync::Arc::make_mut(values),
        }
    }

    fn share(&mut self) {
        if let Self::Owned(values) = self {
            *self = Self::Shared(std::sync::Arc::new(std::mem::take(values)));
        }
    }

    fn capacity(&self) -> usize {
        match self {
            Self::Owned(values) => values.capacity(),
            Self::Shared(values) => values.capacity(),
        }
    }
}

impl<T: Clone> std::ops::Deref for InlineStorage<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(values) => values,
            Self::Shared(values) => values,
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct InlineTokens {
    pub(super) dense: InlineStorage<InlineToken>,
    pub(super) runs: InlineStorage<InlineTokenMetrics>,
    pub(super) summary_blocks: InlineStorage<InlineSummaryBlock>,
    pub(super) replaced: Vec<ReplacedToken>,
    pub(super) kp_plan: Option<std::sync::Arc<[std::sync::OnceLock<KpPlan>; 2]>>,
}

impl InlineTokens {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self { dense: InlineStorage::with_capacity(capacity), runs: InlineStorage::default(), summary_blocks: InlineStorage::with_capacity(capacity.div_ceil(INLINE_SUMMARY_BLOCK_TOKENS)), replaced: Vec::new(), kp_plan: None }
    }

    pub(super) fn bind_metrics_in(runs: &mut Vec<InlineTokenMetrics>, previous_run_idx: Option<u32>, token: &mut InlineToken, metrics: InlineTokenMetrics) -> u32 {
        if let Some(run_idx) = previous_run_idx
            && runs.get(run_idx as usize).is_some_and(|previous| *previous == metrics)
        {
            token.run_idx = run_idx;
            return run_idx;
        }

        let run_idx = runs.iter().position(|candidate| *candidate == metrics).unwrap_or_else(|| {
            runs.push(metrics);
            runs.len() - 1
        });
        let run_idx = u32::try_from(run_idx).expect("inline run-metrics arena capacity exhausted");
        token.run_idx = run_idx;
        run_idx
    }

    pub(super) fn bind_metrics(&mut self, mut token: InlineToken, metrics: InlineTokenMetrics) -> InlineToken {
        let previous_run_idx = self.dense.last().map(|previous| previous.run_idx);
        Self::bind_metrics_in(self.runs.make_mut(), previous_run_idx, &mut token, metrics);
        token
    }

    pub(super) fn push(&mut self, token: InlineToken, metrics: InlineTokenMetrics) {
        let token = self.bind_metrics(token, metrics);
        self.dense.make_mut().push(token);
    }

    fn push_replaced(&mut self, payload: ReplacedToken, kind: impl FnOnce(u32) -> InlineTokenKind, mut token: InlineToken, metrics: InlineTokenMetrics) {
        let payload_idx = u32::try_from(self.replaced.len()).expect("inline replaced-token arena capacity exhausted");
        self.replaced.push(payload);
        let packed_kind = kind(payload_idx);
        token.data = match packed_kind {
            InlineTokenKind::Image { payload_idx } | InlineTokenKind::AtomicBox { payload_idx } | InlineTokenKind::InlineBoundary { payload_idx } => payload_idx,
            _ => unreachable!("replaced token kind must use a payload index"),
        };
        token.metadata = (token.metadata & !InlineToken::KIND_MASK)
            | match packed_kind {
                InlineTokenKind::Image { .. } => 2,
                InlineTokenKind::AtomicBox { .. } => 3,
                InlineTokenKind::InlineBoundary { .. } => 6,
                _ => unreachable!(),
            };
        let token = self.bind_metrics(token, metrics);
        self.dense.make_mut().push(token);
    }

    fn as_slice(&self) -> &[InlineToken] {
        &self.dense
    }

    pub(super) fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    fn share(&mut self) {
        self.dense.share();
        self.runs.share();
        self.summary_blocks.share();
    }

    fn rebuild_summary_blocks(&mut self) {
        let summaries = self.summary_blocks.make_mut();
        summaries.clear();
        summaries.extend(self.dense.chunks(INLINE_SUMMARY_BLOCK_TOKENS).map(|tokens| InlineSummaryBlock::build(tokens, &self.runs)));
    }

    /// Collapsible indentation remains line-edge whitespace when inline
    /// borders, padding, or margins precede/follow it. Boundary tokens carry
    /// geometry but no text content, so walk through them while zeroing soft
    /// spaces at either paragraph edge.
    fn collapse_edge_whitespace_through_boundaries(&mut self) {
        let transparent_boundary = |token: &InlineToken| matches!(token.kind(), InlineTokenKind::InlineBoundary { .. } | InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. });
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
        fn glyph_character(engine: &LayoutEngine<'_, '_>, token: &InlineToken) -> Option<char> {
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else { return None };
            Some(engine.text.glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default()).ch())
        }

        fn context_character(engine: &LayoutEngine<'_, '_>, tokens: &[InlineToken], mut index: usize, direction: isize) -> Option<char> {
            loop {
                index = index.checked_add_signed(direction)?;
                let token = tokens.get(index)?;
                if let Some(character) = glyph_character(engine, token) {
                    return Some(character);
                }
                match token.kind() {
                    InlineTokenKind::InlineBoundary { .. } | InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. } | InlineTokenKind::Opportunity => {}
                    _ => return None,
                }
            }
        }

        let classes = self
            .dense
            .iter()
            .enumerate()
            .filter_map(|(index, token)| {
                (glyph_character(engine, token) == Some(' ')).then(|| {
                    let previous = context_character(engine, &self.dense, index, -1);
                    let next = context_character(engine, &self.dense, index, 1);
                    (index, classify_space(previous, next))
                })
            })
            .collect::<Vec<_>>();
        let dense = self.dense.make_mut();
        for (index, class) in classes {
            dense[index].set_space_glue_class(class);
        }
    }

    /// `pre-wrap` breaks after a complete sequence of preserved spaces, while
    /// `break-spaces` breaks after every preserved space. A normal soft token
    /// represents a break before discardable whitespace, so use explicit
    /// zero-width opportunities for these after-space rules.
    fn insert_preserved_space_break_opportunities(&mut self, engine: &LayoutEngine<'_, '_>) {
        let InlineStorage::Owned(dense) = &mut self.dense else { unreachable!("new inline token plans must own their construction buffer") };
        let mut expanded = Vec::with_capacity(dense.len());
        for (index, token) in dense.iter().copied().enumerate() {
            expanded.push(token);
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else { continue };
            let character = engine.text.glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default()).ch();
            if !matches!(character, ' ' | '\t') {
                continue;
            }
            let white_space = token.run_metrics(&self.runs).white_space;
            let break_after = match white_space {
                WhiteSpace::BreakSpaces => true,
                WhiteSpace::PreWrap => {
                    dense[index + 1..].iter().find(|next| !matches!(next.kind(), InlineTokenKind::InlineBoundary { .. } | InlineTokenKind::FloatAnchor { .. } | InlineTokenKind::AbsoluteAnchor { .. })).is_none_or(|next| {
                        let InlineTokenKind::Glyph { glyph_idx } = next.kind() else { return true };
                        let next_character = engine.text.glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default()).ch();
                        !matches!(next_character, ' ' | '\t') || !next.run_metrics(&self.runs).white_space.preserves_spaces()
                    })
                }
                _ => false,
            };
            if break_after {
                let mut opportunity = InlineToken::new(InlineTokenKind::Opportunity, 0.0, BreakKind::Discretionary, TokenWrap::Normal, true);
                opportunity.run_idx = token.run_idx;
                expanded.push(opportunity);
            }
        }
        *dense = expanded;
    }

    /// Inserts zero-source-length logical breaks at Unicode sentence
    /// boundaries. Existing collapsible whitespace becomes the break token
    /// where possible; otherwise a synthetic zero-width break is inserted.
    /// Glyph indices and source ranges remain untouched, so selection and
    /// annotation anchoring still refer to the original document.
    fn insert_sentence_breaks(&mut self, engine: &LayoutEngine<'_, '_>) {
        let mut text = String::new();
        let mut glyph_tokens = Vec::new();
        for (token_idx, token) in self.dense.iter().enumerate() {
            let InlineTokenKind::Glyph { glyph_idx } = token.kind() else { continue };
            if token.run_metrics(&self.runs).white_space.preserves_spaces() {
                return;
            }
            let character = engine.text.glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default()).ch();
            let byte_start = text.len();
            text.push(character);
            glyph_tokens.push((byte_start, text.len(), token_idx, character));
        }
        if glyph_tokens.len() < 2 {
            return;
        }

        let mut break_positions = Vec::new();
        let segments = text.split_sentence_bound_indices().collect::<Vec<_>>();
        for (segment_index, (segment_start, segment)) in segments.iter().copied().enumerate() {
            if segment_index + 1 == segments.len() {
                break;
            }
            let trimmed = segment.trim_end_matches(char::is_whitespace);
            if trimmed.is_empty() {
                continue;
            }
            let content_end = segment_start + trimmed.len();
            let Some((_, _, content_token, _)) = glyph_tokens.iter().copied().rev().find(|(_, end, _, _)| *end <= content_end) else { continue };
            let whitespace_token = glyph_tokens.iter().copied().find(|(start, _, _, character)| *start >= content_end && character.is_whitespace()).map(|(_, _, token_idx, _)| token_idx);
            break_positions.push((content_token + 1, whitespace_token));
        }
        break_positions.sort_unstable();
        break_positions.dedup();

        let dense = self.dense.make_mut();
        for (insert_after, whitespace_token) in break_positions.into_iter().rev() {
            if let Some(token_idx) = whitespace_token {
                dense[token_idx].set_break_kind(BreakKind::Hard);
            } else {
                let Some(previous) = dense.get(insert_after.saturating_sub(1)).copied() else { continue };
                let mut break_token = InlineToken::new(InlineTokenKind::Break { clear: html_style_model::Clear::None }, 0.0, BreakKind::Hard, TokenWrap::Normal, true);
                break_token.run_idx = previous.run_idx;
                dense.insert(insert_after, break_token);
            }
        }
        self.kp_plan = None;
    }

    fn enable_kp_plan_cache(&mut self) {
        self.kp_plan.get_or_insert_with(|| std::sync::Arc::new([std::sync::OnceLock::new(), std::sync::OnceLock::new()]));
    }

    pub(super) fn kp_plan(&self, punctuation_aware: bool) -> Option<&KpPlan> {
        self.kp_plan.as_deref().map(|plans| plans[usize::from(punctuation_aware)].get_or_init(|| KpPlan::build(self.as_slice(), &self.runs, punctuation_aware)))
    }
}

impl std::ops::Deref for InlineTokens {
    type Target = [InlineToken];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct InlineTokenCacheKey {
    pub(super) run_start: u32,
    pub(super) run_end: u32,
    pub(super) container_box_idx: u32,
}

/// Width-independent token plans retained by a laid-out document. Only pure
/// text/marker formatting contexts enter this cache; replaced content and
/// other layout-sensitive runs continue through the ordinary builder.
#[derive(Clone, Default)]
pub(crate) struct InlineTokenCache {
    text_plans: rustc_data_structures::fx::FxHashMap<InlineTokenCacheKey, InlineTokens>,
}

impl InlineTokenCache {
    fn get(&self, key: InlineTokenCacheKey) -> Option<InlineTokens> {
        self.text_plans.get(&key).cloned()
    }

    fn insert(&mut self, key: InlineTokenCacheKey, tokens: InlineTokens) {
        self.text_plans.insert(key, tokens);
    }

    pub(crate) fn memory_usage_bytes(&self) -> usize {
        self.text_plans
            .values()
            .map(|tokens| {
                let kp_plan_bytes = tokens.kp_plan.as_deref().map_or(0, |plans| plans.iter().filter_map(std::sync::OnceLock::get).map(KpPlan::memory_usage_bytes).sum());
                tokens.dense.capacity() * std::mem::size_of::<InlineToken>() + tokens.runs.capacity() * std::mem::size_of::<InlineTokenMetrics>() + tokens.summary_blocks.capacity() * std::mem::size_of::<InlineSummaryBlock>() + kp_plan_bytes
            })
            .sum()
    }

    pub(crate) fn len(&self) -> usize {
        self.text_plans.len()
    }
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::layout) enum BreakKind {
    None,
    Soft,
    Hard,
    Discretionary,
}

pub(super) fn token_is_collapsible_line_edge_space(token: &InlineToken, runs: &[InlineTokenMetrics]) -> bool {
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

fn create_image_token(
    engine: &crate::layout::LayoutEngine<'_, '_>, image_idx: u32, box_idx: u32, container_box_idx: usize, max_width: f64, containing_block_height: Option<f64>, standalone_image: Option<u32>,
) -> (InlineToken, InlineTokenMetrics, ReplacedToken) {
    let style = engine.reader.style(box_idx as usize);
    let self_owned_replaced = box_idx as usize == container_box_idx;
    let vertical_align = if self_owned_replaced { VerticalAlignValue::Top } else { effective_vertical_align(engine, box_idx as usize) };
    let smart_standalone = standalone_image == Some(box_idx) && style.position() != html_style_model::PositionMode::Absolute;
    let max_width = max_width.max(0.0);
    let authored_margin_left = style.margin_left().resolve(max_width);
    let authored_margin_right = style.margin_right().resolve(max_width);
    let margin_top = style.margin_top().resolve(max_width);
    let margin_bottom = style.margin_bottom().resolve(max_width);
    let padding_left = style.padding_left().resolve(max_width);
    let padding_top = style.padding_top().resolve(max_width);
    let horizontal_padding = style.get_horizontal_padding(max_width);
    let vertical_padding = style.get_vertical_padding(max_width);
    let border_left = style.border_left_width() as f64;
    let border_top = style.border_top_width() as f64;
    let horizontal_border = border_left + style.border_right_width() as f64;
    let vertical_border = border_top + style.border_bottom_width() as f64;
    let intrinsic = engine.replaced.intrinsic_size(&engine.reader, box_idx as usize).unwrap_or_else(|| {
        let (width, height) = engine.replaced.image_display_size(image_idx);
        Size::new(width, height)
    });
    if self_owned_replaced {
        let content_height = containing_block_height.unwrap_or_else(|| if intrinsic.width > 0.0 { max_width * intrinsic.height / intrinsic.width } else { intrinsic.height });
        let content_size = Size::new(max_width, content_height.max(0.0));
        let font_size = style.font_size();
        let line_height = resolved_line_height(style);
        return (
            InlineToken::new(InlineTokenKind::Image { payload_idx: u32::MAX }, content_size.width, BreakKind::None, TokenWrap::Normal, true),
            InlineTokenMetrics {
                owner_box_idx: box_idx,
                ascent: content_size.height as f32,
                descent: 0.0,
                line_height,
                tab_interval: -1.0,
                tab_min_advance: 0.0,
                font_size,
                vertical_align,
                white_space: style.white_space(),
                placement_required: false,
            },
            ReplacedToken::Image { image_idx, box_idx, content_size, border_size: content_size, content_inset: Point::ZERO, margin_left: 0.0, margin_top: 0.0, position_offset: Vec2::ZERO, set_box_geometry: false },
        );
    }
    let authored_ratio = style.aspect_ratio();
    let intrinsic_ratio = engine.replaced.intrinsic_ratio(&engine.reader, box_idx as usize, intrinsic);
    let aspect_ratio = if authored_ratio.uses_intrinsic() { intrinsic_ratio.or_else(|| authored_ratio.preferred().map(f64::from)) } else { authored_ratio.preferred().map(f64::from) };
    let smart_width = engine.replaced.smart_standalone_image_width(
        &engine.reader,
        engine.config.image_sizing_policy(),
        box_idx as usize,
        intrinsic,
        max_width,
        engine.config.viewport_height(),
        horizontal_padding + horizontal_border,
        margin_top + margin_bottom + vertical_padding + vertical_border,
        smart_standalone,
    );
    let (margin_left, margin_right) = if smart_width.is_some() { (0.0, 0.0) } else { (authored_margin_left, authored_margin_right) };
    let preferred_width = smart_width.unwrap_or_else(|| style.width());
    let content_size = resolve_replaced_content_size(ReplacedSizeInput {
        intrinsic,
        aspect_ratio,
        width: preferred_width,
        height: style.height(),
        min_width: if smart_width.is_some() { PreferredSize::Auto } else { style.min_width() },
        min_height: style.min_height(),
        max_width: if smart_width.is_some() { PreferredSize::Auto } else { style.max_width() },
        max_height: style.max_height(),
        available_width: max_width,
        available_height: containing_block_height,
        horizontal_margin: margin_left + margin_right,
        vertical_margin: margin_top + margin_bottom,
        horizontal_padding_border: horizontal_padding + horizontal_border,
        vertical_padding_border: vertical_padding + vertical_border,
        box_sizing: style.box_sizing(),
    });
    let border_size = Size::new(content_size.width + horizontal_padding + horizontal_border, content_size.height + vertical_padding + vertical_border);
    let outer_width = (border_size.width + margin_left + margin_right).max(0.0);
    let outer_height = (border_size.height + margin_top + margin_bottom).max(0.0);
    let font_size = style.font_size();
    let line_height = resolved_line_height(style);
    let position_offset = if style.position() == html_style_model::PositionMode::Relative { crate::layout::box_positioning::relative_position_offset(&style, max_width, containing_block_height) } else { Vec2::ZERO };
    (
        InlineToken::new(InlineTokenKind::Image { payload_idx: u32::MAX }, outer_width, BreakKind::None, TokenWrap::Normal, true),
        InlineTokenMetrics { owner_box_idx: box_idx, ascent: outer_height as f32, descent: 0.0, line_height, tab_interval: -1.0, tab_min_advance: 0.0, font_size, vertical_align, white_space: style.white_space(), placement_required: false },
        ReplacedToken::Image { image_idx, box_idx, content_size, border_size, content_inset: Point::new(padding_left + border_left, padding_top + border_top), margin_left, margin_top, position_offset, set_box_geometry: true },
    )
}

fn create_break_token(box_idx: u32, clear: html_style_model::Clear, style: html_style_model::UsedStyleView, vertical_align: VerticalAlignValue) -> (InlineToken, InlineTokenMetrics) {
    let line_height = resolved_line_height(style);
    (
        InlineToken::new(InlineTokenKind::Break { clear }, 0.0, BreakKind::Hard, TokenWrap::Normal, true),
        InlineTokenMetrics {
            owner_box_idx: box_idx,
            ascent: 0.0,
            descent: 0.0,
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: style.font_size(),
            vertical_align,
            white_space: style.white_space(),
            placement_required: false,
        },
    )
}

fn create_float_anchor_token(box_idx: u32, style: html_style_model::UsedStyleView, vertical_align: VerticalAlignValue) -> (InlineToken, InlineTokenMetrics) {
    let line_height = resolved_line_height(style);
    (
        InlineToken::new(InlineTokenKind::FloatAnchor { box_idx }, 0.0, BreakKind::None, TokenWrap::Normal, true),
        InlineTokenMetrics {
            owner_box_idx: box_idx,
            ascent: 0.0,
            descent: 0.0,
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: style.font_size(),
            vertical_align,
            white_space: style.white_space(),
            placement_required: false,
        },
    )
}

fn create_absolute_anchor_token(box_idx: u32, style: html_style_model::UsedStyleView, vertical_align: VerticalAlignValue) -> (InlineToken, InlineTokenMetrics) {
    let line_height = resolved_line_height(style);
    (
        InlineToken::new(InlineTokenKind::AbsoluteAnchor { box_idx }, 0.0, BreakKind::None, TokenWrap::Normal, true),
        InlineTokenMetrics {
            owner_box_idx: box_idx,
            ascent: 0.0,
            descent: 0.0,
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: style.font_size(),
            vertical_align,
            white_space: style.white_space(),
            placement_required: false,
        },
    )
}

fn create_inline_boundary_token(
    engine: &crate::layout::LayoutEngine<'_, '_>, box_idx: u32, inline_start: bool, inline_end: bool, style: html_style_model::UsedStyleView, vertical_align: VerticalAlignValue, max_width: f64,
) -> (InlineToken, InlineTokenMetrics, ReplacedToken) {
    let margin_left = if inline_start { style.margin_left().resolve(max_width) } else { 0.0 };
    let margin_right = if inline_end { style.margin_right().resolve(max_width) } else { 0.0 };
    let left = if inline_start { style.padding_left().resolve(max_width) + style.border_left_width() as f64 } else { 0.0 };
    let right = if inline_end { style.padding_right().resolve(max_width) + style.border_right_width() as f64 } else { 0.0 };
    let line_height = resolved_line_height(style);
    let font_size = style.font_size() as f64;
    // A boundary is the zero-content strut of its inline box. Its baseline
    // therefore uses the selected font's ascent/descent plus half-leading,
    // exactly like text owned by that box. Basing edge boundaries on a fixed
    // 80/20 split made empty inlines disagree with otherwise identical text.
    let (font_ascent, font_descent) = engine.reader.font_metrics(box_idx as usize).line_box_ratios().map_or((font_size * 0.8, font_size * 0.2), |(ascent, descent)| (font_size * ascent as f64, font_size * descent as f64));
    let half_leading = (line_height - font_ascent - font_descent) * 0.5;
    let (ascent, descent) = (font_ascent + half_leading, font_descent + half_leading);
    let token = InlineToken::new(InlineTokenKind::InlineBoundary { payload_idx: u32::MAX }, margin_left + left + right + margin_right, BreakKind::None, TokenWrap::Normal, true);
    let metrics = InlineTokenMetrics {
        owner_box_idx: box_idx,
        ascent: ascent as f32,
        descent: descent as f32,
        line_height,
        tab_interval: -1.0,
        tab_min_advance: 0.0,
        font_size: font_size as f32,
        vertical_align,
        white_space: style.white_space(),
        placement_required: false,
    };
    (token, metrics, ReplacedToken::InlineBoundary { box_idx, margin_left, left_inset: left, inline_start, inline_end })
}

/// Converts inline items into layout tokens.
///
/// Each token carries the geometry and break metadata needed by the line
/// breaker plus the placement inputs needed later by line measurement.
pub(super) fn build_inline_tokens(engine: &mut crate::layout::LayoutEngine<'_, '_>, run_range: Range<u32>, container_box_idx: usize, max_width: f64, containing_block_height: Option<f64>) -> InlineTokens {
    let timing_started = Instant::now();
    let cache_key = InlineTokenCacheKey { run_start: run_range.start, run_end: run_range.end, container_box_idx: u32::try_from(container_box_idx).expect("inline token-cache box index exhausted") };
    let source_runs = &engine.text.inline_items()[run_range.start as usize..run_range.end as usize];
    let cacheable = !engine.config.sentence_per_line()
        && source_runs.iter().all(|run| matches!(run.kind, InlineItemKind::Text { .. } | InlineItemKind::Marker { .. }))
        && source_runs
            .iter()
            .map(|run| match &run.kind {
                InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => glyphs.len(),
                _ => 0,
            })
            .sum::<usize>()
            >= INLINE_TOKEN_CACHE_MIN_GLYPHS;
    if cacheable && let Some(tokens) = engine.inline_token_cache.get(cache_key) {
        engine.record_timing(|t| t.build_inline_tokens += timing_started.elapsed());
        return tokens;
    }

    // Atomic descendants allocate their own run ranges in the shared arena.
    // Clone this small range so token measurement can mutably invoke their
    // independent layout algorithms without aliasing the arena borrow.
    let runs = engine.text.inline_items()[run_range.start as usize..run_range.end as usize].to_vec();
    let standalone_image = standalone_inline_image(engine, &runs, container_box_idx);
    let mut tokens = build_inline_tokens_from_runs(engine, &runs, container_box_idx, max_width, containing_block_height, standalone_image);
    if engine.config.sentence_per_line() {
        tokens.insert_sentence_breaks(engine);
        tokens.rebuild_summary_blocks();
    }
    if cacheable {
        if engine.config.force_justify() || engine.reader.style(container_box_idx).text_align() == TextAlign::Justify || (engine.config.book_optimized_text() && engine.reader.style(container_box_idx).text_align() == TextAlign::Left) {
            tokens.enable_kp_plan_cache();
        }
        tokens.share();
        engine.inline_token_cache.insert(cache_key, tokens.clone());
    }
    // Nested timing is measured in build_inline_tokens_from_runs; this captures the outer wrapper.
    engine.record_timing(|t| t.build_inline_tokens += timing_started.elapsed());
    tokens
}

pub(super) fn build_inline_tokens_from_runs(
    engine: &mut crate::layout::LayoutEngine<'_, '_>, runs: &[InlineItem], container_box_idx: usize, max_width: f64, containing_block_height: Option<f64>, standalone_image: Option<u32>,
) -> InlineTokens {
    let timing_started = Instant::now();
    let glyph_capacity = runs
        .iter()
        .map(|run| match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => glyphs.end.saturating_sub(glyphs.start) as usize,
            _ => 1,
        })
        .sum();
    let mut tokens = InlineTokens::with_capacity(glyph_capacity);
    for run in runs {
        let ownership_box = inline_item_ownership_box(engine, run);
        if !ownership_box.is_some_and(|box_idx| run_belongs_to_inline_context(engine, box_idx, container_box_idx)) {
            continue;
        }
        let style = engine.reader.style(run.box_idx as usize);
        let white_space = style.white_space();
        let vertical_align = effective_vertical_align(engine, run.box_idx as usize);
        match &run.kind {
            InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs } => {
                let vertical_align = text_vertical_align(engine, run.box_idx as usize);
                append_text_tokens(engine, &mut tokens, glyphs.clone(), run.box_idx as usize, style, white_space, vertical_align);
            }
            InlineItemKind::Image { image_idx } => {
                // A blockified replaced element is represented by a self-owned
                // image run so it can reuse the image fragment pipeline. It is
                // content of its own border box, not a baseline-aligned child.
                // Blockified images reuse an inline item only as an internal
                // fragment carrier. The image is content of its own border
                // box, so baseline alignment must never shift or clip it.
                // The absolute box's hypothetical static position is a
                // separate positioning decision made by the parent flow.
                let (token, metrics, payload) = create_image_token(engine, *image_idx, run.box_idx, container_box_idx, max_width, containing_block_height, standalone_image);
                if white_space.allows_wrap() && run.box_idx as usize != container_box_idx {
                    tokens.push(InlineToken::new(InlineTokenKind::Opportunity, 0.0, BreakKind::Discretionary, TokenWrap::Normal, true), metrics);
                }
                tokens.push_replaced(payload, |payload_idx| InlineTokenKind::Image { payload_idx }, token, metrics);
                if white_space.allows_wrap() && run.box_idx as usize != container_box_idx {
                    tokens.push(InlineToken::new(InlineTokenKind::Opportunity, 0.0, BreakKind::Discretionary, TokenWrap::Normal, true), metrics);
                }
            }
            InlineItemKind::AtomicBox { box_idx } => {
                let (token, metrics, payload) = create_atomic_box_token(engine, *box_idx, style, vertical_align, max_width, containing_block_height);
                if white_space.allows_wrap() {
                    tokens.push(InlineToken::new(InlineTokenKind::Opportunity, 0.0, BreakKind::Discretionary, TokenWrap::Normal, true), metrics);
                }
                tokens.push_replaced(payload, |payload_idx| InlineTokenKind::AtomicBox { payload_idx }, token, metrics);
                if white_space.allows_wrap() {
                    tokens.push(InlineToken::new(InlineTokenKind::Opportunity, 0.0, BreakKind::Discretionary, TokenWrap::Normal, true), metrics);
                }
            }
            InlineItemKind::Break { clear } => {
                let (token, metrics) = create_break_token(run.box_idx, *clear, style, vertical_align);
                tokens.push(token, metrics);
            }
            InlineItemKind::FloatAnchor { box_idx } => {
                let (token, metrics) = create_float_anchor_token(*box_idx, style, vertical_align);
                tokens.push(token, metrics);
            }
            InlineItemKind::AbsoluteAnchor { box_idx } => {
                let (token, metrics) = create_absolute_anchor_token(*box_idx, style, vertical_align);
                tokens.push(token, metrics);
            }
            InlineItemKind::InlineBoundary { inline_start, inline_end } => {
                let (token, metrics, payload) = create_inline_boundary_token(engine, run.box_idx, *inline_start, *inline_end, style, vertical_align, max_width);
                tokens.push_replaced(payload, |payload_idx| InlineTokenKind::InlineBoundary { payload_idx }, token, metrics);
            }
        }
    }
    tokens.insert_preserved_space_break_opportunities(engine);
    tokens.collapse_edge_whitespace_through_boundaries();
    tokens.classify_space_glue(engine);
    tokens.rebuild_summary_blocks();
    engine.record_timing(|t| t.build_inline_tokens_from_runs += timing_started.elapsed());
    tokens
}

/// Identifies an image that is the sole meaningful inline content of its
/// formatting context. Whitespace and transparent inline wrappers are
/// harmless; text, another image, a break, a float, or an atomic inline
/// box makes the context ineligible.
pub(super) fn standalone_inline_image(engine: &crate::layout::LayoutEngine<'_, '_>, runs: &[InlineItem], container_box_idx: usize) -> Option<u32> {
    let mut candidate = None;
    for run in runs {
        if !inline_item_ownership_box(engine, run).is_some_and(|box_idx| run_belongs_to_inline_context(engine, box_idx, container_box_idx)) {
            continue;
        }
        match &run.kind {
            InlineItemKind::Image { .. } if candidate.is_none() => candidate = Some(run.box_idx),
            InlineItemKind::Text { glyphs } => {
                let has_text = glyphs.clone().any(|glyph_idx| {
                    let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
                    !engine.text.glyph_metric(glyph).ch().is_whitespace()
                });
                if has_text {
                    return None;
                }
            }
            InlineItemKind::Image { .. }
            | InlineItemKind::Marker { .. }
            | InlineItemKind::Break { .. }
            | InlineItemKind::FloatAnchor { .. }
            | InlineItemKind::AbsoluteAnchor { .. }
            | InlineItemKind::AtomicBox { .. }
            | InlineItemKind::InlineBoundary { .. } => {
                return None;
            }
        }
    }
    candidate
}

pub(in crate::layout) fn inline_item_ownership_box(engine: &crate::layout::LayoutEngine<'_, '_>, item: &InlineItem) -> Option<usize> {
    match item.kind {
        // The atomic box is a formatting-context root. Its parent owns the
        // placeholder in the surrounding inline formatting context.
        InlineItemKind::AtomicBox { box_idx } => engine.reader.get_parent(box_idx as usize),
        _ => Some(item.box_idx as usize),
    }
}

/// Shared inline items belonging to a nested formatting context must not be
/// flattened into an ancestor line. Stop at the requested container; any
/// block/table/flex/grid root encountered before it owns the run instead.
pub(in crate::layout) fn run_belongs_to_inline_context(engine: &crate::layout::LayoutEngine<'_, '_>, run_box_idx: usize, container_box_idx: usize) -> bool {
    let mut current = Some(run_box_idx);
    while let Some(box_idx) = current {
        if box_idx == container_box_idx {
            return true;
        }
        if matches!(
            engine.reader.box_layout_mode(box_idx),
            Some(
                crate::layout_model::LayoutMode::Block(_)
                    | crate::layout_model::LayoutMode::Table(_)
                    | crate::layout_model::LayoutMode::TableRow(_)
                    | crate::layout_model::LayoutMode::TableCell(_)
                    | crate::layout_model::LayoutMode::Flex(_)
                    | crate::layout_model::LayoutMode::Grid(_)
            )
        ) {
            return false;
        }
        current = engine.reader.get_parent(box_idx);
    }
    false
}

fn create_atomic_box_token(
    engine: &mut crate::layout::LayoutEngine<'_, '_>, box_idx: u32, style: html_style_model::UsedStyleView, vertical_align: VerticalAlignValue, max_width: f64, containing_block_height: Option<f64>,
) -> (InlineToken, InlineTokenMetrics, ReplacedToken) {
    let max_width = max_width.max(0.0);
    let margin_left = style.margin_left().resolve(max_width);
    let margin_right = style.margin_right().resolve(max_width);
    let margin_top = style.margin_top().resolve(max_width);
    let margin_bottom = style.margin_bottom().resolve(max_width);
    let resolved_padding = style.get_horizontal_padding(max_width);
    let horizontal_border = style.border_left_width() as f64 + style.border_right_width() as f64;
    let vertical_padding = style.get_vertical_padding(max_width);
    let vertical_border = style.border_top_width() as f64 + style.border_bottom_width() as f64;
    // Anonymous inline tables carry an anonymous reset style whose computed
    // `display` is not `inline-table`; participation in this atomic inline
    // context is represented authoritatively by the layout mode instead.
    let is_inline_table = matches!(engine.reader.box_layout_mode(box_idx as usize), Some(crate::layout_model::LayoutMode::Table(_)));
    let (border_size, first_baseline, last_baseline) = if let Some(intrinsic) = engine.replaced.intrinsic_size(&engine.reader, box_idx as usize) {
        let authored_ratio = style.aspect_ratio();
        let intrinsic_ratio = (intrinsic.height > 0.0).then_some(intrinsic.width / intrinsic.height);
        let aspect_ratio = if authored_ratio.uses_intrinsic() { intrinsic_ratio.or_else(|| authored_ratio.preferred().map(f64::from)) } else { authored_ratio.preferred().map(f64::from) };
        let content = resolve_replaced_content_size(ReplacedSizeInput {
            intrinsic,
            aspect_ratio,
            width: style.width(),
            height: style.height(),
            min_width: style.min_width(),
            min_height: style.min_height(),
            max_width: style.max_width(),
            max_height: style.max_height(),
            available_width: max_width,
            available_height: containing_block_height,
            horizontal_margin: margin_left + margin_right,
            vertical_margin: margin_top + margin_bottom,
            horizontal_padding_border: resolved_padding + horizontal_border,
            vertical_padding_border: vertical_padding + vertical_border,
            box_sizing: style.box_sizing(),
        });
        (Size::new(content.width + resolved_padding + horizontal_border, content.height + vertical_padding + vertical_border), None, None)
    } else if is_inline_table && matches!(style.border_collapse(), html_style_model::BorderCollapseMode::Collapse) {
        // The collapsed edge grid, not the authored full borders, determines
        // the inline table's used border box. Let normal table layout resolve
        // that grid instead of duplicating box sizing here.
        crate::layout::measure_box_and_baselines_isolated(engine, box_idx as usize, max_width, containing_block_height)
    } else {
        // Atomic inline sizing needs the raw content contributions here. The
        // outer intrinsic helper already applies the box's preferred width;
        // using it for `fit-content` collapses min-content to max-content and
        // prevents the available inline size from clamping between them.
        let (intrinsic_min, intrinsic_max) = crate::layout::box_content_intrinsic_widths(engine, box_idx as usize);
        let min_border_width = (intrinsic_min + resolved_padding + horizontal_border).max(0.0);
        let max_border_width = (intrinsic_max + resolved_padding + horizontal_border).max(min_border_width);
        let border_inset = resolved_padding + horizontal_border;
        let available_border_width = (max_width - margin_left - margin_right).max(border_inset);
        let shrink_to_fit = max_border_width.min(available_border_width.max(min_border_width));
        let resolve_border_width = |size: PreferredSize, auto: f64| match size {
            PreferredSize::Auto => auto,
            PreferredSize::MinContent => min_border_width,
            PreferredSize::MaxContent => max_border_width,
            PreferredSize::FitContent => shrink_to_fit,
            PreferredSize::Stretch => available_border_width,
            _ => {
                let resolved = resolve_used_preferred_size(size, auto, max_width).max(0.0);
                match style.box_sizing() {
                    BoxSizing::ContentBox => resolved + border_inset,
                    BoxSizing::BorderBox => resolved.max(border_inset),
                }
            }
        };
        let ratio_transferred_width = if matches!(style.width(), PreferredSize::Auto) {
            (|| {
                let authored_ratio = style.aspect_ratio();
                let ratio = authored_ratio.preferred().map(f64::from)?;
                let vertical_border_inset = vertical_padding + vertical_border;
                let vertical_box_sizing_inset = matches!(style.box_sizing(), BoxSizing::BorderBox).then_some(vertical_border_inset).unwrap_or(0.0);
                let content_height = crate::layout::resolve_vertical_size(style.height(), containing_block_height, vertical_box_sizing_inset)?;
                Some(match style.box_sizing() {
                    BoxSizing::ContentBox => content_height * ratio + border_inset,
                    BoxSizing::BorderBox => ((content_height + vertical_border_inset) * ratio).max(border_inset),
                })
            })()
        } else {
            None
        };
        let preferred = ratio_transferred_width.unwrap_or_else(|| resolve_border_width(style.width(), shrink_to_fit));
        let automatic_minimum = if ratio_transferred_width.is_some()
            && !matches!(style.overflow_x(), html_style_model::OverflowMode::Hidden | html_style_model::OverflowMode::Scroll | html_style_model::OverflowMode::Auto)
        {
            min_border_width
        } else {
            border_inset
        };
        let minimum = resolve_border_width(style.min_width(), automatic_minimum);
        let maximum = resolve_border_width(style.max_width(), f64::INFINITY).max(minimum);
        let assigned_width = preferred.clamp(minimum, maximum);
        crate::layout::measure_box_width_and_baselines_isolated(engine, box_idx as usize, max_width, assigned_width, containing_block_height)
    };
    let outer_width = (border_size.width + margin_left + margin_right).max(0.0);
    let outer_height = (border_size.height + margin_top + margin_bottom).max(0.0);
    let is_inline_block = matches!(style.display(), html_style_model::Display::InlineBlock);
    let is_inline_flex_or_grid = matches!(engine.reader.box_layout_mode(box_idx as usize), Some(crate::layout_model::LayoutMode::Flex(_) | crate::layout_model::LayoutMode::Grid(_)));
    let visible_overflow = !style.overflow_x().clips() && !style.overflow_y().clips();
    let propagated_baseline = if is_inline_table {
        // CSS exposes the first row's baseline for an inline-table. When
        // that row has no line boxes, its bottom edge is the fallback
        // baseline; treating the entire table as bottom-baseline incorrectly
        // adds the parent font strut below the table's margin box.
        first_baseline.or_else(|| inline_table_first_row_bottom(engine, box_idx as usize))
    } else if is_inline_block && visible_overflow {
        last_baseline
    } else if is_inline_flex_or_grid {
        first_baseline
    } else {
        None
    };
    let (ascent, descent) = if let Some(baseline) = propagated_baseline {
        let ascent = margin_top + baseline;
        (ascent, (outer_height - ascent).max(0.0))
    } else {
        (outer_height, 0.0)
    };
    let line_height = if is_inline_table { outer_height } else { resolved_line_height(style) };
    (
        InlineToken::new(InlineTokenKind::AtomicBox { payload_idx: u32::MAX }, outer_width, BreakKind::None, TokenWrap::Normal, true),
        InlineTokenMetrics {
            owner_box_idx: box_idx,
            ascent: ascent as f32,
            descent: descent as f32,
            line_height,
            tab_interval: -1.0,
            tab_min_advance: 0.0,
            font_size: style.font_size(),
            vertical_align,
            white_space: style.white_space(),
            placement_required: false,
        },
        ReplacedToken::AtomicBox { box_idx, border_size, containing_width: max_width, containing_height: containing_block_height, margin_left, margin_top },
    )
}

fn inline_table_first_row_bottom(engine: &crate::layout::LayoutEngine<'_, '_>, table_idx: usize) -> Option<f64> {
    let crate::layout_model::LayoutMode::Table(table) = engine.reader.box_layout_mode(table_idx)? else {
        return None;
    };
    let row_idx = *table.rows.first()? as usize;
    let table_y = engine.geometry.point(table_idx).y;
    let row_point = engine.geometry.point(row_idx);
    let row_size = engine.geometry.size(row_idx);
    Some(row_point.y + row_size.height - table_y)
}

/// Style supplying the space/ch metric used by number-valued `tab-size`.
/// CSS defines it on the nearest block container rather than the inline
/// span that happens to contain the tab.
pub(in crate::layout) fn tab_reference_style<'input>(engine: &crate::layout::LayoutEngine<'input, '_>, box_idx: usize) -> html_style_model::UsedStyleView<'input> {
    let mut current = Some(box_idx);
    while let Some(idx) = current {
        let style = engine.reader.style(idx);
        if style.display() != html_style_model::Display::Inline {
            return style;
        }
        current = engine.reader.get_parent(idx);
    }
    engine.reader.style(box_idx)
}

/// Canonical metrics for a preserved tab. Both intrinsic measurement and
/// final line placement use this helper so tab stops cannot diverge between
/// sizing and layout.
pub(in crate::layout) fn preserved_tab_metrics(style: html_style_model::UsedStyleView<'_>, reference_style: html_style_model::UsedStyleView<'_>, uses_ahem: bool) -> (f64, f64) {
    let ch_advance = reference_style.font_size() as f64 * if uses_ahem { 1.0 } else { 0.5 };
    let interval = match style.tab_size().kind() {
        TabSizeKind::Spaces => style.tab_size().value() as f64 * (ch_advance + reference_style.letter_spacing() as f64 + reference_style.word_spacing() as f64).max(0.0),
        TabSizeKind::LengthPx => style.tab_size().value() as f64,
    };
    (interval.min(f32::MAX as f64), (ch_advance * 0.5).min(f32::MAX as f64))
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

pub(in crate::layout) fn canonical_text_unit(engine: &crate::layout::LayoutEngine<'_, '_>, glyph_idx: u32, style: html_style_model::UsedStyleView<'_>, white_space: WhiteSpace) -> CanonicalTextUnit {
    let glyph = engine.text.glyph_at(glyph_idx as usize).unwrap_or_default();
    let metric = engine.text.glyph_metric(glyph);
    let character = metric.ch();
    let break_kind = match engine.text.whitespace_wrap_before(glyph_idx as usize) {
        crate::layout_model::WhitespaceWrapOverride::Style => break_kind(character, white_space),
        crate::layout_model::WhitespaceWrapOverride::Allow => BreakKind::Soft,
        crate::layout_model::WhitespaceWrapOverride::Suppress => BreakKind::None,
    };
    let natural_advance = if matches!(character, '\u{00ad}' | '\u{200b}') || style.font_size() <= 0.0 {
        0.0
    } else {
        engine.text.text_advance(glyph_idx as usize, metric.advance()) as f64 + style.letter_spacing() as f64 + if character == ' ' { style.word_spacing() as f64 } else { 0.0 }
    };
    CanonicalTextUnit { character, break_kind, natural_advance }
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
