//! Reuse of completed element styles after exact selector matching.
//!
//! The cache deliberately sits after matching: structural, stateful, scoped,
//! and relational selectors have already contributed their exact matched-rule
//! identity. DOM-dependent presentational hints stay on the uncached path.

use super::cache_index::CacheIndex;
use super::plan::MatchedRule;
use html_dom::ElementRef;
use html_style_model::{CounterDirectives, StyleIndices};
use rustc_data_structures::fx::FxHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

#[derive(Clone)]
pub(super) struct SharedElementStyle {
    pub(super) style: StyleIndices,
    pub(super) custom_map_id: u32,
    pub(super) counters: CounterDirectives,
}

pub(super) struct StyleSharingSignature {
    hash: u64,
    parent_style: Option<StyleIndices>,
    parent_custom_map_id: u32,
    matched_rules: Range<usize>,
}

struct StyleSharingEntry {
    next: Option<usize>,
    parent_style: Option<StyleIndices>,
    parent_custom_map_id: u32,
    matched_rules: Range<usize>,
    result: SharedElementStyle,
}

#[derive(Default)]
pub(super) struct StyleSharingCache {
    index: CacheIndex,
    entries: Vec<StyleSharingEntry>,
    matched_rules: Vec<MatchedRule>,
    #[cfg(test)]
    hit_count: usize,
}

pub(super) enum StyleSharingProbe {
    Uncacheable,
    Hit(SharedElementStyle),
    Miss(StyleSharingSignature),
}

pub(super) fn element_is_shareable(element: ElementRef<'_>, has_inline_style: bool) -> bool {
    if has_inline_style {
        return false;
    }
    // Cell hints can inherit border and padding from an ancestor table, an
    // input not represented by the parent computed style.
    if element.tag().eq_ignore_ascii_case("td") || element.tag().eq_ignore_ascii_case("th") {
        return false;
    }
    // Selector-visible attributes are already represented by the exact
    // matched-rule list. Exclude only attributes consumed outside selector
    // matching by the current style pipeline.
    const OUT_OF_BAND_ATTRIBUTES: &[&str] = &[
        "style",
        "lang",
        "dir",
        "hidden",
        "text",
        "color",
        "size",
        "width",
        "height",
        "bgcolor",
        "align",
        "valign",
        "border",
        "bordercolor",
        "type",
        "clear",
        "hspace",
        "vspace",
        "marginwidth",
        "marginheight",
        "topmargin",
        "rightmargin",
        "bottommargin",
        "leftmargin",
        "cellspacing",
        "cellpadding",
        "nowrap",
        "noshade",
    ];
    !element.attributes().any(|attribute| {
        OUT_OF_BAND_ATTRIBUTES
            .iter()
            .any(|name| attribute.local_name().eq_ignore_ascii_case(name))
    })
}

impl StyleSharingCache {
    pub(super) fn probe(
        &mut self,
        element: ElementRef<'_>,
        has_inline_style: bool,
        parent_style: Option<StyleIndices>,
        parent_custom_map_id: u32,
        matched_rules: &[MatchedRule],
    ) -> StyleSharingProbe {
        if !element_is_shareable(element, has_inline_style) {
            return StyleSharingProbe::Uncacheable;
        }
        let mut hasher = FxHasher::default();
        parent_style.hash(&mut hasher);
        parent_custom_map_id.hash(&mut hasher);
        matched_rules.hash(&mut hasher);
        let hash = hasher.finish();
        let mut next = self.index.first(hash);
        while let Some(index) = next {
            let entry = &self.entries[index];
            if entry.parent_style == parent_style
                && entry.parent_custom_map_id == parent_custom_map_id
                && self.matched_rules[entry.matched_rules.clone()] == *matched_rules
            {
                #[cfg(test)]
                {
                    self.hit_count += 1;
                }
                return StyleSharingProbe::Hit(entry.result.clone());
            }
            next = entry.next;
        }
        if !self.index.admit(hash, self.entries.len()) {
            return StyleSharingProbe::Uncacheable;
        }
        let start = self.matched_rules.len();
        self.matched_rules.extend_from_slice(matched_rules);
        StyleSharingProbe::Miss(StyleSharingSignature {
            hash,
            parent_style,
            parent_custom_map_id,
            matched_rules: start..self.matched_rules.len(),
        })
    }

    pub(super) fn insert(
        &mut self,
        signature: StyleSharingSignature,
        style: StyleIndices,
        custom_map_id: u32,
        counters: CounterDirectives,
    ) {
        let next = self.index.insert(signature.hash, self.entries.len());
        self.entries.push(StyleSharingEntry {
            next,
            parent_style: signature.parent_style,
            parent_custom_map_id: signature.parent_custom_map_id,
            matched_rules: signature.matched_rules,
            result: SharedElementStyle {
                style,
                custom_map_id,
                counters,
            },
        });
    }

    #[cfg(test)]
    pub(super) fn hit_count(&self) -> usize {
        self.hit_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use html_style_model::ComputedStylesBuilder;

    #[test]
    fn exact_rule_and_parent_inputs_share_only_hint_free_elements() {
        let doc = html_parse::parse_dom_document(
            "<!doctype html><html><body><p class='same'></p><p id='other' class='same'></p><p class='same' align='right'></p><p class='same' data-index='different'></p><table><tr><td class='same'></td></tr></table></body></html>",
        )
        .unwrap();
        let elements = || doc.node_ids().filter_map(|node| doc.try_element_ref(node));
        let paragraphs = elements()
            .filter(|element| element.tag() == "p")
            .collect::<Vec<_>>();
        let cell = elements().find(|element| element.tag() == "td").unwrap();
        let styles = ComputedStylesBuilder::new(&doc);
        let style = styles.default_indices();
        let mut cache = StyleSharingCache::default();

        assert!(matches!(
            cache.probe(paragraphs[0], false, Some(style), 0, &[]),
            StyleSharingProbe::Uncacheable
        ));
        let signature = match cache.probe(paragraphs[1], false, Some(style), 0, &[]) {
            StyleSharingProbe::Miss(signature) => signature,
            _ => panic!("a repeated hash must request an exact signature"),
        };
        cache.insert(signature, style, 0, CounterDirectives::default());
        assert!(matches!(
            cache.probe(paragraphs[1], true, Some(style), 0, &[]),
            StyleSharingProbe::Uncacheable
        ));
        assert!(matches!(
            cache.probe(paragraphs[2], false, Some(style), 0, &[]),
            StyleSharingProbe::Uncacheable
        ));
        assert!(matches!(
            cache.probe(paragraphs[3], false, Some(style), 0, &[]),
            StyleSharingProbe::Hit(_)
        ));
        assert_eq!(cache.hit_count(), 1);
        assert!(matches!(
            cache.probe(cell, false, Some(style), 0, &[]),
            StyleSharingProbe::Uncacheable
        ));
    }
}
