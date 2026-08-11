use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use html_dom::RootFontSize;
use html_layout::{GlyphShaper as LayoutGlyphShaper, LaidOutDocument, LayoutConstraints as LayoutConstraintsOutput, PreparedDocument, ShapedDocument};
use html_parse::{MarkupSyntax, ParsedHtml, parse_document};
use html_resources::ResourceProvider;
use html_style::{MediaMatchKey, MediaQuerySet};

use crate::parser::DocumentFactory;
use crate::types::{LayoutCacheKey, ParsedCacheKey, PreparedCacheKey, RenderOnlyCacheKey, ShapedCacheKey};
use crate::types::{LayoutConstraints, ResourceRevision, StylesheetRevision};
use crate::{EarliestStage, PipelineError, PipelineInputs, PipelineRetainedBytes, PipelineStageCounts, PipelineTimings, PipelineUpdate, Reuse, ReuseReport};

#[derive(Default)]
pub struct PipelineCacheState {
    pub inputs: Option<PipelineInputs>,
    pub parsed_key: Option<ParsedCacheKey>,
    pub parsed: Option<Arc<ParsedHtml>>,
    pub prepared_key: Option<PreparedCacheKey>,
    pub prepared: Option<PreparedDocument>,
    pub media_queries: Option<MediaQuerySet>,
    pub media_match_key: Option<MediaMatchKey>,
    pub shaped_key: Option<ShapedCacheKey>,
    pub shaped: Option<ShapedDocument>,
    pub laid_out_key: Option<LayoutCacheKey>,
    pub laid_out: Option<LaidOutDocument>,
    pub retained_bytes: PipelineRetainedBytes,
}

impl PipelineCacheState {
    fn parsed_matches(&self, key: &ParsedCacheKey) -> bool {
        self.parsed_key.as_ref() == Some(key) && self.parsed.is_some()
    }

    fn prepared_matches(&self, key: &PreparedCacheKey) -> bool {
        self.prepared_key.as_ref() == Some(key) && self.prepared.is_some()
    }

    fn shaped_matches(&self, key: &ShapedCacheKey) -> bool {
        self.shaped_key.as_ref() == Some(key) && self.shaped.is_some()
    }

    fn laid_out_matches(&self, key: &LayoutCacheKey) -> bool {
        self.laid_out_key.as_ref() == Some(key) && self.laid_out.is_some()
    }
}

pub struct PipelineSession {
    cached_provider: Arc<RevisionCachedResourceProvider>,
    cached_resource_key: Option<(ResourceRevision, StylesheetRevision)>,
    factory: DocumentFactory,
    cache: PipelineCacheState,
    /// Resources as the shaper currently holds them, once notes have been
    /// appended. Each note continues past the last, so the next one has to be
    /// seeded from this rather than from the page, whose table stops short.
    /// Cleared whenever the page is reshaped and the store starts over.
    notes_shaped: Option<ShapedDocument>,
}

impl PipelineSession {
    pub fn new(provider: Arc<dyn ResourceProvider>) -> Self {
        let cached_provider = Arc::new(RevisionCachedResourceProvider::new(provider.clone()));
        Self { cached_provider, cached_resource_key: None, factory: DocumentFactory::new(), cache: PipelineCacheState::default(), notes_shaped: None }
    }

    pub fn update(&mut self, requested_inputs: PipelineInputs, glyph_shaper: &mut impl LayoutGlyphShaper) -> Result<PipelineUpdate, PipelineError> {
        let update_started = Instant::now();
        let decision_started = Instant::now();
        self.refresh_resource_cache(requested_inputs.resource_revision, requested_inputs.stylesheet_revision);
        let base_uri = if requested_inputs.base_uri.is_empty() { "index.html".to_string() } else { requested_inputs.base_uri.clone() };
        self.factory.set_resource_context(self.cached_provider.clone(), base_uri);
        self.factory.set_root_font_size(to_root_font_size(requested_inputs.style_environment.root_font_size));
        self.factory.set_media_environment(requested_inputs.style_environment.media);
        self.factory.set_reader_overrides(requested_inputs.reader_overrides.clone());
        self.factory.set_note_flow(requested_inputs.note_flow);

        let media_match_changed = self.media_match_changed(&requested_inputs);
        let mut timings = PipelineTimings::default();
        if let Some(previous) = self.cache.inputs.as_ref() {
            timings.change_cause = requested_inputs.change_mask(previous);
        }
        if media_match_changed {
            timings.change_cause.style_environment_changed = true;
        }

        let requested_stage = self.normalized_earliest_stage(&requested_inputs, media_match_changed);
        println!("HTML_PIPELINE_UPDATE phase=begin stage={requested_stage:?}");
        let layout_constraints = to_layout_constraints(requested_inputs.layout);
        let parsed_key = requested_inputs.parsed_cache_key();
        let prepared_key = requested_inputs.prepared_cache_key();
        let shaped_key = requested_inputs.shaped_cache_key();
        let layout_key = requested_inputs.layout_cache_key();
        let render_only_key = requested_inputs.render_only_cache_key();

        let mut report = ReuseReport::default();
        let mut stage_runs = PipelineStageCounts::default();
        let mut stage_reuses = PipelineStageCounts::default();
        let mut rebuilt_media_queries = None;

        match requested_stage {
            EarliestStage::Parse => {
                let parsed = self.parse(&requested_inputs.source, requested_inputs.markup_syntax)?;
                let user_styles = requested_inputs.user_styles.iter().map(String::as_str).collect::<Vec<_>>();
                let (prepared, media_queries) = self.factory.build_pipeline_from_parsed_with_media_queries(&parsed, &user_styles);
                let (shaped, laid_out) = self.shape_and_layout(&prepared, layout_constraints, &requested_inputs.image_metrics, glyph_shaper)?;
                rebuilt_media_queries = Some(media_queries);

                self.cache.parsed = Some(parsed.clone());
                self.cache.parsed_key = Some(parsed_key.clone());
                self.cache.prepared = Some(prepared);
                self.cache.prepared_key = Some(prepared_key.clone());
                self.cache.shaped = Some(shaped);
                self.cache.shaped_key = Some(shaped_key.clone());
                self.cache.laid_out = Some(laid_out);
                self.cache.laid_out_key = Some(layout_key.clone());

                stage_runs.parsed += 1;
                stage_runs.styled += 1;
                stage_runs.prepared += 1;
                stage_runs.shaped += 1;
                stage_runs.laid_out += 1;

                report.parsed = Reuse::recomputed(EarliestStage::Parse);
                report.styled = Reuse::recomputed(EarliestStage::Parse);
                report.prepared = Reuse::recomputed(EarliestStage::Parse);
                report.shaped = Reuse::recomputed(EarliestStage::Parse);
                report.laid_out = Reuse::recomputed(EarliestStage::Parse);
            }
            EarliestStage::Style | EarliestStage::Prepare => {
                let parsed = self.parse_cached(&parsed_key, &requested_inputs.source)?;
                let user_styles = requested_inputs.user_styles.iter().map(String::as_str).collect::<Vec<_>>();
                let (prepared, media_queries) = self.factory.build_pipeline_from_parsed_with_media_queries(&parsed, &user_styles);

                let (shaped, laid_out) = self.shape_and_layout(&prepared, layout_constraints, &requested_inputs.image_metrics, glyph_shaper)?;
                rebuilt_media_queries = Some(media_queries);

                self.cache.parsed = Some(parsed);
                self.cache.parsed_key = Some(parsed_key.clone());
                self.cache.prepared = Some(prepared);
                self.cache.prepared_key = Some(prepared_key.clone());
                self.cache.shaped = Some(shaped);
                self.cache.shaped_key = Some(shaped_key.clone());
                self.cache.laid_out = Some(laid_out);
                self.cache.laid_out_key = Some(layout_key.clone());

                stage_reuses.parsed += 1;
                stage_runs.styled += 1;
                stage_runs.prepared += 1;
                stage_runs.shaped += 1;
                stage_runs.laid_out += 1;

                report.parsed = Reuse::reused(EarliestStage::Style);
                report.styled = Reuse::recomputed(EarliestStage::Style);
                report.prepared = Reuse::recomputed(EarliestStage::Style);
                report.shaped = Reuse::recomputed(EarliestStage::Style);
                report.laid_out = Reuse::recomputed(EarliestStage::Style);
            }
            EarliestStage::Shape => {
                let parsed = self.parse_cached(&parsed_key, &requested_inputs.source)?;
                let prepared_reused = self.cache.prepared_matches(&prepared_key);
                let (prepared, media_queries) = self.prepare_from_cache_or_rebuild(&parsed, &prepared_key, &requested_inputs.user_styles);
                let (shaped, laid_out) = self.shape_and_layout(&prepared, layout_constraints, &requested_inputs.image_metrics, glyph_shaper)?;
                rebuilt_media_queries = media_queries;

                self.cache.parsed = Some(parsed);
                self.cache.parsed_key = Some(parsed_key.clone());
                self.cache.prepared = Some(prepared);
                self.cache.prepared_key = Some(prepared_key.clone());
                self.cache.shaped = Some(shaped);
                self.cache.shaped_key = Some(shaped_key.clone());
                self.cache.laid_out = Some(laid_out);
                self.cache.laid_out_key = Some(layout_key.clone());

                stage_reuses.parsed += 1;
                stage_reuses.styled += 1;
                if prepared_reused {
                    stage_reuses.prepared += 1;
                } else {
                    stage_runs.prepared += 1;
                }
                stage_runs.shaped += 1;
                stage_runs.laid_out += 1;

                report.parsed = Reuse::reused(EarliestStage::Shape);
                report.styled = Reuse::reused(EarliestStage::Shape);
                report.prepared = if prepared_reused { Reuse::reused(EarliestStage::Shape) } else { Reuse::recomputed(EarliestStage::Shape) };
                report.shaped = Reuse::recomputed(EarliestStage::Shape);
                report.laid_out = Reuse::recomputed(EarliestStage::Shape);
            }
            EarliestStage::Layout => {
                let parsed = self.parse_cached(&parsed_key, &requested_inputs.source)?;
                let prepared_reused = self.cache.prepared_matches(&prepared_key);
                let (prepared, media_queries) = self.prepare_from_cache_or_rebuild(&parsed, &prepared_key, &requested_inputs.user_styles);
                let shaped_reused = self.cache.shaped_matches(&shaped_key);
                let shaped = self.shape_from_cache_or_rebuild(&prepared, &shaped_key, glyph_shaper)?;
                self.relayout_laid_out(layout_constraints, &requested_inputs.image_metrics, &shaped_key, &layout_key, &shaped, glyph_shaper)?;
                rebuilt_media_queries = media_queries;

                self.cache.parsed = Some(parsed);
                self.cache.parsed_key = Some(parsed_key.clone());
                self.cache.prepared = Some(prepared);
                self.cache.prepared_key = Some(prepared_key.clone());
                self.cache.shaped = Some(shaped);
                self.cache.shaped_key = Some(shaped_key.clone());

                stage_reuses.parsed += 1;
                stage_reuses.styled += 1;
                if prepared_reused {
                    stage_reuses.prepared += 1;
                } else {
                    stage_runs.prepared += 1;
                }
                if shaped_reused {
                    stage_reuses.shaped += 1;
                } else {
                    stage_runs.shaped += 1;
                }
                stage_runs.laid_out += 1;

                report.parsed = Reuse::reused(EarliestStage::Layout);
                report.styled = Reuse::reused(EarliestStage::Layout);
                report.prepared = if prepared_reused { Reuse::reused(EarliestStage::Layout) } else { Reuse::recomputed(EarliestStage::Layout) };
                report.shaped = if shaped_reused { Reuse::reused(EarliestStage::Layout) } else { Reuse::recomputed(EarliestStage::Layout) };
                report.laid_out = Reuse::recomputed(EarliestStage::Layout);
            }
            EarliestStage::Paint => {
                debug_assert_eq!(self.cache.laid_out_key.as_ref(), Some(&render_only_key.layout));
                stage_reuses.parsed += 1;
                stage_reuses.styled += 1;
                stage_reuses.prepared += 1;
                stage_reuses.shaped += 1;
                stage_reuses.laid_out += 1;

                report.parsed = Reuse::reused(EarliestStage::Paint);
                report.styled = Reuse::reused(EarliestStage::Paint);
                report.prepared = Reuse::reused(EarliestStage::Paint);
                report.shaped = Reuse::reused(EarliestStage::Paint);
                report.laid_out = Reuse::reused(EarliestStage::Paint);
            }
            EarliestStage::None => {
                stage_reuses.parsed += 1;
                stage_reuses.styled += 1;
                stage_reuses.prepared += 1;
                stage_reuses.shaped += 1;
                stage_reuses.laid_out += 1;

                report.parsed = Reuse::reused(EarliestStage::None);
                report.styled = Reuse::reused(EarliestStage::None);
                report.prepared = Reuse::reused(EarliestStage::None);
                report.shaped = Reuse::reused(EarliestStage::None);
                report.laid_out = Reuse::reused(EarliestStage::None);
            }
        }

        if let Some(media_queries) = rebuilt_media_queries {
            self.cache.media_queries = Some(media_queries);
        }
        self.cache.media_match_key = self.cache.media_queries.as_ref().map(|queries| queries.match_key(requested_inputs.style_environment.media, f64::from(requested_inputs.style_environment.root_font_size)));

        self.validate_cache_against_requested(&parsed_key, &prepared_key, &shaped_key, &layout_key, &render_only_key);
        self.cache.inputs = Some(requested_inputs);
        if !matches!(requested_stage, EarliestStage::Paint | EarliestStage::None) {
            self.cache.retained_bytes = self.retained_bytes();
        }

        timings.decision_time = decision_started.elapsed();
        timings.stage_runs = stage_runs;
        timings.stage_reuses = stage_reuses;
        timings.retained_bytes = self.cache.retained_bytes;
        println!(
            "HTML_PIPELINE_UPDATE phase=complete stage={requested_stage:?} total_ms={} decision_ms={} runs={stage_runs:?} reuses={stage_reuses:?} retained_bytes={}",
            update_started.elapsed().as_millis(),
            timings.decision_time.as_millis(),
            timings.retained_bytes.total(),
        );
        Ok(PipelineUpdate { stage: requested_stage, timings, report, anchors_preserved: !matches!(requested_stage, EarliestStage::Parse | EarliestStage::Style | EarliestStage::Prepare | EarliestStage::Shape) })
    }

