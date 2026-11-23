use super::filter::SelectorFilter;
use super::names::GeneralNames;
use super::traversal::{StyleTraversal, SummaryTest};
#[cfg(test)]
use crate::style::syntax::selectors::selector_specificity;
use crate::style::syntax::selectors::{
    PSEUDO_AFTER_MASK, PSEUDO_BEFORE_MASK, PSEUDO_FIRST_LETTER_MASK, PSEUDO_FIRST_LINE_MASK,
};
use html_dom::{Document, DomNodeId, ElementRef, NodeRef};
use lightningcss::selector::{Combinator, Component, Direction, PseudoClass, PseudoElement, Selector};
use parcel_selectors::attr::{
    AttrSelectorOperator, CaseSensitivity, NamespaceConstraint, ParsedAttrSelectorOperation,
    ParsedCaseSensitivity,
};
use parcel_selectors::parser::{NthSelectorData, NthType};
use rustc_data_structures::fx::{FxHashMap, FxHasher};
use std::hash::{Hash, Hasher};

/// Traversal facts shared by one style-resolution pass. Off-path searches
/// retain lazy document-wide sibling links as an exact fallback.
pub(crate) struct TraversalContext<'context, 'a, 'css> {
    document: &'a Document,
    traversal: &'context mut StyleTraversal<'a>,
    caches: &'context mut MatchCaches<'css>,
}

#[derive(Default)]
pub(crate) struct MatchCaches<'css> {
    previous: Option<Vec<DomNodeId>>,
    search_scratch: Vec<DomNodeId>,
    has_results: FxHashMap<(DomNodeId, usize), bool>,
    names: GeneralNames,
    selector_ids: SelectorIds<'css>,
    nth_indices: FxHashMap<NthIndexKey, i32>,
    relative_filters: FxHashMap<(DomNodeId, bool), Option<Box<SelectorFilter>>>,
}

/// Structural interning gives recursive selectors stable pass-local IDs.
/// Hash collisions are checked against the complete selector list.
#[derive(Default)]
struct SelectorIds<'css> {
    buckets: FxHashMap<u64, Vec<(Vec<Selector<'css>>, usize)>>,
    next: usize,
}

impl<'css> SelectorIds<'css> {
    fn intern(&mut self, selectors: &[Selector<'css>]) -> usize {
        let mut hash = FxHasher::default();
        selectors.hash(&mut hash);
        let bucket = self.buckets.entry(hash.finish()).or_default();
        if let Some((_, id)) = bucket
            .iter()
            .find(|(stored, _)| stored.as_slice() == selectors)
        {
            return *id;
        }
        let id = self.next;
        self.next += 1;
        bucket.push((selectors.to_vec(), id));
        id
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct NthIndexKey {
    filter: usize,
    scope: Option<DomNodeId>,
    ancestor_floor: Option<DomNodeId>,
    // This cache belongs to one document; indices are only opaque keys.
    node: DomNodeId,
    of_type: bool,
    from_end: bool,
}

impl<'context, 'a, 'css> TraversalContext<'context, 'a, 'css> {
    pub(crate) fn new(
        document: &'a Document,
        traversal: &'context mut StyleTraversal<'a>,
        caches: &'context mut MatchCaches<'css>,
    ) -> Self {
        Self {
            document,
            traversal,
            caches,
        }
    }

    fn with_search_scratch<R>(&mut self, search: impl FnOnce(&mut Vec<DomNodeId>) -> R) -> R {
        // Release the borrow before matching: nested logical selectors may
        // themselves search. Retain the larger allocation when they finish.
        let mut pending = std::mem::take(&mut self.caches.search_scratch);
        let matched = search(&mut pending);
        pending.clear();
        let cached = &mut self.caches.search_scratch;
        if pending.capacity() > cached.capacity() {
            *cached = pending;
        }
        matched
    }

    pub(super) fn ancestor_matches(
        &mut self,
        node: DomNodeId,
        test: SummaryTest<'_>,
    ) -> Option<bool> {
        self.traversal.ancestor_matches(node, test)
    }

    fn earlier_sibling_matches(&mut self, node: DomNodeId, test: SummaryTest<'_>) -> Option<bool> {
        self.traversal.earlier_sibling_matches(node, test)
    }

    fn matches_nth_child(&mut self, nth: &NthSelectorData, node: DomNodeId) -> Option<bool> {
        let from_end = nth.ty.is_from_end();
        let (position, count) = self
            .traversal
            .sibling_position(node, from_end || nth.ty.is_only())?;
        Some(if nth.ty.is_only() {
            count == 1
        } else {
            let index = if from_end {
                count - position + 1
            } else {
                position
            };
            nth_index_matches(nth.a, nth.b, index)
        })
    }

    fn nth_index(
        element: ElementRef<'_>,
        selectors: &[Selector<'css>],
        of_type: bool,
        from_end: bool,
        context: &mut MatchContext<'_, '_, '_, 'css>,
    ) -> i32 {
        let document = context.siblings.document;
        let key = NthIndexKey {
            node: element.node_id(),
            filter: context.siblings.caches.selector_ids.intern(selectors),
            of_type,
            from_end,
            scope: if selectors.is_empty() {
                None
            } else {
                context.scope
            },
            ancestor_floor: if selectors.is_empty() {
                None
            } else {
                context.exclusive_ancestor_floor
            },
        };
        if let Some(&index) = context.siblings.caches.nth_indices.get(&key) {
            return index;
        }
        let mut matches_filter = |element: ElementRef<'_>| {
            selectors.is_empty()
                || selectors.iter().any(|selector| {
                    selector_matches_dom_node_with_context(
                        selector,
                        document,
                        element.node_id(),
                        context,
                    )
                })
        };
        let Some(parent) = element.parent().map(|parent| document.element_ref(parent)) else {
            return i32::from(matches_filter(element));
        };
        let mut children = parent.children();
        let mut position = 0;
        let mut result = 0;
        let mut entries = Vec::new();
        // Populate the sibling group once. Release all cache borrows before
        // evaluating a filter, since it may recursively use positional matching.
        while let Some(child) = if from_end {
            children.next_back()
        } else {
            children.next()
        } {
            let Some(child) = document.try_element_ref(child) else {
                continue;
            };
            if of_type && (child.tag() != element.tag() || child.namespace() != element.namespace())
            {
                continue;
            }
            let index = if matches_filter(child) {
                position += 1;
                position
            } else {
                0
            };
            if child.node_id() == element.node_id() {
                result = index;
            }
            entries.push((
                NthIndexKey {
                    node: child.node_id(),
                    ..key.clone()
                },
                index,
            ));
        }
        context.siblings.caches.nth_indices.extend(entries);
        result
    }

    fn reject_relative_subtree(
        &mut self,
        components: &[Component<'_>],
        anchor: DomNodeId,
        children_only: bool,
    ) -> bool {
        let requirements = SelectorFilter::for_subject(components);
        if requirements.is_empty() {
            return false;
        }
        let key = (anchor, children_only);
        {
            let filters = &mut self.caches.relative_filters;
            match filters.get(&key) {
                Some(Some(filter)) => return !filter.contains(requirements),
                Some(None) => {}
                None => {
                    // A one-off search should not pay to summarize its subtree.
                    filters.insert(key, None);
                    return false;
                }
            }
        }
        let filter = self.with_search_scratch(|pending| {
            let mut filter = SelectorFilter::default();
            if let Some(element) = self.document.try_element_ref(anchor) {
                pending.extend(element.child_elements().map(|child| child.node_id()));
            }
            while let Some(node) = pending.pop() {
                if let Some(element) = self.document.try_element_ref(node) {
                    filter.insert_element(element);
                    if !children_only {
                        pending.extend(element.child_elements().map(|child| child.node_id()));
                    }
                }
            }
            filter
        });
        let rejected = !filter.contains(requirements);
        self.caches
            .relative_filters
            .insert(key, Some(Box::new(filter)));
        rejected
    }

    pub(super) fn effective_language(&mut self, node: DomNodeId) -> Option<Option<&'a str>> {
        self.traversal.effective_language(node)
    }

    pub(super) fn previous_element_sibling(&mut self, node: DomNodeId) -> Option<DomNodeId> {
        if self.caches.previous.as_ref().is_none() {
            if let Some(previous) = self.traversal.previous_sibling(node) {
                return previous;
            }
        }
        self.document.try_element_ref(node)?;
        let previous = self.caches.previous.get_or_insert_with(|| {
            // A node cannot be its own previous sibling; self denotes absence.
            let mut previous: Vec<_> = self.document.element_ids().collect();
            for (_, node) in self.document.nodes() {
                let NodeRef::Element(parent) = node else {
                    continue;
                };
                let mut sibling = None;
                for child in parent.children() {
                    if self.document.is_element_node(child) {
                        previous[child.index()] = sibling.unwrap_or(child);
                        sibling = Some(child);
                    }
                }
            }
            previous
        });
        let sibling = previous[node.index()];
        (sibling != node).then_some(sibling)
    }
}

// ============================================================================
// DOM-Based Selector Matching
// ============================================================================

/// Match a selector against a DOM node in the Document
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PseudoTarget {
    Before,
    After,
    FirstLine,
    FirstLetter,
}

impl PseudoTarget {
    pub(crate) const fn mask(self) -> u8 {
        match self {
            Self::Before => PSEUDO_BEFORE_MASK,
            Self::After => PSEUDO_AFTER_MASK,
            Self::FirstLine => PSEUDO_FIRST_LINE_MASK,
            Self::FirstLetter => PSEUDO_FIRST_LETTER_MASK,
        }
    }
}

#[cfg(test)]
pub fn selector_matches_dom_node<'css>(
    selector: &Selector<'css>,
    doc: &Document,
    node_idx: DomNodeId,
) -> bool {
    let mut traversal = StyleTraversal::new(doc);
    let mut caches = MatchCaches::default();
    let mut siblings = TraversalContext::new(doc, &mut traversal, &mut caches);
    match_from_position_dom(
        selector.iter_raw_match_order().as_slice(),
        doc,
        node_idx,
        None,
        &mut MatchContext {
            scope: None,
            exclusive_ancestor_floor: None,
            siblings: &mut siblings,
        },
    ) == MatchResult::Matched
}

pub(crate) fn selector_matches_dom_node_in_scope<'css>(
    selector: &Selector<'css>,
    doc: &Document,
    node_idx: DomNodeId,
    scope: DomNodeId,
    siblings: &mut TraversalContext<'_, '_, 'css>,
) -> bool {
    selector_matches_indexed_in_scope(selector, doc, node_idx, None, scope, siblings, u16::MAX)
}

/// Only candidate-index callers may supply a proved subject predicate.
/// Pseudo callers also check the compiled pseudo-target mask first.
pub(crate) fn selector_matches_indexed_in_scope<'css>(
    selector: &Selector<'css>,
    doc: &Document,
    node_idx: DomNodeId,
    pseudo: Option<PseudoTarget>,
    scope: DomNodeId,
    siblings: &mut TraversalContext<'_, '_, 'css>,
    bucket_component: u16,
) -> bool {
    match_from_position_dom_skipping(
        selector.iter_raw_match_order().as_slice(),
        doc,
        node_idx,
        pseudo,
        &mut MatchContext {
            scope: Some(scope),
            exclusive_ancestor_floor: None,
            siblings,
        },
        (bucket_component != u16::MAX).then_some(usize::from(bucket_component)),
    ) == MatchResult::Matched
}

