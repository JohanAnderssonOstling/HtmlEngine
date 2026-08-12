//! Reuse of candidate collection and context-free selector matches.
//!
//! Only compact single-compound tag, ID, class, and universal selectors enter
//! this cache. DOM-dependent selectors continue through the exact matcher.

use super::*;
use rustc_data_structures::fx::{FxHashMap, FxHashSet, FxHasher};
use std::hash::{Hash, Hasher};

const MAX_ENTRIES: usize = 512;
const MAX_OBSERVED_SIGNATURES: usize = 1024;

pub(super) struct LocalMatchSignature {
    hash: u64,
    representative: DomNodeId,
}

struct LocalMatchEntry {
    signature: LocalMatchSignature,
    candidates: Box<[EffectiveRuleId]>,
    specificities: Box<[Option<u32>]>,
}

#[derive(Default)]
pub(super) struct LocalMatchCache {
    buckets: FxHashMap<u64, Vec<LocalMatchEntry>>,
    observed_hashes: FxHashSet<u64>,
    entry_count: usize,
    #[cfg(test)]
    hit_count: usize,
}

pub(super) enum LocalMatchProbe {
    Hit,
    Miss(LocalMatchSignature),
    Uncached,
}

impl LocalMatchCache {
    pub(super) fn probe(
        &mut self,
        doc: &Document,
        node: DomNodeId,
        candidates: &mut Vec<EffectiveRuleId>,
        specificities: &mut Vec<Option<u32>>,
    ) -> LocalMatchProbe {
        let Some(element) = doc.element_ref(node) else {
            return LocalMatchProbe::Uncached;
        };
        let mut hasher = FxHasher::default();
        element.tag().hash(&mut hasher);
        element.id().hash(&mut hasher);
        element.is_html_element_in_html_document().hash(&mut hasher);
        for class in element.classes() {
            class.hash(&mut hasher);
        }
        let hash = hasher.finish();
        if let Some(entries) = self.buckets.get(&hash)
            && let Some(entry) = entries
                .iter()
                .find(|entry| same_element_signature(doc, entry.signature.representative, node))
        {
            candidates.clear();
            candidates.extend_from_slice(&entry.candidates);
            specificities.clear();
            specificities.extend_from_slice(&entry.specificities);
            #[cfg(test)]
            {
                self.hit_count += 1;
            }
            return LocalMatchProbe::Hit;
        }
        if self.entry_count >= MAX_ENTRIES {
            return LocalMatchProbe::Uncached;
        }
        if !self.observed_hashes.contains(&hash) {
            if self.observed_hashes.len() < MAX_OBSERVED_SIGNATURES {
                self.observed_hashes.insert(hash);
            }
            return LocalMatchProbe::Uncached;
        }
        LocalMatchProbe::Miss(LocalMatchSignature {
            hash,
            representative: node,
        })
    }

    pub(super) fn insert(
        &mut self,
        signature: LocalMatchSignature,
        candidates: &[EffectiveRuleId],
        specificities: &[Option<u32>],
    ) {
        debug_assert_eq!(candidates.len(), specificities.len());
        self.buckets
            .entry(signature.hash)
            .or_default()
            .push(LocalMatchEntry {
                signature,
                candidates: candidates.to_vec().into_boxed_slice(),
                specificities: specificities.to_vec().into_boxed_slice(),
            });
        self.entry_count += 1;
    }

    #[cfg(test)]
    pub(super) fn hit_count(&self) -> usize {
        self.hit_count
    }
}

fn same_element_signature(doc: &Document, left: DomNodeId, right: DomNodeId) -> bool {
    let Some(left) = doc.element_ref(left) else {
        return false;
    };
    let Some(right) = doc.element_ref(right) else {
        return false;
    };
    left.tag() == right.tag()
        && left.id() == right.id()
        && left.is_html_element_in_html_document() == right.is_html_element_in_html_document()
        && left.classes().eq(right.classes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_element_signatures_reuse_local_selector_results() {
        let doc = html_parse::parse_dom_document("<html><body><p class='same'></p><p class='same'></p><p class='different'></p></body></html>").unwrap();
        let nodes = doc
            .node_ids()
            .filter(|&node| doc.get_dom_tag(node) == Some("p"))
            .collect::<Vec<_>>();
        let mut cache = LocalMatchCache::default();
        let mut candidates = Vec::new();
        let mut specificities = Vec::new();
        assert!(matches!(
            cache.probe(&doc, nodes[0], &mut candidates, &mut specificities),
            LocalMatchProbe::Uncached
        ));
        let signature = match cache.probe(&doc, nodes[1], &mut candidates, &mut specificities) {
            LocalMatchProbe::Miss(signature) => signature,
            _ => panic!("the repeated local signature should be admitted"),
        };
        cache.insert(signature, &[], &[]);
        assert!(matches!(
            cache.probe(&doc, nodes[1], &mut candidates, &mut specificities),
            LocalMatchProbe::Hit
        ));
        assert_eq!(cache.hit_count(), 1);
        assert!(matches!(
            cache.probe(&doc, nodes[2], &mut candidates, &mut specificities),
            LocalMatchProbe::Uncached
        ));
    }
}
