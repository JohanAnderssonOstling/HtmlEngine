//! Document-level orchestration for selector matching and computed styles.
//!
//! Per-target work flows through `CascadePlan`, custom-property resolution,
//! the shared cascade runner, property-family application, and style interning.

use crate::style::compiled::{
    CompiledStyles, PreparedRuleSet, ScopeId, SelectorCandidate, SelectorIndex,
};
use crate::style::environment::MediaQuerySet;
use crate::style::matching::dom::{MatchCaches, PseudoTarget, TraversalContext};
use crate::style::matching::scope::scope_match;
use crate::style::matching::{BoundSelectors, StyleTraversal};
use crate::style::source::inline::InlineStyles;
use crate::style::source::inline::PreparedInlineStyle;
use context::ResolutionContext;
use custom_properties::{
    CustomCascadeCache, CustomCascadeProbe, CustomPropertyStore, cascade_custom_properties,
};
use html_dom::{Document, DomNodeId, ElementRef};
use html_style_model::{ComputedStyles, ComputedStylesBuilder, ReaderStyleOverrides, StyleIndices};
use local_matching::{LocalMatchCache, LocalMatchProbe, PseudoCandidates};
use plan::{CascadePlan, MatchedRule};
use runner::{CascadeInputs, CascadeResolver};
use session::{PhaseTimer, ResolveStyleTimings, StyleResolverContext};
use specified::{ElementDeclarations, build_cascade_events};
use state::{ParentStyle, WorkingStyle};
use style_sharing::{SharedElementStyle, StyleSharingCache, StyleSharingProbe, StyleSharingSignature};
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

enum CascadeTarget<'target, 'css> {
    Element {
        inline: Option<&'target PreparedInlineStyle<'css>>,
        custom_cache: &'target mut CustomCascadeCache,
    },
    Pseudo,
}

