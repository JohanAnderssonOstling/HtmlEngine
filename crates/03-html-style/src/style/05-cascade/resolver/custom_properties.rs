//! Custom-property collection, cycle detection, inheritance, and substitution.

use super::specified::{
    CandidateAction, CascadeBoundary, CascadeRollback, DeclarationEvent, ElementDeclarations,
    candidate_action, rollback_keyword,
};
use crate::style::syntax::wide_keywords::token_list_css_wide_keyword;
use cssparser::CowRcStr;
use lightningcss::printer::PrinterOptions;
use lightningcss::properties::custom::{
    CustomProperty, Token, TokenList, TokenOrValue, UnparsedProperty, Variable,
};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::visitor::{Visit, Visitor};
use rustc_data_structures::fx::{FxHashMap, FxHasher};
use static_self::IntoOwned;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::mem::{replace, take};

pub(super) use crate::style::syntax::substitution::needs_substitution;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct CustomDeclarationIdentity {
    property_address: usize,
    boundary: CascadeBoundary,
}

pub(super) struct CustomCascadeSignature {
    hash: u64,
    parent_map_id: u32,
    declarations: Box<[CustomDeclarationIdentity]>,
}

struct CustomCascadeCacheEntry {
    parent_map_id: u32,
    declarations: Box<[CustomDeclarationIdentity]>,
    map_id: u32,
}

#[derive(Default)]
pub(super) struct CustomCascadeCache {
    buckets: FxHashMap<u64, Vec<CustomCascadeCacheEntry>>,
    entry_count: usize,
}

pub(super) enum CustomCascadeProbe {
    NoDeclarations,
    UncachedDeclarations,
    Hit(u32),
    Miss(CustomCascadeSignature),
}

// Bound retained signatures for documents whose elements genuinely have many
// unique inline or selector-driven custom-property cascades.
const MAX_CUSTOM_CASCADE_CACHE_ENTRIES: usize = 256;

impl CustomCascadeCache {
    /// Look up a prior cascade without allocating on a hit. The hash narrows
    /// candidates only; declaration identity and ordering are compared again
    /// so hash collisions cannot reuse the wrong custom-property map.
    pub(super) fn probe<'sheet, 'css>(
        &self,
        events: &[DeclarationEvent<'sheet, 'css>],
        element: ElementDeclarations<'_, 'css>,
        parent_map_id: u32,
    ) -> CustomCascadeProbe {
        let mut hasher = FxHasher::default();
        parent_map_id.hash(&mut hasher);
        let mut declaration_count = 0usize;
        let declarations = events.iter().filter_map(|event| {
            let property = event.property(element);
            custom_declaration(property)?;
            Some(CustomDeclarationIdentity {
                property_address: property as *const Property<'css> as usize,
                boundary: event.boundary,
            })
        });
        for identity in declarations.clone() {
            identity.hash(&mut hasher);
            declaration_count += 1;
        }
        if declaration_count == 0 {
            return CustomCascadeProbe::NoDeclarations;
        }
        declaration_count.hash(&mut hasher);
        let hash = hasher.finish();

        if let Some(entries) = self.buckets.get(&hash) {
            for entry in entries {
                if entry.parent_map_id == parent_map_id
                    && declarations.clone().eq(entry.declarations.iter().copied())
                {
                    return CustomCascadeProbe::Hit(entry.map_id);
                }
            }
        }
        if self.entry_count >= MAX_CUSTOM_CASCADE_CACHE_ENTRIES {
            return CustomCascadeProbe::UncachedDeclarations;
        }

        let declarations = declarations.collect::<Vec<_>>().into_boxed_slice();
        CustomCascadeProbe::Miss(CustomCascadeSignature {
            hash,
            parent_map_id,
            declarations,
        })
    }

