use super::compiled::{CompiledSelectors, PreparedSelector};
use crate::style::rules::prepared::{EffectiveRuleId, PreparedRuleSet};
use html_dom::{Document, DomNodeId};
use lightningcss::properties::Property;
use lightningcss::properties::custom::{CustomProperty, CustomPropertyName, Token, TokenList, TokenOrValue};
use lightningcss::selector::{Component, PseudoClass, PseudoElement, Selector, SelectorList};
use lightningcss::stylesheet::{ParserOptions, PrinterOptions};
use lightningcss::traits::ParseWithOptions;
use lightningcss::values::ident::DashedIdent;
use rustc_data_structures::fx::FxHashMap;

pub(crate) const PSEUDO_BEFORE_MASK: u8 = 1 << 0;
pub(crate) const PSEUDO_AFTER_MASK: u8 = 1 << 1;
pub(crate) const PSEUDO_FIRST_LINE_MASK: u8 = 1 << 2;
pub(crate) const PSEUDO_FIRST_LETTER_MASK: u8 = 1 << 3;

// ============================================================================
// Bloom Filter for fast ancestor rejection
// ============================================================================

#[derive(Clone, Copy, Default)]
pub struct AncestorFilter {
    bits: u64,
}

impl AncestorFilter {
    #[inline]
    pub(super) fn hash(s: &str) -> u64 {
        let h = s.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        1u64 << (h % 64)
    }

    #[inline]
    pub fn insert(&mut self, s: &str) {
        self.bits |= Self::hash(s);
    }

    #[inline]
    pub fn might_contain(&self, s: &str) -> bool {
        (self.bits & Self::hash(s)) != 0
    }

    /// Insert tag, id, and all classes from an element
    #[allow(dead_code)]
    pub fn insert_element(&mut self, tag: &str, id: Option<&str>, classes: Option<&str>) {
        self.insert(tag);
        if let Some(id) = id {
            self.insert(id);
        }
        if let Some(c) = classes {
            for class in c.split_whitespace() {
                self.insert(class);
            }
        }
    }
}

// ============================================================================
// Selector Index for O(1) candidate lookup
// ============================================================================

/// Index of rule keys, keyed by selector target (tag/class/id)
pub struct SelectorIndex {
    by_tag: FxHashMap<String, Vec<EffectiveRuleId>>,
    by_class: FxHashMap<String, Vec<EffectiveRuleId>>,
    by_id: FxHashMap<String, Vec<EffectiveRuleId>>,
    universal: Vec<EffectiveRuleId>, // *, [attr], :pseudo with no tag/class/id
    compiled: CompiledSelectors,
}

/// Dense duplicate suppression for candidate rule IDs. Prepared IDs are
/// contiguous, so advancing an epoch is cheaper than clearing a hash table.
pub(crate) struct CandidateDeduper {
    generations: Vec<u32>,
    generation: u32,
}

impl CandidateDeduper {
    pub(crate) fn new(rule_count: usize) -> Self {
        Self { generations: vec![0; rule_count], generation: 0 }
    }

    fn begin_element(&mut self) {
        if self.generation == u32::MAX {
            self.generations.fill(0);
            self.generation = 1;
        } else {
            self.generation += 1;
        }
    }

