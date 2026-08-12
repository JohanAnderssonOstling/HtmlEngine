//! Compact execution path for the common selector subset.
//!
//! Lightning CSS remains the parser and source of truth. Selectors outside
//! this deliberately small subset retain the general DOM matcher.

use super::dom;
use crate::style::rules::prepared::{EffectiveRuleId, PreparedRuleSet};
use html_dom::{Document, DomNodeId};
use lightningcss::selector::{Combinator, Component, Selector};
use lightningcss::values::ident::Ident;
use static_self::IntoOwned;

#[derive(Clone, Copy)]
pub(crate) struct PreparedSelector {
    pub(crate) ancestor_requirements: u64,
    specificity: u32,
    fast_start: u32,
    fast_len: u16,
}

#[derive(Clone)]
enum FastTest {
    LocalName {
        name: Ident<'static>,
        lower_name: Ident<'static>,
    },
    Id(Ident<'static>),
    Class(Ident<'static>),
}

#[derive(Clone, Copy)]
enum Relation {
    End,
    Parent,
    Ancestor,
}

#[derive(Clone, Copy)]
struct Compound {
    test_start: u32,
    test_len: u16,
    relation: Relation,
}

pub(crate) struct CompiledSelectors {
    rule_starts: Vec<u32>,
    selectors: Vec<PreparedSelector>,
    tests: Vec<FastTest>,
    compounds: Vec<Compound>,
}

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
                    specificity: dom::selector_specificity(selector),
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

    pub(crate) fn rule_count(&self) -> usize {
        self.rule_starts.len().saturating_sub(1)
    }

    pub(crate) fn for_rule(&self, id: EffectiveRuleId) -> &[PreparedSelector] {
        let start = self.rule_starts[id.index()] as usize;
        let end = self.rule_starts[id.index() + 1] as usize;
        &self.selectors[start..end]
    }

    #[inline]
    pub(crate) fn specificity(&self, selector: PreparedSelector) -> u32 {
        selector.specificity
    }

    #[inline]
    pub(crate) fn is_context_free(&self, selector: PreparedSelector) -> bool {
        selector.fast_len == 1
    }

    #[inline]
    pub(crate) fn matches(
        &self,
        selector: PreparedSelector,
        doc: &Document,
        node: DomNodeId,
    ) -> Option<bool> {
        match selector.fast_len {
            0 => None,
            1 => Some(self.matches_compound(selector.fast_start as usize, doc, node)),
            _ => Some(self.matches_from(selector.fast_start as usize, doc, node)),
        }
    }

    #[inline]
    fn matches_compound(&self, compound_index: usize, doc: &Document, node: DomNodeId) -> bool {
        let Some(element) = doc.element_ref(node) else {
            return false;
        };
        let compound = self.compounds[compound_index];
        for test in &self.tests[compound.test_start as usize
            ..compound.test_start as usize + compound.test_len as usize]
        {
            let matches = match test {
                FastTest::LocalName { name, lower_name } => {
                    let expected = if element.is_html_element_in_html_document() {
                        lower_name.as_ref()
                    } else {
                        name.as_ref()
                    };
                    element.tag() == expected
                }
                FastTest::Id(id) => element.id().is_some_and(|value| value == id.as_ref()),
                FastTest::Class(class) => element.has_class(class.as_ref()),
            };
            if !matches {
                return false;
            }
        }
        true
    }

    fn matches_from(&self, compound_index: usize, doc: &Document, node: DomNodeId) -> bool {
        if !self.matches_compound(compound_index, doc, node) {
            return false;
        }
        let element = doc
            .element_ref(node)
            .expect("a matching compact compound belongs to an element");
        match self.compounds[compound_index].relation {
            Relation::End => true,
            Relation::Parent => element
                .parent()
                .is_some_and(|parent| self.matches_from(compound_index + 1, doc, parent)),
            Relation::Ancestor => doc
                .dom_ancestors(node)
                .any(|ancestor| self.matches_from(compound_index + 1, doc, ancestor)),
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
                requirements |= super::selectors::AncestorFilter::hash(value)
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
    use lightningcss::selector::SelectorList;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::traits::ParseWithOptions;

    fn parsed(text: &str) -> SelectorList<'_> {
        SelectorList::parse_string_with_options(text, ParserOptions::default()).unwrap()
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
