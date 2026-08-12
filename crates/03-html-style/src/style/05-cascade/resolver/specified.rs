//! Selection of cascaded specified values before computed-value conversion.
//!
//! Lightning CSS exposes shorthand expansion, so winners are tracked by
//! longhand rather than by declaration spelling. This is also where
//! `revert` and `revert-layer` discard candidates from their cascade scope.

use super::*;
use std::sync::Arc;

const ALWAYS_COMPUTABLE: u8 = 1 << 0;
const ELIGIBILITY_KNOWN: u8 = 1 << 1;
const RENDERER_ELIGIBLE: u8 = 1 << 2;

fn declaration_flags(always_computable: bool, renderer_eligibility: Option<bool>) -> u8 {
    u8::from(always_computable) * ALWAYS_COMPUTABLE
        | u8::from(renderer_eligibility.is_some()) * ELIGIBILITY_KNOWN
        | u8::from(renderer_eligibility == Some(true)) * RENDERER_ELIGIBLE
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) enum CascadeBoundary {
    Rule { priority: RulePriority, important: bool, declaration_flags: u8, normal_rank: usize, layer_start: usize },
    Inline { important: bool, normal_rank: usize },
}

impl CascadeBoundary {
    pub(super) fn origin(self) -> CascadeOrigin {
        match self {
            Self::Rule { priority, .. } => priority.origin(),
            Self::Inline { .. } => CascadeOrigin::Author,
        }
    }

    pub(super) fn same_layer(self, other: Self) -> bool {
        match (self, other) {
            (Self::Rule { priority: left, .. }, Self::Rule { priority: right, .. }) => left.same_origin_and_layer(right),
            (Self::Inline { .. }, Self::Inline { .. }) => true,
            _ => false,
        }
    }

    fn important(self) -> bool {
        match self {
            Self::Rule { important, .. } | Self::Inline { important, .. } => important,
        }
    }

    fn normal_rank(self) -> usize {
        match self {
            Self::Rule { normal_rank, .. } | Self::Inline { normal_rank, .. } => normal_rank,
        }
    }

    fn layer_start(self) -> usize {
        match self {
            Self::Rule { layer_start, .. } => layer_start,
            Self::Inline { normal_rank, .. } => normal_rank,
        }
    }

    pub(super) fn rollback_excludes(self, candidate: Self, layer: bool) -> bool {
        if !layer {
            return candidate.origin() == self.origin();
        }
        if matches!(self, Self::Inline { important: true, .. }) {
            // Important element-attached styles are their own outer layer.
            // Their rollback does not cross into author stylesheet rules.
            return matches!(candidate, Self::Inline { .. });
        }
        if self.important() { candidate.origin() == self.origin() && (candidate.important() || candidate.normal_rank() >= self.layer_start()) } else { candidate.same_layer(self) }
    }
}

#[derive(Clone, Copy)]
enum DeclarationSource<'sheet, 'css> {
    Rule { property: &'sheet Property<'css>, targets: &'sheet [PreparedPropertyTarget] },
    Inline { important: bool, index: u32 },
}

#[derive(Clone, Copy)]
pub(super) struct DeclarationEvent<'sheet, 'css> {
    source: DeclarationSource<'sheet, 'css>,
    pub(super) boundary: CascadeBoundary,
}

impl<'sheet, 'css> DeclarationEvent<'sheet, 'css> {
    pub(super) fn property<'a>(&self, inline_style: Option<&'a StyleAttribute<'css>>) -> &'a Property<'css>
    where
        'sheet: 'a,
    {
        let (declarations, index) = match self.source {
            DeclarationSource::Rule { property, .. } => return property,
            DeclarationSource::Inline { important, index } => {
                let declarations = &inline_style.expect("inline declaration events require their parsed style attribute").declarations;
                (if important { &declarations.important_declarations } else { &declarations.declarations }, index)
            }
        };
        &declarations[index as usize]
    }

    fn prepared_targets(&self) -> Option<&'sheet [PreparedPropertyTarget]> {
        match self.source {
            DeclarationSource::Rule { targets, .. } => Some(targets),
            DeclarationSource::Inline { .. } => None,
        }
    }

