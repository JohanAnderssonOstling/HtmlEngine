//! Execute compact selector instructions against document nodes.
use super::dom::{
    SiblingIndex, attribute_value_matches, parsed_case_sensitivity, pseudo_matches_dom,
};
use super::traversal::SummaryTest;
use crate::style::compiled::{
    CompiledSelectors, EffectiveRuleId, FastAlternative, FastAttribute, FastTest, PreparedSelector,
    Relation,
};
use html_dom::storage;
use html_dom::{Document, DomNodeId, ElementRef};
use lightningcss::selector::PseudoClass;

impl CompiledSelectors {
    pub(crate) fn for_rule(&self, id: EffectiveRuleId) -> &[PreparedSelector] {
        let start = self.rule_starts[id.index()].start as usize;
        let end = self.rule_starts[id.index() + 1].start as usize;
        &self.selectors[start..end]
    }

    pub(crate) fn max_specificity(&self, id: EffectiveRuleId) -> u32 {
        self.rule_starts[id.index()].max_specificity
    }

    /// Bind immutable CSS tests to this document without changing the shared program.
    pub(crate) fn bind<'a>(&'a self, document: &'a Document) -> BoundSelectors<'a> {
        // One allocation for all document-bound logical alternatives.
        let alternative_count = self
            .tests
            .iter()
            .map(|test| match test {
                FastTest::Any(alternatives) => alternatives.len(),
                _ => 0,
            })
            .sum();
        let mut alternatives = Vec::with_capacity(alternative_count);
        let tests = self
            .tests
            .iter()
            .map(|test| BoundTest::new(document, test, &mut alternatives))
            .collect();
        BoundSelectors {
            compiled: self,
            document,
            tests,
            alternatives,
        }
    }
}

pub(crate) struct BoundSelectors<'a> {
    compiled: &'a CompiledSelectors,
    document: &'a Document,
    tests: Vec<BoundTest<'a>>,
    alternatives: Vec<BoundAlternative>,
}

struct BoundClass {
    index: Option<u16>,
    mask: u32,
}

impl BoundClass {
    fn new(document: &Document, name: &str) -> Self {
        Self {
            index: document.lookup_string(name),
            mask: storage::selector_name_mask(name, false),
        }
    }

    fn matches(&self, element: ElementRef<'_>) -> bool {
        self.index
            .is_some_and(|index| element.has_class_idx(index, self.mask))
    }
}

enum BoundAlternative {
    LocalName {
        name: Option<u16>,
        lower_name: Option<u16>,
    },
    Id(Option<u16>),
    Class(BoundClass),
}

enum BoundTest<'a> {
    PseudoClass {
        pseudo: &'a PseudoClass<'static>,
        negated: bool,
    },
    LocalName {
        name: Option<u16>,
        lower_name: Option<u16>,
    },
    Id(Option<u16>),
    Class(BoundClass),
    NotLocalName {
        name: Option<u16>,
        lower_name: Option<u16>,
    },
    NotId(Option<u16>),
    NotClass(BoundClass),
    Any {
        start: usize,
        end: usize,
    },
    Attribute {
        attribute: &'a FastAttribute,
        name: Option<u16>,
        lower_name: Option<u16>,
        mask: u32,
        negated: bool,
    },
}

impl<'a> BoundTest<'a> {
    fn new(
        document: &Document,
        test: &'a FastTest,
        bound_alternatives: &mut Vec<BoundAlternative>,
    ) -> Self {
        let lookup = |name: &str| document.lookup_string(name);
        match test {
            FastTest::PseudoClass(pseudo) | FastTest::NotPseudoClass(pseudo) => Self::PseudoClass {
                pseudo,
                negated: matches!(test, FastTest::NotPseudoClass(_)),
            },
            FastTest::LocalName { name, lower_name } => Self::LocalName {
                name: lookup(name.as_ref()),
                lower_name: lookup(lower_name.as_ref()),
            },
            FastTest::NotLocalName { name, lower_name } => Self::NotLocalName {
                name: lookup(name.as_ref()),
                lower_name: lookup(lower_name.as_ref()),
            },
            FastTest::Id(name) => Self::Id(lookup(name.as_ref())),
            FastTest::NotId(name) => Self::NotId(lookup(name.as_ref())),
            FastTest::Class(name) => Self::Class(BoundClass::new(document, name.as_ref())),
            FastTest::NotClass(name) => Self::NotClass(BoundClass::new(document, name.as_ref())),
            FastTest::Any(alternatives) => {
                let start = bound_alternatives.len();
                bound_alternatives.extend(alternatives.iter().map(|test| match test {
                    FastAlternative::LocalName { name, lower_name } => {
                        BoundAlternative::LocalName {
                            name: lookup(name.as_ref()),
                            lower_name: lookup(lower_name.as_ref()),
                        }
                    }
                    FastAlternative::Id(name) => BoundAlternative::Id(lookup(name.as_ref())),
                    FastAlternative::Class(name) => {
                        BoundAlternative::Class(BoundClass::new(document, name.as_ref()))
                    }
                }));
                let end = bound_alternatives.len();
                Self::Any { start, end }
            }
            FastTest::Attribute(attribute) | FastTest::NotAttribute(attribute) => {
                let (name, lower_name) = match attribute.as_ref() {
                    FastAttribute::Exists { name, lower_name } => {
                        (name.as_ref(), lookup(lower_name.as_ref()))
                    }
                    FastAttribute::Value { name, .. } => {
                        let name = name.as_ref();
                        let lower_name = if name.bytes().any(|byte| byte.is_ascii_uppercase()) {
                            lookup(&name.to_ascii_lowercase())
                        } else {
                            lookup(name)
                        };
                        (name, lower_name)
                    }
                };
                Self::Attribute {
                    attribute,
                    name: lookup(name),
                    lower_name,
                    mask: storage::selector_name_mask(name, true),
                    negated: matches!(test, FastTest::NotAttribute(_)),
                }
            }
        }
    }
}

