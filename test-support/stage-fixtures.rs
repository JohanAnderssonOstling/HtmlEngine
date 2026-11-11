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
    use crate::{FontMetrics, ShapedRun, TextRunShapeRequest, ShapedGlyph};

    use std::collections::HashMap;

    use html_engine_model::GlyphHandle;

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
        ) -> crate::PreparedDocument<'static> {
            match css {
                Some(css) => self.parse_with_new_pipeline_css_chunks(html, &[css]),
                None => self.parse_with_new_pipeline_css_chunks(html, &[]),
            }
        }

        pub(crate) fn parse_with_new_pipeline_css_chunks(
            &mut self,
            html: &str,
            css: &[&str],
        ) -> crate::PreparedDocument<'static> {
            let document =
                html_parse::parse_dom_document(html).expect("parser produced a valid DOM");
            let styled = html_style::style_document(document, css);
            let (document, styles) = styled.into_parts();
            crate::PreparedDocument::try_new(document, styles)
                .expect("style resolver must produce complete styles for its document")
        }
    }

    pub(crate) struct TestGlyphShaper {
        glyphs: HashMap<(char, u32), GlyphHandle>,
        generation: crate::GlyphResourceGeneration,
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
                generation: crate::GlyphResourceGeneration::default(),
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
                generation: crate::GlyphResourceGeneration::default(),
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
                generation: crate::GlyphResourceGeneration::default(),
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
                generation: crate::GlyphResourceGeneration::default(),
                x_height_ratio,
                ch_advance_ratio,
                cap_height_ratio: 0.8,
                ascent_ratio: None,
                font_metric_calls: 0,
            }
        }
    }

    impl GlyphShaper for TestGlyphShaper {
        fn resource_generation(&self) -> crate::GlyphResourceGeneration {
            self.generation
        }

        fn shape_run(&mut self, request: TextRunShapeRequest<'_>) -> ShapedRun {
            ShapedRun::characters(request, |character, style| {
                let key = (character, style.font_size.to_bits());
                let ascent = style.font_size * self.ascent_ratio.unwrap_or(0.75);
                let descent = (style.font_size - ascent).max(0.0);
                let advance = style.font_size * 0.5;
                let metrics = FontMetrics { ascent, descent };
                if let Some(&glyph) = self.glyphs.get(&key) {
                    return ShapedGlyph::new(glyph, advance, metrics);
                }
                let glyph = GlyphHandle(self.glyphs.len() as u32);
                self.glyphs.insert(key, glyph);
                ShapedGlyph::new(glyph, advance, metrics)
            })
        }

        fn font_relative_metrics(
            &mut self,
            _request: &crate::ShapingStyle<'_>,
        ) -> crate::FontRelativeMetrics {
            self.font_metric_calls += 1;
            crate::FontRelativeMetrics {
                x_height_ratio: self.x_height_ratio,
                ch_advance_ratio: self.ch_advance_ratio,
                cap_height_ratio: self.cap_height_ratio,
                line_metrics: self
                    .ascent_ratio
                    .map(|ascent_ratio| crate::FontLineMetrics {
                        ascent_ratio,
                        descent_ratio: (1.0 - ascent_ratio).max(0.0),
                    }),
                ..crate::FontRelativeMetrics::default()
            }
        }
    }
}