struct MatchContext<'context, 'cache, 'dom, 'css> {
    /// The element represented by `:scope`. Outside relative selector
    /// matching, `:scope` defaults to the document element.
    scope: Option<DomNodeId>,
    /// An implicit relative selector is scoped to descendants of its anchor.
    /// Ancestor combinators must not escape that subtree or match the anchor
    /// itself (the omitted leading combinator is `:scope `).
    exclusive_ancestor_floor: Option<DomNodeId>,
    siblings: &'context mut TraversalContext<'cache, 'dom, 'css>,
}

/// Failure describes the nearest combinator at which retrying may succeed.
/// Ancestor exhaustion cannot be repaired by trying another sibling or a
/// higher ancestor; missing siblings can still be repaired by moving upwards.
#[derive(Clone, Copy, Eq, PartialEq)]
enum MatchResult {
    Matched,
    RetrySibling,
    RetryAncestor,
    Failed,
}

/// Match selector components from a position against a DOM node.
#[inline]
fn match_from_position_dom<'css>(
    components: &[Component<'css>],
    doc: &Document,
    node_idx: DomNodeId,
    pseudo: Option<PseudoTarget>,
    context: &mut MatchContext<'_, '_, '_, 'css>,
) -> MatchResult {
    match_from_position_dom_skipping(components, doc, node_idx, pseudo, context, None)
}

