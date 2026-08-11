//! Document-level orchestration for selector matching and computed styles.
//!
//! Per-target work flows through `CascadePlan`, custom-property resolution,
//! the shared cascade runner, property-family application, and style interning.

use crate::style::matching::dom::{PseudoTarget, selector_matches_dom_pseudo_in_scope};
use crate::style::matching::selectors::{AncestorFilter, CandidateDeduper, SelectorIndex, selector_might_match_ancestors};
use crate::style::rules::prepared::{
    CascadeOrigin, EffectiveRuleId, PreparedPropertyTarget, PreparedRuleSet, RulePriority,
    compile_property_targets,
};
use crate::style::source::declarations::normalize as normalize_declarations;
use crate::style::syntax::values::tab_size::{
    CASCADE_MARKER as TAB_SIZE_CASCADE_MARKER, ParsedTabSize, parse as parse_tab_size,
};
use crate::style::syntax::values::text_spacing::{
    LETTER_SPACING_MARKER, ParsedSpacing, WORD_SPACING_MARKER, parse as parse_text_spacing,
};
use crate::style::syntax::values::white_space::{
    CASCADE_MARKER as WHITE_SPACE_CASCADE_MARKER, from_marker_tokens,
};
use html_dom::{Document, DomNodeId};
use html_style_model::{
    AspectRatio as ComputedAspectRatio, Background, Border, BorderRadii, BorderStyle, BoxModel,
    BoxSizing, BreakBetween, BreakInside, Clear,
    ComputedSizeComponent, ComputedStyleValueError, ComputedStyles, ComputedStylesBuilder,
    ContentAlignment, CornerRadius, CounterDirective, CounterDirectives, CounterStyle,
    DecorationColor, Display, FlexDirection, FlexWrap, Float, Font,
    FontRelativeLength, FontStyle, GeneratedContent, GeneratedContentItem, GridAutoFlow,
    GridPlacement, GridPlacementRange, GridRepeatCount, GridTemplateArea, GridTemplateTrack,
    GridTrackBreadth, GridTrackSize, Hyphens, InheritedText, ItemAlignment, LayoutStyle, LengthPct,
    LogicalTextAlign, ObjectFit, ObjectPosition, ObjectPositionAxis, ObjectPositionOrigin,
    OpenTypeFeature, OverflowMode, OverflowWrap, PositionMode, PreferredSize,
    QuoteStyle, SizeComparison, StyleIndices, StyleStringId, TabSize, TextAlign,
    TextDecorationLines, TextDecorationStyle, TextDecorationThickness, TextDirection,
    TextOverflow, TextSpacing, TextTransform, VerticalAlignValue, Visibility, WhiteSpace, WordBreak,
};
use lightningcss::printer::{Printer, PrinterOptions};
use lightningcss::properties::border::{BorderSideWidth, LineStyle};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue, Variable};
use lightningcss::properties::font::LineHeight as CssLineHeight;
use lightningcss::properties::font::{
    AbsoluteFontWeight, Font as CssFont, FontFamily, FontSize, FontWeight, VerticalAlign,
    VerticalAlignKeyword,
};
use lightningcss::properties::overflow::{OverflowKeyword, TextOverflow as CssTextOverflow};
use lightningcss::properties::size::{MaxSize, Size};
use lightningcss::properties::text::Spacing;
use lightningcss::properties::{CSSWideKeyword, Property, PropertyId};
use lightningcss::stylesheet::{ParserOptions, StyleAttribute};
use lightningcss::traits::{Parse, ParseWithOptions, ToCss};
use lightningcss::values::calc::{Calc, MathFunction};
use lightningcss::values::color::{CssColor, SystemColor};
use lightningcss::values::length::{Length, LengthPercentage, LengthPercentageOrAuto, LengthValue};
use lightningcss::values::size::Size2D;
use lightningcss::visitor::{Visit, Visitor};
use rustc_data_structures::fx::FxHashMap;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::ops::Deref;
use std::time::{Duration, Instant};

