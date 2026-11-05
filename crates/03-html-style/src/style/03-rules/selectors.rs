//! Compile selector indices and compact match instructions from prepared rules.
use crate::style::compiled::{
    CompiledSelectors, Compound, EffectiveRuleId, FastTest, PreparedRuleSet, PreparedSelector,
    Relation, SelectorIndex,
};
use crate::style::syntax::selectors::{ancestor_hash, selector_specificity};
use lightningcss::selector::{Combinator, Component, Selector};
use rustc_data_structures::fx::FxHashMap;
use static_self::IntoOwned;

impl CompiledSelectors {
    pub(crate) fn empty() -> Self {
        Self {
            rule_starts: Vec::new(),
            selectors: Vec::new(),
            tests: Vec::new(),
            compounds: Vec::new(),
        }
    }

    pub(crate) fn from_prepared(prepared: &PreparedRuleSet<'_>) -> Self {
        let mut rule_count = 0;
        let mut selector_count = 0;
        let mut fast_test_count = 0;
        let mut fast_compound_count = 0;
        for (_, rule) in prepared.iter() {
            rule_count += 1;
            selector_count += rule.style_rule().selectors.0.len();
            for selector in &rule.style_rule().selectors.0 {
                if let Some((tests, compounds)) = fast_shape(selector) {
                    fast_test_count += tests;
                    fast_compound_count += compounds;
                }
            }
        }
        let mut output = Self {
            rule_starts: Vec::with_capacity(rule_count + 1),
            selectors: Vec::with_capacity(selector_count),
            tests: Vec::with_capacity(fast_test_count),
            compounds: Vec::with_capacity(fast_compound_count),
        };
        for (_, rule) in prepared.iter() {
            output.rule_starts.push(
                u32::try_from(output.selectors.len()).expect("prepared selector count fits in u32"),
            );
            for selector in &rule.style_rule().selectors.0 {
                let fast = append_fast(selector, &mut output.tests, &mut output.compounds);
                output.selectors.push(PreparedSelector {
                    ancestor_requirements: ancestor_requirement_mask(selector),
                    specificity: selector_specificity(selector),
                    fast_start: fast.map_or(0, |range| range.0),
                    fast_len: fast.map_or(0, |range| range.1),
                });
            }
        }
        output.rule_starts.push(
            u32::try_from(output.selectors.len()).expect("prepared selector count fits in u32"),
        );
        output
    }
}

impl SelectorIndex {
    pub fn new() -> Self {
        Self { by_tag: FxHashMap::default(), by_class: FxHashMap::default(), by_id: FxHashMap::default(), universal: Vec::new(), compiled: CompiledSelectors::empty() }
    }

    /// Build an index only from rules admitted by stylesheet preparation.
    pub fn from_prepared(prepared: &PreparedRuleSet<'_>) -> Self {
        let mut index = Self::new();
        index.compiled = CompiledSelectors::from_prepared(prepared);
        for (id, rule) in prepared.iter() {
            for selector in &rule.style_rule().selectors.0 {
                index.add_selector(selector, id);
            }
        }
        index
    }

    /// Add a selector to the index
    fn add_selector(&mut self, selector: &Selector, rule_id: EffectiveRuleId) {
        let mut indexed = false;

        // Raw match order starts at the rightmost compound, which is the only
        // compound guaranteed to match the element currently being resolved.
        // Indexing ancestor compounds admits false candidates for every
        // descendant selector and makes selector matching needlessly broad.
        for component in selector.iter_raw_match_order() {
            if component.is_combinator() {
                break;
            }
            let (index, key) = match component {
                Component::LocalName(name) => (&mut self.by_tag, name.lower_name.0.as_ref()),
                Component::Class(class) => (&mut self.by_class, class.0.as_ref()),
                Component::ID(id) => (&mut self.by_id, id.0.as_ref()),
                _ => continue,
            };
            index.entry(key.to_owned()).or_default().push(rule_id);
            indexed = true;
        }

        // Selectors without an indexable key remain universal candidates.
        if !indexed {
            self.universal.push(rule_id);
        }
    }
}

