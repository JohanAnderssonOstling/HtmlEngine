//! Font alignment, indentation and interned token metrics.
#[cfg(any(test, feature = "testing"))]
use super::InlineToken;
#[cfg(any(test, feature = "testing"))]
use super::InlineTokenKind;
use crate::LayoutReader;
use html_style_model::{VerticalAlignValue, WhiteSpace};
use std::iter::once;
use std::mem::{discriminant, size_of};

/// Derives a final line box from ascent, descent, and a minimum height.
///
/// Callers use this both for raw font metrics and for the adjusted metrics after
/// vertical-align offsets have been applied.
pub(in crate::inline) fn compute_baseline(
    max_ascent: f64,
    max_descent: f64,
    min_height: f64,
) -> (f64, f64) {
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
pub(in crate::inline) fn vertical_align_offset(
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
        f64::from(metrics.font_size) * 0.5,
    )
}

pub(in crate::inline) fn vertical_align_metrics_offset(
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
    parent_x_height: f64,
) -> f64 {
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
        // Align the margin-box midpoint with the parent baseline plus half
        // its x-height. This must not depend on the evolving line box.
        VerticalAlignValue::Middle => (descent - ascent + parent_x_height) * 0.5,
    }
}

/// Resolves the indent to apply to the current line.
///
/// This keeps the first-line and hanging-indent policy out of the main line
/// breaking loop.
pub(crate) fn indent_for_line(text_indent: f64, hanging: bool, is_first_line: bool) -> f64 {
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
pub(in crate::inline) fn text_vertical_align(
    doc: &LayoutReader<'_>,
    box_idx: usize,
) -> VerticalAlignValue {
    if doc.is_inline_box(box_idx) {
        doc.style(box_idx).vertical_align()
    } else {
        VerticalAlignValue::Baseline
    }
}

pub(in crate::inline) fn has_aligned_inline_ancestor(
    doc: &LayoutReader<'_>,
    box_idx: usize,
) -> bool {
    // Source text is wrapped by a conceptual anonymous inline box. Its own
    // non-inherited `vertical-align` is therefore `baseline`; alignment on
    // the element owning the text is movement of the enclosing box.
    for ancestor_idx in once(box_idx).chain(doc.layout_tree().ancestors(box_idx)) {
        let ancestor_style = doc.style(ancestor_idx);
        if ancestor_style.display() != html_style_model::Display::Inline {
            break;
        }
        if !ancestor_style.vertical_align().is_initial()
            || ancestor_style.position() == html_style_model::PositionMode::Relative
        {
            return true;
        }
    }
    false
}

/// Numeric alignment is resolved against the owner's font size and this run's
/// own line-height before interning. Only the remaining keywords depend on
/// the containing line or parent font during placement.
#[derive(Clone, Copy, Debug)]
pub(in crate::inline) enum InlineVerticalAlign {
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
    pub(in crate::inline) fn resolve(
        value: VerticalAlignValue,
        line_height: f64,
        font_size: f32,
    ) -> Self {
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
            VerticalAlignValue::Calc {
                absolute_px,
                line_height_fraction,
                ..
            } => Self::Offset(
                (f64::from(absolute_px) + line_height * f64::from(line_height_fraction)) as f32,
            ),
        }
    }

    /// Reuse the shared placement calculation without retaining the larger
    /// computed-style enum in each metrics entry.
    #[inline]
    pub(in crate::inline) fn as_style_value(self) -> VerticalAlignValue {
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
pub(in crate::inline) struct InlineTokenMetrics {
    /// Prepared layout box that owns this run. Keeping ownership once per
    /// interned metrics run lets fragment publication recover inline ancestry
    /// without expanding the 12-byte dense token representation.
    pub(in crate::inline) owner_box_idx: u32,
    pub(in crate::inline) ascent: f32,
    pub(in crate::inline) descent: f32,
    pub(in crate::inline) line_height: f32,
    pub(in crate::inline) font_size: f32,
    pub(in crate::inline) vertical_align: InlineVerticalAlign,
    pub(in crate::inline) white_space: WhiteSpace,
    pub(in crate::inline) visible: bool,
    pub(in crate::inline) placement_required: bool,
    // Directional controls retain source slots but own no glyph layout box.
    pub(in crate::inline) contributes_line_metrics: bool,
}

impl InlineTokenMetrics {
    /// Minimum vertical extent used when probing a float exclusion lane.
    pub(in crate::inline) fn line_extent(&self) -> f64 {
        (f64::from(self.ascent) + f64::from(self.descent)).max(f64::from(self.line_height))
    }
}

const _: () = assert!(size_of::<InlineTokenMetrics>() <= 32);