    /// Rebuild the shaped and laid-out stages for a cached document.
    ///
    /// Glyph shapers may keep renderer-specific data indexed by `GlyphId`. That
    /// data is reset whenever another document is shaped, so restoring a cached
    /// document must rehydrate it even when its parse and style inputs are still
    /// valid.
    pub fn rehydrate_glyphs(&mut self, glyph_shaper: &mut impl LayoutGlyphShaper) -> Result<LaidOutDocument, PipelineError> {
        let inputs = self.cache.inputs.clone().ok_or_else(|| PipelineError("cannot rehydrate an empty pipeline session".to_owned()))?;
        let prepared = self.cache.prepared.clone().ok_or_else(|| PipelineError("cannot rehydrate a pipeline session without a prepared document".to_owned()))?;
        let (shaped, laid_out) = self.shape_and_layout(&prepared, to_layout_constraints(inputs.layout), &inputs.image_metrics, glyph_shaper)?;

        self.cache.shaped = Some(shaped);
        self.cache.shaped_key = Some(inputs.shaped_cache_key());
        self.cache.laid_out = Some(laid_out.clone());
        self.cache.laid_out_key = Some(inputs.layout_cache_key());
        self.cache.retained_bytes = self.retained_bytes();
        Ok(laid_out)
    }

    fn normalized_earliest_stage(&self, requested_inputs: &PipelineInputs, media_match_changed: bool) -> EarliestStage {
        let input_stage = self.cache.inputs.as_ref().map(|previous| requested_inputs.earliest_stage(previous)).unwrap_or(EarliestStage::Parse);
        let base_stage = input_stage.coalesce(if media_match_changed { EarliestStage::Style } else { EarliestStage::None });
        let parsed_key = requested_inputs.parsed_cache_key();
        let prepared_key = requested_inputs.prepared_cache_key();
        let shaped_key = requested_inputs.shaped_cache_key();
        let layout_key = requested_inputs.layout_cache_key();

        match base_stage {
            EarliestStage::Parse => EarliestStage::Parse,
            EarliestStage::Style | EarliestStage::Prepare => {
                if !self.cache.parsed_matches(&parsed_key) {
                    EarliestStage::Parse
                } else if !self.cache.prepared_matches(&prepared_key) {
                    EarliestStage::Prepare
                } else {
                    base_stage
                }
            }
            EarliestStage::Shape => {
                if !self.cache.parsed_matches(&parsed_key) {
                    EarliestStage::Parse
                } else if !self.cache.prepared_matches(&prepared_key) {
                    EarliestStage::Prepare
                } else if self.cache.shaped_matches(&shaped_key) {
                    EarliestStage::Shape
                } else {
                    EarliestStage::Shape
                }
            }
            EarliestStage::Layout => {
                if !self.cache.parsed_matches(&parsed_key) {
                    EarliestStage::Parse
                } else if !self.cache.prepared_matches(&prepared_key) {
                    EarliestStage::Prepare
                } else if !self.cache.shaped_matches(&shaped_key) {
                    EarliestStage::Shape
                } else if self.cache.laid_out_matches(&layout_key) {
                    EarliestStage::Layout
                } else {
                    EarliestStage::Layout
                }
            }
            EarliestStage::Paint => {
                if self.cache.laid_out_matches(&layout_key) {
                    EarliestStage::Paint
                } else {
                    EarliestStage::Layout
                }
            }
            EarliestStage::None => {
                if self.cache.laid_out_matches(&layout_key) {
                    EarliestStage::Paint
                } else {
                    EarliestStage::Layout
                }
            }
        }
    }

