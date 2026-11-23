//! Compile selector indices and compact match instructions from prepared rules.
use crate::style::compiled::{
    CandidateRange, CompiledSelectors, Compound, FastAlternative, FastAttribute, FastTest,
    PreparedRuleSet, PreparedSelector, Relation, RuleSelectorRange, ScopeId, SelectorBuckets,
    SelectorCandidate, SelectorIndex, SelectorState,
};
use crate::style::syntax::selectors::{selector_pseudo_mask, selector_specificity};
use lightningcss::selector::{Combinator, Component, PseudoClass, PseudoElement, Selector};
use rustc_data_structures::fx::FxHashMap;
use static_self::IntoOwned;

impl CompiledSelectors {
    pub(crate) fn empty() -> Self {
        Self {
            rule_starts: Vec::new(),
            selectors: Vec::new(),
            sibling_attributes: Vec::new(),
            ancestor_tests: Vec::new(),
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
            sibling_attributes: Vec::new(),
            ancestor_tests: Vec::new(),
            tests: Vec::with_capacity(fast_test_count),
            compounds: Vec::with_capacity(fast_compound_count),
        };
        for (_, rule) in prepared.iter() {
            let start =
                u32::try_from(output.selectors.len()).expect("prepared selector count fits in u32");
            let mut max_specificity = 0;
            for selector in &rule.style_rule().selectors.0 {
                let fast = append_fast(selector, &mut output.tests, &mut output.compounds);
                let specificity = selector_specificity(selector);
                max_specificity = max_specificity.max(specificity);
                let start = output.sibling_attributes.len();
                // Pseudos are matched separately even when the normal style is shared.
                let shareable = selector.has_pseudo_element()
                    || (rule.scope() == ScopeId::NONE
                        && sibling_invariant(selector, &mut output.sibling_attributes));
                if !shareable {
                    output.sibling_attributes.truncate(start);
                }
                let sibling_attributes = shareable.then(|| {
                    (
                        u32::try_from(start).expect("selector attributes fit in u32"),
                        u32::try_from(output.sibling_attributes.len())
                            .expect("selector attributes fit in u32"),
                    )
                });
                output.selectors.push(PreparedSelector {
                    sibling_attributes,
                    ancestor_requirements: append_ancestor_requirements(
                        selector,
                        &mut output.ancestor_tests,
                    ),
                    specificity,
                    fast_start: fast.map_or(0, |range| range.0),
                    fast_len: fast.map_or(0, |range| range.1),
                    pseudo_mask: selector_pseudo_mask(selector),
                    has_node_conditions: selector_has_node_conditions(selector),
                    bucket_component: bucket_component(selector),
                    bucket_test: fast.map_or(u16::MAX, |range| range.2),
                });
            }
            output.rule_starts.push(RuleSelectorRange {
                start,
                max_specificity,
            });
        }
        output.rule_starts.push(RuleSelectorRange {
            start: u32::try_from(output.selectors.len())
                .expect("prepared selector count fits in u32"),
            max_specificity: 0,
        });
        output
    }
}

/// Record attributes read by safe selectors. Siblings agreeing on these and
/// normalized HTML state share all ancestors. Reject positional, subtree,
/// scope or unknown dependencies, including nested logical selectors.
fn sibling_invariant(selector: &Selector<'_>, attributes: &mut Vec<String>) -> bool {
    selector
        .iter_raw_match_order()
        .all(|component| match component {
        Component::ExplicitAnyNamespace | Component::ExplicitNoNamespace
        | Component::DefaultNamespace(_) | Component::Namespace(_, _)
        | Component::ExplicitUniversalType | Component::LocalName(_)
        | Component::ID(_) | Component::Class(_)
        // Current non-tree pseudo-classes use normalized HTML state or
        // effective language, whose inherited source is the shared parent.
        | Component::NonTSPseudoClass(_)
        | Component::Combinator(Combinator::Child | Combinator::Descendant) => true,
        Component::AttributeInNoNamespaceExists { local_name, .. }
        | Component::AttributeInNoNamespace { local_name, .. } => {
            attributes.push(local_name.as_ref().to_ascii_lowercase());
            true
        }
        Component::AttributeOther(attribute) => {
            attributes.push(attribute.local_name.as_ref().to_ascii_lowercase());
            true
        }
        Component::Is(selectors) | Component::Where(selectors) | Component::Negation(selectors) => {
            selectors.iter().all(|selector| sibling_invariant(selector, attributes))
        }
        _ => false,
    })
}

