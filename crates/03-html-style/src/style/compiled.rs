//! Document-independent compiled style data shared by compilation and resolution.
//! This module contains records, ordering, and accessors; it never parses CSS,
//! matches DOM nodes, manages caches, or invokes the coordinator.
use crate::style::environment::{MediaEnvironment, MediaQuerySet};
use crate::style::property_catalogue::PropertyKey;
use crate::style::rules::prepared::{PreparedDeclarationBlock, PreparedStylesheetFragment};
use html_style_model::FontFace;
use lightningcss::rules::style::StyleRule;
use lightningcss::selector::{PseudoClass, SelectorList};
use lightningcss::values::ident::Ident;
use lightningcss::values::string::CSSString;
use parcel_selectors::attr::{AttrSelectorOperator, ParsedCaseSensitivity};
use rustc_data_structures::fx::FxHashMap;
use std::cmp::Reverse;
use std::sync::Arc;

pub(crate) struct CompiledStyles {
    pub(crate) rules: PreparedRuleSet<'static>,
    pub(crate) selectors: SelectorIndex,
    pub(crate) input_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub(crate) enum CascadeOrigin {
    UserAgent = 0,
    PresentationalHint = 1,
    Author = 2,
}

/// Integer layer order. Unlayered rules currently share the sentinel rank;
/// layer expansion can assign concrete ranks without changing selector or
/// resolver APIs.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct LayerOrder(pub(super) u32);

impl LayerOrder {
    pub(super) const UNLAYERED: Self = Self(u32::MAX);