fn match_from_position_dom_skipping<'css>(
    components: &[Component<'css>],
    doc: &Document,
    node_idx: DomNodeId,
    pseudo: Option<PseudoTarget>,
    context: &mut MatchContext<'_, '_, '_, 'css>,
    skip: Option<usize>,
) -> MatchResult {
    let element = doc.try_element_ref(node_idx);
    let mut pos = 0;
    let mut deferred = false;
    while let Some(component) = components.get(pos) {
        if component.is_combinator() {
            break;
        }
        let Some(element) = element else {
            return MatchResult::RetrySibling;
        };
        if Some(pos) == skip {
            pos += 1;
            continue;
        }
        if component_needs_search(component) {
            deferred = true;
        } else if !component_matches_dom(component, doc, element, pseudo, context) {
            return MatchResult::RetrySibling;
        }
        pos += 1;
    }
    // Local tag/ID/class/attribute failures avoid logical and structural
    // searches. Revisit metadata only when this compound deferred a search.
    if deferred {
        let Some(element) = element else {
            return MatchResult::RetrySibling;
        };
        for (position, component) in components[..pos].iter().enumerate() {
            if Some(position) != skip
                && component_needs_search(component)
                && !component_matches_dom(component, doc, element, pseudo, context)
            {
                return MatchResult::RetrySibling;
            }
        }
    }
    let Some(Component::Combinator(combinator)) = components.get(pos) else {
        return MatchResult::Matched;
    };
    let remaining = &components[pos + 1..];
    match combinator {
        // Both sides of this internal boundary refer to the same DOM node.
        Combinator::PseudoElement => {
            // The originating element is still the subject. Every real DOM
            // relation and every nested logical match uses the exact wrapper.
            match_from_position_dom_skipping(
                remaining,
                doc,
                node_idx,
                None,
                context,
                skip.and_then(|position| position.checked_sub(pos + 1)),
            )
        }
        Combinator::Child => {
            let Some(parent) = element
                .and_then(|element| element.parent())
                .filter(|parent| Some(*parent) != context.exclusive_ancestor_floor)
            else {
                return MatchResult::Failed;
            };
            match match_from_position_dom(remaining, doc, parent, pseudo, context) {
                result @ (MatchResult::Matched | MatchResult::Failed) => result,
                // A different sibling has the same parent. Only an ancestor
                // retry can change which element is tested as the parent.
                _ => MatchResult::RetryAncestor,
            }
        }
        Combinator::Descendant => {
            if context.exclusive_ancestor_floor.is_none()
                && let Some(test) = SummaryTest::from_components(remaining)
                && let Some(matched) = context.siblings.ancestor_matches(node_idx, test)
            {
                return if matched {
                    MatchResult::Matched
                } else {
                    MatchResult::Failed
                };
            }
            for ancestor in doc.dom_ancestors(node_idx) {
                if Some(ancestor) == context.exclusive_ancestor_floor {
                    break;
                }
                match match_from_position_dom(remaining, doc, ancestor, pseudo, context) {
                    result @ (MatchResult::Matched | MatchResult::Failed) => return result,
                    _ => {}
                }
            }
            MatchResult::Failed
        }
        Combinator::NextSibling => {
            let Some(sibling) = context.siblings.previous_element_sibling(node_idx) else {
                return MatchResult::RetryAncestor;
            };
            match_from_position_dom(remaining, doc, sibling, pseudo, context)
        }
        Combinator::LaterSibling => {
            if let Some(test) = SummaryTest::from_components(remaining)
                && let Some(matched) = context.siblings.earlier_sibling_matches(node_idx, test)
            {
                return if matched {
                    MatchResult::Matched
                } else {
                    MatchResult::RetryAncestor
                };
            }
            let mut current = context.siblings.previous_element_sibling(node_idx);
            while let Some(sibling) = current {
                match match_from_position_dom(remaining, doc, sibling, pseudo, context) {
                    MatchResult::RetrySibling => {}
                    result => return result,
                }
                current = context.siblings.previous_element_sibling(sibling);
            }
            MatchResult::RetryAncestor
        }
        _ => MatchResult::RetrySibling,
    }
}

fn component_needs_search(component: &Component<'_>) -> bool {
    matches!(
        component,
        Component::Has(_)
            | Component::Is(_)
            | Component::Where(_)
            | Component::Negation(_)
            | Component::Nth(_)
            | Component::NthOf(_)
            | Component::NonTSPseudoClass(PseudoClass::Lang { .. })
    )
}

/// Keep common local checks separate from structural searches so callers
/// can inline them without the search helpers' larger stack frames.
#[inline]
fn component_matches_dom<'css>(
    component: &Component<'css>,
    doc: &Document,
    element: ElementRef<'_>,
    pseudo: Option<PseudoTarget>,
    context: &mut MatchContext<'_, '_, '_, 'css>,
) -> bool {
    if let Some(matched) = context
        .siblings
        .caches
        .names
        .matches(component, doc, element)
    {
        return matched;
    }
    match component {
        Component::ExplicitAnyNamespace => true,
        Component::ExplicitNoNamespace => element.namespace().is_none(),
        Component::DefaultNamespace(namespace) | Component::Namespace(_, namespace) => {
            element.namespace() == Some(namespace.as_ref())
        }
        Component::ExplicitUniversalType => true,

        Component::LocalName(name) => {
            let selector_name = if element.is_html_element_in_html_document() {
                name.lower_name.0.as_ref()
            } else {
                name.name.0.as_ref()
            };
            element.tag() == selector_name
        }

        Component::ID(id) => element.id().is_some_and(|node_id| node_id == id.0.as_ref()),

        Component::Class(class_selector) => element.has_class(class_selector.0.as_ref()),

        _ => component_matches_complex_dom(component, doc, element, pseudo, context),
    }
}

fn component_matches_complex_dom<'css>(
    component: &Component<'css>,
    doc: &Document,
    element: ElementRef<'_>,
    pseudo: Option<PseudoTarget>,
    context: &mut MatchContext<'_, '_, '_, 'css>,
) -> bool {
    let node_idx = element.node_id();
    match component {
        Component::AttributeInNoNamespaceExists {
            local_name,
            local_name_lower,
        } => {
            let attr_name = local_name_string(
                local_name,
                local_name_lower,
                element.is_html_element_in_html_document(),
            );
            element.has_attr(attr_name)
        }

        Component::AttributeInNoNamespace {
            local_name,
            operator,
            value,
            case_sensitivity,
            ..
        } => {
            let attr_name = local_name_string(
                local_name,
                local_name,
                element.is_html_element_in_html_document(),
            );
            let Some(attr_value) = element.attr(attr_name) else {
                return false;
            };

            attribute_value_matches(
                *operator,
                attr_value,
                value.as_ref(),
                parsed_case_sensitivity(
                    *case_sensitivity,
                    element.is_html_element_in_html_document(),
                    Some(attr_name),
                ),
            )
        }

        Component::AttributeOther(other) => {
            let local_name = other.local_name.as_ref();
            match &other.operation {
                ParsedAttrSelectorOperation::Exists => attribute_exists_in_namespace(
                    element,
                    namespace_match(other.namespace()),
                    local_name,
                ),
                ParsedAttrSelectorOperation::WithValue {
                    operator,
                    case_sensitivity,
                    expected_value,
                } => {
                    let Some(attr_value) = get_attribute_in_namespace(
                        element,
                        namespace_match(other.namespace()),
                        local_name,
                    ) else {
                        return false;
                    };

                    let html_attribute =
                        matches!(namespace_match(other.namespace()), NamespaceMatch::None)
                            .then_some(local_name);
                    attribute_value_matches(
                        *operator,
                        attr_value,
                        expected_value.as_ref(),
                        parsed_case_sensitivity(
                            *case_sensitivity,
                            element.is_html_element_in_html_document(),
                            html_attribute,
                        ),
                    )
                }
            }
        }

        Component::NonTSPseudoClass(pseudo) => {
            pseudo_matches_dom(pseudo, doc, element, &mut *context.siblings)
        }
        Component::PseudoElement(PseudoElement::Before) => pseudo == Some(PseudoTarget::Before),
        Component::PseudoElement(PseudoElement::After) => pseudo == Some(PseudoTarget::After),
        Component::PseudoElement(PseudoElement::FirstLine) => {
            pseudo == Some(PseudoTarget::FirstLine)
        }
        Component::PseudoElement(PseudoElement::FirstLetter) => {
            pseudo == Some(PseudoTarget::FirstLetter)
        }
        Component::PseudoElement(_) => false,

        Component::Root => doc.dom_root() == Some(node_idx),
        Component::Scope => context.scope.or_else(|| doc.dom_root()) == Some(node_idx),
        // Selectors L3 `:empty`: no children at all (text nodes count as content).
        Component::Empty => element.children().next().is_none(),
        Component::Nth(nth) => nth_matches_dom(nth, element, context),
        Component::NthOf(nth) => {
            nth_of_matches_dom(nth.nth_data(), nth.selectors(), element, context)
        }

        // Forgiving (`:is()`) and non-specificity (`:where()`) matching: the
        // node matches if any inner selector matches it. The UA stylesheet uses
        // these for nested-list markers and margins.
        Component::Is(selectors) | Component::Where(selectors) => {
            selectors.iter().any(|selector| {
                selector_matches_dom_node_with_context(selector, doc, node_idx, context)
            })
        }
        // `:not()` matches when none of the inner selectors match.
        Component::Negation(selectors) => !selectors.iter().any(|selector| {
            selector_matches_dom_node_with_context(selector, doc, node_idx, context)
        }),
        Component::Has(selectors) => selectors
            .iter()
            .any(|selector| relative_selector_matches_dom_node(selector, doc, node_idx, context)),

        _ => false,
    }
}

