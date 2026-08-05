//! The boundary between parser output and selector/cascade input.
//!
//! A `ParsedStylesheetSet` is only a borrowed view of Lightning CSS parser
//! output. Calling `prepare` validates which style rules are effective and
//! assigns compact handles plus cascade metadata. Downstream stages cannot
//! accidentally index arbitrary parser nodes because they accept only a
//! `PreparedRuleSet` and its private `EffectiveRuleId` values.

use super::media::{CompiledMediaList, MediaEnvironment, MediaQuerySet};
use super::selectors::selector_list_is_web_valid;
use crate::{PropertyCapability, PropertySyntax, declaration_support, supports_selector_syntax_is_valid};
use html_dom::{Document, DomNodeId};
use lightningcss::rules::layer::LayerName;
use lightningcss::rules::style::StyleRule;
use lightningcss::rules::supports::SupportsCondition;
use lightningcss::rules::{CssRule, CssRuleList};
use lightningcss::selector::SelectorList;
use lightningcss::stylesheet::StyleSheet;
use rustc_data_structures::fx::FxHashMap;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub(crate) enum CascadeOrigin {
    UserAgent = 0,
    Author = 1,
}

/// Integer layer order. Unlayered rules currently share the sentinel rank;
/// layer expansion can assign concrete ranks without changing selector or
/// resolver APIs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LayerOrder(u32);

impl LayerOrder {
    const UNLAYERED: Self = Self(u32::MAX);

    fn from_layer_id(id: LayerId) -> Self {
        Self(id.0)
    }

