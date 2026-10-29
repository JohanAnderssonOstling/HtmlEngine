use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use crate::engine::{
    GlyphResourceGeneration, GlyphResourceStore, GlyphShaper as LayoutGlyphShaper, LaidOutDocument,
    LayoutConstraints as LayoutConstraintsOutput, NoteLayout, PreparedDocument, RenderView,
};
use html_dom::RootFontSize;
use html_parse::{MarkupSyntax, ParsedHtml, parse_document};
use html_resources::ResourceProvider;
use html_style::{FontFace, MediaMatchKey, MediaQuerySet};

use crate::parser::{DocumentFactory, ResourceContext};
use crate::types::{
    LayoutCacheKey, ParsedCacheKey, PreparedCacheKey, RenderOnlyCacheKey, ShapedCacheKey,
};
use crate::types::{LayoutConstraints, ResourceRevision, StylesheetRevision};
use crate::{
    EarliestStage, MediaEnvironment, PipelineChanges, PipelineError, PipelineInputs,
    PipelineUpdate, Reuse, ReuseReport,
};

#[derive(Default)]
pub struct PipelineCacheState {
    pub inputs: Option<PipelineInputs>,
    /// Parse validity and source encoding; the syntax tree is consumed into the DOM.
    pub parsed: Option<CacheEntry<ParsedCacheKey, Option<String>>>,
    /// Cache metadata; published preparation lives in the document owner.
    pub prepared: Option<CacheEntry<PreparedCacheKey, ()>>,
    /// Owned preparation while a worker has not yet attached a glyph backend.
    pub pending_prepared: Option<PreparedDocument<'static>>,
    pub media_queries: Option<MediaQuerySet>,
    /// Active declarations shared with the compiled style program.
    pub font_faces: Arc<[FontFace]>,
    pub media_match_key: Option<MediaMatchKey>,
    /// Shaping validity only; glyph data has exactly one document owner.
    pub shaped: Option<CacheEntry<ShapedCacheKey, ()>>,
    pub laid_out: Option<CacheEntry<LayoutCacheKey, LaidOutDocument<'static>>>,
}

/// Only the comparisons needed while proposed inputs are being evaluated.
/// Large input values remain owned in one place throughout an update.
#[derive(Clone, Copy)]
struct PreviousInputs {
    source_matches: bool,
    media: MediaEnvironment,
    color_preferences_changed: bool,
}

pub struct CacheEntry<K, V> {
    pub key: K,
    pub value: V,
}

impl<K: PartialEq, V> CacheEntry<K, V> {
    fn matches(&self, key: &K) -> bool {
        &self.key == key
    }
}

impl PipelineCacheState {
    fn publish_layout_key(&mut self, key: LayoutCacheKey) {
        self.prepared = Some(CacheEntry {
            key: key.shaped.prepared.clone(),
            value: (),
        });
        self.shaped = Some(CacheEntry {
            key: key.shaped.clone(),
            value: (),
        });
        self.laid_out
            .as_mut()
            .expect("published document owner")
            .key = key;
    }

    fn parsed_matches(&self, key: &ParsedCacheKey) -> bool {
        self.parsed.as_ref().is_some_and(|entry| entry.matches(key))
    }

    fn prepared_matches(&self, key: &PreparedCacheKey) -> bool {
        self.prepared
            .as_ref()
            .is_some_and(|entry| entry.matches(key))
    }

    fn shaped_matches(&self, key: &ShapedCacheKey) -> bool {
        self.shaped.as_ref().is_some_and(|entry| entry.matches(key))
    }

    fn laid_out_matches(&self, key: &LayoutCacheKey) -> bool {
        self.laid_out
            .as_ref()
            .is_some_and(|entry| entry.matches(key))
    }
}

pub struct PipelineSession {
    glyph_resources: GlyphResourceStore,
    cached_provider: Arc<RevisionCachedResourceProvider>,
    cached_resource_key: Option<(ResourceRevision, StylesheetRevision)>,
    factory: DocumentFactory,
    cache: PipelineCacheState,
    active_resource_generation: Option<GlyphResourceGeneration>,
}

/// Renderer-bound half of a pipeline session. The underlying
/// [`PipelineSession`] can be prepared on a worker before a backend is
/// attached; this view keeps the backend and its glyph ledger together for
/// every shaping, relayout, and note operation.
pub struct ActivePipelineSession<'a, S: LayoutGlyphShaper> {
    session: &'a mut PipelineSession,
    glyph_shaper: &'a mut S,
}

/// An attached pipeline that owns both the renderer backend and the document
/// cache. A worker may build a [`PipelineSession`] through style preparation
/// and move it into this type on the thread that owns the backend.
pub struct OwnedActivePipelineSession<S: LayoutGlyphShaper> {
    session: PipelineSession,
    glyph_shaper: S,
}

impl<S: LayoutGlyphShaper> OwnedActivePipelineSession<S> {
    pub fn update(&mut self, inputs: PipelineInputs) -> Result<PipelineUpdate, PipelineError> {
        self.session.update(inputs, &mut self.glyph_shaper)
    }

    pub fn inputs(&self) -> Option<&PipelineInputs> {
        self.session.inputs()
    }

    pub fn apply_changes(
        &mut self,
        changes: PipelineChanges,
    ) -> Result<PipelineUpdate, PipelineError> {
        self.session.apply_changes(changes, &mut self.glyph_shaper)
    }

    pub fn rehydrate_glyphs(&mut self) -> Result<(), PipelineError> {
        self.session.rehydrate_glyphs(&mut self.glyph_shaper)
    }

    pub fn layout_note(
        &mut self,
        id: &str,
        constraints: LayoutConstraintsOutput,
    ) -> Option<NoteLayout> {
        self.session
            .layout_note(id, constraints, &mut self.glyph_shaper)
    }

    pub fn font_faces(&self) -> &[FontFace] {
        self.session.font_faces()
    }

    pub fn document(&self) -> Option<&LaidOutDocument<'static>> {
        self.session.document()
    }

    pub fn document_mut(&mut self) -> Option<&mut LaidOutDocument<'static>> {
        self.session.document_mut()
    }

    pub fn glyph_resources(&self) -> &GlyphResourceStore {
        self.session.glyph_resources()
    }

    pub fn render_view(&self) -> Option<RenderView<'_>> {
        self.session.render_view()
    }

    pub fn document_mut_and_resources(
        &mut self,
    ) -> Option<(&mut LaidOutDocument<'static>, &GlyphResourceStore)> {
        self.session.document_mut_and_resources()
    }

    pub fn note_ids(&self) -> Vec<&str> {
        self.session.note_ids()
    }

    pub fn into_parts(self) -> (PipelineSession, S) {
        (self.session, self.glyph_shaper)
    }
}

impl<S: LayoutGlyphShaper> ActivePipelineSession<'_, S> {
    pub fn update(&mut self, inputs: PipelineInputs) -> Result<PipelineUpdate, PipelineError> {
        self.session.update(inputs, self.glyph_shaper)
    }

    pub fn inputs(&self) -> Option<&PipelineInputs> {
        self.session.inputs()
    }

    pub fn apply_changes(
        &mut self,
        changes: PipelineChanges,
    ) -> Result<PipelineUpdate, PipelineError> {
        self.session.apply_changes(changes, self.glyph_shaper)
    }

    pub fn rehydrate_glyphs(&mut self) -> Result<(), PipelineError> {
        self.session.rehydrate_glyphs(self.glyph_shaper)
    }

    pub fn layout_note(
        &mut self,
        id: &str,
        constraints: LayoutConstraintsOutput,
    ) -> Option<NoteLayout> {
        self.session.layout_note(id, constraints, self.glyph_shaper)
    }

    pub fn font_faces(&self) -> &[FontFace] {
        self.session.font_faces()
    }

    pub fn document(&self) -> Option<&LaidOutDocument<'static>> {
        self.session.document()
    }

    pub fn document_mut(&mut self) -> Option<&mut LaidOutDocument<'static>> {
        self.session.document_mut()
    }

    pub fn glyph_resources(&self) -> &GlyphResourceStore {
        self.session.glyph_resources()
    }

    pub fn render_view(&self) -> Option<RenderView<'_>> {
        self.session.render_view()
    }

    pub fn document_mut_and_resources(
        &mut self,
    ) -> Option<(&mut LaidOutDocument<'static>, &GlyphResourceStore)> {
        self.session.document_mut_and_resources()
    }

    pub fn note_ids(&self) -> Vec<&str> {
        self.session.note_ids()
    }
}

