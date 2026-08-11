//! The boundary between parser output and selector/cascade input.
//!
//! A `ParsedStylesheetSet` is only a borrowed view of Lightning CSS parser
//! output. Calling `prepare` validates which style rules are effective and
//! assigns compact handles plus cascade metadata. Downstream stages cannot
//! accidentally index arbitrary parser nodes because they accept only a
//! `PreparedRuleSet` and its private `EffectiveRuleId` values.

use super::media::{CompiledMediaList, MediaEnvironment, MediaQuerySet};
use crate::style::matching::selectors::{selector_list_is_web_valid, selector_list_pseudo_mask};
use crate::{PropertyCapability, PropertySyntax, declaration_support, supports_selector_syntax_is_valid};
use html_dom::{Document, DomNodeId};
use lightningcss::properties::{Property, PropertyId};
use lightningcss::rules::layer::LayerName;
use lightningcss::rules::style::StyleRule;
use lightningcss::rules::supports::SupportsCondition;
use lightningcss::rules::{CssRule, CssRuleList};
use lightningcss::selector::SelectorList;
use lightningcss::stylesheet::StyleSheet;
use rustc_data_structures::fx::FxHashMap;
use std::cmp::Ordering;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub(crate) enum CascadeOrigin {
    UserAgent = 0,
    Author = 1,
}

/// Integer layer order. Unlayered rules currently share the sentinel rank;
/// layer expansion can assign concrete ranks without changing selector or
/// resolver APIs.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RulePriority {
    origin: CascadeOrigin,
    layer: LayerOrder,
    source_order: u32,
}

impl RulePriority {
    #[cfg(test)]
    pub(crate) fn source_order(self) -> u32 {
        self.source_order
    }

    pub(crate) fn origin(self) -> CascadeOrigin {
        self.origin
    }

