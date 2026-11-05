#[cfg(test)]
use html_dom as dom_values;
#[cfg(test)]
use html_engine_model::layout_state;
#[cfg(test)]
use html_engine_model::shaping_contracts;
use crate::stages::search_queries;
#[cfg(test)]
use html_style_model::computed_style;
use html_style_model::reader_style;
use std::collections::HashMap;
use std::io;
use std::mem;
use std::sync::{Arc, Mutex, Weak};

use crate::engine::{
    DetachedLayout, GlyphResourceGeneration, GlyphResourceStore, GlyphShaper as LayoutGlyphShaper,
    ImageMetrics, LaidOutDocument, NoteLayout, PreparedDocument, RenderView,
};
use crate::layout::InlinePlanGeneration;
use html_dom::RootFontSize;
use html_engine_model::shaping::resource_generation_mismatch;
use html_parse::parse_document;
use html_resources::ResourceProvider;
use html_style::{FontFace, MediaMatchKey, MediaQuerySet};

use crate::parser::{DocumentFactory, ResourceContext};
use crate::types::{
    LayoutCacheKey, ParsedCacheKey, PreparedCacheKey, ShapedCacheKey,
};
use crate::types::{LayoutConstraints, ResourceRevision, StylesheetRevision};
use crate::{
    EarliestStage, MediaEnvironment, PipelineChanges, PipelineError, PipelineErrorKind,
    PipelineInputs,
};
use crate::{GlyphResourceCheckpoint, NativeResourceCheckpoint};

struct ScopeResources {
    owner_shape: InlinePlanGeneration,
    native: NativeResourceCheckpoint,
    end: NativeResourceCheckpoint,
    metrics: GlyphResourceCheckpoint,
    owners: Vec<Weak<()>>,
}

#[derive(Default)]
struct PipelineCacheState {
    inputs: Option<PipelineInputs>,
    /// Parse validity and source encoding; the syntax tree is consumed into the DOM.
    parsed: Option<CacheEntry<ParsedCacheKey, Option<String>>>,
    document: DocumentOwner,
    /// Query definitions and their activation key belong to one preparation.
    media: Option<CacheEntry<MediaMatchKey, MediaQuerySet>>,
    /// Active declarations shared with the compiled style program.
    font_faces: Arc<[FontFace]>,
    /// Shaping validity and its backend generation are published together;
    /// glyph data has exactly one document owner.
    shaped: Option<CacheEntry<ShapedCacheKey, GlyphResourceGeneration>>,
}

/// Preparation and published layout share one document owner. A worker result
/// remains prepared until shaping succeeds; publication replaces that variant.
#[derive(Default)]
enum DocumentOwner {
    #[default]
    Empty,
    Prepared(CacheEntry<PreparedCacheKey, PreparedDocument<'static>>),
    Published(CacheEntry<LayoutCacheKey, LaidOutDocument<'static>>),
}

impl DocumentOwner {
    fn prepared_key(&self) -> Option<&PreparedCacheKey> {
        match self {
            Self::Empty => None,
            Self::Prepared(entry) => Some(&entry.key),
            Self::Published(entry) => Some(&entry.key.shaped.prepared),
        }
    }

    fn prepared(&self) -> Option<&PreparedDocument<'static>> {
        match self {
            Self::Prepared(entry) => Some(&entry.value),
            _ => None,
        }
    }

    fn prepared_mut(&mut self) -> Option<&mut PreparedDocument<'static>> {
        match self {
            Self::Prepared(entry) => Some(&mut entry.value),
            _ => None,
        }
    }

    fn published(&self) -> Option<&CacheEntry<LayoutCacheKey, LaidOutDocument<'static>>> {
        match self {
            Self::Published(entry) => Some(entry),
            _ => None,
        }
    }

    fn published_mut(
        &mut self,
    ) -> Option<&mut CacheEntry<LayoutCacheKey, LaidOutDocument<'static>>> {
        match self {
            Self::Published(entry) => Some(entry),
            _ => None,
        }
    }

    fn publish_prepared(&mut self, key: LayoutCacheKey, layout: DetachedLayout) {
        let Self::Prepared(entry) = mem::take(self) else {
            unreachable!("publication follows successful layout of prepared ownership");
        };
        *self = Self::Published(CacheEntry {
            key,
            value: entry.value.with_layout(layout),
        });
    }
}

/// Only the comparisons needed while proposed inputs are being evaluated.
/// Large input values remain owned in one place throughout an update.
#[derive(Clone, Copy)]
struct PreviousInputs {
    source_matches: bool,
    media: MediaEnvironment,
    color_preferences_changed: bool,
    same_styles: bool,
    same_overrides: bool,
    inherited_image_metrics: bool,
}

struct CacheEntry<K, V> {
    key: K,
    value: V,
}

fn cache_matches<K: PartialEq, V>(entry: Option<&CacheEntry<K, V>>, key: &K) -> bool {
    entry.is_some_and(|entry| &entry.key == key)
}

impl PipelineCacheState {
    fn renderer_generation(&self) -> Option<GlyphResourceGeneration> {
        self.shaped.as_ref().map(|entry| entry.value)
    }

    fn publish_layout_key(&mut self, key: LayoutCacheKey, generation: GlyphResourceGeneration) {
        self.shaped = Some(CacheEntry {
            key: key.shaped.clone(),
            value: generation,
        });
        self.document
            .published_mut()
            .expect("published document owner")
            .key = key;
    }
}

pub struct PipelineSession {
    scope_resources: Option<ScopeResources>,
    glyph_resources: GlyphResourceStore,
    cached_provider: Arc<RevisionCachedResourceProvider>,
    cached_resource_key: Option<(ResourceRevision, StylesheetRevision)>,
    factory: DocumentFactory,
    cache: PipelineCacheState,
}

/// Renderer-bound half of a pipeline session. The underlying
/// [`PipelineSession`] can be prepared on a worker before a backend is
/// attached; this view keeps the backend and its glyph ledger together for
/// every shaping, relayout, and note operation.
pub struct ActivePipelineSession<'a, S: LayoutGlyphShaper> {
    session: &'a mut PipelineSession,
    glyph_shaper: &'a mut S,
}

impl<S: LayoutGlyphShaper> ActivePipelineSession<'_, S> {
    pub fn update(&mut self, inputs: PipelineInputs) -> Result<EarliestStage, PipelineError> {
        self.session.update(inputs, self.glyph_shaper)
    }

    pub fn apply_changes(
        &mut self,
        changes: PipelineChanges,
    ) -> Result<EarliestStage, PipelineError> {
        self.session.apply_changes(changes, self.glyph_shaper)
    }

    pub fn rehydrate_glyphs(&mut self) -> Result<(), PipelineError> {
        self.session.rehydrate_glyphs(self.glyph_shaper)
    }

    pub fn layout_note(
        &mut self,
        id: &str,
        constraints: LayoutConstraints,
    ) -> Result<Option<NoteLayout>, PipelineError> {
        self.session.layout_note(id, constraints, self.glyph_shaper)
    }
}

