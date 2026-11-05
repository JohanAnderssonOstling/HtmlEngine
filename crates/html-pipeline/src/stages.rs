use crate::layout::{InlinePlanGeneration, LayoutScratch};
use crate::layout_model::{
    AnchorPosition, BoxGeometry, GlyphId, GlyphMetrics, InlineContent, InlineItem, InlineItemKind,
    LayoutMode, LayoutState, LayoutTree,
};
use crate::shaping::{GlyphPatch, GlyphSlice, ShapedTextGeometry, TextGeometryView};
use crate::{GlyphResourceStore, GlyphShaper, LayoutTimings, ShapeError};
use html_dom::{Document, MemoryUsageReport};
use html_engine_model::shaping::first_line_refinement_did_not_converge;
use html_style_model::{ComputedStyles, StyleIndices};
use rustc_data_structures::fx::{FxHashMap, FxHashSet};
use std::ops::Range;
use std::time::Duration;
use web_time::Instant;

#[path = "stages_output/mod.rs"]
mod output;
use html_format::mathml;
pub use output::{
    DocumentTarget, ResolvedDestination, address_queries, box_queries, export_queries,
    fragment_paint, geometry_queries, pagination_queries, reading_queries, search_queries,
    semantic_queries, text_paint,
};
pub use output::{
    BoxTextFormat, CaretAffinity, ContentRole, ContentSemantics, ElementTarget, ImageMetrics,
    LinkTarget, Pagination, PaginationGroup, PaginationLine, PaginationPosition, PaginationTarget,
    ReadingFragment, ReadingPage, ReadingTarget, RenderAddressingView, RenderAuthoritativeTextRun,
    RenderBoxView, RenderDecoration, RenderDecorationPattern, RenderDecorations,
    RenderEllipsisFragment, RenderExportView, RenderForcedBreak, RenderFragmentView,
    RenderGeometryView, RenderGlyphAdvanceRun, RenderGlyphAdvanceRuns, RenderGlyphOffsetRun,
    RenderGlyphOffsetRuns, RenderHyphenFragment, RenderImageFragment, RenderImageFragments,
    RenderLine, RenderLinePlacement, RenderLineTextFragment, RenderLineTextFragments, RenderLines,
    RenderListItemMarker, RenderOverflowClip, RenderPaintItem, RenderResourceLease,
    RenderSemanticsView, RenderTable, RenderTableCell, RenderTableRow, RenderTextRun,
    RenderTextRuns, RenderTextView, RenderView, SourceElementStep, SourceLocation, SourcePosition,
    SourceTextMatch, SourceTextMatches, TableExportFormat, TableExportRequest, TextExport,
    TextExportFormat, TextPatternError, TextPatternMatch, TextPatternMatcher, TextQuote,
    TextSearchOptions, find_text_matches, merge_text_ranges, text_matches,
};
#[path = "stages/lifecycle.rs"]
mod lifecycle;
pub use html_engine_model::{
    ImageSizingPolicy, LayoutConstraintError, LayoutConstraints, NoteFlow, PrepareError,
    TextCompositionPolicy,
};
use lifecycle::{DocumentRead, LayoutProducts, with_append_shaping};
mod paint;
#[path = "stages/semantics.rs"]
mod semantics;

pub(crate) use html_dom::element_is_note_target;
pub(crate) use html_engine_model::TextPaintRange;
pub use lifecycle::{
    DetachedLayout, FormattingDocument, LaidOutDocument, LayoutRequest, NoteLayout, PreparedDocument,
    ShapeLayoutTimings, ShapedDocument,
};
pub(crate) use lifecycle::{FirstLineRefinement, PreparedInputs, ShapedText};
use semantics::{LinkGlyphTarget, box_id, collect_glyph_targets};

#[derive(Clone, Debug, Eq, PartialEq)]
struct FirstLineStyleRange {
    glyphs: Range<u32>,
    style: StyleIndices,
    base_style: StyleIndices,
}

struct FirstLineTextRun<'a> {
    glyphs: &'a Range<u32>,
    box_idx: u32,
    may_join_previous: bool,
}

/// Nonempty text ranges retain source order after preparation. Empty ranges
/// become `0..0`, so they cannot participate in the binary search. Record
/// adjacency before filtering: empty text and structural items still affect
/// whether two selected runs may share a shaping range.
struct FirstLineTextRuns<'a> {
    runs: Vec<FirstLineTextRun<'a>>,
}

impl<'a> FirstLineTextRuns<'a> {
    fn new(items: &'a [InlineItem]) -> Self {
        let mut previous_text_end = None;
        let runs = items
            .iter()
            .filter_map(|item| {
                let InlineItemKind::Text { glyphs } = &item.kind else {
                    previous_text_end = None;
                    return None;
                };
                let may_join_previous = previous_text_end == Some(glyphs.start);
                previous_text_end = Some(glyphs.end);
                (!glyphs.is_empty()).then_some(FirstLineTextRun {
                    glyphs,
                    box_idx: item.box_idx,
                    may_join_previous,
                })
            })
            .collect();
        Self { runs }
    }

    fn overlapping(&self, glyphs: Range<u32>) -> impl Iterator<Item = &FirstLineTextRun<'a>> {
        let start = self
            .runs
            .partition_point(|run| run.glyphs.end <= glyphs.start);
        self.runs[start..]
            .iter()
            .take_while(move |run| glyphs.start < glyphs.end && run.glyphs.start < glyphs.end)
    }
}