    #[cfg(test)]
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

#[derive(Clone)]
pub(crate) struct PreparedPropertyTarget {
    pub(crate) name: Rc<str>,
    pub(crate) slot: u32,
    pub(crate) longhand: Option<PropertyId<'static>>,
}

#[derive(Clone, Copy)]
struct PreparedTargetRange {
    start: u32,
    len_and_flags: u32,
}

#[derive(Clone, Copy)]
struct PreparedRuleDeclarations {
    normal_start: u32,
    important_start: u32,
    normal_shadow_mask: u64,
    important_shadow_mask: u64,
}

const ALL_DECLARATIONS_SHADOWABLE: u64 = 1 << 63;

fn shadow_mask(declarations: &[Property<'_>]) -> u64 {
    // One flag plus 63 declaration bits keeps the hot rule metadata inline.
    // Larger rules simply retain the normal event path.
    let mut mask = 0;
    let mut all_shadowable = declarations.len() < 63;
    for (index, property) in declarations.iter().enumerate() {
        let shadowable = property_is_always_computable(property);
        all_shadowable &= shadowable;
        if shadowable && index < 63 {
            mask |= 1 << index;
        }
    }
    mask | if all_shadowable { ALL_DECLARATIONS_SHADOWABLE } else { 0 }
}

impl PreparedTargetRange {
    const ALWAYS_COMPUTABLE: u32 = 1 << 31;
    const RENDERER_ELIGIBLE: u32 = 1 << 30;
    const ELIGIBILITY_KNOWN: u32 = 1 << 29;
    const FLAGS: u32 = Self::ALWAYS_COMPUTABLE | Self::RENDERER_ELIGIBLE | Self::ELIGIBILITY_KNOWN;

    fn new(start: u32, len: u32, property: &Property<'_>) -> Self {
        assert!(len < Self::ELIGIBILITY_KNOWN, "one declaration's property target count fits in 29 bits");
        let always_computable = property_is_always_computable(property);
        let renderer_eligibility = crate::style::syntax::capabilities::declaration_renderer_eligibility(property);
        Self { start, len_and_flags: len | if always_computable { Self::ALWAYS_COMPUTABLE } else { 0 } | if renderer_eligibility == Some(true) { Self::RENDERER_ELIGIBLE } else { 0 } | if renderer_eligibility.is_some() { Self::ELIGIBILITY_KNOWN } else { 0 } }
    }

    fn len(self) -> usize {
        (self.len_and_flags & !Self::FLAGS) as usize
    }

    fn is_always_computable(self) -> bool {
        self.len_and_flags & Self::ALWAYS_COMPUTABLE != 0
    }

    fn renderer_eligibility(self) -> Option<bool> {
        (self.len_and_flags & Self::ELIGIBILITY_KNOWN != 0).then_some(self.len_and_flags & Self::RENDERER_ELIGIBLE != 0)
    }
}

/// Properties whose renderer conversion cannot reject a parsed value.
/// False keeps the normal scratch validation, so this stays conservative.
fn property_is_always_computable(property: &Property<'_>) -> bool {
    matches!(property,
        Property::Color(_) | Property::BackgroundColor(_) | Property::Background(_) | Property::BackgroundImage(_)
        | Property::FontStyle(_) | Property::FontFamily(_)
        | Property::TextAlign(_) | Property::TextAlignLast(_, _) | Property::FontVariantCaps(_)
        | Property::ListStylePosition(_) | Property::ListStyleImage(_)
        | Property::TextDecorationLine(_, _) | Property::TextDecorationColor(_, _) | Property::OutlineColor(_)
        | Property::Overflow(_) | Property::OverflowX(_) | Property::OverflowY(_) | Property::TextOverflow(_, _) | Property::WhiteSpace(_)
        | Property::Hyphens(_, _) | Property::WordBreak(_) | Property::OverflowWrap(_) | Property::WordWrap(_) | Property::BoxSizing(_, _)
        | Property::ZIndex(_)
        | Property::BorderColor(_) | Property::BorderTopColor(_) | Property::BorderRightColor(_) | Property::BorderBottomColor(_) | Property::BorderLeftColor(_)
        | Property::BorderStyle(_) | Property::BorderTopStyle(_) | Property::BorderRightStyle(_) | Property::BorderBottomStyle(_) | Property::BorderLeftStyle(_)
        | Property::FlexDirection(_, _) | Property::FlexWrap(_, _) | Property::FlexFlow(_, _) | Property::Order(_, _) | Property::JustifyContent(_, _)
        | Property::GridAutoFlow(_)
    )
}

fn canonical_slot(name: &str) -> &str {
    match name {
        "word-wrap" => "overflow-wrap",
        "page-break-before" => "break-before",
        "page-break-after" => "break-after",
        "page-break-inside" => "break-inside",
        "grid-row-gap" => "row-gap",
        "grid-column-gap" => "column-gap",
        _ => name,
    }
}

fn intern_target(name: &str, longhand: Option<PropertyId<'static>>, slot_ids: &mut FxHashMap<Rc<str>, u32>) -> PreparedPropertyTarget {
    if let Some((name, slot)) = slot_ids.get_key_value(name) {
        return PreparedPropertyTarget { name: name.clone(), slot: *slot, longhand };
    }
    let name: Rc<str> = Rc::from(name);
    let slot = u32::try_from(slot_ids.len()).expect("CSS property slot count fits in u32");
    slot_ids.insert(name.clone(), slot);
    PreparedPropertyTarget { name, slot, longhand }
}

fn append_property_targets(property: &Property<'_>, slot_ids: &mut FxHashMap<Rc<str>, u32>, output: &mut Vec<PreparedPropertyTarget>) {
    if matches!(property, Property::Custom(value) if value.name.as_ref().starts_with("--")) || matches!(property, Property::Unparsed(value) if value.property_id.name().starts_with("--")) {
        return;
    }
    if matches!(property, Property::All(_)) {
        output.push(intern_target("all", None, slot_ids));
        return;
    }
    let property_id = property.property_id();
    if let Some(longhands) = property_id.longhands() {
        output.extend(longhands.into_iter().filter(|longhand| !crate::style::syntax::capabilities::property_uses_gradient(property) || longhand.name() == "background-image").map(|longhand| {
            let mut target = intern_target(canonical_slot(longhand.name()), None, slot_ids);
            target.longhand = Some(longhand);
            target
        }));
        return;
    }
    output.push(intern_target(canonical_slot(property_id.name()), None, slot_ids));
}

pub(crate) fn compile_property_targets(property: &Property<'_>, slot_ids: &mut FxHashMap<Rc<str>, u32>, targets: &mut Vec<PreparedPropertyTarget>) {
    targets.clear();
    append_property_targets(property, slot_ids, targets);
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
    #[cfg(test)]
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

        let rule_pseudo_masks = rules
            .iter()
            .map(|rule| selector_list_pseudo_mask(&rule.style_rule.selectors))
            .collect();

        let declaration_capacity = rules.iter().map(|rule| rule.style_rule.declarations.declarations.len() + rule.style_rule.declarations.important_declarations.len()).sum();
        // Compile Lightning CSS declaration expansion once. Per-element
        // cascade work then follows dense ranges instead of rebuilding and
        // caching one target vector for every encountered AST declaration.
        let mut declaration_target_ranges = Vec::with_capacity(declaration_capacity);
        let mut property_targets = Vec::with_capacity(declaration_capacity);
        let mut property_slots = FxHashMap::default();
        let mut rule_target_starts = Vec::with_capacity(rules.len());
        for rule in &rules {
            let normal_start = u32::try_from(declaration_target_ranges.len()).expect("prepared declaration target ranges fit in u32");
            for property in &rule.style_rule.declarations.declarations {
                let start = u32::try_from(property_targets.len()).expect("prepared property targets fit in u32");
                append_property_targets(property, &mut property_slots, &mut property_targets);
                let len = u32::try_from(property_targets.len() - start as usize).expect("one declaration's property targets fit in u32");
                declaration_target_ranges.push(PreparedTargetRange::new(start, len, property));
            }
            let important_start = u32::try_from(declaration_target_ranges.len()).expect("prepared declaration target ranges fit in u32");
            for property in &rule.style_rule.declarations.important_declarations {
                let start = u32::try_from(property_targets.len()).expect("prepared property targets fit in u32");
                append_property_targets(property, &mut property_slots, &mut property_targets);
                let len = u32::try_from(property_targets.len() - start as usize).expect("one declaration's property targets fit in u32");
                declaration_target_ranges.push(PreparedTargetRange::new(start, len, property));
            }
            rule_target_starts.push(PreparedRuleDeclarations {
                normal_start,
                important_start,
                normal_shadow_mask: shadow_mask(&rule.style_rule.declarations.declarations),
                important_shadow_mask: shadow_mask(&rule.style_rule.declarations.important_declarations),
            });
        }

        PreparedRuleSet {
            rules,
            rule_pseudo_masks,
            scopes,
            rule_target_starts,
            declaration_target_ranges,
            property_targets,
            property_slots,
            media_queries,
            environment,
        }
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
    rule_pseudo_masks: Vec<u8>,
    scopes: Vec<ScopeDescriptor<'sheet, 'css>>,
    rule_target_starts: Vec<PreparedRuleDeclarations>,
    declaration_target_ranges: Vec<PreparedTargetRange>,
    property_targets: Vec<PreparedPropertyTarget>,
    property_slots: FxHashMap<Rc<str>, u32>,
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

    pub(crate) fn pseudo_mask(&self, id: EffectiveRuleId) -> u8 {
        self.rule_pseudo_masks[id.0 as usize]
    }

    pub(crate) fn declaration_targets(&self, id: EffectiveRuleId, important: bool, index: usize) -> &[PreparedPropertyTarget] {
        self.declaration_metadata(id, important, index).0
    }

    pub(crate) fn declaration_metadata(&self, id: EffectiveRuleId, important: bool, index: usize) -> (&[PreparedPropertyTarget], bool, Option<bool>) {
        let starts = self.rule_target_starts[id.0 as usize];
        let ranges_start = if important { starts.important_start } else { starts.normal_start } as usize;
        let range = self.declaration_target_ranges[ranges_start + index];
        (&self.property_targets[range.start as usize..range.start as usize + range.len()], range.is_always_computable(), range.renderer_eligibility())
    }

    #[cfg(test)]
    pub(crate) fn declaration_is_always_computable(&self, id: EffectiveRuleId, important: bool, index: usize) -> bool {
        let starts = self.rule_target_starts[id.0 as usize];
        let ranges_start = if important { starts.important_start } else { starts.normal_start } as usize;
        self.declaration_target_ranges[ranges_start + index].is_always_computable()
    }

    pub(crate) fn rule_shadow_declarations(&self, id: EffectiveRuleId, important: bool) -> (u64, bool) {
        let starts = self.rule_target_starts[id.0 as usize];
        let mask = if important { starts.important_shadow_mask } else { starts.normal_shadow_mask };
        (mask & !ALL_DECLARATIONS_SHADOWABLE, mask & ALL_DECLARATIONS_SHADOWABLE != 0)
    }

    pub(crate) fn clone_property_slots(&self) -> FxHashMap<Rc<str>, u32> {
        self.property_slots.clone()
    }

    pub(crate) fn property_slot_count(&self) -> usize {
        self.property_slots.len()
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
                if descriptor.end.as_ref().is_some_and(|end| end.0.iter().any(|selector| crate::style::matching::dom::selector_matches_dom_node_in_scope(selector, document, candidate, root))) {
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
                .filter(|candidate| start.0.iter().any(|selector| crate::style::matching::dom::selector_matches_dom_node_in_scope(selector, document, *candidate, parent.root)))
                .find_map(match_for_root)
        } else {
            let root = descriptor.implicit_root?;
            inside_parent(root).then(|| match_for_root(root)).flatten()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CascadeOrigin, EffectiveRule, EffectiveRuleId, ParsedStylesheetSet, PreparedTargetRange};
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

        assert_eq!(allocations, 7, "preparation should allocate only its dense rule and declaration metadata");
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
        assert_eq!(std::mem::size_of::<PreparedTargetRange>(), 8);
    }

    #[test]
    fn preparation_flattens_declaration_targets_for_all_priorities() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse("p { margin: 1px; color: red !important }", ParserOptions::default()).unwrap();
        let authors = [author];
        let prepared = ParsedStylesheetSet::new(&user_agent, &authors).prepare(MediaEnvironment::default(), 16.0);
        let (id, _) = prepared.iter().next().unwrap();

        let (normal_names, allocations) = count_allocations(|| prepared.declaration_targets(id, false, 0).iter().map(|target| target.name.as_ref()).collect::<Vec<_>>());
        assert_eq!(normal_names.len(), 4);
        assert!(normal_names.contains(&"margin-left"));
        assert_eq!(allocations, 1, "only the test's collected result should allocate");
        assert!(!prepared.declaration_is_always_computable(id, false, 0));

        let important = prepared.declaration_targets(id, true, 0);
        assert_eq!(important.len(), 1);
        assert_eq!(important[0].name.as_ref(), "color");
        assert!(prepared.declaration_is_always_computable(id, true, 0));
    }

    #[test]
    fn preparation_flattens_active_rules_and_assigns_layer_ranks() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse("@layer early, late; @layer late { p { color: red } } @supports (width: 1px) { @layer early { p { color: green } } } @media print { p { color: blue } } p { color: black }", ParserOptions::default()).unwrap();
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
        let author = StyleSheet::parse("@supports selector(div) { html { color: green } } @supports selector(div, div) { html { color: red } } @supports selector(:is(.ok, :unknown)) { html { background: red } }", ParserOptions::default()).unwrap();
        let authors = [author];
        let prepared = ParsedStylesheetSet::new(&user_agent, &authors).prepare(MediaEnvironment::default(), 16.0);

        assert_eq!(prepared.len(), 1);
    }
}
