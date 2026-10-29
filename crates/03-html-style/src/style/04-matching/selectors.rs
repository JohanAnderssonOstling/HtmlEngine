//! Runtime selector candidate collection and ancestor filtering.
use crate::style::compiled::{EffectiveRuleId, PreparedSelector, SelectorIndex};
use crate::style::syntax::selectors::ancestor_hash;

// ============================================================================
// Bloom Filter for fast ancestor rejection
// ============================================================================

#[derive(Clone, Copy, Default)]
pub struct AncestorFilter {
    bits: u64,
}

impl AncestorFilter {
    #[inline]
    pub fn insert(&mut self, s: &str) {
        self.bits |= ancestor_hash(s);
    }
}

// ============================================================================
// Selector Index for O(1) candidate lookup
// ============================================================================

/// Dense duplicate suppression for candidate rule IDs. Prepared IDs are
/// contiguous, so advancing an epoch is cheaper than clearing a hash table.
pub(crate) struct CandidateDeduper {
    generations: Vec<u32>,
    generation: u32,
}

impl CandidateDeduper {
    pub(crate) fn new(rule_count: usize) -> Self {
        Self { generations: vec![0; rule_count], generation: 0 }
    }

    fn begin_element(&mut self) {
        if self.generation == u32::MAX {
            self.generations.fill(0);
            self.generation = 1;
        } else {
            self.generation += 1;
        }
    }

    #[inline]
    fn append(&mut self, rules: &[EffectiveRuleId], candidates: &mut Vec<EffectiveRuleId>) {
        for &id in rules {
            let generation = &mut self.generations[id.index()];
            if *generation != self.generation {
                *generation = self.generation;
                candidates.push(id);
            }
        }
    }
}

impl SelectorIndex {
    #[inline]
    pub(crate) fn selector_might_match(&self, selector: PreparedSelector, ancestor_filter: &AncestorFilter) -> bool {
        ancestor_filter.bits & selector.ancestor_requirements == selector.ancestor_requirements
    }

    /// Get candidate rule indices for an element
    pub fn collect_candidates<'a>(&self, tag: &str, id: Option<&str>, classes: impl IntoIterator<Item = &'a str>, candidates: &mut Vec<EffectiveRuleId>, seen: &mut CandidateDeduper) {
        candidates.clear();
        seen.begin_element();

        // Add by tag
        if let Some(indices) = self.by_tag.get(tag) {
            seen.append(indices, candidates);
        }

        // Add by id
        if let Some(id) = id
            && let Some(indices) = self.by_id.get(id)
        {
            seen.append(indices, candidates);
        }

        // Add by class
        for class in classes {
            if let Some(indices) = self.by_class.get(class) {
                seen.append(indices, candidates);
            }
        }

        // Always include universal selectors
        seen.append(&self.universal, candidates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_collection_preserves_bucket_order_and_resets_duplicate_tracking() {
        let mut index = SelectorIndex::new();
        index
            .by_tag
            .insert("p".into(), vec![EffectiveRuleId(1), EffectiveRuleId(2)]);
        index.by_id.insert(
            "chosen".into(),
            vec![EffectiveRuleId(2), EffectiveRuleId(3)],
        );
        index
            .by_class
            .insert("a".into(), vec![EffectiveRuleId(3), EffectiveRuleId(4)]);
        index
            .by_class
            .insert("b".into(), vec![EffectiveRuleId(4), EffectiveRuleId(5)]);
        index.universal = vec![EffectiveRuleId(5), EffectiveRuleId(0)];
        let mut seen = CandidateDeduper::new(6);
        let mut candidates = vec![EffectiveRuleId(5)];
        for generation in [0, u32::MAX] {
            seen.generation = generation;
            index.collect_candidates(
                "p",
                Some("chosen"),
                ["a", "b", "a"],
                &mut candidates,
                &mut seen,
            );
            assert_eq!(candidates, [1, 2, 3, 4, 5, 0].map(EffectiveRuleId));
            index.collect_candidates("unknown", None, [], &mut candidates, &mut seen);
            assert_eq!(candidates, [EffectiveRuleId(5), EffectiveRuleId(0)]);
        }
    }
}