    pub(super) fn is_always_computable(&self) -> bool {
        matches!(self.boundary, CascadeBoundary::Rule { declaration_flags, .. } if declaration_flags & ALWAYS_COMPUTABLE != 0)
    }

    pub(super) fn prepared_renderer_eligibility(&self) -> Option<bool> {
        match self.boundary {
            CascadeBoundary::Rule { declaration_flags, .. } if declaration_flags & ELIGIBILITY_KNOWN != 0 => Some(declaration_flags & RENDERER_ELIGIBLE != 0),
            CascadeBoundary::Rule { .. } => None,
            CascadeBoundary::Inline { .. } => None,
        }
    }
}

#[derive(Default)]
pub(super) struct EventScratch<'sheet, 'css> {
    pub(super) events: Vec<DeclarationEvent<'sheet, 'css>>,
    normal_ranks: FxHashMap<EffectiveRuleId, usize>,
    layer_starts: Vec<usize>,
    skipped_rules: Vec<bool>,
    shadow_epochs: Vec<u32>,
    shadow_epoch: u32,
}

fn mark_fully_shadowed_rules(prepared: &PreparedRuleSet<'_>, rules: &[MatchedRule], important: bool, origin: Option<CascadeOrigin>, skipped_rules: &mut Vec<bool>, shadow_epochs: &mut Vec<u32>, shadow_epoch: &mut u32) {
    skipped_rules.clear();
    skipped_rules.resize(rules.len(), false);
    let mut group_end = rules.len();
    while group_end != 0 {
        let priority = prepared.get(rules[group_end - 1].id).priority();
        let mut group_start = group_end - 1;
        while group_start != 0 && prepared.get(rules[group_start - 1].id).priority().same_origin_and_layer(priority) {
            group_start -= 1;
        }
        if origin.is_some_and(|origin| priority.origin() != origin) {
            group_end = group_start;
            continue;
        }
        // Sparse mixed-property groups cost more to inspect than they can
        // save. Only activate when safe whole-rule candidates are dense.
        let candidate_count = rules[group_start..group_end].iter().filter(|matched| prepared.rule_shadow_declarations(matched.id, important).1).count();
        if candidate_count < 2 || candidate_count * 2 < group_end - group_start {
            group_end = group_start;
            continue;
        }
        if shadow_epochs.len() < prepared.property_slot_count() {
            shadow_epochs.resize(prepared.property_slot_count(), 0);
        }
        *shadow_epoch = shadow_epoch.wrapping_add(1);
        if *shadow_epoch == 0 {
            shadow_epochs.fill(0);
            *shadow_epoch = 1;
        }
        for rule_index in (group_start..group_end).rev() {
            let matched = &rules[rule_index];
            let (declaration_mask, all_shadowable) = prepared.rule_shadow_declarations(matched.id, important);
            let mut covered = all_shadowable;
            let mut declarations = declaration_mask;
            while covered && declarations != 0 {
                let index = declarations.trailing_zeros() as usize;
                declarations &= declarations - 1;
                covered = prepared.declaration_targets(matched.id, important, index).iter().all(|target| shadow_epochs.get(target.slot as usize).copied() == Some(*shadow_epoch));
            }
            skipped_rules[rule_index] = covered;
            let mut declarations = declaration_mask;
            while declarations != 0 {
                let index = declarations.trailing_zeros() as usize;
                declarations &= declarations - 1;
                for target in prepared.declaration_targets(matched.id, important, index) {
                    let slot = target.slot as usize;
                    shadow_epochs[slot] = *shadow_epoch;
                }
            }
        }
        group_end = group_start;
    }
}

#[derive(Default)]
pub(super) struct CascadeScratch<'sheet, 'css> {
    pub(super) events: EventScratch<'sheet, 'css>,
    pub(super) valid_events: Vec<DeclarationEvent<'sheet, 'css>>,
    pub(super) invalid_sequences: Vec<usize>,
    pub(super) selection: SpecifiedSelection,
}