    #[inline]
    fn insert(&mut self, id: EffectiveRuleId) -> bool {
        let generation = &mut self.generations[id.index()];
        if *generation == self.generation {
            false
        } else {
            *generation = self.generation;
            true
        }
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

    pub(crate) fn rule_count(&self) -> usize {
        self.compiled.rule_count()
    }

    pub(crate) fn prepared_selectors(&self, id: EffectiveRuleId) -> &[PreparedSelector] {
        self.compiled.for_rule(id)
    }

    #[inline]
    pub(crate) fn selector_might_match(&self, selector: PreparedSelector, ancestor_filter: &AncestorFilter) -> bool {
        ancestor_filter.bits & selector.ancestor_requirements == selector.ancestor_requirements
    }

    #[inline]
    pub(crate) fn selector_specificity(&self, selector: PreparedSelector) -> u32 {
        self.compiled.specificity(selector)
    }

    #[inline]
    pub(crate) fn selector_is_context_free(&self, selector: PreparedSelector) -> bool {
        self.compiled.is_context_free(selector)
    }

    #[inline]
    pub(crate) fn matches_fast_selector(&self, selector: PreparedSelector, doc: &Document, node: DomNodeId) -> Option<bool> {
        self.compiled.matches(selector, doc, node)
    }

    /// Add a selector to the index
    fn add_selector(&mut self, selector: &Selector, rule_id: EffectiveRuleId) {
        let mut has_tag = false;
        let mut has_class = false;
        let mut has_id = false;

        // Raw match order starts at the rightmost compound, which is the only
        // compound guaranteed to match the element currently being resolved.
        // Indexing ancestor compounds admits false candidates for every
        // descendant selector and makes selector matching needlessly broad.
        for component in selector.iter_raw_match_order() {
            if component.is_combinator() {
                break;
            }
            match component {
                Component::LocalName(name) => {
                    let tag = name.lower_name.0.as_ref().to_string();
                    self.by_tag.entry(tag).or_default().push(rule_id);
                    has_tag = true;
                }
                Component::Class(class) => {
                    let class_str = class.0.as_ref().to_string();
                    self.by_class.entry(class_str).or_default().push(rule_id);
                    has_class = true;
                }
                Component::ID(id) => {
                    let id_str = id.0.as_ref().to_string();
                    self.by_id.entry(id_str).or_default().push(rule_id);
                    has_id = true;
                }
                _ => {}
            }
        }

        // If no specific target, it's universal (matches anything)
        if !has_tag && !has_class && !has_id {
            self.universal.push(rule_id);
        }
    }

    /// Get candidate rule indices for an element
    pub fn collect_candidates<'a>(&self, tag: &str, id: Option<&str>, classes: impl IntoIterator<Item = &'a str>, candidates: &mut Vec<EffectiveRuleId>, seen: &mut CandidateDeduper) {
        candidates.clear();
        seen.begin_element();

        // Add by tag
        if let Some(indices) = self.by_tag.get(tag) {
            for &idx in indices {
                if seen.insert(idx) {
                    candidates.push(idx);
                }
            }
        }

        // Add by id
        if let Some(id) = id
            && let Some(indices) = self.by_id.get(id)
        {
            for &idx in indices {
                if seen.insert(idx) {
                    candidates.push(idx);
                }
            }
        }

        // Add by class
        for class in classes {
            if let Some(indices) = self.by_class.get(class) {
                for &idx in indices {
                    if seen.insert(idx) {
                        candidates.push(idx);
                    }
                }
            }
        }

        // Always include universal selectors
        for &idx in &self.universal {
            if seen.insert(idx) {
                candidates.push(idx);
            }
        }
    }
}

/// Validate browser-facing selector semantics that Lightning CSS deliberately
/// preserves as custom selector nodes. The parser's permissive representation
/// is useful for transforms, but invalid selector lists must never enter the
/// renderer's matching index.
pub(crate) fn selector_list_is_web_valid(selectors: &SelectorList<'_>) -> bool {
    !selectors.0.is_empty() && selectors.0.iter().all(|selector| selector_is_web_valid(selector, SelectorContext::default()))
}

pub(crate) fn selector_list_is_web_valid_for_scope(selectors: &SelectorList<'_>) -> bool {
    !selectors.0.is_empty() && selectors.0.iter().all(|selector| selector_is_web_valid(selector, SelectorContext { allow_pseudo_elements: false, ..SelectorContext::default() }))
}

/// Validate the single complex selector accepted by `@supports selector()`.
/// Unlike selectors in style rules, this grammar is unforgiving: a comma at
/// the top level, or an invalid member hidden inside `:is()`/`:where()`, makes
/// the whole feature query false. Locally recognized custom pseudo-elements
/// without matching support also remain false.
pub(crate) fn selector_list_is_web_valid_for_supports(selectors: &SelectorList<'_>) -> bool {
    selectors.0.len() == 1 && selector_is_web_valid(&selectors.0[0], SelectorContext { strict_forgiving_lists: true, require_supported_features: true, ..SelectorContext::default() })
}