fn selector_matches_dom_node_with_context<'css>(
    selector: &Selector<'css>,
    doc: &Document,
    node_idx: DomNodeId,
    context: &mut MatchContext<'_, '_, '_, 'css>,
) -> bool {
    match_from_position_dom(
        selector.iter_raw_match_order().as_slice(),
        doc,
        node_idx,
        None,
        context,
    ) == MatchResult::Matched
}

/// Match a selector from `:has()` relative to its anchor element. Parcel
/// represents an explicit leading combinator by appending `:scope` to the raw
/// match-order components. Without one, the selector has an implicit
/// descendant combinator and is limited to the anchor's descendant subtree.
fn relative_selector_matches_dom_node<'css>(
    selector: &Selector<'css>,
    doc: &Document,
    anchor: DomNodeId,
    context: &mut MatchContext<'_, '_, '_, 'css>,
) -> bool {
    let key = (
        anchor,
        context
            .siblings
            .caches
            .selector_ids
            .intern(std::slice::from_ref(selector)),
    );
    if let Some(&matched) = context.siblings.caches.has_results.get(&key) {
        return matched;
    }
    let matched = match_relative_selector_uncached(selector, doc, anchor, context);
    context.siblings.caches.has_results.insert(key, matched);
    matched
}

fn match_relative_selector_uncached<'css>(
    selector: &Selector<'css>,
    doc: &Document,
    anchor: DomNodeId,
    context: &mut MatchContext<'_, '_, '_, 'css>,
) -> bool {
    let components = selector.iter_raw_match_order().as_slice();
    let explicitly_anchored = components
        .iter()
        .any(|component| matches!(component, Component::Scope));
    let context = &mut MatchContext {
        scope: Some(anchor),
        exclusive_ancestor_floor: (!explicitly_anchored).then_some(anchor),
        siblings: &mut *context.siblings,
    };
    let (relation, single_relation, candidate_components) = match components {
        [rest @ .., Component::Combinator(relation), Component::Scope] => {
            let single = !rest.iter().any(|component| component.is_combinator());
            // Traversal already guarantees this leading child/sibling relation.
            // Match its compound directly instead of walking back to :scope.
            let direct = single
                && matches!(
                    relation,
                    Combinator::Descendant
                        | Combinator::Child
                        | Combinator::NextSibling
                        | Combinator::LaterSibling
                );
            (*relation, single, if direct { rest } else { components })
        }
        _ if !explicitly_anchored => (
            Combinator::Descendant,
            !components.iter().any(|component| component.is_combinator()),
            components,
        ),
        // Source-authored :scope without the parser's leading relation keeps
        // its existing search semantics.
        _ => {
            return doc.nodes().any(|(node, value)| {
                matches!(value, NodeRef::Element(_))
                    && selector_matches_dom_node_with_context(selector, doc, node, context)
            });
        }
    };
    let following_siblings = matches!(relation, Combinator::NextSibling | Combinator::LaterSibling);
    if !following_siblings
        && context.siblings.reject_relative_subtree(
            candidate_components,
            anchor,
            relation == Combinator::Child && single_relation,
        )
    {
        return false;
    }
    let Some(anchor_element) = doc.try_element_ref(anchor) else {
        return false;
    };
    let root = if following_siblings {
        let Some(parent) = anchor_element
            .parent()
            .map(|parent| doc.element_ref(parent))
        else {
            return false;
        };
        parent
    } else {
        anchor_element
    };
    // Chained sibling selectors may end in a following sibling's subtree.
    let descend = relation == Combinator::Descendant || !single_relation;
    let adjacent_only = relation == Combinator::NextSibling && single_relation;
    let mut pending = std::mem::take(&mut context.siblings.caches.search_scratch);
    let mut matches_candidate = |candidate| {
        match_from_position_dom(candidate_components, doc, candidate, None, context)
            == MatchResult::Matched
    };
    let mut search = |pending: &mut Vec<DomNodeId>| {
        let mut after_anchor = !following_siblings;
        for child in root.child_elements() {
            if !after_anchor {
                after_anchor = child.node_id() == anchor;
                continue;
            }
            if matches_candidate(child.node_id()) {
                return true;
            }
            if descend {
                pending.extend(child.child_elements().map(|element| element.node_id()));
                while let Some(node) = pending.pop() {
                    if matches_candidate(node) {
                        return true;
                    }
                    if let Some(element) = doc.try_element_ref(node) {
                        pending.extend(element.child_elements().map(|element| element.node_id()));
                    }
                }
            }
            if adjacent_only {
                return false;
            }
        }
        false
    };
    let matched = search(&mut pending);
    pending.clear();
    if pending.capacity() > context.siblings.caches.search_scratch.capacity() {
        context.siblings.caches.search_scratch = pending;
    }
    matched
}

