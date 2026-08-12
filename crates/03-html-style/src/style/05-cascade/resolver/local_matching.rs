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
    tag: String,
    id: Option<String>,
    classes: Box<[String]>,
    html_element: bool,
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
    pub(super) fn probe(&mut self, doc: &Document, node: DomNodeId, candidates: &mut Vec<EffectiveRuleId>, specificities: &mut Vec<Option<u32>>) -> LocalMatchProbe {
        let Some(element) = doc.element_ref(node) else {
            return LocalMatchProbe::Uncached;
        };
        let tag = element.tag();
        let id = element.id();
        let html_element = element.is_html_element_in_html_document();
        let mut hasher = FxHasher::default();
        tag.hash(&mut hasher);
        id.hash(&mut hasher);
        html_element.hash(&mut hasher);
        for class in element.classes() {
            class.hash(&mut hasher);
        }
        let hash = hasher.finish();
        if let Some(entries) = self.buckets.get(&hash)
            && let Some(entry) = entries.iter().find(|entry| {
                entry.signature.tag == tag
                    && entry.signature.id.as_deref() == id
                    && entry.signature.html_element == html_element
                    && entry.signature.classes.iter().map(String::as_str).eq(element.classes())
            })
        {
            candidates.clear();
            candidates.extend_from_slice(&entry.candidates);
            specificities.clear();
            specificities.extend_from_slice(&entry.specificities);
            #[cfg(test)]
            { self.hit_count += 1; }
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
            tag: tag.to_owned(),
            id: id.map(str::to_owned),
            classes: element.classes().map(str::to_owned).collect(),
            html_element,
        })
    }

    pub(super) fn insert(&mut self, signature: LocalMatchSignature, candidates: &[EffectiveRuleId], specificities: &[Option<u32>]) {
        debug_assert_eq!(candidates.len(), specificities.len());
        self.buckets.entry(signature.hash).or_default().push(LocalMatchEntry {
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