    pub(super) fn is_unlayered(self) -> bool {
        self == Self::UNLAYERED
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RulePriority {
    pub(super) origin: CascadeOrigin,
    pub(super) layer: LayerOrder,
    pub(super) source_order: u32,
}

impl RulePriority {
    #[cfg(test)]
    pub(crate) fn source_order(self) -> u32 {
        self.source_order
    }

    pub(crate) fn origin(self) -> CascadeOrigin {
        self.origin
    }

    #[cfg(test)]
    pub(crate) fn layer(self) -> LayerOrder {
        self.layer
    }

    pub(crate) fn same_origin_and_layer(self, other: Self) -> bool {
        self.origin == other.origin && self.layer == other.layer
    }

    pub(crate) fn order_key(
        self,
        specificity: u32,
        scope_proximity: u32,
        important: bool,
    ) -> (CascadeOrigin, u32, u32, Reverse<u32>, u32) {
        let layer = if important {
            u32::MAX - self.layer.0
        } else {
            self.layer.0
        };
        (
            self.origin,
            layer,
            specificity,
            Reverse(scope_proximity),
            self.source_order,
        )
    }
}

/// Stable within a compiled program, independent of parser allocation addresses.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) enum DeclarationId {
    Rule { fragment: usize, declaration: usize },
    Inline { block: usize, declaration: usize },
}

/// Dense, compact identifier issued only by `PreparedRuleSet`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct EffectiveRuleId(pub(super) u32);

impl EffectiveRuleId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScopeId(pub(super) u32);

impl ScopeId {
    pub(super) const NONE: Self = Self(u32::MAX);
}

pub(crate) struct ScopeDescriptor<'css> {
    pub(super) parent: ScopeId,
    pub(super) start: Option<SelectorList<'css>>,
    pub(super) end: Option<SelectorList<'css>>,
    pub(super) implicit_root_index: Option<u32>,
}

#[derive(Clone, Copy)]
pub(crate) struct EffectiveRule<'sheet, 'css> {
    pub(super) style_rule: &'sheet StyleRule<'css>,
    pub(super) priority: RulePriority,
    pub(super) scope: ScopeId,
}

#[derive(Clone, Copy)]
pub(crate) struct PreparedRuleMetadata {
    pub(super) priority: RulePriority,
    pub(super) scope: ScopeId,
    pub(super) has_important: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct PreparedTargetRange {
    pub(super) start: u32,
    pub(super) len_and_flags: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct PreparedRuleDeclarations {
    pub(super) valid_selectors: bool,
    pub(super) normal_start: u32,
    pub(super) important_start: u32,
    pub(super) normal_shadow_mask: u64,
    pub(super) important_shadow_mask: u64,
}

pub(super) const ALL_DECLARATIONS_SHADOWABLE: u64 = 1 << 63;

impl PreparedTargetRange {
    pub(super) const REQUIRES_VIEWPORT_HEIGHT: u32 = 1 << 31;
    pub(super) const RENDERER_ELIGIBLE: u32 = 1 << 30;
    pub(super) const FLAGS: u32 = Self::REQUIRES_VIEWPORT_HEIGHT | Self::RENDERER_ELIGIBLE;

    pub(super) fn new(
        start: u32,
        len: u32,
        requires_viewport_height: bool,
        renderer_eligibility: bool,
    ) -> Self {
        assert!(
            len < Self::RENDERER_ELIGIBLE,
            "one declaration's property target count fits in 30 bits"
        );
        Self {
            start,
            len_and_flags: len
                | if requires_viewport_height {
                    Self::REQUIRES_VIEWPORT_HEIGHT
                } else {
                    0
                }
                | if renderer_eligibility {
                    Self::RENDERER_ELIGIBLE
                } else {
                    0
                },
        }
    }

    pub(super) fn len(self) -> usize {
        (self.len_and_flags & !Self::FLAGS) as usize
    }

    pub(super) fn requires_viewport_height(self) -> bool {
        self.len_and_flags & Self::REQUIRES_VIEWPORT_HEIGHT != 0
    }

    pub(super) fn renderer_eligibility(self) -> bool {
        self.len_and_flags & Self::RENDERER_ELIGIBLE != 0
    }
}

impl<'sheet, 'css> EffectiveRule<'sheet, 'css> {
    pub(crate) fn style_rule(self) -> &'sheet StyleRule<'css> {
        self.style_rule
    }

    pub(crate) fn priority(self) -> RulePriority {
        self.priority
    }

    pub(crate) fn scope(self) -> ScopeId {
        self.scope
    }
}

/// Effective rule order references immutable fragment-owned declaration tables.
pub(crate) struct PreparedRuleSet<'css> {
    pub(super) fragments: Vec<Arc<PreparedStylesheetFragment<'css>>>,
    pub(super) rule_sources: Vec<(usize, usize)>,
    pub(super) rules: Vec<PreparedRuleMetadata>,
    pub(super) scopes: Vec<ScopeDescriptor<'css>>,
    pub(super) media_queries: MediaQuerySet,
    pub(super) font_faces: Arc<[FontFace]>,
    pub(super) environment: MediaEnvironment,
}

impl<'css> PreparedRuleSet<'css> {
    fn rule_source(&self, id: EffectiveRuleId) -> (&PreparedStylesheetFragment<'css>, usize) {
        let (fragment, local_rule) = self.rule_sources[id.index()];
        (&self.fragments[fragment], local_rule)
    }

    pub(crate) fn declaration_id(
        &self,
        id: EffectiveRuleId,
        important: bool,
        index: usize,
    ) -> DeclarationId {
        let (fragment, local_rule) = self.rule_sources[id.index()];
        let starts = self.fragments[fragment].rule_declarations[local_rule];
        DeclarationId::Rule {
            fragment,
            declaration: index
                + if important {
                    starts.important_start
                } else {
                    starts.normal_start
                } as usize,
        }
    }

    pub(crate) fn declaration_block(
        &self,
        id: EffectiveRuleId,
        important: bool,
        index: usize,
    ) -> (&PreparedDeclarationBlock<'css>, usize) {
        let (fragment, local_rule) = self.rule_source(id);
        let starts = fragment.rule_declarations[local_rule];
        let start = if important {
            starts.important_start
        } else {
            starts.normal_start
        };
        (&fragment.declarations, start as usize + index)
    }

    pub(crate) fn font_faces(&self) -> &Arc<[FontFace]> {
        &self.font_faces
    }
    pub(crate) fn media_queries(&self) -> &MediaQuerySet {
        &self.media_queries
    }
    pub(crate) fn environment(&self) -> MediaEnvironment {
        self.environment
    }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rules.len()
    }