#[derive(Clone, Copy)]
struct SelectorContext {
    inside_has: bool,
    allow_pseudo_elements: bool,
    strict_forgiving_lists: bool,
    require_supported_features: bool,
}

impl Default for SelectorContext {
    fn default() -> Self {
        Self { inside_has: false, allow_pseudo_elements: true, strict_forgiving_lists: false, require_supported_features: false }
    }
}

fn selector_is_web_valid(selector: &Selector<'_>, context: SelectorContext) -> bool {
    let mut user_action_state = false;
    let mut excludes_user_action_state = false;
    let mut part = false;
    let mut relative_selector = false;
    let mut scroll_button = false;
    let mut search_text = false;
    let mut compound_to_right = false;
    for component in selector.iter_raw_match_order() {
        if component.is_combinator() {
            // A pseudo-element has its own compound. States on its originating
            // element remain valid and must not be confused with its states.
            user_action_state = false;
            excludes_user_action_state = false;
            part = false;
            relative_selector = false;
            scroll_button = false;
            search_text = false;
            compound_to_right = true;
        } else {
            if search_text && !search_text_state_is_valid(component, context.strict_forgiving_lists) {
                return false;
            }
            search_text |= matches!(component, Component::PseudoElement(PseudoElement::SearchText));
            if scroll_button && !scroll_button_state_is_valid(component, context) {
                return false;
            }
            if matches!(component, Component::PseudoElement(PseudoElement::CustomFunction { name, .. }) if name.eq_ignore_ascii_case("scroll-button")) {
                // The dependency stores custom pseudos in the originating
                // compound. Enforce the same subject/state restrictions as
                // a dedicated pseudo-element node would receive.
                if compound_to_right {
                    return false;
                }
                scroll_button = true;
            }
        }
        user_action_state |= matches!(
            component,
            Component::NonTSPseudoClass(
                PseudoClass::Hover
                    | PseudoClass::Active
                    | PseudoClass::Focus
                    | PseudoClass::FocusWithin
                    | PseudoClass::FocusVisible
            )
        );
        excludes_user_action_state |= matches!(
            component,
            Component::PseudoElement(
                PseudoElement::ViewTransition
                    | PseudoElement::ViewTransitionGroup { .. }
                    | PseudoElement::ViewTransitionImagePair { .. }
                    | PseudoElement::ViewTransitionOld { .. }
                    | PseudoElement::ViewTransitionNew { .. }
                    | PseudoElement::Selection(_)
                    | PseudoElement::TargetText
                    | PseudoElement::HighlightFunction { .. }
                    | PseudoElement::SpellingError
                    | PseudoElement::GrammarError
                    | PseudoElement::SearchText
            )
        );
        part |= matches!(component, Component::Part(_));
        relative_selector |= contains_has(component, context.strict_forgiving_lists);
        if (user_action_state && excludes_user_action_state)
            || (part && relative_selector)
            || !component_is_web_valid(component, context)
        {
            return false;
        }
    }
    true
}

fn contains_has(component: &Component<'_>, strict_forgiving_lists: bool) -> bool {
    let selectors = match component {
        Component::Has(_) => return true,
        Component::Negation(selectors) => selectors,
        Component::Is(selectors) | Component::Where(selectors) if strict_forgiving_lists => {
            selectors
        }
        _ => return false,
    };
    selectors.iter().any(|selector| {
        selector
            .iter_raw_match_order()
            .any(|component| contains_has(component, strict_forgiving_lists))
    })
}

pub(crate) fn selector_list_pseudo_mask(selectors: &SelectorList<'_>) -> u8 {
    selectors.0.iter().fold(0, |mask, selector| {
        selector.iter_raw_match_order().fold(mask, |mask, component| {
            mask | match component {
                Component::PseudoElement(PseudoElement::Before) => PSEUDO_BEFORE_MASK,
                Component::PseudoElement(PseudoElement::After) => PSEUDO_AFTER_MASK,
                Component::PseudoElement(PseudoElement::FirstLine) => PSEUDO_FIRST_LINE_MASK,
                Component::PseudoElement(PseudoElement::FirstLetter) => PSEUDO_FIRST_LETTER_MASK,
                _ => 0,
            }
        })
    })
}

