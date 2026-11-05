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
pub(super) struct CascadePlan {
    pub(super) normal: Vec<MatchedRule>,
    pub(super) important: Vec<MatchedRule>,
}

impl CascadePlan {
    pub(super) fn build(
        mut normal: Vec<MatchedRule>,
        important_scratch: &mut Vec<MatchedRule>,
        prepared: &PreparedRuleSet<'_>,
    ) -> Self {
        // Preserve this buffer's capacity between elements. Cloning `normal`
        // allocated and copied every matching rule even when only a small
        // subset (usually none) carried important declarations.
        let mut important = std::mem::take(important_scratch);
        important.clear();
        important.extend(normal.iter().copied().filter(|matched| {
            !prepared
                .get(matched.id)
                .style_rule()
                .declarations
                .important_declarations
                .is_empty()
        }));
        normal.sort_by_key(|matched| {
            prepared.get(matched.id).priority().order_key(
                matched.specificity,
                matched.scope_proximity,
                false,
            )
        });
        important.sort_by_key(|matched| {
            prepared.get(matched.id).priority().order_key(
                matched.specificity,
                matched.scope_proximity,
                true,
            )
        });
        Self { normal, important }
    }
}
