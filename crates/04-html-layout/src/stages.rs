use crate::layout::InlinePlanGeneration;
use crate::shaping::{GlyphPatch, GlyphSlice, TextGeometryView, ShapedTextGeometry};
use crate::layout_model::{
    AnchorPosition, BoxGeometry, GlyphId, GlyphMetrics, InlineContent, InlineItemKind, LayoutMode,
    LayoutState, LayoutTree,
};
use html_dom::{Document, MemoryUsageReport};
use html_style_model::{ComputedStyles, ComputedStylesValidationError, StyleIndices};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::fmt;
use std::time::Duration;
use web_time::Instant;

#[path = "stages_output/mod.rs"]
mod output;
pub use output::{
    CaretAffinity, PositionedShapedLine, RenderGeometryView,
    TextSearchOptions, find_text_matches,
    SourceLocation,
    ContentRole, ContentSemantics, RenderSemanticsView,
    BoxTextFormat, ImageMetrics, RenderAddressingView, RenderAnchorPosition, RenderAnchorPositions, ReadingFragment, ReadingTarget, is_navigation_section_title,
    RenderAuthoritativeTextRun, RenderBoxView, RenderDecoration, RenderDecorationPattern,
    RenderDecorations, RenderEllipsisFragment, RenderExportView, RenderForcedBreak, RenderFragmentView,
    RenderGlyphAdvanceRun, RenderGlyphAdvanceRuns, RenderGlyphOffsetRun, RenderGlyphOffsetRuns,
    RenderHyphenFragment, RenderImageFragment, RenderImageFragments, RenderLine,
    RenderLineTextFragment, RenderLineTextFragments, RenderLines, RenderListItemMarker,
    RenderOverflowClip, RenderTable, RenderTableCell, RenderTableRow, RenderTextRun,
    RenderTextRuns, RenderTextView, RenderView, SourceElementStep, SourcePosition,
};
mod mathml;
#[path = "stages/lifecycle.rs"]
mod lifecycle;
use lifecycle::LayoutCandidate;
mod paint;
pub(crate) use paint::PaintStyles;
#[path = "stages/semantics.rs"]
mod semantics;

pub(crate) use html_dom::element_is_note_target;
pub use lifecycle::{ImageSizingPolicy, LaidOutDocument, LayoutConstraintError, LayoutConstraints, NoteFlow, PrepareError, PreparedDocument, ShapeLayoutTimings, ShapedDocument, TextCompositionPolicy};
pub(crate) use lifecycle::{FirstLineRefinement, PaintColorRun, PreparedInputs, ShapedText, TextPaintRange};
use semantics::{LinkGlyphTarget, box_id, collect_anchor_glyphs, collect_link_glyph_targets};

#[derive(Clone, Debug, Eq, PartialEq)]
struct FirstLineStyleRange {
    glyphs: std::ops::Range<u32>,
    style: StyleIndices,
    base_style: StyleIndices,
}

fn first_line_style_ranges(inputs: &PreparedInputs, layout_state: &LayoutState) -> Vec<FirstLineStyleRange> {
    if !inputs.styles.has_first_line_styles() {
        return Vec::new();
    }
    let first_letter = inputs.text.first_letter_styles();
    let mut result = Vec::new();
    for line in layout_state
        .line_output
        .lines
        .iter()
        .filter(|line| line.paint_color(&layout_state.line_output.line_details).is_some())
    {
        let line_result_start = result.len();
        let mut previous_text_end = None;
        for run in inputs.text.content().inline_items() {
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
            let root = crate::shaping::whitespace_context_root(
                &inputs.layout_tree,
                run.box_idx as usize,
            );
            let Some(first_line_style) = crate::shaping::first_line_style_for_inline_root(
                &inputs.document,
                &inputs.styles,
                &inputs.layout_tree,
                inputs.text.content(),
                root,
            ) else {
                continue;
            };
            let base_style = inputs
                .layout_tree
                .get_box_style_indices(run.box_idx as usize)
                .unwrap_or_else(|| inputs.styles.default_indices());
            let mut segment_start = start;
            let mut segment_style = first_letter.segment(start, start + 1).0
                .unwrap_or(first_line_style);
            for glyph in start + 1..end {
                let style = first_letter.segment(glyph, glyph + 1).0
                    .unwrap_or(first_line_style);
                if style != segment_style {
                    append_first_line_style_range(
                        &inputs.styles,
                        &mut result,
                        FirstLineStyleRange {
                            glyphs: segment_start..glyph,
                            style: segment_style,
                            base_style,
                        },
                        false,
                        line_result_start,
                    );
                    segment_start = glyph;
                    segment_style = style;
                }
            }
            append_first_line_style_range(
                &inputs.styles,
                &mut result,
                FirstLineStyleRange {
                    glyphs: segment_start..end,
                    style: segment_style,
                    base_style,
                },
                may_join_previous_run && segment_start == start,
                line_result_start,
            );
        }
    }
    result
}

