//! Selection of cascaded specified values before computed-value conversion.
//!
//! Lightning CSS exposes shorthand expansion, so winners are tracked by
//! longhand rather than by declaration spelling. This is also where
//! `revert` and `revert-layer` discard candidates from their cascade scope.

use super::context::ResolvedDeclaration;
use super::custom_properties::CustomPropertyScratch;
use super::plan::MatchedRule;
use super::runner::SubstitutionCache;
use crate::style::cascade::resolver::prepared_native::PreparedNativeValue;
use crate::style::compiled::{
    CascadeOrigin, DeclarationId, EffectiveRuleId, PreparedRuleSet, RulePriority,
};
use crate::style::property_catalogue::PropertyKey;
use crate::style::rules::prepared::PreparedDeclarationBlock;
use crate::style::source::inline::PreparedInlineStyle;
use crate::style::syntax::capabilities::declaration_is_custom_property;
use crate::style::syntax::prepared_renderer::PreparedRendererValue;
use crate::style::syntax::wide_keywords::CssWideKeyword;
use crate::style::syntax::wide_keywords::property_css_wide_keyword;
use lightningcss::properties::Property;
use lightningcss::properties::custom::TokenOrValue;
use rustc_data_structures::fx::FxHashMap;
use std::hash::Hash;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) enum CascadeBoundary {
    Rule {
        priority: RulePriority,
        important: bool,
        normal_rank: usize,
        layer_start: usize,
    },
    Inline {
        important: bool,
        normal_rank: usize,
    },
    Hints,
}

impl CascadeBoundary {
    pub(super) fn origin(self) -> CascadeOrigin {
        match self {
            Self::Rule { priority, .. } => priority.origin(),
            Self::Inline { .. } => CascadeOrigin::Author,
            Self::Hints => CascadeOrigin::PresentationalHint,
        }
    }

    pub(super) fn same_layer(self, other: Self) -> bool {
        match (self, other) {
            (
                Self::Rule { priority: left, .. },
                Self::Rule {
                    priority: right, ..
                },
            ) => left.same_origin_and_layer(right),
            (Self::Inline { .. }, Self::Inline { .. }) | (Self::Hints, Self::Hints) => true,
            _ => false,
        }
    }

    fn important(self) -> bool {
        match self {
            Self::Rule { important, .. } | Self::Inline { important, .. } => important,
            Self::Hints => false,
        }
    }

    fn normal_rank(self) -> usize {
        match self {
            Self::Rule { normal_rank, .. } | Self::Inline { normal_rank, .. } => normal_rank,
            Self::Hints => 0,
        }
    }

    fn layer_start(self) -> usize {
        match self {
            Self::Rule { layer_start, .. } => layer_start,
            Self::Inline { normal_rank, .. } => normal_rank,
            Self::Hints => 0,
        }
    }

    pub(super) fn rollback_excludes(self, candidate: Self, layer: bool) -> bool {
        if !layer {
            // Author-origin revert also discards HTML hints; layer rollback
            // keeps them available below every author layer.
            return candidate.origin() == self.origin()
                || (self.origin() == CascadeOrigin::Author
                    && candidate.origin() == CascadeOrigin::PresentationalHint);
        }
        if matches!(
            self,
            Self::Inline {
                important: true,
                ..
            }
        ) {
            // Important element-attached styles are their own outer layer.
            // Their rollback does not cross into author stylesheet rules.
            return matches!(candidate, Self::Inline { .. });
        }
        if self.important() {
            candidate.origin() == self.origin()
                && (candidate.important() || candidate.normal_rank() >= self.layer_start())
        } else {
            candidate.same_layer(self)
        }
    }
}

/// Element-attached declarations share the same event and winner-selection path.
#[derive(Clone, Copy, Default)]
pub(super) struct ElementDeclarations<'a, 'css> {
    pub(super) prepared: Option<&'a PreparedRuleSet<'css>>,
    pub(super) inline: Option<&'a PreparedInlineStyle<'css>>,
    pub(super) hints: &'a [Property<'css>],
    pub(super) hint_preparation: Option<&'a PreparedDeclarationBlock<'css>>,
}

