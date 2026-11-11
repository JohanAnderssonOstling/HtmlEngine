//! Custom-property collection, cycle detection, inheritance, and substitution.

use super::specified::{
    CandidateAction, CascadeBoundary, CascadeRollback, DeclarationEvent, ElementDeclarations,
    RollbackMap, rollback_keyword};
use crate::style::syntax::wide_keywords::CssWideKeyword;
use crate::style::syntax::wide_keywords::token_list_css_wide_keyword;
use lightningcss::properties::custom::{
    Token, TokenList, TokenOrValue, UnparsedProperty, UnresolvedColor};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::visitor::{Visit, Visitor};
use rustc_data_structures::fx::{FxHashMap, FxHasher};
use static_self::IntoOwned;
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::iter::once;
use std::ops::Range;

pub(super) use crate::style::syntax::substitution::needs_substitution;
pub(super) use crate::style::syntax::value::token_list_to_css_string;

const INVALID_VALUE: u32 = u32::MAX;
const MAX_MAP_DEPTH: u8 = 8;
const UNDECIDED: u32 = u32::MAX;
const INHERITED: u32 = u32::MAX - 1;

struct CustomMap {
    parent: u32,
    entries: Range<usize>,
    depth: u8,
}

/// Names are allocated once per resolution. Maps contain only IDs, and token
/// payloads live in a single pool shared by all descendants.
pub(super) struct CustomPropertyStore<'css> {
    names: FxHashMap<String, u32>,
    values: Vec<TokenList<'css>>,
    maps: Vec<CustomMap>,
    entry_names: Vec<u32>,
    entry_values: Vec<u32>,
}

