//! Reuse of candidates, local matches and eligible completed sibling styles.
//!
//! Compact single-compound tag, ID, class, universal and simple negation
//! results are cached independently of DOM context. Early style sharing can
//! also reuse ancestor matches for identical siblings with safe candidates.

use super::cache_index::CacheIndex;
use super::style_sharing::{SharedElementStyle, element_is_shareable};
use crate::style::compiled::{CompiledSelectors, SelectorCandidate, SelectorIndex};
use crate::style::matching::dom::PseudoTarget;
use html_dom::{Document, DomNodeId, ElementRef};
use html_style_model::{CounterDirectives, StyleIndices};
use rustc_data_structures::fx::FxHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

pub(super) struct LocalMatchSignature {
    hash: u64,
    representative: DomNodeId,
}

struct LocalMatchEntry {
    signature: LocalMatchSignature,
    candidates: Range<usize>,
    next: Option<usize>,
    // One local maximum per contiguous rule group in candidates.
    specificities: Range<usize>,
    pseudo_candidates: PseudoCandidates,
    // Eligibility is checked once per cached candidate list. Completed styles
    // are reused only after exact sibling input checks, before matching.
    early_shareable: Option<bool>,
    early_attributes: Box<[String]>,
    early_style: Option<(DomNodeId, SharedElementStyle)>,
}

/// Selector candidates grouped by pseudo target, prepared once per local
/// signature. Empty groups retain no allocation.
#[derive(Clone, Default)]
pub(super) struct PseudoCandidates {
    candidates: [Vec<SelectorCandidate>; 4],
    mask: u8,
}

impl PseudoCandidates {
    pub(super) fn prepare(
        &mut self,
        candidates: &[SelectorCandidate],
        compiled: &CompiledSelectors,
    ) {
        self.mask = 0;
        for group in &mut self.candidates {
            group.clear();
        }
        for &candidate in candidates {
            let selector = compiled.for_rule(candidate.rule)[candidate.selector as usize];
            if !selector.has_pseudo_target() {
                continue;
            }
            for (index, group) in self.candidates.iter_mut().enumerate() {
                let mask = 1 << index;
                if selector.targets_pseudo(mask) {
                    self.mask |= mask;
                    group.push(candidate);
                }
            }
        }
    }

    pub(super) fn mask(&self) -> u8 {
        self.mask
    }

    pub(super) fn for_target(&self, target: PseudoTarget) -> &[SelectorCandidate] {
        &self.candidates[target.mask().trailing_zeros() as usize]
    }
}

#[derive(Clone, Copy, Default)]
struct CandidateDependencies {
    attributes: bool,
    namespace: bool,
    state: bool,
    root: bool,
}

#[derive(Default)]
pub(super) struct LocalMatchCache {
    candidate_dependencies: CandidateDependencies,
    index: CacheIndex,
    entries: Vec<LocalMatchEntry>,
    candidates: Vec<SelectorCandidate>,
    specificities: Vec<Option<u32>>,
    last_entry: Option<usize>,
    #[cfg(test)]
    hit_count: usize,
}

pub(super) enum LocalMatchProbe {
    Hit,
    Miss(LocalMatchSignature),
    Uncached,
}

impl LocalMatchCache {
    pub(super) fn configure_candidates(&mut self, index: &SelectorIndex) {
        self.candidate_dependencies = CandidateDependencies {
            attributes: !index.by_attribute.is_empty(),
            namespace: !index.by_namespace.is_empty(),
            state: !index.by_state.is_empty(),
            root: index.root.len != 0,
        };
    }