impl SelectorIndex {
    /// Build an index only from rules admitted by stylesheet preparation.
    pub fn from_prepared(prepared: &PreparedRuleSet<'_>) -> Self {
        let mut index = SelectorBuckets::default();
        for (id, rule) in prepared.iter() {
            for (selector_index, selector) in rule.style_rule().selectors.0.iter().enumerate() {
                index.add_selector(
                    selector,
                    SelectorCandidate {
                        rule: id,
                        selector: u32::try_from(selector_index)
                            .expect("rule selector count fits in u32"),
                    },
                );
            }
        }
        index.finish(CompiledSelectors::from_prepared(prepared))
    }
}

impl SelectorBuckets {
    pub(crate) fn finish(self, compiled: CompiledSelectors) -> SelectorIndex {
        let count = self
            .by_tag
            .values()
            .chain(self.by_class.values())
            .chain(self.by_id.values())
            .chain(self.by_attribute.values())
            .chain(self.by_namespace.values())
            .chain(self.by_state.values())
            .map(Vec::len)
            .sum::<usize>()
            + self.universal.len()
            + self.root.len();
        let mut candidates = Vec::with_capacity(count);
        let mut append = |mut bucket: Vec<SelectorCandidate>| {
            bucket.sort_unstable_by_key(|candidate| (candidate.rule.index(), candidate.selector));
            bucket.dedup();
            let range = CandidateRange {
                start: u32::try_from(candidates.len()).expect("candidate pool fits u32"),
                len: u32::try_from(bucket.len()).expect("candidate bucket fits u32"),
            };
            candidates.extend(bucket);
            range
        };
        let by_tag = self
            .by_tag
            .into_iter()
            .map(|(key, bucket)| (key, append(bucket)))
            .collect();
        let by_class = self
            .by_class
            .into_iter()
            .map(|(key, bucket)| (key, append(bucket)))
            .collect();
        let by_id = self
            .by_id
            .into_iter()
            .map(|(key, bucket)| (key, append(bucket)))
            .collect();
        let by_attribute = self
            .by_attribute
            .into_iter()
            .map(|(key, bucket)| (key, append(bucket)))
            .collect();
        let by_namespace = self
            .by_namespace
            .into_iter()
            .map(|(key, bucket)| (key, append(bucket)))
            .collect();
        let by_state = self
            .by_state
            .into_iter()
            .map(|(key, bucket)| (key, append(bucket)))
            .collect();
        let root = append(self.root);
        let universal = append(self.universal);
        SelectorIndex {
            by_tag,
            by_class,
            by_id,
            by_attribute,
            by_namespace,
            by_state,
            root,
            universal,
            candidates,
            compiled,
        }
    }

    /// Prefer an ID, then a class, then a tag from the target compound.
    /// A selector is admitted only by its own key, not another selector in
    /// the same rule. Logical selectors without such a key stay universal.
    fn add_selector(&mut self, selector: &Selector<'_>, candidate: SelectorCandidate) {
        match target_key(selector).map(|(_, component)| component) {
            Some(Component::ID(id)) => {
                push_candidate(&mut self.by_id, id.as_ref(), candidate);
            }
            Some(Component::Class(class)) => {
                push_candidate(&mut self.by_class, class.as_ref(), candidate);
            }
            Some(Component::LocalName(tag)) => {
                push_candidate(&mut self.by_tag, tag.lower_name.as_ref(), candidate);
                // XML preserves case; a lower-case alias is only a candidate,
                // so mixed-case type selectors cannot skip exact checking.
                if tag.name != tag.lower_name {
                    push_candidate(&mut self.by_tag, tag.name.as_ref(), candidate);
                }
            }
            Some(Component::Is(alternatives) | Component::Where(alternatives)) => {
                for alternative in alternatives.iter() {
                    self.add_selector(alternative, candidate);
                }
            }
            Some(
                Component::AttributeInNoNamespaceExists { local_name, .. }
                | Component::AttributeInNoNamespace { local_name, .. },
            ) => {
                self.add_attribute(local_name.as_ref(), candidate);
            }
            Some(Component::AttributeOther(attribute)) => {
                self.add_attribute(attribute.local_name.as_ref(), candidate)
            }
            Some(Component::DefaultNamespace(namespace) | Component::Namespace(_, namespace)) => {
                push_candidate(&mut self.by_namespace, namespace.as_ref(), candidate);
            }
            Some(Component::ExplicitNoNamespace) => {
                push_candidate(&mut self.by_namespace, "", candidate)
            }
            Some(Component::Root) => self.root.push(candidate),
            Some(Component::NonTSPseudoClass(pseudo)) => {
                self.by_state
                    .entry(
                        selector_state(pseudo).expect("indexed state checked during key selection"),
                    )
                    .or_default()
                    .push(candidate);
            }
            _ => self.universal.push(candidate),
        }
    }