/// Every declaration is a block/index handle. The boundary carries cascade
/// semantics; element sources use the same prepared view as stylesheet blocks.
#[derive(Clone, Copy)]
pub(super) struct DeclarationEvent {
    block: u32,
    index: u32,
    pub(super) boundary: CascadeBoundary,
}

struct DeclarationView<'a, 'css> {
    property: &'a Property<'css>,
    targets: &'a [PropertyKey],
    longhands: Option<&'a [Property<'css>]>,
    renderer: Option<&'a PreparedRendererValue>,
    native: &'a [Option<PreparedNativeValue>],
    eligible: bool,
}

impl DeclarationEvent {
    fn view<'a, 'css>(&self, element: ElementDeclarations<'a, 'css>) -> DeclarationView<'a, 'css> {
        let index = self.index as usize;
        let (property, targets, longhands, renderer, native, eligible) = match self.boundary {
            CascadeBoundary::Rule { important, .. } => {
                let prepared = element.prepared.expect("stylesheet declaration blocks");
                let id = EffectiveRuleId(self.block);
                let rule = prepared.get(id).style_rule();
                let property = if important {
                    &rule.declarations.important_declarations[index]
                } else {
                    &rule.declarations.declarations[index]
                };
                let (targets, eligible) = prepared.declaration_metadata(id, important, index);
                (
                    property,
                    targets,
                    prepared.declaration_longhands(id, important, index),
                    prepared.renderer_value(id, important, index),
                    prepared.native_values(id, important, index),
                    eligible,
                )
            }
            CascadeBoundary::Inline { important, .. } => {
                let inline = element.inline.expect("inline declaration block");
                let property = if important {
                    &inline.declarations.important_declarations[index]
                } else {
                    &inline.declarations.declarations[index]
                };
                let (targets, eligible) = inline.metadata(important, index);
                (
                    property,
                    targets,
                    inline.longhands(important, index),
                    inline.renderer_value(important, index),
                    inline.native_values(important, index),
                    eligible,
                )
            }
            CascadeBoundary::Hints => {
                let block = element.hint_preparation.expect("hint declaration block");
                let (targets, eligible) = block.metadata(index);
                (
                    &element.hints[index],
                    targets,
                    block.longhands(index),
                    block.renderer_value(index),
                    block.native_values(index),
                    eligible,
                )
            }
        };
        DeclarationView {
            property,
            targets,
            longhands,
            renderer,
            native,
            eligible,
        }
    }

    pub(super) fn rule_property<'a, 'css>(
        &self,
        element: ElementDeclarations<'a, 'css>,
    ) -> Option<(DeclarationId, &'a Property<'css>)> {
        let CascadeBoundary::Rule { important, .. } = self.boundary else {
            return None;
        };
        let id = element
            .prepared
            .expect("stylesheet declaration blocks")
            .declaration_id(EffectiveRuleId(self.block), important, self.index as usize);
        Some((id, self.property(element)))
    }