    pub(super) fn insert(&mut self, signature: CustomCascadeSignature, map_id: u32) {
        self.buckets
            .entry(signature.hash)
            .or_default()
            .push(CustomCascadeCacheEntry {
                parent_map_id: signature.parent_map_id,
                declarations: signature.declarations,
                map_id,
            });
        self.entry_count += 1;
    }
}

fn custom_declaration<'property, 'css>(
    property: &'property Property<'css>,
) -> Option<(&'property str, &'property TokenList<'css>)> {
    match property {
        Property::Custom(custom) if custom.name.as_ref().starts_with("--") => {
            Some((custom.name.as_ref(), &custom.value))
        }
        Property::Unparsed(unparsed) if unparsed.property_id.name().starts_with("--") => {
            Some((unparsed.property_id.name(), &unparsed.value))
        }
        _ => None,
    }
}

/// Cascade custom properties as specified token values. Rollback discards
/// candidates by origin/layer rather than cloning the whole map at every
/// boundary.
pub(super) fn cascade_custom_properties<'sheet, 'css>(
    events: &[DeclarationEvent<'sheet, 'css>],
    element: ElementDeclarations<'_, 'css>,
    parent: &FxHashMap<String, TokenList<'css>>,
) -> Option<FxHashMap<String, TokenList<'css>>> {
    if !events
        .iter()
        .any(|event| custom_declaration(event.property(element)).is_some())
    {
        return None;
    }
    let mut values = parent.clone();
    let mut decided = HashSet::<&str>::new();
    let mut rollbacks = FxHashMap::<&str, Vec<CascadeRollback>>::default();

    for event in events.iter().rev() {
        let Some((name, value)) = custom_declaration(event.property(element)) else {
            continue;
        };
        if decided.contains(&name) {
            continue;
        }
        let keyword = token_list_css_wide_keyword(value);
        let rollback = keyword.and_then(rollback_keyword);
        match candidate_action(
            rollbacks.get(&name).map_or(&[], Vec::as_slice),
            event.boundary,
            rollback,
        ) {
            CandidateAction::Excluded => continue,
            CandidateAction::Rollback(kind) => {
                rollbacks
                    .entry(name)
                    .or_default()
                    .push(CascadeRollback::new(event.boundary, kind));
                continue;
            }
            CandidateAction::Select => {}
        }
        match keyword {
            Some("initial") => {
                values.remove(name);
            }
            Some("inherit" | "unset") => {
                // `values` began as the inherited map.
            }
            _ => {
                values.insert(name.to_string(), value.clone());
            }
        }
        decided.insert(name);
    }
    // Literal winners and inherited values are already resolved. Keep the
    // dependency-graph pass out of the common flat-map path.
    if values.values().any(needs_substitution) {
        resolve_custom_properties(&mut values, parent);
    }
    Some(values)
}

pub(super) fn effective_custom_properties<'map, 'css>(
    properties: &'map Option<FxHashMap<String, TokenList<'css>>>,
    parent: &'map FxHashMap<String, TokenList<'css>>,
) -> &'map FxHashMap<String, TokenList<'css>> {
    properties.as_ref().unwrap_or(parent)
}

struct CustomPropertyDependencyCollector<'a> {
    indices: &'a FxHashMap<&'a str, usize>,
    dependencies: Vec<usize>,
}

impl<'i> Visitor<'i> for CustomPropertyDependencyCollector<'_> {
    type Error = Infallible;

    fn visit_types(&self) -> lightningcss::visitor::VisitTypes {
        lightningcss::visit_types!(VARIABLES | FUNCTIONS)
    }

    fn visit_variable(&mut self, variable: &mut Variable<'i>) -> Result<(), Self::Error> {
        if let Some(&index) = self.indices.get(variable.name.ident.as_ref())
            && !self.dependencies.contains(&index)
        {
            self.dependencies.push(index);
        }
        variable.visit_children(self)
    }
}

#[derive(Clone, Copy, Default)]
struct DependencyState {
    index: usize,
    lowlink: usize,
    on_stack: bool,
}