    fn add_attribute(&mut self, name: &str, candidate: SelectorCandidate) {
        push_candidate(&mut self.by_attribute, name, candidate);
        if name.bytes().any(|byte| byte.is_ascii_uppercase()) {
            push_candidate(
                &mut self.by_attribute,
                &name.to_ascii_lowercase(),
                candidate,
            );
        }
    }
}

fn selector_state(pseudo: &PseudoClass<'_>) -> Option<SelectorState> {
    match pseudo {
        PseudoClass::Link | PseudoClass::AnyLink(_) => Some(SelectorState::Link),
        PseudoClass::Enabled => Some(SelectorState::Enabled),
        PseudoClass::Disabled => Some(SelectorState::Disabled),
        PseudoClass::Checked => Some(SelectorState::Checked),
        _ => None,
    }
}

/// Allocate a key only for a new bucket, rather than for every selector.
fn push_candidate(
    buckets: &mut FxHashMap<String, Vec<SelectorCandidate>>,
    key: &str,
    candidate: SelectorCandidate,
) {
    if let Some(bucket) = buckets.get_mut(key) {
        bucket.push(candidate);
    } else {
        buckets.insert(key.to_owned(), vec![candidate]);
    }
}

/// Share key selection with proof metadata so the skipped predicate is
/// exactly the one that admitted this selector into the candidate list.
fn target_key<'a, 'css>(selector: &'a Selector<'css>) -> Option<(usize, &'a Component<'css>)> {
    let mut tag = None;
    let mut class = None;
    let mut alternatives = None;
    let mut attribute = None;
    let mut state = None;
    let mut namespace = None;
    let mut root = None;
    for (position, component) in selector.iter_raw_match_order().enumerate() {
        match component {
            Component::Combinator(Combinator::PseudoElement) => continue,
            Component::Combinator(_) => break,
            Component::ID(_) => return Some((position, component)),
            Component::Class(_) => class = Some((position, component)),
            Component::LocalName(_) => tag = Some((position, component)),
            Component::AttributeInNoNamespaceExists { .. }
            | Component::AttributeInNoNamespace { .. }
            | Component::AttributeOther(_) => attribute = Some((position, component)),
            Component::NonTSPseudoClass(pseudo) if selector_state(pseudo).is_some() => {
                state = Some((position, component))
            }
            Component::DefaultNamespace(_)
            | Component::Namespace(_, _)
            | Component::ExplicitNoNamespace => namespace = Some((position, component)),
            Component::Root => root = Some((position, component)),
            Component::Is(selectors) | Component::Where(selectors)
                if simple_alternatives(selectors) =>
            {
                alternatives = Some((position, component))
            }
            _ => {}
        }
    }
    class
        .or(tag)
        .or(alternatives)
        .or(attribute)
        .or(state)
        .or(root)
        .or(namespace)
}

fn bucket_component(selector: &Selector<'_>) -> u16 {
    let Some((position, component)) = target_key(selector) else {
        return u16::MAX;
    };
    let proved = match component {
        Component::ID(_) | Component::Class(_) => true,
        Component::LocalName(name) => name.name == name.lower_name,
        Component::Is(selectors) | Component::Where(selectors) => {
            selectors
                .iter()
                .all(|selector| match simple_component(selector) {
                    Some(Component::ID(_) | Component::Class(_)) => true,
                    Some(Component::LocalName(name)) => name.name == name.lower_name,
                    _ => false,
                })
        }
        _ => false,
    };
    if proved {
        u16::try_from(position).unwrap_or(u16::MAX)
    } else {
        u16::MAX
    }
}