pub(super) fn attribute_value_matches(
    operator: AttrSelectorOperator,
    attribute_value: &str,
    selector_value: &str,
    case_sensitivity: CaseSensitivity,
) -> bool {
    // Selectors requires these four operators to match nothing when their
    // operand is empty. Parcel's generic string matcher intentionally does
    // not enforce that selector-level restriction (notably, splitting an
    // empty string for `~=` produces one empty token).
    if selector_value.is_empty()
        && matches!(
            operator,
            AttrSelectorOperator::Includes
                | AttrSelectorOperator::Prefix
                | AttrSelectorOperator::Suffix
                | AttrSelectorOperator::Substring
        )
    {
        return false;
    }
    operator.eval_str(attribute_value, selector_value, case_sensitivity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightningcss::selector::SelectorList;
    use lightningcss::stylesheet::ParserOptions;
    use lightningcss::traits::ParseWithOptions;

    #[test]
    fn empty_attribute_selector_operands_match_nothing() {
        for operator in [
            AttrSelectorOperator::Includes,
            AttrSelectorOperator::Prefix,
            AttrSelectorOperator::Suffix,
            AttrSelectorOperator::Substring,
        ] {
            assert!(!attribute_value_matches(
                operator,
                "",
                "",
                CaseSensitivity::CaseSensitive
            ));
            assert!(!attribute_value_matches(
                operator,
                "value",
                "",
                CaseSensitivity::CaseSensitive
            ));
        }

        // Equality and dash matching retain their ordinary empty-string
        // semantics; the special rule applies only to the four operators above.
        assert!(attribute_value_matches(
            AttrSelectorOperator::Equal,
            "",
            "",
            CaseSensitivity::CaseSensitive
        ));
        assert!(attribute_value_matches(
            AttrSelectorOperator::DashMatch,
            "",
            "",
            CaseSensitivity::CaseSensitive
        ));
    }

    #[test]
    fn enumerated_html_attribute_values_default_to_ascii_case_insensitive_matching() {
        let document =
            html_parse::parse_dom_document("<!doctype html><table rules='RoWs'></table>")
                .expect("valid standards HTML");
        let table = document
            .nodes()
            .find_map(|(id, node)| {
                matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "table")
                    .then_some(id)
            })
            .expect("table");
        let default =
            SelectorList::parse_string_with_options("table[rules=rows]", ParserOptions::default())
                .expect("selector parses");
        let explicit_sensitive = SelectorList::parse_string_with_options(
            "table[rules=rows s]",
            ParserOptions::default(),
        )
        .expect("selector parses");

        assert!(selector_matches_dom_node(
            default.0.first().expect("selector"),
            &document,
            table
        ));
        assert!(!selector_matches_dom_node(
            explicit_sensitive.0.first().expect("selector"),
            &document,
            table
        ));
    }

    #[test]
    fn language_ranges_use_selectors_four_extended_filtering() {
        assert!(language_range_matches("en", "EN"));
        assert!(language_range_matches("en-US", "en"));
        assert!(language_range_matches("sv-SE", "*"));
        assert!(language_range_matches("fr-Latn-FR", "fr-FR"));
        assert!(language_range_matches("fr-Latn-FR", "*-Latn"));
        assert!(language_range_matches("fr-Latn-FR", "*-FR"));
        assert!(language_range_matches("fr-Latn-FR-x-foobar", "fr-x-foobar"));
        assert!(language_range_matches("fr-Latn-FR-x-foobar", "*-x-foobar"));
        assert!(language_range_matches("en-GB-oed", "en-gb-oed"));
        assert!(
            !language_range_matches("de-x-DE", "de-DE"),
            "a singleton starts an extension boundary that cannot be skipped"
        );
        assert!(!language_range_matches("english", "en"));
        assert!(!language_range_matches("", "*"));
        assert!(!language_range_matches("en-US", "en-"));
    }

    #[test]
    fn type_selectors_preserve_case_in_xml_documents() {
        let parsed =
            html_parse::parse_xml_document("<root><Item/><item/></root>").expect("valid XML");
        let document = parsed.build_dom();
        let root = document
            .try_element_ref(document.dom_root().expect("root"))
            .expect("root element");
        let upper = root
            .children()
            .find(|&node| {
                document.try_element_ref(node).map(|element| element.tag()) == Some("Item")
            })
            .expect("upper-case element");
        let lower = root
            .children()
            .find(|&node| {
                document.try_element_ref(node).map(|element| element.tag()) == Some("item")
            })
            .expect("lower-case element");
        let selectors = SelectorList::parse_string_with_options("Item", ParserOptions::default())
            .expect("selector parses");
        let selector = selectors.0.first().expect("one selector");

        assert!(selector_matches_dom_node(selector, &document, upper));
        assert!(!selector_matches_dom_node(selector, &document, lower));
    }

    #[test]
    fn unprefixed_descendant_type_selector_matches_xhtml_elements() {
        let parsed = html_parse::parse_xml_document("<html xmlns='http://www.w3.org/1999/xhtml'><body><div><div id='target'/></div></body></html>").expect("valid XHTML");
        let document = parsed.build_dom();
        let root = document
            .try_element_ref(document.dom_root().expect("root"))
            .expect("root element");
        let body = root
            .children()
            .find(|&node| {
                document.try_element_ref(node).map(|element| element.tag()) == Some("body")
            })
            .expect("body");
        let outer = document
            .try_element_ref(body)
            .expect("body element")
            .children()
            .find(|&node| {
                document.try_element_ref(node).map(|element| element.tag()) == Some("div")
            })
            .expect("outer div");
        let target = document
            .try_element_ref(outer)
            .expect("outer element")
            .children()
            .find(|&node| {
                document
                    .try_element_ref(node)
                    .and_then(|element| element.id())
                    == Some("target")
            })
            .expect("target div");
        let selectors =
            SelectorList::parse_string_with_options("div div", ParserOptions::default())
                .expect("selector parses");
        let selector = selectors.0.first().expect("one selector");

        assert!(selector_matches_dom_node(selector, &document, target));
    }

    #[test]
    fn adjacent_sibling_selectors_ignore_intervening_text_nodes() {
        let parsed = html_parse::parse_html_document(
            "<!doctype html><html><body><img/> <img/></body></html>",
        );
        let document = parsed.build_dom();
        let body = document
            .try_element_ref(document.dom_root().expect("root"))
            .expect("html")
            .children()
            .find(|&node| {
                document.try_element_ref(node).map(|element| element.tag()) == Some("body")
            })
            .expect("body");
        let images = document
            .try_element_ref(body)
            .expect("body element")
            .children()
            .filter(|&node| {
                document.try_element_ref(node).map(|element| element.tag()) == Some("img")
            })
            .collect::<Vec<_>>();
        let selectors =
            SelectorList::parse_string_with_options("img + img", ParserOptions::default())
                .expect("selector parses");
        let selector = selectors.0.first().expect("one selector");

        assert!(!selector_matches_dom_node(selector, &document, images[0]));
        assert!(selector_matches_dom_node(selector, &document, images[1]));
    }

    #[test]
    fn nth_child_of_counts_only_siblings_matching_the_selector_list() {
        let document = html_parse::parse_dom_document("<!doctype html><ol><li class='eligible' id='first'></li><li></li><li class='eligible' id='second'></li><li class='eligible skip' id='third'></li></ol>").expect("valid standards HTML");
        let by_id = |id| {
            document
                .nodes()
                .find_map(|(node, value)| {
                    matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some(id))
                        .then_some(node)
                })
                .expect("test element")
        };
        let second_of_class = SelectorList::parse_string_with_options(
            "li:nth-child(2 of .eligible)",
            ParserOptions::default(),
        )
        .expect("selector parses");
        let last_not_skipped = SelectorList::parse_string_with_options(
            "li:nth-last-child(1 of :not(.skip))",
            ParserOptions::default(),
        )
        .expect("selector parses");
        let fourth_in_any_namespace = SelectorList::parse_string_with_options(
            "li:nth-child(4 of *|*)",
            ParserOptions::default(),
        )
        .expect("selector parses");

        assert!(!selector_matches_dom_node(
            second_of_class.0.first().expect("selector"),
            &document,
            by_id("first")
        ));
        assert!(selector_matches_dom_node(
            second_of_class.0.first().expect("selector"),
            &document,
            by_id("second")
        ));
        assert!(!selector_matches_dom_node(
            second_of_class.0.first().expect("selector"),
            &document,
            by_id("third")
        ));
        assert!(selector_matches_dom_node(
            last_not_skipped.0.first().expect("selector"),
            &document,
            by_id("second")
        ));
        assert!(selector_matches_dom_node(
            fourth_in_any_namespace.0.first().expect("selector"),
            &document,
            by_id("third")
        ));
    }

    #[test]
    fn nth_child_of_requires_the_subject_to_match_its_filter() {
        let document = html_parse::parse_dom_document(
            "<!doctype html><div><p class='other' id='target'></p><p class='eligible'></p></div>",
        )
        .expect("valid standards HTML");
        let target = document.nodes().find_map(|(node, value)| matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some("target")).then_some(node)).expect("target");
        let selectors = SelectorList::parse_string_with_options(
            "p:nth-child(1 of .eligible)",
            ParserOptions::default(),
        )
        .expect("selector parses");

        assert!(!selector_matches_dom_node(
            selectors.0.first().expect("selector"),
            &document,
            target
        ));
    }

    #[test]
    fn has_matches_relative_descendant_child_and_sibling_selectors() {
        let document = html_parse::parse_dom_document("<!doctype html><main><section id='anchor'><div><span></span></div></section><p id='next'></p><aside></aside></main>").expect("valid standards HTML");
        let by_id = |id| {
            document
                .nodes()
                .find_map(|(node, value)| {
                    matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some(id))
                        .then_some(node)
                })
                .expect("test element")
        };
        let anchor = by_id("anchor");

        for selector_text in [
            ":has(span)",
            ":has(div > span)",
            ":has(> div)",
            ":has(+ p)",
            ":has(~ aside)",
        ] {
            let selectors =
                SelectorList::parse_string_with_options(selector_text, ParserOptions::default())
                    .expect("selector parses");
            assert!(
                selector_matches_dom_node(
                    selectors.0.first().expect("selector"),
                    &document,
                    anchor
                ),
                "{selector_text}"
            );
        }
    }

    #[test]
    fn has_does_not_match_descendants_outside_its_anchor() {
        let document = html_parse::parse_dom_document("<!doctype html><div class='outside'><section id='anchor'><span></span></section></div>").expect("valid standards HTML");
        let anchor = document.nodes().find_map(|(node, value)| matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some("anchor")).then_some(node)).expect("anchor");
        let selectors = SelectorList::parse_string_with_options(
            ":has(.outside span)",
            ParserOptions::default(),
        )
        .expect("selector parses");

        assert!(!selector_matches_dom_node(
            selectors.0.first().expect("selector"),
            &document,
            anchor
        ));
    }

    #[test]
    fn has_composes_with_not_and_nth_child_of() {
        let document = html_parse::parse_dom_document("<!doctype html><main><div id='first'><span></span></div><div id='second'></div><div id='third'><span></span></div></main>").expect("valid standards HTML");
        let by_id = |id| {
            document
                .nodes()
                .find_map(|(node, value)| {
                    matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some(id))
                        .then_some(node)
                })
                .expect("test element")
        };
        let not_has = SelectorList::parse_string_with_options(
            "div:not(:has(span))",
            ParserOptions::default(),
        )
        .expect("selector parses");
        let second_with_span = SelectorList::parse_string_with_options(
            "div:nth-child(2 of :has(span))",
            ParserOptions::default(),
        )
        .expect("selector parses");

        assert!(selector_matches_dom_node(
            not_has.0.first().expect("selector"),
            &document,
            by_id("second")
        ));
        assert!(!selector_matches_dom_node(
            not_has.0.first().expect("selector"),
            &document,
            by_id("first")
        ));
        assert!(selector_matches_dom_node(
            second_with_span.0.first().expect("selector"),
            &document,
            by_id("third")
        ));
    }

    #[test]
    fn has_child_matches_only_elements_with_a_direct_matching_child() {
        let document = html_parse::parse_dom_document(
            "<!doctype html><div id='match'><span></span></div><div id='miss'></div>",
        )
        .expect("valid standards HTML");
        let selector =
            SelectorList::parse_string_with_options(":has(> span)", ParserOptions::default())
                .expect("selector parses");
        assert!(crate::style::syntax::selectors::selector_list_is_web_valid(
            &selector
        ));
        let matching_tags_and_ids = document
            .nodes()
            .filter_map(|(node, value)| match value {
                html_dom::NodeRef::Element(element)
                    if selector_matches_dom_node(
                        selector.0.first().expect("selector"),
                        &document,
                        node,
                    ) =>
                {
                    Some((element.tag().to_owned(), element.id().map(str::to_owned)))
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            matching_tags_and_ids,
            [("div".to_owned(), Some("match".to_owned()))]
        );
    }

    #[test]
    fn relational_selector_specificity_composes_through_selector_lists() {
        let specificity = |text| {
            let selectors = SelectorList::parse_string_with_options(text, ParserOptions::default())
                .expect("selector parses");
            selector_specificity(selectors.0.first().expect("selector"))
        };

        assert_eq!(specificity(":has(#target, .fallback)"), 1 << 20);
        assert_eq!(specificity(":is(:has(#target), .fallback)"), 1 << 20);
        assert_eq!(specificity(":not(:has(#target), .fallback)"), 1 << 20);
        assert_eq!(specificity(":where(:has(#target))"), 0);
        assert_eq!(
            specificity(":nth-child(1 of :has(#target))"),
            (1 << 20) | (1 << 10)
        );
    }

    #[test]
    fn scope_matches_the_document_element_without_an_explicit_scope() {
        let document = html_parse::parse_dom_document("<!doctype html><html><body></body></html>")
            .expect("valid standards HTML");
        let root = document.dom_root().expect("document element");
        let body = document
            .try_element_ref(root)
            .expect("html")
            .children()
            .find(|&node| {
                document.try_element_ref(node).map(|element| element.tag()) == Some("body")
            })
            .expect("body");
        let selectors = SelectorList::parse_string_with_options(":scope", ParserOptions::default())
            .expect("selector parses");

        assert!(selector_matches_dom_node(
            selectors.0.first().expect("selector"),
            &document,
            root
        ));
        assert!(!selector_matches_dom_node(
            selectors.0.first().expect("selector"),
            &document,
            body
        ));
    }

    fn selector_matches_id(document: &Document, id: &str, selector_text: &str) -> bool {
        let node = document
            .node_ids()
            .find(|node| {
                document
                    .try_element_ref(*node)
                    .and_then(|element| element.id())
                    == Some(id)
            })
            .unwrap_or_else(|| panic!("missing fixture element #{id}"));
        let selectors =
            SelectorList::parse_string_with_options(selector_text, ParserOptions::default())
                .expect("selector parses");
        selector_matches_dom_node(selectors.0.first().expect("selector"), document, node)
    }

    #[test]
    fn static_html_control_states_are_normalized_before_selector_matching() {
        let document = html_parse::parse_dom_document(
            "<!doctype html><form><input id='required' required><input id='optional'><input id='checked' type='checkbox' checked><option id='selected' selected></option><textarea id='readonly' readonly></textarea><textarea id='writable'></textarea></form>",
        )
        .expect("valid standards HTML");

        for (id, selector) in [
            ("required", ":required:enabled"),
            ("optional", ":optional:enabled"),
            ("checked", ":checked:default"),
            ("selected", ":checked:default"),
            ("readonly", ":read-only"),
            ("writable", ":read-write"),
        ] {
            assert!(
                selector_matches_id(&document, id, selector),
                "{id} must match {selector}"
            );
        }
    }

    #[test]
    fn disabled_fieldset_exempts_only_its_first_legend_subtree() {
        let document = html_parse::parse_dom_document(
            "<!doctype html><fieldset disabled><legend><input id='legend-input'></legend><legend><input id='second-legend'></legend><input id='disabled-input'><select><optgroup disabled><option id='disabled-option'></option></optgroup></select></fieldset>",
        )
        .expect("valid standards HTML");

        assert!(selector_matches_id(&document, "legend-input", ":enabled"));
        assert!(selector_matches_id(&document, "second-legend", ":disabled"));
        assert!(selector_matches_id(
            &document,
            "disabled-input",
            ":disabled"
        ));
        assert!(selector_matches_id(
            &document,
            "disabled-option",
            ":disabled"
        ));
    }

    #[test]
    fn auto_direction_and_bdi_are_available_to_direction_selectors() {
        let document = html_parse::parse_dom_document("<!doctype html><main dir='rtl'><p dir='auto' id='hebrew'><bdi>abc</bdi>אבג<span id='child'></span></p><bdi id='latin'>abc</bdi></main>").unwrap();
        assert!(selector_matches_id(&document, "hebrew", ":dir(rtl)"));
        assert!(selector_matches_id(&document, "child", ":dir(rtl)"));
        assert!(selector_matches_id(&document, "latin", ":dir(ltr)"));
    }

    #[test]
    fn inherited_direction_and_open_state_are_available_to_relational_selectors() {
        let document =
            html_parse::parse_dom_document("<!doctype html><main dir='rtl'><section id='owner'><span id='inherited'></span></section><details id='open' open></details><details id='closed'></details></main>").expect("valid standards HTML");

        assert!(selector_matches_id(&document, "inherited", ":dir(rtl)"));
        assert!(selector_matches_id(
            &document,
            "owner",
            ":has(span:dir(rtl))"
        ));
        assert!(selector_matches_id(&document, "open", ":open"));
        assert!(selector_matches_id(&document, "closed", ":closed"));
    }

    #[test]
    fn radio_indeterminate_state_is_resolved_per_static_group() {
        let document =
            html_parse::parse_dom_document("<!doctype html><form><input id='a1' type='radio' name='a'><input id='a2' type='radio' name='a'><input id='b1' type='radio' name='b'><input id='b2' type='radio' name='b' checked></form>")
                .expect("valid standards HTML");

        assert!(selector_matches_id(&document, "a1", ":indeterminate"));
        assert!(selector_matches_id(&document, "a2", ":indeterminate"));
        assert!(!selector_matches_id(&document, "b1", ":indeterminate"));
        assert!(selector_matches_id(&document, "b2", ":checked"));
    }

    #[test]
    fn html_state_does_not_leak_to_same_named_foreign_xml_elements() {
        let document = html_parse::parse_xml_document("<root><input id='input' required='required'/><details id='details' open='open'/></root>").expect("valid XML").build_dom();

        assert!(!selector_matches_id(&document, "input", ":required"));
        assert!(!selector_matches_id(&document, "details", ":open"));
    }
}

/// Match the tree-structural `:first-child`/`:last-child`/`:only-child` and
/// `:nth-*` selectors (all are `Component::Nth` with an `an+b` formula).
fn nth_matches_dom<'css>(
    nth: &NthSelectorData,
    element: ElementRef<'_>,
    context: &mut MatchContext<'_, '_, '_, 'css>,
) -> bool {
    if matches!(nth.ty, NthType::Col | NthType::LastCol) {
        return false;
    }
    if !nth.ty.is_of_type()
        && let Some(matched) = context.siblings.matches_nth_child(nth, element.node_id())
    {
        return matched;
    }
    let from_end = nth.ty.is_from_end();
    let index = TraversalContext::nth_index(element, &[], nth.ty.is_of_type(), from_end, context);
    index > 0
        && if nth.ty.is_only() {
            index == 1
                && TraversalContext::nth_index(
                    element,
                    &[],
                    nth.ty.is_of_type(),
                    !from_end,
                    context,
                ) == 1
        } else {
            nth_index_matches(nth.a, nth.b, index)
        }
}