    pub(super) fn probe(&mut self, doc: &Document, element: ElementRef<'_>) -> LocalMatchProbe {
        self.last_entry = None;
        let node = element.node_id();
        let mut hasher = FxHasher::default();
        element.tag().hash(&mut hasher);
        element.id().hash(&mut hasher);
        element.is_html_element_in_html_document().hash(&mut hasher);
        for class in element.classes() {
            class.hash(&mut hasher);
        }
        let dependencies = self.candidate_dependencies;
        if dependencies.attributes {
            for attribute in element.attributes() {
                attribute.local_name().hash(&mut hasher);
            }
        }
        if dependencies.namespace {
            element.namespace().hash(&mut hasher);
        }
        if dependencies.state {
            candidate_states(element).hash(&mut hasher);
        }
        if dependencies.root {
            (doc.dom_root() == Some(node)).hash(&mut hasher);
        }
        let hash = hasher.finish();
        let mut current = self.index.first(hash);
        while let Some(index) = current {
            let entry = &self.entries[index];
            if same_element_signature(doc, entry.signature.representative, element, dependencies) {
                self.last_entry = Some(index);
                #[cfg(test)]
                {
                    self.hit_count += 1;
                }
                return LocalMatchProbe::Hit;
            }
            current = entry.next;
        }
        if !self.index.admit(hash, self.entries.len()) {
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
        candidates: &[SelectorCandidate],
        specificities: &[Option<u32>],
        pseudo_candidates: &PseudoCandidates,
    ) {
        debug_assert_eq!(
            candidates.chunk_by(|a, b| a.rule == b.rule).count(),
            specificities.len()
        );
        let hash = signature.hash;
        let index = self.entries.len();
        let next = self.index.insert(hash, index);
        let candidate_start = self.candidates.len();
        let specificity_start = self.specificities.len();
        self.candidates.extend_from_slice(candidates);
        self.specificities.extend_from_slice(specificities);
        self.entries.push(LocalMatchEntry {
            signature,
            candidates: candidate_start..self.candidates.len(),
            specificities: specificity_start..self.specificities.len(),
            pseudo_candidates: pseudo_candidates.clone(),
            next,
            early_shareable: None,
            early_attributes: Box::default(),
            early_style: None,
        });
        self.last_entry = Some(index);
    }

    pub(super) fn candidates(&self) -> Option<&[SelectorCandidate]> {
        Some(&self.candidates[self.entries[self.last_entry?].candidates.clone()])
    }

    pub(super) fn specificities(&self) -> Option<&[Option<u32>]> {
        Some(&self.specificities[self.entries[self.last_entry?].specificities.clone()])
    }

    pub(super) fn pseudo_candidates(&self) -> Option<&PseudoCandidates> {
        Some(&self.entries[self.last_entry?].pseudo_candidates)
    }

    /// Call immediately after a successful local probe. The immutable document
    /// and actual shared parent fix ancestor matching and inherited inputs;
    /// selector-visible attributes and normalized HTML state are checked here.
    pub(super) fn early_style(
        &self,
        doc: &Document,
        element: ElementRef<'_>,
        has_element_declarations: bool,
    ) -> Option<SharedElementStyle> {
        let entry = &self.entries[self.last_entry?];
        let (representative, style) = entry.early_style.as_ref()?;
        // Attribute differences can now be ignored, so check the subject for
        // presentational hints too, after ruling out cache entries without a style.
        if !element_is_shareable(element, has_element_declarations) {
            return None;
        }
        let representative = doc.try_element_ref(*representative)?;
        if representative.parent().is_some()
            && representative.parent() == element.parent()
            && representative.namespace() == element.namespace()
            && representative.html_state() == element.html_state()
            && representative
                .attributes()
                .filter(|attr| {
                    entry.early_attributes.iter().any(|name| {
                        attr.local_name().eq_ignore_ascii_case(name)
                            || attr.name().eq_ignore_ascii_case(name)
                    })
                })
                .map(|attr| {
                    (
                        attr.namespace(),
                        attr.name(),
                        attr.local_name(),
                        attr.value(),
                    )
                })
                .eq(element
                    .attributes()
                    .filter(|attr| {
                        entry.early_attributes.iter().any(|name| {
                            attr.local_name().eq_ignore_ascii_case(name)
                                || attr.name().eq_ignore_ascii_case(name)
                        })
                    })
                    .map(|attr| {
                        (
                            attr.namespace(),
                            attr.name(),
                            attr.local_name(),
                            attr.value(),
                        )
                    }))
        {
            Some(style.clone())
        } else {
            None
        }
    }

    pub(super) fn remember_style(
        &mut self,
        element: ElementRef<'_>,
        has_element_declarations: bool,
        compiled: &CompiledSelectors,
        style: StyleIndices,
        custom_map_id: u32,
        counters: &CounterDirectives,
    ) {
        let Some(entry_index) = self.last_entry else {
            return;
        };
        let entry = &mut self.entries[entry_index];
        if entry.early_shareable == Some(false) {
            return;
        }
        if !element_is_shareable(element, has_element_declarations) {
            return;
        }
        if entry.early_shareable.is_none() {
            let mut attributes = Vec::new();
            let eligible = self.candidates[entry.candidates.clone()]
                .iter()
                .all(|candidate| {
                    let selector = compiled.for_rule(candidate.rule)[candidate.selector as usize];
                    let Some((start, end)) = selector.sibling_attributes else {
                        return false;
                    };
                    attributes.extend_from_slice(
                        &compiled.sibling_attributes[start as usize..end as usize],
                    );
                    true
                });
            attributes.sort_unstable();
            attributes.dedup();
            entry.early_attributes = attributes.into_boxed_slice();
            entry.early_shareable = Some(eligible);
        }
        if entry.early_shareable == Some(true) {
            entry.early_style = Some((
                element.node_id(),
                SharedElementStyle {
                    style,
                    custom_map_id,
                    counters: counters.clone(),
                },
            ));
        }
    }

    #[cfg(test)]
    pub(super) fn hit_count(&self) -> usize {
        self.hit_count
    }
}

fn candidate_states(element: ElementRef<'_>) -> (bool, bool, bool, bool) {
    let state = element.html_state();
    (
        state.is_link(),
        state.is_enabled(),
        state.is_disabled(),
        state.is_checked(),
    )
}

fn same_element_signature(
    doc: &Document,
    left: DomNodeId,
    right: ElementRef<'_>,
    dependencies: CandidateDependencies,
) -> bool {
    let Some(left) = doc.try_element_ref(left) else {
        return false;
    };
    left.tag() == right.tag()
        && left.id() == right.id()
        && left.is_html_element_in_html_document()
            == right.is_html_element_in_html_document()
        && left.classes().eq(right.classes())
        && (!dependencies.attributes
            || left
                .attributes()
                .map(|attr| attr.local_name())
                .eq(right.attributes().map(|attr| attr.local_name())))
        && (!dependencies.namespace || left.namespace() == right.namespace())
        && (!dependencies.state || candidate_states(left) == candidate_states(right))
        && (!dependencies.root
            || (doc.dom_root() == Some(left.node_id()))
                == (doc.dom_root() == Some(right.node_id())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_element_signatures_reuse_local_selector_results() {
        let doc = html_parse::parse_dom_document("<html><body><p class='same'></p><p class='same'></p><p class='different'></p></body></html>").unwrap();
        let elements = doc
            .node_ids()
            .filter_map(|node| doc.try_element_ref(node))
            .filter(|element| element.tag() == "p")
            .collect::<Vec<_>>();
        let mut cache = LocalMatchCache::default();
        assert!(matches!(
            cache.probe(&doc, elements[0]),
            LocalMatchProbe::Uncached
        ));
        let signature = match cache.probe(&doc, elements[1]) {
            LocalMatchProbe::Miss(signature) => signature,
            _ => panic!("the repeated local signature should be admitted"),
        };
        cache.insert(signature, &[], &[], &PseudoCandidates::default());
        assert!(matches!(
            cache.probe(&doc, elements[1]),
            LocalMatchProbe::Hit
        ));
        assert_eq!(cache.hit_count(), 1);
        assert!(matches!(
            cache.probe(&doc, elements[2]),
            LocalMatchProbe::Uncached
        ));
    }
}
