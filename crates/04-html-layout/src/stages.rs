use crate::layout_model::{AnchorPosition, BoxGeometry, GlyphId, GlyphMetrics, InlineContent, InlineItemKind, LayoutMode, LayoutState, LayoutTree};
use html_dom::{Document, MemoryUsageReport};
use html_style_model::{ComputedStyles, ComputedStylesValidationError, StyleIndices};
use rustc_data_structures::fx::FxHashMap;
use std::fmt;

#[path = "stages_output/mod.rs"]
mod output;
pub use output::{
    BoxTextFormat, ImageMetrics, RenderAddressingView, RenderAnchorPosition, RenderAnchorPositions, RenderAuthoritativeTextRun, RenderBoxView, RenderDecoration, RenderDecorationPattern, RenderDecorations,
    RenderEllipsisFragment, RenderForcedBreak, RenderFragmentView, RenderGlyphAdvanceRun, RenderGlyphAdvanceRuns, RenderGlyphOffsetRun, RenderGlyphOffsetRuns, RenderHyphenFragment, RenderImageFragment, RenderImageFragments, RenderLine,
    RenderLineTextFragment, RenderLineTextFragments, RenderLines, RenderListItemMarker, RenderOverflowClip, RenderTable, RenderTableCell, RenderTableRow, RenderTextRun, RenderTextRuns, RenderTextView, RenderView,
    SourceElementStep, SourcePosition,
};
#[path = "stages/lifecycle.rs"]
mod lifecycle;
#[path = "stages/semantics.rs"]
mod semantics;

pub use lifecycle::{ImageSizingPolicy, LaidOutDocument, LayoutConstraintError, LayoutConstraints, NoteFlow, PrepareError, PreparedDocument, ShapedDocument, TextCompositionPolicy};
pub(crate) use lifecycle::{PreparedInputs, ShapedText};
pub(crate) use html_dom::element_is_note_target;
use semantics::{LinkGlyphTarget, box_id, collect_anchor_glyphs, collect_link_glyph_targets};

#[derive(Clone, Debug, Eq, PartialEq)]
struct FirstLineStyleRange {
    glyphs: std::ops::Range<u32>,
    style: StyleIndices,
    base_style: StyleIndices,
}

fn first_line_style_ranges(document: &LaidOutDocument) -> Vec<FirstLineStyleRange> {
    let first_letter = crate::shaping::first_letter_style_overrides(&document.inputs.document, &document.inputs.styles, &document.inputs.layout_tree, &document.base_shaped.inline_content);
    let mut result = Vec::new();
    for line in document.layout_state.line_output.lines.iter().filter(|line| line.paint_color.is_some()) {
        let line_result_start = result.len();
        let mut previous_text_end = None;
        for run in document.base_shaped.inline_content.inline_items() {
            let InlineItemKind::Text { glyphs } = &run.kind else {
                // Atomic inline content is a real shaping boundary even when
                // it occupies no positions in the document glyph buffer.
                previous_text_end = None;
                continue;
            };
            let may_join_previous_run = previous_text_end == Some(glyphs.start);
            previous_text_end = Some(glyphs.end);
            let start = glyphs.start.max(line.glyphs.start);
            let end = glyphs.end.min(line.glyphs.end);
            if start >= end {
                continue;
            }
            let root = crate::shaping::whitespace_context_root(&document.inputs.layout_tree, run.box_idx as usize);
            let Some(first_line_style) = crate::shaping::first_line_style_for_inline_root(&document.inputs.document, &document.inputs.styles, &document.inputs.layout_tree, &document.base_shaped.inline_content, root) else {
                continue;
            };
            let base_style = document.inputs.layout_tree.get_box_style_indices(run.box_idx as usize).unwrap_or_else(|| document.inputs.styles.default_indices());
            let mut segment_start = start;
            let mut segment_style = first_letter[start as usize].unwrap_or(first_line_style);
            for glyph in start + 1..end {
                let style = first_letter[glyph as usize].unwrap_or(first_line_style);
                if style != segment_style {
                    append_first_line_style_range(document, &mut result, FirstLineStyleRange { glyphs: segment_start..glyph, style: segment_style, base_style }, false, line_result_start);
                    segment_start = glyph;
                    segment_style = style;
                }
            }
            append_first_line_style_range(document, &mut result, FirstLineStyleRange { glyphs: segment_start..end, style: segment_style, base_style }, may_join_previous_run && segment_start == start, line_result_start);
        }
    }
    result
}