struct BoundMatcher<'a, 'cache, 'dom, 'css> {
    bound: &'a BoundSelectors<'dom>,
    siblings: Option<&'a mut SiblingIndex<'cache, 'dom, 'css>>,
}

impl<'dom> std::ops::Deref for BoundMatcher<'_, '_, 'dom, '_> {
    type Target = BoundSelectors<'dom>;
    fn deref(&self) -> &Self::Target {
        self.bound
    }
}

impl<'dom> BoundSelectors<'dom> {
    pub(crate) fn matches(&self, selector: PreparedSelector, node: DomNodeId) -> Option<bool> {
        BoundMatcher {
            bound: self,
            siblings: None,
        }
        .matches(selector, node)
    }
    pub(crate) fn matches_indexed(
        &self,
        selector: PreparedSelector,
        node: DomNodeId,
    ) -> Option<bool> {
        BoundMatcher {
            bound: self,
            siblings: None,
        }
        .matches_indexed(selector, node)
    }
    pub(crate) fn matches_indexed_in_traversal(
        &self,
        selector: PreparedSelector,
        node: DomNodeId,
        traversal: &mut SiblingIndex<'_, 'dom, '_>,
    ) -> Option<bool> {
        BoundMatcher {
            bound: self,
            siblings: Some(traversal),
        }
        .matches_indexed_in_traversal(selector, node)
    }
}

impl BoundMatcher<'_, '_, '_, '_> {
    #[inline]
    pub(crate) fn matches(&mut self, selector: PreparedSelector, node: DomNodeId) -> Option<bool> {
        match selector.fast_len {
            0 => None,
            1 => Some(self.matches_compound(selector.fast_start as usize, node)),
            _ => Some(self.matches_from(selector.fast_start as usize, node)),
        }
    }

    /// The candidate index already proved one predicate in the subject.
    #[inline]
    pub(crate) fn matches_indexed(
        &mut self,
        selector: PreparedSelector,
        node: DomNodeId,
    ) -> Option<bool> {
        if selector.bucket_test == u16::MAX {
            return self.matches(selector, node);
        }
        let start = selector.fast_start as usize;
        if selector.fast_len == 1 && self.compiled.compounds[start].test_len == 1 {
            return Some(true);
        }
        Some(
            self.matches_compound_skipping(start, node, selector.bucket_test)
                && (selector.fast_len == 1 || self.matches_relation(start, node)),
        )
    }

    /// A single local ancestor predicate can be answered from traversal facts.
    /// Longer chains and compound conjunctions retain exact matching.
    pub(crate) fn matches_indexed_in_traversal(
        &mut self,
        selector: PreparedSelector,
        node: DomNodeId,
    ) -> Option<bool> {
        if selector.fast_len == 2 {
            let start = selector.fast_start as usize;
            let first = self.compiled.compounds[start];
            let ancestor = self.compiled.compounds[start + 1];
            if matches!(first.relation, Relation::Ancestor)
                && matches!(ancestor.relation, Relation::End)
            {
                let test = match ancestor.test_len {
                    0 => Some(SummaryTest::Universal),
                    1 => SummaryTest::from_fast_test(
                        &self.bound.compiled.tests[ancestor.test_start as usize],
                    ),
                    _ => None,
                };
                if let Some(test) = test {
                    if !self.matches_compound_skipping(start, node, selector.bucket_test) {
                        return Some(false);
                    }
                    if let Some(matched) = self
                        .siblings
                        .as_deref_mut()
                        .and_then(|traversal| traversal.ancestor_matches(node, test))
                    {
                        return Some(matched);
                    }
                }
            }
        }
        self.matches_indexed(selector, node)
    }