fn component_is_web_valid(component: &Component<'_>, context: SelectorContext) -> bool {
    match component {
        Component::PseudoElement(PseudoElement::PickerFunction { identifier }) => context.allow_pseudo_elements && identifier.0.eq_ignore_ascii_case("select"),
        Component::NonTSPseudoClass(PseudoClass::Custom { name }) if name.eq_ignore_ascii_case("xr-overlay") => !context.require_supported_features,
        Component::NonTSPseudoClass(PseudoClass::Custom { name }) => matches_custom_bare_pseudo(name.as_ref()),
        Component::NonTSPseudoClass(PseudoClass::CustomFunction { name, arguments }) => validate_custom_function(name.as_ref(), arguments, context),
        Component::PseudoElement(PseudoElement::CustomFunction { name, arguments }) if name.eq_ignore_ascii_case("scroll-button") => {
            context.allow_pseudo_elements && !context.require_supported_features && scroll_button_arguments_are_valid(arguments)
        }
        Component::PseudoElement(PseudoElement::Custom { .. } | PseudoElement::CustomFunction { .. }) => false,
        Component::PseudoElement(_) | Component::Part(_) | Component::Slotted(_) if !context.allow_pseudo_elements => false,
        Component::Negation(selectors) => !selectors.is_empty() && selectors.iter().all(|selector| selector_is_web_valid(selector, SelectorContext { allow_pseudo_elements: false, ..context })),
        // :is() and :where() are forgiving selector lists. Invalid members are
        // discarded rather than invalidating the outer selector. Feature
        // queries deliberately request the stricter grammar instead.
        Component::Is(selectors) | Component::Where(selectors) => {
            !context.strict_forgiving_lists || (!selectors.is_empty() && selectors.iter().all(|selector| selector_is_web_valid(selector, SelectorContext { allow_pseudo_elements: false, ..context })))
        }
        Component::Has(selectors) => !context.inside_has && !selectors.is_empty() && selectors.iter().all(|selector| selector_is_web_valid(selector, SelectorContext { inside_has: true, allow_pseudo_elements: false, ..context })),
        Component::Host(Some(selector)) | Component::Slotted(selector) => selector_is_web_valid(selector, SelectorContext { allow_pseudo_elements: false, ..context }),
        Component::NthOf(nth) => nth.selectors().iter().all(|selector| selector_is_web_valid(selector, SelectorContext { allow_pseudo_elements: false, ..context })),
        Component::Part(names) => !names.is_empty(),
        _ => true,
    }
}

fn scroll_button_arguments_are_valid(arguments: &TokenList<'_>) -> bool {
    let mut tokens = arguments.0.iter().filter(|token| {
        !matches!(
            token,
            TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_))
        )
    });
    let valid = match tokens.next() {
        Some(TokenOrValue::Token(Token::Ident(name))) => [
            "up",
            "down",
            "left",
            "right",
            "block-start",
            "block-end",
            "inline-start",
            "inline-end",
            "prev",
            "next",
        ]
        .iter()
        .any(|direction| name.eq_ignore_ascii_case(direction)),
        Some(TokenOrValue::Token(Token::Delim('*'))) => true,
        _ => false,
    };
    valid && tokens.next().is_none()
}

fn scroll_button_state_is_valid(component: &Component<'_>, context: SelectorContext) -> bool {
    match component {
        Component::NonTSPseudoClass(
            PseudoClass::Hover
            | PseudoClass::Active
            | PseudoClass::Focus
            | PseudoClass::FocusVisible
            | PseudoClass::FocusWithin
            | PseudoClass::Enabled
            | PseudoClass::Disabled,
        ) => true,
        Component::Negation(selectors) => {
            !selectors.is_empty()
                && selectors.iter().all(|selector| {
                    selector
                        .iter_raw_match_order()
                        .all(|component| scroll_button_state_is_valid(component, context))
                })
        }
        Component::Is(selectors) | Component::Where(selectors) => {
            !context.strict_forgiving_lists
                || (!selectors.is_empty()
                    && selectors.iter().all(|selector| {
                        selector
                            .iter_raw_match_order()
                            .all(|component| scroll_button_state_is_valid(component, context))
                    }))
        }
        _ => false,
    }
}