    fn parse_cached(&mut self, key: &ParsedCacheKey, source: &str) -> Result<Arc<ParsedHtml>, PipelineError> {
        if self.cache.parsed_matches(key) {
            return Ok(self.cache.parsed.clone().expect("parsed html should exist when parsed key matches"));
        }
        let parsed = self.parse(source, key.markup_syntax)?;
        self.cache.parsed = Some(parsed.clone());
        self.cache.parsed_key = Some(key.clone());
        Ok(parsed)
    }

    fn refresh_resource_cache(&mut self, resource_revision: ResourceRevision, stylesheet_revision: StylesheetRevision) {
        let cache_key = (resource_revision, stylesheet_revision);
        if self.cached_resource_key.as_ref() != Some(&cache_key) {
            self.cached_provider.clear();
            self.cached_resource_key = Some(cache_key);
        }
    }

    fn retained_bytes(&self) -> PipelineRetainedBytes {
        PipelineRetainedBytes {
            parsed: self.cache.inputs.as_ref().map_or(0, |inputs| inputs.source.len()),
            styled: self.cache.prepared.as_ref().map_or(0, |prepared| prepared.memory_usage_bytes()),
            shaped: self.cache.shaped.as_ref().map_or(0, |shaped| shaped.memory_usage_bytes()),
            laid_out: self.cache.laid_out.as_ref().map_or(0, |laid_out| laid_out.memory_usage_bytes()),
        }
    }

    fn parse(&mut self, source: &str, syntax: MarkupSyntax) -> Result<Arc<ParsedHtml>, PipelineError> {
        let started = Instant::now();
        let result = parse_document(source, syntax).map(Arc::new).map_err(|error| PipelineError(error.to_string()));
        println!("HTML_PIPELINE_PARSE bytes={} duration_ms={}", source.len(), started.elapsed().as_millis());
        result
    }

    fn media_match_changed(&self, requested_inputs: &PipelineInputs) -> bool {
        let Some(previous_inputs) = self.cache.inputs.as_ref() else { return false };
        if previous_inputs.style_environment.media == requested_inputs.style_environment.media {
            return false;
        }
        let (Some(media_queries), Some(previous_key)) = (self.cache.media_queries.as_ref(), self.cache.media_match_key.as_ref()) else {
            return true;
        };
        if media_queries.uses_viewport_units() {
            return true;
        }
        media_queries.match_key(requested_inputs.style_environment.media, f64::from(requested_inputs.style_environment.root_font_size)) != *previous_key
    }

    fn prepare_from_cache_or_rebuild(&mut self, parsed: &Arc<ParsedHtml>, requested_prepared_key: &PreparedCacheKey, user_styles: &[String]) -> (PreparedDocument, Option<MediaQuerySet>) {
        if self.cache.prepared_matches(requested_prepared_key) {
            debug_assert_eq!(self.cache.prepared_key.as_ref(), Some(requested_prepared_key));
            return (self.cache.prepared.clone().expect("prepared document should exist when prepared key matches"), None);
        }
        let user_styles = user_styles.iter().map(String::as_str).collect::<Vec<_>>();
        let (prepared, media_queries) = self.factory.build_pipeline_from_parsed_with_media_queries(parsed.as_ref(), &user_styles);
        (prepared, Some(media_queries))
    }

    fn shape_from_cache_or_rebuild(&mut self, prepared: &PreparedDocument, requested_shaped_key: &ShapedCacheKey, glyph_shaper: &mut impl LayoutGlyphShaper) -> Result<ShapedDocument, PipelineError> {
        if self.cache.shaped_matches(requested_shaped_key) {
            debug_assert_eq!(self.cache.shaped_key.as_ref(), Some(requested_shaped_key));
            return self.cache.shaped.clone().ok_or_else(|| PipelineError("cached shaped document missing".to_owned()));
        }
        // Reshaping the page replaces the shaper's resources, so anything notes
        // appended to the old ones is gone with them.
        self.notes_shaped = None;
        self.shape(prepared, glyph_shaper)
    }

    fn relayout_laid_out(
        &mut self, constraints: LayoutConstraintsOutput, image_metrics: &html_layout::ImageMetrics, requested_shaped_key: &ShapedCacheKey, requested_layout_key: &LayoutCacheKey, shaped: &ShapedDocument,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<(), PipelineError> {
        let can_relayout = self.cache.laid_out.is_some() && self.cache.shaped_matches(requested_shaped_key);
        match self.cache.laid_out.as_mut() {
            Some(laid_out) if can_relayout => {
                debug_assert_eq!(self.cache.shaped_key.as_ref(), Some(requested_shaped_key));
                laid_out.relayout_with_metrics_and_shaper(constraints, image_metrics, glyph_shaper).map_err(|error| PipelineError(error.to_string()))?;
                self.cache.laid_out_key = Some(requested_layout_key.clone());
            }
            _ => {
                let laid_out = self.layout_exact(shaped.clone(), constraints, image_metrics, glyph_shaper)?;
                self.cache.laid_out = Some(laid_out);
                self.cache.laid_out_key = Some(requested_layout_key.clone());
            }
        }
        Ok(())
    }

    fn layout_exact(&self, shaped: ShapedDocument, constraints: LayoutConstraintsOutput, image_metrics: &html_layout::ImageMetrics, glyph_shaper: &mut impl LayoutGlyphShaper) -> Result<LaidOutDocument, PipelineError> {
        shaped.layout_with_metrics_and_shaper(constraints, image_metrics, glyph_shaper).map_err(|error| PipelineError(error.to_string()))
    }

    fn shape(&mut self, prepared: &PreparedDocument, glyph_shaper: &mut impl LayoutGlyphShaper) -> Result<ShapedDocument, PipelineError> {
        prepared.shape(glyph_shaper).map_err(|error| PipelineError(error.to_string()))
    }

    fn shape_and_layout(
        &self, prepared: &PreparedDocument, constraints: LayoutConstraintsOutput, image_metrics: &html_layout::ImageMetrics, glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<(ShapedDocument, LaidOutDocument), PipelineError> {
        prepared.shape_and_layout_with_metrics_and_shaper(constraints, image_metrics, glyph_shaper).map_err(|error| PipelineError(error.to_string()))
    }

    fn validate_cache_against_requested(&self, parsed_key: &ParsedCacheKey, prepared_key: &PreparedCacheKey, shaped_key: &ShapedCacheKey, layout_key: &LayoutCacheKey, render_only_key: &RenderOnlyCacheKey) {
        if self.cache.inputs.is_none() {
            return;
        }
        debug_assert!(self.cache.inputs.is_some());

        if self.cache.parsed_matches(parsed_key) {
            debug_assert!(self.cache.parsed.is_some());
        }
        if self.cache.prepared_matches(prepared_key) {
            debug_assert!(self.cache.prepared.is_some());
        }
        if self.cache.shaped_matches(shaped_key) {
            debug_assert!(self.cache.shaped.is_some());
        }
        if self.cache.laid_out_matches(layout_key) {
            debug_assert!(self.cache.laid_out.is_some());
            debug_assert_eq!(self.cache.laid_out_key.as_ref(), Some(layout_key));
        }
        if self.cache.laid_out.is_some() {
            debug_assert_eq!(self.cache.laid_out_key.as_ref(), Some(&render_only_key.layout));
        }
    }

    /// Lays out one note body under constraints of the caller's choosing, for
    /// embedders that present notes outside the reading flow.
    ///
    /// This session's parse and computed styles are reused, so the cost is a
    /// box tree, shaping and layout over a single subtree rather than a second
    /// document. The note lays out whatever [`PipelineInputs::note_flow`] asked
    /// for the containing document: holding notes back from the flow is what
    /// creates the need for this call.
    ///
    /// Returns `None` before the first successful update, or when `id` names no
    /// element.
    pub fn layout_note(&mut self, id: &str, constraints: LayoutConstraintsOutput, glyph_shaper: &mut impl LayoutGlyphShaper) -> Option<LaidOutDocument> {
        let inputs = self.cache.inputs.as_ref()?;
        let scoped = self.cache.prepared.as_ref()?.scoped_to_element_id(id)?;
        // Appended to the page's renderer resources rather than shaped as a
        // document of its own: a note's glyphs have to coexist with the page's,
        // which are still referenced by what is on screen. Every note appends,
        // so the seed is whatever the last one left behind.
        let base = self.notes_shaped.as_ref().or(self.cache.shaped.as_ref())?;
        let (shaped, laid_out) = scoped.shape_and_layout_into_active_resources(base, constraints, &inputs.image_metrics, glyph_shaper).ok()?;
        self.notes_shaped = Some(shaped);
        Some(laid_out)
    }

    /// Ids of the note bodies in the current document, in document order.
    /// Empty before the first successful update.
    pub fn note_ids(&self) -> Vec<&str> {
        self.cache.prepared.as_ref().map(PreparedDocument::note_ids).unwrap_or_default()
    }

    pub fn document(&self) -> Option<&LaidOutDocument> {
        self.cache.laid_out.as_ref()
    }

    pub fn document_mut(&mut self) -> Option<&mut LaidOutDocument> {
        self.cache.laid_out.as_mut()
    }
}

/// Caches immutable resource reads for the resource and stylesheet revisions
/// currently being rendered. `PipelineSession` clears this cache before either
/// revision changes, so providers may safely replace resources between updates.
struct RevisionCachedResourceProvider {
    inner: Arc<dyn ResourceProvider>,
    bytes: Mutex<HashMap<String, Vec<u8>>>,
    strings: Mutex<HashMap<String, String>>,
}

impl RevisionCachedResourceProvider {
    fn new(inner: Arc<dyn ResourceProvider>) -> Self {
        Self { inner, bytes: Mutex::new(HashMap::new()), strings: Mutex::new(HashMap::new()) }
    }

    fn clear(&self) {
        self.bytes.lock().expect("resource byte cache lock").clear();
        self.strings.lock().expect("resource string cache lock").clear();
    }
}

impl ResourceProvider for RevisionCachedResourceProvider {
    fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
        if let Some(bytes) = self.bytes.lock().expect("resource byte cache lock").get(uri).cloned() {
            return Ok(bytes);
        }
        let bytes = self.inner.read_bytes(uri)?;
        self.bytes.lock().expect("resource byte cache lock").insert(uri.to_owned(), bytes.clone());
        Ok(bytes)
    }

    fn read_string(&self, uri: &str) -> io::Result<String> {
        if let Some(string) = self.strings.lock().expect("resource string cache lock").get(uri).cloned() {
            return Ok(string);
        }
        let string = self.inner.read_string(uri)?;
        self.strings.lock().expect("resource string cache lock").insert(uri.to_owned(), string.clone());
        Ok(string)
    }

    fn metadata(&self, uri: &str) -> io::Result<html_resources::ResourceMetadata> {
        self.inner.metadata(uri)
    }

    fn exists(&self, uri: &str) -> bool {
        self.inner.exists(uri)
    }

    fn resolve(&self, base: &str, href: &str) -> String {
        self.inner.resolve(base, href)
    }

    fn list_html_candidates(&self, root: &str) -> io::Result<Vec<String>> {
        self.inner.list_html_candidates(root)
    }

    fn toc(&self) -> io::Result<Option<Vec<html_resources::TocEntry>>> {
        self.inner.toc()
    }
}

fn to_root_font_size(root_font_size: u32) -> RootFontSize {
    RootFontSize::new(root_font_size as f32).unwrap_or_default()
}

fn to_layout_constraints(requested: LayoutConstraints) -> LayoutConstraintsOutput {
    LayoutConstraintsOutput::new(requested.viewport_width, requested.line_height)
        .expect("requested constraints must be valid")
        .with_viewport_height(requested.viewport_height)
        .expect("requested viewport height must be valid")
        .with_image_sizing_policy(requested.image_sizing_policy)
        .with_text_composition_policy(requested.text_composition_policy)
}

#[cfg(test)]
mod tests {
    use super::{PipelineSession, to_layout_constraints};
    use crate::{
        EarliestStage, FontEnvironmentRevision, FontEnvironmentRevision as FontEnv, ImageMetricsRevision, LayoutConstraints, MarkupSyntax, MediaEnvironment, PaintSettingsRevision, PipelineChange, PipelineError, PipelineInputs,
        ResourceRevision, Reuse, SourceRevision, StyleEnvironment, StylesheetRevision,
    };
    use html_layout::{GlyphId, GlyphMetric, GlyphShaper as LayoutGlyphShaper, LaidOutDocument};
    use html_resources::ResourceProvider;
    use std::{
        collections::HashMap,
        io,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    struct MockProvider;

    impl ResourceProvider for MockProvider {
        fn read_bytes(&self, _uri: &str) -> io::Result<Vec<u8>> {
            Err(io::Error::new(io::ErrorKind::NotFound, "no resources in mock provider"))
        }

        fn exists(&self, _uri: &str) -> bool {
            false
        }

        fn resolve(&self, _base: &str, href: &str) -> String {
            href.to_owned()
        }

        fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    #[derive(Clone, Default)]
    struct MutableProvider {
        resources: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        reads: Arc<Mutex<usize>>,
    }

    impl MutableProvider {
        fn put_bytes(&self, path: &str, bytes: Vec<u8>) {
            self.resources.lock().expect("resource lock").insert(path.to_owned(), bytes);
        }

        fn read_count(&self) -> usize {
            *self.reads.lock().expect("read count lock")
        }
    }

    impl ResourceProvider for MutableProvider {
        fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
            *self.reads.lock().expect("read count lock") += 1;
            let resources = self.resources.lock().expect("resource lock");
            resources.get(uri).cloned().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, uri.to_owned()))
        }