mod custom_properties;
#[path = "../html_presentational_hints.rs"]
mod html_presentational_hints;
mod parent_style;
mod plan;
mod properties;
mod runner;
mod specified;
mod state;
mod style_sharing;
mod traversal;
mod values;
mod wide_keywords;

use custom_properties::*;
use parent_style::ParentStyle;
use plan::*;
use properties::{apply_property_in_phase, property_is_computable};
use runner::{CascadeInputs, CascadePhase};
use state::{WorkingStyle, initial_border, initial_box_model};
use style_sharing::{SharedElementStyle, StyleSharingCache, StyleSharingProbe, StyleSharingSignature};
use traversal::*;
pub(crate) use values::parse_font_kerning;
use values::*;
use wide_keywords::*;

// ============================================================================
// Style Resolver - applies CSS styles to Document boxes
// ============================================================================
/// Resolve styles for all DOM elements
/// This works with the DOM tree before boxes are created
#[derive(Clone, Debug, Default)]
pub(crate) struct ResolveStyleTimings {
    pub selector_index: Duration,
    pub resolver_setup: Duration,
    pub selector_matching: Duration,
    pub cascade: Duration,
    pub style_store: Duration,
}

pub(super) struct StyleResolverContext<'a, 'sheet, 'css> {
    pub(super) doc: &'a Document,
    pub(super) prepared: &'a PreparedRuleSet<'sheet, 'css>,
    pub(super) index: &'a SelectorIndex,
    pub(super) styles: &'a mut ComputedStylesBuilder,
    pub(super) candidate_scratch: &'a mut Vec<EffectiveRuleId>,
    pub(super) candidate_seen: &'a mut CandidateDeduper,
    matched_rule_scratch: &'a mut Vec<MatchedRule>,
    important_rule_scratch: &'a mut Vec<MatchedRule>,
    property_targets: &'a mut specified::PropertyTargetState,
    cascade_scratch: &'a mut specified::CascadeScratch<'a, 'css>,
    style_sharing_cache: &'a mut StyleSharingCache,
    validation_style: &'a mut WorkingStyle,
    pub(super) timings: &'a mut ResolveStyleTimings,
}

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

thread_local! {
    /// Length conversion is a synchronous part of one style-resolution pass.
    /// Keeping its immutable media environment thread-local avoids threading
    /// duplicate viewport arguments through every typed property converter,
    /// while remaining isolated when documents are styled in parallel.
    static RESOLUTION_MEDIA_ENVIRONMENT: Cell<crate::MediaEnvironment> = Cell::new(crate::MediaEnvironment::default());
    static RESOLUTION_USES_VIEWPORT_UNITS: Cell<bool> = const { Cell::new(false) };
    /// The computed line height against which `lh` resolves for the property
    /// currently being converted. `None` means no finite basis is available.
    static RESOLUTION_LINE_HEIGHT: Cell<Option<f32>> = const { Cell::new(None) };
    /// The root element's computed line height for `rlh` conversion.
    static RESOLUTION_ROOT_LINE_HEIGHT: Cell<Option<f32>> = const { Cell::new(None) };
}

