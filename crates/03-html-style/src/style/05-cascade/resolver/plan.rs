//! Cascade ordering for one style target.

use crate::style::compiled::{EffectiveRuleId, PreparedRuleSet};
use std::hash::Hash;

/// Matched rule with specificity for cascade sorting
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct MatchedRule {
    pub(super) specificity: u32,
    pub(super) id: EffectiveRuleId,
    pub(super) scope_proximity: u32,
}

/// One target's rules in normal and important cascade order.
#[derive(Default)]
pub(super) struct CascadePlan {
    pub(super) normal: Vec<MatchedRule>,
    pub(super) important: Vec<MatchedRule>,
}

impl CascadePlan {
    pub(super) fn build(&mut self, prepared: &PreparedRuleSet<'_>) {
        let Self { normal, important } = self;
        important.clear();
        important.extend(normal.iter().copied().filter(|matched| prepared.has_important(matched.id)));
        normal.sort_by_cached_key(|matched| {
            prepared.priority(matched.id).order_key(
                matched.specificity,
                matched.scope_proximity,
                false,
            )
        });
        important.sort_by_cached_key(|matched| {
            prepared.priority(matched.id).order_key(
                matched.specificity,
                matched.scope_proximity,
                true,
            )
        });
    }
}