impl Default for CustomPropertyStore<'_> {
    fn default() -> Self {
        Self {
            names: FxHashMap::default(),
            values: Vec::new(),
            maps: vec![CustomMap {
                parent: 0,
                entries: 0..0,
                depth: 0,
            }],
            entry_names: Vec::new(),
            entry_values: Vec::new(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct CustomProperties<'store, 'css> {
    store: &'store CustomPropertyStore<'css>,
    map: u32,
}

impl<'store, 'css> CustomProperties<'store, 'css> {
    /// An immutable environment's index. Temporary pseudo-map indices can be
    /// reused after rewind and must not key entries retained by the cascade.
    pub(super) fn map_id(&self) -> u32 {
        self.map
    }

    fn get(&self, name: &str) -> Option<&'store TokenList<'css>> {
        let name = *self.store.names.get(name)?;
        self.store
            .values
            .get(self.store.lookup(self.map, name) as usize)
    }
}

impl<'css> CustomPropertyStore<'css> {
    pub(super) fn view(&self, map: u32) -> CustomProperties<'_, 'css> {
        CustomProperties { store: self, map }
    }

    fn intern_name(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.names.get(name) {
            return id;
        }
        let id = u32::try_from(self.names.len()).expect("custom property names fit in u32");
        self.names.insert(name.to_owned(), id);
        id
    }

    fn push_value(&mut self, value: TokenList<'css>) -> u32 {
        let id = u32::try_from(self.values.len()).expect("custom property values fit in u32");
        assert_ne!(id, INVALID_VALUE);
        self.values.push(value);
        id
    }

    fn lookup(&self, mut map: u32, name: u32) -> u32 {
        while map != 0 {
            let data = &self.maps[map as usize];
            if let Ok(index) = self.entry_names[data.entries.clone()].binary_search(&name) {
                return self.entry_values[data.entries.start + index];
            }
            map = data.parent;
        }
        INVALID_VALUE
    }

    fn push_map(&mut self, mut parent: u32, scratch: &mut CustomPropertyScratch<'css>) -> u32 {
        let entries = &mut scratch.entries;
        entries.clear();
        entries.extend(
            scratch
                .names
                .iter()
                .copied()
                .zip(scratch.variables.iter().copied()),
        );
        // Bound lookup cost on long chains of overrides. Compaction copies IDs
        // only, uses retained scratch, and happens once per eight overlay levels.
        if self.maps[parent as usize].depth == MAX_MAP_DEPTH {
            scratch.merged.clear();
            scratch.merged.extend(entries.drain(..));
            while parent != 0 {
                let data = &self.maps[parent as usize];
                for index in data.entries.clone() {
                    scratch
                        .merged
                        .entry(self.entry_names[index])
                        .or_insert(self.entry_values[index]);
                }
                parent = data.parent;
            }
            entries.extend(
                scratch
                    .merged
                    .drain()
                    .filter(|&(_, value)| value != INVALID_VALUE),
            );
        }
        if entries.is_empty() {
            return parent;
        }
        entries.sort_unstable_by_key(|&(name, _)| name);
        let start = self.entry_names.len();
        for &(name, value) in entries.iter() {
            self.entry_names.push(name);
            self.entry_values.push(value);
        }
        let map = u32::try_from(self.maps.len()).expect("custom property maps fit in u32");
        self.maps.push(CustomMap {
            parent,
            entries: start..self.entry_names.len(),
            depth: self.maps[parent as usize].depth + 1,
        });
        map
    }

    // Pseudo maps are temporary: computed styles retain parsed results, never
    // references into this pool. Reclaim their payloads after applying the cascade.
    pub(super) fn checkpoint(&self) -> (usize, usize, usize) {
        (self.values.len(), self.maps.len(), self.entry_names.len())
    }

    pub(super) fn rewind(&mut self, checkpoint: (usize, usize, usize)) {
        self.values.truncate(checkpoint.0);
        self.maps.truncate(checkpoint.1);
        self.entry_names.truncate(checkpoint.2);
        self.entry_values.truncate(checkpoint.2);
    }
}

#[derive(Default)]
pub(super) struct CustomPropertyScratch<'css> {
    names: Vec<u32>,
    values: Vec<Option<TokenList<'css>>>,
    variables: Vec<u32>,
    // Indexed by interned name. A slot is a local value index, INHERITED,
    // or UNDECIDED. Reset only touched slots between elements.
    indices: Vec<u32>,
    decided: Vec<u32>,
    rollbacks: RollbackMap<u32>,
    entries: Vec<(u32, u32)>,
    merged: FxHashMap<u32, u32>,
    graph: CustomPropertyOrder,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct CustomDeclarationIdentity {
    property_address: usize,
    boundary: CascadeBoundary,
}

pub(super) struct CustomCascadeSignature {
    hash: u64,
    parent_map_id: u32,
    declarations: Range<usize>,
}

struct CustomCascadeCacheEntry {
    next: Option<usize>,
    parent_map_id: u32,
    declarations: Range<usize>,
    map_id: u32,
}

#[derive(Default)]
pub(super) struct CustomCascadeCache {
    buckets: FxHashMap<u64, usize>,
    entries: Vec<CustomCascadeCacheEntry>,
    declarations: Vec<CustomDeclarationIdentity>,
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
        &mut self,
        events: &[DeclarationEvent],
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

        let mut next = self.buckets.get(&hash).copied();
        while let Some(index) = next {
            let entry = &self.entries[index];
            if entry.parent_map_id == parent_map_id
                && declarations
                    .clone()
                    .eq(self.declarations[entry.declarations.clone()]
                        .iter()
                        .copied())
            {
                return CustomCascadeProbe::Hit(entry.map_id);
            }
            next = entry.next;
        }
        if self.entries.len() >= MAX_CUSTOM_CASCADE_CACHE_ENTRIES {
            return CustomCascadeProbe::UncachedDeclarations;
        }
        let start = self.declarations.len();
        self.declarations.extend(declarations);
        let declarations = start..self.declarations.len();
        CustomCascadeProbe::Miss(CustomCascadeSignature {
            hash,
            parent_map_id,
            declarations,
        })
    }

    pub(super) fn insert(&mut self, signature: CustomCascadeSignature, map_id: u32) {
        let next = self.buckets.insert(signature.hash, self.entries.len());
        self.entries.push(CustomCascadeCacheEntry {
            next,
            parent_map_id: signature.parent_map_id,
            declarations: signature.declarations,
            map_id,
        });
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
    events: &[DeclarationEvent],
    element: ElementDeclarations<'_, 'css>,
    store: &mut CustomPropertyStore<'css>,
    parent: u32,
    scratch: &mut CustomPropertyScratch<'css>,
) -> u32 {
    scratch.names.clear();
    scratch.values.clear();
    scratch.variables.clear();
    for name in scratch.decided.drain(..) {
        scratch.indices[name as usize] = UNDECIDED;
    }
    scratch.rollbacks.clear();
    let mut resolve = false;
    for event in events.iter().rev() {
        let Some((name, value)) = custom_declaration(event.property(element)) else {
            continue;
        };
        let name = store.intern_name(name);
        if scratch
            .indices
            .get(name as usize)
            .is_some_and(|&slot| slot != UNDECIDED)
        {
            continue;
        }
        let keyword = token_list_css_wide_keyword(value);
        match scratch
            .rollbacks
            .action(name, event.boundary, keyword.and_then(rollback_keyword))
        {
            CandidateAction::Excluded => continue,
            CandidateAction::Rollback(kind) => {
                scratch
                    .rollbacks
                    .push(name, CascadeRollback::new(event.boundary, kind));
                continue;
            }
            CandidateAction::Select => {}
        }
        if scratch.indices.len() <= name as usize {
            scratch.indices.resize(name as usize + 1, UNDECIDED);
        }
        scratch.decided.push(name);
        if matches!(
            keyword,
            Some(CssWideKeyword::Inherit | CssWideKeyword::Unset)
        ) {
            scratch.indices[name as usize] = INHERITED;
            continue;
        }
        let index = u32::try_from(scratch.names.len()).expect("local custom properties fit in u32");
        assert!(
            index < INHERITED,
            "local custom property index is not a sentinel"
        );
        scratch.indices[name as usize] = index;
        scratch.names.push(name);
        scratch
            .values
            .push(if keyword == Some(CssWideKeyword::Initial) {
                None
            } else {
                resolve |= needs_substitution(value);
                Some(value.clone())
            });
    }
    if scratch.names.is_empty() {
        return parent;
    }
    scratch.variables.resize(scratch.names.len(), INVALID_VALUE);
    if resolve {
        resolve_custom_properties(store, parent, scratch);
    } else {
        for (value, result) in scratch.values.iter_mut().zip(&mut scratch.variables) {
            if let Some(value) = value.take() {
                *result = store.push_value(value);
            }
        }
    }
    let map = store.push_map(parent, scratch);
    scratch.values.clear();
    map
}

fn local_index(indices: &[u32], name: u32) -> Option<usize> {
    indices
        .get(name as usize)
        .copied()
        .filter(|&index| index < INHERITED)
        .map(|index| index as usize)
}

// Walk immutable tokens so inherited payloads never need copy-on-write just
// to discover dependencies. Fallback references also participate in cycles.
fn collect_dependencies(
    tokens: &TokenList<'_>,
    names: &FxHashMap<String, u32>,
    indices: &[u32],
    edges: &mut Vec<usize>,
    start: usize,
) {
    for token in &tokens.0 {
        match token {
            TokenOrValue::Var(variable) => {
                if let Some(index) = names
                    .get(variable.name.ident.as_ref())
                    .and_then(|&name| local_index(indices, name))
                    && !edges[start..].contains(&index)
                {
                    edges.push(index);
                }
                if let Some(fallback) = &variable.fallback {
                    collect_dependencies(fallback, names, indices, edges, start);
                }
            }
            TokenOrValue::Function(function) => {
                collect_dependencies(&function.arguments, names, indices, edges, start);
            }
            TokenOrValue::UnresolvedColor(color) => match color {
                UnresolvedColor::RGB { alpha, .. } | UnresolvedColor::HSL { alpha, .. } => {
                    collect_dependencies(alpha, names, indices, edges, start);
                }
                UnresolvedColor::LightDark { light, dark } => {
                    collect_dependencies(light, names, indices, edges, start);
                    collect_dependencies(dark, names, indices, edges, start);
                }
            },
            TokenOrValue::Env(environment) => {
                if let Some(fallback) = &environment.fallback {
                    collect_dependencies(fallback, names, indices, edges, start);
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Default)]
struct DependencyState {
    index: usize,
    lowlink: usize,
    on_stack: bool,
}

#[derive(Default)]
struct CustomPropertyOrder {
    dependencies: Vec<Range<usize>>,
    edges: Vec<usize>,
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

        for edge in self.dependencies[node].clone() {
            let dependency = self.edges[edge];
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
        let cyclic = self.stack.len() - start > 1
            || self.edges[self.dependencies[node].clone()].contains(&node);
        for member in self.stack.drain(start..) {
            self.states[member].on_stack = false;
        }
        if !cyclic {
            self.order.push(node);
        }
    }
}

fn custom_property_resolution_order(
    names: &FxHashMap<String, u32>,
    indices: &[u32],
    values: &[Option<TokenList<'_>>],
    graph: &mut CustomPropertyOrder,
) {
    graph.dependencies.clear();
    graph.edges.clear();
    graph.states.clear();
    graph
        .states
        .resize(values.len(), DependencyState::default());
    graph.next_index = 0;
    graph.stack.clear();
    graph.order.clear();
    for value in values {
        let start = graph.edges.len();
        if let Some(value) = value {
            collect_dependencies(value, names, indices, &mut graph.edges, start);
        }
        graph.dependencies.push(start..graph.edges.len());
    }
    for node in 0..values.len() {
        if graph.states[node].index == 0 {
            graph.visit(node);
        }
    }
}

fn resolve_custom_properties<'css>(
    store: &mut CustomPropertyStore<'css>,
    parent: u32,
    scratch: &mut CustomPropertyScratch<'css>,
) {
    custom_property_resolution_order(
        &store.names,
        &scratch.indices,
        &scratch.values,
        &mut scratch.graph,
    );
    for &index in &scratch.graph.order {
        let Some(mut value) = scratch.values[index].take() else {
            continue;
        };
        if !substitute_with(&mut value, |name| {
            let name = *store.names.get(name)?;
            let value = local_index(&scratch.indices, name).map_or_else(
                || store.lookup(parent, name),
                |index| scratch.variables[index],
            );
            store.values.get(value as usize)
        }) {
            continue;
        }
        scratch.variables[index] = match token_list_css_wide_keyword(&value) {
            Some(CssWideKeyword::Initial) => INVALID_VALUE,
            Some(CssWideKeyword::Inherit | CssWideKeyword::Unset) => {
                store.lookup(parent, scratch.names[index])
            }
            _ => store.push_value(value),
        };
    }
}

pub(super) fn resolve_single_var_property<'a>(
    unparsed: &UnparsedProperty<'a>,
    custom_properties: &CustomProperties<'_, 'a>,
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

/// Read values directly from the caller's map. Only tokens actually inserted
/// into the declaration are copied; unrelated custom properties stay borrowed.
pub(super) fn substitute_property_tokens<'css>(
    tokens: &mut TokenList<'css>,
    variables: &CustomProperties<'_, 'css>,
) -> bool {
    substitute_with(tokens, |name| variables.get(name))
}

fn substitute_with<'values, 'css: 'values>(
    tokens: &mut TokenList<'css>,
    lookup: impl Fn(&str) -> Option<&'values TokenList<'css>>,
) -> bool {
    struct Inliner<F>(F);
    impl<'values, 'css: 'values, F> Visitor<'css> for Inliner<F>
    where
        F: Fn(&str) -> Option<&'values TokenList<'css>>,
    {
        type Error = Infallible;

        fn visit_types(&self) -> lightningcss::visitor::VisitTypes {
            lightningcss::visit_types!(TOKENS | VARIABLES)
        }

        fn visit_token_list(&mut self, tokens: &mut TokenList<'css>) -> Result<(), Infallible> {
            let mut index = 0;
            while index < tokens.0.len() {
                let token = &mut tokens.0[index];
                // Resolve nested functions and fallbacks before moving them.
                token.visit(self)?;
                if let TokenOrValue::Var(variable) = token {
                    // Lookup returns computed values. Cycles were excluded by
                    // the dependency graph, so inserted tokens need no revisit.
                    let comment = || TokenOrValue::Token(Token::Comment("".into()));
                    if let Some(value) = (self.0)(variable.name.ident.as_ref()) {
                        let count = value.0.len() + 2;
                        tokens.0.splice(
                            index..index + 1,
                            once(comment())
                                .chain(value.0.iter().cloned())
                                .chain(once(comment())),
                        );
                        index += count;
                        continue;
                    } else if let Some(fallback) = variable.fallback.take() {
                        let count = fallback.0.len() + 2;
                        tokens.0.splice(
                            index..index + 1,
                            once(comment()).chain(fallback.0).chain(once(comment())),
                        );
                        index += count;
                        continue;
                    }
                }
                index += 1;
            }
            Ok(())
        }
    }

    match tokens.visit(&mut Inliner(lookup)) {
        Ok(()) => !needs_substitution(tokens),
        Err(error) => match error {},
    }
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
        let inline = crate::style::source::inline::PreparedInlineStyle::from(inline);
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let prepared = prepare_stylesheets(&user_agent, &[], MediaEnvironment::default(), 16.0);
        let element = ElementDeclarations {
            prepared: None,
            inline: Some(&inline),
            hints: &[],
            hint_preparation: None,
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
        while cache.entries.len() < MAX_CUSTOM_CASCADE_CACHE_ENTRIES {
            cache.entries.push(CustomCascadeCacheEntry {
                next: None,
                parent_map_id: 0,
                declarations: 0..0,
                map_id: 0,
            });
        }
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
        let inline = crate::style::source::inline::PreparedInlineStyle::from(inline);
        let (names, values): (Vec<String>, Vec<_>) = inline
            .declarations
            .declarations
            .iter()
            .map(|property| {
                let (name, value) = custom_declaration(property).unwrap();
                (name.to_owned(), Some(value.clone()))
            })
            .unzip();
        let original = values.clone();
        let names: FxHashMap<_, _> = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| (name, index as u32))
            .collect();
        let indices: Vec<_> = (0..values.len() as u32).collect();
        let mut graph = CustomPropertyOrder::default();
        custom_property_resolution_order(&names, &indices, &values, &mut graph);
        assert_eq!(graph.order, [3, 2, 1, 0]);
        assert_eq!(values, original);
    }

    #[test]
    fn resolution_order_matches_reachability_for_every_four_node_graph() {
        let names = [0, 1, 2, 3];
        for edges in 0u32..1 << 16 {
            let mut reachable = [[false; 4]; 4];
            let mut dependency_edges = Vec::new();
            let dependencies = names
                .iter()
                .enumerate()
                .map(|(from, _)| {
                    let start = dependency_edges.len();
                    for (to, &dependency) in names.iter().enumerate() {
                        let connected = edges & (1 << (from * 4 + to)) != 0;
                        reachable[from][to] = connected;
                        if connected {
                            dependency_edges.push(dependency);
                        }
                    }
                    start..dependency_edges.len()
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
                edges: dependency_edges,
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
                    for dependency in &graph.edges[graph.dependencies[*name].clone()] {
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