pub(crate) fn resolve_styles_for_dom_timed<'sheet, 'css>(
    doc: &Document,
    prepared: &PreparedRuleSet<'sheet, 'css>,
) -> (ComputedStyles, ResolveStyleTimings) {
    RESOLUTION_MEDIA_ENVIRONMENT.set(prepared.environment());
    RESOLUTION_USES_VIEWPORT_UNITS.set(false);
    RESOLUTION_ROOT_LINE_HEIGHT.set(None);
    // Build selector index for fast candidate lookup
    let started = Instant::now();
    let index = SelectorIndex::from_prepared(prepared);
    let mut timings = ResolveStyleTimings {
        selector_index: started.elapsed(),
        ..ResolveStyleTimings::default()
    };

    // Process each DOM element
    let started = Instant::now();
    let mut node_custom_map_ids = vec![0u32; doc.node_count()];
    let mut custom_maps = Vec::with_capacity(16);
    custom_maps.push(FxHashMap::<String, TokenList<'css>>::default());
    let mut custom_cascade_cache = CustomCascadeCache::default();
    let mut style_sharing_cache = StyleSharingCache::default();
    let mut computed_styles = ComputedStylesBuilder::new(doc);
    let mut candidate_scratch = Vec::new();
    let mut candidate_seen = CandidateDeduper::new(index.rule_count());
    let mut matched_rule_scratch = Vec::new();
    let mut important_rule_scratch = Vec::new();
    let mut property_targets = specified::PropertyTargetState::default();
    let mut cascade_scratch = specified::CascadeScratch::default();
    let mut validation_style = WorkingStyle::default();
    let mut ancestor_filters = vec![AncestorFilter::default(); doc.node_count()];
    let inline_styles = InlineStyleCache::new(doc);
    timings.resolver_setup = started.elapsed();
    {
        let mut resolver = StyleResolverContext {
            doc,
            prepared,
            index: &index,
            styles: &mut computed_styles,
            candidate_scratch: &mut candidate_scratch,
            candidate_seen: &mut candidate_seen,
            matched_rule_scratch: &mut matched_rule_scratch,
            important_rule_scratch: &mut important_rule_scratch,
            property_targets: &mut property_targets,
            cascade_scratch: &mut cascade_scratch,
            style_sharing_cache: &mut style_sharing_cache,
            validation_style: &mut validation_style,
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
            let parent_custom_id = doc
                .get_dom_parent(node_idx)
                .map_or(0, |parent_idx| node_custom_map_ids[parent_idx.index()] as usize);

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
                ElementStyleResult::Shared(shared) => (shared.style, shared.custom_map_id, shared.counters, None),
                ElementStyleResult::Computed { mut style, custom_map, sharing_signature } => {
                    let custom_map_id = match custom_map {
                        CustomMapResult::Inherited => parent_custom_id as u32,
                        CustomMapResult::Reused(map_id) => map_id,
                        CustomMapResult::New { values, cache_signature } => {
                            custom_maps.push(values);
                            let map_id = u32::try_from(custom_maps.len() - 1).expect("custom property map count fits in u32");
                            if let Some(signature) = cache_signature {
                                custom_cascade_cache.insert(signature, map_id);
                            }
                            map_id
                        }
                    };
                    let counters = std::mem::take(&mut style.counters);
                    let style_indices = style.intern(resolver.styles).expect("resolver built style values should pass style validation");
                    (style_indices, custom_map_id, counters, sharing_signature)
                }
            };
            resolver
                .styles
                .set_node_style(node_idx, style_indices)
                .expect("resolver only assigns styles to nodes from its source document");
            if let Some(signature) = sharing_signature {
                resolver.style_sharing_cache.insert(signature, style_indices, custom_map_id, counters.clone());
            }
            if !counters.is_empty() {
                resolver
                    .styles
                    .set_counter_directives(node_idx, counters)
                    .expect("counter directives belong to their originating element");
            }
            let custom_map = &custom_maps[custom_map_id as usize];
            let first_line = if candidate_pseudo_mask & PseudoTarget::FirstLine.mask() != 0 {
                resolver
                    .compute_pseudo_style_for_dom_element(
                        node_idx,
                        style_indices,
                        PseudoTarget::FirstLine,
                        custom_map,
                    )
                    .map(|style| {
                        style
                            .intern(resolver.styles)
                            .expect("pseudo style values should pass validation")
                    })
            } else {
                None
            };
            if let Some(style) = first_line {
                resolver
                    .styles
                    .set_first_line_style(node_idx, style)
                    .expect("pseudo style belongs to its originating element");
            }
            let mut before_first_letter_parent = None;
            if candidate_pseudo_mask & PseudoTarget::Before.mask() != 0
                && let Some(mut style) = resolver.compute_pseudo_style_for_dom_element(
                    node_idx,
                    style_indices,
                    PseudoTarget::Before,
                    custom_map,
                )
                && let Some(content) = style.generated_content.take()
            {
                let counters = std::mem::take(&mut style.counters);
                let style = style
                    .intern(resolver.styles)
                    .expect("pseudo style values should pass validation");
                resolver
                    .styles
                    .set_before_style(node_idx, style, content, counters)
                    .expect("pseudo style belongs to its originating element");
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
            let first_letter_parent = first_line.unwrap_or(style_indices);
            let first_letter = if candidate_pseudo_mask & PseudoTarget::FirstLetter.mask() != 0 {
                resolver
                    .compute_pseudo_style_for_dom_element(
                        node_idx,
                        first_letter_parent,
                        PseudoTarget::FirstLetter,
                        custom_map,
                    )
                    .map(|style| {
                        style
                            .intern(resolver.styles)
                            .expect("pseudo style values should pass validation")
                    })
            } else {
                None
            };
            if let Some(style) = first_letter {
                resolver
                    .styles
                    .set_first_letter_style(node_idx, style)
                    .expect("pseudo style belongs to its originating element");
            }
            if candidate_pseudo_mask & PseudoTarget::FirstLetter.mask() != 0
                && let Some(parent) = before_first_letter_parent
                && let Some(style) = resolver
                    .compute_pseudo_style_for_dom_element(
                        node_idx,
                        parent,
                        PseudoTarget::FirstLetter,
                        custom_map,
                    )
                    .map(|style| {
                        style
                            .intern(resolver.styles)
                            .expect("pseudo style values should pass validation")
                    })
            {
                resolver
                    .styles
                    .set_before_first_letter_style(node_idx, style)
                    .expect("pseudo style belongs to its originating element");
            }
            if candidate_pseudo_mask & PseudoTarget::After.mask() != 0
                && let Some(mut style) = resolver.compute_pseudo_style_for_dom_element(
                    node_idx,
                    style_indices,
                    PseudoTarget::After,
                    custom_map,
                )
                && let Some(content) = style.generated_content.take()
            {
                let counters = std::mem::take(&mut style.counters);
                let style = style
                    .intern(resolver.styles)
                    .expect("pseudo style values should pass validation");
                resolver
                    .styles
                    .set_after_style(node_idx, style, content, counters)
                    .expect("pseudo style belongs to its originating element");
            }
            node_custom_map_ids[node_idx.index()] = custom_map_id;
            resolver.timings.style_store += started.elapsed();
        }
    }
    let started = Instant::now();
    let styles = computed_styles
        .finish()
        .expect("resolver must assign a valid computed style to every element");
    timings.style_store += started.elapsed();
    if RESOLUTION_USES_VIEWPORT_UNITS.get() {
        prepared.media_queries().mark_viewport_unit_dependency();
    }
    (styles, timings)
}

impl<'a, 'sheet, 'css> StyleResolverContext<'a, 'sheet, 'css> {
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
            let Some(scope_match) = prepared.scope_match(prepared.get(id).scope(), doc, node_idx)
            else {
                continue;
            };
            // Parcel selectors represents the pseudo-to-originating-element hop as
            // a synthetic combinator. The ancestor bloom filter treats that as a
            // real tree hop, so it cannot safely prefilter pseudo selectors.
            let specificity = style_rule
                .selectors
                .0
                .iter()
                .filter(|selector| {
                    selector_matches_dom_pseudo_in_scope(
                        selector,
                        doc,
                        node_idx,
                        pseudo,
                        scope_match.root,
                    )
                })
                .map(crate::style::matching::dom::selector_specificity)
                .max();
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

        let inherited = ParentStyle::from_indices(self.styles, inherited_from);
        let mut style = WorkingStyle {
            font: inherited.font.clone(),
            text: inherited.text.clone(),
            box_model: initial_box_model(),
            border: initial_border(),
            radii: BorderRadii::default(),
            background: Background::default(),
            layout: LayoutStyle::default(),
            line_height_spec: None,
            generated_content: None,
            counters: CounterDirectives::default(),
        };
        inherit_box_model_properties(&mut style.box_model, &inherited.box_model);
        // Generated pseudo-elements use the CSS initial value of `display`,
        // which is `inline`.  Ordinary DOM elements commonly become blocks
        // through the UA sheet, but a pseudo has no element-name UA rule to
        // override this initial value.
        style.box_model.display = Display::Inline;
        let parent_font_size = style.font.font_size;
        let mut cascade_scratch = std::mem::take(self.cascade_scratch);
        let hints_sequence = specified::build_cascade_events(
            prepared,
            &matched_rules,
            &important_rules,
            None,
            &mut cascade_scratch.events,
        );
        *self.matched_rule_scratch = matched_rules;
        *self.important_rule_scratch = important_rules;
        let custom_properties = cascade_custom_properties(
            &cascade_scratch.events.events,
            None,
            inherited_custom_properties,
        );
        let custom_properties =
            effective_custom_properties(&custom_properties, inherited_custom_properties);

        self.apply_cascade(
            &mut style,
            CascadeInputs {
                events: &cascade_scratch.events.events,
                inline_style: None,
                hints_sequence,
                parent_font_size,
                custom_properties,
                parent: &inherited,
                presentational_hints_node: None,
            },
            &mut cascade_scratch.valid_events,
            &mut cascade_scratch.invalid_sequences,
            &mut cascade_scratch.selection,
        );
        *self.cascade_scratch = cascade_scratch;
        Some(style)
    }
}

