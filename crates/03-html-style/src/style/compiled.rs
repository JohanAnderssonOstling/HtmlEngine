//! Document-independent compiled style data shared by compilation and resolution.
//! This module contains records, ordering, and accessors; it never parses CSS,
//! matches DOM nodes, manages caches, or invokes the coordinator.
use crate::style::environment::{MediaEnvironment, MediaQuerySet};
use crate::style::property_catalogue::PropertyKey;
use crate::style::source::fonts::FontFace;
use lightningcss::rules::style::StyleRule;
use lightningcss::selector::SelectorList;
use lightningcss::values::ident::Ident;
use rustc_data_structures::fx::FxHashMap;
use std::cmp::Ordering;
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

    pub(crate) fn compare_normal(
        self,
        specificity: u32,
        scope_proximity: u32,
        other: Self,
        other_specificity: u32,
        other_scope_proximity: u32,
    ) -> Ordering {
        self.origin
            .cmp(&other.origin)
            .then_with(|| self.layer.cmp(&other.layer))
            .then_with(|| specificity.cmp(&other_specificity))
            .then_with(|| other_scope_proximity.cmp(&scope_proximity))
            .then_with(|| self.source_order.cmp(&other.source_order))
    }

    pub(crate) fn compare_important(
        self,
        specificity: u32,
        scope_proximity: u32,
        other: Self,
        other_specificity: u32,
        other_scope_proximity: u32,
    ) -> Ordering {
        fn layer_rank(layer: LayerOrder) -> (u8, u32) {
            if layer.is_unlayered() {
                (0, 0)
            } else {
                (1, u32::MAX - layer.0)
            }
        }
        self.origin
            .cmp(&other.origin)
            .then_with(|| layer_rank(self.layer).cmp(&layer_rank(other.layer)))
            .then_with(|| specificity.cmp(&other_specificity))
            .then_with(|| other_scope_proximity.cmp(&scope_proximity))
            .then_with(|| self.source_order.cmp(&other.source_order))
    }
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
}

#[derive(Clone, Copy)]
pub(crate) struct PreparedTargetRange {
    pub(super) start: u32,
    pub(super) len_and_flags: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct PreparedRuleDeclarations {
    pub(super) normal_start: u32,
    pub(super) important_start: u32,
    pub(super) normal_shadow_mask: u64,
    pub(super) important_shadow_mask: u64,
}

pub(super) const ALL_DECLARATIONS_SHADOWABLE: u64 = 1 << 63;

impl PreparedTargetRange {
    pub(super) const ALWAYS_COMPUTABLE: u32 = 1 << 31;
    pub(super) const RENDERER_ELIGIBLE: u32 = 1 << 30;
    pub(super) const ELIGIBILITY_KNOWN: u32 = 1 << 29;
    pub(super) const FLAGS: u32 =
        Self::ALWAYS_COMPUTABLE | Self::RENDERER_ELIGIBLE | Self::ELIGIBILITY_KNOWN;

    pub(super) fn new(
        start: u32,
        len: u32,
        always_computable: bool,
        renderer_eligibility: Option<bool>,
    ) -> Self {
        assert!(
            len < Self::ELIGIBILITY_KNOWN,
            "one declaration's property target count fits in 29 bits"
        );
        Self {
            start,
            len_and_flags: len
                | if always_computable {
                    Self::ALWAYS_COMPUTABLE
                } else {
                    0
                }
                | if renderer_eligibility == Some(true) {
                    Self::RENDERER_ELIGIBLE
                } else {
                    0
                }
                | if renderer_eligibility.is_some() {
                    Self::ELIGIBILITY_KNOWN
                } else {
                    0
                },
        }
    }

    pub(super) fn len(self) -> usize {
        (self.len_and_flags & !Self::FLAGS) as usize
    }

    pub(super) fn is_always_computable(self) -> bool {
        self.len_and_flags & Self::ALWAYS_COMPUTABLE != 0
    }

