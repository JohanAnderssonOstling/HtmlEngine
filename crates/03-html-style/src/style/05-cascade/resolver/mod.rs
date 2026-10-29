//! Document-level orchestration for selector matching and computed styles.
//!
//! Per-target work flows through `CascadePlan`, custom-property resolution,
//! the shared cascade runner, property-family application, and style interning.

use crate::style::compiled::CompiledStyles;
use crate::style::environment::MediaQuerySet;
use crate::style::matching::dom::{PseudoTarget, selector_matches_dom_pseudo_in_scope};
use crate::style::matching::scope::scope_match;
use crate::style::matching::selectors::{AncestorFilter, CandidateDeduper};
use crate::style::source::inline::InlineStyles;
use context::ResolutionContext;
use custom_properties::{
    CustomCascadeCache, CustomCascadeProbe, CustomCascadeSignature, cascade_custom_properties,
    effective_custom_properties,
};
use html_dom::{Document, DomNodeId};
use html_style_model::{ComputedStyles, ComputedStylesBuilder, ReaderStyleOverrides, StyleIndices};
use lightningcss::properties::custom::TokenList;
use lightningcss::stylesheet::StyleAttribute;
use local_matching::{LocalMatchCache, LocalMatchProbe};
use plan::{CascadePlan, MatchedRule};
use runner::CascadeInputs;
use rustc_data_structures::fx::FxHashMap;
use session::{ResolveStyleTimings, StyleResolverContext};
use specified::{ElementDeclarations, build_cascade_events};
use state::WorkingStyle;
use style_sharing::{
    SharedElementStyle, StyleSharingCache, StyleSharingProbe, StyleSharingSignature,
};
use traversal::extend_ancestor_filter_dom;
use values::default_justify_items;
use web_time::Instant;

mod context;
mod custom_properties;
#[path = "../html_presentational_hints.rs"]
mod html_presentational_hints;
mod local_matching;
mod plan;
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

enum CustomMapResult<'css> {
    Inherited,
    Reused(u32),
    New {
        values: FxHashMap<String, TokenList<'css>>,
        cache_signature: Option<CustomCascadeSignature>,
    },
}