pub(super) fn build_cascade_events<'prepared, 'css>(prepared: &'prepared PreparedRuleSet<'css>, normal_rules: &[MatchedRule], important_rules: &[MatchedRule], inline_style: Option<&StyleAttribute<'css>>, scratch: &mut EventScratch<'prepared, 'css>) -> usize {
    let inline_capacity = inline_style.map_or(0, |inline| inline.declarations.declarations.len() + inline.declarations.important_declarations.len());
    let event_capacity = normal_rules.iter().map(|matched| prepared.get(matched.id).style_rule().declarations.declarations.len()).sum::<usize>() + important_rules.iter().map(|matched| prepared.get(matched.id).style_rule().declarations.important_declarations.len()).sum::<usize>() + inline_capacity;
    let events = &mut scratch.events;
    events.clear();
    events.reserve(event_capacity);
    let mut hints_sequence = None;
    let normal_ranks = &mut scratch.normal_ranks;
    normal_ranks.clear();
    normal_ranks.reserve(normal_rules.len());
    let layer_starts = &mut scratch.layer_starts;
    layer_starts.clear();
    layer_starts.reserve(normal_rules.len());
    let mut layer_start = 0;
    for (rank, matched) in normal_rules.iter().enumerate() {
        normal_ranks.insert(matched.id, rank);
        if rank > 0 && !prepared.get(normal_rules[rank - 1].id).priority().same_origin_and_layer(prepared.get(matched.id).priority()) {
            layer_start = rank;
        }
        layer_starts.push(layer_start);
    }
    mark_fully_shadowed_rules(prepared, normal_rules, false, None, &mut scratch.skipped_rules, &mut scratch.shadow_epochs, &mut scratch.shadow_epoch);
    for (normal_rank, matched) in normal_rules.iter().enumerate() {
        if scratch.skipped_rules[normal_rank] {
            continue;
        }
        let rule = prepared.get(matched.id);
        if hints_sequence.is_none() && rule.priority().origin() == CascadeOrigin::Author {
            hints_sequence = Some(events.len());
        }
        events.extend(rule.style_rule().declarations.declarations.iter().enumerate().map(|(index, property)| {
            let (targets, always_computable, renderer_eligibility) = prepared.declaration_metadata(matched.id, false, index);
            DeclarationEvent {
            source: DeclarationSource::Rule { property, targets },
            boundary: CascadeBoundary::Rule {
                priority: rule.priority(),
                important: false,
                declaration_flags: declaration_flags(always_computable, renderer_eligibility),
                normal_rank,
                layer_start: layer_starts[normal_rank],
            },
        } }));
    }
    let hints_sequence = hints_sequence.unwrap_or(events.len());
    if let Some(inline) = inline_style {
        events.extend(inline.declarations.declarations.iter().enumerate().map(|(index, _)| DeclarationEvent {
            source: DeclarationSource::Inline { important: false, index: u32::try_from(index).expect("declaration indices fit in u32") },
            boundary: CascadeBoundary::Inline { important: false, normal_rank: normal_rules.len() },
        }));
    }
    mark_fully_shadowed_rules(prepared, important_rules, true, Some(CascadeOrigin::Author), &mut scratch.skipped_rules, &mut scratch.shadow_epochs, &mut scratch.shadow_epoch);
    for (rule_index, matched) in important_rules.iter().enumerate().filter(|(_, matched)| prepared.get(matched.id).priority().origin() == CascadeOrigin::Author) {
        if scratch.skipped_rules[rule_index] {
            continue;
        }
        let rule = prepared.get(matched.id);
        let normal_rank = normal_ranks.get(&matched.id).copied().unwrap_or(normal_rules.len());
        events.extend(rule.style_rule().declarations.important_declarations.iter().enumerate().map(|(index, property)| {
            let (targets, always_computable, renderer_eligibility) = prepared.declaration_metadata(matched.id, true, index);
            DeclarationEvent {
            source: DeclarationSource::Rule { property, targets },
            boundary: CascadeBoundary::Rule {
                priority: rule.priority(),
                important: true,
                declaration_flags: declaration_flags(always_computable, renderer_eligibility),
                normal_rank,
                layer_start: layer_starts.get(normal_rank).copied().unwrap_or(normal_rank),
            },
        } }));
    }
    if let Some(inline) = inline_style {
        events.extend(inline.declarations.important_declarations.iter().enumerate().map(|(index, _)| DeclarationEvent {
            source: DeclarationSource::Inline { important: true, index: u32::try_from(index).expect("declaration indices fit in u32") },
            boundary: CascadeBoundary::Inline { important: true, normal_rank: normal_rules.len() },
        }));
    }
    mark_fully_shadowed_rules(prepared, important_rules, true, Some(CascadeOrigin::UserAgent), &mut scratch.skipped_rules, &mut scratch.shadow_epochs, &mut scratch.shadow_epoch);
    for (rule_index, matched) in important_rules.iter().enumerate().filter(|(_, matched)| prepared.get(matched.id).priority().origin() == CascadeOrigin::UserAgent) {
        if scratch.skipped_rules[rule_index] {
            continue;
        }
        let rule = prepared.get(matched.id);
        let normal_rank = normal_ranks.get(&matched.id).copied().unwrap_or(normal_rules.len());
        events.extend(rule.style_rule().declarations.important_declarations.iter().enumerate().map(|(index, property)| {
            let (targets, always_computable, renderer_eligibility) = prepared.declaration_metadata(matched.id, true, index);
            DeclarationEvent {
            source: DeclarationSource::Rule { property, targets },
            boundary: CascadeBoundary::Rule {
                priority: rule.priority(),
                important: true,
                declaration_flags: declaration_flags(always_computable, renderer_eligibility),
                normal_rank,
                layer_start: layer_starts.get(normal_rank).copied().unwrap_or(normal_rank),
            },
        } }));
    }
    hints_sequence
}