    pub(super) fn property<'a, 'css>(
        &self,
        element: ElementDeclarations<'a, 'css>,
    ) -> &'a Property<'css> {
        self.view(element).property
    }
    pub(super) fn native_values<'a, 'css>(
        &self,
        element: ElementDeclarations<'a, 'css>,
    ) -> &'a [Option<PreparedNativeValue>] {
        self.view(element).native
    }
    pub(super) fn renderer_value<'a, 'css>(
        &self,
        element: ElementDeclarations<'a, 'css>,
    ) -> Option<&'a PreparedRendererValue> {
        self.view(element).renderer
    }
    pub(super) fn prepared_longhands<'a, 'css>(
        &self,
        element: ElementDeclarations<'a, 'css>,
    ) -> Option<&'a [Property<'css>]> {
        self.view(element).longhands
    }
    pub(super) fn prepared_targets<'a, 'css>(
        &self,
        element: ElementDeclarations<'a, 'css>,
    ) -> &'a [PropertyKey] {
        self.view(element).targets
    }
    pub(super) fn requires_viewport_height(&self, element: ElementDeclarations<'_, '_>) -> bool {
        match self.boundary {
            CascadeBoundary::Rule { important, .. } => element
                .prepared
                .expect("prepared rules")
                .declaration_requires_viewport_height(
                    EffectiveRuleId(self.block),
                    important,
                    self.index as usize,
                ),
            CascadeBoundary::Inline { important, .. } => element
                .inline
                .expect("prepared inline")
                .requires_viewport_height(important, self.index as usize),
            CascadeBoundary::Hints => element
                .hint_preparation
                .expect("prepared hints")
                .requires_viewport_height(self.index as usize),
        }
    }

    pub(super) fn prepared_renderer_eligibility(
        &self,
        element: ElementDeclarations<'_, '_>,
    ) -> bool {
        self.view(element).eligible
    }
}

#[derive(Default)]
pub(super) struct EventScratch {
    pub(super) events: Vec<DeclarationEvent>,
    normal_ranks: FxHashMap<EffectiveRuleId, usize>,
    layer_starts: Vec<usize>,
    skipped_rules: Vec<bool>,
}

fn mark_fully_shadowed_rules(
    prepared: &PreparedRuleSet<'_>,
    rules: &[MatchedRule],
    important: bool,
    origin: Option<CascadeOrigin>,
    skipped_rules: &mut Vec<bool>,
) {
    skipped_rules.clear();
    skipped_rules.resize(rules.len(), false);
    let mut group_end = rules.len();
    while group_end != 0 {
        let priority = prepared.priority(rules[group_end - 1].id);
        let mut group_start = group_end - 1;
        while group_start != 0
            && prepared
                .priority(rules[group_start - 1].id)
                .same_origin_and_layer(priority)
        {
            group_start -= 1;
        }
        if origin.is_some_and(|origin| priority.origin() != origin) {
            group_end = group_start;
            continue;
        }
        // Sparse mixed-property groups cost more to inspect than they can
        // save. Only activate when safe whole-rule candidates are dense.
        let candidate_count = rules[group_start..group_end]
            .iter()
            .filter(|matched| prepared.rule_shadow_declarations(matched.id, important).1)
            .count();
        if candidate_count < 2 || candidate_count * 2 < group_end - group_start {
            group_end = group_start;
            continue;
        }
        let mut claimed = PropertySet::default();
        for rule_index in (group_start..group_end).rev() {
            let matched = &rules[rule_index];
            let (declaration_mask, all_shadowable) =
                prepared.rule_shadow_declarations(matched.id, important);
            let mut covered = all_shadowable;
            let mut declarations = declaration_mask;
            while covered && declarations != 0 {
                let index = declarations.trailing_zeros() as usize;
                declarations &= declarations - 1;
                covered = prepared
                    .declaration_targets(matched.id, important, index)
                    .iter()
                    .all(|&target| claimed.contains(target));
            }
            skipped_rules[rule_index] = covered;
            let mut declarations = declaration_mask;
            while declarations != 0 {
                let index = declarations.trailing_zeros() as usize;
                declarations &= declarations - 1;
                for &target in prepared.declaration_targets(matched.id, important, index) {
                    claimed.insert(target);
                }
            }
        }
        group_end = group_start;
    }
}

#[derive(Default)]
pub(super) struct CascadeScratch<'css> {
    pub(super) events: EventScratch,
    pub(super) hints: Vec<Property<'css>>,
    pub(super) hint_preparation: PreparedDeclarationBlock<'css>,
    pub(super) custom_properties: CustomPropertyScratch<'css>,
    pub(super) computation: CascadeComputation<'css>,
}

