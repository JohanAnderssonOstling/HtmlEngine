//! Selector AST inspection without document access.
use lightningcss::selector::{Component, PseudoClass, PseudoElement, Selector, SelectorList};

pub(crate) const PSEUDO_BEFORE_MASK: u8 = 1 << 0;

pub(crate) const PSEUDO_AFTER_MASK: u8 = 1 << 1;

pub(crate) const PSEUDO_FIRST_LINE_MASK: u8 = 1 << 2;

pub(crate) const PSEUDO_FIRST_LETTER_MASK: u8 = 1 << 3;

/// Native parsing already checked syntax; preparation requires a nonempty list.
pub(crate) fn selector_list_is_web_valid(selectors: &SelectorList<'_>) -> bool {
    !selectors.0.is_empty()
}

pub(crate) fn selector_list_is_web_valid_for_scope(selectors: &SelectorList<'_>) -> bool {
    selectors.0.iter().all(|selector| {
        !selector
            .iter_raw_match_order()
            .any(|component| matches!(component, Component::PseudoElement(_)))
    })
}

pub(crate) fn selector_list_is_web_valid_for_supports(selectors: &SelectorList<'_>) -> bool {
    selectors.0.len() == 1 && selectors.0[0].iter_raw_match_order().all(component_is_supported)
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
