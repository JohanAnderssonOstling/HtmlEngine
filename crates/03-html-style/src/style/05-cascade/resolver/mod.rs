//! Document-level orchestration for selector matching and computed styles.
//!
//! Per-target work flows through `CascadePlan`, custom-property resolution,
//! the shared cascade runner, property-family application, and style interning.

use crate::style::compiled::{CompiledStyles, PreparedRuleSet, ScopeId, SelectorCandidate, SelectorIndex};
use crate::style::environment::MediaQuerySet;
use crate::style::matching::dom::{
    MatchCaches, PseudoTarget, SiblingIndex, selector_matches_indexed_in_scope,
};
use crate::style::matching::scope::scope_match;
use crate::style::matching::selectors::AncestorFilter;
use crate::style::matching::{BoundSelectors, StyleTraversal};
use crate::style::source::inline::InlineStyles;
use crate::style::source::inline::PreparedInlineStyle;
use context::ResolutionContext;
use custom_properties::{
    CustomCascadeCache, CustomCascadeProbe, CustomCascadeSignature, CustomPropertyStore,
    cascade_custom_properties,
};
use html_dom::{Document, DomNodeId, ElementRef};
use html_style_model::{ComputedStyles, ComputedStylesBuilder, ReaderStyleOverrides, StyleIndices};
use local_matching::{LocalMatchCache, LocalMatchProbe, PseudoCandidates};
use plan::{CachedRuleMatches, CascadePlan, MatchedRule};
use runner::{CascadeInputs, CascadeResolver};
use session::{PhaseTimer, ResolveStyleTimings, StyleResolverContext};
use specified::{ElementDeclarations, build_cascade_events};
use state::{ParentStyle, WorkingStyle};
use style_sharing::{
    SharedElementStyle, StyleSharingCache, StyleSharingProbe, StyleSharingSignature,
};
use values::default_justify_items;
use web_time::Instant;

mod cache_index;
mod context;
mod custom_properties;
#[path = "../html_presentational_hints.rs"]
mod html_presentational_hints;
mod local_matching;
mod plan;
pub(crate) mod prepared_native;
mod properties;
mod runner;
mod session;
mod specified;
mod state;
mod style_sharing;
mod traversal;
mod values;
mod wide_keywords;

// ============================================================================
// Style Resolver - applies CSS styles to Document boxes
// ============================================================================

enum CustomMapResult {
    Inherited,
    Reused(u32),
    New {
        map_id: u32,
        cache_signature: Option<CustomCascadeSignature>,
    },
}

enum ElementStyleResult {
    Shared(SharedElementStyle),
    Computed {
        style: WorkingStyle,
        custom_map: CustomMapResult,
        sharing_signature: Option<StyleSharingSignature>,
    },
}