pub(super) type PropertyTarget = PreparedPropertyTarget;

#[derive(Default)]
pub(super) struct PropertyTargetState {
    slot_ids: Option<FxHashMap<Arc<str>, u32>>,
    claim_epochs: Vec<u32>,
    epoch: u32,
}

impl PropertyTargetState {
    fn begin_selection(&mut self) -> u32 {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.claim_epochs.fill(0);
            self.epoch = 1;
        }
        self.epoch
    }

    fn is_claimed(&self, slot: u32, epoch: u32) -> bool {
        self.claim_epochs.get(slot as usize).copied() == Some(epoch)
    }

    fn claim(&mut self, slot: u32, epoch: u32) {
        let slot = slot as usize;
        if self.claim_epochs.len() <= slot {
            self.claim_epochs.resize(slot + 1, 0);
        }
        self.claim_epochs[slot] = epoch;
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
    targets: Vec<PropertyTarget>,
    inline_targets: Vec<PropertyTarget>,
    /// Presentational hints cascade as a distinct origin, but `revert` in the
    /// author origin also rolls that origin back.
    pub(super) reverted_hint_targets: rustc_data_structures::fx::FxHashSet<Arc<str>>,
    pub(super) reverted_all_hints: bool,
}

impl SpecifiedSelection {
    pub(super) fn targets_for(&self, declaration: &SelectedDeclaration) -> &[PropertyTarget] {
        &self.targets[declaration.target_start..declaration.target_start + declaration.target_len]
    }
}

#[derive(Clone, Copy)]
enum Rollback {
    Origin(CascadeOrigin),
    Layer(CascadeBoundary),
}

impl Rollback {
    fn excludes(self, boundary: CascadeBoundary) -> bool {
        match self {
            Self::Origin(origin) => boundary.origin() == origin,
            Self::Layer(layer) => layer.rollback_excludes(boundary, true),
        }
    }
}