fn ancestor_requirement_mask(selector: &Selector<'_>) -> u64 {
    let mut iter = selector.iter();
    for _ in iter.by_ref() {}
    let mut requirements = 0;
    while let Some(combinator) = iter.next_sequence() {
        if !matches!(combinator, Combinator::Child | Combinator::Descendant) {
            return 0;
        }
        for component in iter.by_ref() {
            let value = match component {
                Component::LocalName(name) => Some(name.lower_name.0.as_ref()),
                Component::Class(class) => Some(class.0.as_ref()),
                Component::ID(id) => Some(id.0.as_ref()),
                _ => None,
            };
            if let Some(value) = value {
                requirements |= ancestor_hash(value)
            }
        }
    }
    requirements
}

fn append_fast(
    selector: &Selector<'_>,
    tests: &mut Vec<FastTest>,
    compounds: &mut Vec<Compound>,
) -> Option<(u32, u16)> {
    let tests_before = tests.len();
    let compounds_before = compounds.len();
    let mut test_start = tests.len();
    for component in selector.iter_raw_match_order() {
        match component {
            Component::LocalName(name) => tests.push(FastTest::LocalName {
                name: name.name.clone().into_owned(),
                lower_name: name.lower_name.clone().into_owned(),
            }),
            Component::ID(id) => tests.push(FastTest::Id(id.clone().into_owned())),
            Component::Class(class) => tests.push(FastTest::Class(class.clone().into_owned())),
            Component::ExplicitUniversalType => {}
            Component::Combinator(combinator @ (Combinator::Child | Combinator::Descendant)) => {
                let Ok(test_len) = u16::try_from(tests.len() - test_start) else {
                    return rollback(tests, compounds, tests_before, compounds_before);
                };
                compounds.push(Compound {
                    test_start: u32::try_from(test_start)
                        .expect("prepared selector tests fit in u32"),
                    test_len,
                    relation: if *combinator == Combinator::Child {
                        Relation::Parent
                    } else {
                        Relation::Ancestor
                    },
                });
                test_start = tests.len();
            }
            _ => return rollback(tests, compounds, tests_before, compounds_before),
        }
    }
    let Ok(test_len) = u16::try_from(tests.len() - test_start) else {
        return rollback(tests, compounds, tests_before, compounds_before);
    };
    compounds.push(Compound {
        test_start: u32::try_from(test_start).expect("prepared selector tests fit in u32"),
        test_len,
        relation: Relation::End,
    });
    let Ok(compound_len) = u16::try_from(compounds.len() - compounds_before) else {
        return rollback(tests, compounds, tests_before, compounds_before);
    };
    Some((
        u32::try_from(compounds_before).expect("prepared selector compounds fit in u32"),
        compound_len,
    ))
}

fn rollback<T>(
    tests: &mut Vec<T>,
    compounds: &mut Vec<Compound>,
    tests_before: usize,
    compounds_before: usize,
) -> Option<(u32, u16)> {
    tests.truncate(tests_before);
    compounds.truncate(compounds_before);
    None
}