    pub(super) fn renderer_eligibility(self) -> Option<bool> {
        (self.len_and_flags & Self::ELIGIBILITY_KNOWN != 0)
            .then_some(self.len_and_flags & Self::RENDERER_ELIGIBLE != 0)
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

pub(crate) struct PreparedRuleSet<'css> {
    pub(super) style_rules: Vec<Arc<StyleRule<'css>>>,
    pub(super) rules: Vec<PreparedRuleMetadata>,
    pub(super) rule_pseudo_masks: Vec<u8>,
    pub(super) scopes: Vec<ScopeDescriptor<'css>>,
    pub(super) rule_target_starts: Vec<PreparedRuleDeclarations>,
    pub(super) declaration_target_ranges: Vec<PreparedTargetRange>,
    pub(super) property_targets: Vec<PropertyKey>,
    pub(super) media_queries: MediaQuerySet,
    pub(super) font_faces: Arc<[FontFace]>,
    pub(super) environment: MediaEnvironment,
}

impl<'css> PreparedRuleSet<'css> {
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
        self.rules
            .iter()
            .copied()
            .enumerate()
            .map(|(index, metadata)| {
                let index = u32::try_from(index).expect("prepared rule IDs fit in u32");
                (
                    EffectiveRuleId(index),
                    EffectiveRule {
                        style_rule: &self.style_rules[index as usize],
                        priority: metadata.priority,
                        scope: metadata.scope,
                    },
                )
            })
    }

    pub(crate) fn get(&self, id: EffectiveRuleId) -> EffectiveRule<'_, 'css> {
        let metadata = self.rules[id.0 as usize];
        EffectiveRule {
            style_rule: &self.style_rules[id.0 as usize],
            priority: metadata.priority,
            scope: metadata.scope,
        }
    }

    pub(crate) fn pseudo_mask(&self, id: EffectiveRuleId) -> u8 {
        self.rule_pseudo_masks[id.0 as usize]
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
    ) -> (&[PropertyKey], bool, Option<bool>) {
        let starts = self.rule_target_starts[id.0 as usize];
        let ranges_start = if important {
            starts.important_start
        } else {
            starts.normal_start
        } as usize;
        let range = self.declaration_target_ranges[ranges_start + index];
        (
            &self.property_targets[range.start as usize..range.start as usize + range.len()],
            range.is_always_computable(),
            range.renderer_eligibility(),
        )
    }

    #[cfg(test)]
    pub(crate) fn declaration_is_always_computable(
        &self,
        id: EffectiveRuleId,
        important: bool,
        index: usize,
    ) -> bool {
        let starts = self.rule_target_starts[id.0 as usize];
        let ranges_start = if important {
            starts.important_start
        } else {
            starts.normal_start
        } as usize;
        self.declaration_target_ranges[ranges_start + index].is_always_computable()
    }

    pub(crate) fn rule_shadow_declarations(
        &self,
        id: EffectiveRuleId,
        important: bool,
    ) -> (u64, bool) {
        let starts = self.rule_target_starts[id.0 as usize];
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
    pub(crate) ancestor_requirements: u64,
    pub(super) specificity: u32,
    pub(super) fast_start: u32,
    pub(super) fast_len: u16,
}

impl PreparedSelector {
    #[inline]
    pub(crate) fn specificity(self) -> u32 {
        self.specificity
    }

    #[inline]
    pub(crate) fn is_context_free(self) -> bool {
        self.fast_len == 1
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
}

#[derive(Clone, Copy)]
pub(crate) enum Relation {
    End,
    Parent,
    Ancestor,
}

#[derive(Clone, Copy)]
pub(crate) struct Compound {
    pub(super) test_start: u32,
    pub(super) test_len: u16,
    pub(super) relation: Relation,
}

pub(crate) struct CompiledSelectors {
    pub(super) rule_starts: Vec<u32>,
    pub(super) selectors: Vec<PreparedSelector>,
    pub(super) tests: Vec<FastTest>,
    pub(super) compounds: Vec<Compound>,
}

/// Index of rule keys, keyed by selector target (tag/class/id)
pub struct SelectorIndex {
    pub(super) by_tag: FxHashMap<String, Vec<EffectiveRuleId>>,
    pub(super) by_class: FxHashMap<String, Vec<EffectiveRuleId>>,
    pub(super) by_id: FxHashMap<String, Vec<EffectiveRuleId>>,
    pub(super) universal: Vec<EffectiveRuleId>, // *, [attr], :pseudo with no tag/class/id
    pub(super) compiled: CompiledSelectors,
}