pub(crate) fn resolve_styles_for_dom_timed(
    doc: &Document,
    compiled: &CompiledStyles,
    author_roots: &[Option<DomNodeId>],
    inline_styles: &InlineStyles,
    reader_overrides: &ReaderStyleOverrides,
) -> (ComputedStyles, ResolveStyleTimings, MediaQuerySet) {
    let prepared = &compiled.rules;
    let index = &compiled.selectors;
    let mut resolution = ResolutionContext::new(prepared.environment());
    resolution.font_size_scale = reader_overrides
        .canonical_font_size
        .filter(|size| size.is_finite() && *size > 0.0)
        .map(|size| doc.root_font_size() / size)
        .filter(|scale| scale.is_finite() && *scale > 0.0)
        .unwrap_or(1.0);
    let mut timings = ResolveStyleTimings::default();

    // Process each DOM element
    let started = Instant::now();
    let mut node_custom_map_ids = vec![0u32; doc.element_count()];
    let mut custom_maps = CustomPropertyStore::default();
    let mut custom_cascade_cache = CustomCascadeCache::default();
    let style_sharing_cache = StyleSharingCache::default();
    let mut computed_styles = ComputedStylesBuilder::new(doc);
    let candidate_scratch = Vec::new();
    let pseudo_candidate_scratch = PseudoCandidates::default();
    let mut traversal = StyleTraversal::new(doc);
    let mut match_caches = MatchCaches::default();
    let selectors = index.compiled.bind(doc);
    let local_specificity_scratch = Vec::new();
    let mut local_match_cache = LocalMatchCache::default();
    local_match_cache.configure_candidates(index);
    let cascade_scratch = specified::CascadeScratch::default();
    timings.resolver_setup = started.elapsed();
    let traversal_started = Instant::now();
    {
        let mut resolver = StyleResolverContext {
            doc,
            prepared,
            author_roots,
            index,
            selectors: &selectors,
            styles: &mut computed_styles,
            reader_overrides,
            candidate_scratch,
            pseudo_candidate_scratch,
            local_specificity_scratch,
            local_match_cache,
            matched_rules: CachedRuleMatches::default(),
            cascade_scratch,
            style_sharing_cache,
            resolution: &mut resolution,
            timings: &mut timings,
        };
        while let Some((element, ancestor_filter)) = traversal.next() {
            let mut sibling_index = SiblingIndex::new(doc, &mut traversal, &mut match_caches);
            let node_idx = element.node_id();
            let parent_custom_id = doc.node_ref(node_idx).parent().map_or(0, |parent_idx| {
                node_custom_map_ids[parent_idx.index()] as usize
            });

            let inline_style = inline_styles.get(node_idx);
            let style_result = resolver.compute_style_for_dom_element(
                &mut sibling_index,
                element,
                parent_custom_id as u32,
                &mut custom_maps,
                &mut custom_cascade_cache,
                &ancestor_filter,
                inline_style,
            );
            let candidate_pseudo_mask = resolver
                .local_match_cache
                .pseudo_candidates()
                .unwrap_or(&resolver.pseudo_candidate_scratch)
                .mask();

            let started = PhaseTimer::start();
            let (style_indices, custom_map_id, counters, sharing_signature) = match style_result {
                ElementStyleResult::Shared(shared) => {
                    (shared.style, shared.custom_map_id, shared.counters, None)
                }
                ElementStyleResult::Computed {
                    mut style,
                    custom_map,
                    sharing_signature,
                } => {
                    let custom_map_id = match custom_map {
                        CustomMapResult::Inherited => parent_custom_id as u32,
                        CustomMapResult::Reused(map_id) => map_id,
                        CustomMapResult::New {
                            map_id,
                            cache_signature,
                        } => {
                            if let Some(signature) = cache_signature {
                                custom_cascade_cache.insert(signature, map_id);
                            }
                            map_id
                        }
                    };
                    let counters = std::mem::take(&mut style.counters);
                    let style_indices = style
                        .intern(resolver.styles)
                        .expect("resolver built style values should pass style validation");
                    (style_indices, custom_map_id, counters, sharing_signature)
                }
            };
            resolver
                .styles
                .set_node_style(node_idx, style_indices)
                .expect("resolver only assigns styles to nodes from its source document");
            resolver.local_match_cache.remember_style(
                element,
                inline_style.is_some(),
                prepared,
                style_indices,
                custom_map_id,
                &counters,
            );
            if let Some(signature) = sharing_signature {
                resolver.style_sharing_cache.insert(
                    signature,
                    style_indices,
                    custom_map_id,
                    counters.clone(),
                );
            }
            if !counters.is_empty() {
                resolver
                    .styles
                    .set_counter_directives(node_idx, counters)
                    .expect("counter directives belong to their originating element");
            }
            resolver.store_pseudo_styles(
                &mut sibling_index,
                node_idx,
                style_indices,
                candidate_pseudo_mask,
                &mut custom_maps,
                custom_map_id,
            );
            node_custom_map_ids[node_idx.index()] = custom_map_id;
            resolver.timings.style_store += started.elapsed();
        }
    }
    if cfg!(target_arch = "wasm32") {
        timings.cascade += traversal_started.elapsed();
    }
    let started = Instant::now();
    computed_styles.apply_reader_overrides(doc, reader_overrides);
    let styles = computed_styles
        .finish()
        .expect("resolver must assign a valid computed style to every element");
    timings.style_store += started.elapsed();
    let mut media_queries = prepared.media_queries().clone();
    if resolution.uses_viewport_units {
        media_queries.mark_viewport_unit_dependency();
    }
    (styles, timings, media_queries)
}