impl PipelineSession {
    pub fn glyph_resources(&self) -> &GlyphResourceStore {
        &self.glyph_resources
    }
    pub fn glyph_resources_mut(&mut self) -> &mut GlyphResourceStore {
        &mut self.glyph_resources
    }

    pub fn render_view(&self) -> Option<RenderView<'_>> {
        self.document()?.try_render_view(&self.glyph_resources)
    }

    pub fn document_mut_and_resources(
        &mut self,
    ) -> Option<(&mut LaidOutDocument<'static>, &GlyphResourceStore)> {
        Some((
            &mut self.cache.laid_out.as_mut()?.value,
            &self.glyph_resources,
        ))
    }

    /// Active `@font-face` declarations from the last successful preparation
    /// or update. URLs use the declaring stylesheet's resource-provider base.
    /// After `prepare_through_style`, hosts can load these faces before calling
    /// `rehydrate_glyphs`. The engine never reads font bytes.
    pub fn font_faces(&self) -> &[FontFace] {
        &self.cache.font_faces
    }

    pub fn into_active<S: LayoutGlyphShaper>(
        mut self,
        mut glyph_shaper: S,
    ) -> OwnedActivePipelineSession<S> {
        self.invalidate_stale_renderer_state(&mut glyph_shaper);
        OwnedActivePipelineSession {
            session: self,
            glyph_shaper,
        }
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
        if self.active_resource_generation.as_ref() != Some(&glyph_shaper.resource_generation()) {
            self.cache.shaped = None;
            // Published geometry remains available to borrowed readers after
            // failure or backend replacement. The generation gate prevents
            // treating it as reusable renderer state until shaping succeeds.
            self.active_resource_generation = None;
        }
    }

    pub fn new(provider: Arc<dyn ResourceProvider>) -> Self {
        let cached_provider = Arc::new(RevisionCachedResourceProvider::new(provider.clone()));
        Self {
            glyph_resources: GlyphResourceStore::default(),
            cached_provider,
            cached_resource_key: None,
            factory: DocumentFactory::new(),
            cache: PipelineCacheState::default(),
            active_resource_generation: None,
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
            f64::from(requested_inputs.style_environment.root_font_size),
        );

        self.cache.font_faces = self.factory.take_font_faces();
        self.cache.inputs = Some(requested_inputs);
        self.cache.parsed = Some(CacheEntry {
            key: parsed_key,
            value: encoding,
        });
        self.cache.prepared = Some(CacheEntry {
            key: prepared_key,
            value: (),
        });
        self.cache.pending_prepared = Some(prepared);
        self.cache.media_queries = Some(media_queries);
        self.cache.media_match_key = Some(media_match_key);
        self.cache.shaped = None;
        self.cache.laid_out = None;
        self.active_resource_generation = None;
        Ok(())
    }

    fn prepare_document(
        &mut self,
        requested_inputs: &PipelineInputs,
    ) -> Result<(PreparedDocument<'static>, Option<String>, MediaQuerySet), PipelineError> {
        let parsed = self.parse(&requested_inputs.source, requested_inputs.markup_syntax)?;
        let user_styles = requested_inputs
            .user_styles
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
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
            .build_pipeline_from_owned_parsed(parsed, &user_styles);
        let prepared = prepared
            .into_reader_paint_overrides(&requested_inputs.reader_overrides)
            .map_err(|error| PipelineError(error.to_string()))?;
        Ok((prepared, encoding, media_queries))
    }

    fn configure_factory(&mut self, requested_inputs: &PipelineInputs) {
        self.refresh_resource_cache(
            requested_inputs.resource_revision,
            requested_inputs.stylesheet_revision,
        );
        self.factory.set_root_font_size(to_root_font_size(
            requested_inputs.style_environment.root_font_size,
        ));
        self.factory
            .set_media_environment(requested_inputs.style_environment.media);
        self.factory
            .set_reader_overrides(requested_inputs.reader_overrides.layout_overrides());
        self.factory.set_note_flow(requested_inputs.note_flow);
    }

    /// Borrow the last committed inputs. The session is their only owner.
    pub fn inputs(&self) -> Option<&PipelineInputs> {
        self.cache.inputs.as_ref()
    }

    pub fn update(
        &mut self,
        requested_inputs: PipelineInputs,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<PipelineUpdate, PipelineError> {
        let previous_inputs = self.cache.inputs.take();
        let previous = previous_inputs.as_ref().map(|inputs| PreviousInputs {
            source_matches: inputs.same_source(&requested_inputs),
            media: inputs.style_environment.media,
            color_preferences_changed: !inputs
                .reader_overrides
                .same_paint_preferences(&requested_inputs.reader_overrides),
        });
        let mut resources = std::mem::take(&mut self.glyph_resources);
        let result = self.update_inner(&requested_inputs, previous, glyph_shaper, &mut resources);
        self.glyph_resources = resources;
        self.cache.inputs = if result.is_ok() {
            Some(requested_inputs)
        } else {
            previous_inputs
        };
        self.invalidate_stale_renderer_state(glyph_shaper);
        result
    }

    /// Apply only changed fields, borrowing all unchanged document inputs.
    /// On failure, the proposed values are swapped back out before returning.
    pub fn apply_changes(
        &mut self,
        mut changes: PipelineChanges,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<PipelineUpdate, PipelineError> {
        let mut inputs =
            self.cache.inputs.take().ok_or_else(|| {
                PipelineError("cannot update an empty pipeline session".to_owned())
            })?;
        let previous = PreviousInputs {
            source_matches: changes
                .source
                .as_ref()
                .is_none_or(|source| source == &inputs.source),
            media: inputs.style_environment.media,
            color_preferences_changed: changes.reader_overrides.as_ref().is_some_and(|overrides| {
                !overrides.same_paint_preferences(&inputs.reader_overrides)
            }),
        };
        changes.swap_with(&mut inputs);
        let mut resources = std::mem::take(&mut self.glyph_resources);
        let result = self.update_inner(&inputs, Some(previous), glyph_shaper, &mut resources);
        self.glyph_resources = resources;
        if result.is_err() {
            changes.swap_with(&mut inputs);
        }
        self.cache.inputs = Some(inputs);
        self.invalidate_stale_renderer_state(glyph_shaper);
        result
    }

    fn update_inner(
        &mut self,
        requested_inputs: &PipelineInputs,
        previous: Option<PreviousInputs>,
        glyph_shaper: &mut impl LayoutGlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<PipelineUpdate, PipelineError> {
        self.configure_factory(requested_inputs);
        let constraints = to_layout_constraints(requested_inputs.layout)?;
        let render_only_key = requested_inputs.render_only_cache_key();
        let layout_key = &render_only_key.layout;
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
                self.cache.parsed_matches(parsed_key)
                    && previous.source_matches
                    && self.cache.laid_out_matches(layout_key)
                    && !media_match_changed
                    && self.active_resource_generation.as_ref()
                        == Some(&glyph_shaper.resource_generation())
            });
        if can_repaint {
            self.cache
                .laid_out
                .as_mut()
                .expect("paint update has layout")
                .value
                .repaint_with_reader_overrides(&requested_inputs.reader_overrides, resources)
                .map_err(|error| PipelineError(error.to_string()))?;
            self.cache.publish_layout_key(layout_key.clone());
            let stage = EarliestStage::Paint;
            let reused = Reuse::reused(stage);
            return Ok(PipelineUpdate {
                stage,
                anchors_preserved: true,
                report: ReuseReport {
                    parsed: reused,
                    styled: reused,
                    prepared: reused,
                    shaped: reused,
                    laid_out: reused,
                },
            });
        }
        let stage = self.earliest_uncached_stage(
            parsed_key,
            prepared_key,
            shaped_key,
            layout_key,
            source_matches,
            media_match_changed,
            self.active_resource_generation.as_ref() == Some(&glyph_shaper.resource_generation()),
        );

        if stage <= EarliestStage::Prepare
            && self.cache.parsed_matches(parsed_key)
            && source_matches
        {
            return self.restyle_cached(
                requested_inputs,
                constraints,
                stage,
                glyph_shaper,
                resources,
            );
        }

        let report = if stage <= EarliestStage::Layout {
            let parsed_reused = stage != EarliestStage::Parse;
            let prepared_reused = stage > EarliestStage::Prepare;
            let shaped_reused = stage == EarliestStage::Layout;
            let mut queries = None;
            if shaped_reused || (prepared_reused && self.cache.pending_prepared.is_none()) {
                let layout = &mut self
                    .cache
                    .laid_out
                    .as_mut()
                    .expect("cached document owner")
                    .value;
                let result = if color_preferences_changed {
                    layout.update_with_reader_overrides(
                        &requested_inputs.reader_overrides,
                        constraints,
                        &requested_inputs.image_metrics,
                        !shaped_reused,
                        glyph_shaper,
                        resources,
                    )
                } else if shaped_reused {
                    layout.relayout_with_metrics_and_shaper(
                        constraints,
                        &requested_inputs.image_metrics,
                        glyph_shaper,
                        resources,
                    )
                } else {
                    layout.reshape_and_layout_with_metrics_and_shaper(
                        constraints,
                        &requested_inputs.image_metrics,
                        glyph_shaper,
                        resources,
                    )
                };
                result.map_err(|error| PipelineError(error.to_string()))?;
            } else {
                let (prepared, encoding) = if prepared_reused {
                    (
                        self.cache
                            .pending_prepared
                            .take()
                            .expect("prepared worker result"),
                        None,
                    )
                } else {
                    let (prepared, encoding, media) = self.prepare_document(requested_inputs)?;
                    queries = Some(media);
                    (prepared, encoding)
                };
                let painted = if prepared_reused && color_preferences_changed {
                    Some(
                        prepared
                            .with_reader_paint_overrides(&requested_inputs.reader_overrides)
                            .map_err(|error| PipelineError(error.to_string()))?,
                    )
                } else {
                    None
                };
                let result = Self::shape_and_layout(
                    painted.as_ref().unwrap_or(&prepared),
                    constraints,
                    &requested_inputs.image_metrics,
                    glyph_shaper,
                    resources,
                );

                let laid_out = match result {
                    Ok(document) => {
                        drop(painted);
                        prepared.with_layout(document)
                    }
                    Err(error) => {
                        if prepared_reused {
                            self.cache.pending_prepared = Some(prepared);
                        }
                        return Err(error);
                    }
                };
                // Move publication ownership to the laid-out document. The
                // prepared-stage cache retains only its validity key.

                self.cache.laid_out = Some(CacheEntry {
                    key: layout_key.clone(),
                    value: laid_out,
                });
                if !parsed_reused {
                    self.cache.parsed = Some(CacheEntry {
                        key: parsed_key.clone(),
                        value: encoding,
                    });
                    self.cache.font_faces = self.factory.take_font_faces();
                }
            }
            self.cache.publish_layout_key(layout_key.clone());
            self.active_resource_generation = Some(glyph_shaper.resource_generation());
            if let Some(media) = queries {
                self.cache.media_queries = Some(media);
            }
            let reuse = |reused| {
                if reused {
                    Reuse::reused(stage)
                } else {
                    Reuse::recomputed(stage)
                }
            };
            ReuseReport {
                parsed: reuse(parsed_reused),
                styled: reuse(prepared_reused),
                prepared: reuse(prepared_reused),
                shaped: reuse(shaped_reused),
                laid_out: reuse(false),
            }
        } else {
            let reused = Reuse::reused(stage);
            ReuseReport {
                parsed: reused,
                styled: reused,
                prepared: reused,
                shaped: reused,
                laid_out: reused,
            }
        };

        self.cache.media_match_key = self.cache.media_queries.as_ref().map(|queries| {
            queries.match_key(
                requested_inputs.style_environment.media,
                f64::from(requested_inputs.style_environment.root_font_size),
            )
        });
        self.validate_cache_against_requested(&render_only_key);
        Ok(PipelineUpdate {
            stage,
            report,
            anchors_preserved: stage >= EarliestStage::Layout,
        })
    }

    fn restyle_cached(
        &mut self,
        inputs: &PipelineInputs,
        constraints: LayoutConstraintsOutput,
        stage: EarliestStage,
        shaper: &mut impl LayoutGlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<PipelineUpdate, PipelineError> {
        let encoding = self
            .cache
            .parsed
            .as_ref()
            .and_then(|entry| entry.value.as_deref());
        let css: Vec<_> = inputs.user_styles.iter().map(String::as_str).collect();
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
            let (styles, media) = factory.resolve_canonical_dom(document, encoding, &css);
            queries = Some(media);
            styles
        };
        let root_font = to_root_font_size(inputs.style_environment.root_font_size);
        let result = if let Some(prepared) = &mut self.cache.pending_prepared {
            prepared
                .reprepare_and_layout(
                    root_font,
                    inputs.note_flow,
                    &inputs.reader_overrides,
                    resolve,
                    constraints,
                    &inputs.image_metrics,
                    shaper,
                    resources,
                )
                .map(|layout| Some(layout.into_detached()))
        } else {
            self.cache
                .laid_out
                .as_mut()
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
                    resources,
                )
                .map(|()| None)
        };
        match result {
            Ok(layout) => {
                if let Some(layout) = layout {
                    self.cache.laid_out = Some(CacheEntry {
                        key: inputs.layout_cache_key(),
                        value: self
                            .cache
                            .pending_prepared
                            .take()
                            .expect("prepared owner")
                            .with_layout(layout),
                    });
                }
                self.cache.pending_prepared = None;
                self.cache.publish_layout_key(inputs.layout_cache_key());
                self.active_resource_generation = Some(shaper.resource_generation());
                self.cache.font_faces = self.factory.take_font_faces();
                self.cache.media_queries = queries;
                self.cache.media_match_key = self.cache.media_queries.as_ref().map(|queries| {
                    queries.match_key(
                        inputs.style_environment.media,
                        f64::from(inputs.style_environment.root_font_size),
                    )
                });
                Ok(PipelineUpdate {
                    stage,
                    anchors_preserved: false,
                    report: ReuseReport {
                        parsed: Reuse::reused(stage),
                        styled: Reuse::recomputed(stage),
                        prepared: Reuse::recomputed(stage),
                        shaped: Reuse::recomputed(stage),
                        laid_out: Reuse::recomputed(stage),
                    },
                })
            }
            Err(error) => Err(PipelineError(error.to_string())),
        }
    }

    /// Rebuild the shaped and laid-out stages for a cached document.
    ///
    /// Glyph shapers may keep renderer-specific data indexed by `GlyphId`. That
    /// data is reset whenever another document is shaped, so restoring a cached
    /// document must rehydrate it even when its parse and style inputs are still
    /// valid. Publishes the result in place; borrow it through `document()`
    /// instead of retaining a second owned layout.
    pub fn rehydrate_glyphs(
        &mut self,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Result<(), PipelineError> {
        let mut resources = std::mem::take(&mut self.glyph_resources);
        let result = self.rehydrate_glyphs_inner(glyph_shaper, &mut resources);
        self.glyph_resources = resources;
        self.invalidate_stale_renderer_state(glyph_shaper);
        result
    }

    fn rehydrate_glyphs_inner(
        &mut self,
        glyph_shaper: &mut impl LayoutGlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), PipelineError> {
        let inputs = self.cache.inputs.as_ref().ok_or_else(|| {
            PipelineError("cannot rehydrate an empty pipeline session".to_owned())
        })?;
        let constraints = to_layout_constraints(inputs.layout)?;
        if let Some(prepared) = self.cache.pending_prepared.as_ref() {
            let laid_out = Self::shape_and_layout(
                prepared,
                constraints,
                &inputs.image_metrics,
                glyph_shaper,
                resources,
            )?;
            let prepared = self.cache.pending_prepared.take().expect("prepared owner");
            self.cache.laid_out = Some(CacheEntry {
                key: inputs.layout_cache_key(),
                value: prepared.with_layout(laid_out),
            });
        } else {
            self.cache
                .laid_out
                .as_mut()
                .ok_or_else(|| {
                    PipelineError(
                        "cannot rehydrate a pipeline session without preparation".to_owned(),
                    )
                })?
                .value
                .reshape_and_layout_with_metrics_and_shaper(
                    constraints,
                    &inputs.image_metrics,
                    glyph_shaper,
                    resources,
                )
                .map_err(|error| PipelineError(error.to_string()))?;
            self.cache
                .laid_out
                .as_mut()
                .expect("rehydrated document owner")
                .key = inputs.layout_cache_key();
        }
        self.cache.shaped = Some(CacheEntry {
            key: inputs.shaped_cache_key(),
            value: (),
        });
        self.active_resource_generation = Some(glyph_shaper.resource_generation());
        Ok(())
    }

    fn earliest_uncached_stage(
        &self,
        parsed_key: &ParsedCacheKey,
        prepared_key: &PreparedCacheKey,
        shaped_key: &ShapedCacheKey,
        layout_key: &LayoutCacheKey,
        source_matches: bool,
        media_match_changed: bool,
        active_resources_match: bool,
    ) -> EarliestStage {
        // Source text is compared with cached inputs so cache keys stay small
        // even when callers replace text without advancing its revision.
        if !source_matches || !self.cache.parsed_matches(parsed_key) {
            EarliestStage::Parse
        } else if !self.cache.prepared_matches(prepared_key) {
            EarliestStage::Prepare
        } else if media_match_changed {
            EarliestStage::Style
        } else if !active_resources_match || !self.cache.shaped_matches(shaped_key) {
            EarliestStage::Shape
        } else if !self.cache.laid_out_matches(layout_key) {
            EarliestStage::Layout
        } else {
            EarliestStage::Paint
        }
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

    fn parse(&mut self, source: &str, syntax: MarkupSyntax) -> Result<ParsedHtml, PipelineError> {
        parse_document(source, syntax).map_err(|error| PipelineError(error.to_string()))
    }

    fn media_match_changed(
        &self,
        requested_inputs: &PipelineInputs,
        previous_media: Option<MediaEnvironment>,
    ) -> bool {
        let Some(previous_media) = previous_media else {
            return false;
        };
        if previous_media == requested_inputs.style_environment.media {
            return false;
        }
        self.media_dependencies_changed(
            requested_inputs.style_environment.media,
            requested_inputs.style_environment.root_font_size,
        )
    }

    pub fn media_environment_change_affects_style(
        &self,
        media: crate::MediaEnvironment,
        root_font_size: u32,
    ) -> bool {
        let Some(previous_inputs) = self.cache.inputs.as_ref() else {
            return true;
        };
        if previous_inputs.style_environment.media == media {
            return false;
        }
        self.media_dependencies_changed(media, root_font_size)
    }

    fn media_dependencies_changed(&self, media: MediaEnvironment, root_font_size: u32) -> bool {
        let (Some(media_queries), Some(previous_key)) = (
            self.cache.media_queries.as_ref(),
            self.cache.media_match_key.as_ref(),
        ) else {
            return true;
        };
        media_queries.uses_viewport_units()
            || media_queries.match_key(media, f64::from(root_font_size)) != *previous_key
    }

    fn shape_and_layout(
        prepared: &PreparedDocument,
        constraints: LayoutConstraintsOutput,
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
            .map_err(|error| PipelineError(error.to_string()))
    }

    fn validate_cache_against_requested(&self, render_only_key: &RenderOnlyCacheKey) {
        if let Some(entry) = self.cache.laid_out.as_ref() {
            debug_assert_eq!(&entry.key, &render_only_key.layout);
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
    pub fn layout_note(
        &mut self,
        id: &str,
        constraints: LayoutConstraintsOutput,
        glyph_shaper: &mut impl LayoutGlyphShaper,
    ) -> Option<NoteLayout> {
        let mut resources = std::mem::take(&mut self.glyph_resources);
        let result = self.layout_note_inner(id, constraints, glyph_shaper, &mut resources);
        self.glyph_resources = resources;
        self.invalidate_stale_renderer_state(glyph_shaper);
        result
    }

    fn layout_note_inner(
        &mut self,
        id: &str,
        constraints: LayoutConstraintsOutput,
        glyph_shaper: &mut impl LayoutGlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Option<NoteLayout> {
        let inputs = self.cache.inputs.as_ref()?;
        self.cache.laid_out.as_ref()?;
        if self.active_resource_generation.as_ref() != Some(&glyph_shaper.resource_generation()) {
            return None;
        }
        let scoped = self
            .cache
            .laid_out
            .as_ref()?
            .value
            .scoped_to_element_id(id)?;
        // Appended to the page's renderer resources rather than shaped as a
        // document of its own: a note's glyphs have to coexist with the page's,
        // which are still referenced by what is on screen. Every note appends,
        // so the seed is whatever the last one left behind.
        let laid_out = scoped
            .shape_and_layout_into_active_resources(
                constraints,
                &inputs.image_metrics,
                glyph_shaper,
                resources,
            )
            .ok()?
            .into_detached();
        Some(
            scoped
                .with_layout(laid_out)
                .into_note(&self.cache.laid_out.as_ref()?.value),
        )
    }

    /// Ids of the note bodies in the current document, in document order.
    /// Empty before the first successful update.
    pub fn note_ids(&self) -> Vec<&str> {
        if let Some(prepared) = &self.cache.pending_prepared {
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
        self.cache.laid_out.as_ref().map(|entry| &entry.value)
    }

    pub fn document_mut(&mut self) -> Option<&mut LaidOutDocument<'static>> {
        self.cache.laid_out.as_mut().map(|entry| &mut entry.value)
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
        if let Some(bytes) = self
            .bytes
            .lock()
            .expect("resource byte cache lock")
            .get(uri)
            .cloned()
        {
            return Ok(bytes);
        }
        let bytes = self.inner.read_bytes(uri)?;
        self.bytes
            .lock()
            .expect("resource byte cache lock")
            .insert(uri.to_owned(), bytes.clone());
        Ok(bytes)
    }

    fn read_string(&self, uri: &str) -> io::Result<String> {
        if let Some(string) = self
            .strings
            .lock()
            .expect("resource string cache lock")
            .get(uri)
            .cloned()
        {
            return Ok(string);
        }
        let string = self.inner.read_string(uri)?;
        self.strings
            .lock()
            .expect("resource string cache lock")
            .insert(uri.to_owned(), string.clone());
        Ok(string)
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

fn to_root_font_size(root_font_size: u32) -> RootFontSize {
    RootFontSize::new(root_font_size as f32).unwrap_or_default()
}

fn to_layout_constraints(
    requested: LayoutConstraints,
) -> Result<LayoutConstraintsOutput, PipelineError> {
    let constraints = LayoutConstraintsOutput::new(requested.viewport_width, requested.line_height)
        .map_err(|error| PipelineError(error.to_string()))?
        .with_viewport_height(requested.viewport_height)
        .map_err(|error| PipelineError(error.to_string()))?;
    Ok(constraints
        .with_image_sizing_policy(requested.image_sizing_policy)
        .with_text_composition_policy(requested.text_composition_policy))
}

#[cfg(test)]
mod tests {
    use crate::{GlyphRequest, ShapedGlyph};

    use super::{PipelineSession, to_layout_constraints};
    use crate::engine::{
        GlyphId, GlyphMetric, GlyphResourceStore, GlyphShaper as LayoutGlyphShaper,
    };
    use crate::{DocumentFactory, PipelineChanges};
    use crate::{
        EarliestStage, FontEnvironmentRevision, FontEnvironmentRevision as FontEnv,
        ImageMetricsRevision, LayoutConstraints, MarkupSyntax, MediaEnvironment,
        PaintSettingsRevision, PipelineChange, PipelineError, PipelineInputs, ResourceRevision,
        Reuse, SourceRevision, StyleEnvironment, StylesheetRevision,
    };
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
        cache: HashMap<(char, u32), GlyphId>,
        generation: crate::engine::GlyphResourceGeneration,
        append_checkpoint: Option<(
            HashMap<(char, u32), GlyphId>,
            crate::engine::GlyphResourceGeneration,
        )>,
    }

    impl LayoutGlyphShaper for TracingShaper {
        fn shape_glyph_run(
            &mut self,
            request: crate::engine::TextRunShapeRequest<'_>,
            glyphs: &mut [GlyphId],
            resources: &mut crate::engine::GlyphResourceStore,
        ) -> Result<Option<crate::engine::ShapedTextRun>, crate::engine::ShapeError> {
            let style = request.style();
            for (glyph, ch) in glyphs.iter_mut().zip(request.text().chars()) {
                *glyph = resources.shape_glyph(
                    self,
                    GlyphRequest {
                        character: ch,
                        font_size: style.font_size(),
                        font_weight: style.font_weight(),
                        font_slant: style.font_slant(),

                        font_family: style.font_family(),
                    },
                )?;
            }
            Ok(None)
        }

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
            let key = (request.character, request.font_size.to_bits());

            let idx = u32::try_from(self.cache.len()).expect("glyph ids must fit");
            let glyph = idx;
            let metric = GlyphMetric::try_new(request.character, 1.0, 1.0, 0.0)
                .expect("deterministic metrics must be valid");
            if let Some(&glyph) = self.cache.get(&key) {
                return Ok(ShapedGlyph::from_metric(glyph, metric));
            }

            self.cache.insert(key, glyph);
            Ok(ShapedGlyph::from_metric(glyph, metric))
        }
    }

    #[derive(Clone, Default)]
    struct SizedShaper {
        cache: HashMap<(char, u32), GlyphId>,
        generation: crate::engine::GlyphResourceGeneration,
        append_checkpoint: Option<(
            HashMap<(char, u32), GlyphId>,
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
            let key = (request.character, request.font_size.to_bits());

            let idx = u32::try_from(self.cache.len()).expect("glyph ids must fit");
            let glyph = idx;
            let width = request.font_size.max(1.0) * 0.75;
            let metric = GlyphMetric::try_new(
                request.character,
                width,
                request.font_size,
                request.font_size * 0.2,
            )
            .expect("deterministic metrics must be valid");
            if let Some(&glyph) = self.cache.get(&key) {
                return Ok(ShapedGlyph::from_metric(glyph, metric));
            }

            self.cache.insert(key, glyph);
            Ok(ShapedGlyph::from_metric(glyph, metric))
        }
    }

    #[derive(Default)]
    struct TransactionalShaper {
        cache: HashMap<(char, u32), GlyphId>,
        resources: Vec<char>,
        generation: crate::engine::GlyphResourceGeneration,
        previous_document: Option<(
            HashMap<(char, u32), GlyphId>,
            Vec<char>,
            crate::engine::GlyphResourceGeneration,
        )>,
        previous_append: Option<(
            HashMap<(char, u32), GlyphId>,
            Vec<char>,
            crate::engine::GlyphResourceGeneration,
        )>,
        fail_on: Option<char>,
        fail_measurement: bool,
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
            if self.fail_on == Some(request.character) {
                return Err(crate::engine::ShapeError::unregistered_glyph_id(
                    u32::MAX,
                    self.cache.len(),
                ));
            }
            let key = (request.character, request.font_size.to_bits());
            let metric = GlyphMetric::try_new(request.character, 1.0, 1.0, 0.0)
                .expect("test metrics must be valid");
            if let Some(&glyph) = self.cache.get(&key) {
                return Ok(ShapedGlyph::from_metric(glyph, metric));
            }
            let glyph = self.cache.len() as GlyphId;
            self.cache.insert(key, glyph);
            self.resources.push(request.character);
            Ok(ShapedGlyph::from_metric(glyph, metric))
        }

        fn measure_line(
            &mut self,
            request: crate::engine::TextShapeRequest<'_>,
        ) -> Result<Option<crate::engine::ShapedTextRun>, crate::engine::ShapeError> {
            if self.fail_measurement {
                return Err(crate::engine::ShapeError::unregistered_glyph_id(
                    u32::MAX,
                    request.text_range().len(),
                ));
            }
            Ok(None)
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
            layout: LayoutConstraints {
                viewport_width: 600.0,
                viewport_height: None,
                line_height: StyleEnvironment::default().root_font_size as f64,
                image_sizing_policy: crate::engine::ImageSizingPolicy::WebCompatible,
                text_composition_policy: crate::engine::TextCompositionPolicy::WebCompatible,
            },
            image_metrics: Default::default(),
            paint: PaintSettingsRevision::INITIAL,
        }
    }

    fn glyph_text<'a>(document: impl Into<crate::engine::RenderView<'a>>) -> String {
        let text = document.into().text();
        (0..text.glyph_count())
            .filter_map(|index| {
                text.glyph_at(index)
                    .and_then(|glyph| text.glyph_metric(glyph))
                    .map(|metric| metric.ch())
            })
            .collect()
    }

    #[test]
    fn hyphenation_language_fallback_survives_preparation_and_style_rebuilds() {
        for (publication, markup, expected) in [
            (None, "", true),
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
            inputs.layout.viewport_width = 8.0;
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
                    (0..text.line_count()).any(|index| text.hyphen_for_line(index).is_some()),
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
        assert_eq!(update.stage, EarliestStage::Shape);
        assert!(session.cache.pending_prepared.is_none());

        let text = session
            .document()
            .unwrap()
            .render_view(session.glyph_resources())
            .text();
        assert_eq!(text.paint_color_at(0), Some(0xeeeeeeff));
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
        assert!(session.cache.pending_prepared.is_some());
        let mut shaper = TracingShaper::default();
        session
            .rehydrate_glyphs(&mut shaper)
            .expect("shape after host font registration");
        assert!(session.document().is_some());
        assert!(
            session.cache.pending_prepared.is_none(),
            "published preparation has one owner"
        );
        assert!(
            session.cache.prepared.is_some() && session.cache.shaped.is_some(),
            "stage caches retain validity keys"
        );
        assert_eq!(session.attach(&mut shaper).font_faces()[0].family, "Reader");
        assert_eq!(provider.read_count(), 0);
        assert_eq!(session.into_active(shaper).font_faces()[0].family, "Reader");
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
        inputs.layout.viewport_width = 800.0;
        assert!(session.media_environment_change_affects_style(
            inputs.style_environment.media,
            inputs.style_environment.root_font_size
        ));
        let update = session
            .update(inputs.clone(), &mut shaper)
            .expect("font-only media change");
        assert_eq!(update.stage, EarliestStage::Style);
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
            .expect("the held-back note must lay out on demand");
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
                .is_none(),
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
        let note = session.layout_note("n", constraints, &mut shaper).unwrap();

        let owner = session.document().unwrap();
        let view = note.render_view(owner, session.glyph_resources()).unwrap();
        assert!(std::ptr::eq(
            view.images().as_ptr(),
            owner
                .render_view(session.glyph_resources())
                .images()
                .as_ptr()
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
        let note = session.layout_note("n", constraints, &mut shaper).unwrap();
        inputs.reader_overrides.foreground = Some(0xeeeeeeff);
        session.update(inputs, &mut shaper).unwrap();
        assert!(
            note.render_view(session.document().unwrap(), session.glyph_resources())
                .is_none()
        );
        let note = session.layout_note("n", constraints, &mut shaper).unwrap();
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
    fn pipeline_selects_strict_xml_for_xhtml_uris() {
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        let malformed =
            "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>broken</body></html>";

        let mut xhtml_inputs = base_inputs(malformed, SourceRevision::INITIAL);
        xhtml_inputs.base_uri = "reference.xht".to_owned();
        xhtml_inputs.markup_syntax = MarkupSyntax::Xml;
        let error = session
            .update(xhtml_inputs, &mut shaper)
            .expect_err("XHTML must not use HTML recovery");
        assert!(error.to_string().contains("XML parse error"));

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

        let colors = session
            .document()
            .expect("the XML pipeline produced a document")
            .render_view(session.glyph_resources())
            .fragments()
            .decorations()
            .iter()
            .map(|fragment| fragment.color())
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
        inputs.layout.viewport_width = 800.0;
        inputs.layout.viewport_height = Some(600.0);
        session
            .update(inputs, &mut shaper)
            .expect("well-formed XHTML pipeline");

        let view = session
            .document()
            .expect("XHTML document")
            .render_view(session.glyph_resources());
        let size_for = |id| {
            let index = (0..view.boxes().len())
                .find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some(id))
                .expect("identified layout box");
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
        session
            .update(inputs.clone(), &mut shaper)
            .expect("initial XHTML pipeline");
        inputs.layout.viewport_height = Some(600.0);
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
            let index = (0..view.boxes().len())
                .find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some(id))
                .expect("identified layout box");
            view.boxes().size(index).expect("box geometry")
        };
        assert_eq!(size_for("body").height, 600.0);
        assert_eq!(size_for("child").height, 600.0);
        let green = view
            .fragments()
            .decorations()
            .iter()
            .find(|fragment| fragment.color() == 0x008000FF)
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
        inputs.layout.viewport_width = 800.0;
        inputs.layout.viewport_height = Some(600.0);
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
        inputs.layout.viewport_width = 500.0;
        inputs.image_metrics.set(0, 96, 96);
        session
            .update(inputs, &mut shaper)
            .expect("well-formed XHTML pipeline");

        let view = session
            .document()
            .expect("the XML pipeline produced a document")
            .render_view(session.glyph_resources());
        let target = (0..view.boxes().len())
            .find(|&idx| view.boxes().id(idx).map(|value| view.string(value)) == Some("target"))
            .expect("image layout box");
        let size = view.boxes().size(target).expect("image box size");
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
            .laid_out
            .as_ref()
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
            session.cache.laid_out.as_ref().map(|entry| &entry.key),
            original_layout_key.as_ref()
        );
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
        assert!(session.active_resource_generation.is_none());
        assert!(
            session
                .layout_note(
                    "n",
                    crate::engine::LayoutConstraints::new(100.0, 16.0).unwrap(),
                    &mut shaper
                )
                .is_none()
        );
        shaper.fail_on = None;
        assert_eq!(
            session.attach(&mut shaper).update(inputs).unwrap().stage,
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
            session
                .document()
                .unwrap()
                .render_view(session.glyph_resources())
                .addressing()
                .source_location_on_line(0, Some(0))
                .unwrap()
        };
        let original = location(&session);
        session.rehydrate_glyphs(&mut shaper).unwrap();
        assert_eq!(location(&session), original);
        inputs.layout.viewport_width = 80.0;
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
    fn width_only_relayout_does_not_invoke_backend_line_measurement() {
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
        shaper.fail_measurement = true;
        let mut narrower = original_inputs.clone();
        narrower.layout.viewport_width = 10.0;
        let (changed_layout, changed_layout_resources) = session
            .document_mut_and_resources()
            .expect("initial document");
        changed_layout
            .relayout_with_metrics(
                to_layout_constraints(narrower.layout).expect("valid layout constraints"),
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
                    let Some(runs) = text.line_glyph_advances(line_idx) else {
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
                .map(|glyph_idx| addressing.link_for_glyph(glyph_idx))
                .collect(),
            lines,
            decorations: fragments
                .decorations()
                .iter()
                .map(|decoration| {
                    let rect = decoration.rect();
                    DecorationSnapshot {
                        x0_bits: rect.x0.to_bits(),
                        y0_bits: rect.y0.to_bits(),
                        x1_bits: rect.x1.to_bits(),
                        y1_bits: rect.y1.to_bits(),
                        color: decoration.color(),
                        is_inline: decoration.is_inline(),
                    }
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
        assert_eq!(update.stage, EarliestStage::Parse);
        assert_eq!(
            update.report.parsed,
            Reuse::recomputed(EarliestStage::Parse)
        );

        let shape_calls = shaper.calls;
        baseline.layout.viewport_width = 420.0;
        let layout_only = session.update(baseline.clone(), &mut shaper).unwrap();
        assert_eq!(layout_only.stage, EarliestStage::Layout);
        assert_eq!(
            layout_only.report.parsed,
            Reuse::reused(EarliestStage::Layout)
        );
        assert_eq!(
            layout_only.report.styled,
            Reuse::reused(EarliestStage::Layout)
        );
        assert_eq!(
            layout_only.report.prepared,
            Reuse::reused(EarliestStage::Layout)
        );
        assert_eq!(
            layout_only.report.shaped,
            Reuse::reused(EarliestStage::Layout)
        );
        assert_eq!(
            layout_only.report.laid_out,
            Reuse::recomputed(EarliestStage::Layout)
        );
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
        assert_eq!(font_update.stage, EarliestStage::Shape);
        assert_eq!(
            font_update.report.parsed,
            Reuse::reused(EarliestStage::Shape)
        );
        assert_eq!(
            font_update.report.styled,
            Reuse::reused(EarliestStage::Shape)
        );
        assert_eq!(
            font_update.report.prepared,
            Reuse::reused(EarliestStage::Shape)
        );
        assert_eq!(
            font_update.report.shaped,
            Reuse::recomputed(EarliestStage::Shape)
        );
        assert_eq!(
            font_update.report.laid_out,
            Reuse::recomputed(EarliestStage::Shape)
        );
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
        assert_eq!(update.stage, EarliestStage::Shape);
        assert_eq!(update.report.styled, Reuse::reused(EarliestStage::Shape));
        assert_eq!(update.report.prepared, Reuse::reused(EarliestStage::Shape));
        assert_eq!(
            update.report.shaped,
            Reuse::recomputed(EarliestStage::Shape)
        );
        assert_eq!(
            update.report.laid_out,
            Reuse::recomputed(EarliestStage::Shape)
        );

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
                    image_sizing_policy: crate::engine::ImageSizingPolicy::WebCompatible,
                    text_composition_policy: crate::engine::TextCompositionPolicy::WebCompatible,
                };
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
        assert_eq!(
            below_breakpoint.report.styled,
            Reuse::reused(EarliestStage::Layout)
        );
        assert_eq!(
            below_breakpoint.report.prepared,
            Reuse::reused(EarliestStage::Layout)
        );
        assert_eq!(shaper.calls, initial_shape_calls);

        inputs.layout.viewport_width = 600.0;
        inputs.style_environment.media = MediaEnvironment::screen(600.0, Some(800.0)).unwrap();
        let crossing_breakpoint = session.update(inputs.clone(), &mut shaper).unwrap();
        assert_eq!(crossing_breakpoint.stage, EarliestStage::Style);
        assert_eq!(
            crossing_breakpoint.report.styled,
            Reuse::recomputed(EarliestStage::Style)
        );
        assert_eq!(
            crossing_breakpoint.report.prepared,
            Reuse::recomputed(EarliestStage::Style)
        );
        assert!(shaper.calls > initial_shape_calls);
        let after_crossing_calls = shaper.calls;

        inputs.layout.viewport_width = 650.0;
        inputs.style_environment.media = MediaEnvironment::screen(650.0, Some(800.0)).unwrap();
        let above_breakpoint = session.update(inputs, &mut shaper).unwrap();
        assert_eq!(above_breakpoint.stage, EarliestStage::Layout);
        assert_eq!(
            above_breakpoint.report.styled,
            Reuse::reused(EarliestStage::Layout)
        );
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
        assert_eq!(
            update.report.styled,
            Reuse::recomputed(EarliestStage::Style)
        );
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
        inputs.layout.viewport_width = 420.0;
        let update = session.update(inputs, &mut shaper).unwrap();
        assert_eq!(update.stage, EarliestStage::Prepare);
        assert_eq!(
            update.report.styled,
            Reuse::recomputed(EarliestStage::Prepare)
        );
        assert_eq!(
            update.report.prepared,
            Reuse::recomputed(EarliestStage::Prepare)
        );
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

        assert_eq!(update.stage, EarliestStage::Prepare);
        assert_eq!(update.report.parsed, Reuse::reused(EarliestStage::Prepare));
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
            inputs.style_environment.root_font_size = font_size;
            let update = session.update(inputs.clone(), &mut shaper).unwrap();
            assert_eq!(update.stage, EarliestStage::Prepare);
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
        let image = &view.images()[0];
        let ImageSource::MathMl { svg, baseline, .. } = &image.source else {
            panic!("formula")
        };
        let formula = (svg.as_ptr(), image.width, image.height, *baseline);
        let resources = shaper.resources.clone();
        let generation = shaper.resource_generation();
        inputs.style_environment.root_font_size = 32;
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
        let image = &view.images()[0];
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
            .line(0)
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
            .line(0)
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
                ('B', theme.adapt(0x008000ff)),
                ('A', theme.adapt(0xff00ffff)),
            ] {
                let text = session
                    .document()
                    .unwrap()
                    .render_view(session.glyph_resources())
                    .text();
                let index = (0..text.glyph_count())
                    .find(|&index| {
                        text.glyph_at(index)
                            .and_then(|glyph| text.glyph_metric(glyph))
                            .is_some_and(|metric| metric.ch() == character)
                    })
                    .unwrap();
                assert_eq!(text.paint_color_at(index as u32), Some(expected));
                if update.stage == EarliestStage::Paint {
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
            inputs.style_environment.root_font_size = desired;
            inputs.layout.line_height = f64::from(desired);
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
        assert_eq!(update.stage, EarliestStage::Prepare);
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
        layout.viewport_width = 120.0;
        let update = session
            .apply_changes(
                PipelineChanges {
                    layout: Some(layout),
                    ..Default::default()
                },
                &mut shaper,
            )
            .unwrap();
        assert_eq!(update.stage, EarliestStage::Layout);
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
        assert_eq!(update.stage, EarliestStage::Parse);
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
        let mut metrics = crate::engine::ImageMetrics::default();
        metrics.set(0, 900, 800);
        shaper.fail_on = Some('!');
        assert!(
            session
                .apply_changes(
                    PipelineChanges {
                        source: Some("<p>replacement!</p>".to_owned()),
                        source_revision: Some(SourceRevision::INITIAL.next()),
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
        assert_eq!(update.stage, EarliestStage::Paint);
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
        layout.viewport_width = 100.0;
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
        assert_eq!(update.stage, EarliestStage::Style);
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
        assert_eq!(update.stage, EarliestStage::Paint);
        assert_eq!(shaper.calls, calls);
        assert_eq!(session.inputs().unwrap().source.as_ptr(), source_pointer);
    }

    #[test]
    fn changes_reject_empty_sessions_and_invalid_layout_without_committing() {
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
        let mut invalid = original;
        invalid.viewport_width = f64::NAN;
        assert!(
            session
                .apply_changes(
                    PipelineChanges {
                        layout: Some(invalid),
                        ..Default::default()
                    },
                    &mut shaper
                )
                .is_err()
        );
        assert_eq!(session.inputs().unwrap().layout, original);
        session
            .apply_changes(PipelineChanges::default(), &mut shaper)
            .unwrap();
        assert!(session.document().is_some());
    }

    #[test]
    fn color_preferences_change_paint_but_font_preferences_change_style() {
        let inputs = base_inputs("<p>colors</p>", SourceRevision::INITIAL);
        let mut next = inputs.clone();
        next.reader_overrides.link_foreground = Some(0x123456ff);
        assert_eq!(
            next.change_mask(&inputs).earliest_stage(),
            EarliestStage::Paint
        );
        assert_eq!(next.changes_from(&inputs).1, EarliestStage::Paint);
        next.reader_overrides.font_family = Some("serif".into());
        assert_eq!(
            next.change_mask(&inputs).earliest_stage(),
            EarliestStage::Style
        );
    }

    #[test]
    fn note_flow_changes_are_reported_at_preparation_and_applied() {
        let inputs = base_inputs(
            "<p>Main text</p><aside epub:type='footnote'>unicornnote</aside>",
            SourceRevision::INITIAL,
        );
        let mut next = inputs.clone();
        next.note_flow = crate::engine::NoteFlow::Excluded;
        assert_eq!(next.earliest_stage(&inputs), EarliestStage::Prepare);
        let (changes, stage) = next.changes_from(&inputs);
        assert_eq!(stage, EarliestStage::Prepare);
        assert_eq!(changes, [PipelineChange::NoteFlowChanged(next.note_flow)]);
        assert!(next.change_mask(&inputs).note_flow_changed);
        assert_ne!(next, inputs);
        let mut session = PipelineSession::new(Arc::new(MockProvider));
        let mut shaper = TracingShaper::default();
        session.update(inputs, &mut shaper).unwrap();
        let has_note_text = |session: &PipelineSession| {
            !session
                .render_view()
                .unwrap()
                .text()
                .find_matches("unicornnote", crate::TextSearchOptions::default())
                .is_empty()
        };
        assert!(has_note_text(&session));
        assert_eq!(
            session.update(next, &mut shaper).unwrap().stage,
            EarliestStage::Prepare
        );
        assert!(!has_note_text(&session));
    }

    #[test]
    fn paint_updates_restore_publication_colors_and_match_fresh_geometry() {
        let source = "<html><head><style>a::before {content:'G'; color:orange} p::first-letter {color:purple}</style></head><body><div style='background:#fafafa; border:2px solid red; outline:1px dotted blue'><p id='anchor' style='color:green'>B <a href='#anchor'>L<span style='color:blue'>N</span></a></p><p>Other text wraps across several lines in this paragraph.</p></div></body></html>";
        let mut inputs = base_inputs(source, SourceRevision::INITIAL);
        inputs.layout.viewport_width = 140.0;
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
            assert_eq!(update.stage, EarliestStage::Paint);
            assert!(update.report.shaped.reused && update.report.laid_out.reused);
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
            session
                .document()
                .unwrap()
                .render_view(session.glyph_resources())
                .text()
                .paint_color_at(0),
            Some(0x800080ff)
        );
        inputs.layout.viewport_width = 200.0;
        assert_eq!(
            session.update(inputs.clone(), &mut shaper).unwrap().stage,
            EarliestStage::Layout
        );
        inputs.reader_overrides.link_foreground = Some(0xabcdef88);
        assert_eq!(
            session.update(inputs.clone(), &mut shaper).unwrap().stage,
            EarliestStage::Paint
        );
        inputs.source = "<p>different chapter</p>".into();
        assert_eq!(
            session.update(inputs, &mut shaper).unwrap().stage,
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
                session.update(inputs.clone(), &mut shaper).unwrap().stage,
                EarliestStage::Paint
            );
            assert_eq!(shaper.calls, calls);
            shaper.reset();
            inputs.reader_overrides.foreground = Some(0x112233ff);
            let update = session.update(inputs, &mut shaper).unwrap();
            assert!(update.stage < EarliestStage::Paint);
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
            session
                .document()
                .unwrap()
                .render_view(session.glyph_resources())
                .text()
                .paint_color_at(0),
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
        assert_ne!(
            original.render_only_cache_key(),
            painted.render_only_cache_key()
        );
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
        let layout = &session.cache.laid_out.as_ref().unwrap().key;
        let prepared = &session.cache.prepared.as_ref().unwrap().key;
        let shaped = &session.cache.shaped.as_ref().unwrap().key;
        let parsed = &session.cache.parsed.as_ref().unwrap().key;
        assert_eq!(layout, &expected);
        assert!(Arc::ptr_eq(
            &layout.shaped.prepared.user_styles,
            &prepared.user_styles
        ));
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
            inputs.layout.viewport_width = 140.0;
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
                    session
                        .document()
                        .unwrap()
                        .render_view(session.glyph_resources())
                        .text()
                        .line_overflow_clip(line.index())
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
                assert_eq!(update.stage, EarliestStage::Paint, "{source}");
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
                        to_layout_constraints(inputs.layout).unwrap(),
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
                    view.fragments()
                        .decorations()
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
                    candidate
                        .render_view(session.glyph_resources())
                        .canvas_background_color(),
                    reference
                        .render_view(&reference_resources)
                        .canvas_background_color()
                );
                let text = candidate.render_view(session.glyph_resources()).text();
                assert_eq!(
                    text.lines()
                        .iter()
                        .map(|line| text.line_overflow_clip(line.index()))
                        .collect::<Vec<_>>(),
                    line_clips
                );
                for index in 0..text.glyph_count() {
                    let actual_color = text.paint_color_at(index as u32).unwrap();
                    let expected_color = reference
                        .render_view(&reference_resources)
                        .text()
                        .paint_color_at(index as u32)
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
                session.update(inputs, &mut shaper).unwrap().stage,
                EarliestStage::Paint
            );
            let constraints = crate::engine::LayoutConstraints::new(200.0, 20.0).unwrap();
            let note = session.layout_note("n", constraints, &mut shaper).unwrap();
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
                view.fragments()
                    .decorations()
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
                assert_eq!(text.paint_color_at(index as u32), Some(expected_color));
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
        inputs.layout.viewport_width = 10.0;
        inputs.reader_overrides.foreground = Some(0xabcdef88);
        inputs.reader_overrides.link_foreground = Some(0x998877ff);
        assert_eq!(
            session.update(inputs.clone(), &mut shaper).unwrap().stage,
            EarliestStage::Layout
        );
        assert_eq!(shaper.calls, calls);

        let doc = session.document().unwrap();
        let text = doc.render_view(session.glyph_resources()).text();
        assert!(text.line_count() > 1);
        assert_eq!(text.paint_color_at(0), Some(0x998877ff));
        let mut fresh = PipelineSession::new(Arc::new(MockProvider));
        fresh.update(inputs, &mut TracingShaper::default()).unwrap();
        assert_eq!(
            build_snapshot((doc).render_view(session.glyph_resources())),
            build_snapshot((fresh.document().unwrap()).render_view(fresh.glyph_resources()))
        );
    }

    #[test]
    fn paragraph_controls_match_fresh_cascades_and_restore_book_layout() {
        use html_dom::RootFontSize;
        for source in [
            "<html><body style='margin:0;font-size:1.29167em'><span style='font-size:0.77419em'>An opening passage outside a paragraph.</span><p style='margin:1em 0;text-indent:7%'><span style='font-size:0.77419em'>Words that wrap across several lines for paragraph controls.</span></p><p style='font-size:12px;margin:8px 0'>Another paragraph follows the first.</p></body></html>",
            "<html><body style='margin:0;font-size:1.29167em;text-indent:7%;text-align:justify'><div><span style='font-size:.77419em'>An opening passage with enough words to wrap.<br><br>A second passage with more words for wrapping.</span><br> <br>A final passage.</div></body></html>",
        ] {
            let mut inputs = base_inputs(source, SourceRevision::INITIAL);
            inputs.layout.viewport_width = 100.0;
            inputs.style_environment.root_font_size = 24;
            inputs.layout.line_height = 24.0;
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
                assert_eq!(update.stage, EarliestStage::Prepare);
                for width in [100.0, 140.0, 100.0] {
                    inputs.layout.viewport_width = width;
                    session.update(inputs.clone(), &mut shaper).unwrap();
                    let mut factory = DocumentFactory::new();
                    factory.set_root_font_size(RootFontSize::new(24.0).unwrap());
                    factory.set_reader_overrides(inputs.reader_overrides.clone());
                    let prepared = factory.parse_with_new_pipeline(source, None);
                    let mut reference_shaper = SizedShaper::default();
                    let mut reference_resources = GlyphResourceStore::default();
                    let reference = prepared
                        .shape(&mut reference_shaper, &mut reference_resources)
                        .unwrap()
                        .layout_with_metrics_and_shaper(
                            to_layout_constraints(inputs.layout).unwrap(),
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

        assert_eq!(update.stage, EarliestStage::Prepare);
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
        assert_eq!(style_update.stage, EarliestStage::Prepare);
        assert_eq!(
            style_update.report.styled,
            Reuse::recomputed(EarliestStage::Prepare)
        );

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
        assert_eq!(restore.stage, EarliestStage::Prepare);

        let restored = {
            let document = session.document().expect("document should exist");
            build_snapshot((document).render_view(session.glyph_resources()))
        };
        assert_eq!(restore.report.parsed, Reuse::reused(EarliestStage::Prepare));
        assert_eq!(baseline, restored);
    }

    #[test]
    fn pipeline_changes_map_to_expected_earliest_stage() {
        let previous = base_inputs(
            "<html><body><p>x</p></body></html>",
            SourceRevision::INITIAL,
        );
        let mut next = previous.clone();
        next.source = "<html><body><p>y</p></body></html>".into();
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
        next.layout.image_sizing_policy = crate::engine::ImageSizingPolicy::SmartStandalone;
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Layout);

        let mut next = previous.clone();
        next.layout.text_composition_policy = crate::engine::TextCompositionPolicy::BookOptimized;
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Layout);

        let mut next = previous.clone();
        next.image_metrics_revision = ImageMetricsRevision::next(previous.image_metrics_revision);
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Layout);

        let mut next = previous.clone();
        next.paint = PaintSettingsRevision::next(previous.paint);
        assert_eq!(next.earliest_stage(&previous), EarliestStage::Paint);

        let changes = [
            (
                PipelineChange::SourceChanged(SourceRevision::INITIAL),
                EarliestStage::Parse,
            ),
            (
                PipelineChange::ResourceBaseChanged(ResourceRevision::INITIAL),
                EarliestStage::Parse,
            ),
            (
                PipelineChange::StylesheetsChanged(StylesheetRevision::INITIAL),
                EarliestStage::Style,
            ),
            (
                PipelineChange::StyleEnvironmentChanged(StyleEnvironment::default()),
                EarliestStage::Style,
            ),
            (
                PipelineChange::NoteFlowChanged(crate::engine::NoteFlow::Excluded),
                EarliestStage::Prepare,
            ),
            (
                PipelineChange::FontEnvironmentChanged(FontEnv::INITIAL),
                EarliestStage::Shape,
            ),
            (
                PipelineChange::ImageMetricsChanged(ImageMetricsRevision::INITIAL),
                EarliestStage::Layout,
            ),
            (
                PipelineChange::LayoutConstraintsChanged(LayoutConstraints {
                    viewport_width: 600.0,
                    viewport_height: None,
                    line_height: 20.0,
                    image_sizing_policy: crate::engine::ImageSizingPolicy::WebCompatible,
                    text_composition_policy: crate::engine::TextCompositionPolicy::WebCompatible,
                }),
                EarliestStage::Layout,
            ),
            (
                PipelineChange::PaintSettingsChanged(PaintSettingsRevision::INITIAL),
                EarliestStage::Paint,
            ),
        ];
        for (change, expected) in changes {
            assert_eq!(change.invalidated_stage(), expected);
        }
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
        let green = document
            .render_view(session.glyph_resources())
            .fragments()
            .decorations()
            .iter()
            .find(|fragment| fragment.color() == 0x008000FF)
            .expect("absolute green background");
        assert!(green.is_in_positioned_layer());
        assert!(!green.is_in_negative_positioned_layer());
        assert!(
            !green.is_in_independent_positioned_layer(),
            "auto-z absolute and relative boxes share the source-ordered positioned stream"
        );
        assert!(green.line_idx().is_none());
        let root = document.render_view(session.glyph_resources());
        let images = root.fragments().images().iter().collect::<Vec<_>>();
        assert!(
            !images.is_empty(),
            "the regression fixture must produce a replaced-image fragment"
        );
        for image in images {
            let line = root
                .text()
                .line(image.line_idx())
                .expect("image owning line");
            assert!(
                line.is_in_positioned_layer(),
                "the relative ancestor keeps its normal descendants in positioned paint"
            );
            assert!(
                !line.is_in_independent_positioned_layer(),
                "the preceding image remains in the common source-ordered positioned stream"
            );
        }
    }
}