        fn exists(&self, uri: &str) -> bool {
            self.resources.lock().expect("resource lock").contains_key(uri)
        }

        fn resolve(&self, _base: &str, href: &str) -> String {
            href.to_owned()
        }

        fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    #[derive(Clone, Default)]
    struct TracingShaper {
        calls: usize,
        cache: HashMap<(char, u32), GlyphId>,
    }

    impl LayoutGlyphShaper for TracingShaper {
        fn reset(&mut self) {
            self.cache.clear();
        }

        fn shape_glyph<'a>(
            &mut self, glyph_metrics: &mut html_layout::GlyphRegistry<'a>, ch: char, font_size: f32, _font_weight: u16, _font_slant: html_layout::FontSlant, _color: u32, _family: Option<&str>,
        ) -> Result<GlyphId, html_layout::ShapeError> {
            let _ = glyph_metrics;
            self.calls += 1;
            let key = (ch, font_size.to_bits());
            if let Some(&glyph) = self.cache.get(&key) {
                return Ok(glyph);
            }

            let idx = u32::try_from(self.cache.len()).expect("glyph ids must fit");
            let glyph = idx;
            let metric = GlyphMetric::try_new(ch, 1.0, 1.0, 0.0, 0.0).expect("deterministic metrics must be valid");
            glyph_metrics.register(metric).expect("glyph metrics registry capacity should not be exceeded");
            self.cache.insert(key, glyph);
            Ok(glyph)
        }
    }

    #[derive(Clone, Default)]
    struct SizedShaper {
        cache: HashMap<(char, u32), GlyphId>,
    }

    impl LayoutGlyphShaper for SizedShaper {
        fn reset(&mut self) {
            self.cache.clear();
        }