/// Shared scoped matching for ordinary elements and pseudo targets. Local
/// context-free specificity is supplied by the caller's sibling cache.
struct RuleMatcher<'a, 'cache, 'dom, 'css> {
    doc: &'dom Document,
    prepared: &'a PreparedRuleSet<'css>,
    index: &'a SelectorIndex,
    selectors: &'a BoundSelectors<'dom>,
    roots: &'a [Option<DomNodeId>],
    siblings: &'a mut SiblingIndex<'cache, 'dom, 'css>,
}

impl RuleMatcher<'_, '_, '_, '_> {
    fn match_rule(
        &mut self,
        node: DomNodeId,
        candidates: &[SelectorCandidate],
        pseudo: Option<PseudoTarget>,
        ancestors: Option<&AncestorFilter>,
        mut specificity: Option<u32>,
    ) -> Option<MatchedRule> {
        let id = candidates[0].rule;
        let scope_id = self.prepared.scope(id);
        let scope = if scope_id == ScopeId::NONE {
            None
        } else {
            Some(scope_match(
                self.prepared,
                scope_id,
                self.doc,
                self.roots,
                node,
                self.siblings,
            )?)
        };
        let selectors = self.index.compiled.for_rule(id);
        let maximum = self.index.compiled.max_specificity(id);
        for candidate in candidates {
            if specificity == Some(maximum) {
                break;
            }
            let selector_index = candidate.selector as usize;
            let selector = selectors[selector_index];
            let target_matches = match pseudo {
                Some(pseudo) => selector.targets_pseudo(pseudo.mask()),
                None => !selector.has_pseudo_target() && !selector.is_context_free(),
            };
            if !target_matches || specificity.is_some_and(|value| value >= selector.specificity()) {
                continue;
            }
            // A pseudo selector's synthetic originating-element hop is not a
            // real ancestor, so its bloom prefilter must remain disabled.
            if ancestors.is_some_and(|filter| !self.index.selector_might_match(selector, filter)) {
                continue;
            }
            let matches = self
                .selectors
                .matches_indexed_in_traversal(selector, node, self.siblings)
                .unwrap_or_else(|| {
                    let Some(root) = scope.map(|scope| scope.root).or_else(|| self.doc.dom_root()) else {
                        return false;
                    };
                    selector_matches_indexed_in_scope(
                        &self.prepared.get(id).style_rule().selectors.0[selector_index],
                        self.doc,
                        node,
                        pseudo,
                        root,
                        self.siblings,
                        selector.bucket_component,
                    )
                });
            if matches {
                specificity = specificity.max(Some(selector.specificity()));
            }
        }
        specificity.map(|specificity| MatchedRule {
            id,
            specificity,
            scope_proximity: scope.map_or(u32::MAX, |scope| scope.proximity),
        })
    }
}

