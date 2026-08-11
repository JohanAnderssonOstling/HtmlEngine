use super::selectors::AncestorFilter;
use html_dom::{Document, DomNodeId};
use lightningcss::selector::{Combinator, Component, Direction, PseudoClass, PseudoElement, Selector};
use parcel_selectors::attr::{AttrSelectorOperator, CaseSensitivity, NamespaceConstraint, ParsedAttrSelectorOperation, ParsedCaseSensitivity};
use parcel_selectors::parser::{NthSelectorData, NthType};

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

#[cfg(test)]
pub fn selector_matches_dom_node(selector: &Selector, doc: &Document, node_idx: DomNodeId) -> bool {
    match_from_position_dom(selector.iter_raw_match_order().as_slice(), doc, node_idx, None, MatchContext::default())
}

pub(crate) fn selector_matches_dom_node_in_scope(selector: &Selector, doc: &Document, node_idx: DomNodeId, scope: DomNodeId) -> bool {
    match_from_position_dom(selector.iter_raw_match_order().as_slice(), doc, node_idx, None, MatchContext { scope: Some(scope), exclusive_ancestor_floor: None })
}

pub(crate) fn selector_matches_dom_pseudo_in_scope(selector: &Selector, doc: &Document, node_idx: DomNodeId, pseudo: PseudoTarget, scope: DomNodeId) -> bool {
    selector_matches_dom_pseudo_with_context(selector, doc, node_idx, pseudo, MatchContext { scope: Some(scope), exclusive_ancestor_floor: None })
}

fn selector_matches_dom_pseudo_with_context(selector: &Selector, doc: &Document, node_idx: DomNodeId, pseudo: PseudoTarget, context: MatchContext) -> bool {
    let has_target = selector.iter_raw_match_order().any(|component| {
        matches!(
            (component, pseudo),
            (Component::PseudoElement(PseudoElement::Before), PseudoTarget::Before)
                | (Component::PseudoElement(PseudoElement::After), PseudoTarget::After)
                | (Component::PseudoElement(PseudoElement::FirstLine), PseudoTarget::FirstLine)
                | (Component::PseudoElement(PseudoElement::FirstLetter), PseudoTarget::FirstLetter)
        )
    });
    if !has_target {
        return false;
    }
    match_from_position_dom(selector.iter_raw_match_order().as_slice(), doc, node_idx, Some(pseudo), context)
}

#[derive(Clone, Copy, Default)]
struct MatchContext {
    /// The element represented by `:scope`. Outside relative selector
    /// matching, `:scope` defaults to the document element.
    scope: Option<DomNodeId>,
    /// An implicit relative selector is scoped to descendants of its anchor.
    /// Ancestor combinators must not escape that subtree or match the anchor
    /// itself (the omitted leading combinator is `:scope `).
    exclusive_ancestor_floor: Option<DomNodeId>,
}

/// Match selector components from a position against a DOM node
fn match_from_position_dom<'i>(components: &[Component<'i>], doc: &Document, node_idx: DomNodeId, pseudo: Option<PseudoTarget>, context: MatchContext) -> bool {
    let mut pos = 0;

    // Match components in current sequence
    while pos < components.len() {
        match &components[pos] {
            Component::Combinator(combinator) => {
                // Found combinator, recurse with appropriate ancestor/sibling
                let remaining = &components[pos + 1..];
                return match combinator {
                    // Parcel selectors inserts this internal boundary between
                    // a pseudo-element and its originating element. Both are
                    // matched against the same DOM node; only the pseudo side
                    // receives the requested pseudo target.
                    Combinator::PseudoElement => match_from_position_dom(remaining, doc, node_idx, None, context),
                    Combinator::Child => doc
                        .element_ref(node_idx)
                        .and_then(|element| element.parent())
                        .filter(|parent_idx| Some(*parent_idx) != context.exclusive_ancestor_floor)
                        .is_some_and(|parent_idx| match_from_position_dom(remaining, doc, parent_idx, pseudo, context)),
                    Combinator::Descendant => {
                        for ancestor_idx in doc.dom_ancestors(node_idx) {
                            if Some(ancestor_idx) == context.exclusive_ancestor_floor {
                                break;
                            }
                            if match_from_position_dom(remaining, doc, ancestor_idx, pseudo, context) {
                                return true;
                            }
                        }
                        false
                    }
                    Combinator::NextSibling => doc.element_ref(node_idx).and_then(|element| element.previous_element_sibling()).is_some_and(|sibling| match_from_position_dom(remaining, doc, sibling.node_id(), pseudo, context)),
                    Combinator::LaterSibling => {
                        let mut current = doc.element_ref(node_idx).and_then(|element| element.previous_element_sibling());
                        while let Some(sibling) = current {
                            if match_from_position_dom(remaining, doc, sibling.node_id(), pseudo, context) {
                                return true;
                            }
                            current = sibling.previous_element_sibling();
                        }
                        false
                    }
                    _ => false,
                };
            }
            component => {
                if !component_matches_dom(component, doc, node_idx, pseudo, context) {
                    return false;
                }
                pos += 1;
            }
        }
    }

    true // All components matched
}

