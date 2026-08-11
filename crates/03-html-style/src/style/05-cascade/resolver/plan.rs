//! Cascade ordering and rollback boundaries for one style target.

use super::*;

/// Matched rule with specificity for cascade sorting
#[derive(Clone, Copy)]
pub(super) struct MatchedRule {
    pub(super) specificity: u32,
    pub(super) id: EffectiveRuleId,
    pub(super) scope_proximity: u32,
}

/// One target's rules in both cascade orders, together with the rollback
/// boundaries those rules require. Elements and pseudo-elements build the
/// same plan before their declarations are applied.
pub(super) struct CascadePlan {
    pub(super) normal: Vec<MatchedRule>,
    pub(super) important: Vec<MatchedRule>,
    pub(super) rollback_layers: Vec<RulePriority>,
    pub(super) rollback_origins: Vec<CascadeOrigin>,
}

impl CascadePlan {
    pub(super) fn build(mut normal: Vec<MatchedRule>, prepared: &PreparedRuleSet<'_, '_>) -> Self {
        let mut important = normal.clone();
        normal.sort_by(|a, b| {
            prepared.get(a.id).priority().compare_normal(
                a.specificity,
                a.scope_proximity,
                prepared.get(b.id).priority(),
                b.specificity,
                b.scope_proximity,
            )
        });
        important.sort_by(|a, b| {
            prepared.get(a.id).priority().compare_important(
                a.specificity,
                a.scope_proximity,
                prepared.get(b.id).priority(),
                b.specificity,
                b.scope_proximity,
            )
        });
        let rollback_layers = rollback_layers(&normal, prepared);
        let rollback_origins = rollback_origins(&normal, prepared);
        Self {
            normal,
            important,
            rollback_layers,
            rollback_origins,
        }
    }
}

pub(super) fn layer_baseline<'a>(
    baselines: &'a [(RulePriority, WorkingStyle)],
    priority: RulePriority,
) -> Option<&'a WorkingStyle> {
    baselines
        .iter()
        .find_map(|(candidate, style)| candidate.same_origin_and_layer(priority).then_some(style))
}

pub(super) fn origin_baseline<'a>(
    baselines: &'a [(CascadeOrigin, WorkingStyle)],
    origin: CascadeOrigin,
) -> Option<&'a WorkingStyle> {
    baselines
        .iter()
        .find_map(|(candidate, style)| (*candidate == origin).then_some(style))
}

pub(super) fn declarations_use_rollback_keyword(
    declarations: &[Property<'_>],
    expected: CSSWideKeyword,
    keyword: &str,
) -> bool {
    declarations.iter().any(|property| match property {
        Property::All(value) => *value == expected,
        Property::Unparsed(unparsed) => single_ident_keyword(&unparsed.value)
            .is_some_and(|value| value.eq_ignore_ascii_case(keyword)),
        Property::Custom(custom) => single_ident_keyword(&custom.value)
            .is_some_and(|value| value.eq_ignore_ascii_case(keyword)),
        _ => false,
    })
}

pub(super) fn declarations_use_revert(declarations: &[Property<'_>]) -> bool {
    declarations_use_rollback_keyword(declarations, CSSWideKeyword::Revert, "revert")
}

pub(super) fn declarations_use_revert_layer(declarations: &[Property<'_>]) -> bool {
    declarations_use_rollback_keyword(declarations, CSSWideKeyword::RevertLayer, "revert-layer")
}

pub(super) fn rollback_layers(
    matched_rules: &[MatchedRule],
    prepared: &PreparedRuleSet<'_, '_>,
) -> Vec<RulePriority> {
    let mut layers = Vec::new();
    for matched in matched_rules {
        let rule = prepared.get(matched.id);
        if (declarations_use_revert_layer(&rule.style_rule().declarations.declarations)
            || declarations_use_revert_layer(
                &rule.style_rule().declarations.important_declarations,
            ))
            && !layers
                .iter()
                .any(|existing: &RulePriority| existing.same_origin_and_layer(rule.priority()))
        {
            layers.push(rule.priority());
        }
    }
    layers
}

pub(super) fn layer_needs_rollback(layers: &[RulePriority], priority: RulePriority) -> bool {
    layers
        .iter()
        .any(|candidate| candidate.same_origin_and_layer(priority))
}

pub(super) fn rollback_origins(
    matched_rules: &[MatchedRule],
    prepared: &PreparedRuleSet<'_, '_>,
) -> Vec<CascadeOrigin> {
    let mut origins = Vec::new();
    for matched in matched_rules {
        let rule = prepared.get(matched.id);
        if (declarations_use_revert(&rule.style_rule().declarations.declarations)
            || declarations_use_revert(&rule.style_rule().declarations.important_declarations))
            && !origins.contains(&rule.priority().origin())
        {
            origins.push(rule.priority().origin());
        }
    }
    origins
}

pub(super) fn origin_needs_rollback(origins: &[CascadeOrigin], origin: CascadeOrigin) -> bool {
    origins.contains(&origin)
}