struct CustomPropertyOrder {
    dependencies: Vec<Vec<usize>>,
    states: Vec<DependencyState>,
    next_index: usize,
    stack: Vec<usize>,
    order: Vec<usize>,
}

impl CustomPropertyOrder {
    fn visit(&mut self, node: usize) {
        self.next_index += 1;
        let index = self.next_index;
        self.states[node] = DependencyState {
            index,
            lowlink: index,
            on_stack: true,
        };
        self.stack.push(node);

        for edge in 0..self.dependencies[node].len() {
            let dependency = self.dependencies[node][edge];
            let state = self.states[dependency];
            let lowlink = if state.index == 0 {
                self.visit(dependency);
                self.states[dependency].lowlink
            } else if state.on_stack {
                state.index
            } else {
                continue;
            };
            self.states[node].lowlink = self.states[node].lowlink.min(lowlink);
        }
        if self.states[node].lowlink != index {
            return;
        }
        let start = self
            .stack
            .iter()
            .rposition(|&member| member == node)
            .expect("component root is on the stack");
        let cyclic = self.stack.len() - start > 1 || self.dependencies[node].contains(&node);
        for member in self.stack.drain(start..) {
            self.states[member].on_stack = false;
        }
        if !cyclic {
            self.order.push(node);
        }
    }
}

fn custom_property_resolution_order(names: &[String], values: &mut [TokenList<'_>]) -> Vec<usize> {
    let indices = names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.as_str(), index))
        .collect();
    let dependencies = values
        .iter_mut()
        .map(|value| {
            let mut collector = CustomPropertyDependencyCollector {
                indices: &indices,
                dependencies: Vec::new(),
            };
            match value.visit(&mut collector) {
                Ok(()) => collector.dependencies,
                Err(error) => match error {},
            }
        })
        .collect();
    let mut graph = CustomPropertyOrder {
        dependencies,
        states: vec![DependencyState::default(); names.len()],
        next_index: 0,
        stack: Vec::new(),
        order: Vec::new(),
    };
    for node in 0..names.len() {
        if graph.states[node].index == 0 {
            graph.visit(node);
        }
    }
    graph.order
}

fn resolve_custom_properties<'a>(
    custom_properties: &mut FxHashMap<String, TokenList<'a>>,
    parent_custom: &FxHashMap<String, TokenList<'a>>,
) {
    let (names, mut values): (Vec<_>, Vec<_>) = take(custom_properties).into_iter().unzip();
    let order = custom_property_resolution_order(&names, &mut values);
    // The SCC walk excludes cycles and puts dependencies first. Acyclic
    // dependents can still recover from an excluded dependency via fallback.
    let mut variables = HashMap::new();
    for index in order {
        let name = names[index].as_str();
        let mut value = replace(&mut values[index], TokenList(Vec::new()));
        if !substitute_property_tokens(&mut value, &variables) {
            continue;
        }
        match token_list_css_wide_keyword(&value) {
            Some("initial") => continue,
            Some("inherit" | "unset") => {
                let Some(inherited) = parent_custom.get(name) else {
                    continue;
                };
                value = inherited.clone();
            }
            _ => {}
        }
        variables.insert(name, value);
    }
    *custom_properties = variables
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
}

/// Keep the component-value boundaries that surrounded each `var()` when
/// LightningCSS substitutes its token list. Its inliner splices lists directly,
/// and its printer otherwise turns adjacent identifiers such as `orange` and
/// `red` into the single, valid color `orangered`. Empty comments disappear
/// when the result is parsed, while preventing tokens on either side of the
/// substitution from being re-tokenized as one token.
pub(super) fn mark_var_substitution_boundaries(tokens: &mut TokenList<'_>) {
    for token in &mut tokens.0 {
        match token {
            TokenOrValue::Function(function) => {
                mark_var_substitution_boundaries(&mut function.arguments)
            }
            TokenOrValue::Var(variable) => {
                if let Some(fallback) = &mut variable.fallback {
                    mark_var_substitution_boundaries(fallback);
                }
            }
            TokenOrValue::Env(environment) => {
                if let Some(fallback) = &mut environment.fallback {
                    mark_var_substitution_boundaries(fallback);
                }
            }
            _ => {}
        }
    }

    let mut marked = Vec::with_capacity(tokens.0.len());
    for token in tokens.0.drain(..) {
        if matches!(token, TokenOrValue::Var(_)) {
            marked.push(TokenOrValue::Token(Token::Comment("".into())));
            marked.push(token);
            marked.push(TokenOrValue::Token(Token::Comment("".into())));
        } else {
            marked.push(token);
        }
    }
    tokens.0 = marked;
}