#[derive(Default)]
pub(super) struct CascadeComputation<'css> {
    pub(super) valid_events: Vec<DeclarationEvent>,
    pub(super) selection: SpecifiedSelection,
    // Retain each entry's native-value buffers between elements.
    pub(super) resolved: Vec<ResolvedDeclaration<'css>>,
    // Aligned with valid_events; usize::MAX denotes an unresolved declaration.
    pub(super) slots: Vec<usize>,
    pub(super) substitution_tokens: Vec<TokenOrValue<'css>>,
    pub(super) substitution_cache: SubstitutionCache<'css>,
}

pub(super) fn build_cascade_events<'prepared, 'css>(
    prepared: &'prepared PreparedRuleSet<'css>,
    normal_rules: &[MatchedRule],
    important_rules: &[MatchedRule],
    element: ElementDeclarations<'_, 'css>,
    scratch: &mut EventScratch,
) {
    let inline_capacity = element.inline.map_or(0, |inline| {
        inline.declarations.declarations.len() + inline.declarations.important_declarations.len()
    });
    let event_capacity = normal_rules
        .iter()
        .map(|matched| {
            prepared
                .get(matched.id)
                .style_rule()
                .declarations
                .declarations
                .len()
        })
        .sum::<usize>()
        + important_rules
            .iter()
            .map(|matched| {
                prepared
                    .get(matched.id)
                    .style_rule()
                    .declarations
                    .important_declarations
                    .len()
            })
            .sum::<usize>()
        + inline_capacity
        + element.hints.len();
    let events = &mut scratch.events;
    events.clear();
    events.reserve(event_capacity);
    let mut hint_events = element
        .hints
        .iter()
        .enumerate()
        .map(|(index, _)| DeclarationEvent {
            block: 0,
            index: u32::try_from(index).expect("HTML hint count fits in u32"),
            boundary: CascadeBoundary::Hints,
        });
    let normal_ranks = &mut scratch.normal_ranks;
    normal_ranks.clear();
    // Only important-order rollback needs to look up a rule's normal rank.
    if !important_rules.is_empty() {
        normal_ranks.extend(
            normal_rules
                .iter()
                .enumerate()
                .map(|(rank, matched)| (matched.id, rank)),
        );
    }
    let layer_starts = &mut scratch.layer_starts;
    layer_starts.clear();
    layer_starts.reserve(normal_rules.len());
    let mut layer_start = 0;
    for (rank, matched) in normal_rules.iter().enumerate() {
        if rank > 0
            && !prepared
                .priority(normal_rules[rank - 1].id)
                .same_origin_and_layer(prepared.priority(matched.id))
        {
            layer_start = rank;
        }
        layer_starts.push(layer_start);
    }
    // Inline declarations follow normal rules and important author rules.
    // HTML hints precede normal author rules; important UA rules come last.
    for (rules, important, origin) in [
        (normal_rules, false, None),
        (important_rules, true, Some(CascadeOrigin::Author)),
        (important_rules, true, Some(CascadeOrigin::UserAgent)),
    ] {
        mark_fully_shadowed_rules(
            prepared,
            rules,
            important,
            origin,
            &mut scratch.skipped_rules,
        );
        for (rule_index, matched) in rules.iter().enumerate() {
            if scratch.skipped_rules[rule_index] {
                continue;
            }
            let priority = prepared.priority(matched.id);
            if origin.is_some_and(|origin| priority.origin() != origin) {
                continue;
            }
            if !important && priority.origin() == CascadeOrigin::Author {
                events.extend(hint_events.by_ref());
            }
            let normal_rank = if important {
                normal_ranks
                    .get(&matched.id)
                    .copied()
                    .unwrap_or(normal_rules.len())
            } else {
                rule_index
            };
            let layer_start = layer_starts
                .get(normal_rank)
                .copied()
                .unwrap_or(normal_rank);
            let declarations = &prepared.get(matched.id).style_rule().declarations;
            let declarations = if important {
                &declarations.important_declarations
            } else {
                &declarations.declarations
            };
            for (index, _) in declarations.iter().enumerate() {
                events.push(DeclarationEvent {
                    block: matched.id.0,
                    index: u32::try_from(index).expect("declaration index"),
                    boundary: CascadeBoundary::Rule {
                        priority,
                        important,
                        normal_rank,
                        layer_start,
                    },
                });
            }
        }
        if !important {
            events.extend(hint_events.by_ref());
        }
        if origin != Some(CascadeOrigin::UserAgent)
            && let Some(inline) = element.inline
        {
            let declarations = if important {
                &inline.declarations.important_declarations
            } else {
                &inline.declarations.declarations
            };
            events.extend(
                declarations
                    .iter()
                    .enumerate()
                    .map(|(index, _)| DeclarationEvent {
                        block: 0,
                        index: u32::try_from(index).expect("declaration indices fit in u32"),
                        boundary: CascadeBoundary::Inline {
                            important,
                            normal_rank: normal_rules.len(),
                        },
                    }),
            );
        }
    }
}