    pub(crate) fn iter(
        &self,
    ) -> impl Iterator<Item = (EffectiveRuleId, EffectiveRule<'_, 'css>)> + '_ {
        self.rules.iter().enumerate().map(|(index, _)| {
            let id = EffectiveRuleId(u32::try_from(index).expect("prepared rule IDs fit in u32"));
            (id, self.get(id))
        })
    }

    #[inline]
    pub(crate) fn priority(&self, id: EffectiveRuleId) -> RulePriority {
        self.rules[id.index()].priority
    }

    #[inline]
    pub(crate) fn scope(&self, id: EffectiveRuleId) -> ScopeId {
        self.rules[id.index()].scope
    }

    #[inline]
    pub(crate) fn has_important(&self, id: EffectiveRuleId) -> bool {
        self.rules[id.index()].has_important
    }

    pub(crate) fn get(&self, id: EffectiveRuleId) -> EffectiveRule<'_, 'css> {
        let metadata = self.rules[id.index()];
        let (fragment, local_rule) = self.rule_source(id);
        EffectiveRule {
            style_rule: &fragment.style_rules[local_rule],
            priority: metadata.priority,
            scope: metadata.scope,
        }
    }

    pub(crate) fn declaration_targets(
        &self,
        id: EffectiveRuleId,
        important: bool,
        index: usize,
    ) -> &[PropertyKey] {
        self.declaration_metadata(id, important, index).0
    }

    pub(crate) fn declaration_metadata(
        &self,
        id: EffectiveRuleId,
        important: bool,
        index: usize,
    ) -> (&[PropertyKey], bool) {
        let (block, index) = self.declaration_block(id, important, index);
        block.metadata(index)
    }

    pub(crate) fn rule_shadow_declarations(
        &self,
        id: EffectiveRuleId,
        important: bool,
    ) -> (u64, bool) {
        let (fragment, local_rule) = self.rule_source(id);
        let starts = fragment.rule_declarations[local_rule];
        let mask = if important {
            starts.important_shadow_mask
        } else {
            starts.normal_shadow_mask
        };
        (
            mask & !ALL_DECLARATIONS_SHADOWABLE,
            mask & ALL_DECLARATIONS_SHADOWABLE != 0,
        )
    }

    pub(crate) fn scope_descriptor(&self, id: ScopeId) -> &ScopeDescriptor<'css> {
        &self.scopes[id.0 as usize]
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PreparedSelector {
    pub(crate) ancestor_requirements: (u32, u32),
    /// Attribute range for safe sibling sharing; None marks context-sensitive selectors.
    pub(super) sibling_attributes: Option<(u32, u32)>,
    pub(super) specificity: u32,
    pub(super) fast_start: u32,
    pub(super) fast_len: u16,
    pub(super) pseudo_mask: u8,
    /// Attribute, language and state predicates are not fixed by a cached
    /// tag/ID/class signature, even when the selector is a single compound.
    pub(super) has_node_conditions: bool,
    /// Index-proven predicates, or u16::MAX when exact checking is required.
    pub(crate) bucket_component: u16,
    pub(super) bucket_test: u16,
}

impl PreparedSelector {
    #[inline]
    pub(crate) fn specificity(self) -> u32 {
        self.specificity
    }

    #[inline]
    pub(crate) fn is_context_free(self) -> bool {
        self.fast_len == 1 && self.pseudo_mask == 0 && !self.has_node_conditions
    }

    #[inline]
    pub(crate) fn targets_pseudo(self, mask: u8) -> bool {
        self.pseudo_mask & mask != 0
    }