pub(super) fn resolve_single_var_property<'a>(
    unparsed: &UnparsedProperty<'a>,
    custom_properties: &FxHashMap<String, TokenList<'a>>,
) -> Option<Property<'a>> {
    let [TokenOrValue::Var(var)] = unparsed.value.0.as_slice() else {
        return None;
    };
    let replacement = custom_properties.get(var.name.ident.as_ref())?;
    parse_property_tokens(&unparsed.property_id, replacement)
}

pub(super) fn parse_property_tokens<'a>(
    id: &PropertyId<'_>,
    tokens: &TokenList<'_>,
) -> Option<Property<'a>> {
    let css = token_list_to_css_string(tokens)?;
    crate::style::syntax::value::parsed_property(id.clone(), &css).map(IntoOwned::into_owned)
}

pub(super) fn token_list_to_css_string(tokens: &TokenList<'_>) -> Option<String> {
    // A custom property preserves units on zero lengths during serialization.
    Property::Custom(CustomProperty {
        name: CowRcStr::from("--html-value").into(),
        value: tokens.clone(),
    })
    .value_to_css_string(PrinterOptions::default())
    .ok()
}

pub(super) fn substitute_property_tokens<'css>(
    tokens: &mut TokenList<'css>,
    variables: &HashMap<&str, TokenList<'css>>,
) -> bool {
    mark_var_substitution_boundaries(tokens);
    tokens.substitute_variables(variables);
    !needs_substitution(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_printing_preserves_zero_length_units() {
        use lightningcss::stylesheet::ParserOptions;
        use lightningcss::traits::ParseWithOptions;

        for css in ["", "0px", "calc(0px + 2em)", "var(--a, 0px)"] {
            let tokens =
                TokenList::parse_string_with_options(css, ParserOptions::default()).unwrap();
            assert_eq!(token_list_to_css_string(&tokens).as_deref(), Some(css));
        }
    }

    #[test]
    fn cache_keys_preserve_custom_order_parent_and_non_custom_filtering() {
        use crate::MediaEnvironment;
        use crate::allocation_test_support::count_allocations;
        use crate::style::cascade::resolver::specified::{EventScratch, build_cascade_events};
        use crate::style::rules::prepared::prepare_stylesheets;
        use lightningcss::stylesheet::{ParserOptions, StyleAttribute, StyleSheet};

        let inline = StyleAttribute::parse(
            "color:red;--a:1px;--b:2px;--a:3px!important",
            ParserOptions::default(),
        )
        .unwrap();
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let prepared = prepare_stylesheets(&user_agent, &[], MediaEnvironment::default(), 16.0);
        let element = ElementDeclarations {
            inline: Some(&inline),
            hints: &[],
        };
        let mut events = EventScratch::default();
        build_cascade_events(&prepared, &[], &[], element, &mut events);
        let mut cache = CustomCascadeCache::default();
        let signature = match cache.probe(&events.events, element, 1) {
            CustomCascadeProbe::Miss(signature) => signature,
            _ => panic!("new custom declarations require a signature"),
        };
        assert_eq!(signature.declarations.len(), 3);
        cache.insert(signature, 11);
        let (probe, allocations) = count_allocations(|| cache.probe(&events.events, element, 1));
        assert!(matches!(probe, CustomCascadeProbe::Hit(11)));
        assert_eq!(allocations, 0, "cache hits must remain allocation-free");
        assert!(matches!(
            cache.probe(&events.events, element, 2),
            CustomCascadeProbe::Miss(_)
        ));
        events.events.swap(0, 1);
        assert!(matches!(
            cache.probe(&events.events, element, 1),
            CustomCascadeProbe::Hit(11)
        ));
        events.events.reverse();
        assert!(matches!(
            cache.probe(&events.events, element, 1),
            CustomCascadeProbe::Miss(_)
        ));
        cache.entry_count = MAX_CUSTOM_CASCADE_CACHE_ENTRIES;
        assert!(matches!(
            cache.probe(&events.events, element, 1),
            CustomCascadeProbe::UncachedDeclarations
        ));
        events.events.reverse();
        assert!(matches!(
            cache.probe(&events.events, element, 1),
            CustomCascadeProbe::Hit(11)
        ));
        assert!(matches!(
            cache.probe(&[], element, 1),
            CustomCascadeProbe::NoDeclarations
        ));
    }

    #[test]
    fn dependency_collection_preserves_values_and_case_sensitive_names() {
        use lightningcss::stylesheet::{ParserOptions, StyleAttribute};

        let inline = StyleAttribute::parse(
            "--c:rgb(var(--b) 0 0); --b:calc(var(--missing,var(--a)) + var(--a)); --a:var(--A); --A:1px",
            ParserOptions::default(),
        ).unwrap();
        let (names, mut values): (Vec<_>, Vec<_>) = inline
            .declarations
            .declarations
            .iter()
            .map(|property| {
                let (name, value) = custom_declaration(property).unwrap();
                (name.to_owned(), value.clone())
            })
            .unzip();
        let original = values.clone();
        assert_eq!(
            custom_property_resolution_order(&names, &mut values),
            [3, 2, 1, 0]
        );
        assert_eq!(values, original);
    }

    #[test]
    fn resolution_order_matches_reachability_for_every_four_node_graph() {
        let names = [0, 1, 2, 3];
        for edges in 0u32..1 << 16 {
            let mut reachable = [[false; 4]; 4];
            let dependencies = names
                .iter()
                .enumerate()
                .map(|(from, _)| {
                    names
                        .iter()
                        .enumerate()
                        .filter_map(|(to, &dependency)| {
                            let connected = edges & (1 << (from * 4 + to)) != 0;
                            reachable[from][to] = connected;
                            connected.then_some(dependency)
                        })
                        .collect()
                })
                .collect();
            // Transitive reachability is independent of the Tarjan traversal:
            // self-reachability defines exactly which nodes must be excluded.
            for via in 0..4 {
                for from in 0..4 {
                    for to in 0..4 {
                        reachable[from][to] |= reachable[from][via] && reachable[via][to];
                    }
                }
            }
            let mut graph = CustomPropertyOrder {
                dependencies,
                states: vec![DependencyState::default(); 4],
                next_index: 0,
                stack: Vec::new(),
                order: Vec::new(),
            };
            for name in names {
                if graph.states[name].index == 0 {
                    graph.visit(name);
                }
            }
            assert_eq!(
                graph.order.len(),
                (0..4).filter(|&node| !reachable[node][node]).count(),
                "graph {edges:#x}"
            );
            for (node, name) in names.iter().enumerate() {
                let position = graph.order.iter().position(|member| member == name);
                assert_eq!(
                    position.is_some(),
                    !reachable[node][node],
                    "{name} in graph {edges:#x}"
                );
                if let Some(position) = position {
                    for dependency in &graph.dependencies[*name] {
                        if let Some(dependency_position) =
                            graph.order.iter().position(|member| member == dependency)
                        {
                            assert!(
                                dependency_position < position,
                                "{name} depends on {dependency} in graph {edges:#x}"
                            );
                        }
                    }
                }
            }
        }
    }
}
