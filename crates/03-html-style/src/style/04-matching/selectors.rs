//! Runtime selector candidate collection and ancestor filtering.
pub use super::filter::SelectorFilter as AncestorFilter;
use crate::style::compiled::{
    CandidateRange, PreparedSelector, SelectorCandidate, SelectorIndex, SelectorState};
use html_dom::{ElementRef};

// ============================================================================
// Selector candidate lookup
// ============================================================================

impl SelectorIndex {
    #[inline]
    pub(crate) fn selector_might_match(
        &self,
        selector: PreparedSelector,
        ancestor_filter: &AncestorFilter,
    ) -> bool {
        ancestor_filter.contains(selector.ancestor_requirements)
    }

    /// Merge the sorted target-key buckets without allocating merge state.
    /// Unusually many buckets retain the general sorting path.
    pub(crate) fn collect_candidates<'index, 'a>(
        &'index self,
        tag: &str,
        id: Option<&str>,
        classes: impl IntoIterator<Item = &'a str>,
        candidates: &mut Vec<SelectorCandidate>,
    ) {
        self.collect_candidates_inner(tag, id, classes, std::iter::empty(), candidates);
    }

    pub(crate) fn collect_element_candidates(
        &self,
        element: ElementRef<'_>,
        is_root: bool,
        candidates: &mut Vec<SelectorCandidate>,
    ) {
        if self.by_attribute.is_empty()
            && self.by_namespace.is_empty()
            && self.by_state.is_empty()
            && self.root.len == 0
        {
            self.collect_candidates(element.tag(), element.id(), element.classes(), candidates);
            return;
        }
        let attributes = element.attributes().filter_map(|attribute| {
            let name = attribute.local_name();
            self.by_attribute.get(name).copied().or_else(|| {
                (element.is_html_element_in_html_document()
                    && name.bytes().any(|byte| byte.is_ascii_uppercase()))
                .then(|| self.by_attribute.get(&name.to_ascii_lowercase()).copied())
                .flatten()
            })
        });
        let namespace = self
            .by_namespace
            .get(element.namespace().unwrap_or(""))
            .copied();
        let root = is_root.then_some(self.root);
        let states = self.by_state.iter().filter_map(|(state, range)| {
            let flags = element.html_state();
            let matched = match state {
                SelectorState::Link => flags.is_link(),
                SelectorState::Enabled => flags.is_enabled(),
                SelectorState::Disabled => flags.is_disabled(),
                SelectorState::Checked => flags.is_checked(),
            };
            matched.then_some(*range)
        });
        self.collect_candidates_inner(
            element.tag(),
            element.id(),
            element.classes(),
            attributes.chain(namespace).chain(root).chain(states),
            candidates,
        );
    }

    fn collect_candidates_inner<'index, 'a>(
        &'index self,
        tag: &str,
        id: Option<&str>,
        classes: impl IntoIterator<Item = &'a str>,
        extra: impl IntoIterator<Item = CandidateRange>,
        candidates: &mut Vec<SelectorCandidate>,
    ) {
        candidates.clear();
        let mut buckets: [&[SelectorCandidate]; 8] = [&[]; 8];
        let mut bucket_count = 0;
        let mut overflow = false;
        let mut add = |bucket: &'index [SelectorCandidate]| {
            if bucket.is_empty() {
                return;
            }
            if overflow {
                candidates.extend_from_slice(bucket);
            } else if bucket_count < buckets.len() {
                buckets[bucket_count] = bucket;
                bucket_count += 1;
            } else {
                for stored in &buckets {
                    candidates.extend_from_slice(stored);
                }
                candidates.extend_from_slice(bucket);
                overflow = true;
            }
        };

        // Add by tag
        if let Some(indices) = self.by_tag.get(tag) {
            add(self.bucket(*indices));
        }

        // Add by id
        if let Some(id) = id
            && let Some(indices) = self.by_id.get(id)
        {
            add(self.bucket(*indices));
        }

        // Add by class
        for class in classes {
            if let Some(indices) = self.by_class.get(class) {
                add(self.bucket(*indices));
            }
        }

        for range in extra {
            add(self.bucket(range));
        }

        // Always include universal selectors
        add(self.bucket(self.universal));
        if overflow {
            candidates
                .sort_unstable_by_key(|candidate| (candidate.rule.index(), candidate.selector));
        } else {
            while bucket_count > 1 {
                let smallest = (1..bucket_count).fold(0, |best, next| {
                    let key =
                        |slot: usize| (buckets[slot][0].rule.index(), buckets[slot][0].selector);
                    if key(next) < key(best) { next } else { best }
                });
                candidates.push(buckets[smallest][0]);
                buckets[smallest] = &buckets[smallest][1..];
                if buckets[smallest].is_empty() {
                    bucket_count -= 1;
                    buckets[smallest] = buckets[bucket_count];
                }
            }
            if bucket_count == 1 {
                candidates.extend_from_slice(buckets[0]);
            }
        }
        // Duplicate class tokens can revisit the same bucket.
        candidates.dedup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::compiled::{CompiledSelectors, EffectiveRuleId, SelectorBuckets};

    #[test]
    fn candidate_collection_groups_selectors_by_rule_and_removes_duplicates() {
        let mut index = SelectorBuckets::default();
        index.by_tag.insert(
            "p".into(),
            vec![
                SelectorCandidate {
                    rule: EffectiveRuleId(1),
                    selector: 0,
                },
                SelectorCandidate {
                    rule: EffectiveRuleId(2),
                    selector: 0,
                },
            ],
        );
        index.by_id.insert(
            "chosen".into(),
            vec![
                SelectorCandidate {
                    rule: EffectiveRuleId(2),
                    selector: 0,
                },
                SelectorCandidate {
                    rule: EffectiveRuleId(3),
                    selector: 0,
                },
            ],
        );
        index.by_class.insert(
            "a".into(),
            vec![
                SelectorCandidate {
                    rule: EffectiveRuleId(3),
                    selector: 0,
                },
                SelectorCandidate {
                    rule: EffectiveRuleId(4),
                    selector: 0,
                },
            ],
        );
        index.by_class.insert(
            "b".into(),
            vec![
                SelectorCandidate {
                    rule: EffectiveRuleId(4),
                    selector: 0,
                },
                SelectorCandidate {
                    rule: EffectiveRuleId(5),
                    selector: 0,
                },
            ],
        );
        index.universal = vec![
            SelectorCandidate {
                rule: EffectiveRuleId(0),
                selector: 0,
            },
            SelectorCandidate {
                rule: EffectiveRuleId(5),
                selector: 0,
            },
        ];
        let index = index.finish(CompiledSelectors::empty());
        let mut candidates = vec![SelectorCandidate {
            rule: EffectiveRuleId(5),
            selector: 0,
        }];
        for _ in 0..2 {
            index.collect_candidates("p", Some("chosen"), ["a", "b", "a"], &mut candidates);
            assert_eq!(
                candidates,
                [0, 1, 2, 3, 4, 5].map(|rule| SelectorCandidate {
                    rule: EffectiveRuleId(rule),
                    selector: 0
                })
            );
            index.collect_candidates("unknown", None, [], &mut candidates);
            assert_eq!(
                candidates,
                [
                    SelectorCandidate {
                        rule: EffectiveRuleId(0),
                        selector: 0
                    },
                    SelectorCandidate {
                        rule: EffectiveRuleId(5),
                        selector: 0
                    }
                ]
            );
        }
    }
}