        fn shape_glyph<'a>(
            &mut self, glyph_metrics: &mut html_layout::GlyphRegistry<'a>, ch: char, font_size: f32, _font_weight: u16, _font_slant: html_layout::FontSlant, _color: u32, _family: Option<&str>,
        ) -> Result<GlyphId, html_layout::ShapeError> {
            let key = (ch, font_size.to_bits());
            if let Some(&glyph) = self.cache.get(&key) {
                return Ok(glyph);
            }

            let idx = u32::try_from(self.cache.len()).expect("glyph ids must fit");
            let glyph = idx;
            let width = font_size.max(1.0) * 0.75;
            let metric = GlyphMetric::try_new(ch, width, font_size, font_size * 0.2, 0.0).expect("deterministic metrics must be valid");
            glyph_metrics.register(metric).expect("glyph metrics registry capacity should not be exceeded");
            self.cache.insert(key, glyph);
            Ok(glyph)
        }
    }

    #[derive(Default)]
    struct TransactionalShaper {
        cache: HashMap<(char, u32), GlyphId>,
        resources: Vec<char>,
        previous_document: Option<(HashMap<(char, u32), GlyphId>, Vec<char>)>,
        fail_on: Option<char>,
        fail_measurement: bool,
    }

    impl LayoutGlyphShaper for TransactionalShaper {
        fn reset(&mut self) {
            self.cache.clear();
            self.resources.clear();
        }

        fn begin_document_shaping(&mut self) {
            assert!(self.previous_document.is_none(), "document shaping transactions cannot be nested");
            self.previous_document = Some((std::mem::take(&mut self.cache), std::mem::take(&mut self.resources)));
        }

        fn commit_document_shaping(&mut self) {
            self.previous_document = None;
        }

        fn rollback_document_shaping(&mut self) {
            if let Some((cache, resources)) = self.previous_document.take() {
                self.cache = cache;
                self.resources = resources;
            }
        }

        fn shape_glyph<'a>(
            &mut self, glyph_metrics: &mut html_layout::GlyphRegistry<'a>, ch: char, font_size: f32, _font_weight: u16, _font_slant: html_layout::FontSlant, _color: u32, _family: Option<&str>,
        ) -> Result<GlyphId, html_layout::ShapeError> {
            if self.fail_on == Some(ch) {
                return Err(html_layout::ShapeError::unregistered_glyph_id(u32::MAX, glyph_metrics.len()));
            }
            let key = (ch, font_size.to_bits());
            if let Some(&glyph) = self.cache.get(&key) {
                return Ok(glyph);
            }
            let metric = GlyphMetric::try_new(ch, 1.0, 1.0, 0.0, 0.0).expect("test metrics must be valid");
            let glyph = glyph_metrics.register(metric)?;
            self.cache.insert(key, glyph);
            self.resources.push(ch);
            Ok(glyph)
        }

        fn measure_line(&mut self, request: html_layout::TextShapeRequest<'_>) -> Result<Option<html_layout::ShapedTextRun>, html_layout::ShapeError> {
            if self.fail_measurement {
                return Err(html_layout::ShapeError::unregistered_glyph_id(u32::MAX, request.text_range().len()));
            }
            Ok(None)
        }
    }

    fn base_inputs(html: &str, source_revision: SourceRevision) -> PipelineInputs {
        PipelineInputs {
            source: html.to_owned(),
            markup_syntax: MarkupSyntax::Html,
            user_styles: Vec::new(),
            reader_overrides: Default::default(),
            note_flow: Default::default(),
            source_revision,
            base_uri: "index.html".to_owned(),
            resource_revision: ResourceRevision::INITIAL,
            stylesheet_revision: StylesheetRevision::INITIAL,
            style_environment: StyleEnvironment::default(),
            font_environment: FontEnvironmentRevision::INITIAL,
            image_metrics_revision: ImageMetricsRevision::INITIAL,
            layout: LayoutConstraints {
                viewport_width: 600.0,
                viewport_height: None,
                line_height: StyleEnvironment::default().root_font_size as f64,
                image_sizing_policy: html_layout::ImageSizingPolicy::WebCompatible,
                text_composition_policy: html_layout::TextCompositionPolicy::WebCompatible,
            },
            image_metrics: Default::default(),
            paint: PaintSettingsRevision::INITIAL,
        }
    }

    fn glyph_text(document: &LaidOutDocument) -> String {
        let text = document.render_view().text();
        (0..text.glyph_count()).filter_map(|index| text.glyph_at(index).and_then(|glyph| text.glyph_metric(glyph)).map(|metric| metric.ch())).collect()
    }

    #[test]
    fn an_excluded_note_is_absent_from_the_flow_and_available_through_scoped_layout() {
        const SOURCE: &str = "<html><body><p>Reading</p><aside id='n' epub:type='footnote'><p>zebra</p></aside><p>Continues</p></body></html>";
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        let mut inputs = base_inputs(SOURCE, SourceRevision::INITIAL);
        inputs.note_flow = html_layout::NoteFlow::Excluded;
        session.update(inputs, &mut shaper).expect("pipeline update should succeed");

        let flow = glyph_text(session.document().expect("a document"));
        assert!(flow.contains("Reading") && flow.contains("Continues"));
        assert!(!flow.contains("zebra"), "an excluded note must not consume the reading flow");

        assert_eq!(session.note_ids(), vec!["n"]);

        // The note is still reachable: scoped layout gives an embedder the
        // content it held back, under its own width.
        let constraints = html_layout::LayoutConstraints::new(200.0, 20.0).expect("valid constraints");
        let note = session.layout_note("n", constraints, &mut shaper).expect("the held-back note must lay out on demand");
        assert!(glyph_text(&note).contains("zebra"), "scoped layout produces the note's own content");
        assert!(!glyph_text(&note).contains("Reading"), "a scoped note is only its own subtree");

        assert!(session.layout_note("absent", constraints, &mut shaper).is_none(), "an id that names nothing lays out nothing");
    }

    #[test]
    fn pipeline_selects_strict_xml_for_xhtml_uris() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        let malformed = "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>broken</body></html>";

        let mut xhtml_inputs = base_inputs(malformed, SourceRevision::INITIAL);
        xhtml_inputs.base_uri = "reference.xht".to_owned();
        xhtml_inputs.markup_syntax = MarkupSyntax::Xml;
        let error = session.update(xhtml_inputs, &mut shaper).expect_err("XHTML must not use HTML recovery");
        assert!(error.to_string().contains("XML parse error"));

        let mut html_inputs = base_inputs(malformed, SourceRevision::INITIAL.next());
        html_inputs.base_uri = "reference.html".to_owned();
        session.update(html_inputs, &mut shaper).expect("HTML continues to use html5ever recovery");
    }

    #[test]
    fn pipeline_accepts_cdata_stylesheets_in_xhtml() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        let mut inputs = base_inputs(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style><![CDATA[p { background: green; } div { background: blue; height: 5px; } div + div { background: orange; }]]></style></head><body><p>XML</p><div></div><div></div></body></html>"#,
            SourceRevision::INITIAL,
        );
        inputs.base_uri = "reference.xhtml".to_owned();
        inputs.markup_syntax = MarkupSyntax::Xml;
        session.update(inputs, &mut shaper).expect("well-formed XHTML pipeline");
        let colors = session.document().expect("the XML pipeline produced a document").render_view().fragments().decorations().iter().map(|fragment| fragment.color()).collect::<Vec<_>>();
        assert!(colors.contains(&0x008000FF), "CDATA rules must reach computed paint values");
        assert!(colors.contains(&0x0000FFFF), "the first sibling must retain the type-selector background");
        assert!(colors.contains(&0xFFA500FF), "the adjacent-sibling selector must style the second sibling; colors={colors:#x?}");
    }

    #[test]
    fn xhtml_absolute_shrink_to_fit_honors_floated_em_max_width() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = SizedShaper::default();
        let mut inputs = base_inputs(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style><![CDATA[
                body { margin: 8px; }
                div#outer { background: red; font: 30px/4 Ahem; left: auto; position: absolute; right: auto; width: auto; }
                div#inner { background: green; float: left; max-width: 4em; }
            ]]></style></head><body><p>preceding normal-flow content</p><div id="outer"><div id="inner">12345678</div></div></body></html>"#,
            SourceRevision::INITIAL,
        );
        inputs.base_uri = "absolute-width.xht".to_owned();
        inputs.markup_syntax = MarkupSyntax::Xml;
        inputs.layout.viewport_width = 800.0;
        inputs.layout.viewport_height = Some(600.0);
        session.update(inputs, &mut shaper).expect("well-formed XHTML pipeline");

        let view = session.document().expect("XHTML document").render_view();
        let size_for = |id| {
            let index = (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some(id)).expect("identified layout box");
            view.boxes().size(index).expect("box geometry")
        };
        assert_eq!(size_for("inner").width, 120.0);
        assert_eq!(size_for("outer").width, 120.0);
    }

    #[test]
    fn xhtml_root_percentage_height_is_definite_for_descendants() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        let mut inputs = base_inputs(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style><![CDATA[html, body, p { height: 100%; margin: 0 } p { background: green }]]></style></head><body id="body"><p id="child">text</p></body></html>"#,
            SourceRevision::INITIAL,
        );
        inputs.base_uri = "reference.xht".to_owned();
        inputs.markup_syntax = MarkupSyntax::Xml;
        inputs.layout.viewport_width = 300.0;
        session.update(inputs.clone(), &mut shaper).expect("initial XHTML pipeline");
        inputs.layout.viewport_height = Some(600.0);
        inputs.style_environment.media = MediaEnvironment::screen(300.0, Some(600.0)).expect("valid viewport");
        session.update(inputs, &mut shaper).expect("viewport relayout");

        let view = session.document().expect("the XML pipeline produced a document").render_view();
        let size_for = |id| {
            let index = (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some(id)).expect("identified layout box");
            view.boxes().size(index).expect("box geometry")
        };
        assert_eq!(size_for("body").height, 600.0);
        assert_eq!(size_for("child").height, 600.0);
        let green = view.fragments().decorations().iter().find(|fragment| fragment.color() == 0x008000FF).expect("green child background");
        assert_eq!(green.rect().height(), 600.0);
    }

    #[test]
    fn xhtml_absolute_text_with_top_zero_overlays_earlier_table_content() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = SizedShaper::default();
        let mut inputs = base_inputs(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style><![CDATA[span { display: table-cell !important; }]]></style></head><body style="margin:0"><div id="parent" style="position:relative;font-size:32px"><div id="flow" style="position:relative;padding:1px"><span style="display:block">a b</span><span style="display:block">c d</span></div><div id="absolute" style="position:absolute;top:0;padding:1px">a bc d</div></div></body></html>"#,
            SourceRevision::INITIAL,
        );
        inputs.base_uri = "table-overlay.xht".to_owned();
        inputs.markup_syntax = MarkupSyntax::Xml;
        inputs.layout.viewport_width = 800.0;
        inputs.layout.viewport_height = Some(600.0);
        session.update(inputs, &mut shaper).expect("well-formed XHTML pipeline");

        let lines = session.document().expect("XHTML document").render_view().text().lines().iter().map(|line| line.point()).collect::<Vec<_>>();
        let min_y = lines.iter().map(|point| point.y).fold(f64::INFINITY, f64::min);
        let max_y = lines.iter().map(|point| point.y).fold(f64::NEG_INFINITY, f64::max);
        assert!((max_y - min_y).abs() < 0.01, "top:0 XHTML text must translate to the earlier table content: {lines:?}");
    }

    #[test]
    fn xhtml_cdata_stylesheet_sizes_inline_image_by_percentage() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        let mut inputs = base_inputs(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style><![CDATA[img { width: 100%; height: 1px; vertical-align: top; }]]></style></head><body style="margin:0"><div><img id="target" src="black.png" /></div></body></html>"#,
            SourceRevision::INITIAL,
        );
        inputs.base_uri = "reference.xht".to_owned();
        inputs.markup_syntax = MarkupSyntax::Xml;
        inputs.layout.viewport_width = 500.0;
        inputs.image_metrics.set(0, 96, 96);
        session.update(inputs, &mut shaper).expect("well-formed XHTML pipeline");

        let view = session.document().expect("the XML pipeline produced a document").render_view();
        let target = (0..view.boxes().len()).find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some("target")).expect("image layout box");
        let size = view.boxes().size(target).expect("image box size");
        assert_eq!((size.width, size.height), (500.0, 1.0));
    }

    #[test]
    fn failed_document_shaping_restores_cached_document_and_renderer_resources() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let original_inputs = base_inputs("<html><body><p>original document</p></body></html>", SourceRevision::INITIAL);
        session.update(original_inputs.clone(), &mut shaper).expect("initial update");
        let original_document = build_snapshot(session.document().expect("initial document"));
        let original_resources = shaper.resources.clone();
        let original_layout_key = session.cache.laid_out_key.clone();

        shaper.fail_on = Some('!');
        let replacement = base_inputs("<html><body><p>replacement!</p></body></html>", SourceRevision::INITIAL.next());
        session.update(replacement, &mut shaper).expect_err("replacement shaping must fail");

        assert_eq!(build_snapshot(session.document().expect("cached document must remain")), original_document);
        assert_eq!(shaper.resources, original_resources, "renderer resources must roll back with the document");
        assert!(shaper.previous_document.is_none(), "the failed transaction must be closed");
        assert_eq!(session.cache.inputs.as_ref(), Some(&original_inputs));
        assert_eq!(session.cache.laid_out_key, original_layout_key);
    }

    #[test]
    fn width_only_relayout_does_not_invoke_backend_line_measurement() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let original_inputs = base_inputs("<html><body><p>text that wraps across several words</p></body></html>", SourceRevision::INITIAL);
        session.update(original_inputs.clone(), &mut shaper).expect("initial update");
        let original_document = build_snapshot(session.document().expect("initial document"));
        shaper.fail_measurement = true;
        let mut narrower = original_inputs.clone();
        narrower.layout.viewport_width = 10.0;
        let mut changed_layout = session.document().expect("initial document").clone();
        changed_layout.relayout_with_metrics(to_layout_constraints(narrower.layout), &narrower.image_metrics);
        assert_ne!(build_snapshot(&changed_layout), original_document, "the request must exercise geometry that differs from the cached layout");
        session.update(narrower.clone(), &mut shaper).expect("layout must reuse authoritative document shaping");

        assert_eq!(build_snapshot(session.document().expect("updated document")), build_snapshot(&changed_layout));
        assert_eq!(session.cache.inputs.as_ref(), Some(&narrower));
    }

    #[test]
    fn stylesheet_resource_reads_are_reused_until_a_resource_or_stylesheet_revision_changes() {
        let provider = MutableProvider::default();
        provider.put_bytes("theme.css", b"p { color: red; }".to_vec());
        let mut session = PipelineSession::new(Arc::new(provider.clone()));
        let mut shaper = TracingShaper::default();
        let html = "<html><head><link rel=\"stylesheet\" href=\"theme.css\"></head><body><p>cached stylesheet</p></body></html>";

        session.update(base_inputs(html, SourceRevision::INITIAL), &mut shaper).expect("initial update");
        let mut source_changed = base_inputs(html, SourceRevision::INITIAL.next());
        session.update(source_changed.clone(), &mut shaper).expect("source-only update");
        assert_eq!(provider.read_count(), 1, "same resource and stylesheet revisions reuse CSS bytes");

        source_changed.resource_revision = ResourceRevision::INITIAL.next();
        session.update(source_changed, &mut shaper).expect("resource revision update");
        assert_eq!(provider.read_count(), 2, "resource revision invalidates CSS bytes");
    }

    fn build_snapshot(document: &LaidOutDocument) -> RenderSnapshot {
        let root = document.render_view();
        let text = root.text();
        let fragments = root.fragments();
        let addressing = root.addressing();
        let mut glyphs = Vec::with_capacity(text.glyph_count());
        for idx in 0..text.glyph_count() {
            glyphs.push(text.glyph_at(idx).unwrap_or(0));
        }
        let mut lines = Vec::with_capacity(text.line_count());
        for line_idx in 0..text.line_count() {
            let line = text.line(line_idx).expect("line should exist");
            lines.push(LineSnapshot {
                start: line.start(),
                end: line.end(),
                point_x_bits: line.point().x.to_bits(),
                point_y_bits: line.point().y.to_bits(),
                height_bits: line.height().to_bits(),
                baseline_bits: line.baseline().to_bits(),
                word_spacing_bits: line.word_spacing().to_bits(),
                letter_spacing_bits: line.letter_spacing().to_bits(),
            });
        }
        RenderSnapshot {
            glyphs,
            glyph_advances: (0..text.line_count())
                .map(|line_idx| {
                    let Some(runs) = text.line_glyph_advances(line_idx) else { return Vec::new() };
                    runs.iter()
                        .map(|run| {
                            let range = run.range();
                            (range.start, range.end, run.advance().to_bits())
                        })
                        .collect()
                })
                .collect(),
            line_links: (0..text.glyph_count() as u32).map(|glyph_idx| addressing.link_for_glyph(glyph_idx)).collect(),
            lines,
            decorations: fragments
                .decorations()
                .iter()
                .map(|decoration| {
                    let rect = decoration.rect();
                    DecorationSnapshot { x0_bits: rect.x0.to_bits(), y0_bits: rect.y0.to_bits(), x1_bits: rect.x1.to_bits(), y1_bits: rect.y1.to_bits(), color: decoration.color(), is_inline: decoration.is_inline() }
                })
                .collect(),
            images: fragments
                .images()
                .iter()
                .map(|image| {
                    let clip = image.clip();
                    ImageSnapshot {
                        line_idx: image.line_idx(),
                        image_idx: image.image_idx(),
                        offset_x_bits: image.offset().x.to_bits(),
                        offset_y_bits: image.offset().y.to_bits(),
                        width_bits: image.size().width.to_bits(),
                        height_bits: image.size().height.to_bits(),
                        clip_x0_bits: clip.x0.to_bits(),
                        clip_y0_bits: clip.y0.to_bits(),
                        clip_x1_bits: clip.x1.to_bits(),
                        clip_y1_bits: clip.y1.to_bits(),
                    }
                })
                .collect(),
            anchor_glyphs: {
                let mut anchors: Vec<(u16, u32)> = addressing.anchor_glyphs().collect();
                anchors.sort_by_key(|(id_idx, _)| *id_idx);
                anchors
            },
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct LineSnapshot {
        start: u32,
        end: u32,
        point_x_bits: u64,
        point_y_bits: u64,
        height_bits: u64,
        baseline_bits: u64,
        word_spacing_bits: u64,
        letter_spacing_bits: u64,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct DecorationSnapshot {
        x0_bits: u64,
        y0_bits: u64,
        x1_bits: u64,
        y1_bits: u64,
        color: u32,
        is_inline: bool,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ImageSnapshot {
        line_idx: usize,
        image_idx: u32,
        offset_x_bits: u64,
        offset_y_bits: u64,
        width_bits: u64,
        height_bits: u64,
        clip_x0_bits: u64,
        clip_y0_bits: u64,
        clip_x1_bits: u64,
        clip_y1_bits: u64,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RenderSnapshot {
        glyphs: Vec<GlyphId>,
        glyph_advances: Vec<Vec<(u32, u32, u64)>>,
        line_links: Vec<Option<u16>>,
        lines: Vec<LineSnapshot>,
        decorations: Vec<DecorationSnapshot>,
        images: Vec<ImageSnapshot>,
        anchor_glyphs: Vec<(u16, u32)>,
    }

    fn update_and_snapshot(updates: &[PipelineInputs]) -> Result<RenderSnapshot, PipelineError> {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        for input in updates {
            session.update(input.clone(), &mut shaper)?;
        }
        let document = session.document().expect("document should exist");
        Ok(build_snapshot(document))
    }

    #[test]
    fn width_change_triggers_layout_only() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));

        let mut baseline = base_inputs("<html><body><p>Hello world</p></body></html>", SourceRevision::INITIAL);
        let update = session.update(baseline.clone(), &mut shaper).unwrap();
        assert_eq!(update.stage, EarliestStage::Parse);
        assert_eq!(update.report.parsed, Reuse::recomputed(EarliestStage::Parse));

        let shape_calls = shaper.calls;
        baseline.layout.viewport_width = 420.0;
        let layout_only = session.update(baseline.clone(), &mut shaper).unwrap();
        assert_eq!(layout_only.stage, EarliestStage::Layout);
        assert_eq!(layout_only.report.parsed, Reuse::reused(EarliestStage::Layout));
        assert_eq!(layout_only.report.styled, Reuse::reused(EarliestStage::Layout));
        assert_eq!(layout_only.report.prepared, Reuse::reused(EarliestStage::Layout));
        assert_eq!(layout_only.report.shaped, Reuse::reused(EarliestStage::Layout));
        assert_eq!(layout_only.report.laid_out, Reuse::recomputed(EarliestStage::Layout));
        assert_eq!(shaper.calls, shape_calls);
    }

    #[test]
    fn font_environment_change_reuses_parse_style_and_prepare() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));

        let mut update_inputs = base_inputs("<html><body><p>Glyphs: abcdef</p></body></html>", SourceRevision::INITIAL);
        let _ = session.update(update_inputs.clone(), &mut shaper).unwrap();
        let before_calls = shaper.calls;

        update_inputs.font_environment = FontEnv::INITIAL.next();
        let font_update = session.update(update_inputs, &mut shaper).unwrap();
        assert_eq!(font_update.stage, EarliestStage::Shape);
        assert_eq!(font_update.report.parsed, Reuse::reused(EarliestStage::Shape));
        assert_eq!(font_update.report.styled, Reuse::reused(EarliestStage::Shape));
        assert_eq!(font_update.report.prepared, Reuse::reused(EarliestStage::Shape));
        assert_eq!(font_update.report.shaped, Reuse::recomputed(EarliestStage::Shape));
        assert_eq!(font_update.report.laid_out, Reuse::recomputed(EarliestStage::Shape));
        assert!(shaper.calls > before_calls);
    }

    #[test]
    fn font_backend_generation_change_is_shape_stage_and_matches_clean_rebuild() {
        let source = "<html><body><p>Glyph backend swap changes shaping.</p></body></html>";
        let baseline_inputs = {
            let mut inputs = base_inputs(source, SourceRevision::INITIAL);
            inputs.font_environment = FontEnvironmentRevision::INITIAL;
            inputs
        };
        let changed_inputs = {
            let mut inputs = baseline_inputs.clone();
            inputs.font_environment = FontEnvironmentRevision::next(baseline_inputs.font_environment);
            inputs
        };

        let mut shaper = SizedShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let _ = session.update(baseline_inputs.clone(), &mut shaper).unwrap();

        let update = session.update(changed_inputs.clone(), &mut shaper).unwrap();
        assert_eq!(update.stage, EarliestStage::Shape);
        assert_eq!(update.report.styled, Reuse::reused(EarliestStage::Shape));
        assert_eq!(update.report.prepared, Reuse::reused(EarliestStage::Shape));
        assert_eq!(update.report.shaped, Reuse::recomputed(EarliestStage::Shape));
        assert_eq!(update.report.laid_out, Reuse::recomputed(EarliestStage::Shape));
        let incremental = build_snapshot(session.document().expect("document should exist after backend-generation update"));
        let mut fresh_shaper = SizedShaper::default();
        let mut fresh_session = PipelineSession::new(Arc::new(MockProvider));
        let _ = fresh_session.update(changed_inputs.clone(), &mut fresh_shaper).unwrap();
        let fresh = build_snapshot(fresh_session.document().expect("document should exist after full rebuild"));

        assert_eq!(incremental, fresh);
    }

    #[test]
    fn incremental_and_fresh_rebuilds_match_for_width_then_font_then_width() {
        let source = "<html><body><p>Hello world</p><ul><li>one</li><li>two</li><li>three</li></ul></body></html>";
        let mut updates = vec![
            {
                let mut base = base_inputs(source, SourceRevision::INITIAL);
                base.layout.viewport_width = 580.0;
                base
            },
            {
                let mut font_up = base_inputs(source, SourceRevision::INITIAL);
                font_up.layout.viewport_width = 580.0;
                font_up.style_environment.root_font_size = 20;
                font_up.layout.line_height = 20.0;
                font_up
            },
            {
                let mut final_input = base_inputs(source, SourceRevision::INITIAL);
                final_input.style_environment.root_font_size = 20;
                final_input.layout = LayoutConstraints {
                    viewport_width: 430.0,
                    viewport_height: None,
                    line_height: 20.0,
                    image_sizing_policy: html_layout::ImageSizingPolicy::WebCompatible,
                    text_composition_policy: html_layout::TextCompositionPolicy::WebCompatible,
                };
                final_input
            },
        ];

        let incremental = update_and_snapshot(&updates).expect("incremental pipeline should succeed");
        let fresh = {
            let mut shaper = TracingShaper::default();
            let mut session = PipelineSession::new(Arc::new(MockProvider));
            let mut inputs = updates.pop().expect("final input must exist");
            inputs.source_revision = SourceRevision::INITIAL;
            session.update(inputs, &mut shaper).unwrap();
            build_snapshot(session.document().expect("document should exist"))
        };

        assert_eq!(incremental, fresh);
    }

    #[test]
    fn width_changes_can_be_repeated_without_triggering_reparse_or_reshaping() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));

        let input = base_inputs("<html><body><p>layout stability test</p></body></html>", SourceRevision::INITIAL);
        let update = session.update(input.clone(), &mut shaper).unwrap();
        assert_eq!(update.stage, EarliestStage::Parse);

        let after_parse_calls = shaper.calls;
        let mut next = input.clone();
        next.layout.viewport_width = 450.0;
        let width_update_1 = session.update(next.clone(), &mut shaper).unwrap();
        assert_eq!(width_update_1.stage, EarliestStage::Layout);
        assert_eq!(shaper.calls, after_parse_calls);

        let no_change = session.update(next.clone(), &mut shaper).unwrap();
        assert_eq!(no_change.stage, EarliestStage::Paint);
        assert_eq!(shaper.calls, after_parse_calls);

        next.layout.viewport_width = 640.0;
        let width_update_2 = session.update(next.clone(), &mut shaper).unwrap();
        assert_eq!(width_update_2.stage, EarliestStage::Layout);
        assert_eq!(shaper.calls, after_parse_calls);
    }

    #[test]
    fn column_width_only_restyles_when_a_media_rule_changes_activation() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let source = "<html><head><style>@media (min-width: 500px) { p { font-size: 32px; } }</style></head><body><p>responsive text</p></body></html>";

        let mut inputs = base_inputs(source, SourceRevision::INITIAL);
        inputs.layout.viewport_width = 400.0;
        inputs.style_environment.media = MediaEnvironment::screen(400.0, Some(800.0)).unwrap();
        session.update(inputs.clone(), &mut shaper).unwrap();
        let initial_shape_calls = shaper.calls;

        inputs.layout.viewport_width = 450.0;
        inputs.style_environment.media = MediaEnvironment::screen(450.0, Some(800.0)).unwrap();
        let below_breakpoint = session.update(inputs.clone(), &mut shaper).unwrap();
        assert_eq!(below_breakpoint.stage, EarliestStage::Layout);
        assert_eq!(below_breakpoint.report.styled, Reuse::reused(EarliestStage::Layout));
        assert_eq!(below_breakpoint.report.prepared, Reuse::reused(EarliestStage::Layout));
        assert_eq!(shaper.calls, initial_shape_calls);

        inputs.layout.viewport_width = 600.0;
        inputs.style_environment.media = MediaEnvironment::screen(600.0, Some(800.0)).unwrap();
        let crossing_breakpoint = session.update(inputs.clone(), &mut shaper).unwrap();
        assert_eq!(crossing_breakpoint.stage, EarliestStage::Style);
        assert_eq!(crossing_breakpoint.report.styled, Reuse::recomputed(EarliestStage::Style));
        assert_eq!(crossing_breakpoint.report.prepared, Reuse::recomputed(EarliestStage::Style));
        assert!(shaper.calls > initial_shape_calls);
        let after_crossing_calls = shaper.calls;

        inputs.layout.viewport_width = 650.0;
        inputs.style_environment.media = MediaEnvironment::screen(650.0, Some(800.0)).unwrap();
        let above_breakpoint = session.update(inputs, &mut shaper).unwrap();
        assert_eq!(above_breakpoint.stage, EarliestStage::Layout);
        assert_eq!(above_breakpoint.report.styled, Reuse::reused(EarliestStage::Layout));
        assert_eq!(shaper.calls, after_crossing_calls);
    }

    #[test]
    fn viewport_unit_dimensions_restyle_and_reshape_without_a_media_breakpoint() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let source = "<html><head><style>p { font-size: 10vw; line-height: 10vh }</style></head><body><p>viewport text</p></body></html>";

        let mut inputs = base_inputs(source, SourceRevision::INITIAL);
        inputs.layout.viewport_width = 400.0;
        inputs.layout.viewport_height = Some(800.0);
        inputs.style_environment.media = MediaEnvironment::screen(400.0, Some(800.0)).unwrap();
        session.update(inputs.clone(), &mut shaper).unwrap();
        let initial_shape_calls = shaper.calls;

        inputs.layout.viewport_width = 450.0;
        inputs.style_environment.media = MediaEnvironment::screen(450.0, Some(800.0)).unwrap();
        let update = session.update(inputs, &mut shaper).unwrap();

        assert_eq!(update.stage, EarliestStage::Style);
        assert_eq!(update.report.styled, Reuse::recomputed(EarliestStage::Style));
        assert!(shaper.calls > initial_shape_calls);
    }

    #[test]
    fn simultaneous_style_and_layout_changes_choose_style_stage() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));

        let mut inputs = base_inputs("<html><body><p>one</p></body></html>", SourceRevision::INITIAL);
        let _ = session.update(inputs.clone(), &mut shaper).unwrap();

        inputs.stylesheet_revision = StylesheetRevision::next(inputs.stylesheet_revision);
        inputs.layout.viewport_width = 420.0;
        let update = session.update(inputs, &mut shaper).unwrap();
        assert_eq!(update.stage, EarliestStage::Prepare);
        assert_eq!(update.report.styled, Reuse::recomputed(EarliestStage::Style));
        assert_eq!(update.report.prepared, Reuse::recomputed(EarliestStage::Style));
    }

    #[test]
    fn reader_user_styles_rebuild_prepared_document_without_reparsing() {
        let mut shaper = SizedShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut inputs = base_inputs("<html><body><p>Text that wraps differently when enlarged.</p></body></html>", SourceRevision::INITIAL);
        session.update(inputs.clone(), &mut shaper).unwrap();
        let baseline = build_snapshot(session.document().unwrap());

        inputs.user_styles = vec!["p { font-size: 42px !important; }".to_owned()];
        let update = session.update(inputs, &mut shaper).unwrap();
        let styled = build_snapshot(session.document().unwrap());

        assert_eq!(update.stage, EarliestStage::Prepare);
        assert_eq!(update.report.parsed, Reuse::reused(EarliestStage::Style));
        assert_ne!(baseline, styled);
    }

    #[test]
    fn typed_reader_overrides_apply_without_injecting_css() {
        let mut shaper = SizedShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut inputs = base_inputs("<html><body><p style='font-size: 9px; line-height: 1'>Text that wraps after reader overrides.</p></body></html>", SourceRevision::INITIAL);
        session.update(inputs.clone(), &mut shaper).unwrap();
        let baseline = build_snapshot(session.document().unwrap());

        inputs.reader_overrides = crate::ReaderStyleOverrides {
            font_family: Some("serif".to_owned()),
            text_align: Some(crate::TextAlign::Justify),
            line_height: Some(1.8),
            minimum_font_size: Some(28.0),
            foreground: Some(0x112233ff),
            background: Some(0xfefefeff),
        };
        assert!(inputs.user_styles.is_empty());
        let update = session.update(inputs, &mut shaper).unwrap();
        let overridden = build_snapshot(session.document().unwrap());

        assert_eq!(update.stage, EarliestStage::Prepare);
        assert_ne!(baseline, overridden);
    }

    #[test]
    fn stylesheet_replacement_and_restoration_maps_to_style_stage_and_preserves_fresh_rebuild_equivalence() {
        let mut shaper = SizedShaper::default();
        let provider = MutableProvider::default();
        provider.put_bytes("style.css", b"p { font-size: 9px; }".to_vec());

        let source = r#"<html>
            <head>
                <link rel="stylesheet" href="style.css" />
            </head>
            <body><p>Long text used to prove wrapping changes across style revisions.</p></body>
            </html>"#;
        let mut update_inputs = base_inputs(source, SourceRevision::INITIAL);
        update_inputs.stylesheet_revision = StylesheetRevision::INITIAL;

        let mut session = PipelineSession::new(Arc::new(provider.clone()));
        session.update(update_inputs.clone(), &mut shaper).expect("baseline update should succeed");
        let baseline = {
            let document = session.document().expect("document should exist");
            build_snapshot(document)
        };

        provider.put_bytes("style.css", b"p { font-size: 36px; }".to_vec());
        update_inputs.stylesheet_revision = StylesheetRevision::next(update_inputs.stylesheet_revision);
        let style_update = session.update(update_inputs.clone(), &mut shaper).expect("style replacement update should succeed");
        assert_eq!(style_update.stage, EarliestStage::Prepare);
        assert_eq!(style_update.report.styled, Reuse::recomputed(EarliestStage::Style));
        let replaced = {
            let document = session.document().expect("document should exist");
            build_snapshot(document)
        };
        assert_ne!(baseline, replaced);

        provider.put_bytes("style.css", b"p { font-size: 9px; }".to_vec());
        update_inputs.stylesheet_revision = StylesheetRevision::next(update_inputs.stylesheet_revision);
        let restore = session.update(update_inputs, &mut shaper).expect("stylesheet restoration update should succeed");
        assert_eq!(restore.stage, EarliestStage::Prepare);
        let restored = {
            let document = session.document().expect("document should exist");
            build_snapshot(document)
        };
        assert_eq!(restore.report.parsed, Reuse::reused(EarliestStage::Style));
        assert_eq!(baseline, restored);
    }

    #[test]
    fn pipeline_changes_map_to_expected_earliest_stage() {
        let previous = base_inputs("<html><body><p>x</p></body></html>", SourceRevision::INITIAL);
        let mut next = previous.clone();
        next.source = "<html><body><p>y</p></body></html>".to_owned();
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Parse);

        let mut next = previous.clone();
        next.stylesheet_revision = StylesheetRevision::next(previous.stylesheet_revision);
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Style);

        let mut next = previous.clone();
        next.font_environment = FontEnv::next(previous.font_environment);
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Shape);

        let mut next = previous.clone();
        next.layout.viewport_width = 500.0;
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Layout);

        let mut next = previous.clone();
        next.layout.line_height = 20.0;
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Layout);

        let mut next = previous.clone();
        next.layout.image_sizing_policy = html_layout::ImageSizingPolicy::SmartStandalone;
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Layout);

        let mut next = previous.clone();
        next.layout.text_composition_policy = html_layout::TextCompositionPolicy::BookOptimized;
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Layout);

        let mut next = previous.clone();
        next.image_metrics_revision = ImageMetricsRevision::next(previous.image_metrics_revision);
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Layout);

        let mut next = previous.clone();
        next.paint = PaintSettingsRevision::next(previous.paint);
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Paint);

        let changes = [
            (PipelineChange::SourceChanged(SourceRevision::INITIAL), EarliestStage::Parse),
            (PipelineChange::ResourceBaseChanged(ResourceRevision::INITIAL), EarliestStage::Parse),
            (PipelineChange::StylesheetsChanged(StylesheetRevision::INITIAL), EarliestStage::Style),
            (PipelineChange::StyleEnvironmentChanged(StyleEnvironment::default()), EarliestStage::Style),
            (PipelineChange::FontEnvironmentChanged(FontEnv::INITIAL), EarliestStage::Shape),
            (PipelineChange::ImageMetricsChanged(ImageMetricsRevision::INITIAL), EarliestStage::Layout),
            (
                PipelineChange::LayoutConstraintsChanged(LayoutConstraints {
                    viewport_width: 600.0,
                    viewport_height: None,
                    line_height: 20.0,
                    image_sizing_policy: html_layout::ImageSizingPolicy::WebCompatible,
                    text_composition_policy: html_layout::TextCompositionPolicy::WebCompatible,
                }),
                EarliestStage::Layout,
            ),
            (PipelineChange::PaintSettingsChanged(PaintSettingsRevision::INITIAL), EarliestStage::Paint),
        ];
        for (change, expected) in changes {
            assert_eq!(change.invalidated_stage(), expected);
        }
    }

    #[test]
    fn pipeline_input_equality_includes_source_and_base_uri() {
        let inputs = base_inputs("<html><body><p>x</p></body></html>", SourceRevision::INITIAL);
        assert_eq!(inputs, inputs.clone());

        let mut different_source = inputs.clone();
        different_source.source = "<html><body><p>y</p></body></html>".to_owned();
        assert_ne!(different_source, inputs, "source text must remain part of input identity even when its revision is unchanged");

        let mut different_base = inputs.clone();
        different_base.base_uri = "other/chapter.xhtml".to_owned();
        assert_ne!(different_base, inputs, "resource resolution base must remain part of input identity even when its revision is unchanged");
    }

    #[test]
    fn xhtml_absolute_inline_background_reaches_positioned_paint_output() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        let mut inputs = base_inputs(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style><![CDATA[div.a { width:300px; height:100px } div.b { width:300px; position:relative } div.b p { width:200% } div.b img { width:50%; height:100px } div.b span { position:absolute; top:0; left:0; background:green; width:300px; height:100px }]]></style></head><body><div class="a"></div><div class="b"><p><img src="red.png"/><span id="cover"></span></p></div></body></html>"#,
            SourceRevision::INITIAL,
        );
        inputs.base_uri = "absolute-inline.xhtml".to_owned();
        inputs.markup_syntax = MarkupSyntax::Xml;
        session.update(inputs, &mut shaper).expect("well-formed XHTML pipeline");

        let document = session.document().expect("laid out document");
        let green = document.render_view().fragments().decorations().iter().find(|fragment| fragment.color() == 0x008000FF).expect("absolute green background");
        assert!(green.is_in_positioned_layer());
        assert!(!green.is_in_negative_positioned_layer());
        assert!(!green.is_in_independent_positioned_layer(), "auto-z absolute and relative boxes share the source-ordered positioned stream");
        assert!(green.line_idx().is_none());
        let root = document.render_view();
        let images = root.fragments().images().iter().collect::<Vec<_>>();
        assert!(!images.is_empty(), "the regression fixture must produce a replaced-image fragment");
        for image in images {
            let line = root.text().line(image.line_idx()).expect("image owning line");
            assert!(line.is_in_positioned_layer(), "the relative ancestor keeps its normal descendants in positioned paint");
            assert!(!line.is_in_independent_positioned_layer(), "the preceding image remains in the common source-ordered positioned stream");
        }
    }
}