/// Selectors 4 `:nth-child(An+B of S)` counts the subject only among element
/// siblings matching the forgiving selector list `S`.
fn nth_of_matches_dom<'css>(
    nth: &NthSelectorData,
    selectors: &[Selector<'css>],
    element: ElementRef<'_>,
    context: &mut MatchContext<'_, '_, '_, 'css>,
) -> bool {
    let index =
        TraversalContext::nth_index(element, selectors, false, nth.ty.is_from_end(), context);
    index > 0 && nth_index_matches(nth.a, nth.b, index)
}

/// True if the 1-based `index` is in the set `{ a*k + b | integer k >= 0 }`.
fn nth_index_matches(a: i32, b: i32, index: i32) -> bool {
    if a == 0 {
        return index == b;
    }
    let diff = index - b;
    diff % a == 0 && diff / a >= 0
}

fn local_name_string<'a>(
    name: &'a impl AsRef<str>,
    lower_name: &'a impl AsRef<str>,
    is_html_element_in_html_document: bool,
) -> &'a str {
    let name = name.as_ref();
    let lower_name = lower_name.as_ref();
    if is_html_element_in_html_document || name == lower_name {
        lower_name
    } else {
        name
    }
}

pub(super) fn parsed_case_sensitivity(
    parsed: ParsedCaseSensitivity,
    is_html_element_in_html_document: bool,
    attribute_name: Option<&str>,
) -> CaseSensitivity {
    if is_html_element_in_html_document
        && matches!(parsed, ParsedCaseSensitivity::CaseSensitive)
        && attribute_name.is_some_and(html_attribute_value_is_ascii_case_insensitive)
    {
        return CaseSensitivity::AsciiCaseInsensitive;
    }
    parsed.to_unconditional(is_html_element_in_html_document)
}