/// Compute style for a single DOM element (used in new pipeline)
impl<'a, 'sheet, 'css> StyleResolverContext<'a, 'sheet, 'css> {
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

        // Get candidate rule indices
        index.collect_candidates(
            tag,
            id,
            doc.get_dom_classes(node_idx),
            self.candidate_scratch,
            self.candidate_seen,
        );

        // Check each candidate. A rule cascades with the most specific of its
        // matching selectors, not the first one that happens to match.
        for id in self.candidate_scratch.iter().copied() {
            let style_rule = prepared.get(id).style_rule();
            let Some(scope_match) = prepared.scope_match(prepared.get(id).scope(), doc, node_idx)
            else {
                continue;
            };
            let specificity = style_rule
                .selectors
                .0
                .iter()
                .zip(index.ancestor_requirements(id))
                .filter(|(selector, requirements)| {
                    selector_might_match_ancestors(**requirements, ancestor_filter)
                        && crate::style::matching::dom::selector_matches_dom_node_in_scope(
                            selector,
                            doc,
                            node_idx,
                            scope_match.root,
                        )
                })
                .map(|(selector, _)| crate::style::matching::dom::selector_specificity(selector))
                .max();
            if let Some(specificity) = specificity {
                matched_rules.push(MatchedRule {
                    specificity,
                    id,
                    scope_proximity: scope_match.proximity,
                });
            }
        }