/// A `::first-line` pseudo-element forms one fictional inline box. DOM inline
/// element boundaries therefore must not become font-shaping boundaries when
/// the effective pseudo style and the spacing inherited from the real boxes
/// are identical. Keeping one range also preserves kerning and ligatures
/// across otherwise paint-only spans.
fn append_first_line_style_range(document: &LaidOutDocument, result: &mut Vec<FirstLineStyleRange>, candidate: FirstLineStyleRange, may_join: bool, line_result_start: usize) {
    if may_join && result.len() > line_result_start {
        let previous = result.last_mut().expect("line has a preceding pseudo-style range");
        let previous_base = document.inputs.styles.view(previous.base_style).expect("validated base style");
        let candidate_base = document.inputs.styles.view(candidate.base_style).expect("validated base style");
        if previous.glyphs.end == candidate.glyphs.start
            && previous.style == candidate.style
            && previous_base.letter_spacing().to_bits() == candidate_base.letter_spacing().to_bits()
            && previous_base.word_spacing().to_bits() == candidate_base.word_spacing().to_bits()
        {
            previous.glyphs.end = candidate.glyphs.end;
            return;
        }
    }
    result.push(candidate);
}

fn shape_first_line_ranges(
    inputs: &std::sync::Arc<PreparedInputs>,
    base: &std::sync::Arc<ShapedText>,
    seed: &std::sync::Arc<ShapedText>,
    ranges: &[FirstLineStyleRange],
    glyph_shaper: &mut impl crate::GlyphShaper,
) -> Result<std::sync::Arc<ShapedText>, crate::ShapeError> {
    // Preserve metrics registered by an earlier attempt: renderer glyph IDs
    // remain live for the whole document-shaping transaction.
    let retained_metrics = seed.glyph_metrics.clone();
    let mut shaped = (**base).clone();
    shaped.glyph_metrics = retained_metrics;
    shaped.inline_plans = crate::layout::PreparedInlinePlans::new(inputs.layout_tree.box_count());
    for range in ranges {
        crate::shaping::reshape_range_with_style(&inputs.styles, &mut shaped.inline_content, &mut shaped.glyph_metrics, &mut shaped.text_geometry, range.glyphs.clone(), range.style, range.base_style, glyph_shaper)?;
    }
    Ok(std::sync::Arc::new(shaped))
}

/// Resolve the circular dependency between `::first-line` style and the line
/// boundary with a bounded fixed-point pass. Every attempt starts from the
/// viewport-independent shape, so characters that leave the first line never
/// retain pseudo styling from the preceding width.
fn refine_first_lines(
    mut laid_out: LaidOutDocument,
    base_shaped: std::sync::Arc<ShapedText>,
    mut shaping_seed: std::sync::Arc<ShapedText>,
    constraints: LayoutConstraints,
    image_metrics: &ImageMetrics,
    glyph_shaper: &mut impl crate::GlyphShaper,
) -> Result<LaidOutDocument, crate::ShapeError> {
    let mut ranges = first_line_style_ranges(&laid_out);
    if ranges.is_empty() {
        laid_out.base_shaped = base_shaped;
        return Ok(laid_out);
    }

    // A second pass is enough to remove stale styling when the changed metrics
    // move the first boundary. Stop early when the styled source set is stable.
    for _ in 0..2 {
        let shaped = shape_first_line_ranges(&laid_out.inputs, &base_shaped, &shaping_seed, &ranges, glyph_shaper)?;
        shaping_seed = shaped.clone();
        let candidate = ShapedDocument { inputs: laid_out.inputs.clone(), shaped }.layout_impl(constraints, image_metrics, None);
        let next_ranges = first_line_style_ranges(&candidate);
        laid_out = candidate;
        if next_ranges == ranges {
            break;
        }
        ranges = next_ranges;
    }
    laid_out.base_shaped = base_shaped;
    Ok(laid_out)
}

pub(crate) struct AncestorIter<'a> {
    doc: &'a LaidOutDocument,
    current: Option<usize>,
}

impl<'a> Iterator for AncestorIter<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        let idx = self.current?;
        self.current = self.doc.inputs.layout_tree.box_at(idx).and_then(|b| b.parent().map(|parent| parent as usize));
        Some(idx)
    }
}

include!("stages_tests.rs");