/// One bit per catalogue entry, including shorthand targets. Local sets avoid
/// retaining heap buffers or epochs between cascade passes.
#[derive(Default)]
struct PropertySet([u64; PropertyKey::ALL.len().div_ceil(64)]);

impl PropertySet {
    fn contains(&self, property: PropertyKey) -> bool {
        let index = property as usize;
        self.0[index / 64] & (1u64 << (index % 64)) != 0
    }

    fn insert(&mut self, property: PropertyKey) {
        let index = property as usize;
        self.0[index / 64] |= 1u64 << (index % 64);
    }
}

pub(super) struct SelectedDeclaration {
    target_start: usize,
    target_len: usize,
    pub(super) sequence: usize,
}

#[derive(Default)]
pub(super) struct SpecifiedSelection {
    pub(super) declarations: Vec<SelectedDeclaration>,
    targets: Vec<PropertyKey>,
    rollbacks: RollbackMap<PropertyKey>,
    global_rollbacks: Vec<CascadeRollback>,
}

impl SpecifiedSelection {
    pub(super) fn targets_for(&self, declaration: &SelectedDeclaration) -> &[PropertyKey] {
        &self.targets[declaration.target_start..declaration.target_start + declaration.target_len]
    }
}

#[derive(Clone, Copy)]
pub(super) struct CascadeRollback {
    boundary: CascadeBoundary,
    kind: RollbackKind,
}

impl CascadeRollback {
    pub(super) fn new(boundary: CascadeBoundary, kind: RollbackKind) -> Self {
        Self { boundary, kind }
    }

    pub(super) fn excludes(self, candidate: CascadeBoundary) -> bool {
        self.boundary
            .rollback_excludes(candidate, matches!(self.kind, RollbackKind::Layer))
    }
}

/// Per-property rollback chains share one retained allocation.
pub(super) struct RollbackMap<K> {
    heads: FxHashMap<K, usize>,
    entries: Vec<(CascadeRollback, Option<usize>)>,
}

impl<K> Default for RollbackMap<K> {
    fn default() -> Self {
        Self {
            heads: FxHashMap::default(),
            entries: Vec::new(),
        }
    }
}

impl<K: Eq + Hash> RollbackMap<K> {
    pub(super) fn clear(&mut self) {
        self.heads.clear();
        self.entries.clear();
    }

    pub(super) fn push(&mut self, key: K, rollback: CascadeRollback) {
        let previous = self.heads.insert(key, self.entries.len());
        self.entries.push((rollback, previous));
    }

    pub(super) fn action(
        &self,
        key: K,
        boundary: CascadeBoundary,
        keyword: Option<RollbackKind>,
    ) -> CandidateAction {
        let mut next = self.heads.get(&key).copied();
        while let Some(index) = next {
            let (rollback, previous) = self.entries[index];
            if rollback.excludes(boundary) {
                return CandidateAction::Excluded;
            }
            next = previous;
        }
        keyword.map_or(CandidateAction::Select, CandidateAction::Rollback)
    }
}