enum ElementStyleResult<'css> {
    Shared(SharedElementStyle),
    Computed {
        style: WorkingStyle,
        custom_map: CustomMapResult<'css>,
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
    let mut node_custom_map_ids = vec![0u32; doc.node_count()];
    let mut custom_maps = Vec::with_capacity(16);
    custom_maps.push(FxHashMap::<String, TokenList<'static>>::default());
    let mut custom_cascade_cache = CustomCascadeCache::default();
    let mut style_sharing_cache = StyleSharingCache::default();
    let mut computed_styles = ComputedStylesBuilder::new(doc);
    let mut candidate_scratch = Vec::new();
    let mut candidate_seen = CandidateDeduper::new(index.compiled.rule_count());
    let mut local_specificity_scratch = Vec::new();
    let mut local_match_cache = LocalMatchCache::default();
    let mut matched_rule_scratch = Vec::new();
    let mut important_rule_scratch = Vec::new();
    let mut cascade_scratch = specified::CascadeScratch::default();
    let mut declaration_style = WorkingStyle::default();
    let mut ancestor_filters = vec![AncestorFilter::default(); doc.node_count()];
    timings.resolver_setup = started.elapsed();
    {
        let mut resolver = StyleResolverContext {
            doc,
            prepared,
            author_roots,
            index,
            styles: &mut computed_styles,
            reader_overrides,
            candidate_scratch: &mut candidate_scratch,
            candidate_seen: &mut candidate_seen,
            local_specificity_scratch: &mut local_specificity_scratch,
            local_match_cache: &mut local_match_cache,
            matched_rule_scratch: &mut matched_rule_scratch,
            important_rule_scratch: &mut important_rule_scratch,
            cascade_scratch: &mut cascade_scratch,
            style_sharing_cache: &mut style_sharing_cache,
            declaration_style: &mut declaration_style,
            resolution: &resolution,
            timings: &mut timings,
        };
        for node_idx in doc.node_ids() {
            // Skip text nodes
            if doc.is_text_node(node_idx) {
                continue;
            }

            let ancestor_filter = if let Some(parent_idx) = doc.get_dom_parent(node_idx) {
                let mut filter = ancestor_filters[parent_idx.index()];
                extend_ancestor_filter_dom(doc, parent_idx, &mut filter);
                filter
            } else {
                AncestorFilter::default()
            };
            ancestor_filters[node_idx.index()] = ancestor_filter;
            let parent_custom_id = doc.get_dom_parent(node_idx).map_or(0, |parent_idx| {
                node_custom_map_ids[parent_idx.index()] as usize
            });

            let style_result = resolver.compute_style_for_dom_element(
                node_idx,
                parent_custom_id as u32,
                &custom_maps,
                &custom_cascade_cache,
                &ancestor_filter,
                inline_styles.get(node_idx),
            );
            let candidate_pseudo_mask = resolver
                .candidate_scratch
                .iter()
                .fold(0, |mask, id| mask | prepared.pseudo_mask(*id));

            let started = Instant::now();
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
                            values,
                            cache_signature,
                        } => {
                            custom_maps.push(values);
                            let map_id = u32::try_from(custom_maps.len() - 1)
                                .expect("custom property map count fits in u32");
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
            let custom_map = &custom_maps[custom_map_id as usize];
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
                let Some(mut style) = resolver
                    .compute_pseudo_style_for_dom_element(node_idx, parent, pseudo, custom_map)
                else {
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
                    .intern(resolver.styles)
                    .expect("pseudo style values should pass validation");
                match (pseudo, counters) {
                    (PseudoTarget::FirstLine, _) => {
                        resolver.styles.set_first_line_style(node_idx, style)
                    }
                    (PseudoTarget::FirstLetter, _) => {
                        resolver.styles.set_first_letter_style(node_idx, style)
                    }
                    (PseudoTarget::Before, counters) => resolver.styles.set_before_style(
                        node_idx,
                        style,
                        content.unwrap(),
                        counters,
                    ),
                    (PseudoTarget::After, counters) => {
                        resolver
                            .styles
                            .set_after_style(node_idx, style, content.unwrap(), counters)
                    }
                }
                .expect("pseudo style belongs to its originating element");
                match pseudo {
                    PseudoTarget::FirstLine => first_line = Some(style),
                    PseudoTarget::Before => {
                        before_first_letter_parent = if let Some(first_line) = first_line {
                            resolver
                                .compute_pseudo_style_for_dom_element(
                                    node_idx,
                                    first_line,
                                    PseudoTarget::Before,
                                    custom_map,
                                )
                                .and_then(|mut style| {
                                    style.generated_content.take().map(|_| {
                                        style
                                            .intern(resolver.styles)
                                            .expect("pseudo style values should pass validation")
                                    })
                                })
                        } else {
                            Some(style)
                        };
                    }
                    PseudoTarget::FirstLetter => {
                        if let Some(parent) = before_first_letter_parent
                            && let Some(style) = resolver.compute_pseudo_style_for_dom_element(
                                node_idx,
                                parent,
                                PseudoTarget::FirstLetter,
                                custom_map,
                            )
                        {
                            let style = style
                                .intern(resolver.styles)
                                .expect("pseudo style values should pass validation");
                            resolver
                                .styles
                                .set_before_first_letter_style(node_idx, style)
                                .expect("pseudo style belongs to its originating element");
                        }
                    }
                    PseudoTarget::After => {}
                }
            }
            node_custom_map_ids[node_idx.index()] = custom_map_id;
            resolver.timings.style_store += started.elapsed();
        }
    }
    let started = Instant::now();
    computed_styles.apply_reader_overrides(doc, reader_overrides);
    let styles = computed_styles
        .finish()
        .expect("resolver must assign a valid computed style to every element");
    timings.style_store += started.elapsed();
    let mut media_queries = prepared.media_queries().clone();
    if resolution.uses_viewport_units.get() {
        media_queries.mark_viewport_unit_dependency();
    }
    (styles, timings, media_queries)
}

impl<'a, 'css> StyleResolverContext<'a, 'css> {
    fn compute_pseudo_style_for_dom_element(
        &mut self,
        node_idx: DomNodeId,
        inherited_from: StyleIndices,
        pseudo: PseudoTarget,
        inherited_custom_properties: &FxHashMap<String, TokenList<'css>>,
    ) -> Option<WorkingStyle> {
        let doc = self.doc;
        let prepared = self.prepared;
        let mut matched_rules = std::mem::take(self.matched_rule_scratch);
        matched_rules.clear();
        for id in self.candidate_scratch.iter().copied() {
            if prepared.pseudo_mask(id) & pseudo.mask() == 0 {
                continue;
            }
            let style_rule = prepared.get(id).style_rule();
            let Some(scope_match) = scope_match(
                prepared,
                prepared.get(id).scope(),
                doc,
                self.author_roots,
                node_idx,
            ) else {
                continue;
            };
            // Parcel selectors represents the pseudo-to-originating-element hop as
            // a synthetic combinator. The ancestor bloom filter treats that as a
            // real tree hop, so it cannot safely prefilter pseudo selectors.
            let mut specificity = None;
            for (selector, prepared_selector) in style_rule
                .selectors
                .0
                .iter()
                .zip(self.index.compiled.for_rule(id))
            {
                if selector_matches_dom_pseudo_in_scope(
                    selector,
                    doc,
                    node_idx,
                    pseudo,
                    scope_match.root,
                ) {
                    let matched = prepared_selector.specificity();
                    specificity =
                        Some(specificity.map_or(matched, |current: u32| current.max(matched)));
                }
            }
            if let Some(specificity) = specificity {
                matched_rules.push(MatchedRule {
                    specificity,
                    id,
                    scope_proximity: scope_match.proximity,
                });
            }
        }
        if matched_rules.is_empty() {
            *self.matched_rule_scratch = matched_rules;
            return None;
        }

        let CascadePlan {
            normal: matched_rules,
            important: important_rules,
        } = CascadePlan::build(matched_rules, self.important_rule_scratch, prepared);

        let inherited = WorkingStyle::from_indices(self.styles, inherited_from);
        // Pseudo-elements retain initial layout values; ordinary elements also
        // inherit the legacy justify-items behavior from their parent.
        let mut style =
            WorkingStyle::inherited(&inherited.font, &inherited.text, &inherited.box_model);
        let mut cascade_scratch = std::mem::take(self.cascade_scratch);
        build_cascade_events(
            prepared,
            &matched_rules,
            &important_rules,
            ElementDeclarations::default(),
            &mut cascade_scratch.events,
        );
        *self.matched_rule_scratch = matched_rules;
        *self.important_rule_scratch = important_rules;
        let custom_properties = cascade_custom_properties(
            &cascade_scratch.events.events,
            ElementDeclarations::default(),
            inherited_custom_properties,
        );
        let custom_properties =
            effective_custom_properties(&custom_properties, inherited_custom_properties);

        self.apply_cascade(
            &mut style,
            CascadeInputs {
                events: &cascade_scratch.events.events,
                element: ElementDeclarations::default(),
                custom_properties,
                parent: &inherited,
            },
            &mut cascade_scratch.computation,
        );

        *self.cascade_scratch = cascade_scratch;
        Some(style)
    }
}

/// Compute style for a single DOM element (used in new pipeline)
impl<'a, 'css> StyleResolverContext<'a, 'css> {
    fn compute_style_for_dom_element(
        &mut self,
        node_idx: DomNodeId,
        parent_custom_id: u32,
        custom_maps: &[FxHashMap<String, TokenList<'css>>],
        custom_cascade_cache: &CustomCascadeCache,
        ancestor_filter: &AncestorFilter,
        inline_style: Option<&StyleAttribute<'css>>,
    ) -> ElementStyleResult<'css> {
        let doc = self.doc;
        let prepared = self.prepared;
        let index = self.index;
        let matching_started = Instant::now();
        let mut matched_rules = std::mem::take(self.matched_rule_scratch);
        matched_rules.clear();

        // Get element info for candidate lookup
        let tag = doc.get_dom_tag(node_idx).unwrap_or("");
        let id = doc.get_dom_id(node_idx);

        let local_probe = self.local_match_cache.probe(
            doc,
            node_idx,
            self.candidate_scratch,
            self.local_specificity_scratch,
        );
        let local_hit = matches!(&local_probe, LocalMatchProbe::Hit);
        if !local_hit {
            index.collect_candidates(
                tag,
                id,
                doc.get_dom_classes(node_idx),
                self.candidate_scratch,
                self.candidate_seen,
            );
            self.local_specificity_scratch.clear();
            self.local_specificity_scratch
                .resize(self.candidate_scratch.len(), None);
        }

        // Check each candidate. A rule cascades with the most specific of its
        // matching selectors, not the first one that happens to match.
        for (candidate_index, id) in self.candidate_scratch.iter().copied().enumerate() {
            let style_rule = prepared.get(id).style_rule();
            let mut specificity = self.local_specificity_scratch[candidate_index];
            if !local_hit {
                for prepared_selector in index.compiled.for_rule(id) {
                    if !prepared_selector.is_context_free() {
                        continue;
                    }
                    debug_assert!(index.selector_might_match(*prepared_selector, ancestor_filter));
                    if index.compiled.matches(*prepared_selector, doc, node_idx) == Some(true) {
                        let matched = prepared_selector.specificity();
                        specificity =
                            Some(specificity.map_or(matched, |current| current.max(matched)));
                    }
                }
                self.local_specificity_scratch[candidate_index] = specificity;
            }
            let Some(scope_match) = scope_match(
                prepared,
                prepared.get(id).scope(),
                doc,
                self.author_roots,
                node_idx,
            ) else {
                continue;
            };
            for (selector, prepared_selector) in style_rule
                .selectors
                .0
                .iter()
                .zip(index.compiled.for_rule(id))
            {
                if prepared_selector.is_context_free() {
                    continue;
                }
                if !index.selector_might_match(*prepared_selector, ancestor_filter) {
                    continue;
                }
                let matches = index
                    .compiled
                    .matches(*prepared_selector, doc, node_idx)
                    .unwrap_or_else(|| {
                        crate::style::matching::dom::selector_matches_dom_node_in_scope(
                            selector,
                            doc,
                            node_idx,
                            scope_match.root,
                        )
                    });
                if matches {
                    let matched = prepared_selector.specificity();
                    specificity =
                        Some(specificity.map_or(matched, |current: u32| current.max(matched)));
                }
            }
            if let Some(specificity) = specificity {
                matched_rules.push(MatchedRule {
                    specificity,
                    id,
                    scope_proximity: scope_match.proximity,
                });
            }
        }
        if let LocalMatchProbe::Miss(signature) = local_probe {
            self.local_match_cache.insert(
                signature,
                self.candidate_scratch,
                self.local_specificity_scratch,
            );
        }

        self.timings.selector_matching += matching_started.elapsed();

        // Build computed style from matched rules (same as box-based version)
        let cascade_started = Instant::now();
        let parent_style_indices = doc
            .get_dom_parent(node_idx)
            .and_then(|parent| self.styles.style_for_node(parent));
        let sharing_signature = match self.style_sharing_cache.probe(
            doc,
            node_idx,
            inline_style.is_some(),
            parent_style_indices,
            parent_custom_id,
            &matched_rules,
        ) {
            StyleSharingProbe::Hit(shared) => {
                *self.matched_rule_scratch = matched_rules;
                self.timings.cascade += cascade_started.elapsed();
                return ElementStyleResult::Shared(shared);
            }
            StyleSharingProbe::Miss(signature) => Some(signature),
            StyleSharingProbe::Uncacheable => None,
        };
        // A sharing hit needs neither cascade ordering nor the important-rule
        // split. The exact deterministic match sequence is already the key.
        let CascadePlan {
            normal: matched_rules,
            important: important_rules,
        } = CascadePlan::build(matched_rules, self.important_rule_scratch, prepared);
        let parent_style = WorkingStyle::for_node(doc, self.styles, node_idx);
        let mut style = WorkingStyle::inherited(
            &parent_style.font,
            &parent_style.text,
            &parent_style.box_model,
        );
        style.layout.justify_items = default_justify_items(parent_style.layout.justify_items);
        let mut cascade_scratch = std::mem::take(self.cascade_scratch);
        html_presentational_hints::collect(doc, node_idx, &mut cascade_scratch.hints);
        let element = ElementDeclarations {
            inline: inline_style,
            hints: &cascade_scratch.hints,
        };
        build_cascade_events(
            prepared,
            &matched_rules,
            &important_rules,
            element,
            &mut cascade_scratch.events,
        );
        *self.matched_rule_scratch = matched_rules;
        *self.important_rule_scratch = important_rules;
        let parent_custom = &custom_maps[parent_custom_id as usize];
        let (custom_properties, reused_custom_id, cache_signature) = match custom_cascade_cache
            .probe(&cascade_scratch.events.events, element, parent_custom_id)
        {
            CustomCascadeProbe::NoDeclarations => (None, None, None),
            CustomCascadeProbe::UncachedDeclarations => (
                cascade_custom_properties(&cascade_scratch.events.events, element, parent_custom),
                None,
                None,
            ),
            CustomCascadeProbe::Hit(map_id) => (None, Some(map_id), None),
            CustomCascadeProbe::Miss(signature) => (
                cascade_custom_properties(&cascade_scratch.events.events, element, parent_custom),
                None,
                Some(signature),
            ),
        };
        let custom_base = reused_custom_id
            .map(|map_id| &custom_maps[map_id as usize])
            .unwrap_or(parent_custom);

        self.apply_cascade(
            &mut style,
            CascadeInputs {
                events: &cascade_scratch.events.events,
                element,
                custom_properties: effective_custom_properties(&custom_properties, custom_base),
                parent: &parent_style,
            },
            &mut cascade_scratch.computation,
        );

        *self.cascade_scratch = cascade_scratch;

        // HTML `lang` and XML `xml:lang` participate in inherited text
        // semantics even though they are not CSS properties.
        if let Some(language) = doc.element_ref(node_idx).and_then(|element| {
            element
                .attr_expanded(Some("http://www.w3.org/XML/1998/namespace"), "lang")
                .or_else(|| element.attr("lang"))
        }) {
            style.text.language = self.styles.intern_language(language);
        }

        // Preserve `currentColor` as a computed keyword for inheritance while
        // caching this element's resolved RGBA for paint.
        style.border.cache_current_color(style.text.color);

        self.timings.cascade += cascade_started.elapsed();
        let custom_map = match custom_properties {
            Some(values) => CustomMapResult::New {
                values,
                cache_signature,
            },
            None => reused_custom_id
                .map(CustomMapResult::Reused)
                .unwrap_or(CustomMapResult::Inherited),
        };
        ElementStyleResult::Computed {
            style,
            custom_map,
            sharing_signature,
        }
    }
}

include!("tests.rs");