/// Check if a single selector component matches a DOM node
fn component_matches_dom(component: &Component, doc: &Document, node_idx: DomNodeId, pseudo: Option<PseudoTarget>, context: MatchContext) -> bool {
    let Some(element) = doc.element_ref(node_idx) else {
        return false;
    };
    match component {
        Component::ExplicitAnyNamespace => true,
        Component::ExplicitNoNamespace => element.namespace().is_none(),
        Component::DefaultNamespace(namespace) | Component::Namespace(_, namespace) => element.namespace() == Some(namespace.as_ref()),
        Component::ExplicitUniversalType => true,

        Component::LocalName(name) => {
            let selector_name = if element.is_html_element_in_html_document() { name.lower_name.0.as_ref() } else { name.name.0.as_ref() };
            element.tag() == selector_name
        }

        Component::ID(id) => element.id().is_some_and(|node_id| node_id == id.0.as_ref()),

        Component::Class(class_selector) => element.has_class(class_selector.0.as_ref()),

        Component::AttributeInNoNamespaceExists { local_name, local_name_lower } => {
            let attr_name = local_name_string(local_name, local_name_lower, element.is_html_element_in_html_document());
            element.has_attr(attr_name)
        }

        Component::AttributeInNoNamespace { local_name, operator, value, case_sensitivity, .. } => {
            let attr_name = local_name_string(local_name, local_name, element.is_html_element_in_html_document());
            let Some(attr_value) = element.attr(attr_name) else {
                return false;
            };

            attribute_value_matches(*operator, attr_value, value.as_ref(), parsed_case_sensitivity(*case_sensitivity, element.is_html_element_in_html_document(), Some(attr_name)))
        }

        Component::AttributeOther(other) => {
            let local_name = other.local_name.as_ref();
            match &other.operation {
                ParsedAttrSelectorOperation::Exists => attribute_exists_in_namespace(element, namespace_match(other.namespace()), local_name),
                ParsedAttrSelectorOperation::WithValue { operator, case_sensitivity, expected_value } => {
                    let Some(attr_value) = get_attribute_in_namespace(element, namespace_match(other.namespace()), local_name) else {
                        return false;
                    };

                    let html_attribute = matches!(namespace_match(other.namespace()), NamespaceMatch::None).then_some(local_name);
                    attribute_value_matches(*operator, attr_value, expected_value.as_ref(), parsed_case_sensitivity(*case_sensitivity, element.is_html_element_in_html_document(), html_attribute))
                }
            }
        }

        Component::NonTSPseudoClass(pseudo) => pseudo_matches_dom(pseudo, doc, element),
        Component::PseudoElement(PseudoElement::Before) => pseudo == Some(PseudoTarget::Before),
        Component::PseudoElement(PseudoElement::After) => pseudo == Some(PseudoTarget::After),
        Component::PseudoElement(PseudoElement::FirstLine) => pseudo == Some(PseudoTarget::FirstLine),
        Component::PseudoElement(PseudoElement::FirstLetter) => pseudo == Some(PseudoTarget::FirstLetter),
        Component::PseudoElement(_) => false,

        Component::Root => doc.dom_root() == Some(node_idx),
        Component::Scope => context.scope.or_else(|| doc.dom_root()) == Some(node_idx),
        // Selectors L3 `:empty`: no children at all (text nodes count as content).
        Component::Empty => element.children().next().is_none(),
        Component::Nth(nth) => nth_matches_dom(nth, doc, element),
        Component::NthOf(nth) => nth_of_matches_dom(nth.nth_data(), nth.selectors(), doc, element, context),

        // Forgiving (`:is()`) and non-specificity (`:where()`) matching: the
        // node matches if any inner selector matches it. The UA stylesheet uses
        // these for nested-list markers and margins.
        Component::Is(selectors) | Component::Where(selectors) => selectors.iter().any(|selector| selector_matches_dom_node_with_context(selector, doc, node_idx, context)),
        // `:not()` matches when none of the inner selectors match.
        Component::Negation(selectors) => !selectors.iter().any(|selector| selector_matches_dom_node_with_context(selector, doc, node_idx, context)),
        Component::Has(selectors) => selectors.iter().any(|selector| relative_selector_matches_dom_node(selector, doc, node_idx)),

        _ => false,
    }
}

