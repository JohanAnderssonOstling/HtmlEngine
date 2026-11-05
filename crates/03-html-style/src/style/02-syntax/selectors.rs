//! Selector AST inspection without document access.
use super::value::token_list_to_css_string;
use cssparser::{Parser, ParserInput};
use lightningcss::properties::custom::TokenList;
use lightningcss::selector::{Component, PseudoClass, PseudoElement, Selector, SelectorList};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::ParseWithOptions;
use rustc_data_structures::fx::FxHashMap;

pub(crate) const PSEUDO_BEFORE_MASK: u8 = 1 << 0;

pub(crate) const PSEUDO_AFTER_MASK: u8 = 1 << 1;

pub(crate) const PSEUDO_FIRST_LINE_MASK: u8 = 1 << 2;

pub(crate) const PSEUDO_FIRST_LETTER_MASK: u8 = 1 << 3;

/// Validate semantic constraints retained by the native selector AST.
pub(crate) fn selector_list_is_web_valid(selectors: &SelectorList<'_>) -> bool {
    SelectorValidation::default().selector_list_is_web_valid(selectors)
}

/// Reuse native argument validation while preparing rules from the same fragments.
/// Keys borrow their token lists; the cache owns neither tokens nor parser trees.
#[derive(Default)]
pub(crate) struct SelectorValidation<'a, 'css> {
    host_contexts: FxHashMap<&'a TokenList<'css>, bool>,
}

impl<'a, 'css> SelectorValidation<'a, 'css> {
    pub(crate) fn selector_list_is_web_valid(&mut self, selectors: &'a SelectorList<'css>) -> bool {
        !selectors.0.is_empty()
            && selectors
                .0
                .iter()
                .all(|selector| self.selector_is_web_valid(selector, false))
    }

    fn selector_is_web_valid(&mut self, selector: &'a Selector<'css>, inside_has: bool) -> bool {
        let mut compound_has = false;
        let mut compound_pseudo = false;
        let mut restricted_user_action = false;
        for component in selector.iter_raw_match_order() {
            match component {
                Component::Combinator(_) => {
                    compound_has = false;
                    compound_pseudo = false;
                    restricted_user_action = false;
                }
                Component::Has(selectors) => {
                    if inside_has
                        || selectors.is_empty()
                        || !selectors
                            .iter()
                            .all(|selector| self.selector_is_web_valid(selector, true))
                    {
                        return false;
                    }
                    compound_has = true;
                }
                Component::PseudoElement(pseudo) => {
                    if inside_has || !pseudo_is_web_valid(pseudo) {
                        return false;
                    }
                    compound_pseudo = true;
                    restricted_user_action = matches!(
                        pseudo,
                        PseudoElement::ViewTransition
                            | PseudoElement::ViewTransitionGroup { .. }
                            | PseudoElement::ViewTransitionImagePair { .. }
                            | PseudoElement::ViewTransitionOld { .. }
                            | PseudoElement::ViewTransitionNew { .. }
                            | PseudoElement::Selection(_)
                            | PseudoElement::TargetText
                            | PseudoElement::SpellingError
                            | PseudoElement::GrammarError
                            | PseudoElement::HighlightFunction { .. }
                            | PseudoElement::SearchText
                    );
                }
                Component::Part(_) | Component::Slotted(_) => {
                    if inside_has {
                        return false;
                    }
                    compound_pseudo = true;
                }
                Component::NonTSPseudoClass(PseudoClass::Custom { name })
                    if name.eq_ignore_ascii_case("has")
                        || name.eq_ignore_ascii_case("host-context")
                        || name.eq_ignore_ascii_case("state") =>
                {
                    return false;
                }
                Component::NonTSPseudoClass(PseudoClass::Hover | PseudoClass::Active)
                    if restricted_user_action =>
                {
                    return false;
                }
                Component::NonTSPseudoClass(PseudoClass::CustomFunction { name, arguments }) => {
                    // These names denote pseudo-elements, not pseudo-classes.
                    if name.eq_ignore_ascii_case("part") || name.eq_ignore_ascii_case("slotted") {
                        return false;
                    }
                    if name.eq_ignore_ascii_case("host-context") {
                        let valid = *self
                            .host_contexts
                            .entry(arguments)
                            .or_insert_with(|| host_context_is_valid(arguments));
                        if !valid {
                            return false;
                        }
                    }
                }
                Component::Negation(selectors) | Component::Any(_, selectors) => {
                    if !selectors
                        .iter()
                        .all(|selector| self.selector_is_web_valid(selector, inside_has))
                    {
                        return false;
                    }
                }
                Component::NthOf(nth) => {
                    if !nth
                        .selectors()
                        .iter()
                        .all(|selector| self.selector_is_web_valid(selector, inside_has))
                    {
                        return false;
                    }
                }
                Component::Host(Some(selector)) => {
                    if !self.selector_is_web_valid(selector, inside_has) {
                        return false;
                    }
                }
                // :is() and :where() have forgiving lists. Invalid branches must
                // not invalidate a containing selector (including an empty list).
                _ => {}
            }
            if compound_has && compound_pseudo {
                return false;
            }
        }
        true
    }
}

