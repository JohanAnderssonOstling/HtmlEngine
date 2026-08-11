//! Selection of cascaded specified values before computed-value conversion.
//!
//! Lightning CSS exposes shorthand expansion, so winners are tracked by
//! longhand rather than by declaration spelling. This is also where
//! `revert` and `revert-layer` discard candidates from their cascade scope.

use super::*;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(super) enum CascadeBoundary {
    Rule { priority: RulePriority, important: bool, normal_rank: usize, layer_start: usize },
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

pub(super) struct DeclarationEvent<'event, 'css> {
    pub(super) property: &'event Property<'css>,
    pub(super) boundary: CascadeBoundary,
}

pub(super) fn build_cascade_events<'event, 'sheet, 'css>(prepared: &'event PreparedRuleSet<'sheet, 'css>, normal_rules: &[MatchedRule], important_rules: &[MatchedRule], inline_style: Option<&'event StyleAttribute<'css>>) -> (Vec<DeclarationEvent<'event, 'css>>, usize) {
    let mut events = Vec::new();
    let mut hints_sequence = None;
    let mut normal_ranks = FxHashMap::default();
    let mut layer_starts = Vec::with_capacity(normal_rules.len());
    let mut layer_start = 0;
    for (rank, matched) in normal_rules.iter().enumerate() {
        normal_ranks.insert(matched.id, rank);
        if rank > 0 && !prepared.get(normal_rules[rank - 1].id).priority().same_origin_and_layer(prepared.get(matched.id).priority()) {
            layer_start = rank;
        }
        layer_starts.push(layer_start);
    }
    for (normal_rank, matched) in normal_rules.iter().enumerate() {
        let rule = prepared.get(matched.id);
        if hints_sequence.is_none() && rule.priority().origin() == CascadeOrigin::Author {
            hints_sequence = Some(events.len());
        }
        events.extend(rule.style_rule().declarations.declarations.iter().map(|property| DeclarationEvent {
            property,
            boundary: CascadeBoundary::Rule {
                priority: rule.priority(),
                important: false,
                normal_rank,
                layer_start: layer_starts[normal_rank],
            },
        }));
    }
    let hints_sequence = hints_sequence.unwrap_or(events.len());
    if let Some(inline) = inline_style {
        events.extend(inline.declarations.declarations.iter().map(|property| DeclarationEvent {
            property,
            boundary: CascadeBoundary::Inline { important: false, normal_rank: normal_rules.len() },
        }));
    }
    for matched in important_rules.iter().filter(|matched| prepared.get(matched.id).priority().origin() == CascadeOrigin::Author) {
        let rule = prepared.get(matched.id);
        let normal_rank = normal_ranks.get(&matched.id).copied().unwrap_or(normal_rules.len());
        events.extend(rule.style_rule().declarations.important_declarations.iter().map(|property| DeclarationEvent {
            property,
            boundary: CascadeBoundary::Rule {
                priority: rule.priority(),
                important: true,
                normal_rank,
                layer_start: layer_starts.get(normal_rank).copied().unwrap_or(normal_rank),
            },
        }));
    }
    if let Some(inline) = inline_style {
        events.extend(inline.declarations.important_declarations.iter().map(|property| DeclarationEvent {
            property,
            boundary: CascadeBoundary::Inline { important: true, normal_rank: normal_rules.len() },
        }));
    }
    for matched in important_rules.iter().filter(|matched| prepared.get(matched.id).priority().origin() == CascadeOrigin::UserAgent) {
        let rule = prepared.get(matched.id);
        let normal_rank = normal_ranks.get(&matched.id).copied().unwrap_or(normal_rules.len());
        events.extend(rule.style_rule().declarations.important_declarations.iter().map(|property| DeclarationEvent {
            property,
            boundary: CascadeBoundary::Rule {
                priority: rule.priority(),
                important: true,
                normal_rank,
                layer_start: layer_starts.get(normal_rank).copied().unwrap_or(normal_rank),
            },
        }));
    }
    (events, hints_sequence)
}

#[derive(Clone)]
pub(super) struct PropertyTarget {
    /// Canonical cascade slot. Aliases that affect the same computed property
    /// intentionally share this name.
    pub(super) name: Arc<str>,
    /// Document-local numeric identity used by the hot-path winner tables.
    slot: u32,
    /// Present when a shorthand must be materialized as one longhand before
    /// computed-value conversion.
    pub(super) longhand: Option<Arc<PropertyId<'static>>>,
}