    fn is_unlayered(self) -> bool {
        self == Self::UNLAYERED
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RulePriority {
    origin: CascadeOrigin,
    layer: LayerOrder,
    source_order: u32,
}

impl RulePriority {
    pub(crate) fn source_order(self) -> u32 {
        self.source_order
    }

    pub(crate) fn origin(self) -> CascadeOrigin {
        self.origin
    }

    pub(crate) fn layer(self) -> LayerOrder {
        self.layer
    }

    pub(crate) fn same_origin_and_layer(self, other: Self) -> bool {
        self.origin == other.origin && self.layer == other.layer
    }

    pub(crate) fn compare_normal(self, specificity: u32, scope_proximity: u32, other: Self, other_specificity: u32, other_scope_proximity: u32) -> Ordering {
        self.origin
            .cmp(&other.origin)
            .then_with(|| self.layer.cmp(&other.layer))
            .then_with(|| specificity.cmp(&other_specificity))
            .then_with(|| other_scope_proximity.cmp(&scope_proximity))
            .then_with(|| self.source_order.cmp(&other.source_order))
    }

    pub(crate) fn compare_important(self, specificity: u32, scope_proximity: u32, other: Self, other_specificity: u32, other_scope_proximity: u32) -> Ordering {
        fn layer_rank(layer: LayerOrder) -> (u8, u32) {
            if layer.is_unlayered() { (0, 0) } else { (1, u32::MAX - layer.0) }
        }
        self.origin
            .cmp(&other.origin)
            .then_with(|| layer_rank(self.layer).cmp(&layer_rank(other.layer)))
            .then_with(|| specificity.cmp(&other_specificity))
            .then_with(|| other_scope_proximity.cmp(&scope_proximity))
            .then_with(|| self.source_order.cmp(&other.source_order))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct LayerId(u32);

#[derive(Default)]
struct LayerNode {
    children: Vec<LayerId>,
}

#[derive(Default)]
struct LayerRegistry {
    roots: Vec<LayerId>,
    nodes: Vec<LayerNode>,
    named: FxHashMap<(Option<LayerId>, String), LayerId>,
}

impl LayerRegistry {
    fn register_named(&mut self, parent: Option<LayerId>, name: &LayerName<'_>) -> LayerId {
        let mut parent = parent;
        for part in &name.0 {
            let key = (parent, part.as_ref().to_owned());
            let id = if let Some(id) = self.named.get(&key).copied() {
                id
            } else {
                let id = self.push_node(parent);
                self.named.insert(key, id);
                id
            };
            parent = Some(id);
        }
        parent.expect("a parsed layer name always has at least one component")
    }

    fn register_anonymous(&mut self, parent: Option<LayerId>) -> LayerId {
        self.push_node(parent)
    }

    fn push_node(&mut self, parent: Option<LayerId>) -> LayerId {
        let id = LayerId(u32::try_from(self.nodes.len()).expect("layer IDs fit in u32"));
        self.nodes.push(LayerNode::default());
        if let Some(parent) = parent {
            self.nodes[parent.0 as usize].children.push(id);
        } else {
            self.roots.push(id);
        }
        id
    }

    fn ranks(&self) -> Vec<LayerOrder> {
        fn visit(id: LayerId, nodes: &[LayerNode], ranks: &mut [LayerOrder], next: &mut u32) {
            for child in &nodes[id.0 as usize].children {
                visit(*child, nodes, ranks, next);
            }
            ranks[id.0 as usize] = LayerOrder(*next);
            *next = next.checked_add(1).expect("layer ranks fit in u32");
        }

        let mut ranks = vec![LayerOrder::UNLAYERED; self.nodes.len()];
        let mut next = 0;
        for root in &self.roots {
            visit(*root, &self.nodes, &mut ranks, &mut next);
        }
        ranks
    }
}

/// Dense, compact identifier issued only by `PreparedRuleSet`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct EffectiveRuleId(u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScopeId(u32);

impl ScopeId {
    const NONE: Self = Self(u32::MAX);
}

struct ScopeDescriptor<'sheet, 'css> {
    parent: ScopeId,
    start: Option<&'sheet SelectorList<'css>>,
    end: Option<&'sheet SelectorList<'css>>,
    implicit_root: Option<DomNodeId>,
}

#[derive(Clone, Copy)]
pub(crate) struct ScopeMatch {
    pub(crate) root: DomNodeId,
    pub(crate) proximity: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct EffectiveRule<'sheet, 'css> {
    style_rule: &'sheet StyleRule<'css>,
    priority: RulePriority,
    scope: ScopeId,
}

impl<'sheet, 'css> EffectiveRule<'sheet, 'css> {
    pub(crate) fn style_rule(self) -> &'sheet StyleRule<'css> {
        self.style_rule
    }

    pub(crate) fn priority(self) -> RulePriority {
        self.priority
    }

    pub(crate) fn scope(self) -> ScopeId {
        self.scope
    }
}

/// Parser output plus the split between the built-in user-agent sheet and
/// author sheets. Its fields are private so preparation is the only route to
/// effective rules.
pub(crate) struct ParsedStylesheetSet<'sheet, 'css> {
    user_agent: &'sheet StyleSheet<'css>,
    authors: &'sheet [StyleSheet<'css>],
    author_roots: &'sheet [Option<DomNodeId>],
}

impl<'sheet, 'css> ParsedStylesheetSet<'sheet, 'css> {
    pub(crate) fn new(user_agent: &'sheet StyleSheet<'css>, authors: &'sheet [StyleSheet<'css>]) -> Self {
        Self { user_agent, authors, author_roots: &[] }
    }

    pub(crate) fn with_author_roots(user_agent: &'sheet StyleSheet<'css>, authors: &'sheet [StyleSheet<'css>], author_roots: &'sheet [Option<DomNodeId>]) -> Self {
        debug_assert_eq!(authors.len(), author_roots.len());
        Self { user_agent, authors, author_roots }
    }

    pub(crate) fn prepare(self, environment: MediaEnvironment, initial_font_size: f64) -> PreparedRuleSet<'sheet, 'css> {
        let capacity = self.user_agent.rules.0.len() + self.authors.iter().map(|stylesheet| stylesheet.rules.0.len()).sum::<usize>();
        let mut rules = Vec::with_capacity(capacity);
        let mut source_order = 0u32;
        let mut media_queries = MediaQuerySet::default();
        let mut media_path = Vec::new();
        let mut scopes = Vec::new();

        prepare_origin(std::iter::once((self.user_agent, None)), CascadeOrigin::UserAgent, &mut rules, &mut scopes, &mut source_order, environment, initial_font_size, &mut media_queries, &mut media_path);
        let authors = self.authors.iter().enumerate().map(|(index, stylesheet)| (stylesheet, self.author_roots.get(index).copied().flatten()));
        prepare_origin(authors, CascadeOrigin::Author, &mut rules, &mut scopes, &mut source_order, environment, initial_font_size, &mut media_queries, &mut media_path);

        PreparedRuleSet { rules, scopes, media_queries, environment }
    }
}

fn prepare_origin<'sheet, 'css>(
    stylesheets: impl IntoIterator<Item = (&'sheet StyleSheet<'css>, Option<DomNodeId>)>, origin: CascadeOrigin, output: &mut Vec<EffectiveRule<'sheet, 'css>>, scopes: &mut Vec<ScopeDescriptor<'sheet, 'css>>, source_order: &mut u32,
    environment: MediaEnvironment, initial_font_size: f64, media_queries: &mut MediaQuerySet, media_path: &mut Vec<u32>,
) {
    let first_rule = output.len();
    let mut layers = LayerRegistry::default();
    for (stylesheet, implicit_root) in stylesheets {
        collect_effective_rules(&stylesheet.rules, origin, None, ScopeId::NONE, implicit_root, &mut layers, output, scopes, source_order, environment, initial_font_size, media_queries, media_path);
    }

    let ranks = layers.ranks();
    for rule in &mut output[first_rule..] {
        if !rule.priority.layer.is_unlayered() {
            rule.priority.layer = ranks[rule.priority.layer.0 as usize];
        }
    }
}

fn collect_effective_rules<'sheet, 'css>(
    rule_list: &'sheet CssRuleList<'css>, origin: CascadeOrigin, current_layer: Option<LayerId>, current_scope: ScopeId, implicit_root: Option<DomNodeId>, layers: &mut LayerRegistry, output: &mut Vec<EffectiveRule<'sheet, 'css>>,
    scopes: &mut Vec<ScopeDescriptor<'sheet, 'css>>, source_order: &mut u32, environment: MediaEnvironment, initial_font_size: f64, media_queries: &mut MediaQuerySet, media_path: &mut Vec<u32>,
) {
    for rule in &rule_list.0 {
        match rule {
            CssRule::Style(style_rule) if selector_list_is_web_valid(&style_rule.selectors) => {
                media_queries.record_dependency(media_path);
                let layer = current_layer.map_or(LayerOrder::UNLAYERED, LayerOrder::from_layer_id);
                output.push(EffectiveRule { style_rule, priority: RulePriority { origin, layer, source_order: *source_order }, scope: current_scope });
                *source_order = source_order.checked_add(1).expect("a stylesheet cannot contain more than u32::MAX effective rules");
            }
            CssRule::Media(media) => {
                let compiled = CompiledMediaList::compile(&media.query);
                let applies = compiled.evaluate(environment, initial_font_size).is_true();
                let query_id = media_queries.register_query(compiled);
                media_path.push(query_id);
                if applies {
                    collect_effective_rules(&media.rules, origin, current_layer, current_scope, implicit_root, layers, output, scopes, source_order, environment, initial_font_size, media_queries, media_path);
                } else {
                    collect_media_dependencies(&media.rules, media_queries, media_path);
                }
                media_path.pop();
            }
            CssRule::Supports(supports) if supports_condition_applies(&supports.condition) => {
                collect_effective_rules(&supports.rules, origin, current_layer, current_scope, implicit_root, layers, output, scopes, source_order, environment, initial_font_size, media_queries, media_path);
            }
            CssRule::LayerStatement(statement) => {
                media_queries.record_dependency(media_path);
                for name in &statement.names {
                    layers.register_named(current_layer, name);
                }
            }
            CssRule::LayerBlock(block) => {
                media_queries.record_dependency(media_path);
                let layer = match &block.name {
                    Some(name) => layers.register_named(current_layer, name),
                    None => layers.register_anonymous(current_layer),
                };
                collect_effective_rules(&block.rules, origin, Some(layer), current_scope, implicit_root, layers, output, scopes, source_order, environment, initial_font_size, media_queries, media_path);
            }
            CssRule::Scope(scope) => {
                let start_valid = scope.scope_start.as_ref().is_none_or(selector_list_is_web_valid);
                let end_valid = scope.scope_end.as_ref().is_none_or(selector_list_is_web_valid);
                if start_valid && end_valid {
                    let scope_id = ScopeId(u32::try_from(scopes.len()).expect("scope IDs fit in u32"));
                    scopes.push(ScopeDescriptor { parent: current_scope, start: scope.scope_start.as_ref(), end: scope.scope_end.as_ref(), implicit_root });
                    collect_effective_rules(&scope.rules, origin, current_layer, scope_id, implicit_root, layers, output, scopes, source_order, environment, initial_font_size, media_queries, media_path);
                }
            }
            _ => {}
        }
    }
}

fn collect_media_dependencies(rule_list: &CssRuleList<'_>, media_queries: &mut MediaQuerySet, media_path: &mut Vec<u32>) {
    for rule in &rule_list.0 {
        match rule {
            CssRule::Style(style_rule) if selector_list_is_web_valid(&style_rule.selectors) => media_queries.record_dependency(media_path),
            CssRule::Media(media) => {
                let query_id = media_queries.register_query(CompiledMediaList::compile(&media.query));
                media_path.push(query_id);
                collect_media_dependencies(&media.rules, media_queries, media_path);
                media_path.pop();
            }
            CssRule::Supports(supports) if supports_condition_applies(&supports.condition) => collect_media_dependencies(&supports.rules, media_queries, media_path),
            CssRule::LayerStatement(_) => media_queries.record_dependency(media_path),
            CssRule::LayerBlock(block) => {
                media_queries.record_dependency(media_path);
                collect_media_dependencies(&block.rules, media_queries, media_path);
            }
            CssRule::Scope(scope) => collect_media_dependencies(&scope.rules, media_queries, media_path),
            _ => {}
        }
    }
}

fn supports_condition_applies(condition: &SupportsCondition<'_>) -> bool {
    match condition {
        SupportsCondition::Not(condition) => !supports_condition_applies(condition),
        SupportsCondition::And(conditions) => conditions.iter().all(supports_condition_applies),
        SupportsCondition::Or(conditions) => conditions.iter().any(supports_condition_applies),
        SupportsCondition::Declaration { property_id, value } => {
            let Some(value) = supports_declaration_value(value.as_ref()) else {
                return false;
            };
            let support = declaration_support(property_id.name(), value);
            support.syntax == PropertySyntax::Valid && matches!(support.capability, PropertyCapability::Supported)
        }
        SupportsCondition::Selector(selector) => supports_selector_syntax_is_valid(selector),
        SupportsCondition::Unknown(_) => false,
    }
}

/// Returns the declaration value without a valid `!important` priority.
///
/// A top-level exclamation mark is not part of a declaration value. CSS Syntax
/// only accepts it here as the start of the trailing `!important` priority;
/// notably, custom properties must not make `!bogus` feature queries true.
fn supports_declaration_value(value: &str) -> Option<&str> {
    let value = value.trim();
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0u32;
    let mut top_level_bang = None;

    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' | '[' | '{' => depth = depth.saturating_add(1),
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '!' if depth == 0 => {
                if top_level_bang.is_some() {
                    return None;
                }
                top_level_bang = Some(index);
            }
            _ => {}
        }
    }

    match top_level_bang {
        Some(bang) if value[bang + 1..].trim().eq_ignore_ascii_case("important") => Some(value[..bang].trim_end()),
        Some(_) => None,
        None => Some(value),
    }
}

pub(crate) struct PreparedRuleSet<'sheet, 'css> {
    rules: Vec<EffectiveRule<'sheet, 'css>>,
    scopes: Vec<ScopeDescriptor<'sheet, 'css>>,
    media_queries: MediaQuerySet,
    environment: MediaEnvironment,
}

impl<'sheet, 'css> PreparedRuleSet<'sheet, 'css> {
    pub(crate) fn media_queries(&self) -> &MediaQuerySet {
        &self.media_queries
    }
    pub(crate) fn environment(&self) -> MediaEnvironment {
        self.environment
    }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rules.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (EffectiveRuleId, EffectiveRule<'sheet, 'css>)> + '_ {
        self.rules.iter().copied().enumerate().map(|(index, rule)| {
            let index = u32::try_from(index).expect("prepared rule IDs fit in u32");
            (EffectiveRuleId(index), rule)
        })
    }

    pub(crate) fn get(&self, id: EffectiveRuleId) -> EffectiveRule<'sheet, 'css> {
        self.rules[id.0 as usize]
    }

    pub(crate) fn scope_match(&self, scope: ScopeId, document: &Document, subject: DomNodeId) -> Option<ScopeMatch> {
        if scope == ScopeId::NONE {
            return Some(ScopeMatch { root: document.dom_root()?, proximity: u32::MAX });
        }
        let descriptor = &self.scopes[scope.0 as usize];
        let parent = self.scope_match(descriptor.parent, document, subject)?;
        let inside_parent = |candidate| candidate == parent.root || document.dom_ancestors(candidate).any(|ancestor| ancestor == parent.root);
        let match_for_root = |root| {
            let mut proximity = 0u32;
            for candidate in std::iter::once(subject).chain(document.dom_ancestors(subject)) {
                if descriptor.end.as_ref().is_some_and(|end| end.0.iter().any(|selector| crate::style::selectors_dom::selector_matches_dom_node_in_scope(selector, document, candidate, root))) {
                    return None;
                }
                if candidate == root {
                    return Some(ScopeMatch { root, proximity });
                }
                proximity = proximity.saturating_add(1);
            }
            None
        };
        if let Some(start) = descriptor.start {
            std::iter::once(subject)
                .chain(document.dom_ancestors(subject))
                .take_while(|candidate| inside_parent(*candidate))
                .filter(|candidate| start.0.iter().any(|selector| crate::style::selectors_dom::selector_matches_dom_node_in_scope(selector, document, *candidate, parent.root)))
                .find_map(match_for_root)
        } else {
            let root = descriptor.implicit_root?;
            inside_parent(root).then(|| match_for_root(root)).flatten()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CascadeOrigin, EffectiveRule, EffectiveRuleId, ParsedStylesheetSet};
    use crate::MediaEnvironment;
    use crate::allocation_test_support::count_allocations;
    use lightningcss::rules::CssRule;
    use lightningcss::stylesheet::{ParserOptions, StyleSheet};

    #[test]
    fn preparation_keeps_ast_nodes_borrowed_and_filters_invalid_selectors() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse("p { color: red } p, :unknown { color: blue }", ParserOptions { error_recovery: true, ..ParserOptions::default() }).unwrap();
        let expected = match &author.rules.0[0] {
            CssRule::Style(rule) => rule as *const _,
            _ => panic!("expected a style rule"),
        };
        let authors = [author];
        let (prepared, allocations) = count_allocations(|| ParsedStylesheetSet::new(&user_agent, &authors).prepare(MediaEnvironment::default(), 16.0));

        assert_eq!(allocations, 1, "preparation should allocate only its dense metadata vector");
        assert_eq!(prepared.len(), 1);
        let (_, rule) = prepared.iter().next().unwrap();
        assert_eq!(rule.style_rule() as *const _, expected);
        assert_eq!(rule.priority().origin(), CascadeOrigin::Author);

        let (count, allocations) = count_allocations(|| prepared.iter().count());
        assert_eq!(count, 1);
        assert_eq!(allocations, 0, "iterating prepared rules must stay allocation-free");
    }

    #[test]
    fn hot_path_handles_stay_compact() {
        assert_eq!(std::mem::size_of::<EffectiveRuleId>(), 4);
        assert!(std::mem::size_of::<EffectiveRule<'_, '_>>() <= 24);
    }

    #[test]
    fn preparation_flattens_active_rules_and_assigns_layer_ranks() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author =
            StyleSheet::parse("@layer early, late; @layer late { p { color: red } } @supports (width: 1px) { @layer early { p { color: green } } } @media print { p { color: blue } } p { color: black }", ParserOptions::default()).unwrap();
        let authors = [author];
        let prepared = ParsedStylesheetSet::new(&user_agent, &authors).prepare(MediaEnvironment::default(), 16.0);
        let rules = prepared.iter().map(|(_, rule)| rule.priority()).collect::<Vec<_>>();

        assert_eq!(rules.len(), 3);
        assert!(rules[1].layer() < rules[0].layer(), "the declared early layer must rank below the late layer");
        assert!(rules[2].layer() > rules[0].layer(), "unlayered rules must rank above every normal layer");
        assert_eq!(rules.iter().map(|priority| priority.source_order()).collect::<Vec<_>>(), vec![0, 1, 2]);
    }