fn host_context_is_valid(arguments: &TokenList<'_>) -> bool {
    // The native selector parser supplies the compound-selector grammar. No
    // source splitting or parallel token grammar is needed for this argument.
    let Some(css) = token_list_to_css_string(arguments) else {
        return false;
    };
    let mut input = ParserInput::new(&css);
    Parser::new(&mut input)
        .parse_entirely(|input| Selector::parse_with_options(input, &ParserOptions::default()))
        .is_ok_and(|selector| {
            !selector.iter_raw_match_order().any(|component| {
                matches!(
                    component,
                    Component::Combinator(_)
                        | Component::PseudoElement(_)
                        | Component::Part(_)
                        | Component::Slotted(_)
                )
            }) && SelectorValidation::default().selector_is_web_valid(&selector, false)
        })
}

fn pseudo_is_web_valid(pseudo: &PseudoElement<'_>) -> bool {
    match pseudo {
        PseudoElement::Custom { name } => ![
            "highlight",
            "picker",
            "slotted",
            "scroll-button",
            "view-transition-group",
            "view-transition-image-pair",
            "view-transition-old",
            "view-transition-new",
        ]
        .iter()
        .any(|keyword| name.eq_ignore_ascii_case(keyword)),
        PseudoElement::CustomFunction { name, .. } => {
            !name.eq_ignore_ascii_case("view-transition")
                && !name.eq_ignore_ascii_case("has-slotted")
        }
        PseudoElement::PickerFunction { identifier } => {
            identifier.0.as_ref().eq_ignore_ascii_case("select")
        }
        _ => true,
    }
}
pub(crate) fn selector_list_is_web_valid_for_scope(selectors: &SelectorList<'_>) -> bool {
    selector_list_is_web_valid(selectors)
        && selectors.0.iter().all(|selector| {
            !selector
                .iter_raw_match_order()
                .any(|component| matches!(component, Component::PseudoElement(_)))
        })
}

pub(crate) fn selector_list_is_web_valid_for_supports(selectors: &SelectorList<'_>) -> bool {
    selectors.0.len() == 1 && selector_list_is_web_valid(selectors)
        && selectors.0[0].iter_raw_match_order().all(component_is_supported)
}

fn component_is_supported(component: &Component<'_>) -> bool {
    match component {
        Component::NonTSPseudoClass(PseudoClass::Custom { .. } | PseudoClass::CustomFunction { .. }) => false,
        Component::PseudoElement(pseudo) => matches!(pseudo, PseudoElement::Before | PseudoElement::After | PseudoElement::FirstLine | PseudoElement::FirstLetter),
        Component::Part(_) | Component::Slotted(_) | Component::Host(_) => false,
        Component::Negation(selectors) | Component::Is(selectors) | Component::Where(selectors) | Component::Has(selectors) => selectors.iter().all(|selector| selector.iter_raw_match_order().all(component_is_supported)),
        _ => true,
    }
}

pub(crate) fn selector_list_pseudo_mask(selectors: &SelectorList<'_>) -> u8 {
    selectors.0.iter().fold(0, |mask, selector| {
        selector
            .iter_raw_match_order()
            .fold(mask, |mask, component| {
                mask | match component {
                    Component::PseudoElement(PseudoElement::Before) => PSEUDO_BEFORE_MASK,
                    Component::PseudoElement(PseudoElement::After) => PSEUDO_AFTER_MASK,
                    Component::PseudoElement(PseudoElement::FirstLine) => PSEUDO_FIRST_LINE_MASK,
                    Component::PseudoElement(PseudoElement::FirstLetter) => {
                        PSEUDO_FIRST_LETTER_MASK
                    }
                    _ => 0,
                }
            })
    })
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
            Component::Part(_) | Component::PseudoElement(_) | Component::LocalName(_) => {
                contribution.elements = 1
            }
            Component::Slotted(selector) => {
                contribution.elements = 1;
                contribution.add(specificity_for_components(
                    selector.iter_raw_match_order().as_slice(),
                ));
            }
            Component::Host(selector) => {
                contribution.classes = 1;
                if let Some(selector) = selector {
                    contribution.add(specificity_for_components(
                        selector.iter_raw_match_order().as_slice(),
                    ));
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
            Component::Negation(selectors)
            | Component::Is(selectors)
            | Component::Any(_, selectors)
            | Component::Has(selectors) => {
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

pub(crate) fn ancestor_hash(s: &str) -> u64 {
    let h = s
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    1u64 << (h % 64)
}