enum ElementStyleResult {
    Shared(SharedElementStyle),
    Computed {
        style: WorkingStyle,
        custom_map_id: u32,
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
            matched_rules: CascadePlan::default(),
            cascade_scratch,
            style_sharing_cache,
            resolution: &mut resolution,
            timings: &mut timings,
        };
        while let Some(element) = traversal.next() {
            let mut sibling_index = TraversalContext::new(doc, &mut traversal, &mut match_caches);
            let node_idx = element.node_id();
            let parent_custom_id = doc.node_ref(node_idx).parent().map_or(0, |parent_idx| {
                node_custom_map_ids[parent_idx.index()] as usize
            });

            let inline_style = inline_styles.get(node_idx);
            html_presentational_hints::collect(doc, element, &mut resolver.cascade_scratch.hints);
            let has_element_declarations =
                inline_style.is_some() || !resolver.cascade_scratch.hints.is_empty();
            let style_result = resolver.compute_style_for_dom_element(
                &mut sibling_index,
                element,
                parent_custom_id as u32,
                &mut custom_maps,
                &mut custom_cascade_cache,
                inline_style,
                has_element_declarations,
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
                    custom_map_id,
                    sharing_signature,
                } => {
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
                has_element_declarations,
                &index.compiled,
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
    siblings: &'a mut TraversalContext<'cache, 'dom, 'css>,
}

#[derive(Clone, Copy)]
enum MatchTarget {
    Local,
    Element,
    Pseudo(PseudoTarget),
}

impl RuleMatcher<'_, '_, '_, '_> {
    fn match_rule(
        &mut self,
        node: DomNodeId,
        candidates: &[SelectorCandidate],
        target: MatchTarget,
        mut specificity: Option<u32>,
    ) -> Option<MatchedRule> {
        let id = candidates[0].rule;
        let scope_id = self.prepared.scope(id);
        let scope = if matches!(target, MatchTarget::Local) || scope_id == ScopeId::NONE {
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
            let target_matches = match target {
                MatchTarget::Local => selector.is_context_free(),
                MatchTarget::Element => {
                    !selector.has_pseudo_target() && !selector.is_context_free()
                }
                MatchTarget::Pseudo(pseudo) => selector.targets_pseudo(pseudo.mask()),
            };
            if !target_matches || specificity.is_some_and(|value| value >= selector.specificity()) {
                continue;
            }
            if !self
                .index
                .selector_might_match(selector, node, self.siblings)
            {
                continue;
            }
            let matches = self.selectors.matches_selector(
                selector,
                &self.prepared.get(id).style_rule().selectors.0[selector_index],
                node,
                match target {
                    MatchTarget::Pseudo(pseudo) => Some(pseudo),
                    _ => None,
                },
                scope.map(|scope| scope.root),
                self.siblings,
            );
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
        sibling_index: &mut TraversalContext<'_, 'a, 'css>,
        node_idx: DomNodeId,
        style_indices: StyleIndices,
        candidate_pseudo_mask: u8,
        custom_maps: &mut CustomPropertyStore<'css>,
        custom_map_id: u32,
    ) {
        let mut plan = std::mem::take(&mut self.matched_rules);
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
            plan.normal.clear();
            let mut matcher = RuleMatcher {
                doc: self.doc,
                prepared: self.prepared,
                index: self.index,
                selectors: self.selectors,
                roots: self.author_roots,
                siblings: sibling_index,
            };
            for candidates in self
                .local_match_cache
                .pseudo_candidates()
                .unwrap_or(&self.pseudo_candidate_scratch)
                .for_target(pseudo)
                .chunk_by(|a, b| a.rule == b.rule)
            {
                if let Some(matched) =
                    matcher.match_rule(node_idx, candidates, MatchTarget::Pseudo(pseudo), None)
                {
                    plan.normal.push(matched);
                }
            }
            if plan.normal.is_empty() {
                continue;
            }
            plan.build(self.prepared);
            let mut style = self.cascade_pseudo(&plan, parent, custom_maps, custom_map_id);
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
                        let mut style =
                            self.cascade_pseudo(&plan, first_line, custom_maps, custom_map_id);
                        style.generated_content.take().map(|_| {
                            style
                                .intern(self.styles)
                                .expect("pseudo style values should pass validation")
                        })
                    } else {
                        Some(style)
                    };
                }
                PseudoTarget::FirstLetter => {
                    if let Some(parent) = before_first_letter_parent {
                        let style = self.cascade_pseudo(&plan, parent, custom_maps, custom_map_id);
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
        self.matched_rules = plan;
    }

    fn cascade_pseudo(
        &mut self,
        plan: &CascadePlan,
        inherited_from: StyleIndices,
        custom_maps: &mut CustomPropertyStore<'css>,
        inherited_custom_id: u32,
    ) -> WorkingStyle {
        let checkpoint = custom_maps.checkpoint();
        let (style, _) = self.cascade_target(
            CascadeTarget::Pseudo,
            plan,
            Some(inherited_from),
            custom_maps,
            inherited_custom_id,
        );
        custom_maps.rewind(checkpoint);
        style
    }
}

/// Compute style for a single DOM element (used in new pipeline)
impl<'a, 'css> StyleResolverContext<'a, 'css> {
    fn compute_style_for_dom_element(
        &mut self,
        sibling_index: &mut TraversalContext<'_, 'a, 'css>,
        source_element: ElementRef<'_>,
        parent_custom_id: u32,
        custom_maps: &mut CustomPropertyStore<'css>,
        custom_cascade_cache: &mut CustomCascadeCache,
        inline_style: Option<&PreparedInlineStyle<'css>>,
        has_element_declarations: bool,
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
                    .early_style(doc, source_element, has_element_declarations)
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
        self.matched_rules.normal.clear();
        let matched_rules = &mut self.matched_rules.normal;
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
            let mut specificity = if local_hit {
                self.local_match_cache
                    .specificities()
                    .expect("cached local matches")[candidate_index]
            } else {
                None
            };
            if !local_hit {
                specificity = matcher
                    .match_rule(node_idx, candidates, MatchTarget::Local, None)
                    .map(|matched| matched.specificity);
                self.local_specificity_scratch.push(specificity);
            }
            if let Some(matched) =
                matcher.match_rule(node_idx, candidates, MatchTarget::Element, specificity)
            {
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
            has_element_declarations,
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
        self.matched_rules.build(prepared);
        let plan = std::mem::take(&mut self.matched_rules);
        let (mut style, custom_map_id) = self.cascade_target(
            CascadeTarget::Element {
                inline: inline_style,
                custom_cache: custom_cascade_cache,
            },
            &plan,
            parent_style_indices,
            custom_maps,
            parent_custom_id,
        );

        self.matched_rules = plan;

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
        ElementStyleResult::Computed {
            style,
            custom_map_id,
            sharing_signature,
        }
    }

    fn cascade_target(
        &mut self,
        target: CascadeTarget<'_, 'css>,
        plan: &CascadePlan,
        parent: Option<StyleIndices>,
        custom_maps: &mut CustomPropertyStore<'css>,
        inherited_custom_id: u32,
    ) -> (WorkingStyle, u32) {
        let parent = ParentStyle::new(parent, self.doc.root_font_size());
        let mut style = WorkingStyle::inherited(&parent, self.styles);
        let (element, mut custom_cache) = match target {
            CascadeTarget::Element {
                inline,
                custom_cache,
            } => {
                style.layout.justify_items = parent.with(self.styles, |parent| {
                    default_justify_items(parent.layout.justify_items)
                });
                self.cascade_scratch.hint_preparation.clear();
                self.cascade_scratch
                    .hint_preparation
                    .append(&self.cascade_scratch.hints);
                (
                    ElementDeclarations {
                        prepared: Some(self.prepared),
                        inline,
                        hints: &self.cascade_scratch.hints,
                        hint_preparation: Some(&self.cascade_scratch.hint_preparation),
                    },
                    Some(custom_cache),
                )
            }
            CascadeTarget::Pseudo => (
                ElementDeclarations {
                    prepared: Some(self.prepared),
                    ..ElementDeclarations::default()
                },
                None,
            ),
        };
        build_cascade_events(
            self.prepared,
            &plan.normal,
            &plan.important,
            element,
            &mut self.cascade_scratch.events,
        );
        let probe =
            custom_cache
                .as_mut()
                .map_or(CustomCascadeProbe::UncachedDeclarations, |cache| {
                    cache.probe(
                        &self.cascade_scratch.events.events,
                        element,
                        inherited_custom_id,
                    )
                });
        let custom_map_id = match probe {
            CustomCascadeProbe::NoDeclarations => inherited_custom_id,
            CustomCascadeProbe::Hit(map_id) => map_id,
            CustomCascadeProbe::UncachedDeclarations | CustomCascadeProbe::Miss(_) => {
                let map_id = cascade_custom_properties(
                    &self.cascade_scratch.events.events,
                    element,
                    custom_maps,
                    inherited_custom_id,
                    &mut self.cascade_scratch.custom_properties,
                );
                if let CustomCascadeProbe::Miss(signature) = probe {
                    custom_cache
                        .as_mut()
                        .expect("cache probe requires a cache")
                        .insert(signature, map_id);
                }
                map_id
            }
        };
        let custom_properties = custom_maps.view(custom_map_id);
        CascadeResolver {
            doc: self.doc,
            styles: self.styles,
            reader_overrides: self.reader_overrides,
            resolution: self.resolution,
        }
        .apply_cascade(
            &mut style,
            CascadeInputs {
                events: &self.cascade_scratch.events.events,
                element,
                custom_properties: &custom_properties,
                // Pseudo maps are temporary and their IDs can be reused after rewind.
                cache_substitutions: custom_cache.is_some(),
                parent: &parent,
            },
            &mut self.cascade_scratch.computation,
        );
        (style, custom_map_id)
    }
}

include!("tests.rs");
