//! Cascade ordering for one style target.

use super::*;

/// Matched rule with specificity for cascade sorting
#[derive(Clone, Copy)]
pub(super) struct MatchedRule {
    pub(super) specificity: u32,
    pub(super) id: EffectiveRuleId,
    pub(super) scope_proximity: u32,
}

/// One target's rules in normal and important cascade order.
pub(super) struct CascadePlan {
    pub(super) normal: Vec<MatchedRule>,
    pub(super) important: Vec<MatchedRule>,
}

impl CascadePlan {
    pub(super) fn build(mut normal: Vec<MatchedRule>, prepared: &PreparedRuleSet<'_, '_>) -> Self {
        let mut important = normal.clone();
        important.retain(|matched| {
            !prepared
                .get(matched.id)
                .style_rule()
                .declarations
                .important_declarations
                .is_empty()
        });
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
        Self { normal, important }
    }
}