pub(crate) fn search_text_state_is_valid(component: &Component<'_>, strict: bool) -> bool {
    match component {
        Component::NonTSPseudoClass(PseudoClass::Current) => true,
        Component::Negation(selectors) => {
            !selectors.is_empty()
                && selectors.iter().all(|selector| {
                    selector
                        .iter_raw_match_order()
                        .all(|component| search_text_state_is_valid(component, strict))
                })
        }
        Component::Is(selectors) | Component::Where(selectors) => {
            !strict
                || (!selectors.is_empty()
                    && selectors.iter().all(|selector| {
                        selector
                            .iter_raw_match_order()
                            .all(|component| search_text_state_is_valid(component, strict))
                    }))
        }
        _ => false,
    }
}

fn matches_custom_bare_pseudo(name: &str) -> bool {
    // Selectors Level 5 additions that the current Lightning CSS AST has not
    // promoted to dedicated variants yet.
    name.eq_ignore_ascii_case("heading") || name.eq_ignore_ascii_case("has-slotted")
}

fn validate_custom_function(name: &str, arguments: &TokenList<'_>, context: SelectorContext) -> bool {
    if name.eq_ignore_ascii_case("host-context") {
        return validate_host_context(arguments, context);
    }
    if name.eq_ignore_ascii_case("heading") {
        return validate_heading_arguments(arguments);
    }
    if name.eq_ignore_ascii_case("has-slotted") {
        return validate_has_slotted_arguments(arguments);
    }
    false
}

fn validate_host_context(arguments: &TokenList<'_>, context: SelectorContext) -> bool {
    // Use the dependency's value serializer without exposing or duplicating
    // its private token-list printer, then parse a single compound selector.
    let Ok(css) = Property::Custom(CustomProperty {
        name: CustomPropertyName::Custom(DashedIdent("--selector".into())),
        value: arguments.clone(),
    })
    .value_to_css_string(PrinterOptions::default()) else {
        return false;
    };
    SelectorList::parse_string_with_options(&css, ParserOptions::default()).is_ok_and(|list| {
        list.0.len() == 1
            && list.0[0]
                .iter_raw_match_order()
                .all(|component| !component.is_combinator())
            && selector_is_web_valid(
                &list.0[0],
                SelectorContext {
                    allow_pseudo_elements: false,
                    ..context
                },
            )
    })
}

fn validate_heading_arguments(arguments: &TokenList<'_>) -> bool {
    let mut expect_integer = true;
    let mut saw_integer = false;
    for value in arguments.0.iter().filter(|value| !value.is_whitespace()) {
        match value {
            TokenOrValue::Token(Token::Number { int_value: Some(_), .. }) if expect_integer => {
                saw_integer = true;
                expect_integer = false;
            }
            TokenOrValue::Token(Token::Comma) if !expect_integer => expect_integer = true,
            _ => return false,
        }
    }
    saw_integer && !expect_integer
}

fn validate_has_slotted_arguments(arguments: &TokenList<'_>) -> bool {
    let significant = arguments.0.iter().filter(|value| !value.is_whitespace()).collect::<Vec<_>>();
    let Some(first) = significant.first() else { return false };
    if matches!(first, TokenOrValue::Token(Token::Number { .. } | Token::Dimension { .. } | Token::Percentage { .. })) {
        return false;
    }
    let mut block_depth = 0usize;
    for value in significant {
        match value {
            TokenOrValue::Token(Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock) => block_depth += 1,
            TokenOrValue::Token(Token::CloseParenthesis | Token::CloseSquareBracket | Token::CloseCurlyBracket) => block_depth = block_depth.saturating_sub(1),
            TokenOrValue::Token(Token::Delim('>')) if block_depth == 0 => return false,
            _ => {}
        }
    }
    true
}
