#![cfg_attr(not(test), deny(unsafe_code))]
#![deny(unreachable_pub)]

mod layout_model {
    mod geometry;
    mod glyph_metrics;
    mod inline;
    mod layout_state;
    mod layout_tree;

    pub(crate) use geometry::BoxGeometry;
    pub(crate) use glyph_metrics::GlyphMetrics;
    pub use glyph_metrics::{GlyphMetric, GlyphMetricError};
    pub use inline::GlyphId;
    pub(crate) use inline::{InlineContent, InlineItem, InlineItemKind, WhitespaceWrapOverride};
    pub(crate) use layout_state::{
        AnchorPosition, DecorationFragment, DecorationPattern, DecorationStore, EllipsisFragment,
        GlyphAdvanceRun, GlyphOffsetRun, HyphenFragment, ImageFragment, LayoutState, Line,
        LineInlineBoxFragment, LineTextFragment, OverflowClip, RoundedDecoration,
    };
    pub use layout_state::PreparedTextRunFragment;
    pub(crate) use layout_tree::{
        BlockBox, BoxType, Children, FlexBox, GridBox, LayoutBox, LayoutMode, LayoutTree,
        ListItemMarker, TableBox, TableCellBox, TableColumnGroupSpan, TableColumnTrack,
        TableColumnWidthHint, TableRowBox,
    };
}

#[cfg(test)]
pub(crate) mod parser {
    pub(crate) use crate::test_support::DocumentFactory;
}

#[cfg(test)]
pub mod style {
    pub use html_style::*;
}

#[path = "layout/mod.rs"]
mod layout;

mod flex_grid;
mod shaping;
mod stages;
mod table;

pub use layout::LayoutTimings;
// Stable values crossing the layout-to-renderer boundary.
pub use crate::layout_model::{GlyphId, GlyphMetric, GlyphMetricError, PreparedTextRunFragment};
pub use html_dom::DocumentTocNode;
pub use html_style_model::OpenTypeFeature;
pub use html_style_model::{
    FontStyle, ListStylePosition, ReaderStyleOverrides, StyleStringId, TextAlign,
    TextDecorationLines, UsedBorderRadii,
};
pub use shaping::{
    CharacterPlacement, FontMetricsRequest, FontRelativeMetrics, FontSlant, GlyphRegistry,
    GlyphShaper, ShapeError, ShapedLine, ShapedTextRun, TextRunId, TextRunShapeRequest,
    TextShapeRequest, TextStyleSpan,
};
pub use stages::{
    BoxTextFormat, ImageMetrics, ImageSizingPolicy, LaidOutDocument, LayoutConstraintError,
    LayoutConstraints, NoteFlow, PrepareError, PreparedDocument, RenderAddressingView,
    RenderAnchorPosition, RenderAnchorPositions, RenderAuthoritativeTextRun, RenderBoxView,
    RenderDecoration, RenderDecorationPattern, RenderDecorations, RenderEllipsisFragment,
    RenderForcedBreak, RenderFragmentView, RenderGlyphAdvanceRun, RenderGlyphAdvanceRuns,
    RenderGlyphOffsetRun, RenderGlyphOffsetRuns, RenderHyphenFragment, RenderImageFragment,
    RenderImageFragments, RenderLine, RenderLineTextFragment, RenderLineTextFragments, RenderLines,
    RenderListItemMarker, RenderOverflowClip, RenderTable, RenderTableCell, RenderTableRow,
    RenderTextRun, RenderTextRuns, RenderTextView, RenderView, ShapeLayoutTimings, ShapedDocument,
    SourceElementStep, SourcePosition, TextCompositionPolicy,
};

#[cfg(test)]
mod allocation_test_support {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ALLOCATION_COUNT: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub(crate) struct TrackingAllocator;

    fn record_allocation() {
        let _ = ALLOCATION_COUNT.try_with(|count| {
            if let Some(current) = count.get() {
                count.set(Some(current + 1));
            }
        });
    }

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record_allocation();
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record_allocation();
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    pub(crate) fn count_allocations<T>(operation: impl FnOnce() -> T) -> (T, usize) {
        ALLOCATION_COUNT.with(|count| {
            assert!(
                count.get().is_none(),
                "allocation counters cannot be nested"
            );
            count.set(Some(0));
        });
        let output = operation();
        let allocations =
            ALLOCATION_COUNT.with(|count| count.take().expect("allocation tracking is active"));
        (output, allocations)
    }
}