fn append_ancestor_requirements(
    selector: &Selector<'_>,
    requirements: &mut Vec<FastAlternative>,
) -> (u32, u32) {
    let start = requirements.len();
    let mut iter = selector.iter();
    for _ in iter.by_ref() {}
    while let Some(combinator) = iter.next_sequence() {
        if !matches!(combinator, Combinator::Child | Combinator::Descendant) {
            requirements.truncate(start);
            break;
        }
        for component in iter.by_ref() {
            requirements.push(match component {
                Component::LocalName(name) => FastAlternative::LocalName {
                    name: name.name.clone().into_owned(),
                    lower_name: name.lower_name.clone().into_owned(),
                },
                Component::Class(class) => FastAlternative::Class(class.clone().into_owned()),
                Component::ID(id) => FastAlternative::Id(id.clone().into_owned()),
                _ => continue,
            });
        }
    }
    (
        u32::try_from(start).expect("ancestor requirements fit in u32"),
        u32::try_from(requirements.len()).expect("ancestor requirements fit in u32"),
    )
}

fn append_fast(
    selector: &Selector<'_>,
    tests: &mut Vec<FastTest>,
    compounds: &mut Vec<Compound>,
) -> Option<(u32, u16, u16)> {
    let tests_before = tests.len();
    let compounds_before = compounds.len();
    let mut test_start = tests.len();
    let mut pseudo_seen = false;
    let proven_component = bucket_component(selector);
    let mut proven_test = u16::MAX;
    for (position, component) in selector.iter_raw_match_order().enumerate() {
        if position == usize::from(proven_component) && proven_component != u16::MAX {
            proven_test = u16::try_from(tests.len() - tests_before).unwrap_or(u16::MAX);
        }
        match component {
            Component::LocalName(name) => tests.push(FastTest::LocalName {
                name: name.name.clone().into_owned(),
                lower_name: name.lower_name.clone().into_owned(),
            }),
            Component::ID(id) => tests.push(FastTest::Id(id.clone().into_owned())),
            Component::Class(class) => tests.push(FastTest::Class(class.clone().into_owned())),
            Component::NonTSPseudoClass(pseudo) => {
                tests.push(FastTest::PseudoClass(Box::new(pseudo.clone().into_owned())));
            }
            component @ (Component::AttributeInNoNamespaceExists { .. }
            | Component::AttributeInNoNamespace { .. }) => {
                tests.push(FastTest::Attribute(Box::new(fast_attribute(component))));
            }
            Component::Is(selectors) | Component::Where(selectors) => {
                let alternatives = selectors
                    .iter()
                    .map(|selector| match simple_component(selector) {
                        Some(Component::LocalName(name)) => Some(FastAlternative::LocalName {
                            name: name.name.clone().into_owned(),
                            lower_name: name.lower_name.clone().into_owned(),
                        }),
                        Some(Component::ID(id)) => {
                            Some(FastAlternative::Id(id.clone().into_owned()))
                        }
                        Some(Component::Class(class)) => {
                            Some(FastAlternative::Class(class.clone().into_owned()))
                        }
                        _ => None,
                    })
                    .collect::<Option<Box<[_]>>>();
                let Some(alternatives) = alternatives else {
                    return rollback(tests, compounds, tests_before, compounds_before);
                };
                tests.push(FastTest::Any(alternatives));
            }
            Component::Negation(selectors) => {
                // :not(A, B) is !A && !B. Each argument must remain atomic;
                // compounds and nested logical arguments keep exact general matching.
                for selector in selectors {
                    let test = match negatable_component(selector) {
                        Some(Component::LocalName(name)) => FastTest::NotLocalName {
                            name: name.name.clone().into_owned(),
                            lower_name: name.lower_name.clone().into_owned(),
                        },
                        Some(Component::ID(id)) => FastTest::NotId(id.clone().into_owned()),
                        Some(Component::Class(class)) => {
                            FastTest::NotClass(class.clone().into_owned())
                        }
                        Some(
                            component @ (Component::AttributeInNoNamespaceExists { .. }
                            | Component::AttributeInNoNamespace { .. }),
                        ) => FastTest::NotAttribute(Box::new(fast_attribute(component))),
                        Some(Component::NonTSPseudoClass(pseudo)) => {
                            FastTest::NotPseudoClass(Box::new(pseudo.clone().into_owned()))
                        }
                        _ => return rollback(tests, compounds, tests_before, compounds_before),
                    };
                    tests.push(test);
                }
            }
            Component::PseudoElement(
                PseudoElement::Before
                | PseudoElement::After
                | PseudoElement::FirstLine
                | PseudoElement::FirstLetter,
            ) => {
                if pseudo_seen || compounds.len() != compounds_before {
                    return rollback(tests, compounds, tests_before, compounds_before);
                }
                pseudo_seen = true;
            }
            Component::ExplicitUniversalType | Component::Combinator(Combinator::PseudoElement) => {
            }
            Component::Combinator(
                combinator @ (Combinator::Child | Combinator::Descendant | Combinator::NextSibling),
            ) => {
                let Ok(test_len) = u16::try_from(tests.len() - test_start) else {
                    return rollback(tests, compounds, tests_before, compounds_before);
                };
                compounds.push(Compound {
                    test_start: u32::try_from(test_start)
                        .expect("prepared selector tests fit in u32"),
                    test_len,
                    relation: match combinator {
                        Combinator::Child => Relation::Parent,
                        Combinator::Descendant => Relation::Ancestor,
                        Combinator::NextSibling => Relation::PreviousSibling,
                        _ => unreachable!("compact relation checked above"),
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
        proven_test,
    ))
}

/// Shared by positive and negated unprefixed attribute predicates.
fn fast_attribute(component: &Component<'_>) -> FastAttribute {
    match component {
        Component::AttributeInNoNamespaceExists {
            local_name,
            local_name_lower,
        } => FastAttribute::Exists {
            name: local_name.clone().into_owned(),
            lower_name: local_name_lower.clone().into_owned(),
        },
        Component::AttributeInNoNamespace {
            local_name,
            operator,
            value,
            case_sensitivity,
            ..
        } => FastAttribute::Value {
            operator: *operator,
            name: local_name.clone().into_owned(),
            value: value.clone().into_owned(),
            case_sensitivity: *case_sensitivity,
        },
        _ => unreachable!("compact attribute predicate checked by the caller"),
    }
}

fn rollback<T>(
    tests: &mut Vec<T>,
    compounds: &mut Vec<Compound>,
    tests_before: usize,
    compounds_before: usize,
) -> Option<(u32, u16, u16)> {
    tests.truncate(tests_before);
    compounds.truncate(compounds_before);
    None
}

fn fast_shape(selector: &Selector<'_>) -> Option<(usize, usize)> {
    let mut tests = 0;
    let mut compounds = 1;
    let mut pseudo_seen = false;
    for component in selector.iter_raw_match_order() {
        match component {
            Component::LocalName(_)
            | Component::ID(_)
            | Component::Class(_)
            | Component::NonTSPseudoClass(_) => tests += 1,
            Component::AttributeInNoNamespaceExists { .. }
            | Component::AttributeInNoNamespace { .. } => tests += 1,
            Component::Is(selectors) | Component::Where(selectors) => {
                if !selectors
                    .iter()
                    .all(|selector| simple_component(selector).is_some())
                {
                    return None;
                }
                tests += 1;
            }
            Component::Negation(selectors) => {
                if !selectors
                    .iter()
                    .all(|selector| negatable_component(selector).is_some())
                {
                    return None;
                }
                tests += selectors.len();
            }
            Component::PseudoElement(
                PseudoElement::Before
                | PseudoElement::After
                | PseudoElement::FirstLine
                | PseudoElement::FirstLetter,
            ) => {
                if pseudo_seen || compounds != 1 {
                    return None;
                }
                pseudo_seen = true;
            }
            Component::ExplicitUniversalType | Component::Combinator(Combinator::PseudoElement) => {
            }
            Component::Combinator(
                Combinator::Child | Combinator::Descendant | Combinator::NextSibling,
            ) => compounds += 1,
            _ => return None,
        }
    }
    Some((tests, compounds))
}

fn simple_alternatives(selectors: &[Selector<'_>]) -> bool {
    !selectors.is_empty()
        && selectors
            .iter()
            .all(|selector| simple_component(selector).is_some())
}

fn selector_has_node_conditions(selector: &Selector<'_>) -> bool {
    selector
        .iter_raw_match_order()
        .any(|component| match component {
            Component::AttributeInNoNamespaceExists { .. }
            | Component::AttributeInNoNamespace { .. }
            | Component::AttributeOther(_)
            | Component::NonTSPseudoClass(_) => true,
            Component::Is(selectors)
            | Component::Where(selectors)
            | Component::Negation(selectors) => selectors.iter().any(selector_has_node_conditions),
            _ => false,
        })
}

fn negatable_component<'a, 'css>(selector: &'a Selector<'css>) -> Option<&'a Component<'css>> {
    match selector.iter_raw_match_order().as_slice() {
        [
            component @ (Component::LocalName(_)
            | Component::ID(_)
            | Component::Class(_)
            | Component::AttributeInNoNamespaceExists { .. }
            | Component::AttributeInNoNamespace { .. }
            | Component::NonTSPseudoClass(_)),
        ] => Some(component),
        _ => None,
    }
}

fn simple_component<'a, 'css>(selector: &'a Selector<'css>) -> Option<&'a Component<'css>> {
    match selector.iter_raw_match_order().as_slice() {
        [component @ (Component::LocalName(_) | Component::ID(_) | Component::Class(_))] => {
            Some(component)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::compiled::EffectiveRuleId;
    use crate::style::matching::dom::{self, MatchCaches, TraversalContext};
    use crate::style::matching::traversal::StyleTraversal;
    use lightningcss::selector::SelectorList;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::traits::ParseWithOptions;

    fn parsed(text: &str) -> SelectorList<'_> {
        SelectorList::parse_string_with_options(text, ParserOptions::default()).unwrap()
    }

    #[test]
    fn index_keys_come_only_from_the_rightmost_compound() {
        let mut index = SelectorBuckets::default();
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
            index.add_selector(
                &selectors.0[0],
                SelectorCandidate {
                    rule: EffectiveRuleId(id as u32),
                    selector: 0,
                },
            );
        }
        assert_eq!(index.by_tag["p"], [SelectorCandidate { rule: EffectiveRuleId(2), selector: 0 }]);
        assert!(!index.by_tag.contains_key("item"));
        assert!(!index.by_tag.contains_key("section"));
        assert_eq!(
            index.by_class["note"],
            [SelectorCandidate {
                rule: EffectiveRuleId(4),
                selector: 0
            }]
        );
        let mut peers = index.by_class["peer"].clone();
        peers.dedup();
        assert_eq!(
            peers,
            [SelectorCandidate {
                rule: EffectiveRuleId(3),
                selector: 0
            }]
        );
        assert!(!index.by_class.contains_key("ancestor"));
        assert_eq!(
            index.by_id["target"],
            [SelectorCandidate {
                rule: EffectiveRuleId(0),
                selector: 0
            }]
        );
        assert!(!index.by_id.contains_key("ancestor"));
        assert_eq!(
            index.by_attribute["title"],
            [
                SelectorCandidate {
                    rule: EffectiveRuleId(1),
                    selector: 0
                },
            ]
        );
    }

    fn prepared(selector: &Selector<'_>, compiled: &mut CompiledSelectors) -> PreparedSelector {
        let (fast_start, fast_len, bucket_test) =
            append_fast(selector, &mut compiled.tests, &mut compiled.compounds).unwrap();
        PreparedSelector {
            ancestor_requirements: (0, 0),
            sibling_attributes: None,
            specificity: 0,
            fast_start,
            fast_len,
            pseudo_mask: selector_pseudo_mask(selector),
            has_node_conditions: selector_has_node_conditions(selector),
            bucket_component: bucket_component(selector),
            bucket_test,
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
            let bound = compiled.bind(&document);
            let mut traversal = StyleTraversal::new(&document);
            let mut caches = MatchCaches::default();
            while let Some(element) = traversal.next() {
                let node = element.node_id();
                let mut context = TraversalContext::new(&document, &mut traversal, &mut caches);
                assert_eq!(
                    bound.matches(prepared, node, &mut context),
                    Some(dom::selector_matches_dom_node(selector, &document, node)),
                    "selector {text} at node {node:?}"
                );
            }
        }
        for text in ["p:first-child"] {
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
        let bound = compiled.bind(&xml);
        let mut traversal = StyleTraversal::new(&xml);
        let mut caches = MatchCaches::default();
        while let Some(element) = traversal.next() {
            let node = element.node_id();
            let mut context = TraversalContext::new(&xml, &mut traversal, &mut caches);
            assert_eq!(
                bound.matches(prepared, node, &mut context),
                Some(dom::selector_matches_dom_node(selector, &xml, node))
            );
        }
    }
}