fn selector_matches_dom_node_with_context(selector: &Selector, doc: &Document, node_idx: DomNodeId, context: MatchContext) -> bool {
    match_from_position_dom(selector.iter_raw_match_order().as_slice(), doc, node_idx, None, context)
}

/// Match a selector from `:has()` relative to its anchor element. Parcel
/// represents an explicit leading combinator by appending `:scope` to the raw
/// match-order components. Without one, the selector has an implicit
/// descendant combinator and is limited to the anchor's descendant subtree.
fn relative_selector_matches_dom_node(selector: &Selector, doc: &Document, anchor: DomNodeId) -> bool {
    let explicitly_anchored = selector.iter_raw_match_order().any(|component| matches!(component, Component::Scope));
    let context = MatchContext { scope: Some(anchor), exclusive_ancestor_floor: (!explicitly_anchored).then_some(anchor) };

    doc.nodes().any(|(candidate, node)| {
        if !matches!(node, html_dom::NodeRef::Element(_)) {
            return false;
        }
        if !explicitly_anchored && !doc.dom_ancestors(candidate).any(|ancestor| ancestor == anchor) {
            return false;
        }
        selector_matches_dom_node_with_context(selector, doc, candidate, context)
    })
}

fn attribute_value_matches(operator: AttrSelectorOperator, attribute_value: &str, selector_value: &str, case_sensitivity: CaseSensitivity) -> bool {
    // Selectors requires these four operators to match nothing when their
    // operand is empty. Parcel's generic string matcher intentionally does
    // not enforce that selector-level restriction (notably, splitting an
    // empty string for `~=` produces one empty token).
    if selector_value.is_empty() && matches!(operator, AttrSelectorOperator::Includes | AttrSelectorOperator::Prefix | AttrSelectorOperator::Suffix | AttrSelectorOperator::Substring) {
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
        for operator in [AttrSelectorOperator::Includes, AttrSelectorOperator::Prefix, AttrSelectorOperator::Suffix, AttrSelectorOperator::Substring] {
            assert!(!attribute_value_matches(operator, "", "", CaseSensitivity::CaseSensitive));
            assert!(!attribute_value_matches(operator, "value", "", CaseSensitivity::CaseSensitive));
        }

        // Equality and dash matching retain their ordinary empty-string
        // semantics; the special rule applies only to the four operators above.
        assert!(attribute_value_matches(AttrSelectorOperator::Equal, "", "", CaseSensitivity::CaseSensitive));
        assert!(attribute_value_matches(AttrSelectorOperator::DashMatch, "", "", CaseSensitivity::CaseSensitive));
    }

    #[test]
    fn enumerated_html_attribute_values_default_to_ascii_case_insensitive_matching() {
        let document = html_parse::parse_dom_document("<!doctype html><table rules='RoWs'></table>").expect("valid standards HTML");
        let table = document.nodes().find_map(|(id, node)| matches!(node, html_dom::NodeRef::Element(element) if element.tag() == "table").then_some(id)).expect("table");
        let default = SelectorList::parse_string_with_options("table[rules=rows]", ParserOptions::default()).expect("selector parses");
        let explicit_sensitive = SelectorList::parse_string_with_options("table[rules=rows s]", ParserOptions::default()).expect("selector parses");

        assert!(selector_matches_dom_node(default.0.first().expect("selector"), &document, table));
        assert!(!selector_matches_dom_node(explicit_sensitive.0.first().expect("selector"), &document, table));
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
        assert!(!language_range_matches("de-x-DE", "de-DE"), "a singleton starts an extension boundary that cannot be skipped");
        assert!(!language_range_matches("english", "en"));
        assert!(!language_range_matches("", "*"));
        assert!(!language_range_matches("en-US", "en-"));
    }

    #[test]
    fn type_selectors_preserve_case_in_xml_documents() {
        let parsed = html_parse::parse_xml_document("<root><Item/><item/></root>").expect("valid XML");
        let document = parsed.build_dom();
        let root = document.element_ref(document.dom_root().expect("root")).expect("root element");
        let upper = root.children().find(|&node| document.get_dom_tag(node) == Some("Item")).expect("upper-case element");
        let lower = root.children().find(|&node| document.get_dom_tag(node) == Some("item")).expect("lower-case element");
        let selectors = SelectorList::parse_string_with_options("Item", ParserOptions::default()).expect("selector parses");
        let selector = selectors.0.first().expect("one selector");

        assert!(selector_matches_dom_node(selector, &document, upper));
        assert!(!selector_matches_dom_node(selector, &document, lower));
    }

    #[test]
    fn unprefixed_descendant_type_selector_matches_xhtml_elements() {
        let parsed = html_parse::parse_xml_document("<html xmlns='http://www.w3.org/1999/xhtml'><body><div><div id='target'/></div></body></html>").expect("valid XHTML");
        let document = parsed.build_dom();
        let root = document.element_ref(document.dom_root().expect("root")).expect("root element");
        let body = root.children().find(|&node| document.get_dom_tag(node) == Some("body")).expect("body");
        let outer = document.element_ref(body).expect("body element").children().find(|&node| document.get_dom_tag(node) == Some("div")).expect("outer div");
        let target = document.element_ref(outer).expect("outer element").children().find(|&node| document.element_ref(node).and_then(|element| element.id()) == Some("target")).expect("target div");
        let selectors = SelectorList::parse_string_with_options("div div", ParserOptions::default()).expect("selector parses");
        let selector = selectors.0.first().expect("one selector");

        assert!(selector_matches_dom_node(selector, &document, target));
    }

    #[test]
    fn adjacent_sibling_selectors_ignore_intervening_text_nodes() {
        let parsed = html_parse::parse_html_document("<!doctype html><html><body><img/> <img/></body></html>");
        let document = parsed.build_dom();
        let body = document.element_ref(document.dom_root().expect("root")).expect("html").children().find(|&node| document.get_dom_tag(node) == Some("body")).expect("body");
        let images = document.element_ref(body).expect("body element").children().filter(|&node| document.get_dom_tag(node) == Some("img")).collect::<Vec<_>>();
        let selectors = SelectorList::parse_string_with_options("img + img", ParserOptions::default()).expect("selector parses");
        let selector = selectors.0.first().expect("one selector");

        assert!(!selector_matches_dom_node(selector, &document, images[0]));
        assert!(selector_matches_dom_node(selector, &document, images[1]));
    }

    #[test]
    fn nth_child_of_counts_only_siblings_matching_the_selector_list() {
        let document = html_parse::parse_dom_document("<!doctype html><ol><li class='eligible' id='first'></li><li></li><li class='eligible' id='second'></li><li class='eligible skip' id='third'></li></ol>").expect("valid standards HTML");
        let by_id = |id| document.nodes().find_map(|(node, value)| matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some(id)).then_some(node)).expect("test element");
        let second_of_class = SelectorList::parse_string_with_options("li:nth-child(2 of .eligible)", ParserOptions::default()).expect("selector parses");
        let last_not_skipped = SelectorList::parse_string_with_options("li:nth-last-child(1 of :not(.skip))", ParserOptions::default()).expect("selector parses");
        let fourth_in_any_namespace = SelectorList::parse_string_with_options("li:nth-child(4 of *|*)", ParserOptions::default()).expect("selector parses");

        assert!(!selector_matches_dom_node(second_of_class.0.first().expect("selector"), &document, by_id("first")));
        assert!(selector_matches_dom_node(second_of_class.0.first().expect("selector"), &document, by_id("second")));
        assert!(!selector_matches_dom_node(second_of_class.0.first().expect("selector"), &document, by_id("third")));
        assert!(selector_matches_dom_node(last_not_skipped.0.first().expect("selector"), &document, by_id("second")));
        assert!(selector_matches_dom_node(fourth_in_any_namespace.0.first().expect("selector"), &document, by_id("third")));
    }

    #[test]
    fn nth_child_of_requires_the_subject_to_match_its_filter() {
        let document = html_parse::parse_dom_document("<!doctype html><div><p class='other' id='target'></p><p class='eligible'></p></div>").expect("valid standards HTML");
        let target = document.nodes().find_map(|(node, value)| matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some("target")).then_some(node)).expect("target");
        let selectors = SelectorList::parse_string_with_options("p:nth-child(1 of .eligible)", ParserOptions::default()).expect("selector parses");

        assert!(!selector_matches_dom_node(selectors.0.first().expect("selector"), &document, target));
    }

    #[test]
    fn has_matches_relative_descendant_child_and_sibling_selectors() {
        let document = html_parse::parse_dom_document("<!doctype html><main><section id='anchor'><div><span></span></div></section><p id='next'></p><aside></aside></main>").expect("valid standards HTML");
        let by_id = |id| document.nodes().find_map(|(node, value)| matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some(id)).then_some(node)).expect("test element");
        let anchor = by_id("anchor");

        for selector_text in [":has(span)", ":has(div > span)", ":has(> div)", ":has(+ p)", ":has(~ aside)"] {
            let selectors = SelectorList::parse_string_with_options(selector_text, ParserOptions::default()).expect("selector parses");
            assert!(selector_matches_dom_node(selectors.0.first().expect("selector"), &document, anchor), "{selector_text}");
        }
    }

    #[test]
    fn has_does_not_match_descendants_outside_its_anchor() {
        let document = html_parse::parse_dom_document("<!doctype html><div class='outside'><section id='anchor'><span></span></section></div>").expect("valid standards HTML");
        let anchor = document.nodes().find_map(|(node, value)| matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some("anchor")).then_some(node)).expect("anchor");
        let selectors = SelectorList::parse_string_with_options(":has(.outside span)", ParserOptions::default()).expect("selector parses");

        assert!(!selector_matches_dom_node(selectors.0.first().expect("selector"), &document, anchor));
    }

    #[test]
    fn has_composes_with_not_and_nth_child_of() {
        let document = html_parse::parse_dom_document("<!doctype html><main><div id='first'><span></span></div><div id='second'></div><div id='third'><span></span></div></main>").expect("valid standards HTML");
        let by_id = |id| document.nodes().find_map(|(node, value)| matches!(value, html_dom::NodeRef::Element(element) if element.id() == Some(id)).then_some(node)).expect("test element");
        let not_has = SelectorList::parse_string_with_options("div:not(:has(span))", ParserOptions::default()).expect("selector parses");
        let second_with_span = SelectorList::parse_string_with_options("div:nth-child(2 of :has(span))", ParserOptions::default()).expect("selector parses");

        assert!(selector_matches_dom_node(not_has.0.first().expect("selector"), &document, by_id("second")));
        assert!(!selector_matches_dom_node(not_has.0.first().expect("selector"), &document, by_id("first")));
        assert!(selector_matches_dom_node(second_with_span.0.first().expect("selector"), &document, by_id("third")));
    }

    #[test]
    fn has_child_matches_only_elements_with_a_direct_matching_child() {
        let document = html_parse::parse_dom_document("<!doctype html><div id='match'><span></span></div><div id='miss'></div>").expect("valid standards HTML");
        let selector = SelectorList::parse_string_with_options(":has(> span)", ParserOptions::default()).expect("selector parses");
        assert!(crate::style::matching::selectors::selector_list_is_web_valid(&selector));
        assert!(selector_might_match_with_filter(selector.0.first().expect("selector"), &AncestorFilter::default()));
        let matching_tags_and_ids = document
            .nodes()
            .filter_map(|(node, value)| match value {
                html_dom::NodeRef::Element(element) if selector_matches_dom_node(selector.0.first().expect("selector"), &document, node) => Some((element.tag().to_owned(), element.id().map(str::to_owned))),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(matching_tags_and_ids, [("div".to_owned(), Some("match".to_owned()))]);
    }

    #[test]
    fn relational_selector_specificity_composes_through_selector_lists() {
        let specificity = |text| {
            let selectors = SelectorList::parse_string_with_options(text, ParserOptions::default()).expect("selector parses");
            selector_specificity(selectors.0.first().expect("selector"))
        };

        assert_eq!(specificity(":has(#target, .fallback)"), 1 << 20);
        assert_eq!(specificity(":is(:has(#target), .fallback)"), 1 << 20);
        assert_eq!(specificity(":not(:has(#target), .fallback)"), 1 << 20);
        assert_eq!(specificity(":where(:has(#target))"), 0);
        assert_eq!(specificity(":nth-child(1 of :has(#target))"), (1 << 20) | (1 << 10));
    }

    #[test]
    fn scope_matches_the_document_element_without_an_explicit_scope() {
        let document = html_parse::parse_dom_document("<!doctype html><html><body></body></html>").expect("valid standards HTML");
        let root = document.dom_root().expect("document element");
        let body = document.element_ref(root).expect("html").children().find(|&node| document.get_dom_tag(node) == Some("body")).expect("body");
        let selectors = SelectorList::parse_string_with_options(":scope", ParserOptions::default()).expect("selector parses");

        assert!(selector_matches_dom_node(selectors.0.first().expect("selector"), &document, root));
        assert!(!selector_matches_dom_node(selectors.0.first().expect("selector"), &document, body));
    }

    fn selector_matches_id(document: &Document, id: &str, selector_text: &str) -> bool {
        let node = document.node_ids().find(|node| document.get_dom_id(*node) == Some(id)).unwrap_or_else(|| panic!("missing fixture element #{id}"));
        let selectors = SelectorList::parse_string_with_options(selector_text, ParserOptions::default()).expect("selector parses");
        selector_matches_dom_node(selectors.0.first().expect("selector"), document, node)
    }

    #[test]
    fn static_html_control_states_are_normalized_before_selector_matching() {
        let document = html_parse::parse_dom_document(
            "<!doctype html><form><input id='required' required><input id='optional'><input id='checked' type='checkbox' checked><option id='selected' selected></option><textarea id='readonly' readonly></textarea><textarea id='writable'></textarea></form>",
        )
        .expect("valid standards HTML");

        for (id, selector) in [("required", ":required:enabled"), ("optional", ":optional:enabled"), ("checked", ":checked:default"), ("selected", ":checked:default"), ("readonly", ":read-only"), ("writable", ":read-write")] {
            assert!(selector_matches_id(&document, id, selector), "{id} must match {selector}");
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
        assert!(selector_matches_id(&document, "disabled-input", ":disabled"));
        assert!(selector_matches_id(&document, "disabled-option", ":disabled"));
    }

    #[test]
    fn inherited_direction_and_open_state_are_available_to_relational_selectors() {
        let document =
            html_parse::parse_dom_document("<!doctype html><main dir='rtl'><section id='owner'><span id='inherited'></span></section><details id='open' open></details><details id='closed'></details></main>").expect("valid standards HTML");

        assert!(selector_matches_id(&document, "inherited", ":dir(rtl)"));
        assert!(selector_matches_id(&document, "owner", ":has(span:dir(rtl))"));
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
fn nth_matches_dom(nth: &NthSelectorData, doc: &Document, element: html_dom::ElementRef<'_>) -> bool {
    if matches!(nth.ty, NthType::Col | NthType::LastCol) {
        return false;
    }
    let of_type = matches!(nth.ty, NthType::OfType | NthType::LastOfType | NthType::OnlyOfType);
    let from_end = matches!(nth.ty, NthType::LastChild | NthType::LastOfType);

    let (sibling_count, position) = match element.parent().and_then(|parent_idx| doc.element_ref(parent_idx)) {
        Some(parent) => {
            let mut sibling_count = 0;
            let mut position = None;
            for child_idx in parent.children() {
                if sibling_counts(doc, child_idx, of_type, element.tag()) {
                    sibling_count += 1;
                    if child_idx == element.node_id() {
                        position = Some(sibling_count);
                    }
                }
            }
            (sibling_count, position)
        }
        // The root element is its parent's only element child.
        None => (1, Some(1)),
    };
    let Some(position) = position else {
        return false;
    };

    if nth.ty.is_only() {
        return sibling_count == 1;
    }
    let index = if from_end { sibling_count - position + 1 } else { position };
    nth_index_matches(nth.a, nth.b, index)
}

/// Selectors 4 `:nth-child(An+B of S)` counts the subject only among element
/// siblings matching the forgiving selector list `S`.
fn nth_of_matches_dom(nth: &NthSelectorData, selectors: &[Selector<'_>], doc: &Document, element: html_dom::ElementRef<'_>, context: MatchContext) -> bool {
    let matches_filter = |node| selectors.iter().any(|selector| selector_matches_dom_node_with_context(selector, doc, node, context));
    let (sibling_count, position) = match element.parent().and_then(|parent_idx| doc.element_ref(parent_idx)) {
        Some(parent) => {
            let mut sibling_count = 0;
            let mut position = None;
            for child_idx in parent.children() {
                if doc.element_ref(child_idx).is_some() && matches_filter(child_idx) {
                    sibling_count += 1;
                    if child_idx == element.node_id() {
                        position = Some(sibling_count);
                    }
                }
            }
            (sibling_count, position)
        }
        None if matches_filter(element.node_id()) => (1, Some(1)),
        None => (0, None),
    };
    let Some(position) = position else {
        return false;
    };
    let index = if nth.ty == NthType::LastChild { sibling_count - position + 1 } else { position };
    nth_index_matches(nth.a, nth.b, index)
}

fn sibling_counts(doc: &Document, child_idx: DomNodeId, of_type: bool, tag: &str) -> bool {
    match doc.element_ref(child_idx) {
        Some(child) => !of_type || child.tag() == tag,
        None => false,
    }
}

/// True if the 1-based `index` is in the set `{ a*k + b | integer k >= 0 }`.
fn nth_index_matches(a: i32, b: i32, index: i32) -> bool {
    if a == 0 {
        return index == b;
    }
    let diff = index - b;
    diff % a == 0 && diff / a >= 0
}

fn local_name_string<'a>(name: &'a impl AsRef<str>, lower_name: &'a impl AsRef<str>, is_html_element_in_html_document: bool) -> &'a str {
    let name = name.as_ref();
    let lower_name = lower_name.as_ref();
    if is_html_element_in_html_document || name == lower_name { lower_name } else { name }
}

fn parsed_case_sensitivity(parsed: ParsedCaseSensitivity, is_html_element_in_html_document: bool, attribute_name: Option<&str>) -> parcel_selectors::attr::CaseSensitivity {
    if is_html_element_in_html_document && matches!(parsed, ParsedCaseSensitivity::CaseSensitive) && attribute_name.is_some_and(html_attribute_value_is_ascii_case_insensitive) {
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

fn namespace_match<'a>(namespace: Option<NamespaceConstraint<&'a lightningcss::values::string::CowArcStr<'a>>>) -> NamespaceMatch<'a> {
    match namespace {
        None => NamespaceMatch::None,
        Some(NamespaceConstraint::Any) => NamespaceMatch::Any,
        Some(NamespaceConstraint::Specific(url)) if url.as_ref().is_empty() => NamespaceMatch::None,
        Some(NamespaceConstraint::Specific(url)) => NamespaceMatch::Specific(url.as_ref()),
    }
}

fn get_attribute_in_namespace<'a>(element: html_dom::ElementRef<'a>, namespace: NamespaceMatch<'_>, local_name: &str) -> Option<&'a str> {
    match namespace {
        NamespaceMatch::None => element.attr(local_name),
        NamespaceMatch::Any => element.attr_in_any_namespace(local_name),
        NamespaceMatch::Specific(url) => element.attr_expanded(Some(url), local_name),
    }
}

fn attribute_exists_in_namespace(element: html_dom::ElementRef<'_>, namespace: NamespaceMatch<'_>, local_name: &str) -> bool {
    get_attribute_in_namespace(element, namespace, local_name).is_some()
}

/// Check if pseudo-class matches DOM node
fn pseudo_matches_dom(pseudo: &PseudoClass, doc: &Document, element: html_dom::ElementRef<'_>) -> bool {
    let state = element.html_state();
    match pseudo {
        PseudoClass::Link | PseudoClass::AnyLink(_) => state.is_link(),
        PseudoClass::Visited => false, // Don't match visited for privacy
        PseudoClass::Lang { languages } => effective_language(doc, element).is_some_and(|language| languages.iter().any(|range| language_range_matches(language, range.as_ref()))),
        PseudoClass::Dir { direction: Direction::Ltr } => !state.is_rtl(),
        PseudoClass::Dir { direction: Direction::Rtl } => state.is_rtl(),
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

/// Resolve the language of an element according to document semantics.  An
/// explicitly empty language stops inheritance and intentionally matches no
/// non-empty `:lang()` range.
fn effective_language<'a>(doc: &'a Document, element: html_dom::ElementRef<'a>) -> Option<&'a str> {
    let mut current = Some(element.node_id());
    while let Some(node) = current {
        let candidate = doc.element_ref(node)?;
        if let Some(language) = candidate.attr_expanded(Some(XML_NAMESPACE), "lang").or_else(|| candidate.attr("lang")) {
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

/// Compute Selectors specificity from the parsed component tree. Parcel's
/// current specificity cache intentionally assigns `:has()` zero specificity,
/// contrary to Selectors 4, so the renderer owns the cascade-facing value.
pub(crate) fn selector_specificity(selector: &Selector<'_>) -> u32 {
    specificity_for_components(selector.iter_raw_match_order().as_slice()).packed()
}

#[derive(Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
struct Specificity {
    ids: u32,
    classes: u32,
    elements: u32,
}

impl Specificity {
    fn add(&mut self, other: Self) {
        self.ids = self.ids.saturating_add(other.ids);
        self.classes = self.classes.saturating_add(other.classes);
        self.elements = self.elements.saturating_add(other.elements);
    }

    fn packed(self) -> u32 {
        const MAX: u32 = 0x3ff;
        self.ids.min(MAX) << 20 | self.classes.min(MAX) << 10 | self.elements.min(MAX)
    }
}

fn specificity_for_selector_list(selectors: &[Selector<'_>]) -> Specificity {
    selectors.iter().map(|selector| specificity_for_components(selector.iter_raw_match_order().as_slice())).max().unwrap_or_default()
}

fn specificity_for_components(components: &[Component<'_>]) -> Specificity {
    let mut result = Specificity::default();
    for component in components {
        let mut contribution = Specificity::default();
        match component {
            Component::Part(_) | Component::PseudoElement(_) | Component::LocalName(_) => contribution.elements = 1,
            Component::Slotted(selector) => {
                contribution.elements = 1;
                contribution.add(specificity_for_components(selector.iter_raw_match_order().as_slice()));
            }
            Component::Host(selector) => {
                contribution.classes = 1;
                if let Some(selector) = selector {
                    contribution.add(specificity_for_components(selector.iter_raw_match_order().as_slice()));
                }
            }
            Component::ID(_) => contribution.ids = 1,
            Component::Class(_)
            | Component::AttributeInNoNamespace { .. }
            | Component::AttributeInNoNamespaceExists { .. }
            | Component::AttributeOther(_)
            | Component::Root
            | Component::Empty
            | Component::Scope
            | Component::Nth(_)
            | Component::NonTSPseudoClass(_) => contribution.classes = 1,
            Component::NthOf(nth) => {
                contribution.classes = 1;
                contribution.add(specificity_for_selector_list(nth.selectors()));
            }
            Component::Negation(selectors) | Component::Is(selectors) | Component::Any(_, selectors) | Component::Has(selectors) => {
                contribution = specificity_for_selector_list(selectors);
            }
            Component::Where(_)
            | Component::Combinator(_)
            | Component::ExplicitUniversalType
            | Component::ExplicitAnyNamespace
            | Component::ExplicitNoNamespace
            | Component::DefaultNamespace(_)
            | Component::Namespace(_, _)
            | Component::Nesting => {}
        }
        result.add(contribution);
    }
    result
}

// ============================================================================
// Bloom Filter Optimization for Fast Selector Rejection
// ============================================================================

/// Fast pre-check using bloom filter - can reject selectors that definitely won't match
/// This is an optimization to avoid expensive full matching for selectors targeting ancestors
pub fn selector_might_match_with_filter(selector: &Selector, ancestor_filter: &AncestorFilter) -> bool {
    let mut iter = selector.iter();

    // Skip target sequence (already checked by selector index)
    for _ in iter.by_ref() {}

    // Check only sequences that are actually ancestors of the target. A
    // sibling compound is not represented in the target's ancestor bloom
    // filter; rejecting it here would turn valid `A + B` and `A ~ B`
    // selectors into false negatives before the full matcher sees them.
    while let Some(combinator) = iter.next_sequence() {
        if !matches!(combinator, Combinator::Child | Combinator::Descendant) {
            return true;
        }
        for component in iter.by_ref() {
            match component {
                Component::LocalName(name) if !ancestor_filter.might_contain(name.lower_name.0.as_ref()) => {
                    return false; // Definitely no match
                }
                Component::Class(class) if !ancestor_filter.might_contain(class.0.as_ref()) => {
                    return false; // Definitely no match
                }
                Component::ID(id) if !ancestor_filter.might_contain(id.0.as_ref()) => {
                    return false; // Definitely no match
                }
                _ => {}
            }
        }
    }

    true // Might match (need full check)
}