impl<'a, 'css> StyleResolverContext<'a, 'css> {
    fn store_pseudo_styles(
        &mut self,
        sibling_index: &mut SiblingIndex<'_, 'a, 'css>,
        node_idx: DomNodeId,
        style_indices: StyleIndices,
        candidate_pseudo_mask: u8,
        custom_maps: &mut CustomPropertyStore<'css>,
        custom_map_id: u32,
    ) {
        let mut first_line = None;
        let mut before_first_letter_parent = None;
        // First-letter styles need the preceding first-line and generated-before parents.
        for pseudo in [
            PseudoTarget::FirstLine,
            PseudoTarget::Before,
            PseudoTarget::FirstLetter,
            PseudoTarget::After,
        ] {
            if candidate_pseudo_mask & pseudo.mask() == 0 {
                continue;
            }
            let parent = if pseudo == PseudoTarget::FirstLetter {
                first_line.unwrap_or(style_indices)
            } else {
                style_indices
            };
            let Some(mut style) = self.compute_pseudo_style_for_dom_element(
                sibling_index,
                node_idx,
                parent,
                pseudo,
                custom_maps,
                custom_map_id,
            ) else {
                continue;
            };
            let generated = matches!(pseudo, PseudoTarget::Before | PseudoTarget::After);
            let content = if generated {
                style.generated_content.take()
            } else {
                None
            };
            if generated && content.is_none() {
                continue;
            }
            let counters = std::mem::take(&mut style.counters);
            let style = style
                .intern(self.styles)
                .expect("pseudo style values should pass validation");
            match (pseudo, counters) {
                (PseudoTarget::FirstLine, _) => self.styles.set_first_line_style(node_idx, style),
                (PseudoTarget::FirstLetter, _) => {
                    self.styles.set_first_letter_style(node_idx, style)
                }
                (PseudoTarget::Before, counters) => {
                    self.styles
                        .set_before_style(node_idx, style, content.unwrap(), counters)
                }
                (PseudoTarget::After, counters) => {
                    self.styles
                        .set_after_style(node_idx, style, content.unwrap(), counters)
                }
            }
            .expect("pseudo style belongs to its originating element");
            match pseudo {
                PseudoTarget::FirstLine => first_line = Some(style),
                PseudoTarget::Before => {
                    before_first_letter_parent = if let Some(first_line) = first_line {
                        self.compute_pseudo_style_for_dom_element(
                            sibling_index,
                            node_idx,
                            first_line,
                            PseudoTarget::Before,
                            custom_maps,
                            custom_map_id,
                        )
                        .and_then(|mut style| {
                            style.generated_content.take().map(|_| {
                                style
                                    .intern(self.styles)
                                    .expect("pseudo style values should pass validation")
                            })
                        })
                    } else {
                        Some(style)
                    };
                }
                PseudoTarget::FirstLetter => {
                    if let Some(parent) = before_first_letter_parent
                        && let Some(style) = self.compute_pseudo_style_for_dom_element(
                            sibling_index,
                            node_idx,
                            parent,
                            PseudoTarget::FirstLetter,
                            custom_maps,
                            custom_map_id,
                        )
                    {
                        let style = style
                            .intern(self.styles)
                            .expect("pseudo style values should pass validation");
                        self.styles
                            .set_before_first_letter_style(node_idx, style)
                            .expect("pseudo style belongs to its originating element");
                    }
                }
                PseudoTarget::After => {}
            }
        }
    }

    fn compute_pseudo_style_for_dom_element(
        &mut self,
        sibling_index: &mut SiblingIndex<'_, 'a, 'css>,
        node_idx: DomNodeId,
        inherited_from: StyleIndices,
        pseudo: PseudoTarget,
        custom_maps: &mut CustomPropertyStore<'css>,
        inherited_custom_id: u32,
    ) -> Option<WorkingStyle> {
        let prepared = self.prepared;
        let mut matcher = RuleMatcher {
            doc: self.doc,
            prepared: self.prepared,
            index: self.index,
            selectors: self.selectors,
            roots: self.author_roots,
            siblings: sibling_index,
        };
        let (reuse_matches, matched_rules) = self.matched_rules.begin_pseudo(node_idx, pseudo);
        if !reuse_matches {
            for candidates in self
                .local_match_cache
                .pseudo_candidates()
                .unwrap_or(&self.pseudo_candidate_scratch)
                .for_target(pseudo)
                .chunk_by(|a, b| a.rule == b.rule)
            {
                if let Some(matched) =
                    matcher.match_rule(node_idx, candidates, Some(pseudo), None, None)
                {
                    matched_rules.push(matched);
                }
            }
        }
        if matched_rules.is_empty() {
            return None;
        }

        if !reuse_matches {
            self.matched_rules.order(prepared);
        }
        let CascadePlan {
            normal: matched_rules,
            important: important_rules,
        } = self.matched_rules.plan();

        let inherited = ParentStyle::new(Some(inherited_from), self.doc.root_font_size());
        // Pseudo-elements retain initial layout values; ordinary elements also
        // inherit the legacy justify-items behavior from their parent.
        let mut style = WorkingStyle::inherited(&inherited, self.styles);
        let cascade_scratch = &mut self.cascade_scratch;
        build_cascade_events(
            prepared,
            &matched_rules,
            &important_rules,
            ElementDeclarations {
                prepared: Some(self.prepared),
                ..ElementDeclarations::default()
            },
            &mut cascade_scratch.events,
        );
        let checkpoint = custom_maps.checkpoint();
        let custom_id = cascade_custom_properties(
            &cascade_scratch.events.events,
            ElementDeclarations {
                prepared: Some(self.prepared),
                ..ElementDeclarations::default()
            },
            custom_maps,
            inherited_custom_id,
            &mut cascade_scratch.custom_properties,
        );
        let custom_properties = custom_maps.view(custom_id);

        CascadeResolver {
            doc: self.doc,
            styles: self.styles,
            reader_overrides: self.reader_overrides,
            resolution: self.resolution,
        }
        .apply_cascade(
            &mut style,
            CascadeInputs {
                events: &cascade_scratch.events.events,
                element: ElementDeclarations {
                    prepared: Some(self.prepared),
                    ..ElementDeclarations::default()
                },
                custom_properties: &custom_properties,
                // Temporary pseudo maps are rewound below; their IDs can be reused.
                cache_substitutions: false,
                parent: &inherited,
            },
            &mut cascade_scratch.computation,
        );

        custom_maps.rewind(checkpoint);
        Some(style)
    }
}