#[derive(Clone, Copy)]
pub(super) enum CandidateAction {
    Excluded,
    Rollback(RollbackKind),
    Select,
}

pub(super) fn candidate_action(
    rollbacks: &[CascadeRollback],
    boundary: CascadeBoundary,
    keyword: Option<RollbackKind>,
) -> CandidateAction {
    if rollbacks
        .iter()
        .copied()
        .any(|rollback| rollback.excludes(boundary))
    {
        CandidateAction::Excluded
    } else if let Some(kind) = keyword {
        CandidateAction::Rollback(kind)
    } else {
        CandidateAction::Select
    }
}

#[derive(Clone, Copy)]
pub(super) enum RollbackKind {
    Origin,
    Layer,
}

pub(super) fn rollback_keyword(keyword: CssWideKeyword) -> Option<RollbackKind> {
    match keyword {
        CssWideKeyword::Revert => Some(RollbackKind::Origin),
        CssWideKeyword::RevertLayer => Some(RollbackKind::Layer),
        _ => None,
    }
}

/// Return only the declarations that win at least one longhand. The output is
/// restored to low-to-high order so computed-value conversion remains ordered
/// for logical/physical aliases that share renderer storage.
pub(super) fn select_specified_values<'sheet, 'css>(
    events: &[DeclarationEvent],
    element: ElementDeclarations<'_, 'css>,
    selected: &mut SpecifiedSelection,
) {
    let mut claimed = PropertySet::default();
    let rollbacks = &mut selected.rollbacks;
    rollbacks.clear();
    let global_rollbacks = &mut selected.global_rollbacks;
    global_rollbacks.clear();
    let mut all_claimed = false;
    selected.declarations.clear();
    selected.declarations.reserve(events.len());
    selected.targets.clear();
    selected.targets.reserve(events.len());

    for (sequence, event) in events.iter().enumerate().rev() {
        let property = event.property(element);
        if declaration_is_custom_property(property) {
            continue;
        }
        let targets = event.prepared_targets(element);
        let is_all = matches!(property, Property::All(_));

        if is_all {
            match candidate_action(
                &global_rollbacks,
                event.boundary,
                property_css_wide_keyword(property).and_then(rollback_keyword),
            ) {
                CandidateAction::Excluded => continue,
                CandidateAction::Rollback(kind) => {
                    global_rollbacks.push(CascadeRollback::new(event.boundary, kind));
                }
                CandidateAction::Select if !all_claimed => {
                    let target_start = selected.targets.len();
                    selected.targets.extend_from_slice(targets);
                    selected.declarations.push(SelectedDeclaration {
                        target_start,
                        target_len: targets.len(),
                        sequence,
                    });
                    all_claimed = true;
                }
                CandidateAction::Select => {}
            }
            continue;
        }

        let target_start = selected.targets.len();
        for &target in targets {
            // `all` deliberately excludes direction and custom properties.
            if (target.affected_by_all() && all_claimed) || claimed.contains(target) {
                continue;
            }
            if target.affected_by_all()
                && matches!(
                    candidate_action(&global_rollbacks, event.boundary, None),
                    CandidateAction::Excluded
                )
            {
                continue;
            }
            match rollbacks.action(
                target,
                event.boundary,
                property_css_wide_keyword(property).and_then(rollback_keyword),
            ) {
                CandidateAction::Excluded => continue,
                CandidateAction::Rollback(kind) => {
                    rollbacks.push(target, CascadeRollback::new(event.boundary, kind));
                }
                CandidateAction::Select => {
                    claimed.insert(target);
                    selected.targets.push(target);
                }
            }
        }
        let target_len = selected.targets.len() - target_start;
        if target_len != 0 {
            selected.declarations.push(SelectedDeclaration {
                target_start,
                target_len,
                sequence,
            });
        }
    }

    selected
        .declarations
        .sort_by_key(|declaration| declaration.sequence);
}