        self.timings.selector_matching += matching_started.elapsed();

        // Build computed style from matched rules (same as box-based version)
        let cascade_started = Instant::now();
        let parent_style_indices = doc.get_dom_parent(node_idx).and_then(|parent| self.styles.style_for_node(parent));
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
        let mut style = get_inherited_style_dom(doc, self.styles, node_idx);
        let parent_font_size = style.font.font_size;
        let mut cascade_scratch = std::mem::take(self.cascade_scratch);
        let hints_sequence = specified::build_cascade_events(
            prepared,
            &matched_rules,
            &important_rules,
            inline_style,
            &mut cascade_scratch.events,
        );
        *self.matched_rule_scratch = matched_rules;
        *self.important_rule_scratch = important_rules;
        let parent_custom = &custom_maps[parent_custom_id as usize];
        let (mut custom_properties, reused_custom_id, cache_signature) =
            match custom_cascade_cache.probe(
                &cascade_scratch.events.events,
                inline_style,
                parent_custom_id,
            ) {
                CustomCascadeProbe::NoDeclarations => (None, None, None),
                CustomCascadeProbe::UncachedDeclarations => (
                    cascade_custom_properties(
                        &cascade_scratch.events.events,
                        inline_style,
                        parent_custom,
                    ),
                    None,
                    None,
                ),
                CustomCascadeProbe::Hit(map_id) => (None, Some(map_id), None),
                CustomCascadeProbe::Miss(signature) => (
                    cascade_custom_properties(
                        &cascade_scratch.events.events,
                        inline_style,
                        parent_custom,
                    ),
                    None,
                    Some(signature),
                ),
            };
        let custom_base = reused_custom_id
            .map(|map_id| &custom_maps[map_id as usize])
            .unwrap_or(parent_custom);