fn fast_shape(selector: &Selector<'_>) -> Option<(usize, usize)> {
    let mut tests = 0;
    let mut compounds = 1;
    for component in selector.iter_raw_match_order() {
        match component {
            Component::LocalName(_) | Component::ID(_) | Component::Class(_) => tests += 1,
            Component::ExplicitUniversalType => {}
            Component::Combinator(Combinator::Child | Combinator::Descendant) => compounds += 1,
            _ => return None,
        }
    }
    Some((tests, compounds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::matching::dom;
    use lightningcss::selector::SelectorList;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::traits::ParseWithOptions;

    fn parsed(text: &str) -> SelectorList<'_> {
        SelectorList::parse_string_with_options(text, ParserOptions::default()).unwrap()
    }

    #[test]
    fn index_keys_come_only_from_the_rightmost_compound() {
        let mut index = SelectorIndex::new();
        for (id, text) in [
            "SECTION.ancestor > P.note#target",
            ".ancestor *[title]",
            "#ancestor :is(p, div)",
            ".ancestor + .peer.peer",
            "Item.note",
        ]
        .into_iter()
        .enumerate()
        {
            let selectors = parsed(text);
            index.add_selector(&selectors.0[0], EffectiveRuleId(id as u32));
        }
        assert_eq!(index.by_tag["p"], [EffectiveRuleId(0)]);
        assert_eq!(index.by_tag["item"], [EffectiveRuleId(4)]);
        assert!(!index.by_tag.contains_key("section"));
        assert_eq!(
            index.by_class["note"],
            [EffectiveRuleId(0), EffectiveRuleId(4)]
        );
        let mut peers = index.by_class["peer"].clone();
        peers.dedup();
        assert_eq!(peers, [EffectiveRuleId(3)]);
        assert!(!index.by_class.contains_key("ancestor"));
        assert_eq!(index.by_id["target"], [EffectiveRuleId(0)]);
        assert!(!index.by_id.contains_key("ancestor"));
        assert_eq!(index.universal, [EffectiveRuleId(1), EffectiveRuleId(2)]);
    }

    fn prepared(
        selector: &Selector<'_>,
        compiled: &mut CompiledSelectors,
    ) -> PreparedSelector {
        let (fast_start, fast_len) =
            append_fast(selector, &mut compiled.tests, &mut compiled.compounds).unwrap();
        PreparedSelector {
            ancestor_requirements: 0,
            specificity: 0,
            fast_start,
            fast_len,
        }
    }

    #[test]
    fn compact_matcher_agrees_with_general_matcher_and_rejects_complex_forms() {
        let document = html_parse::parse_dom_document("<!doctype html><html><body><section class='chapter'><div><p id='target' class='note'>x</p><p class='note'>y</p></div></section><p class='note'>z</p></body></html>").unwrap();
        for text in [
            "*",
            ".note",
            "p.note",
            "#target",
            ".chapter > div",
            "body .chapter p.note",
            "section > div > p",
        ] {
            let selectors = parsed(text);
            let selector = &selectors.0[0];
            let mut compiled = CompiledSelectors::empty();
            let prepared = prepared(selector, &mut compiled);
            for node in document
                .node_ids()
                .filter(|node| document.element_ref(*node).is_some())
            {
                assert_eq!(
                    compiled.matches(prepared, &document, node),
                    Some(dom::selector_matches_dom_node(selector, &document, node)),
                    "selector {text} at node {node:?}"
                );
            }
        }
        for text in [
            "[data-x]",
            "p + p",
            "p:first-child",
            ":is(p, div)",
            "p::before",
        ] {
            let selectors = parsed(text);
            let mut tests = Vec::new();
            let mut compounds = Vec::new();
            assert!(
                append_fast(&selectors.0[0], &mut tests, &mut compounds).is_none(),
                "complex selector {text} must use the general matcher"
            );
            assert!(tests.is_empty() && compounds.is_empty());
        }
        let xml = html_parse::parse_xml_document("<root><Item/><item/></root>")
            .unwrap()
            .build_dom();
        let selectors = parsed("Item");
        let selector = &selectors.0[0];
        let mut compiled = CompiledSelectors::empty();
        let prepared = prepared(selector, &mut compiled);
        for node in xml
            .node_ids()
            .filter(|node| xml.element_ref(*node).is_some())
        {
            assert_eq!(
                compiled.matches(prepared, &xml, node),
                Some(dom::selector_matches_dom_node(selector, &xml, node))
            );
        }
    }
}
