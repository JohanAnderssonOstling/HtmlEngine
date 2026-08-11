//! Reuse of completed element styles after exact selector matching.
//!
//! The cache deliberately sits after matching: structural, stateful, scoped,
//! and relational selectors have already contributed their exact matched-rule
//! identity. DOM-dependent presentational hints stay on the uncached path.

use super::*;
use rustc_data_structures::fx::{FxHashMap, FxHashSet, FxHasher};
use std::hash::{Hash, Hasher};

const MAX_STYLE_SHARING_ENTRIES: usize = 512;
const MAX_OBSERVED_SIGNATURES: usize = 1024;

pub(super) struct SharedElementStyle {
    pub(super) style: StyleIndices,
    pub(super) custom_map_id: u32,
    pub(super) counters: CounterDirectives,
}

pub(super) struct StyleSharingSignature {
    hash: u64,
    parent_style: Option<StyleIndices>,
    parent_custom_map_id: u32,
    matched_rules: Box<[MatchedRule]>,
}

struct StyleSharingEntry {
    parent_style: Option<StyleIndices>,
    parent_custom_map_id: u32,
    matched_rules: Box<[MatchedRule]>,
    result: SharedElementStyle,
}

#[derive(Default)]
pub(super) struct StyleSharingCache {
    buckets: FxHashMap<u64, Vec<StyleSharingEntry>>,
    observed_hashes: FxHashSet<u64>,
    entry_count: usize,
    #[cfg(test)]
    hit_count: usize,
}

pub(super) enum StyleSharingProbe {
    Uncacheable,
    Hit(SharedElementStyle),
    Miss(StyleSharingSignature),
}

fn element_is_shareable(doc: &Document, node: DomNodeId, has_inline_style: bool) -> bool {
    if has_inline_style {
        return false;
    }
    let Some(element) = doc.element_ref(node) else {
        return false;
    };
    // Cell hints can inherit border and padding from an ancestor table, an
    // input not represented by the parent computed style.
    if element.tag().eq_ignore_ascii_case("td") || element.tag().eq_ignore_ascii_case("th") {
        return false;
    }
    // Selector-visible attributes are already represented by the exact
    // matched-rule list. Exclude only attributes consumed outside selector
    // matching by the current style pipeline.
    const OUT_OF_BAND_ATTRIBUTES: &[&str] = &[
        "style", "lang", "dir", "hidden", "text", "color", "size", "width", "height", "bgcolor", "align", "valign", "border", "bordercolor", "type", "clear", "hspace", "vspace", "marginwidth", "marginheight", "topmargin", "rightmargin", "bottommargin", "leftmargin", "cellspacing", "cellpadding", "nowrap", "noshade",
    ];
    !element.attributes().any(|attribute| OUT_OF_BAND_ATTRIBUTES.iter().any(|name| attribute.local_name().eq_ignore_ascii_case(name)))
}

impl StyleSharingCache {
    pub(super) fn probe(
        &mut self,
        doc: &Document,
        node: DomNodeId,
        has_inline_style: bool,
        parent_style: Option<StyleIndices>,
        parent_custom_map_id: u32,
        matched_rules: &[MatchedRule],
    ) -> StyleSharingProbe {
        if !element_is_shareable(doc, node, has_inline_style) {
            return StyleSharingProbe::Uncacheable;
        }
        let mut hasher = FxHasher::default();
        parent_style.hash(&mut hasher);
        parent_custom_map_id.hash(&mut hasher);
        matched_rules.hash(&mut hasher);
        let hash = hasher.finish();
        if let Some(entries) = self.buckets.get(&hash) {
            if let Some(entry) = entries.iter().find(|entry| {
                entry.parent_style == parent_style
                    && entry.parent_custom_map_id == parent_custom_map_id
                    && entry.matched_rules.as_ref() == matched_rules
            }) {
                #[cfg(test)]
                {
                    self.hit_count += 1;
                }
                return StyleSharingProbe::Hit(SharedElementStyle {
                    style: entry.result.style,
                    custom_map_id: entry.result.custom_map_id,
                    counters: entry.result.counters.clone(),
                });
            }
        }
        if self.entry_count >= MAX_STYLE_SHARING_ENTRIES {
            return StyleSharingProbe::Uncacheable;
        }
        // Do not retain a full, collision-checked rule sequence until its hash
        // repeats. One-off element styles remain allocation-light; a repeated
        // signature is still compared exactly before any completed style is
        // reused.
        if !self.observed_hashes.contains(&hash) {
            if self.observed_hashes.len() < MAX_OBSERVED_SIGNATURES {
                self.observed_hashes.insert(hash);
            }
            return StyleSharingProbe::Uncacheable;
        }
        StyleSharingProbe::Miss(StyleSharingSignature {
            hash,
            parent_style,
            parent_custom_map_id,
            matched_rules: matched_rules.to_vec().into_boxed_slice(),
        })
    }

    pub(super) fn insert(
        &mut self,
        signature: StyleSharingSignature,
        style: StyleIndices,
        custom_map_id: u32,
        counters: CounterDirectives,
    ) {
        self.buckets.entry(signature.hash).or_default().push(StyleSharingEntry {
            parent_style: signature.parent_style,
            parent_custom_map_id: signature.parent_custom_map_id,
            matched_rules: signature.matched_rules,
            result: SharedElementStyle { style, custom_map_id, counters },
        });
        self.entry_count += 1;
    }

    #[cfg(test)]
    pub(super) fn hit_count(&self) -> usize {
        self.hit_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_rule_and_parent_inputs_share_only_hint_free_elements() {
        let doc = html_parse::parse_dom_document(
            "<!doctype html><html><body><p class='same'></p><p id='other' class='same'></p><p class='same' align='right'></p><p class='same' data-index='different'></p><table><tr><td class='same'></td></tr></table></body></html>",
        )
        .unwrap();
        let paragraphs = doc.node_ids().filter(|&node| doc.get_dom_tag(node) == Some("p")).collect::<Vec<_>>();
        let cell = doc.node_ids().find(|&node| doc.get_dom_tag(node) == Some("td")).unwrap();
        let styles = ComputedStylesBuilder::new(&doc);
        let style = styles.default_indices();
        let mut cache = StyleSharingCache::default();

        assert!(matches!(cache.probe(&doc, paragraphs[0], false, Some(style), 0, &[]), StyleSharingProbe::Uncacheable));
        let signature = match cache.probe(&doc, paragraphs[1], false, Some(style), 0, &[]) {
            StyleSharingProbe::Miss(signature) => signature,
            _ => panic!("a repeated hash must request an exact signature"),
        };
        cache.insert(signature, style, 0, CounterDirectives::default());
        assert!(matches!(cache.probe(&doc, paragraphs[1], true, Some(style), 0, &[]), StyleSharingProbe::Uncacheable));
        assert!(matches!(cache.probe(&doc, paragraphs[2], false, Some(style), 0, &[]), StyleSharingProbe::Uncacheable));
        assert!(matches!(cache.probe(&doc, paragraphs[3], false, Some(style), 0, &[]), StyleSharingProbe::Hit(_)));
        assert_eq!(cache.hit_count(), 1);
        assert!(matches!(cache.probe(&doc, cell, false, Some(style), 0, &[]), StyleSharingProbe::Uncacheable));
    }
}
