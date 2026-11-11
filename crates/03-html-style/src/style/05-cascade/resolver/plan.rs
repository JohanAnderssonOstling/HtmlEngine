//! Cascade ordering for one style target.

use crate::style::compiled::{EffectiveRuleId, PreparedRuleSet};
use crate::style::matching::dom::PseudoTarget;
use html_dom::DomNodeId;
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

/// The target key and its ordered rule buffers form one cache entry.
#[derive(Default)]
pub(super) struct CachedRuleMatches {
    pseudo: Option<(DomNodeId, PseudoTarget)>,
    plan: CascadePlan,
}

impl CachedRuleMatches {
    pub(super) fn begin_element(&mut self) -> &mut Vec<MatchedRule> {
        self.pseudo = None;
        self.plan.normal.clear();
        &mut self.plan.normal
    }

    pub(super) fn begin_pseudo(
        &mut self,
        node: DomNodeId,
        pseudo: PseudoTarget,
    ) -> (bool, &mut Vec<MatchedRule>) {
        let reuse = self.pseudo == Some((node, pseudo));
        if !reuse {
            self.plan.normal.clear();
            self.plan.important.clear();
            self.pseudo = Some((node, pseudo));
        }
        (reuse, &mut self.plan.normal)
    }

    pub(super) fn order(&mut self, prepared: &PreparedRuleSet<'_>) {
        self.plan.build(prepared);
    }

    pub(super) fn plan(&self) -> &CascadePlan {
        &self.plan
    }
}