#[derive(Default)]
pub(super) struct PropertyTargetState {
    cache: FxHashMap<usize, Arc<[PropertyTarget]>>,
    slot_ids: FxHashMap<Arc<str>, u32>,
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

pub(super) struct SelectedDeclaration<'event, 'css> {
    pub(super) property: &'event Property<'css>,
    pub(super) targets: Vec<PropertyTarget>,
    pub(super) sequence: usize,
}

pub(super) struct SpecifiedSelection<'event, 'css> {
    pub(super) declarations: Vec<SelectedDeclaration<'event, 'css>>,
    /// Presentational hints cascade as a distinct origin, but `revert` in the
    /// author origin also rolls that origin back.
    pub(super) reverted_hint_targets: rustc_data_structures::fx::FxHashSet<Arc<str>>,
    pub(super) reverted_all_hints: bool,
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

fn canonical_slot(name: &str) -> &str {
    match name {
        "word-wrap" => "overflow-wrap",
        "page-break-before" => "break-before",
        "page-break-after" => "break-after",
        "page-break-inside" => "break-inside",
        "grid-row-gap" => "row-gap",
        "grid-column-gap" => "column-gap",
        _ => name,
    }
}

fn property_target(name: &str, longhand: Option<Arc<PropertyId<'static>>>, slot_ids: &mut FxHashMap<Arc<str>, u32>) -> PropertyTarget {
    if let Some((name, slot)) = slot_ids.get_key_value(name) {
        return PropertyTarget { name: name.clone(), slot: *slot, longhand };
    }
    let name: Arc<str> = Arc::from(name);
    let slot = u32::try_from(slot_ids.len()).expect("CSS property slot count fits in u32");
    slot_ids.insert(name.clone(), slot);
    PropertyTarget { name, slot, longhand }
}

fn property_targets(property: &Property<'_>, slot_ids: &mut FxHashMap<Arc<str>, u32>) -> Vec<PropertyTarget> {
    if matches!(property, Property::All(_)) {
        return vec![property_target("all", None, slot_ids)];
    }
    let property_id = property.property_id();
    if let Some(longhands) = property_id.longhands() {
        return longhands
            .into_iter()
            .filter(|longhand| !crate::style::syntax::capabilities::property_uses_gradient(property) || longhand.name() == "background-image")
            .map(|longhand| {
                let mut target = property_target(canonical_slot(longhand.name()), None, slot_ids);
                target.longhand = Some(Arc::new(longhand));
                target
            })
            .collect();
    }
    vec![property_target(canonical_slot(property_id.name()), None, slot_ids)]
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
pub(super) fn select_specified_values<'event, 'css>(events: &[DeclarationEvent<'event, 'css>], target_state: &mut PropertyTargetState) -> SpecifiedSelection<'event, 'css> {
    let claim_epoch = target_state.begin_selection();
    let mut rollbacks = FxHashMap::<u32, Vec<Rollback>>::default();
    let mut global_rollbacks = Vec::<Rollback>::new();
    let mut all_claimed = false;
    let mut selected = Vec::new();
    let mut reverted_hint_targets = rustc_data_structures::fx::FxHashSet::default();
    let mut reverted_all_hints = false;

    for (sequence, event) in events.iter().enumerate().rev() {
        if declaration_is_custom_property(event.property) {
            continue;
        }
        let targets = if matches!(event.boundary, CascadeBoundary::Rule { .. }) {
            let key = event.property as *const Property<'css> as usize;
            let (cache, slot_ids) = (&mut target_state.cache, &mut target_state.slot_ids);
            cache.entry(key).or_insert_with(|| Arc::from(property_targets(event.property, slot_ids))).clone()
        } else {
            Arc::from(property_targets(event.property, &mut target_state.slot_ids))
        };
        let is_all = matches!(event.property, Property::All(_));
        let rollback_kind = rollback(event.property);

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
                selected.push(SelectedDeclaration { property: event.property, targets: targets.to_vec(), sequence });
                all_claimed = true;
            }
            continue;
        }

        let mut winning_targets = Vec::new();
        for target in targets.iter().cloned() {
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
                    reverted_hint_targets.insert(target.name.clone());
                }
                rollbacks.entry(target.slot).or_default().push(match kind {
                    RollbackKind::Origin => Rollback::Origin(event.boundary.origin()),
                    RollbackKind::Layer => Rollback::Layer(event.boundary),
                });
            } else {
                target_state.claim(target.slot, claim_epoch);
                winning_targets.push(target);
            }
        }
        if !winning_targets.is_empty() {
            selected.push(SelectedDeclaration { property: event.property, targets: winning_targets, sequence });
        }
    }

    selected.sort_by_key(|declaration| declaration.sequence);
    SpecifiedSelection { declarations: selected, reverted_hint_targets, reverted_all_hints }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorthand_and_longhand_select_independent_winners() {
        let shorthand = Property::parse_string(PropertyId::from("margin"), "1px", ParserOptions::default()).unwrap();
        let longhand = Property::parse_string(PropertyId::from("margin-left"), "2px", ParserOptions::default()).unwrap();
        let events = [
            DeclarationEvent {
                property: &shorthand,
                boundary: CascadeBoundary::Inline { important: false, normal_rank: 0 },
            },
            DeclarationEvent {
                property: &longhand,
                boundary: CascadeBoundary::Inline { important: false, normal_rank: 0 },
            },
        ];
        let selected = select_specified_values(&events, &mut PropertyTargetState::default());
        assert_eq!(selected.declarations.len(), 2);
        assert!(!selected.declarations[0].targets.iter().any(|target| &*target.name == "margin-left"));
        assert_eq!(&*selected.declarations[1].targets[0].name, "margin-left");
    }
}