#[cfg(test)]
mod tests {
    use super::super::plan::CascadePlan;
    use super::*;
    use crate::MediaEnvironment;
    use crate::allocation_test_support::count_allocations;
    use crate::style::rules::prepared::prepare_stylesheets;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::stylesheet::StyleAttribute;
    use lightningcss::stylesheet::StyleSheet;

    #[test]
    fn shorthand_and_longhand_select_independent_winners() {
        let inline =
            StyleAttribute::parse("margin: 1px; margin-left: 2px", ParserOptions::default())
                .unwrap();
        let inline = crate::style::source::inline::PreparedInlineStyle::from(inline);
        let events = [
            DeclarationEvent {
                block: 0,
                index: 0,
                boundary: CascadeBoundary::Inline {
                    important: false,
                    normal_rank: 0,
                },
            },
            DeclarationEvent {
                block: 0,
                index: 1,
                boundary: CascadeBoundary::Inline {
                    important: false,
                    normal_rank: 0,
                },
            },
        ];
        let mut selected = SpecifiedSelection::default();
        select_specified_values(
            &events,
            ElementDeclarations {
                prepared: None,
                inline: Some(&inline),
                hints: &[],
                hint_preparation: None,
            },
            &mut selected,
        );
        assert_eq!(selected.declarations.len(), 2);
        assert!(
            !selected
                .targets_for(&selected.declarations[0])
                .contains(&PropertyKey::MarginLeft)
        );
        assert_eq!(
            selected.targets_for(&selected.declarations[1])[0],
            PropertyKey::MarginLeft
        );
    }