/// A `::first-line` pseudo-element forms one fictional inline box. DOM inline
/// element boundaries therefore must not become font-shaping boundaries when
/// the effective pseudo style and the spacing inherited from the real boxes
/// are identical. Keeping one range also preserves kerning and ligatures
/// across otherwise paint-only spans.
fn append_first_line_style_range(
    styles: &ComputedStyles,
    result: &mut Vec<FirstLineStyleRange>,
    candidate: FirstLineStyleRange,
    may_join: bool,
    line_result_start: usize,
) {
    if may_join && result.len() > line_result_start {
        let previous = result
            .last_mut()
            .expect("line has a preceding pseudo-style range");
        let previous_base = styles
            .view(previous.base_style)
            .expect("validated base style");
        let candidate_base = styles
            .view(candidate.base_style)
            .expect("validated base style");
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
    inputs: &PreparedInputs,
    base: &ShapedText,
    ranges: &[FirstLineStyleRange],
    glyph_shaper: &mut impl crate::GlyphShaper,
) -> Result<FirstLineRefinement, crate::ShapeError> {
    let mut refined = FirstLineRefinement {
        glyph_patches: Vec::new(),
        paint_ranges: ranges
            .iter()
            .map(|range| TextPaintRange {
                glyphs: range.glyphs.clone(),
                style: range.style,
            })
            .collect(),
        glyph_metrics: GlyphMetrics::default(),
        text_geometry: ShapedTextGeometry::with_len(base.glyphs.len()),
        inline_plan_generation: InlinePlanGeneration::new(),
    };
    refined.paint_ranges.sort_by_key(|range| range.glyphs.start);
    for range in ranges {
        let source = range.glyphs.clone();
        let original = &base.glyphs[source.start as usize..source.end as usize];
        let mut glyphs = original.to_vec();
        crate::shaping::reshape_range_with_style(
            &inputs.styles,
            inputs.text.content(),
            &mut glyphs,
            source.start,
            &base.text_geometry.bidi,
            &mut refined.text_geometry,
            source.clone(),
            range.style,
            range.base_style,
            glyph_shaper,
        )?;
        if glyphs != original {
            refined.glyph_patches.push(GlyphPatch { source, glyphs });
        }
    }
    refined.glyph_metrics = glyph_shaper.glyph_resources().snapshot();
    Ok(refined)
}

/// Resolve the circular dependency between `::first-line` style and the line
/// boundary with a bounded fixed-point pass. Every attempt starts from the
/// viewport-independent shape, so characters that leave the first line never
/// retain pseudo styling from the preceding width.
fn refine_first_lines(
    mut laid_out: LayoutCandidate,
    inputs: &PreparedInputs,
    base_shaped: &ShapedText,
    shaping_seed: GlyphMetrics,
    constraints: LayoutConstraints,
    image_metrics: &ImageMetrics,
    glyph_shaper: &mut impl crate::GlyphShaper,
) -> Result<LayoutCandidate, crate::ShapeError> {
    let mut ranges = first_line_style_ranges(inputs, &laid_out.layout_state);
    if ranges.is_empty() {
        if shaping_seed.len() > base_shaped.glyph_metrics.len() {
            laid_out.glyph_metrics_override = Some(shaping_seed);
        }
        return Ok(laid_out);
    }

    // Font changes can move the boundary more than once. Never publish a
    // layout whose first line differs from the range used to shape it.
    const MAX_FIRST_LINE_PASSES: usize = 32;
    let mut seen_ranges = Vec::new();
    for _ in 0..MAX_FIRST_LINE_PASSES {
        seen_ranges.push(ranges.clone());
        let shaped = shape_first_line_ranges(inputs, base_shaped, &ranges, glyph_shaper)?;
        // This document is still a private candidate. Reuse its layout
        // buffers while the first-line boundary converges, then publish it.
        laid_out.refinement = Some(shaped);
        laid_out.glyph_metrics_override = None;
        laid_out.relayout(inputs, base_shaped, constraints, image_metrics, None);
        let next_ranges = first_line_style_ranges(inputs, &laid_out.layout_state);
        if next_ranges == ranges {
            return Ok(laid_out);
        }
        if seen_ranges.contains(&next_ranges) {
            return Err(crate::ShapeError::first_line_refinement_did_not_converge());
        }
        ranges = next_ranges;
    }
    Err(crate::ShapeError::first_line_refinement_did_not_converge())
}

pub(crate) struct AncestorIter<'a> {
    doc: &'a LaidOutDocument,
    current: Option<usize>,
}

impl<'a> Iterator for AncestorIter<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        let idx = self.current?;
        self.current = self
            .doc
            .inputs
            .layout_tree
            .box_at(idx)
            .and_then(|b| b.parent().map(|parent| parent as usize));
        Some(idx)
    }
}

include!("stages_tests.rs");