        let parent_style = ParentStyle::for_node(doc, self.styles, node_idx);
        self.apply_cascade(
            &mut style,
            CascadeInputs {
                events: &cascade_scratch.events.events,
                inline_style,
                hints_sequence,
                parent_font_size,
                custom_properties: effective_custom_properties(
                    &custom_properties,
                    custom_base,
                ),
                parent: &parent_style,
                presentational_hints_node: Some(node_idx),
            },
            &mut cascade_scratch.valid_events,
            &mut cascade_scratch.invalid_sequences,
            &mut cascade_scratch.selection,
        );
        *self.cascade_scratch = cascade_scratch;

        if let Some(white_space) = effective_custom_properties(&custom_properties, custom_base)
            .get(WHITE_SPACE_CASCADE_MARKER)
            .and_then(from_marker_tokens)
        {
            style.text.white_space = white_space;
        }
        for (marker, target) in [
            (LETTER_SPACING_MARKER, &mut style.text.letter_spacing),
            (WORD_SPACING_MARKER, &mut style.text.word_spacing),
        ] {
            let Some(spacing) = effective_custom_properties(&custom_properties, custom_base)
                .get(marker)
                .and_then(token_list_to_css_string)
                .as_deref()
                .and_then(parse_text_spacing)
                .as_ref()
                .and_then(|parsed| {
                    parsed_text_spacing_to_computed(
                        parsed,
                        style.font.font_size,
                        doc.root_font_size(),
                    )
                })
            else {
                continue;
            };
            *target = spacing;
            if let Some(tokens) = canonical_text_spacing_tokens(spacing) {
                update_custom_property(&mut custom_properties, custom_base, marker, tokens);
            }
        }
        if let Some(tab_size) = effective_custom_properties(&custom_properties, custom_base)
            .get(TAB_SIZE_CASCADE_MARKER)
            .and_then(token_list_to_css_string)
            .as_deref()
            .and_then(parse_tab_size)
            .as_ref()
            .and_then(|parsed| {
                parsed_tab_size_to_computed(parsed, style.font.font_size, doc.root_font_size())
            })
        {
            style.text.tab_size = tab_size;
            if let Some(tokens) = canonical_tab_size_tokens(tab_size) {
                update_custom_property(
                    &mut custom_properties,
                    custom_base,
                    TAB_SIZE_CASCADE_MARKER,
                    tokens,
                );
            }
        }

        // HTML `lang` and XML `xml:lang` participate in inherited text
        // semantics even though they are not CSS properties.
        if let Some(language) = doc
            .element_ref(node_idx)
            .and_then(|element| element.attr_in_any_namespace("lang"))
            .map(str::trim)
            .filter(|language| !language.is_empty())
        {
            style.text.language = Some(self.styles.intern_string(language));
        }

        // Preserve `currentColor` as a computed keyword for inheritance while
        // caching this element's resolved RGBA for paint.
        let current_color = style.text.color;
        if style.border.current_color_sides & 0b0001 != 0 {
            style.border.border_top_color = current_color;
        }
        if style.border.current_color_sides & 0b0010 != 0 {
            style.border.border_right_color = current_color;
        }
        if style.border.current_color_sides & 0b0100 != 0 {
            style.border.border_bottom_color = current_color;
        }
        if style.border.current_color_sides & 0b1000 != 0 {
            style.border.border_left_color = current_color;
        }

        self.timings.cascade += cascade_started.elapsed();
        let custom_map = match custom_properties {
            Some(values) => {
                let cache_signature = cache_signature
                    .filter(|_| custom_map_is_cacheable(&values));
                CustomMapResult::New { values, cache_signature }
            }
            None => reused_custom_id
                .map(CustomMapResult::Reused)
                .unwrap_or(CustomMapResult::Inherited),
        };
        ElementStyleResult::Computed { style, custom_map, sharing_signature }
    }
}

include!("tests.rs");