#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: allocation_test_support::TrackingAllocator =
    allocation_test_support::TrackingAllocator;

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::HashMap;

    use crate::layout_model::{GlyphId, GlyphMetric};

    use crate::GlyphShaper;

    pub(crate) struct DocumentFactory;

    impl DocumentFactory {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn parse_to_dom(&mut self, html: &str) -> html_dom::Document {
            html_parse::parse_dom_document(html).expect("parser produced a valid DOM")
        }

        pub(crate) fn parse_with_new_pipeline(
            &mut self,
            html: &str,
            css: Option<&str>,
        ) -> crate::PreparedDocument {
            match css {
                Some(css) => self.parse_with_new_pipeline_css_chunks(html, &[css]),
                None => self.parse_with_new_pipeline_css_chunks(html, &[]),
            }
        }

        pub(crate) fn parse_with_new_pipeline_css_chunks(
            &mut self,
            html: &str,
            css: &[&str],
        ) -> crate::PreparedDocument {
            let document =
                html_parse::parse_dom_document(html).expect("parser produced a valid DOM");
            let styled = html_style::style_document(document, css);
            let (document, styles) = styled.into_parts();
            crate::PreparedDocument::try_new(document, styles)
                .expect("style resolver must produce complete styles for its document")
        }
    }

    pub(crate) struct TestGlyphShaper {
        glyphs: HashMap<(char, u32), GlyphId>,
        x_height_ratio: f32,
        ch_advance_ratio: f32,
        cap_height_ratio: f32,
        ascent_ratio: Option<f32>,
        font_metric_calls: usize,
    }

    impl Default for TestGlyphShaper {
        fn default() -> Self {
            Self {
                glyphs: HashMap::new(),
                x_height_ratio: 0.5,
                ch_advance_ratio: 0.5,
                cap_height_ratio: 0.8,
                ascent_ratio: None,
                font_metric_calls: 0,
            }
        }
    }

    impl TestGlyphShaper {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        pub(crate) fn len(&self) -> usize {
            self.glyphs.len()
        }

        pub(crate) fn font_metric_calls(&self) -> usize {
            self.font_metric_calls
        }

        pub(crate) fn with_x_height_ratio(x_height_ratio: f32) -> Self {
            Self {
                glyphs: HashMap::new(),
                x_height_ratio,
                ch_advance_ratio: 0.5,
                cap_height_ratio: 0.8,
                ascent_ratio: None,
                font_metric_calls: 0,
            }
        }

        pub(crate) fn with_font_relative_metrics(x_height_ratio: f32, ascent_ratio: f32) -> Self {
            Self {
                glyphs: HashMap::new(),
                x_height_ratio,
                ch_advance_ratio: 0.5,
                cap_height_ratio: 0.8,
                ascent_ratio: Some(ascent_ratio),
                font_metric_calls: 0,
            }
        }

        pub(crate) fn with_font_relative_ratios(
            x_height_ratio: f32,
            ch_advance_ratio: f32,
        ) -> Self {
            Self {
                glyphs: HashMap::new(),
                x_height_ratio,
                ch_advance_ratio,
                cap_height_ratio: 0.8,
                ascent_ratio: None,
                font_metric_calls: 0,
            }
        }
    }

    impl GlyphShaper for TestGlyphShaper {
        fn reset(&mut self) {
            self.glyphs.clear();
        }

        fn shape_glyph<'a>(
            &mut self,
            glyph_metrics: &mut crate::GlyphRegistry<'a>,
            ch: char,
            font_size: f32,
            _font_weight: u16,
            _font_slant: crate::FontSlant,
            _color: u32,
            _family: Option<&str>,
        ) -> Result<GlyphId, crate::ShapeError> {
            let key = (ch, font_size.to_bits());
            if let Some(&glyph) = self.glyphs.get(&key) {
                return Ok(glyph);
            }
            let ascent = font_size * self.ascent_ratio.unwrap_or(0.75);
            let descent = (font_size - ascent).max(0.0);
            let metric =
                GlyphMetric::try_new(ch, font_size * 0.5, ascent, descent, font_size * 0.75)
                    .map_err(crate::ShapeError::rejected_metric)?;
            let glyph = glyph_metrics.register(metric)?;
            self.glyphs.insert(key, glyph);
            Ok(glyph)
        }

        fn font_relative_metrics(
            &mut self,
            _request: crate::FontMetricsRequest<'_>,
        ) -> Result<crate::FontRelativeMetrics, crate::ShapeError> {
            self.font_metric_calls += 1;
            let metrics = match self.ascent_ratio {
                Some(ascent_ratio) => crate::FontRelativeMetrics::from_ratios(
                    self.x_height_ratio,
                    self.ch_advance_ratio,
                    self.cap_height_ratio,
                    ascent_ratio,
                ),
                None => crate::FontRelativeMetrics::new(
                    self.x_height_ratio,
                    self.ch_advance_ratio,
                    self.cap_height_ratio,
                ),
            };
            Ok(metrics.expect("test font-relative metrics must be valid"))
        }
    }
}