fn html_attribute_value_is_ascii_case_insensitive(name: &str) -> bool {
    matches!(
        name,
        "accept"
            | "align"
            | "alink"
            | "axis"
            | "bgcolor"
            | "charset"
            | "checked"
            | "clear"
            | "codetype"
            | "color"
            | "compact"
            | "declare"
            | "defer"
            | "dir"
            | "direction"
            | "disabled"
            | "enctype"
            | "face"
            | "frame"
            | "hreflang"
            | "http-equiv"
            | "lang"
            | "language"
            | "link"
            | "media"
            | "method"
            | "multiple"
            | "nohref"
            | "noresize"
            | "noshade"
            | "nowrap"
            | "readonly"
            | "rel"
            | "rev"
            | "rules"
            | "scope"
            | "scrolling"
            | "selected"
            | "shape"
            | "target"
            | "text"
            | "type"
            | "valign"
            | "valuetype"
            | "vlink"
    )
}

#[derive(Clone, Copy)]
enum NamespaceMatch<'a> {
    None,
    Any,
    Specific(&'a str),
}

fn namespace_match<'a>(
    namespace: Option<NamespaceConstraint<&'a lightningcss::values::string::CowArcStr<'a>>>,
) -> NamespaceMatch<'a> {
    match namespace {
        None => NamespaceMatch::None,
        Some(NamespaceConstraint::Any) => NamespaceMatch::Any,
        Some(NamespaceConstraint::Specific(url)) if url.as_ref().is_empty() => NamespaceMatch::None,
        Some(NamespaceConstraint::Specific(url)) => NamespaceMatch::Specific(url.as_ref()),
    }
}