    #[test]
    fn repeated_inline_target_selection_reuses_its_scratch_buffer() {
        // Exercise both sides of every 64-property boundary and a repeated
        // property; separate target identities must survive each selection.
        let inline = StyleAttribute::parse(
            "width: 1px; color: red; padding-block-start: initial; padding-block-end: initial; word-break: initial; line-break: initial; text-box-trim: initial; text-box-edge: initial; width: 2px",
            ParserOptions::default(),
        )
        .unwrap();
        let inline = crate::style::source::inline::PreparedInlineStyle::from(inline);
        let events = inline
            .declarations
            .declarations
            .iter()
            .enumerate()
            .map(|(index, _)| DeclarationEvent {
                block: 0,
                index: index as u32,
                boundary: CascadeBoundary::Inline {
                    important: false,
                    normal_rank: 0,
                },
            })
            .collect::<Vec<_>>();
        let mut selected = SpecifiedSelection::default();
        select_specified_values(
            &events,
            ElementDeclarations {
                prepared: None,
                inline: Some(&inline),
                hints: &[],
                hint_preparation: None,
            },
            &mut selected,
        );

        let (_, allocations) = count_allocations(|| {
            select_specified_values(
                &events,
                ElementDeclarations {
                    prepared: None,
                    inline: Some(&inline),
                    hints: &[],
                    hint_preparation: None,
                },
                &mut selected,
            )
        });
        assert_eq!(allocations, 0);
        assert_eq!(
            selected.targets,
            [
                PropertyKey::Width,
                PropertyKey::TextBoxEdge,
                PropertyKey::TextBoxTrim,
                PropertyKey::LineBreak,
                PropertyKey::WordBreak,
                PropertyKey::PaddingBlockEnd,
                PropertyKey::PaddingBlockStart,
                PropertyKey::Color,
            ],
        );
        assert_eq!(
            selected
                .declarations
                .iter()
                .map(|declaration| declaration.sequence)
                .collect::<Vec<_>>(),
            (1..9).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn event_passes_preserve_origins_layers_hints_and_inline_order() {
        let user_agent = StyleSheet::parse(
            "@layer ua { p { width:1px; width:11px!important } } p { width:2px; width:12px!important }",
            ParserOptions::default(),
        )
        .unwrap();
        let authors = [StyleSheet::parse(
            "@layer a { p { width:3px; width:13px!important } p { width:3.5px; width:13.5px!important } } @layer b { p { width:4px; width:14px!important } } p { width:5px; width:15px!important }",
            ParserOptions::default(),
        )
        .unwrap()];
        let prepared =
            prepare_stylesheets(&user_agent, &authors, MediaEnvironment::default(), 16.0);
        let matched = prepared
            .iter()
            .map(|(id, _)| MatchedRule {
                specificity: 1,
                id,
                scope_proximity: u32::MAX,
            })
            .collect();
        let mut plan = CascadePlan {
            normal: matched,
            important: Vec::new(),
        };
        plan.build(&prepared);
        let inline =
            StyleAttribute::parse("width:6px; width:16px!important", ParserOptions::default())
                .unwrap();
        let inline = crate::style::source::inline::PreparedInlineStyle::from(inline);
        let hints = StyleAttribute::parse("width:2.5px", ParserOptions::default()).unwrap();
        let mut hint_preparation = PreparedDeclarationBlock::default();
        hint_preparation.append(&hints.declarations.declarations);
        let element = ElementDeclarations {
            prepared: Some(&prepared),
            inline: Some(&inline),
            hints: &hints.declarations.declarations,
            hint_preparation: Some(&hint_preparation),
        };
        let mut scratch = EventScratch::default();
        build_cascade_events(
            &prepared,
            &plan.normal,
            &plan.important,
            element,
            &mut scratch,
        );
        let expected = StyleAttribute::parse(
            "width:1px; width:2px; width:2.5px; width:3.5px; width:4px; width:5px; width:6px; width:15px; width:14px; width:13.5px; width:16px; width:12px; width:11px",
            ParserOptions::default(),
        )
        .unwrap();
        assert_eq!(
            scratch
                .events
                .iter()
                .map(|event| event.property(element))
                .collect::<Vec<_>>(),
            expected
                .declarations
                .declarations
                .iter()
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            scratch
                .events
                .iter()
                .filter_map(|event| match event.boundary {
                    CascadeBoundary::Rule {
                        normal_rank,
                        layer_start,
                        ..
                    } => Some((normal_rank, layer_start)),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [
                (0, 0),
                (1, 1),
                (3, 2),
                (4, 4),
                (5, 5),
                (5, 5),
                (4, 4),
                (3, 2),
                (1, 1),
                (0, 0)
            ],
        );
    }

    #[test]
    fn event_construction_skips_only_safely_shadowed_whole_rules() {
        fn event_count(css: &str) -> usize {
            let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
            let author = StyleSheet::parse(css, ParserOptions::default()).unwrap();
            let authors = [author];
            let prepared =
                prepare_stylesheets(&user_agent, &authors, MediaEnvironment::default(), 16.0);
            let rules = prepared
                .iter()
                .map(|(id, _)| MatchedRule {
                    specificity: 1,
                    id,
                    scope_proximity: u32::MAX,
                })
                .collect::<Vec<_>>();
            let mut scratch = EventScratch::default();
            build_cascade_events(
                &prepared,
                &rules,
                &[],
                ElementDeclarations::default(),
                &mut scratch,
            );
            scratch.events.len()
        }

        assert_eq!(event_count("p { color: red } p { color: blue }"), 1);
        let dense = (0..128)
            .map(|value| format!("p {{ color:rgb({value} 20 30); background-color:rgb(20 {value} 30); text-align:left }}"))
            .collect::<String>();
        assert_eq!(
            event_count(&dense),
            3,
            "384 authored declarations must be pruned before event construction"
        );
        assert_eq!(
            event_count("@layer a { p { color: red } } @layer b { p { color: blue } }"),
            2
        );
        assert_eq!(event_count("p { width: 10px } p { width: -1px }"), 2);
        assert_eq!(
            event_count("p { color: red; --keep: yes } p { color: blue }"),
            3
        );
    }
}