    #[inline]
    pub(crate) fn has_pseudo_target(self) -> bool {
        self.pseudo_mask != 0
    }
}

#[derive(Clone)]
pub(crate) enum FastTest {
    LocalName {
        name: Ident<'static>,
        lower_name: Ident<'static>,
    },
    Id(Ident<'static>),
    Class(Ident<'static>),
    NotLocalName {
        name: Ident<'static>,
        lower_name: Ident<'static>,
    },
    NotId(Ident<'static>),
    NotClass(Ident<'static>),
    Any(Box<[FastAlternative]>),
    Attribute(Box<FastAttribute>),
    NotAttribute(Box<FastAttribute>),
    PseudoClass(Box<PseudoClass<'static>>),
    NotPseudoClass(Box<PseudoClass<'static>>),
}

/// Keep uncommon attribute payloads out of every compact test's inline size.
#[derive(Clone)]
pub(crate) enum FastAttribute {
    Exists {
        name: Ident<'static>,
        lower_name: Ident<'static>,
    },
    Value {
        operator: AttrSelectorOperator,
        name: Ident<'static>,
        value: CSSString<'static>,
        case_sensitivity: ParsedCaseSensitivity,
    },
}

/// Logical alternatives are atomic, keeping the common matcher nonrecursive.
#[derive(Clone)]
pub(crate) enum FastAlternative {
    LocalName {
        name: Ident<'static>,
        lower_name: Ident<'static>,
    },
    Id(Ident<'static>),
    Class(Ident<'static>),
}

#[derive(Clone, Copy)]
pub(crate) enum Relation {
    End,
    Parent,
    Ancestor,
    PreviousSibling,
}

#[derive(Clone, Copy)]
pub(crate) struct Compound {
    pub(super) test_start: u32,
    pub(super) test_len: u16,
    pub(super) relation: Relation,
}

pub(crate) struct RuleSelectorRange {
    pub(super) start: u32,
    pub(super) max_specificity: u32,
}

pub(crate) struct CompiledSelectors {
    pub(super) rule_starts: Vec<RuleSelectorRange>,
    pub(super) selectors: Vec<PreparedSelector>,
    pub(super) sibling_attributes: Vec<String>,
    pub(super) ancestor_tests: Vec<FastAlternative>,
    pub(super) tests: Vec<FastTest>,
    pub(super) compounds: Vec<Compound>,
}

/// One selector within a prepared rule. Candidate lists are ordered by rule
/// and selector so the resolver can cascade each rule once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SelectorCandidate {
    pub(crate) rule: EffectiveRuleId,
    pub(crate) selector: u32,
}

/// A small set of cheap normalized states useful as candidate keys.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) enum SelectorState {
    Link,
    Enabled,
    Disabled,
    Checked,
}

/// Mutable preparation buckets, compacted before matching begins.
#[derive(Default)]
pub(crate) struct SelectorBuckets {
    pub(super) by_tag: FxHashMap<String, Vec<SelectorCandidate>>,
    pub(super) by_class: FxHashMap<String, Vec<SelectorCandidate>>,
    pub(super) by_id: FxHashMap<String, Vec<SelectorCandidate>>,
    pub(super) by_attribute: FxHashMap<String, Vec<SelectorCandidate>>,
    pub(super) by_namespace: FxHashMap<String, Vec<SelectorCandidate>>,
    pub(super) by_state: FxHashMap<SelectorState, Vec<SelectorCandidate>>,
    pub(super) root: Vec<SelectorCandidate>,
    pub(super) universal: Vec<SelectorCandidate>,
}

#[derive(Clone, Copy)]
pub(crate) struct CandidateRange {
    pub(super) start: u32,
    pub(super) len: u32,
}

/// Bucket keys address one contiguous candidate pool.
pub struct SelectorIndex {
    pub(super) by_tag: FxHashMap<String, CandidateRange>,
    pub(super) by_class: FxHashMap<String, CandidateRange>,
    pub(super) by_id: FxHashMap<String, CandidateRange>,
    pub(super) by_attribute: FxHashMap<String, CandidateRange>,
    pub(super) by_namespace: FxHashMap<String, CandidateRange>,
    pub(super) by_state: Vec<(SelectorState, CandidateRange)>,
    pub(super) root: CandidateRange,
    pub(super) universal: CandidateRange,
    pub(super) candidates: Vec<SelectorCandidate>,
    pub(super) compiled: CompiledSelectors,
}

impl SelectorIndex {
    pub(super) fn bucket(&self, range: CandidateRange) -> &[SelectorCandidate] {
        &self.candidates[range.start as usize..range.start as usize + range.len as usize]
    }
}