    #[inline]
    fn matches_compound(&mut self, compound_index: usize, node: DomNodeId) -> bool {
        self.matches_compound_skipping(compound_index, node, u16::MAX)
    }

    #[inline]
    fn matches_compound_skipping(
        &mut self,
        compound_index: usize,
        node: DomNodeId,
        skip: u16,
    ) -> bool {
        let Some(element) = self.document.try_element_ref(node) else {
            return false;
        };
        let compound = self.compiled.compounds[compound_index];
        let tests = &self.bound.tests[compound.test_start as usize
            ..compound.test_start as usize + compound.test_len as usize];
        let skip = usize::from(skip);
        if skip < tests.len() {
            tests[..skip]
                .iter()
                .chain(&tests[skip + 1..])
                .all(|test| self.matches_test(test, element))
        } else {
            tests.iter().all(|test| self.matches_test(test, element))
        }
    }

    #[inline]
    fn matches_test(&mut self, test: &BoundTest<'_>, element: ElementRef<'_>) -> bool {
        match test {
            BoundTest::PseudoClass { pseudo, negated } => {
                pseudo_matches_dom(pseudo, self.document, element, self.siblings.as_deref_mut())
                    != *negated
            }
            BoundTest::LocalName { name, lower_name } => {
                Self::matches_name(*name, *lower_name, element)
            }
            BoundTest::Id(id) => id.is_some() && element.id_idx() == *id,
            BoundTest::Class(class) => class.matches(element),
            BoundTest::NotLocalName { name, lower_name } => {
                !Self::matches_name(*name, *lower_name, element)
            }
            BoundTest::NotId(id) => id.is_none() || element.id_idx() != *id,
            BoundTest::NotClass(class) => !class.matches(element),
            BoundTest::Any { start, end } => {
                self.alternatives[*start..*end]
                    .iter()
                    .any(|test| match test {
                        BoundAlternative::LocalName { name, lower_name } => {
                            Self::matches_name(*name, *lower_name, element)
                        }
                        BoundAlternative::Id(id) => id.is_some() && element.id_idx() == *id,
                        BoundAlternative::Class(class) => class.matches(element),
                    })
            }
            BoundTest::Attribute {
                attribute,
                name,
                lower_name,
                mask,
                negated,
            } => {
                let html = element.is_html_element_in_html_document();
                let name_idx = if html { *lower_name } else { *name };
                let matched = match attribute {
                    FastAttribute::Exists { name, lower_name } => element
                        .attr_with_name_idx(
                            if html {
                                lower_name.as_ref()
                            } else {
                                name.as_ref()
                            },
                            name_idx,
                            *mask,
                        )
                        .is_some(),
                    FastAttribute::Value {
                        operator,
                        name,
                        value,
                        case_sensitivity,
                    } => element
                        .attr_with_name_idx(name.as_ref(), name_idx, *mask)
                        .is_some_and(|actual| {
                            attribute_value_matches(
                                *operator,
                                actual,
                                value.as_ref(),
                                parsed_case_sensitivity(
                                    *case_sensitivity,
                                    html,
                                    Some(name.as_ref()),
                                ),
                            )
                        }),
                };
                matched != *negated
            }
        }
    }

    #[inline]
    fn matches_name(name: Option<u16>, lower_name: Option<u16>, element: ElementRef<'_>) -> bool {
        let expected = if element.is_html_element_in_html_document() {
            lower_name
        } else {
            name
        };
        expected == Some(element.tag_idx())
    }

    fn matches_from(&mut self, compound_index: usize, node: DomNodeId) -> bool {
        if !self.matches_compound(compound_index, node) {
            return false;
        }
        self.matches_relation(compound_index, node)
    }

    #[inline]
    fn matches_relation(&mut self, compound_index: usize, node: DomNodeId) -> bool {
        let element = self.document.element_ref(node);
        match self.compiled.compounds[compound_index].relation {
            Relation::End => true,
            Relation::Parent => element
                .parent()
                .is_some_and(|parent| self.matches_from(compound_index + 1, parent)),
            Relation::Ancestor => self
                .bound
                .document
                .dom_ancestors(node)
                .any(|ancestor| self.matches_from(compound_index + 1, ancestor)),
            Relation::PreviousSibling => {
                let previous = match self.siblings.as_deref_mut() {
                    Some(siblings) => siblings.previous_element_sibling(node),
                    None => element
                        .previous_element_sibling()
                        .map(|element| element.node_id()),
                };
                previous.is_some_and(|sibling| self.matches_from(compound_index + 1, sibling))
            }
        }
    }
}