/// Compute style for a single DOM element (used in new pipeline)
impl<'a, 'css> StyleResolverContext<'a, 'css> {
    fn compute_style_for_dom_element(
        &mut self,
        sibling_index: &mut SiblingIndex<'_, 'a, 'css>,
        source_element: ElementRef<'_>,
        parent_custom_id: u32,
        custom_maps: &mut CustomPropertyStore<'css>,
        custom_cascade_cache: &mut CustomCascadeCache,
        ancestor_filter: &AncestorFilter,
        inline_style: Option<&PreparedInlineStyle<'css>>,
    ) -> ElementStyleResult {
        let node_idx = source_element.node_id();
        let doc = self.doc;
        let prepared = self.prepared;
        let index = self.index;
        let matching_started = PhaseTimer::start();

        let local_probe = self.local_match_cache.probe(doc, source_element);
        let local_hit = matches!(&local_probe, LocalMatchProbe::Hit);
        if local_hit
            && let Some(shared) =
                self.local_match_cache
                    .early_style(doc, source_element, inline_style.is_some())
        {
            self.timings.selector_matching += matching_started.elapsed();
            return ElementStyleResult::Shared(shared);
        }
        let mut matcher = RuleMatcher {
            doc: self.doc,
            prepared: self.prepared,
            index: self.index,
            selectors: self.selectors,
            roots: self.author_roots,
            siblings: sibling_index,
        };
        let matched_rules = self.matched_rules.begin_element();
        if !local_hit {
            index.collect_element_candidates(
                source_element,
                doc.dom_root() == Some(node_idx),
                &mut self.candidate_scratch,
            );
            self.local_specificity_scratch.clear();
            self.pseudo_candidate_scratch
                .prepare(&self.candidate_scratch, &index.compiled);
        }

        // Check each candidate. A rule cascades with the most specific of its
        // matching selectors, not the first one that happens to match.
        for (candidate_index, candidates) in self
            .local_match_cache
            .candidates()
            .unwrap_or(&self.candidate_scratch)
            .chunk_by(|a, b| a.rule == b.rule)
            .enumerate()
        {
            let id = candidates[0].rule;
            let prepared_selectors = index.compiled.for_rule(id);
            let max_specificity = index.compiled.max_specificity(id);
            let mut specificity = if local_hit {
                self.local_match_cache
                    .specificities()
                    .expect("cached local matches")[candidate_index]
            } else {
                None
            };
            if !local_hit {
                for candidate in candidates {
                    if specificity == Some(max_specificity) {
                        break;
                    }
                    let prepared_selector = prepared_selectors[candidate.selector as usize];
                    if !prepared_selector.is_context_free()
                        || specificity
                            .is_some_and(|matched| matched >= prepared_selector.specificity())
                    {
                        continue;
                    }
                    debug_assert!(index.selector_might_match(prepared_selector, ancestor_filter));
                    if self.selectors.matches_indexed(prepared_selector, node_idx) == Some(true) {
                        specificity = specificity.max(Some(prepared_selector.specificity()));
                    }
                }
                self.local_specificity_scratch.push(specificity);
            }
            if let Some(matched) = matcher.match_rule(
                node_idx,
                candidates,
                None,
                Some(ancestor_filter),
                specificity,
            ) {
                matched_rules.push(matched);
            }
        }
        if let LocalMatchProbe::Miss(signature) = local_probe {
            self.local_match_cache.insert(
                signature,
                &mut self.candidate_scratch,
                &self.local_specificity_scratch,
                &self.pseudo_candidate_scratch,
            );
        }

        self.timings.selector_matching += matching_started.elapsed();

        // Build computed style from matched rules (same as box-based version)
        let cascade_started = PhaseTimer::start();
        let parent_style_indices = source_element
            .parent()
            .and_then(|parent| self.styles.style_for_node(parent));
        let sharing_signature = match self.style_sharing_cache.probe(
            source_element,
            inline_style.is_some(),
            parent_style_indices,
            parent_custom_id,
            &matched_rules,
        ) {
            StyleSharingProbe::Hit(shared) => {
                self.timings.cascade += cascade_started.elapsed();
                return ElementStyleResult::Shared(shared);
            }
            StyleSharingProbe::Miss(signature) => Some(signature),
            StyleSharingProbe::Uncacheable => None,
        };
        // A sharing hit needs neither cascade ordering nor the important-rule
        // split. The exact deterministic match sequence is already the key.
        self.matched_rules.order(prepared);
        let CascadePlan {
            normal: matched_rules,
            important: important_rules,
        } = self.matched_rules.plan();
        let parent_style = ParentStyle::new(parent_style_indices, doc.root_font_size());
        let mut style = WorkingStyle::inherited(&parent_style, self.styles);
        style.layout.justify_items = parent_style.with(self.styles, |parent| {
            default_justify_items(parent.layout.justify_items)
        });
        let cascade_scratch = &mut self.cascade_scratch;
        html_presentational_hints::collect(doc, source_element, &mut cascade_scratch.hints);
        cascade_scratch.hint_preparation.clear();
        cascade_scratch
            .hint_preparation
            .append(&cascade_scratch.hints);
        let element = ElementDeclarations {
            prepared: Some(self.prepared),
            inline: inline_style,
            hints: &cascade_scratch.hints,
            hint_preparation: Some(&cascade_scratch.hint_preparation),
        };
        build_cascade_events(
            prepared,
            &matched_rules,
            &important_rules,
            element,
            &mut cascade_scratch.events,
        );
        let (custom_properties, reused_custom_id, cache_signature) = match custom_cascade_cache
            .probe(&cascade_scratch.events.events, element, parent_custom_id)
        {
            CustomCascadeProbe::NoDeclarations => (None, None, None),
            CustomCascadeProbe::UncachedDeclarations => (
                Some(cascade_custom_properties(
                    &cascade_scratch.events.events,
                    element,
                    custom_maps,
                    parent_custom_id,
                    &mut cascade_scratch.custom_properties,
                )),
                None,
                None,
            ),
            CustomCascadeProbe::Hit(map_id) => (None, Some(map_id), None),
            CustomCascadeProbe::Miss(signature) => (
                Some(cascade_custom_properties(
                    &cascade_scratch.events.events,
                    element,
                    custom_maps,
                    parent_custom_id,
                    &mut cascade_scratch.custom_properties,
                )),
                None,
                Some(signature),
            ),
        };
        let custom_view = custom_maps.view(
            custom_properties
                .or(reused_custom_id)
                .unwrap_or(parent_custom_id),
        );

        CascadeResolver {
            doc: self.doc,
            styles: self.styles,
            reader_overrides: self.reader_overrides,
            resolution: self.resolution,
        }
        .apply_cascade(
            &mut style,
            CascadeInputs {
                events: &cascade_scratch.events.events,
                element,
                custom_properties: &custom_view,
                cache_substitutions: true,
                parent: &parent_style,
            },
            &mut cascade_scratch.computation,
        );

        // HTML `lang` and XML `xml:lang` participate in inherited text
        // semantics even though they are not CSS properties.
        if let Some(language) = source_element
            .attr_expanded(Some("http://www.w3.org/XML/1998/namespace"), "lang")
            .or_else(|| source_element.attr("lang"))
        {
            style.text_mut(self.styles).language = self.styles.intern_language(language);
        }

        // Preserve `currentColor` as a computed keyword for inheritance while
        // caching this element's resolved RGBA for paint.
        let color = style.text(self.styles).color;
        style.border.cache_current_color(color);

        self.timings.cascade += cascade_started.elapsed();
        let custom_map = match custom_properties {
            Some(map_id) => CustomMapResult::New {
                map_id,
                cache_signature,
            },
            None => reused_custom_id.map_or(CustomMapResult::Inherited, CustomMapResult::Reused),
        };
        ElementStyleResult::Computed {
            style,
            custom_map,
            sharing_signature,
        }
    }
}

include!("tests.rs");