fn rollback(property: &Property<'_>) -> Option<RollbackKind> {
    let keyword = match property {
        Property::All(CSSWideKeyword::Revert) => return Some(RollbackKind::Origin),
        Property::All(CSSWideKeyword::RevertLayer) => return Some(RollbackKind::Layer),
        Property::Unparsed(value) => single_ident_keyword(&value.value)?,
        Property::Custom(value) if !value.name.as_ref().starts_with("--") => single_ident_keyword(&value.value)?,
        _ => return None,
    };
    if keyword.eq_ignore_ascii_case("revert") {
        Some(RollbackKind::Origin)
    } else if keyword.eq_ignore_ascii_case("revert-layer") {
        Some(RollbackKind::Layer)
    } else {
        None
    }
}

#[derive(Clone, Copy)]
enum RollbackKind {
    Origin,
    Layer,
}

pub(super) fn declaration_is_custom_property(property: &Property<'_>) -> bool {
    match property {
        Property::Custom(value) => value.name.as_ref().starts_with("--"),
        Property::Unparsed(value) => value.property_id.name().starts_with("--"),
        _ => false,
    }
}

/// Return only the declarations that win at least one longhand. The output is
/// restored to low-to-high order so computed-value conversion remains ordered
/// for logical/physical aliases that share renderer storage.
pub(super) fn select_specified_values<'sheet, 'css>(events: &[DeclarationEvent<'sheet, 'css>], prepared: &PreparedRuleSet<'css>, inline_style: Option<&StyleAttribute<'css>>, target_state: &mut PropertyTargetState, selected: &mut SpecifiedSelection) {
    let claim_epoch = target_state.begin_selection();
    let mut rollbacks = FxHashMap::<u32, Vec<Rollback>>::default();
    let mut global_rollbacks = Vec::<Rollback>::new();
    let mut all_claimed = false;
    selected.declarations.clear();
    selected.declarations.reserve(events.len());
    selected.targets.clear();
    selected.targets.reserve(events.len());
    selected.reverted_hint_targets.clear();
    let mut reverted_all_hints = false;

    for (sequence, event) in events.iter().enumerate().rev() {
        let property = event.property(inline_style);
        if declaration_is_custom_property(property) {
            continue;
        }
        let targets = if let Some(targets) = event.prepared_targets() {
            targets
        } else {
            let slot_ids = target_state.slot_ids.get_or_insert_with(|| prepared.clone_property_slots());
            compile_property_targets(property, slot_ids, &mut selected.inline_targets);
            &selected.inline_targets
        };
        let is_all = matches!(property, Property::All(_));
        let rollback_kind = rollback(property);

        if is_all {
            if global_rollbacks.iter().copied().any(|rollback| rollback.excludes(event.boundary)) {
                continue;
            }
            if let Some(kind) = rollback_kind {
                if matches!(kind, RollbackKind::Origin) && event.boundary.origin() == CascadeOrigin::Author {
                    reverted_all_hints = true;
                }
                global_rollbacks.push(match kind {
                    RollbackKind::Origin => Rollback::Origin(event.boundary.origin()),
                    RollbackKind::Layer => Rollback::Layer(event.boundary),
                });
            } else if !all_claimed {
                let target_start = selected.targets.len();
                selected.targets.extend(targets.iter().cloned());
                selected.declarations.push(SelectedDeclaration { target_start, target_len: targets.len(), sequence });
                all_claimed = true;
            }
            continue;
        }

        let target_start = selected.targets.len();
        for target in targets.iter() {
            // `all` deliberately excludes direction and custom properties.
            if (&*target.name != "direction" && all_claimed) || target_state.is_claimed(target.slot, claim_epoch) {
                continue;
            }
            if &*target.name != "direction" && global_rollbacks.iter().copied().any(|rollback| rollback.excludes(event.boundary)) {
                continue;
            }
            if rollbacks.get(&target.slot).is_some_and(|rollbacks| rollbacks.iter().copied().any(|rollback| rollback.excludes(event.boundary))) {
                continue;
            }
            if let Some(kind) = rollback_kind {
                if matches!(kind, RollbackKind::Origin) && event.boundary.origin() == CascadeOrigin::Author {
                    selected.reverted_hint_targets.insert(target.name.clone());
                }
                rollbacks.entry(target.slot).or_default().push(match kind {
                    RollbackKind::Origin => Rollback::Origin(event.boundary.origin()),
                    RollbackKind::Layer => Rollback::Layer(event.boundary),
                });
            } else {
                target_state.claim(target.slot, claim_epoch);
                selected.targets.push(target.clone());
            }
        }
        let target_len = selected.targets.len() - target_start;
        if target_len != 0 {
            selected.declarations.push(SelectedDeclaration { target_start, target_len, sequence });
        }
    }

    selected.declarations.sort_by_key(|declaration| declaration.sequence);
    selected.reverted_all_hints = reverted_all_hints;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MediaEnvironment;
    use crate::allocation_test_support::count_allocations;
    use crate::style::rules::prepared::ParsedStylesheetSet;
    use lightningcss::stylesheet::StyleSheet;

    #[test]
    fn shorthand_and_longhand_select_independent_winners() {
        let stylesheet = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let prepared = ParsedStylesheetSet::new(&stylesheet, &[]).prepare(MediaEnvironment::default(), 16.0);
        let inline = StyleAttribute::parse("margin: 1px; margin-left: 2px", ParserOptions::default()).unwrap();
        let events = [
            DeclarationEvent {
                source: DeclarationSource::Inline { important: false, index: 0 },
                boundary: CascadeBoundary::Inline { important: false, normal_rank: 0 },
            },
            DeclarationEvent {
                source: DeclarationSource::Inline { important: false, index: 1 },
                boundary: CascadeBoundary::Inline { important: false, normal_rank: 0 },
            },
        ];
        let mut selected = SpecifiedSelection::default();
        select_specified_values(&events, &prepared, Some(&inline), &mut PropertyTargetState::default(), &mut selected);
        assert_eq!(selected.declarations.len(), 2);
        assert!(!selected.targets_for(&selected.declarations[0]).iter().any(|target| &*target.name == "margin-left"));
        assert_eq!(&*selected.targets_for(&selected.declarations[1])[0].name, "margin-left");
    }

    #[test]
    fn repeated_inline_target_selection_reuses_its_scratch_buffer() {
        let stylesheet = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let prepared = ParsedStylesheetSet::new(&stylesheet, &[]).prepare(MediaEnvironment::default(), 16.0);
        let inline = StyleAttribute::parse("width: 1px; color: red", ParserOptions::default()).unwrap();
        let events = [
            DeclarationEvent {
                source: DeclarationSource::Inline { important: false, index: 0 },
                boundary: CascadeBoundary::Inline { important: false, normal_rank: 0 },
            },
            DeclarationEvent {
                source: DeclarationSource::Inline { important: false, index: 1 },
                boundary: CascadeBoundary::Inline { important: false, normal_rank: 0 },
            },
        ];
        let mut target_state = PropertyTargetState::default();
        let mut selected = SpecifiedSelection::default();
        select_specified_values(&events, &prepared, Some(&inline), &mut target_state, &mut selected);

        let (_, allocations) = count_allocations(|| select_specified_values(&events, &prepared, Some(&inline), &mut target_state, &mut selected));
        assert_eq!(allocations, 0);
    }

    #[test]
    fn event_construction_skips_only_safely_shadowed_whole_rules() {
        fn event_count(css: &str) -> usize {
            let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
            let author = StyleSheet::parse(css, ParserOptions::default()).unwrap();
            let authors = [author];
            let prepared = ParsedStylesheetSet::new(&user_agent, &authors).prepare(MediaEnvironment::default(), 16.0);
            let rules = prepared.iter().map(|(id, _)| MatchedRule { specificity: 1, id, scope_proximity: u32::MAX }).collect::<Vec<_>>();
            let mut scratch = EventScratch::default();
            build_cascade_events(&prepared, &rules, &[], None, &mut scratch);
            scratch.events.len()
        }

        assert_eq!(event_count("p { color: red } p { color: blue }"), 1);
        assert_eq!(event_count("@layer a { p { color: red } } @layer b { p { color: blue } }"), 2);
        assert_eq!(event_count("p { width: 10px } p { width: -1px }"), 2);
        assert_eq!(event_count("p { color: red; --keep: yes } p { color: blue }"), 3);
    }
}