fn get_attribute_in_namespace<'a>(
    element: html_dom::ElementRef<'a>,
    namespace: NamespaceMatch<'_>,
    local_name: &str,
) -> Option<&'a str> {
    match namespace {
        NamespaceMatch::None => element.attr(local_name),
        NamespaceMatch::Any => element.attr_in_any_namespace(local_name),
        NamespaceMatch::Specific(url) => element.attr_expanded(Some(url), local_name),
    }
}

fn attribute_exists_in_namespace(
    element: html_dom::ElementRef<'_>,
    namespace: NamespaceMatch<'_>,
    local_name: &str,
) -> bool {
    get_attribute_in_namespace(element, namespace, local_name).is_some()
}

/// Check if pseudo-class matches DOM node
pub(super) fn pseudo_matches_dom(
    pseudo: &PseudoClass,
    doc: &Document,
    element: html_dom::ElementRef<'_>,
    siblings: &mut TraversalContext<'_, '_, '_>,
) -> bool {
    let state = element.html_state();
    match pseudo {
        PseudoClass::Link | PseudoClass::AnyLink(_) => state.is_link(),
        PseudoClass::Visited => false, // Don't match visited for privacy
        PseudoClass::Lang { languages } => siblings
            .effective_language(element.node_id())
            .unwrap_or_else(|| effective_language(doc, element))
            .is_some_and(|language| {
                languages
                    .iter()
                    .any(|range| language_range_matches(language, range.as_ref()))
            }),
        PseudoClass::Dir {
            direction: Direction::Ltr,
        } => !state.is_rtl(),
        PseudoClass::Dir {
            direction: Direction::Rtl,
        } => state.is_rtl(),
        PseudoClass::Enabled => state.is_enabled(),
        PseudoClass::Disabled => state.is_disabled(),
        PseudoClass::ReadOnly(_) => state.is_read_only(),
        PseudoClass::ReadWrite(_) => state.is_read_write(),
        PseudoClass::PlaceholderShown(_) => state.is_placeholder_shown(),
        PseudoClass::Default => state.is_default(),
        PseudoClass::Checked => state.is_checked(),
        PseudoClass::Indeterminate => state.is_indeterminate(),
        PseudoClass::Required => state.is_required(),
        PseudoClass::Optional => state.is_optional(),
        PseudoClass::Open => state.is_open(),
        PseudoClass::Closed => state.is_closed(),
        PseudoClass::Defined => state.is_defined(),
        _ => false,
    }
}

const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

pub(super) fn element_language<'a>(element: ElementRef<'a>) -> Option<&'a str> {
    element
        .attr_expanded(Some(XML_NAMESPACE), "lang")
        .or_else(|| element.attr("lang"))
}

/// Resolve the language of an element according to document semantics.  An
/// explicitly empty language stops inheritance and intentionally matches no
/// non-empty `:lang()` range.
fn effective_language<'a>(doc: &'a Document, element: html_dom::ElementRef<'a>) -> Option<&'a str> {
    let mut current = Some(element.node_id());
    while let Some(node) = current {
        let candidate = doc.try_element_ref(node)?;
        if let Some(language) = element_language(candidate) {
            return Some(language);
        }
        current = candidate.parent();
    }
    None
}

fn language_range_matches(language: &str, range: &str) -> bool {
    let mut language_subtags = language.split('-');
    let mut range_subtags = range.split('-');
    let Some(first_language) = language_subtags.next().filter(|subtag| !subtag.is_empty()) else {
        return false;
    };
    let Some(first_range) = range_subtags.next().filter(|subtag| !subtag.is_empty()) else {
        return false;
    };
    if first_range != "*" && !first_language.eq_ignore_ascii_case(first_range) {
        return false;
    }

    // Selectors 4 uses RFC 4647 extended filtering. Non-wildcard range
    // subtags may skip ordinary language subtags, but never cross a singleton
    // extension boundary such as `x` unless that singleton itself matched.
    for range_subtag in range_subtags {
        if range_subtag.is_empty() {
            return false;
        }
        if range_subtag == "*" {
            continue;
        }
        loop {
            let Some(language_subtag) = language_subtags.next() else {
                return false;
            };
            if language_subtag.eq_ignore_ascii_case(range_subtag) {
                break;
            }
            if language_subtag.len() == 1 {
                return false;
            }
        }
    }
    true
}