fn first_line_style_ranges<'a>(
    inputs: &'a PreparedInputs,
    layout_state: &LayoutState,
    text_runs: &mut Option<FirstLineTextRuns<'a>>,
) -> Vec<FirstLineStyleRange> {
    if !inputs.styles.has_first_line_styles() {
        return Vec::new();
    }
    let first_letter = inputs.text.first_letter_styles();
    let mut result = Vec::new();
    for line in layout_state.line_output.lines.iter().filter(|line| {
        line.paint_color(&layout_state.line_output.line_details)
            .is_some()
    }) {
        let line_result_start = result.len();
        let text_runs = text_runs
            .get_or_insert_with(|| FirstLineTextRuns::new(inputs.text.content().inline_items()));
        for run in text_runs.overlapping(line.glyphs.clone()) {
            let glyphs = run.glyphs;
            let start = glyphs.start.max(line.glyphs.start);
            let end = glyphs.end.min(line.glyphs.end);
            if start >= end {
                continue;
            }
            let root = html_text::whitespace_context_root(
                inputs.layout_tree.as_ref(),
                run.box_idx as usize,
            );
            let Some(first_line_style) = inputs.text.content().pseudo_styles().first_line(root) else {
                continue;
            };
            let base_style = inputs
                .layout_tree
                .get_box_style_indices(run.box_idx as usize)
                .unwrap_or_else(|| inputs.styles.default_indices());
            let mut segment_start = start;
            let (style, mut glyph) = first_letter.segment(start, end);
            let mut segment_style = style.unwrap_or(first_line_style);
            while glyph < end {
                let (style, next) = first_letter.segment(glyph, end);
                let style = style.unwrap_or(first_line_style);
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
                glyph = next;
            }
            append_first_line_style_range(
                &inputs.styles,
                &mut result,
                FirstLineStyleRange {
                    glyphs: segment_start..end,
                    style: segment_style,
                    base_style,
                },
                run.may_join_previous && segment_start == start,
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
    glyph_shaper: &mut impl GlyphShaper,
    resources: &mut GlyphResourceStore,
) -> Result<FirstLineRefinement, ShapeError> {
    let mut refined = FirstLineRefinement {
        glyph_patches: Vec::new(),
        paint_ranges: ranges
            .iter()
            .map(|range| TextPaintRange {
                glyphs: range.glyphs.clone(),
                style: range.style,
            })
            .collect(),
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
            &base.shared.font_metrics,
            inputs.text.content(),
            &mut glyphs,
            source.start,
            &base.text_geometry.bidi,
            &mut refined.text_geometry,
            source.clone(),
            range.style,
            range.base_style,
            glyph_shaper,
            resources,
        )?;
        if glyphs != original {
            refined.glyph_patches.push(GlyphPatch { source, glyphs });
        }
    }

    Ok(refined)
}

/// Refinement either shares its caller's transaction or opens an append transaction.
enum RefinementTransaction {
    Existing,
    Append,
}

/// Resolve the circular dependency between `::first-line` style and the line
/// boundary with a bounded fixed-point pass. Every attempt starts from the
/// viewport-independent shape, so characters that leave the first line never
/// retain pseudo styling from the preceding width.
fn layout_with_first_lines(
    inputs: &PreparedInputs,
    base_shaped: &ShapedText,
    constraints: LayoutConstraints,
    image_metrics: &ImageMetrics,
    glyph_shaper: &mut impl GlyphShaper,
    mut timings: Option<&mut LayoutTimings>,
    layout_scratch: LayoutScratch,
    resources: &mut GlyphResourceStore,
    transaction: RefinementTransaction,
) -> Result<LayoutProducts, ShapeError> {
    let mut laid_out = LayoutProducts::new(
        inputs,
        base_shaped,
        constraints,
        image_metrics,
        timings.as_deref_mut(),
        layout_scratch,
        resources,
    );
    let mut text_runs = None;
    let mut ranges = first_line_style_ranges(inputs, &laid_out.state, &mut text_runs);
    if ranges.is_empty() {
        return Ok(laid_out);
    }

    let refine = move |glyph_shaper: &mut _, resources: &mut GlyphResourceStore| {
        // Font changes can move the boundary more than once. Never publish a
        // layout whose first line differs from the range used to shape it.
        const MAX_FIRST_LINE_PASSES: usize = 32;
        let mut seen_ranges = Vec::new();
        for _ in 0..MAX_FIRST_LINE_PASSES {
            seen_ranges.push(ranges.clone());
            let shaped =
                shape_first_line_ranges(inputs, base_shaped, &ranges, glyph_shaper, resources)?;
            // This document is still a private candidate. Reuse its layout
            // buffers while the first-line boundary converges, then publish it.
            laid_out.refinement = Some(shaped);
            laid_out.relayout(
                inputs,
                base_shaped,
                constraints,
                image_metrics,
                timings.as_deref_mut(),
                resources,
            );
            let next_ranges = first_line_style_ranges(inputs, &laid_out.state, &mut text_runs);
            if next_ranges == ranges {
                return Ok(laid_out);
            }
            if seen_ranges.contains(&next_ranges) {
                return Err(first_line_refinement_did_not_converge());
            }
            ranges = next_ranges;
        }
        Err(first_line_refinement_did_not_converge())
    };
    match transaction {
        RefinementTransaction::Existing => refine(glyph_shaper, resources),
        RefinementTransaction::Append => {
            with_append_shaping(glyph_shaper, &inputs.publication_fonts, refine, resources)
        }
    }
}

include!("stages_tests.rs");

pub use output::source_queries;