impl PipelineSession {
    pub fn glyph_resources(&self) -> &GlyphResourceStore {
        &self.glyph_resources
    }
    pub fn render_view(&self) -> Option<RenderView<'_>> {
        self.document()?.try_render_view(&self.glyph_resources)
    }

    /// Shared `@font-face` snapshot from the last successful preparation or
    /// update. URLs use the declaring stylesheet's resource-provider base.
    /// After `prepare_through_style`, hosts can load these faces before calling
    /// `rehydrate_glyphs`. The engine never reads font bytes.
    pub fn font_faces(&self) -> Arc<[FontFace]> {
        self.cache.font_faces.clone()
    }

    pub fn attach<'a, S: LayoutGlyphShaper>(
        &'a mut self,
        glyph_shaper: &'a mut S,
    ) -> ActivePipelineSession<'a, S> {
        self.invalidate_stale_renderer_state(glyph_shaper);
        ActivePipelineSession {
            session: self,
            glyph_shaper,
        }
    }

    fn invalidate_stale_renderer_state(&mut self, glyph_shaper: &mut impl LayoutGlyphShaper) {
        if self.cache.renderer_generation() != Some(glyph_shaper.resource_generation()) {
            self.glyph_resources.invalidate_paint_resources();
            self.cache.shaped = None;
            // Published geometry remains available to borrowed readers after
            // failure or backend replacement. The generation gate prevents
            // treating it as reusable renderer state until shaping succeeds.
        }
    }

    pub fn new(provider: Arc<dyn ResourceProvider>) -> Self {
        let cached_provider = Arc::new(RevisionCachedResourceProvider::new(provider.clone()));
        Self {
            scope_resources: None,
            glyph_resources: GlyphResourceStore::default(),
            cached_provider,
            cached_resource_key: None,
            factory: DocumentFactory::new(),
            cache: PipelineCacheState::default(),
        }
    }

    /// Runs the provider, parse, style, and document-preparation stages without
    /// touching a platform glyph shaper.
    ///
    /// This is the transferable half of initial document construction. A host
    /// can run it on a native worker or Web Worker, move the session back to its
    /// UI thread, and call [`Self::rehydrate_glyphs`] with its own text system.
    pub fn prepare_through_style(
        &mut self,
        requested_inputs: PipelineInputs,
    ) -> Result<(), PipelineError> {
        self.configure_factory(&requested_inputs);
        let prepared_key = requested_inputs.prepared_cache_key();
        let parsed_key = prepared_key.parsed.clone();
        let (prepared, encoding, media_queries) = self.prepare_document(&requested_inputs)?;
        let media_match_key = media_queries.match_key(
            requested_inputs.style_environment.media,
            f64::from(requested_inputs.style_environment.root_font_size.get()),
        );

        self.glyph_resources.invalidate_render_scenes();
        self.cache.font_faces = self.factory.take_font_faces();
        self.cache.inputs = Some(requested_inputs);
        self.cache.parsed = Some(CacheEntry {
            key: parsed_key,
            value: encoding,
        });
        self.cache.document = DocumentOwner::Prepared(CacheEntry {
            key: prepared_key,
            value: prepared,
        });
        self.cache.media = Some(CacheEntry {
            key: media_match_key,
            value: media_queries,
        });
        self.cache.shaped = None;
        self.scope_resources = None;
        Ok(())
    }

    fn prepare_document(
        &mut self,
        requested_inputs: &PipelineInputs,
    ) -> Result<(PreparedDocument<'static>, Option<String>, MediaQuerySet), PipelineError> {
        let parsed = parse_document(&requested_inputs.source, requested_inputs.markup_syntax)
            .map_err(PipelineError::from)?;
        let encoding = parsed.source_encoding().map(str::to_owned);
        let (prepared, media_queries) = self
            .factory
            .with_resources(ResourceContext::new(
                self.cached_provider.as_ref(),
                if requested_inputs.base_uri.is_empty() {
                    "index.html"
                } else {
                    &requested_inputs.base_uri
                },
            ))
            .try_build_pipeline_from_owned_parsed(parsed, &requested_inputs.user_styles)?;
        let prepared = prepared
            .into_reader_paint_overrides(&requested_inputs.reader_overrides)
            .map_err(PipelineError::from)?;
        Ok((prepared, encoding, media_queries))
    }

    fn configure_factory(&mut self, requested_inputs: &PipelineInputs) {
        self.refresh_resource_cache(
            requested_inputs.resource_revision,
            requested_inputs.stylesheet_revision,
        );
        self.factory
            .set_root_font_size(requested_inputs.style_environment.root_font_size);
        self.factory
            .set_media_environment(requested_inputs.style_environment.media);
        self.factory
            .set_reader_overrides(reader_style::layout_overrides(
                &requested_inputs.reader_overrides,
            ));
        self.factory.set_note_flow(requested_inputs.note_flow);
    }

    /// Borrow the last committed inputs. The session is their only owner.
    pub fn inputs(&self) -> Option<&PipelineInputs> {
        self.cache.inputs.as_ref()
    }

    /// Accept natural-size metadata without layout only when the published
    /// resolved sizing proves no geometry depends on the changed dimensions.
    /// This operation cannot change source, style, or backend resources.
    pub fn accept_image_metrics_if_layout_unchanged(
        &mut self,
        metrics: &crate::engine::ImageMetrics,
    ) -> bool {
        let (DocumentOwner::Published(entry), Some(inputs)) =
            (&mut self.cache.document, &mut self.cache.inputs)
        else {
            return false;
        };
        if entry.value.image_metrics_affect_layout(metrics) {
            return false;
        }
        entry.value.accept_image_metrics(metrics);
        entry.key.image_metrics_generation = metrics.cache_generation();
        inputs.image_metrics = metrics.clone();
        self.glyph_resources.invalidate_render_scenes();
        true
    }

    pub fn update(
        &mut self,
        mut requested_inputs: PipelineInputs,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<EarliestStage, PipelineError> {
        self.collect_scope_resources(glyph_shaper);
        let previous_inputs = self.cache.inputs.take();
        let previous = previous_inputs.as_ref().map(|inputs| PreviousInputs {
            inherited_image_metrics: inputs.image_metrics.cache_generation()
                == requested_inputs.image_metrics.cache_generation(),
            same_styles: inputs.user_styles == requested_inputs.user_styles,
            same_overrides: reader_style::same_layout_preferences(
                &inputs.reader_overrides,
                &requested_inputs.reader_overrides,
            ),
            source_matches: inputs.same_source(&requested_inputs),
            media: inputs.style_environment.media,
            color_preferences_changed: !reader_style::same_paint_preferences(
                &inputs.reader_overrides,
                &requested_inputs.reader_overrides,
            ),
        });
        let result = self.update_inner(&mut requested_inputs, previous, glyph_shaper);
        self.cache.inputs = if result.is_ok() {
            Some(requested_inputs)
        } else {
            previous_inputs
        };
        self.finish_update(result, glyph_shaper)
    }

    /// Apply only changed fields, borrowing all unchanged document inputs.
    /// On failure, the proposed values are swapped back out before returning.
    pub fn apply_changes(
        &mut self,
        mut changes: PipelineChanges,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<EarliestStage, PipelineError> {
        self.collect_scope_resources(glyph_shaper);
        let mut inputs = self
            .cache
            .inputs
            .take()
            .ok_or_else(|| PipelineError::from_kind(PipelineErrorKind::EmptySession))?;
        let previous = PreviousInputs {
            inherited_image_metrics: changes.image_metrics.is_none(),
            same_styles: changes
                .user_styles
                .as_ref()
                .is_none_or(|styles| *styles == inputs.user_styles),
            same_overrides: changes.reader_overrides.as_ref().is_none_or(|overrides| {
                reader_style::same_layout_preferences(&overrides, &inputs.reader_overrides)
            }),
            source_matches: changes
                .source
                .as_ref()
                .is_none_or(|source| source == &inputs.source),
            media: inputs.style_environment.media,
            color_preferences_changed: changes.reader_overrides.as_ref().is_some_and(|overrides| {
                !reader_style::same_paint_preferences(&overrides, &inputs.reader_overrides)
            }),
        };
        let previous_revisions = inputs.revisions();
        let revisions = changes.advance_revisions(&inputs);
        changes.swap_with(&mut inputs);
        inputs.set_revisions(revisions);
        let result = self.update_inner(&mut inputs, Some(previous), glyph_shaper);
        if result.is_err() {
            changes.swap_with(&mut inputs);
            inputs.set_revisions(previous_revisions);
        }
        self.cache.inputs = Some(inputs);
        self.finish_update(result, glyph_shaper)
    }

    fn finish_update(
        &mut self,
        result: Result<EarliestStage, PipelineError>,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<EarliestStage, PipelineError> {
        self.invalidate_stale_renderer_state(glyph_shaper);
        if let Ok(stage) = &result {
            if *stage != EarliestStage::None {
                self.glyph_resources.invalidate_render_scenes();
            }
            if *stage <= EarliestStage::Layout
                && let Some(resources) = &self.scope_resources
                && Some(resources.owner_shape)
                    != self.document().map(LaidOutDocument::shaping_generation)
            {
                // A new owner layout may reuse handles first created by a scope.
                // Its resource prefix must survive even after that scope closes.
                self.scope_resources = None;
            }
        }
        result
    }

    fn update_inner(
        &mut self,
        requested_inputs: &mut PipelineInputs,
        previous: Option<PreviousInputs>,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<EarliestStage, PipelineError> {
        self.configure_factory(requested_inputs);
        let constraints = requested_inputs.layout;
        let layout_key = &mut requested_inputs.layout_key_reusing(
            self.cache.document.prepared_key(),
            previous.is_some_and(|previous| previous.same_styles),
            previous.is_some_and(|previous| previous.same_overrides),
        );
        let shaped_key = &layout_key.shaped;
        let prepared_key = &shaped_key.prepared;
        let parsed_key = &prepared_key.parsed;
        let color_preferences_changed =
            previous.is_some_and(|previous| previous.color_preferences_changed);
        let source_matches = previous.is_some_and(|previous| previous.source_matches);
        let media_match_changed =
            self.media_match_changed(requested_inputs, previous.map(|inputs| inputs.media));
        let can_repaint = color_preferences_changed
            && previous.is_some_and(|previous| {
                cache_matches(self.cache.parsed.as_ref(), parsed_key)
                    && previous.source_matches
                    && cache_matches(self.cache.document.published(), layout_key)
                    && !media_match_changed
                    && self.cache.renderer_generation() == Some(glyph_shaper.resource_generation())
            });
        if can_repaint {
            self.cache
                .document
                .published_mut()
                .expect("paint update has layout")
                .value
                .repaint_with_reader_overrides(
                    &requested_inputs.reader_overrides,
                    &self.glyph_resources,
                )
                .map_err(PipelineError::from)?;
            // Repainting requires an equal layout key, so stage validity is
            // already current. Keep the existing keys and their shared payloads.
            let stage = EarliestStage::Paint;
            return Ok(stage);
        }
        let resource_generation = glyph_shaper.resource_generation();
        let active_resources_match = self.cache.renderer_generation() == Some(resource_generation);
        let metrics_only = !color_preferences_changed
            && self.cache.document.published().is_some_and(|entry| {
                entry.key.layout == layout_key.layout
                    && entry.key.image_metrics_revision == layout_key.image_metrics_revision
                    && entry.key.image_metrics_generation != layout_key.image_metrics_generation
                    && !entry
                        .value
                        .image_metrics_affect_layout(&requested_inputs.image_metrics)
            });
        // Source text is compared with cached inputs so cache keys stay small
        // even when callers replace text without advancing its revision.
        let stage = if !source_matches || !cache_matches(self.cache.parsed.as_ref(), parsed_key) {
            EarliestStage::Parse
        } else if self.cache.document.prepared_key() != Some(prepared_key) {
            EarliestStage::Prepare
        } else if media_match_changed {
            EarliestStage::Style
        } else if !active_resources_match || !cache_matches(self.cache.shaped.as_ref(), shaped_key)
        {
            EarliestStage::Shape
        } else if !metrics_only && !cache_matches(self.cache.document.published(), layout_key) {
            EarliestStage::Layout
        } else if previous.is_some_and(|previous| !previous.color_preferences_changed) {
            EarliestStage::None
        } else {
            EarliestStage::Paint
        };

        if metrics_only && stage >= EarliestStage::Paint {
            self.cache
                .document
                .published_mut()
                .expect("published image metrics")
                .value
                .accept_image_metrics(&requested_inputs.image_metrics);
            self.cache
                .publish_layout_key(layout_key.clone(), resource_generation);
        }

        if stage <= EarliestStage::Prepare
            && cache_matches(self.cache.parsed.as_ref(), parsed_key)
            && source_matches
        {
            return self.restyle_cached(
                requested_inputs,
                layout_key,
                constraints,
                stage,
                glyph_shaper,
            );
        }

        if stage <= EarliestStage::Layout {
            let parsed_reused = stage != EarliestStage::Parse;
            let prepared_reused = stage > EarliestStage::Prepare;
            let shaped_reused = stage == EarliestStage::Layout;
            let mut queries = None;
            if shaped_reused || (prepared_reused && self.cache.document.prepared().is_none()) {
                let layout = &mut self
                    .cache
                    .document
                    .published_mut()
                    .expect("cached document owner")
                    .value;
                let result = if color_preferences_changed {
                    layout.update_with_reader_overrides(
                        &requested_inputs.reader_overrides,
                        constraints,
                        &requested_inputs.image_metrics,
                        !shaped_reused,
                        glyph_shaper,
                        &mut self.glyph_resources,
                    )
                } else if shaped_reused {
                    layout.relayout_with_metrics_and_shaper(
                        constraints,
                        &requested_inputs.image_metrics,
                        glyph_shaper,
                        &mut self.glyph_resources,
                    )
                } else {
                    layout.reshape_and_layout_with_metrics_and_shaper(
                        constraints,
                        &requested_inputs.image_metrics,
                        glyph_shaper,
                        &mut self.glyph_resources,
                    )
                };
                result.map_err(PipelineError::from)?;
            } else {
                let fresh = if prepared_reused {
                    None
                } else {
                    let (prepared, encoding, media) = self.prepare_document(requested_inputs)?;
                    queries = Some(media);
                    Some((prepared, encoding))
                };
                let prepared = fresh
                    .as_ref()
                    .map(|(prepared, _)| prepared)
                    .or_else(|| self.cache.document.prepared())
                    .expect("preparation has a document owner");
                let painted = if prepared_reused && color_preferences_changed {
                    Some(
                        prepared
                            .with_reader_paint_overrides(&requested_inputs.reader_overrides)
                            .map_err(PipelineError::from)?,
                    )
                } else {
                    None
                };
                let metrics = if !parsed_reused
                    && previous.is_some_and(|previous| previous.inherited_image_metrics)
                {
                    self.replacement_image_metrics(prepared, requested_inputs)
                } else {
                    requested_inputs.image_metrics.clone()
                };
                let laid_out = Self::shape_and_layout(
                    painted.as_ref().unwrap_or(prepared),
                    constraints,
                    &metrics,
                    glyph_shaper,
                    &mut self.glyph_resources,
                )?;
                drop(painted);
                layout_key.image_metrics_generation = metrics.cache_generation();
                requested_inputs.image_metrics = metrics;
                // Keep borrowed preparation in place on every failure. Only
                // successful layout moves it into the published owner.
                if let Some((prepared, encoding)) = fresh {
                    self.cache.document = DocumentOwner::Published(CacheEntry {
                        key: layout_key.clone(),
                        value: prepared.with_layout(laid_out),
                    });
                    if !parsed_reused {
                        self.cache.parsed = Some(CacheEntry {
                            key: parsed_key.clone(),
                            value: encoding,
                        });
                        self.cache.font_faces = self.factory.take_font_faces();
                    }
                } else {
                    self.cache
                        .document
                        .publish_prepared(layout_key.clone(), laid_out);
                }
            }
            self.cache
                .publish_layout_key(layout_key.clone(), glyph_shaper.resource_generation());
            if let Some(queries) = queries {
                self.cache.media = Some(CacheEntry {
                    key: queries.match_key(
                        requested_inputs.style_environment.media,
                        f64::from(requested_inputs.style_environment.root_font_size.get()),
                    ),
                    value: queries,
                });
            }
        }

        // Retained definitions keep their activation key. Changed media matches
        // or root typography already select style preparation above.
        self.validate_cache_against_requested(layout_key);
        Ok(stage)
    }

    /// Numeric image IDs can be reused by replacement markup. Retain decoded
    /// dimensions only when the same slot still names the same provider source.
    fn replacement_image_metrics(
        &self,
        prepared: &PreparedDocument<'_>,
        inputs: &PipelineInputs,
    ) -> ImageMetrics {
        if inputs.image_metrics.len() == 0 {
            return inputs.image_metrics.clone();
        }
        let Some(parsed) = &self.cache.parsed else {
            return ImageMetrics::default();
        };
        if parsed.key.resource_revision != inputs.resource_revision
            || parsed.key.base_uri.as_ref() != inputs.base_uri
        {
            return ImageMetrics::default();
        }
        let previous = match &self.cache.document {
            DocumentOwner::Prepared(entry) => entry.value.document(),
            DocumentOwner::Published(entry) => entry.value.document(),
            DocumentOwner::Empty => return ImageMetrics::default(),
        };
        let mut metrics = ImageMetrics::from_document(prepared.document());
        for (index, image) in prepared.document().images().iter().enumerate() {
            if previous
                .images()
                .get(index)
                .is_some_and(|old| old.source == image.source)
                && let Some((width, height)) = inputs.image_metrics.get(index as u32)
            {
                metrics.set(index as u32, width, height);
            }
        }
        metrics
    }

    fn restyle_cached(
        &mut self,
        inputs: &PipelineInputs,
        layout_key: &LayoutCacheKey,
        constraints: LayoutConstraints,
        stage: EarliestStage,
        shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<EarliestStage, PipelineError> {
        let encoding = self
            .cache
            .parsed
            .as_ref()
            .and_then(|entry| entry.value.as_deref());
        let mut queries = None;
        let mut factory = self.factory.with_resources(ResourceContext::new(
            self.cached_provider.as_ref(),
            if inputs.base_uri.is_empty() {
                "index.html"
            } else {
                &inputs.base_uri
            },
        ));
        let resolve = |document: &html_dom::Document| {
            let (styles, media) = factory.resolve_canonical_dom(document, encoding, &inputs.user_styles);
            queries = Some(media);
            (styles, factory.shared_font_faces())
        };
        let root_font = inputs.style_environment.root_font_size;
        let result = if let Some(prepared) = self.cache.document.prepared_mut() {
            prepared
                .reprepare_and_layout(
                    root_font,
                    inputs.note_flow,
                    &inputs.reader_overrides,
                    resolve,
                    constraints,
                    &inputs.image_metrics,
                    shaper,
                    &mut self.glyph_resources,
                )
                .map(|layout| Some(layout.into_detached()))
        } else {
            self.cache
                .document
                .published_mut()
                .expect("cached DOM has document owner")
                .value
                .reprepare_and_layout(
                    root_font,
                    inputs.note_flow,
                    &inputs.reader_overrides,
                    resolve,
                    constraints,
                    &inputs.image_metrics,
                    shaper,
                    &mut self.glyph_resources,
                )
                .map(|()| None)
        };
        match result {
            Ok(layout) => {
                if let Some(layout) = layout {
                    self.cache
                        .document
                        .publish_prepared(layout_key.clone(), layout);
                }
                self.cache
                    .publish_layout_key(layout_key.clone(), shaper.resource_generation());
                self.cache.font_faces = self.factory.take_font_faces();
                self.cache.media = queries.map(|queries| CacheEntry {
                    key: queries.match_key(
                        inputs.style_environment.media,
                        f64::from(inputs.style_environment.root_font_size.get()),
                    ),
                    value: queries,
                });
                Ok(stage)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Rebuild the shaped and laid-out stages for a cached document.
    ///
    /// Glyph shapers may keep renderer-specific data indexed by `GlyphHandle`. That
    /// data is reset whenever another document is shaped, so restoring a cached
    /// document must rehydrate it even when its parse and style inputs are still
    /// valid. Publishes the result in place; borrow it through `document()`
    /// instead of retaining a second owned layout.
    pub fn rehydrate_glyphs(
        &mut self,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<(), PipelineError> {
        self.collect_scope_resources(glyph_shaper);
        let result = (|| {
            let inputs = self
                .cache
                .inputs
                .as_ref()
                .ok_or_else(|| PipelineError::from_kind(PipelineErrorKind::EmptySession))?;
            let constraints = inputs.layout;
            if let Some(prepared) = self.cache.document.prepared() {
                let laid_out = Self::shape_and_layout(
                    prepared,
                    constraints,
                    &inputs.image_metrics,
                    glyph_shaper,
                    &mut self.glyph_resources,
                )?;
                self.cache
                    .document
                    .publish_prepared(inputs.layout_cache_key(), laid_out);
            } else {
                let entry = self
                    .cache
                    .document
                    .published_mut()
                    .ok_or_else(|| PipelineError::from_kind(PipelineErrorKind::EmptySession))?;
                entry
                    .value
                    .reshape_and_layout_with_metrics_and_shaper(
                        constraints,
                        &inputs.image_metrics,
                        glyph_shaper,
                        &mut self.glyph_resources,
                    )
                    .map_err(PipelineError::from)?;
                entry.key = inputs.layout_cache_key();
            }
            self.cache.shaped = Some(CacheEntry {
                key: inputs.shaped_cache_key(),
                value: glyph_shaper.resource_generation(),
            });
            Ok(())
        })();
        self.invalidate_stale_renderer_state(glyph_shaper);
        if result.is_ok() {
            self.scope_resources = None;
            self.glyph_resources.invalidate_render_scenes();
        }
        result
    }

    fn refresh_resource_cache(
        &mut self,
        resource_revision: ResourceRevision,
        stylesheet_revision: StylesheetRevision,
    ) {
        let cache_key = (resource_revision, stylesheet_revision);
        if self.cached_resource_key.as_ref() != Some(&cache_key) {
            self.cached_provider.clear();
            self.cached_resource_key = Some(cache_key);
        }
    }

    fn media_match_changed(
        &self,
        requested_inputs: &PipelineInputs,
        previous_media: Option<MediaEnvironment>,
    ) -> bool {
        previous_media.is_some_and(|previous_media| {
            previous_media != requested_inputs.style_environment.media
                && self.media_dependencies_changed(
                    requested_inputs.style_environment.media,
                    requested_inputs.style_environment.root_font_size,
                )
        })
    }

    fn media_dependencies_changed(
        &self,
        media: MediaEnvironment,
        root_font_size: RootFontSize,
    ) -> bool {
        let Some(CacheEntry { key, value: queries }) = &self.cache.media else {
            return true;
        };
        queries.uses_viewport_units()
            || queries.match_key(media, f64::from(root_font_size.get())) != *key
    }

    fn shape_and_layout(
        prepared: &PreparedDocument,
        constraints: LayoutConstraints,
        image_metrics: &crate::engine::ImageMetrics,
        glyph_shaper: &mut impl LayoutGlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<crate::engine::DetachedLayout, PipelineError> {
        prepared
            .shape_and_layout_with_metrics_and_shaper(
                constraints,
                image_metrics,
                glyph_shaper,
                resources,
            )
            .map(LaidOutDocument::into_detached)
            .map_err(PipelineError::from)
    }

    fn validate_cache_against_requested(&self, layout_key: &LayoutCacheKey) {
        if let Some(entry) = self.cache.document.published() {
            debug_assert_eq!(&entry.key, layout_key);
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
    /// Returns `Ok(None)` before the first successful update, or when `id`
    /// names no element. Backend and shaping failures retain their causes.
    pub fn layout_note(
        &mut self,
        id: &str,
        constraints: LayoutConstraints,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<Option<NoteLayout>, PipelineError> {
        self.collect_scope_resources(glyph_shaper);
        let result = (|| {
            let Some(inputs) = self.cache.inputs.as_ref() else {
                return Ok(None);
            };
            let Some(entry) = self.cache.document.published() else {
                return Ok(None);
            };
            let owner = &entry.value;
            if self.cache.renderer_generation() != Some(glyph_shaper.resource_generation()) {
                return Err(resource_generation_mismatch().into());
            }
            let Some(scoped) = owner.scoped_to_element_id(id) else {
                return Ok(None);
            };
            let checkpoint = glyph_shaper
                .resource_checkpoint()
                .map(|native| ScopeResources {
                    owner_shape: owner.shaping_generation(),
                    native,
                    end: native,
                    metrics: self.glyph_resources.checkpoint(),
                    owners: Vec::new(),
                });
            // Appended to the page's renderer resources rather than shaped as a
            // document of its own: a note's glyphs have to coexist with the page's,
            // which are still referenced by what is on screen. Every note appends,
            // so the seed is whatever the last one left behind.
            let laid_out = scoped
                .shape_and_layout_into_active_resources(
                    constraints,
                    &inputs.image_metrics,
                    glyph_shaper,
                    &mut self.glyph_resources,
                )
                ?
                .into_detached();
            let mut note = scoped.with_layout(laid_out).into_note(owner);
            if self.scope_resources.is_none() {
                self.scope_resources = checkpoint;
            }
            if let Some(resources) = &mut self.scope_resources {
                resources.end = glyph_shaper
                    .resource_checkpoint()
                    .expect("checkpoint-capable backend");
                let lifetime = Arc::new(());
                resources.owners.retain(|owner| owner.strong_count() > 0);
                resources.owners.push(Arc::downgrade(&lifetime));
                note.resource_lifetime = Some(lifetime);
            }
            Ok(Some(note))
        })();
        self.invalidate_stale_renderer_state(glyph_shaper);
        result
    }

    fn collect_scope_resources(&mut self, shaper: &mut impl LayoutGlyphShaper) {
        let Some(resources) = &self.scope_resources else {
            return;
        };
        if shaper.resource_checkpoint() != Some(resources.end) {
            self.scope_resources = None;
        } else if resources
            .owners
            .iter()
            .all(|owner| owner.strong_count() == 0)
        {
            let resources = self.scope_resources.take().expect("scope resource batch");
            if shaper.restore_resource_checkpoint(resources.native) {
                assert!(self.glyph_resources.rollback_to(resources.metrics));
            }
        }
    }

    /// Ids of the note bodies in the current document, in document order.
    /// Empty before the first successful update.
    pub fn note_ids(&self) -> Vec<&str> {
        if let Some(prepared) = self.cache.document.prepared() {
            prepared.note_ids()
        } else {
            self.document()
                .map(LaidOutDocument::note_ids)
                .unwrap_or_default()
        }
    }

    /// The last successfully published geometry. Backend invalidation does
    /// not erase it, so source queries and failure recovery remain available.
    /// After replacing renderer resources, update or rehydrate before painting
    /// renderer-owned glyph and native-run handles from this document.
    pub fn document(&self) -> Option<&LaidOutDocument<'static>> {
        self.cache.document.published().map(|entry| &entry.value)
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
        Self {
            inner,
            bytes: Mutex::new(HashMap::new()),
            strings: Mutex::new(HashMap::new()),
        }
    }

    fn read_cached<T: Clone>(
        cache: &Mutex<HashMap<String, T>>,
        uri: &str,
        lock_message: &str,
        load: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        if let Some(value) = cache.lock().expect(lock_message).get(uri).cloned() {
            return Ok(value);
        }
        let value = load()?;
        cache
            .lock()
            .expect(lock_message)
            .insert(uri.to_owned(), value.clone());
        Ok(value)
    }

    fn clear(&self) {
        self.bytes.lock().expect("resource byte cache lock").clear();
        self.strings
            .lock()
            .expect("resource string cache lock")
            .clear();
    }
}

impl ResourceProvider for RevisionCachedResourceProvider {
    fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
        Self::read_cached(&self.bytes, uri, "resource byte cache lock", || {
            self.inner.read_bytes(uri)
        })
    }

    fn read_string(&self, uri: &str) -> io::Result<String> {
        Self::read_cached(&self.strings, uri, "resource string cache lock", || {
            self.inner.read_string(uri)
        })
    }

    fn metadata(&self, uri: &str) -> io::Result<html_resources::ResourceMetadata> {
        self.inner.metadata(uri)
    }

    fn primary_language(&self) -> Option<String> {
        self.inner.primary_language()
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
}

#[cfg(test)]
mod tests {
    use crate::engine::search_queries;
    use html_dom as dom_values;
    use html_engine_model::{layout_state, shaping_contracts};
    use html_style_model::computed_style;
    use crate::engine::source_queries;
    use crate::engine::fragment_paint;

    use crate::engine::{box_queries, text_paint};

    use crate::engine::{ImageId, ImageMetrics};
    use crate::layout_model::BoxId;

    use crate::layout_model::LineId;
    use crate::{GlyphRequest, FontMetrics, ShapedGlyph};

    use super::PipelineSession;
    use crate::engine::{
        GlyphHandle, GlyphId, GlyphResourceStore, GlyphShaper as LayoutGlyphShaper,
        LayoutConstraints,
    };
    use crate::{DocumentFactory, PipelineChanges};
    use crate::{
        EarliestStage, FontEnvironmentRevision, FontEnvironmentRevision as FontEnv,
        ImageMetricsRevision, MarkupSyntax, MediaEnvironment,
        PipelineError, PipelineInputs, ResourceRevision, SourceRevision,
        StyleEnvironment, StylesheetRevision,
    };
    use html_dom::RootFontSize;
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
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no resources in mock provider",
            ))
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
        primary_language: Option<String>,
        resources: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        reads: Arc<Mutex<usize>>,
    }

    impl MutableProvider {
        fn put_bytes(&self, path: &str, bytes: Vec<u8>) {
            self.resources
                .lock()
                .expect("resource lock")
                .insert(path.to_owned(), bytes);
        }

        fn read_count(&self) -> usize {
            *self.reads.lock().expect("read count lock")
        }
    }

    impl ResourceProvider for MutableProvider {
        fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
            *self.reads.lock().expect("read count lock") += 1;
            let resources = self.resources.lock().expect("resource lock");
            resources
                .get(uri)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, uri.to_owned()))
        }

        fn primary_language(&self) -> Option<String> {
            self.primary_language.clone()
        }

        fn exists(&self, uri: &str) -> bool {
            self.resources
                .lock()
                .expect("resource lock")
                .contains_key(uri)
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
        cache: HashMap<(char, u32), GlyphHandle>,
        generation: crate::engine::GlyphResourceGeneration,
        append_checkpoint: Option<(
            HashMap<(char, u32), GlyphHandle>,
            crate::engine::GlyphResourceGeneration,
        )>,
    }

    impl LayoutGlyphShaper for TracingShaper {
        fn reset(&mut self) {
            self.cache.clear();
            self.generation = crate::engine::GlyphResourceGeneration::default();
        }

        fn resource_generation(&self) -> crate::engine::GlyphResourceGeneration {
            self.generation
        }

        fn begin_append_shaping(&mut self) -> Result<(), crate::engine::ShapeError> {
            assert!(self.append_checkpoint.is_none());
            self.append_checkpoint = Some((self.cache.clone(), self.generation.clone()));
            Ok(())
        }

        fn commit_append_shaping(&mut self) {
            self.append_checkpoint = None;
        }

        fn rollback_append_shaping(&mut self) {
            if let Some((cache, glyph_store)) = self.append_checkpoint.take() {
                self.cache = cache;
                self.generation = glyph_store;
            }
        }

        fn shape_glyph(
            &mut self,
            request: GlyphRequest<'_>,
        ) -> Result<ShapedGlyph, crate::engine::ShapeError> {
            self.calls += 1;
            let key = (request.character, request.style.font_size.to_bits());

            let idx = u32::try_from(self.cache.len()).expect("glyph ids must fit");
            let glyph = GlyphHandle(idx);
            let advance = 1.0;
            let metrics = FontMetrics {
                ascent: 1.0,
                descent: 0.0,
            };
            if let Some(&glyph) = self.cache.get(&key) {
                return Ok(ShapedGlyph::new(glyph, advance, metrics));
            }

            self.cache.insert(key, glyph);
            Ok(ShapedGlyph::new(glyph, advance, metrics))
        }
    }

    #[derive(Clone, Default)]
    struct SizedShaper {
        cache: HashMap<(char, u32), GlyphHandle>,
        generation: crate::engine::GlyphResourceGeneration,
        append_checkpoint: Option<(
            HashMap<(char, u32), GlyphHandle>,
            crate::engine::GlyphResourceGeneration,
        )>,
    }

    impl LayoutGlyphShaper for SizedShaper {
        fn reset(&mut self) {
            self.cache.clear();
            self.generation = crate::engine::GlyphResourceGeneration::default();
        }

        fn resource_generation(&self) -> crate::engine::GlyphResourceGeneration {
            self.generation
        }

        fn begin_append_shaping(&mut self) -> Result<(), crate::engine::ShapeError> {
            assert!(self.append_checkpoint.is_none());
            self.append_checkpoint = Some((self.cache.clone(), self.generation.clone()));
            Ok(())
        }

        fn commit_append_shaping(&mut self) {
            self.append_checkpoint = None;
        }

        fn rollback_append_shaping(&mut self) {
            if let Some((cache, glyph_store)) = self.append_checkpoint.take() {
                self.cache = cache;
                self.generation = glyph_store;
            }
        }

        fn shape_glyph(
            &mut self,
            request: GlyphRequest<'_>,
        ) -> Result<ShapedGlyph, crate::engine::ShapeError> {
            let key = (request.character, request.style.font_size.to_bits());

            let idx = u32::try_from(self.cache.len()).expect("glyph ids must fit");
            let glyph = GlyphHandle(idx);
            let width = request.style.font_size.max(1.0) * 0.75;
            let advance = width;
            let metrics = FontMetrics {
                ascent: request.style.font_size,
                descent: request.style.font_size * 0.2,
            };
            if let Some(&glyph) = self.cache.get(&key) {
                return Ok(ShapedGlyph::new(glyph, advance, metrics));
            }

            self.cache.insert(key, glyph);
            Ok(ShapedGlyph::new(glyph, advance, metrics))
        }
    }

    #[derive(Default)]
    struct TransactionalShaper {
        cache: HashMap<(char, u32), GlyphHandle>,
        resources: Vec<char>,
        generation: crate::engine::GlyphResourceGeneration,
        previous_document: Option<(
            HashMap<(char, u32), GlyphHandle>,
            Vec<char>,
            crate::engine::GlyphResourceGeneration,
        )>,
        previous_append: Option<(
            HashMap<(char, u32), GlyphHandle>,
            Vec<char>,
            crate::engine::GlyphResourceGeneration,
        )>,
        fail_on: Option<char>,
        fail_font_size: Option<u32>,
        failure_resource_count: Option<usize>,
    }

    impl LayoutGlyphShaper for TransactionalShaper {
        fn reset(&mut self) {
            self.cache.clear();
            self.resources.clear();
            self.generation = crate::engine::GlyphResourceGeneration::default();
        }

        fn resource_generation(&self) -> crate::engine::GlyphResourceGeneration {
            self.generation
        }

        fn begin_append_shaping(&mut self) -> Result<(), crate::engine::ShapeError> {
            assert!(
                self.previous_append.is_none(),
                "append shaping transactions cannot be nested"
            );
            self.previous_append = Some((
                self.cache.clone(),
                self.resources.clone(),
                self.generation.clone(),
            ));
            Ok(())
        }

        fn commit_append_shaping(&mut self) {
            self.previous_append = None;
        }

        fn rollback_append_shaping(&mut self) {
            if let Some((cache, resources, glyph_store)) = self.previous_append.take() {
                self.cache = cache;
                self.resources = resources;
                self.generation = glyph_store;
            }
        }

        fn begin_document_shaping(&mut self) {
            assert!(
                self.previous_document.is_none(),
                "document shaping transactions cannot be nested"
            );
            self.previous_document = Some((
                std::mem::take(&mut self.cache),
                std::mem::take(&mut self.resources),
                std::mem::take(&mut self.generation),
            ));
        }

        fn commit_document_shaping(&mut self) {
            self.previous_document = None;
        }

        fn rollback_document_shaping(&mut self) {
            if let Some((cache, resources, glyph_store)) = self.previous_document.take() {
                self.cache = cache;
                self.resources = resources;
                self.generation = glyph_store;
            }
        }

        fn shape_glyph(
            &mut self,
            request: GlyphRequest<'_>,
        ) -> Result<ShapedGlyph, crate::engine::ShapeError> {
            if self.fail_on == Some(request.character)
                || self.fail_font_size == Some(request.style.font_size.to_bits())
            {
                self.failure_resource_count = Some(self.resources.len());
                return Err(shaping_contracts::shape_error_registry_capacity_exhausted(self.cache.len(),));
            }
            let key = (request.character, request.style.font_size.to_bits());
            let advance = 1.0;
            let metrics = FontMetrics {
                ascent: 1.0,
                descent: 0.0,
            };
            if let Some(&glyph) = self.cache.get(&key) {
                return Ok(ShapedGlyph::new(glyph, advance, metrics));
            }
            let glyph = GlyphHandle(self.cache.len() as u32);
            self.cache.insert(key, glyph);
            self.resources.push(request.character);
            Ok(ShapedGlyph::new(glyph, advance, metrics))
        }
    }

    fn base_inputs(html: &str, source_revision: SourceRevision) -> PipelineInputs {
        PipelineInputs {
            source: html.into(),
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
            layout: LayoutConstraints::new(
                600.0,
                f64::from(StyleEnvironment::default().root_font_size.get()),
            )
            .unwrap(),
            image_metrics: Default::default(),
        }
    }

    fn glyph_text<'a>(document: impl Into<crate::engine::RenderView<'a>>) -> String {
        let text = document.into().text();
        (0..text.glyph_count())
            .filter_map(|index| {
                text_paint::glyph_at(text, index)
                    .and_then(|glyph| text_paint::glyph_metric(text, glyph))
                    .map(|metric| metric.ch())
            })
            .collect()
    }

    #[test]
    fn hyphenation_language_fallback_survives_preparation_and_style_rebuilds() {
        for (publication, markup, expected) in [
            (None, "", false),
            (Some("en"), "", true),
            (Some("zz"), "", false),
            (Some("zz"), "lang='en'", true),
            (Some("en"), "lang='zz'", false),
        ] {
            let provider = MutableProvider {
                primary_language: publication.map(str::to_owned),
                ..Default::default()
            };
            let mut session = PipelineSession::new(Arc::new(provider));
            let mut inputs = base_inputs(
                &format!(
                    "<html {markup}><body style='margin:0;hyphens:auto'><p style='margin:0'>internationalization internationalization</p></body></html>"
                ),
                SourceRevision::INITIAL,
            );
            inputs.layout = inputs.layout.with_viewport_width(8.0).unwrap();
            let mut shaper = SizedShaper::default();
            session.prepare_through_style(inputs.clone()).unwrap();
            for weight in [0, 100] {
                inputs.reader_overrides.font_weight_adjustment = weight;
                session.update(inputs.clone(), &mut shaper).unwrap();

                let text = session
                    .document()
                    .unwrap()
                    .render_view(session.glyph_resources())
                    .text();
                assert_eq!(
                    text_paint::render_text_view_spatial_order(text)
                        .iter()
                        .copied()
                        .any(|index| text_paint::hyphen_for_line(text, index).is_some()),
                    expected,
                    "publication {publication:?}, explicit {markup}, style adjustment {weight}"
                );
            }
        }
    }

    #[test]
    fn pending_preparation_uses_the_latest_palette_when_published() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut inputs = base_inputs("<p>Hello</p>", SourceRevision::INITIAL);
        session.prepare_through_style(inputs.clone()).unwrap();
        inputs.reader_overrides.foreground = Some(0xeeeeeeff);
        let mut shaper = SizedShaper::default();
        let update = session.update(inputs, &mut shaper).unwrap();
        assert_eq!(update, EarliestStage::Shape);
        assert!(session.cache.document.prepared().is_none());

        let text = session
            .document()
            .unwrap()
            .render_view(session.glyph_resources())
            .text();
        assert_eq!(text_paint::paint_color_at(text, 0), Some(0xeeeeeeff));
    }

    #[test]
    fn font_faces_are_available_before_shaping_without_reading_font_bytes() {
        let provider = Arc::new(MutableProvider::default());
        let mut session = PipelineSession::new(provider.clone());
        assert!(session.font_faces().is_empty());
        let inputs = base_inputs(
            r#"<html><head><style>
                @font-face { font-family: Reader; src: url(reader.woff2) format(woff2); }
            </style></head><body><p>Hello</p></body></html>"#,
            SourceRevision::INITIAL,
        );
        session
            .prepare_through_style(inputs)
            .expect("prepare without shaper");
        assert_eq!(session.font_faces()[0].family, "Reader");
        assert_eq!(provider.read_count(), 0, "font bytes belong to the caller");
        assert!(session.document().is_none());
        assert!(session.cache.document.prepared().is_some());
        let mut shaper = TracingShaper::default();
        session
            .rehydrate_glyphs(&mut shaper)
            .expect("shape after host font registration");
        assert!(session.document().is_some());
        assert!(
            session.cache.document.prepared().is_none(),
            "published preparation has one owner"
        );
        assert!(
            session.cache.document.prepared_key().is_some() && session.cache.shaped.is_some(),
            "stage caches retain validity keys"
        );
        assert_eq!(session.font_faces()[0].family, "Reader");
        assert_eq!(provider.read_count(), 0);
    }

    #[test]
    fn font_faces_follow_media_changes_and_successful_document_replacement() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let mut inputs = base_inputs(
            r#"<html><head><style>
                @media (min-width: 700px) {
                    @font-face { font-family: Wide; src: url(wide.ttf); }
                }
            </style></head><body><p>Hello</p></body></html>"#,
            SourceRevision::INITIAL,
        );
        session
            .update(inputs.clone(), &mut shaper)
            .expect("initial layout");
        assert!(session.font_faces().is_empty());
        inputs.style_environment.media =
            MediaEnvironment::new(html_style::MediaType::Screen, 800.0, None).unwrap();
        inputs.layout = inputs.layout.with_viewport_width(800.0).unwrap();
        let update = session
            .update(inputs.clone(), &mut shaper)
            .expect("font-only media change");
        assert_eq!(update, EarliestStage::Style);
        assert_eq!(session.font_faces()[0].family, "Wide");
        inputs.source = r#"<html><head><style>
            @font-face { font-family: Replacement; src: url(new.ttf); }
        </style></head><body><p>!</p></body></html>"#
            .into();
        shaper.fail_on = Some('!');
        assert!(session.update(inputs.clone(), &mut shaper).is_err());
        assert_eq!(
            session.font_faces()[0].family,
            "Wide",
            "failed updates preserve published metadata"
        );
        shaper.fail_on = None;
        session
            .update(inputs, &mut shaper)
            .expect("successful replacement");
        assert_eq!(session.font_faces()[0].family, "Replacement");
        let no_fonts = base_inputs(
            "<html><body><p>No fonts</p></body></html>",
            SourceRevision::INITIAL,
        );
        session
            .update(no_fonts, &mut shaper)
            .expect("remove declarations");
        assert!(session.font_faces().is_empty());
    }

    #[test]
    fn an_excluded_note_is_absent_from_the_flow_and_available_through_scoped_layout() {
        const SOURCE: &str = "<html><body><p>Reading</p><aside id='n' epub:type='footnote'><p>zebra</p></aside><p>Continues</p></body></html>";
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        let mut inputs = base_inputs(SOURCE, SourceRevision::INITIAL);
        inputs.note_flow = crate::engine::NoteFlow::Excluded;
        session
            .update(inputs, &mut shaper)
            .expect("pipeline update should succeed");

        let flow = glyph_text(
            (session.document().expect("a document")).render_view(session.glyph_resources()),
        );
        assert!(flow.contains("Reading") && flow.contains("Continues"));
        assert!(
            !flow.contains("zebra"),
            "an excluded note must not consume the reading flow"
        );

        assert_eq!(session.note_ids(), vec!["n"]);

        // The note is still reachable: scoped layout gives an embedder the
        // content it held back, under its own width.
        let constraints =
            crate::engine::LayoutConstraints::new(200.0, 20.0).expect("valid constraints");
        let note = session
            .layout_note("n", constraints, &mut shaper)
            .expect("scope shaping succeeds").expect("the held-back note must lay out on demand");
        assert!(
            glyph_text(
                note.render_view(session.document().unwrap(), session.glyph_resources())
                    .unwrap()
            )
            .contains("zebra"),
            "scoped layout produces the note's own content"
        );
        assert!(
            !glyph_text(
                note.render_view(session.document().unwrap(), session.glyph_resources())
                    .unwrap()
            )
            .contains("Reading"),
            "a scoped note is only its own subtree"
        );

        assert!(
            session
                .layout_note("absent", constraints, &mut shaper)
                .unwrap().is_none(),
            "an id that names nothing lays out nothing"
        );
    }

    #[test]
    fn detached_note_borrows_its_owner_and_rejects_replaced_preparation() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let mut inputs = base_inputs(
            "<p>Reading</p><aside id='n' epub:type='footnote'><p>Note body</p><img src='image.png'></aside>",
            SourceRevision::INITIAL,
        );
        inputs.note_flow = crate::engine::NoteFlow::Excluded;
        session.update(inputs.clone(), &mut shaper).unwrap();
        let constraints = crate::engine::LayoutConstraints::new(160.0, 20.0).unwrap();
        let note = session
            .layout_note("n", constraints, &mut shaper)
            .unwrap()
            .unwrap();

        let owner = session.document().unwrap();
        let view = note.render_view(owner, session.glyph_resources()).unwrap();
        assert!(std::ptr::eq(
            text_paint::render_view_images(&view).as_ptr(),
            text_paint::render_view_images(&owner.render_view(session.glyph_resources())).as_ptr()
        ));
        assert!(glyph_text(view).contains("Note body"));

        inputs.user_styles.push("p {font-size:24px}".to_owned());
        shaper.fail_on = Some('R');
        assert!(session.update(inputs.clone(), &mut shaper).is_err());
        assert!(
            note.render_view(session.document().unwrap(), session.glyph_resources())
                .is_some(),
            "rollback restores the note's owner identity"
        );
        shaper.fail_on = None;
        session.update(inputs, &mut shaper).unwrap();
        assert!(
            note.render_view(session.document().unwrap(), session.glyph_resources())
                .is_none()
        );

        // The detached products can outlive the owner, but cannot be queried
        // using a different document, even when its markup is identical.
        drop(session);
        let mut replacement = PipelineSession::new(Arc::new(MockProvider));
        replacement
            .update(
                base_inputs("<aside id='n'>Note body</aside>", SourceRevision::INITIAL),
                &mut shaper,
            )
            .unwrap();
        assert!(
            note.render_view(
                replacement.document().unwrap(),
                replacement.glyph_resources()
            )
            .is_none()
        );
    }

    #[test]
    fn detached_notes_reject_changed_palettes_and_glyph_resources() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let mut inputs = base_inputs(
            "<p>Reading</p><aside id='n' epub:type='footnote'>Note body</aside>",
            SourceRevision::INITIAL,
        );
        inputs.note_flow = crate::engine::NoteFlow::Excluded;
        session.update(inputs.clone(), &mut shaper).unwrap();
        let constraints = crate::engine::LayoutConstraints::new(160.0, 20.0).unwrap();
        let note = session.layout_note("n", constraints, &mut shaper).unwrap().unwrap();
        inputs.reader_overrides.foreground = Some(0xeeeeeeff);
        session.update(inputs, &mut shaper).unwrap();
        assert!(
            note.render_view(session.document().unwrap(), session.glyph_resources())
                .is_none()
        );
        let note = session.layout_note("n", constraints, &mut shaper).unwrap().unwrap();
        assert!(
            note.render_view(session.document().unwrap(), session.glyph_resources())
                .is_some()
        );
        session.rehydrate_glyphs(&mut shaper).unwrap();
        assert!(
            note.render_view(session.document().unwrap(), session.glyph_resources())
                .is_none()
        );
    }

    #[test]
    fn pipeline_keeps_explicit_xml_strict_and_recovers_xhtml() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        let malformed =
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>broken</body></html>";

        let mut xhtml_inputs = base_inputs(malformed, SourceRevision::INITIAL);
        xhtml_inputs.base_uri = "reference.xht".to_owned();
        xhtml_inputs.markup_syntax = MarkupSyntax::Xml;
        let error = session
            .update(xhtml_inputs, &mut shaper)
            .expect_err("explicit XML parsing must remain strict");
        assert!(error.to_string().contains("XML parse error"));

        let mut xhtml_inputs = base_inputs(malformed, SourceRevision::INITIAL.next());
        xhtml_inputs.base_uri = "reference.xht".to_owned();
        xhtml_inputs.markup_syntax = MarkupSyntax::Xhtml;
        session.update(xhtml_inputs, &mut shaper).expect("XHTML recovers malformed markup");

        let mut html_inputs = base_inputs(malformed, SourceRevision::INITIAL.next());
        html_inputs.base_uri = "reference.html".to_owned();
        session
            .update(html_inputs, &mut shaper)
            .expect("HTML continues to use html5ever recovery");
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
        session
            .update(inputs, &mut shaper)
            .expect("well-formed XHTML pipeline");

        let colors = fragment_paint::decorations(
            session
                .document()
                .expect("the XML pipeline produced a document")
                .render_view(session.glyph_resources())
                .fragments(),
        )
        .iter()
        .map(|fragment| fragment_paint::decoration_color(&fragment))
        .collect::<Vec<_>>();
        assert!(
            colors.contains(&0x008000FF),
            "CDATA rules must reach computed paint values"
        );
        assert!(
            colors.contains(&0x0000FFFF),
            "the first sibling must retain the type-selector background"
        );
        assert!(
            colors.contains(&0xFFA500FF),
            "the adjacent-sibling selector must style the second sibling; colors={colors:#x?}"
        );
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
        inputs.layout = inputs.layout.with_viewport_width(800.0).unwrap();
        inputs.layout = inputs.layout.with_viewport_height(Some(600.0)).unwrap();
        session
            .update(inputs, &mut shaper)
            .expect("well-formed XHTML pipeline");

        let view = session
            .document()
            .expect("XHTML document")
            .render_view(session.glyph_resources());
        let size_for = |id| {
            let index = (0..box_queries::len(view.boxes()))
                .map(layout_state::box_id_new)
                .find(|&idx| box_queries::attribute(view.boxes(), idx, "id") == Some(id))
                .expect("identified layout box");
            box_queries::render_box_view_size(view.boxes(), index).expect("box geometry")
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
        inputs.layout = inputs.layout.with_viewport_width(300.0).unwrap();
        session
            .update(inputs.clone(), &mut shaper)
            .expect("initial XHTML pipeline");
        inputs.layout = inputs.layout.with_viewport_height(Some(600.0)).unwrap();
        inputs.style_environment.media =
            MediaEnvironment::screen(300.0, Some(600.0)).expect("valid viewport");
        session
            .update(inputs, &mut shaper)
            .expect("viewport relayout");

        let view = session
            .document()
            .expect("the XML pipeline produced a document")
            .render_view(session.glyph_resources());
        let size_for = |id| {
            let index = (0..box_queries::len(view.boxes()))
                .map(layout_state::box_id_new)
                .find(|&idx| box_queries::attribute(view.boxes(), idx, "id") == Some(id))
                .expect("identified layout box");
            box_queries::render_box_view_size(view.boxes(), index).expect("box geometry")
        };
        assert_eq!(size_for("body").height, 600.0);
        assert_eq!(size_for("child").height, 600.0);
        let green = fragment_paint::decorations(view.fragments())
            .iter()
            .find(|fragment| fragment_paint::decoration_color(&fragment) == 0x008000FF)
            .expect("green child background");
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
        inputs.layout = inputs.layout.with_viewport_width(800.0).unwrap();
        inputs.layout = inputs.layout.with_viewport_height(Some(600.0)).unwrap();
        session
            .update(inputs, &mut shaper)
            .expect("well-formed XHTML pipeline");

        let lines = session
            .document()
            .expect("XHTML document")
            .render_view(session.glyph_resources())
            .text()
            .lines()
            .iter()
            .map(|line| line.point())
            .collect::<Vec<_>>();
        let min_y = lines
            .iter()
            .map(|point| point.y)
            .fold(f64::INFINITY, f64::min);
        let max_y = lines
            .iter()
            .map(|point| point.y)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (max_y - min_y).abs() < 0.01,
            "top:0 XHTML text must translate to the earlier table content: {lines:?}"
        );
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
        inputs.layout = inputs.layout.with_viewport_width(500.0).unwrap();
        inputs.image_metrics.set(0, 96, 96);
        session
            .update(inputs, &mut shaper)
            .expect("well-formed XHTML pipeline");

        let view = session
            .document()
            .expect("the XML pipeline produced a document")
            .render_view(session.glyph_resources());
        let target = (0..box_queries::len(view.boxes()))
            .map(layout_state::box_id_new)
            .find(|&idx| box_queries::attribute(view.boxes(), idx, "id") == Some("target"))
            .expect("image layout box");
        let size = box_queries::render_box_view_size(view.boxes(), target).expect("image box size");
        assert_eq!((size.width, size.height), (500.0, 1.0));
    }

    #[test]
    fn failed_document_shaping_restores_cached_document_and_renderer_resources() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let original_inputs = base_inputs(
            "<html><body><p>original document</p></body></html>",
            SourceRevision::INITIAL,
        );
        session
            .update(original_inputs.clone(), &mut shaper)
            .expect("initial update");

        let original_document = build_snapshot(
            (session.document().expect("initial document")).render_view(session.glyph_resources()),
        );
        let original_resources = shaper.resources.clone();
        let original_layout_key = session
            .cache
            .document
            .published()
            .map(|entry| entry.key.clone());

        shaper.fail_on = Some('!');
        let replacement = base_inputs(
            "<html><body><p>replacement!</p></body></html>",
            SourceRevision::INITIAL.next(),
        );
        session
            .update(replacement, &mut shaper)
            .expect_err("replacement shaping must fail");

        assert_eq!(
            build_snapshot(
                (session.document().expect("cached document must remain"))
                    .render_view(session.glyph_resources())
            ),
            original_document
        );
        assert_eq!(
            shaper.resources, original_resources,
            "renderer resources must roll back with the document"
        );
        assert!(
            shaper.previous_document.is_none(),
            "the failed transaction must be closed"
        );
        assert_eq!(session.cache.inputs.as_ref(), Some(&original_inputs));
        assert_eq!(
            session.cache.document.published().map(|entry| &entry.key),
            original_layout_key.as_ref()
        );
    }

    #[test]
    fn failed_repreparation_preserves_both_document_owners_and_can_retry()
    -> Result<(), &'static str> {
        use crate::engine::NoteFlow;
        use html_dom::{Document, ImageSource};

        fn document(session: &PipelineSession) -> &Document {
            if let Some(prepared) = session.cache.document.prepared() {
                prepared.document()
            } else {
                session.document().unwrap().document()
            }
        }

        for prepared_only in [false, true] {
            for late_failure in [false, true] {
                let mut original = base_inputs(
                    "<p>Alpha beta gamma delta</p><math><mi>x</mi></math><aside id='n' epub:type='footnote'><p>Note body</p></aside>",
                    SourceRevision::INITIAL,
                );
                original.note_flow = NoteFlow::Excluded;
                let mut session = PipelineSession::new(Arc::new(MockProvider));
                let mut shaper = TransactionalShaper::default();
                if prepared_only {
                    session
                        .prepare_through_style(original.clone())
                        .map_err(|_| "initial preparation")?;
                } else {
                    session
                        .update(original.clone(), &mut shaper)
                        .map_err(|_| "initial layout")?;
                }
                let note = if prepared_only {
                    None
                } else {
                    Some(
                        session
                            .layout_note("n", original.layout, &mut shaper)
                            .map_err(|_| "initial detached note")?
                            .ok_or("initial detached note")?,
                    )
                };
                let snapshot = session.render_view().map(build_snapshot);
                let images = document(&session).images().to_vec();
                if !images.iter().any(|image| match &image.source {
                    ImageSource::MathMl { .. } => true,
                    _ => false,
                }) {
                    return Err("fixture must exercise MathML restoration");
                }
                let font = document(&session).root_font_size();
                let dom = document(&session).dom_root();
                let backend = (
                    shaper.cache.clone(),
                    shaper.resources.clone(),
                    shaper.generation,
                );
                let metrics: Vec<_> = (0..session.glyph_resources.len())
                    .map(|index| session.glyph_resources.get(GlyphId(index as u32)))
                    .collect();
                let resource_generation = session.glyph_resources.generation();
                let active_generation = session.cache.renderer_generation();
                let keys = (
                    session.cache.document.prepared_key().cloned(),
                    session.cache.shaped.as_ref().map(|entry| entry.key.clone()),
                    session
                        .cache
                        .document
                        .published()
                        .map(|entry| entry.key.clone()),
                );
                let mut replacement = original.clone();
                replacement.style_environment.root_font_size = dom_values::root_font_size_new(24.0).unwrap();
                replacement.note_flow = NoteFlow::InFlow;
                replacement.reader_overrides.foreground = Some(0x123456ff);
                replacement.user_styles.push(
                    "math {font-size:36px;color:purple} p::first-line {font-size:64px}".to_owned(),
                );
                shaper.fail_on = (!late_failure).then_some('A');
                shaper.fail_font_size = late_failure.then_some(64.0f32.to_bits());
                for _ in 0..2 {
                    shaper.failure_resource_count = None;
                    let failure = session.update(replacement.clone(), &mut shaper);
                    if failure.is_ok() || shaper.failure_resource_count.is_none() {
                        return Err("replacement must reach the injected shaping failure");
                    }
                    if late_failure && shaper.failure_resource_count == Some(0) {
                        return Err("first-line failure must follow successful base shaping");
                    }
                    let current_metrics: Vec<_> = (0..session.glyph_resources.len())
                        .map(|index| session.glyph_resources.get(GlyphId(index as u32)))
                        .collect();
                    if session.inputs() != Some(&original)
                        || document(&session).root_font_size() != font
                        || document(&session).dom_root() != dom
                        || document(&session).images() != images
                        || session.render_view().map(build_snapshot) != snapshot
                        || shaper.cache != backend.0
                        || shaper.resources != backend.1
                        || shaper.generation != backend.2
                        || shaper.previous_document.is_some()
                        || shaper.previous_append.is_some()
                        || current_metrics != metrics
                        || session.glyph_resources.generation() != resource_generation
                        || session.cache.renderer_generation() != active_generation
                        || session.cache.document.prepared_key() != keys.0.as_ref()
                        || session.cache.shaped.as_ref().map(|entry| &entry.key) != keys.1.as_ref()
                        || session.cache.document.published().map(|entry| &entry.key)
                            != keys.2.as_ref()
                    {
                        return Err(
                            "failed preparation changed published state or renderer resources",
                        );
                    }
                    if let Some(note) = &note {
                        if note
                            .render_view(session.document().unwrap(), session.glyph_resources())
                            .is_none()
                        {
                            return Err("failed preparation invalidated a detached note");
                        }
                    }
                }
                shaper.fail_on = None;
                shaper.fail_font_size = None;
                session
                    .update(replacement.clone(), &mut shaper)
                    .map_err(|_| "retry must succeed")?;
                let mut fresh = PipelineSession::new(Arc::new(MockProvider));
                fresh
                    .update(replacement, &mut TransactionalShaper::default())
                    .map_err(|_| "fresh layout")?;
                if session.render_view().map(build_snapshot)
                    != fresh.render_view().map(build_snapshot)
                {
                    return Err("successful retry differs from a fresh pipeline");
                }
                if let Some(note) = &note {
                    if note
                        .render_view(session.document().unwrap(), session.glyph_resources())
                        .is_some()
                    {
                        return Err("successful preparation must invalidate the old note owner");
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn preparation_restores_root_font_if_the_style_resolver_unwinds() -> Result<(), &'static str> {
        use crate::engine::{NoteFlow, ReaderStyleOverrides};
        use crate::layout::InlinePlanGeneration;
        use html_dom::RootFontSize;
        use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let inputs = base_inputs("<p>Original</p>", SourceRevision::INITIAL);
        session
            .update(inputs.clone(), &mut shaper)
            .map_err(|_| "initial update")?;
        let snapshot = session.render_view().map(build_snapshot);
        let result = catch_unwind(AssertUnwindSafe(|| {
            session
                .cache
                .document
                .published_mut()
                .unwrap()
                .value
                .reprepare_and_layout(
                    dom_values::root_font_size_new(32.0).unwrap(),
                    NoteFlow::Excluded,
                    &ReaderStyleOverrides::default(),
                    |_| resume_unwind(Box::new(())),
                    inputs.layout,
                    &inputs.image_metrics,
                    &mut shaper,
                    &mut session.glyph_resources,
                )
        }));
        if result.is_ok()
            || session.document().unwrap().document().root_font_size()
                != inputs.style_environment.root_font_size.get()
            || session.render_view().map(build_snapshot) != snapshot
        {
            return Err("resolver unwind changed the previously published preparation");
        }
        session
            .rehydrate_glyphs(&mut shaper)
            .map_err(|_| "document must remain reusable")?;
        Ok(())
    }

    #[test]
    fn invalidated_backend_retains_published_geometry_but_requires_new_shaping() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let inputs = base_inputs("<p>Published chapter</p>", SourceRevision::INITIAL);
        session.update(inputs.clone(), &mut shaper).unwrap();

        let original =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));
        shaper.reset();
        shaper.fail_on = Some('P');
        session
            .attach(&mut shaper)
            .update(inputs.clone())
            .unwrap_err();
        assert_eq!(
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources())),
            original
        );
        assert!(session.cache.shaped.is_none());
        assert!(
            session
                .layout_note(
                    "n",
                    crate::engine::LayoutConstraints::new(100.0, 16.0).unwrap(),
                    &mut shaper
                )
                .is_err()
        );
        shaper.fail_on = None;
        assert_eq!(
            session.attach(&mut shaper).update(inputs).unwrap(),
            EarliestStage::Shape
        );
        assert_eq!(
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources())),
            original
        );
    }

    #[test]
    fn source_addresses_follow_rehydration_reflow_and_source_replacement() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut inputs = base_inputs("<p>Chapter with text</p>", SourceRevision::INITIAL);
        session.prepare_through_style(inputs.clone()).unwrap();
        let mut shaper = TransactionalShaper::default();
        session.rehydrate_glyphs(&mut shaper).unwrap();

        let location = |session: &PipelineSession| {
            source_queries::source_location_on_line(
                session
                    .document()
                    .unwrap()
                    .render_view(session.glyph_resources())
                    .addressing(),
                LineId::new(0),
                Some(0),
            )
            .unwrap()
        };
        let original = location(&session);
        session.rehydrate_glyphs(&mut shaper).unwrap();
        assert_eq!(location(&session), original);
        inputs.layout = inputs.layout.with_viewport_width(80.0).unwrap();
        session.update(inputs.clone(), &mut shaper).unwrap();
        assert_eq!(location(&session), original);
        inputs.source_revision = inputs.source_revision.next();
        inputs.source = "<blockquote><p>Chapter with text</p></blockquote>".into();
        session.update(inputs, &mut shaper).unwrap();
        assert_ne!(
            location(&session),
            original,
            "queries use the current DOM without retained metadata"
        );
    }

    #[test]
    fn glyph_rehydration_publishes_in_place_and_preserves_the_previous_result_on_failure() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        session
            .update(
                base_inputs("<p>Chapter</p>", SourceRevision::INITIAL),
                &mut shaper,
            )
            .unwrap();

        let original =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));
        let resources = shaper.resources.clone();
        let generation = shaper.resource_generation();
        shaper.fail_on = Some('C');
        session.rehydrate_glyphs(&mut shaper).unwrap_err();
        assert_eq!(shaper.resources, resources);
        assert_eq!(
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources())),
            original
        );
        shaper.fail_on = None;
        let () = session.rehydrate_glyphs(&mut shaper).unwrap();
        assert_eq!(
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources())),
            original
        );
        assert_ne!(shaper.resource_generation(), generation);
    }

    #[test]
    fn width_only_relayout_preserves_authoritative_document_shaping() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let original_inputs = base_inputs(
            "<html><body><p>text that wraps across several words</p></body></html>",
            SourceRevision::INITIAL,
        );
        session
            .update(original_inputs.clone(), &mut shaper)
            .expect("initial update");

        let original_document = build_snapshot(
            (session.document().expect("initial document")).render_view(session.glyph_resources()),
        );
        let mut narrower = original_inputs.clone();
        narrower.layout = narrower.layout.with_viewport_width(10.0).unwrap();
        let changed_layout = &mut session
            .cache
            .document
            .published_mut()
            .expect("initial document")
            .value;
        let changed_layout_resources = &session.glyph_resources;
        changed_layout
            .relayout_with_metrics(
                narrower.layout,
                &narrower.image_metrics,
                changed_layout_resources,
            )
            .expect("layout requires the shaper-aware API for first-line styles");
        let expected = build_snapshot(changed_layout.render_view(changed_layout_resources));
        assert_ne!(
            expected, original_document,
            "the request must exercise geometry that differs from the cached layout"
        );
        session
            .update(narrower.clone(), &mut shaper)
            .expect("layout must reuse authoritative document shaping");

        assert_eq!(
            build_snapshot(
                (session.document().expect("updated document"))
                    .render_view(session.glyph_resources())
            ),
            expected
        );
        assert_eq!(session.cache.inputs.as_ref(), Some(&narrower));
    }

    #[test]
    fn stylesheet_resource_reads_are_reused_until_a_resource_or_stylesheet_revision_changes() {
        let provider = MutableProvider::default();
        provider.put_bytes("theme.css", b"p { color: red; }".to_vec());
        let mut session = PipelineSession::new(Arc::new(provider.clone()));
        let mut shaper = TracingShaper::default();
        let html = "<html><head><link rel=\"stylesheet\" href=\"theme.css\"></head><body><p>cached stylesheet</p></body></html>";

        session
            .update(base_inputs(html, SourceRevision::INITIAL), &mut shaper)
            .expect("initial update");
        let mut source_changed = base_inputs(html, SourceRevision::INITIAL.next());
        session
            .update(source_changed.clone(), &mut shaper)
            .expect("source-only update");
        assert_eq!(
            provider.read_count(),
            1,
            "same resource and stylesheet revisions reuse CSS bytes"
        );

        source_changed.resource_revision = ResourceRevision::INITIAL.next();
        session
            .update(source_changed, &mut shaper)
            .expect("resource revision update");
        assert_eq!(
            provider.read_count(),
            2,
            "resource revision invalidates CSS bytes"
        );
    }

    fn build_snapshot<'a>(document: impl Into<crate::engine::RenderView<'a>>) -> RenderSnapshot {
        let root = document.into();
        let text = root.text();
        let fragments = root.fragments();
        let addressing = root.addressing();
        let mut glyphs = Vec::with_capacity(text.glyph_count());
        for idx in 0..text.glyph_count() {
            glyphs.push(text_paint::glyph_at(text, idx).unwrap_or(GlyphId(0)));
        }
        let mut lines = Vec::with_capacity(text.line_count());
        for line_idx in text_paint::render_text_view_spatial_order(text)
            .iter()
            .copied()
        {
            let line = text.line(line_idx).expect("line should exist");
            lines.push(LineSnapshot {
                start: line.start(),
                end: line.end(),
                point_x_bits: line.point().x.to_bits(),
                point_y_bits: line.point().y.to_bits(),
                height_bits: line.height().to_bits(),
                baseline_bits: text_paint::line_baseline(&line).to_bits(),
                word_spacing_bits: text_paint::line_word_spacing(&line).to_bits(),
                letter_spacing_bits: text_paint::line_letter_spacing(&line).to_bits(),
            });
        }
        RenderSnapshot {
            glyphs,
            glyph_advances: text_paint::render_text_view_spatial_order(text)
                .iter()
                .copied()
                .map(|line_idx| {
                    let Some(runs) = text_paint::line_glyph_advances(text, line_idx) else {
                        return Vec::new();
                    };
                    runs.iter()
                        .map(|run| {
                            let range = run.range();
                            (range.start, range.end, run.advance().to_bits())
                        })
                        .collect()
                })
                .collect(),
            line_links: (0..text.glyph_count() as u32)
                .map(|glyph_idx| {
                    addressing
                        .link_for_glyph(glyph_idx)
                        .map(|link| (link.href.to_owned(), link.is_note_reference))
                })
                .collect(),
            lines,
            decorations: fragment_paint::decorations(fragments)
                .iter()
                .map(|decoration| {
                    let rect = decoration.rect();
                    DecorationSnapshot {
                        x0_bits: rect.x0.to_bits(),
                        y0_bits: rect.y0.to_bits(),
                        x1_bits: rect.x1.to_bits(),
                        y1_bits: rect.y1.to_bits(),
                        color: fragment_paint::decoration_color(&decoration),
                        is_inline: decoration.is_inline(),
                    }
                })
                .collect(),
            images: fragment_paint::render_fragment_view_images(fragments)
                .iter()
                .map(|image| {
                    let clip = fragment_paint::render_image_fragment_clip(&image);
                    ImageSnapshot {
                        line_idx: fragment_paint::render_image_fragment_line(&image),
                        image_idx: image.image(),
                        offset_x_bits: fragment_paint::render_image_fragment_offset(&image)
                            .x
                            .to_bits(),
                        offset_y_bits: fragment_paint::render_image_fragment_offset(&image)
                            .y
                            .to_bits(),
                        width_bits: fragment_paint::render_image_fragment_size(&image)
                            .width
                            .to_bits(),
                        height_bits: fragment_paint::render_image_fragment_size(&image)
                            .height
                            .to_bits(),
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
        line_idx: LineId,
        image_idx: ImageId,
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
        line_links: Vec<Option<(String, bool)>>,
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
        Ok(build_snapshot(
            (document).render_view(session.glyph_resources()),
        ))
    }

    #[test]
    fn width_change_triggers_layout_only() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));

        let mut baseline = base_inputs(
            "<html><body><p>Hello world</p></body></html>",
            SourceRevision::INITIAL,
        );
        let update = session.update(baseline.clone(), &mut shaper).unwrap();
        assert_eq!(update, EarliestStage::Parse);

        let shape_calls = shaper.calls;
        baseline.layout = baseline.layout.with_viewport_width(420.0).unwrap();
        let layout_only = session.update(baseline.clone(), &mut shaper).unwrap();
        assert_eq!(layout_only, EarliestStage::Layout);
        assert_eq!(shaper.calls, shape_calls);
    }

    #[test]
    fn font_environment_change_reuses_parse_style_and_prepare() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));

        let mut update_inputs = base_inputs(
            "<html><body><p>Glyphs: abcdef</p></body></html>",
            SourceRevision::INITIAL,
        );
        let _ = session.update(update_inputs.clone(), &mut shaper).unwrap();
        let before_calls = shaper.calls;

        update_inputs.font_environment = FontEnv::INITIAL.next();
        let font_update = session.update(update_inputs, &mut shaper).unwrap();
        assert_eq!(font_update, EarliestStage::Shape);
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
            inputs.font_environment =
                FontEnvironmentRevision::next(baseline_inputs.font_environment);
            inputs
        };

        let mut shaper = SizedShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let _ = session
            .update(baseline_inputs.clone(), &mut shaper)
            .unwrap();

        let update = session.update(changed_inputs.clone(), &mut shaper).unwrap();
        assert_eq!(update, EarliestStage::Shape);

        let incremental = build_snapshot(
            (session
                .document()
                .expect("document should exist after backend-generation update"))
            .render_view(session.glyph_resources()),
        );
        let mut fresh_shaper = SizedShaper::default();
        let mut fresh_session = PipelineSession::new(Arc::new(MockProvider));
        let _ = fresh_session
            .update(changed_inputs.clone(), &mut fresh_shaper)
            .unwrap();

        let fresh = build_snapshot(
            (fresh_session
                .document()
                .expect("document should exist after full rebuild"))
            .render_view(fresh_session.glyph_resources()),
        );

        assert_eq!(incremental, fresh);
    }

    #[test]
    fn incremental_and_fresh_rebuilds_match_for_width_then_font_then_width() {
        let source = "<html><body><p>Hello world</p><ul><li>one</li><li>two</li><li>three</li></ul></body></html>";
        let mut updates = vec![
            {
                let mut base = base_inputs(source, SourceRevision::INITIAL);
                base.layout = base.layout.with_viewport_width(580.0).unwrap();
                base
            },
            {
                let mut font_up = base_inputs(source, SourceRevision::INITIAL);
                font_up.layout = font_up.layout.with_viewport_width(580.0).unwrap();
                font_up.style_environment.root_font_size = dom_values::root_font_size_new(20.0).unwrap();
                font_up.layout = font_up.layout.with_line_height(20.0).unwrap();
                font_up
            },
            {
                let mut final_input = base_inputs(source, SourceRevision::INITIAL);
                final_input.style_environment.root_font_size = dom_values::root_font_size_new(20.0).unwrap();
                final_input.layout = LayoutConstraints::new(430.0, 20.0).unwrap();
                final_input
            },
        ];

        let incremental =
            update_and_snapshot(&updates).expect("incremental pipeline should succeed");

        let fresh = {
            let mut shaper = TracingShaper::default();
            let mut session = PipelineSession::new(Arc::new(MockProvider));
            let mut inputs = updates.pop().expect("final input must exist");
            inputs.source_revision = SourceRevision::INITIAL;
            session.update(inputs, &mut shaper).unwrap();
            build_snapshot(
                (session.document().expect("document should exist"))
                    .render_view(session.glyph_resources()),
            )
        };

        assert_eq!(incremental, fresh);
    }

    #[test]
    fn width_changes_can_be_repeated_without_triggering_reparse_or_reshaping() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));

        let input = base_inputs(
            "<html><body><p>layout stability test</p></body></html>",
            SourceRevision::INITIAL,
        );
        let update = session.update(input.clone(), &mut shaper).unwrap();
        assert_eq!(update, EarliestStage::Parse);

        let after_parse_calls = shaper.calls;
        let mut next = input.clone();
        next.layout = next.layout.with_viewport_width(450.0).unwrap();
        let width_update_1 = session.update(next.clone(), &mut shaper).unwrap();
        assert_eq!(width_update_1, EarliestStage::Layout);
        assert_eq!(shaper.calls, after_parse_calls);

        let no_change = session.update(next.clone(), &mut shaper).unwrap();
        assert_eq!(no_change, EarliestStage::None);
        assert_eq!(shaper.calls, after_parse_calls);

        next.layout = next.layout.with_viewport_width(640.0).unwrap();
        let width_update_2 = session.update(next.clone(), &mut shaper).unwrap();
        assert_eq!(width_update_2, EarliestStage::Layout);
        assert_eq!(shaper.calls, after_parse_calls);
    }

    #[test]
    fn column_width_only_restyles_when_a_media_rule_changes_activation() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let source = "<html><head><style>@media (min-width: 500px) { p { font-size: 32px; } }</style></head><body><p>responsive text</p></body></html>";

        let mut inputs = base_inputs(source, SourceRevision::INITIAL);
        inputs.layout = inputs.layout.with_viewport_width(400.0).unwrap();
        inputs.style_environment.media = MediaEnvironment::screen(400.0, Some(800.0)).unwrap();
        session.update(inputs.clone(), &mut shaper).unwrap();
        let initial_shape_calls = shaper.calls;

        inputs.layout = inputs.layout.with_viewport_width(450.0).unwrap();
        inputs.style_environment.media = MediaEnvironment::screen(450.0, Some(800.0)).unwrap();
        let below_breakpoint = session.update(inputs.clone(), &mut shaper).unwrap();
        assert_eq!(below_breakpoint, EarliestStage::Layout);
        assert_eq!(shaper.calls, initial_shape_calls);

        inputs.layout = inputs.layout.with_viewport_width(600.0).unwrap();
        inputs.style_environment.media = MediaEnvironment::screen(600.0, Some(800.0)).unwrap();
        let crossing_breakpoint = session.update(inputs.clone(), &mut shaper).unwrap();
        assert_eq!(crossing_breakpoint, EarliestStage::Style);
        assert!(shaper.calls > initial_shape_calls);
        let after_crossing_calls = shaper.calls;

        inputs.layout = inputs.layout.with_viewport_width(650.0).unwrap();
        inputs.style_environment.media = MediaEnvironment::screen(650.0, Some(800.0)).unwrap();
        let above_breakpoint = session.update(inputs, &mut shaper).unwrap();
        assert_eq!(above_breakpoint, EarliestStage::Layout);
        assert_eq!(shaper.calls, after_crossing_calls);
    }

    #[test]
    fn viewport_unit_dimensions_restyle_and_reshape_without_a_media_breakpoint() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let source = "<html><head><style>p { font-size: 10vw; line-height: 10vh }</style></head><body><p>viewport text</p></body></html>";

        let mut inputs = base_inputs(source, SourceRevision::INITIAL);
        inputs.layout = inputs.layout.with_viewport_width(400.0).unwrap();
        inputs.layout = inputs.layout.with_viewport_height(Some(800.0)).unwrap();
        inputs.style_environment.media = MediaEnvironment::screen(400.0, Some(800.0)).unwrap();
        session.update(inputs.clone(), &mut shaper).unwrap();
        let initial_shape_calls = shaper.calls;

        inputs.layout = inputs.layout.with_viewport_width(450.0).unwrap();
        inputs.style_environment.media = MediaEnvironment::screen(450.0, Some(800.0)).unwrap();
        let update = session.update(inputs, &mut shaper).unwrap();

        assert_eq!(update, EarliestStage::Style);
        assert!(shaper.calls > initial_shape_calls);
    }

    #[test]
    fn simultaneous_style_and_layout_changes_choose_style_stage() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));

        let mut inputs = base_inputs(
            "<html><body><p>one</p></body></html>",
            SourceRevision::INITIAL,
        );
        let _ = session.update(inputs.clone(), &mut shaper).unwrap();

        inputs.stylesheet_revision = StylesheetRevision::next(inputs.stylesheet_revision);
        inputs.layout = inputs.layout.with_viewport_width(420.0).unwrap();
        let update = session.update(inputs, &mut shaper).unwrap();
        assert_eq!(update, EarliestStage::Prepare);
    }

    #[test]
    fn reader_user_styles_rebuild_prepared_document_without_reparsing() {
        let mut shaper = SizedShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut inputs = base_inputs(
            "<html><body><p>Text that wraps differently when enlarged.</p></body></html>",
            SourceRevision::INITIAL,
        );
        session.update(inputs.clone(), &mut shaper).unwrap();

        let baseline =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));

        inputs.user_styles = vec!["p { font-size: 42px !important; }".to_owned()];
        let update = session.update(inputs, &mut shaper).unwrap();

        let styled =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));

        assert_eq!(update, EarliestStage::Prepare);
        assert_ne!(baseline, styled);
    }

    #[test]
    fn restyling_borrows_the_canonical_dom_text_storage() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = SizedShaper::default();
        let mut inputs = base_inputs("<p>Canonical source text.</p>", SourceRevision::INITIAL);
        session.update(inputs.clone(), &mut shaper).unwrap();

        let text_pointers = |session: &PipelineSession| {
            session
                .document()
                .unwrap()
                .visible_text_runs()
                .map(|(text, _)| (text.as_ptr(), text.len()))
                .collect::<Vec<_>>()
        };
        let source = text_pointers(&session);
        assert!(!source.is_empty());
        for font_size in [24, 12, 32, 16] {
            inputs.style_environment.root_font_size = dom_values::root_font_size_new(font_size as f32).unwrap();
            let update = session.update(inputs.clone(), &mut shaper).unwrap();
            assert_eq!(update, EarliestStage::Prepare);
            assert_eq!(text_pointers(&session), source);
            assert_eq!(
                session
                    .document()
                    .unwrap()
                    .render_view(session.glyph_resources())
                    .root_font_size(),
                font_size as f32
            );
        }
        inputs
            .user_styles
            .push("p { font-weight: bold }".to_owned());
        session.update(inputs, &mut shaper).unwrap();
        assert_eq!(text_pointers(&session), source);
    }

    #[test]
    fn failed_restyling_restores_root_font_math_resources_and_published_geometry() {
        use html_dom::ImageSource;

        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        let mut inputs = base_inputs(
            "<p>ABC <math><mfrac><mi>x</mi><mn>2</mn></mfrac></math></p>",
            SourceRevision::INITIAL,
        );
        session.update(inputs.clone(), &mut shaper).unwrap();

        let before =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));
        let view = session
            .document()
            .unwrap()
            .render_view(session.glyph_resources());
        let font = view.root_font_size();
        let image = &text_paint::render_view_images(&view)[0];
        let ImageSource::MathMl { svg, baseline, .. } = &image.source else {
            panic!("formula")
        };
        let formula = (svg.as_ptr(), image.width, image.height, *baseline);
        let resources = shaper.resources.clone();
        let generation = shaper.resource_generation();
        inputs.style_environment.root_font_size = dom_values::root_font_size_new(32.0).unwrap();
        inputs.user_styles.push("p { color: red }".to_owned());
        shaper.fail_on = Some('A');
        assert!(session.update(inputs.clone(), &mut shaper).is_err());
        assert_eq!(
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources())),
            before
        );
        let view = session
            .document()
            .unwrap()
            .render_view(session.glyph_resources());
        assert_eq!(view.root_font_size(), font);
        let image = &text_paint::render_view_images(&view)[0];
        let ImageSource::MathMl { svg, baseline, .. } = &image.source else {
            panic!("formula")
        };
        assert_eq!(
            (svg.as_ptr(), image.width, image.height, *baseline),
            formula
        );
        assert_eq!(shaper.resources, resources);
        assert_eq!(shaper.resource_generation(), generation);
        shaper.fail_on = None;
        session.update(inputs, &mut shaper).unwrap();
        assert_eq!(
            session
                .document()
                .unwrap()
                .render_view(session.glyph_resources())
                .root_font_size(),
            32.0
        );
    }

    #[test]
    fn reader_root_spacing_override_removes_authored_page_insets() {
        let mut shaper = SizedShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut inputs = base_inputs(
            "<html style='margin:12px;padding:6px'><body style='margin:24px;padding:16px'><div style='margin:0;padding:0'>EDGE</div></body></html>",
            SourceRevision::INITIAL,
        );
        session.update(inputs.clone(), &mut shaper).unwrap();

        let inset = session
            .document()
            .unwrap()
            .render_view(session.glyph_resources())
            .text()
            .line(LineId::new(0))
            .unwrap()
            .point()
            .x;
        assert!(inset > 0.0);

        inputs.reader_overrides.strip_root_spacing = true;
        session.update(inputs, &mut shaper).unwrap();

        let x = session
            .document()
            .unwrap()
            .render_view(session.glyph_resources())
            .text()
            .line(LineId::new(0))
            .unwrap()
            .point()
            .x;
        assert_eq!(x, 0.0);
    }

    #[test]
    fn reader_link_foreground_overrides_nested_links_and_updates_with_theme() {
        let mut shaper = TracingShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut inputs = base_inputs(
            "<html><head><style>a[href]::before {content: 'G'; color: orange}</style></head><body><p style='color: green'>B <a href='#note' style='color: red'>L<span style='color: blue'>N</span></a> <a id='note' style='color: magenta'>A</a></p></body></html>",
            SourceRevision::INITIAL,
        );
        let theme = crate::ThemeAdaptation {
            ink: 0xeeeeeeff,
            surface: 0x222222ff,
        };
        inputs.reader_overrides.theme = Some(theme);
        for accent in [0x466853ff, 0xc6a25cff] {
            inputs.reader_overrides.link_foreground = Some(accent);
            let calls = shaper.calls;
            let update = session.update(inputs.clone(), &mut shaper).unwrap();
            for (character, expected) in [
                ('L', accent),
                ('N', accent),
                ('G', accent),
                ('B', computed_style::theme_adaptation_adapt(theme, 0x008000ff)),
                ('A', computed_style::theme_adaptation_adapt(theme, 0xff00ffff)),
            ] {
                let text = session
                    .document()
                    .unwrap()
                    .render_view(session.glyph_resources())
                    .text();
                let index = (0..text.glyph_count())
                    .find(|&index| {
                        text_paint::glyph_at(text, index)
                            .and_then(|glyph| text_paint::glyph_metric(text, glyph))
                            .is_some_and(|metric| metric.ch() == character)
                    })
                    .unwrap();
                assert_eq!(
                    text_paint::paint_color_at(text, index as u32),
                    Some(expected)
                );
                if update == EarliestStage::Paint {
                    assert_eq!(shaper.calls, calls, "color updates must not reshape");
                }
            }
        }
    }

    #[test]
    fn canonical_font_size_scales_fixed_text_on_font_size_updates_without_compounding() {
        let mut shaper = SizedShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut inputs = base_inputs(
            "<!doctype html><html><body><p>A</p><p style='font-size: 12px'>B<span style='font-size: 50%'>D</span></p><p style='font-size: 32px'>C</p></body></html>",
            SourceRevision::INITIAL,
        );
        inputs.reader_overrides.canonical_font_size = Some(16.0);
        let mut enlarged = None;
        for desired in [16, 24, 8, 24, 16] {
            inputs.style_environment.root_font_size = dom_values::root_font_size_new(desired as f32).unwrap();
            inputs.layout = inputs.layout.with_line_height(f64::from(desired)).unwrap();
            session.update(inputs.clone(), &mut shaper).unwrap();
            let ratio = desired as f32 / 16.0;
            for (character, canonical) in [('A', 16.0), ('B', 12.0), ('C', 32.0), ('D', 6.0)] {
                let expected = canonical * ratio;
                assert!(
                    shaper.cache.contains_key(&(character, expected.to_bits())),
                    "{character} should shape at {expected}px for a requested size of {desired}px"
                );
            }
            if desired == 24 {
                let snapshot = build_snapshot(
                    (session.document().unwrap()).render_view(session.glyph_resources()),
                );
                if let Some(previous) = enlarged.as_ref() {
                    assert_eq!(&snapshot, previous);
                } else {
                    enlarged = Some(snapshot);
                }
            }
        }
        // Changing only the canonical reference must invalidate cached styles.
        inputs.reader_overrides.canonical_font_size = Some(8.0);
        let update = session.update(inputs.clone(), &mut shaper).unwrap();
        assert_eq!(update, EarliestStage::Prepare);
        assert!(shaper.cache.contains_key(&('B', 24.0f32.to_bits())));
        inputs.reader_overrides.canonical_font_size = None;
        session.update(inputs, &mut shaper).unwrap();
        assert!(shaper.cache.contains_key(&('B', 12.0f32.to_bits())));
    }

    #[test]
    fn changes_borrow_unchanged_source_and_replacements_still_invalidate_parse() {
        let mut inputs = base_inputs("<p>first chapter</p>", SourceRevision::INITIAL);
        inputs.user_styles.push("p { color: red }".to_owned());
        let source_pointer = inputs.source.as_ptr();
        let style_pointer = inputs.user_styles[0].as_ptr();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        session.update(inputs, &mut shaper).unwrap();
        assert_eq!(session.inputs().unwrap().source.as_ptr(), source_pointer);

        let mut layout = session.inputs().unwrap().layout;
        layout = layout.with_viewport_width(120.0).unwrap();
        let update = session
            .apply_changes(
                PipelineChanges {
                    layout: Some(layout),
                    ..Default::default()
                },
                &mut shaper,
            )
            .unwrap();
        assert_eq!(update, EarliestStage::Layout);
        assert_eq!(session.inputs().unwrap().source.as_ptr(), source_pointer);
        assert_eq!(
            session.inputs().unwrap().user_styles[0].as_ptr(),
            style_pointer
        );

        let replacement = "<p>second chapter</p>".to_owned();
        let replacement_pointer = replacement.as_ptr();
        let update = session
            .apply_changes(
                PipelineChanges {
                    source: Some(replacement),
                    ..Default::default()
                },
                &mut shaper,
            )
            .unwrap();
        assert_eq!(update, EarliestStage::Parse);
        assert_eq!(
            session.inputs().unwrap().source.as_ptr(),
            replacement_pointer
        );
        assert!(
            glyph_text((session.document().unwrap()).render_view(session.glyph_resources()))
                .contains("second chapter")
        );
    }

    #[test]
    fn failed_changes_restore_owned_inputs_document_and_backend_resources() {
        let mut inputs = base_inputs("<p>original document</p>", SourceRevision::INITIAL);
        inputs.user_styles.push("p { color: red }".to_owned());
        inputs.image_metrics.set(0, 40, 60);
        let source_pointer = inputs.source.as_ptr();
        let style_pointer = inputs.user_styles[0].as_ptr();
        let original_inputs = inputs.clone();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        session.update(inputs, &mut shaper).unwrap();

        let original_document =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));
        let original_resources = shaper.resources.clone();
        let mut metrics = ImageMetrics::default();
        metrics.set(0, 900, 800);
        shaper.fail_on = Some('!');
        assert!(
            session
                .apply_changes(
                    PipelineChanges {
                        source: Some("<p>replacement!</p>".to_owned()),
                        base_uri: Some("replacement.html".to_owned()),
                        user_styles: Some(vec!["p { color: blue }".to_owned()]),
                        image_metrics: Some(metrics),
                        reader_overrides: Some(crate::ReaderStyleOverrides {
                            foreground: Some(0x123456ff),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    &mut shaper
                )
                .is_err()
        );
        let committed = session.inputs().unwrap();
        assert_eq!(committed, &original_inputs);
        assert_eq!(committed.source.as_ptr(), source_pointer);
        assert_eq!(committed.user_styles[0].as_ptr(), style_pointer);
        assert_eq!(committed.image_metrics.get(0), Some((40, 60)));
        assert_eq!(
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources())),
            original_document
        );
        assert_eq!(shaper.resources, original_resources);
        shaper.fail_on = None;
        let update = session
            .apply_changes(PipelineChanges::default(), &mut shaper)
            .unwrap();
        assert_eq!(update, EarliestStage::None);
        assert_eq!(session.inputs().unwrap().source.as_ptr(), source_pointer);
    }

    #[test]
    fn changes_preserve_media_query_invalidation_and_paint_reuse() {
        let inputs = base_inputs(
            "<style>@media (max-width: 200px) { p { color: red } }</style><p>media</p>",
            SourceRevision::INITIAL,
        );
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        session.update(inputs, &mut shaper).unwrap();
        let source_pointer = session.inputs().unwrap().source.as_ptr();
        let mut layout = session.inputs().unwrap().layout;
        layout = layout.with_viewport_width(100.0).unwrap();
        let mut environment = session.inputs().unwrap().style_environment;
        environment.media = crate::MediaEnvironment::screen(100.0, None).unwrap();
        let update = session
            .apply_changes(
                PipelineChanges {
                    layout: Some(layout),
                    style_environment: Some(environment),
                    ..Default::default()
                },
                &mut shaper,
            )
            .unwrap();
        assert_eq!(update, EarliestStage::Style);
        assert!(
            session
                .document()
                .unwrap()
                .render_view(session.glyph_resources())
                .text()
                .glyph_count()
                > 0
        );
        let calls = shaper.calls;
        let update = session
            .apply_changes(
                PipelineChanges {
                    reader_overrides: Some(crate::ReaderStyleOverrides {
                        foreground: Some(0x112233ff),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                &mut shaper,
            )
            .unwrap();
        assert_eq!(update, EarliestStage::Paint);
        assert_eq!(shaper.calls, calls);
        assert_eq!(session.inputs().unwrap().source.as_ptr(), source_pointer);
    }

    #[test]
    fn changes_reject_empty_sessions_and_preserve_validated_layout() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        assert!(
            session
                .apply_changes(PipelineChanges::default(), &mut shaper)
                .is_err()
        );
        assert!(session.inputs().is_none());
        session
            .prepare_through_style(base_inputs("<p>prepared</p>", SourceRevision::INITIAL))
            .unwrap();
        let original = session.inputs().unwrap().layout;
        assert!(original.with_viewport_width(f64::NAN).is_err());
        assert_eq!(session.inputs().unwrap().layout, original);
        session
            .apply_changes(PipelineChanges::default(), &mut shaper)
            .unwrap();
        assert!(session.document().is_some());
    }

    #[test]
    fn note_flow_changes_are_reported_at_preparation_and_applied() {
        let inputs = base_inputs(
            "<p>Main text</p><aside epub:type='footnote'>unicornnote</aside>",
            SourceRevision::INITIAL,
        );
        let mut next = inputs.clone();
        next.note_flow = crate::engine::NoteFlow::Excluded;
        assert_ne!(next, inputs);
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        session.update(inputs, &mut shaper).unwrap();
        let has_note_text = |session: &PipelineSession| {
            !search_queries::render_text_view_find_matches(session
                .render_view()
                .unwrap()
                .text(), "unicornnote", crate::TextSearchOptions::default())
                .is_empty()
        };
        assert!(has_note_text(&session));
        assert_eq!(
            session.update(next, &mut shaper).unwrap(),
            EarliestStage::Prepare
        );
        assert!(!has_note_text(&session));
    }

    #[test]
    fn paint_updates_restore_publication_colors_and_match_fresh_geometry() {
        let source = "<html><head><style>a::before {content:'G'; color:orange} p::first-letter {color:purple}</style></head><body><div style='background:#fafafa; border:2px solid red; outline:1px dotted blue'><p id='anchor' style='color:green'>B <a href='#anchor'>L<span style='color:blue'>N</span></a></p><p>Other text wraps across several lines in this paragraph.</p></div></body></html>";
        let mut inputs = base_inputs(source, SourceRevision::INITIAL);
        inputs.layout = inputs.layout.with_viewport_width(140.0).unwrap();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        session.update(inputs.clone(), &mut shaper).unwrap();

        let original =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));

        let glyph_pointer = session
            .document()
            .unwrap()
            .render_view(session.glyph_resources())
            .text()
            .glyph_slice(
                0..session
                    .document()
                    .unwrap()
                    .render_view(session.glyph_resources())
                    .text()
                    .glyph_count() as u32,
            )
            .unwrap()
            .as_contiguous()
            .unwrap()
            .as_ptr();
        let generation = shaper.resource_generation();
        let calls = shaper.calls;
        let preferences = [
            crate::ReaderStyleOverrides {
                link_foreground: Some(0x123456ff),
                ..Default::default()
            },
            crate::ReaderStyleOverrides {
                foreground: Some(0xeeeeeeff),
                background: Some(0x101010ff),
                theme: Some(crate::ThemeAdaptation {
                    ink: 0xddddddff,
                    surface: 0x222222ff,
                }),
                ..Default::default()
            },
            crate::ReaderStyleOverrides {
                theme: Some(crate::ThemeAdaptation {
                    ink: 0x111111ff,
                    surface: 0xffffffff,
                }),
                ..Default::default()
            },
            crate::ReaderStyleOverrides::default(),
        ];
        for overrides in preferences {
            inputs.reader_overrides = overrides;
            let update = session.update(inputs.clone(), &mut shaper).unwrap();
            assert_eq!(update, EarliestStage::Paint);
            assert_eq!(shaper.calls, calls);
            assert_eq!(shaper.resource_generation(), generation);
            assert_eq!(
                session
                    .document()
                    .unwrap()
                    .render_view(session.glyph_resources())
                    .text()
                    .glyph_slice(
                        0..session
                            .document()
                            .unwrap()
                            .render_view(session.glyph_resources())
                            .text()
                            .glyph_count() as u32
                    )
                    .unwrap()
                    .as_contiguous()
                    .unwrap()
                    .as_ptr(),
                glyph_pointer
            );
            let mut fresh_session = PipelineSession::new(Arc::new(MockProvider));
            let mut fresh_shaper = TracingShaper::default();
            fresh_session
                .update(inputs.clone(), &mut fresh_shaper)
                .unwrap();

            let mut updated = build_snapshot(
                (session.document().unwrap()).render_view(session.glyph_resources()),
            );

            let mut fresh = build_snapshot(
                (fresh_session.document().unwrap()).render_view(fresh_session.glyph_resources()),
            );
            // Semantic paint traversal is independent of physical decoration storage.
            let order = |d: &DecorationSnapshot| {
                (
                    d.x0_bits,
                    d.y0_bits,
                    d.x1_bits,
                    d.y1_bits,
                    d.color,
                    d.is_inline,
                )
            };
            updated.decorations.sort_by_key(order);
            fresh.decorations.sort_by_key(order);
            assert_eq!(updated, fresh);
        }
        assert_eq!(
            original.lines,
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()))
                .lines
        );
        assert_eq!(
            text_paint::paint_color_at(
                session
                    .document()
                    .unwrap()
                    .render_view(session.glyph_resources())
                    .text(),
                0
            ),
            Some(0x800080ff)
        );
        inputs.layout = inputs.layout.with_viewport_width(200.0).unwrap();
        assert_eq!(
            session.update(inputs.clone(), &mut shaper).unwrap(),
            EarliestStage::Layout
        );
        inputs.reader_overrides.link_foreground = Some(0xabcdef88);
        assert_eq!(
            session.update(inputs.clone(), &mut shaper).unwrap(),
            EarliestStage::Paint
        );
        inputs.source = "<p>different chapter</p>".into();
        assert_eq!(
            session.update(inputs, &mut shaper).unwrap(),
            EarliestStage::Parse
        );
    }

    #[test]
    fn paint_preferences_retain_shaping_for_specialized_layout_and_reject_stale_resources() {
        for source in [
            "<table><tr><td>table</td></tr></table>",
            "<p style='position:relative'>positioned</p>",
        ] {
            let mut inputs = base_inputs(source, SourceRevision::INITIAL);
            let mut session = PipelineSession::new(Arc::new(MockProvider));
            let mut shaper = TracingShaper::default();
            session.update(inputs.clone(), &mut shaper).unwrap();
            let calls = shaper.calls;
            inputs.reader_overrides.foreground = Some(0xabcdef88);
            assert_eq!(
                session.update(inputs.clone(), &mut shaper).unwrap(),
                EarliestStage::Paint
            );
            assert_eq!(shaper.calls, calls);
            shaper.reset();
            inputs.reader_overrides.foreground = Some(0x112233ff);
            let update = session.update(inputs, &mut shaper).unwrap();
            assert!(update < EarliestStage::Paint);
        }
    }

    #[test]
    fn failed_font_and_paint_update_keeps_published_styles_and_resources() {
        let source =
            "<style>p::first-line {font-size:32px}</style><p style='position:relative'>ABC</p>";
        let mut inputs = base_inputs(source, SourceRevision::INITIAL);
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TransactionalShaper::default();
        session.update(inputs.clone(), &mut shaper).unwrap();

        let snapshot =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));
        let resources = shaper.resources.clone();
        let generation = shaper.resource_generation();
        shaper.fail_on = Some('A');
        inputs.reader_overrides.foreground = Some(0x123456ff);
        inputs.font_environment = inputs.font_environment.next();
        assert!(session.update(inputs, &mut shaper).is_err());
        assert_eq!(
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources())),
            snapshot
        );
        assert_eq!(shaper.resources, resources);
        assert_eq!(shaper.resource_generation(), generation);
        assert_eq!(
            session
                .cache
                .inputs
                .as_ref()
                .unwrap()
                .reader_overrides
                .foreground,
            None
        );
        assert_eq!(
            text_paint::paint_color_at(
                session
                    .document()
                    .unwrap()
                    .render_view(session.glyph_resources())
                    .text(),
                0
            ),
            Some(0x000000ff)
        );
    }

    #[test]
    fn paint_preferences_have_independent_cache_keys() {
        let original = base_inputs("<p>text</p>", SourceRevision::INITIAL);
        let mut painted = original.clone();
        painted.reader_overrides.foreground = Some(0xabcdef88);
        painted.reader_overrides.link_foreground = Some(0xabcdef88);
        painted.reader_overrides.background = Some(0x112233ff);
        painted.reader_overrides.theme = Some(crate::ThemeAdaptation {
            ink: 0xddddddff,
            surface: 0x222222ff,
        });
        assert_eq!(original.prepared_cache_key(), painted.prepared_cache_key());
        assert_eq!(original.shaped_cache_key(), painted.shaped_cache_key());
        assert_eq!(original.layout_cache_key(), painted.layout_cache_key());
        painted.reader_overrides.minimum_font_size = Some(32.0);
        assert_ne!(original.prepared_cache_key(), painted.prepared_cache_key());
    }

    #[test]
    fn published_stage_keys_share_payloads_and_remain_independent_of_mutable_inputs() {
        use crate::allocation_test_support::count_allocations;

        let mut inputs = base_inputs("<p>text</p>", SourceRevision::INITIAL);
        inputs.base_uri = "book/chapter.html".into();
        inputs.user_styles = vec!["p { color:red }".into()];
        inputs.reader_overrides.font_family = Some("Reader".into());
        let expected = inputs.layout_cache_key();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        session
            .update(inputs.clone(), &mut TracingShaper::default())
            .unwrap();
        let layout = &session.cache.document.published().unwrap().key;
        let shaped = &session.cache.shaped.as_ref().unwrap().key;
        let parsed = &session.cache.parsed.as_ref().unwrap().key;
        assert_eq!(layout, &expected);
        assert!(Arc::ptr_eq(
            &layout.shaped.prepared.reader_overrides,
            &shaped.prepared.reader_overrides
        ));
        assert!(Arc::ptr_eq(
            &layout.shaped.prepared.parsed.base_uri,
            &parsed.base_uri
        ));
        let (snapshot, allocations) = count_allocations(|| layout.clone());
        assert_eq!(allocations, 0, "stage-key clones share owned payloads");

        inputs.base_uri.push_str("?changed");
        inputs.user_styles[0].push_str("p { color:blue }");
        inputs.reader_overrides.font_family = Some("Other".into());
        assert_eq!(snapshot, expected);
        assert_ne!(snapshot, inputs.layout_cache_key());
    }

    #[test]
    fn retained_paint_program_matches_full_cascade_for_specialized_and_clipped_layouts() {
        let sources = [
            include_str!("../../../testdata/pipeline/paint/01-clipped.html"),
            include_str!("../../../testdata/pipeline/paint/02-collapsed-table.html"),
            include_str!("../../../testdata/pipeline/paint/03-separate-table.html"),
            include_str!("../../../testdata/pipeline/paint/04-positioned-flex-grid.html"),
            include_str!("../../../testdata/pipeline/paint/05-pseudo-styles.html"),
            include_str!("../../../testdata/pipeline/paint/06-math-and-links.html"),
            include_str!("../../../testdata/pipeline/paint/07-inline-atomic.html"),
            include_str!("../../../testdata/pipeline/paint/08-linked-table-columns.html"),
        ];
        let theme = crate::ThemeAdaptation {
            ink: 0xeeeeeeff,
            surface: 0x222222ff,
        };
        for source in sources {
            let mut inputs = base_inputs(source, SourceRevision::INITIAL);
            inputs.layout = inputs.layout.with_viewport_width(140.0).unwrap();
            let mut session = PipelineSession::new(Arc::new(MockProvider));
            let mut shaper = TracingShaper::default();
            session.update(inputs.clone(), &mut shaper).unwrap();
            let calls = shaper.calls;

            let line_clips: Vec<_> = session
                .document()
                .unwrap()
                .render_view(session.glyph_resources())
                .text()
                .lines()
                .iter()
                .map(|line| {
                    text_paint::line_overflow_clip(
                        session
                            .document()
                            .unwrap()
                            .render_view(session.glyph_resources())
                            .text(),
                        line.id(),
                    )
                })
                .collect();
            for palette in [
                crate::ReaderStyleOverrides {
                    foreground: Some(0x123456ff),
                    background: Some(0xeeeeddff),
                    ..Default::default()
                },
                crate::ReaderStyleOverrides {
                    theme: Some(theme),
                    link_foreground: Some(0x99aabbff),
                    ..Default::default()
                },
                crate::ReaderStyleOverrides::default(),
            ] {
                inputs.reader_overrides = palette.clone();
                let update = session.update(inputs.clone(), &mut shaper).unwrap();
                assert_eq!(update, EarliestStage::Paint, "{source}");
                assert_eq!(
                    shaper.calls, calls,
                    "palette changes must not enter shaping"
                );

                let candidate = session.document().unwrap();
                let mut factory = DocumentFactory::new();
                factory.set_reader_overrides(palette);
                let prepared = factory.parse_with_new_pipeline(source, None);
                let mut reference_shaper = TracingShaper::default();
                let mut reference_resources = GlyphResourceStore::default();
                let reference = prepared
                    .shape(&mut reference_shaper, &mut reference_resources)
                    .unwrap()
                    .layout_with_metrics_and_shaper(
                        inputs.layout,
                        &inputs.image_metrics,
                        &mut reference_shaper,
                        &mut reference_resources,
                    )
                    .unwrap();
                let mut actual = build_snapshot((candidate).render_view(session.glyph_resources()));
                let mut expected = build_snapshot((&reference).render_view(&reference_resources));
                actual.glyphs.clear();
                expected.glyphs.clear();
                assert_eq!(actual, expected, "full-cascade paint parity: {source}");
                let decorations = |view: crate::engine::RenderView<'_>| {
                    fragment_paint::decorations(view.fragments())
                        .iter()
                        .map(|decoration| format!("{decoration:?}"))
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    decorations(candidate.render_view(session.glyph_resources())),
                    decorations(reference.render_view(&reference_resources)),
                    "decoration clips, patterns and layers: {source}"
                );
                assert_eq!(
                    text_paint::render_view_canvas_background_color(&candidate
                        .render_view(session.glyph_resources())),
                    text_paint::render_view_canvas_background_color(&reference
                        .render_view(&reference_resources))
                );
                let text = candidate.render_view(session.glyph_resources()).text();
                assert_eq!(
                    text.lines()
                        .iter()
                        .map(|line| text_paint::line_overflow_clip(text, line.id()))
                        .collect::<Vec<_>>(),
                    line_clips
                );
                for index in 0..text.glyph_count() {
                    let actual_color = text_paint::paint_color_at(text, index as u32).unwrap();
                    let expected_color = text_paint::paint_color_at(
                        reference.render_view(&reference_resources).text(),
                        index as u32,
                    )
                    .unwrap();
                    assert_eq!(
                        actual_color, expected_color,
                        "effective glyph color at {index}: {source}"
                    );
                }
            }
        }
    }

    #[test]
    fn excluded_notes_keep_link_paint_bindings_for_non_box_table_styles() {
        let source = "<p>Main flow without links</p><aside id='n' epub:type='footnote'><a href='x'><table><colgroup style='background:currentColor'><col></colgroup><tr><td>note</td></tr></table></a></aside>";
        for palette in [
            crate::ReaderStyleOverrides {
                foreground: Some(0x123456ff),
                ..Default::default()
            },
            crate::ReaderStyleOverrides {
                link_foreground: Some(0x99aabbff),
                ..Default::default()
            },
        ] {
            let mut inputs = base_inputs(source, SourceRevision::INITIAL);
            inputs.note_flow = crate::engine::NoteFlow::Excluded;
            let mut session = PipelineSession::new(Arc::new(MockProvider));
            let mut shaper = TracingShaper::default();
            session.update(inputs.clone(), &mut shaper).unwrap();
            inputs.reader_overrides = palette.clone();
            assert_eq!(
                session.update(inputs, &mut shaper).unwrap(),
                EarliestStage::Paint
            );
            let constraints = crate::engine::LayoutConstraints::new(200.0, 20.0).unwrap();
            let note = session.layout_note("n", constraints, &mut shaper).unwrap().unwrap();
            let mut factory = DocumentFactory::new();
            factory.set_reader_overrides(palette.clone());
            let root = factory.parse_with_new_pipeline(source, None);
            let prepared = root.scoped_to_element_id("n").unwrap();
            let mut reference_shaper = TracingShaper::default();
            let mut reference_resources = GlyphResourceStore::default();
            let reference = prepared
                .shape(&mut reference_shaper, &mut reference_resources)
                .unwrap()
                .layout(constraints, &reference_resources)
                .expect("layout requires the shaper-aware API for first-line styles");

            let mut actual = build_snapshot(
                note.render_view(session.document().unwrap(), session.glyph_resources())
                    .unwrap(),
            );
            let mut expected = build_snapshot((&reference).render_view(&reference_resources));
            actual.glyphs.clear();
            expected.glyphs.clear();
            assert_eq!(actual, expected);
            let decorations = |view: crate::engine::RenderView<'_>| {
                fragment_paint::decorations(view.fragments())
                    .iter()
                    .map(|decoration| format!("{decoration:?}"))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                decorations(
                    note.render_view(session.document().unwrap(), session.glyph_resources())
                        .unwrap()
                ),
                decorations(reference.render_view(&reference_resources))
            );
            let expected_color = palette.link_foreground.or(palette.foreground).unwrap();

            let text = note
                .render_view(session.document().unwrap(), session.glyph_resources())
                .unwrap()
                .text();
            for index in 0..text.glyph_count() {
                assert_eq!(
                    text_paint::paint_color_at(text, index as u32),
                    Some(expected_color)
                );
            }
        }
    }

    #[test]
    fn simultaneous_width_and_paint_changes_retarget_the_retained_layout() {
        let mut inputs = base_inputs(
            "<p><a href='x'>some linked words that wrap across lines</a></p>",
            SourceRevision::INITIAL,
        );
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        session.update(inputs.clone(), &mut shaper).unwrap();
        let calls = shaper.calls;
        inputs.layout = inputs.layout.with_viewport_width(10.0).unwrap();
        inputs.reader_overrides.foreground = Some(0xabcdef88);
        inputs.reader_overrides.link_foreground = Some(0x998877ff);
        assert_eq!(
            session.update(inputs.clone(), &mut shaper).unwrap(),
            EarliestStage::Layout
        );
        assert_eq!(shaper.calls, calls);

        let doc = session.document().unwrap();
        let text = doc.render_view(session.glyph_resources()).text();
        assert!(text.line_count() > 1);
        assert_eq!(text_paint::paint_color_at(text, 0), Some(0x998877ff));
        let mut fresh = PipelineSession::new(Arc::new(MockProvider));
        fresh.update(inputs, &mut TracingShaper::default()).unwrap();
        assert_eq!(
            build_snapshot((doc).render_view(session.glyph_resources())),
            build_snapshot((fresh.document().unwrap()).render_view(fresh.glyph_resources()))
        );
    }

    #[test]
    fn paragraph_controls_match_fresh_cascades_and_restore_book_layout() {
        use crate::layout::InlinePlanGeneration;
        use html_dom::RootFontSize;
        for source in [
            "<html><body style='margin:0;font-size:1.29167em'><span style='font-size:0.77419em'>An opening passage outside a paragraph.</span><p style='margin:1em 0;text-indent:7%'><span style='font-size:0.77419em'>Words that wrap across several lines for paragraph controls.</span></p><p style='font-size:12px;margin:8px 0'>Another paragraph follows the first.</p></body></html>",
            "<html><body style='margin:0;font-size:1.29167em;text-indent:7%;text-align:justify'><div><span style='font-size:.77419em'>An opening passage with enough words to wrap.<br><br>A second passage with more words for wrapping.</span><br> <br>A final passage.</div></body></html>",
        ] {
            let mut inputs = base_inputs(source, SourceRevision::INITIAL);
            inputs.layout = inputs.layout.with_viewport_width(100.0).unwrap();
            inputs.style_environment.root_font_size = dom_values::root_font_size_new(24.0).unwrap();
            inputs.layout = inputs.layout.with_line_height(24.0).unwrap();
            inputs.reader_overrides.canonical_font_size = Some(16.0);
            let mut session = PipelineSession::new(Arc::new(MockProvider));
            let mut shaper = SizedShaper::default();
            session.update(inputs.clone(), &mut shaper).unwrap();
            let snapshot = |document: crate::RenderView<'_>| {
                let mut snapshot = build_snapshot(document);
                snapshot.glyphs.clear();
                snapshot
            };

            let book = snapshot(session.render_view().unwrap());
            for (spacing, indent) in [
                (Some(0.7), Some(1.5)),
                (Some(0.0), None),
                (None, Some(0.0)),
                (None, None),
            ] {
                inputs.reader_overrides.paragraph_spacing_em = spacing;
                inputs.reader_overrides.first_line_indent_em = indent;
                let update = session.update(inputs.clone(), &mut shaper).unwrap();
                assert_eq!(update, EarliestStage::Prepare);
                for width in [100.0, 140.0, 100.0] {
                    inputs.layout = inputs.layout.with_viewport_width(width).unwrap();
                    session.update(inputs.clone(), &mut shaper).unwrap();
                    let mut factory = DocumentFactory::new();
                    factory.set_root_font_size(dom_values::root_font_size_new(24.0).unwrap());
                    factory.set_reader_overrides(inputs.reader_overrides.clone());
                    let prepared = factory.parse_with_new_pipeline(source, None);
                    let mut reference_shaper = SizedShaper::default();
                    let mut reference_resources = GlyphResourceStore::default();
                    let reference = prepared
                        .shape(&mut reference_shaper, &mut reference_resources)
                        .unwrap()
                        .layout_with_metrics_and_shaper(
                            inputs.layout,
                            &inputs.image_metrics,
                            &mut reference_shaper,
                            &mut reference_resources,
                        )
                        .unwrap();
                    assert_eq!(
                        snapshot(session.render_view().unwrap()),
                        snapshot(reference.render_view(&reference_resources)),
                        "cached/fresh mismatch for spacing {spacing:?}, indentation {indent:?}, width {width}"
                    );
                }
            }
            assert_eq!(snapshot(session.render_view().unwrap()), book);
        }
    }

    #[test]
    fn typed_reader_overrides_apply_without_injecting_css() {
        let mut shaper = SizedShaper::default();
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut inputs = base_inputs(
            "<html><body><p style='font-size: 9px; line-height: 1'>Text that wraps after reader overrides.</p></body></html>",
            SourceRevision::INITIAL,
        );
        session.update(inputs.clone(), &mut shaper).unwrap();

        let baseline =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));

        inputs.reader_overrides = crate::ReaderStyleOverrides {
            font_family: Some("serif".to_owned()),
            text_align: Some(crate::TextAlign::Justify),
            line_height: Some(1.8),
            canonical_font_size: None,
            minimum_font_size: Some(28.0),
            foreground: Some(0x112233ff),
            link_foreground: None,
            background: Some(0xfefefeff),
            strip_root_spacing: false,
            theme: None,
            ..Default::default()
        };
        assert!(inputs.user_styles.is_empty());
        let update = session.update(inputs, &mut shaper).unwrap();

        let overridden =
            build_snapshot((session.document().unwrap()).render_view(session.glyph_resources()));

        assert_eq!(update, EarliestStage::Prepare);
        assert_ne!(baseline, overridden);
    }

    #[test]
    fn stylesheet_replacement_and_restoration_maps_to_style_stage_and_preserves_fresh_rebuild_equivalence()
     {
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
        session
            .update(update_inputs.clone(), &mut shaper)
            .expect("baseline update should succeed");

        let baseline = {
            let document = session.document().expect("document should exist");
            build_snapshot((document).render_view(session.glyph_resources()))
        };

        provider.put_bytes("style.css", b"p { font-size: 36px; }".to_vec());
        update_inputs.stylesheet_revision =
            StylesheetRevision::next(update_inputs.stylesheet_revision);
        let style_update = session
            .update(update_inputs.clone(), &mut shaper)
            .expect("style replacement update should succeed");
        assert_eq!(style_update, EarliestStage::Prepare);

        let replaced = {
            let document = session.document().expect("document should exist");
            build_snapshot((document).render_view(session.glyph_resources()))
        };
        assert_ne!(baseline, replaced);

        provider.put_bytes("style.css", b"p { font-size: 9px; }".to_vec());
        update_inputs.stylesheet_revision =
            StylesheetRevision::next(update_inputs.stylesheet_revision);
        let restore = session
            .update(update_inputs, &mut shaper)
            .expect("stylesheet restoration update should succeed");
        assert_eq!(restore, EarliestStage::Prepare);

        let restored = {
            let document = session.document().expect("document should exist");
            build_snapshot((document).render_view(session.glyph_resources()))
        };
        assert_eq!(baseline, restored);
    }

    #[test]
    fn pipeline_input_equality_includes_source_and_base_uri() {
        let inputs = base_inputs(
            "<html><body><p>x</p></body></html>",
            SourceRevision::INITIAL,
        );
        assert_eq!(inputs, inputs.clone());

        let mut different_source = inputs.clone();
        different_source.source = "<html><body><p>y</p></body></html>".into();
        assert_ne!(
            different_source, inputs,
            "source text must remain part of input identity even when its revision is unchanged"
        );

        let mut different_base = inputs.clone();
        different_base.base_uri = "other/chapter.xhtml".to_owned();
        assert_ne!(
            different_base, inputs,
            "resource resolution base must remain part of input identity even when its revision is unchanged"
        );
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
        session
            .update(inputs, &mut shaper)
            .expect("well-formed XHTML pipeline");

        let document = session.document().expect("laid out document");
        let green = fragment_paint::decorations(
            document.render_view(session.glyph_resources()).fragments(),
        )
        .iter()
        .find(|fragment| fragment_paint::decoration_color(&fragment) == 0x008000FF)
        .expect("absolute green background");
        assert!(fragment_paint::decoration_is_in_positioned_layer(&green));
        assert!(!fragment_paint::decoration_is_in_negative_positioned_layer(
            &green
        ));
        assert!(
            !fragment_paint::decoration_is_in_independent_positioned_layer(&green),
            "auto-z absolute and relative boxes share the source-ordered positioned stream"
        );
        assert!(fragment_paint::render_decoration_line(&green).is_none());
        let root = document.render_view(session.glyph_resources());
        let images = fragment_paint::render_fragment_view_images(root.fragments())
            .iter()
            .collect::<Vec<_>>();
        assert!(
            !images.is_empty(),
            "the regression fixture must produce a replaced-image fragment"
        );
        for image in images {
            let line = root
                .text()
                .line(fragment_paint::render_image_fragment_line(&image))
                .expect("image owning line");
            assert!(
                text_paint::line_is_in_positioned_layer(&line),
                "the relative ancestor keeps its normal descendants in positioned paint"
            );
            assert!(
                !text_paint::line_is_in_independent_positioned_layer(&line),
                "the preceding image remains in the common source-ordered positioned stream"
            );
        }
    }
}