    #[test]
    fn supports_declarations_allow_important_priority() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse("@supports (color: green !important) { html { background-color: green } }", ParserOptions::default()).unwrap();
        let authors = [author];
        let prepared = ParsedStylesheetSet::new(&user_agent, &authors).prepare(MediaEnvironment::default(), 16.0);

        assert_eq!(prepared.len(), 1);
    }

    #[test]
    fn supports_declarations_reject_invalid_custom_property_priority() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse("@supports (--theme: green !bogus) { html { color: red } } @supports (--theme: fn(!bogus)) { html { color: green } }", ParserOptions::default()).unwrap();
        let authors = [author];
        let prepared = ParsedStylesheetSet::new(&user_agent, &authors).prepare(MediaEnvironment::default(), 16.0);

        assert_eq!(prepared.len(), 1);
    }

    #[test]
    fn supports_selector_uses_the_unforgiving_single_selector_grammar() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author =
            StyleSheet::parse("@supports selector(div) { html { color: green } } @supports selector(div, div) { html { color: red } } @supports selector(:is(.ok, :unknown)) { html { background: red } }", ParserOptions::default()).unwrap();
        let authors = [author];
        let prepared = ParsedStylesheetSet::new(&user_agent, &authors).prepare(MediaEnvironment::default(), 16.0);

        assert_eq!(prepared.len(), 1);
    }
}
