//! Compile parser output into document-independent effective rules.

use crate::style::compiled::{
    ALL_DECLARATIONS_SHADOWABLE, CascadeOrigin, LayerOrder, PreparedRuleDeclarations,
    PreparedRuleMetadata, PreparedRuleSet, PreparedTargetRange, RulePriority, ScopeDescriptor,
    ScopeId,
};
use crate::style::environment::{MediaEnvironment, MediaQuerySet, evaluate_media_list};
use crate::style::property_catalogue::PropertyKey;
use crate::style::source::fonts::FontFace;
use crate::style::syntax::selectors::{
    selector_list_is_web_valid, selector_list_is_web_valid_for_scope, selector_list_pseudo_mask,
};
use crate::style::syntax::targets::append_property_targets;
use crate::style::syntax::validation::{
    supports_declaration_is_supported, supports_selector_syntax_is_valid,
};
use lightningcss::properties::Property;
use lightningcss::rules::layer::LayerName;
use lightningcss::rules::style::StyleRule;
use lightningcss::rules::supports::SupportsCondition;
use lightningcss::rules::{CssRule, CssRuleList};
use lightningcss::selector::SelectorList;
use lightningcss::stylesheet::StyleSheet;
use rustc_data_structures::fx::FxHashMap;
use std::sync::Arc;

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

struct PendingRule<'fragment, 'css> {
    style_rule: Arc<StyleRule<'css>>,
    metadata: PreparedRuleMetadata,
    fragment_rule: (&'fragment PreparedStylesheetFragment<'css>, usize),
}

/// Environment- and order-independent work compiled once for one stylesheet.
/// Linking still evaluates media, layers, scopes, and source order globally.
pub(crate) struct PreparedStylesheetFragment<'css> {
    stylesheet: StyleSheet<'css>,
    style_rules: Vec<Arc<StyleRule<'css>>>,
    rule_declarations: Vec<PreparedRuleDeclarations>,
    declaration_target_ranges: Vec<PreparedTargetRange>,
    property_targets: Vec<PropertyKey>,
}

impl<'css> PreparedStylesheetFragment<'css> {
    pub(crate) fn compile(mut stylesheet: StyleSheet<'css>) -> Self {
        let mut style_rules = Vec::with_capacity(stylesheet.rules.0.len());
        take_all_style_rules(&mut stylesheet.rules, &mut style_rules);
        let declaration_capacity = style_rules
            .iter()
            .map(|rule| {
                rule.declarations.declarations.len()
                    + rule.declarations.important_declarations.len()
            })
            .sum();
        let mut declaration_target_ranges = Vec::with_capacity(declaration_capacity);
        let mut property_targets = Vec::with_capacity(declaration_capacity);
        let mut rule_declarations = Vec::with_capacity(style_rules.len());
        for rule in &style_rules {
            let (normal_start, normal_shadow_mask) = append_fragment_declarations(
                &rule.declarations.declarations,
                &mut property_targets,
                &mut declaration_target_ranges,
            );
            let (important_start, important_shadow_mask) = append_fragment_declarations(
                &rule.declarations.important_declarations,
                &mut property_targets,
                &mut declaration_target_ranges,
            );
            rule_declarations.push(PreparedRuleDeclarations {
                normal_start,
                important_start,
                normal_shadow_mask,
                important_shadow_mask,
            });
        }
        Self {
            stylesheet,
            style_rules,
            rule_declarations,
            declaration_target_ranges,
            property_targets,
        }
    }
}

/// Properties whose renderer conversion cannot reject a parsed value.
/// The flag is used for prepared shadow masks and stays conservative.
fn property_is_always_computable(property: &Property<'_>) -> bool {
    matches!(
        property,
        Property::Color(_)
            | Property::BackgroundColor(_)
            | Property::Background(_)
            | Property::BackgroundImage(_)
            | Property::FontStyle(_)
            | Property::FontFamily(_)
            | Property::TextAlign(_)
            | Property::UnicodeBidi(_)
            | Property::TextAlignLast(_, _)
            | Property::FontVariantCaps(_)
            | Property::ListStylePosition(_)
            | Property::ListStyleImage(_)
            | Property::TextDecorationLine(_, _)
            | Property::TextDecorationColor(_, _)
            | Property::OutlineColor(_)
            | Property::Overflow(_)
            | Property::OverflowX(_)
            | Property::OverflowY(_)
            | Property::TextOverflow(_, _)
            | Property::WhiteSpace(_)
            | Property::Hyphens(_, _)
            | Property::WordBreak(_)
            | Property::OverflowWrap(_)
            | Property::WordWrap(_)
            | Property::BoxSizing(_, _)
            | Property::ZIndex(_)
            | Property::BorderColor(_)
            | Property::BorderTopColor(_)
            | Property::BorderRightColor(_)
            | Property::BorderBottomColor(_)
            | Property::BorderLeftColor(_)
            | Property::BorderStyle(_)
            | Property::BorderTopStyle(_)
            | Property::BorderRightStyle(_)
            | Property::BorderBottomStyle(_)
            | Property::BorderLeftStyle(_)
            | Property::FlexDirection(_, _)
            | Property::FlexWrap(_, _)
            | Property::FlexFlow(_, _)
            | Property::Order(_, _)
            | Property::JustifyContent(_, _)
            | Property::GridAutoFlow(_)
    )
}

fn append_fragment_declarations(
    declarations: &[Property<'_>],
    targets: &mut Vec<PropertyKey>,
    ranges: &mut Vec<PreparedTargetRange>,
) -> (u32, u64) {
    let first = u32::try_from(ranges.len()).expect("prepared declaration target ranges fit in u32");
    // One flag plus 63 declaration bits keeps the hot rule metadata inline.
    // Larger rules retain per-declaration bits without whole-rule pruning.
    let mut shadow_mask = if declarations.len() < 63 {
        ALL_DECLARATIONS_SHADOWABLE
    } else {
        0
    };
    for (index, property) in declarations.iter().enumerate() {
        let start = u32::try_from(targets.len()).expect("prepared property targets fit in u32");
        append_property_targets(property, targets);
        let len = u32::try_from(targets.len() - start as usize)
            .expect("one declaration's property targets fit in u32");
        let always_computable = property_is_always_computable(property);
        ranges.push(PreparedTargetRange::new(
            start,
            len,
            always_computable,
            crate::style::syntax::capabilities::declaration_renderer_eligibility(property),
        ));
        if always_computable {
            if index < 63 {
                shadow_mask |= 1 << index;
            }
        } else {
            shadow_mask &= !ALL_DECLARATIONS_SHADOWABLE;
        }
    }
    (first, shadow_mask)
}

fn link_fragment_declaration(
    fragment: &PreparedStylesheetFragment<'_>,
    range_index: usize,
    targets: &mut Vec<PropertyKey>,
    ranges: &mut Vec<PreparedTargetRange>,
) {
    let local_range = fragment.declaration_target_ranges[range_index];
    let start = u32::try_from(targets.len()).expect("prepared property targets fit in u32");
    targets.extend_from_slice(
        &fragment.property_targets
            [local_range.start as usize..local_range.start as usize + local_range.len()],
    );
    ranges.push(PreparedTargetRange {
        start,
        len_and_flags: local_range.len_and_flags,
    });
}

fn take_all_style_rules<'css>(
    rules: &mut CssRuleList<'css>,
    output: &mut Vec<Arc<StyleRule<'css>>>,
) {
    for rule in &mut rules.0 {
        match rule {
            CssRule::Style(style) => {
                let placeholder = StyleRule {
                    selectors: SelectorList::new(Default::default()),
                    vendor_prefix: style.vendor_prefix,
                    declarations: Default::default(),
                    rules: CssRuleList(Vec::new()),
                    loc: style.loc,
                };
                output.push(Arc::new(std::mem::replace(style, placeholder)));
            }
            CssRule::Media(media) => take_all_style_rules(&mut media.rules, output),
            CssRule::Supports(supports) => take_all_style_rules(&mut supports.rules, output),
            CssRule::LayerBlock(layer) => take_all_style_rules(&mut layer.rules, output),
            CssRule::Scope(scope) => take_all_style_rules(&mut scope.rules, output),
            _ => {}
        }
    }
}

/// Link precompiled fragments for one environment and global cascade order.
pub(crate) fn prepare<'css>(
    user_agent: &PreparedStylesheetFragment<'css>,
    authors: &[(Arc<PreparedStylesheetFragment<'css>>, usize)],
    environment: MediaEnvironment,
    initial_font_size: f64,
) -> PreparedRuleSet<'css> {
    let capacity = user_agent.style_rules.len()
        + authors
            .iter()
            .map(|(fragment, _)| fragment.style_rules.len())
            .sum::<usize>();
    let mut rules = Vec::with_capacity(capacity);
    let mut source_order = 0u32;
    let mut media_queries = MediaQuerySet::default();
    let mut media_path = Vec::new();
    let mut scopes = Vec::new();
    let mut font_faces = Vec::new();

    prepare_origin(
        std::iter::once((user_agent, None)),
        CascadeOrigin::UserAgent,
        &mut rules,
        &mut scopes,
        &mut font_faces,
        &mut source_order,
        environment,
        initial_font_size,
        &mut media_queries,
        &mut media_path,
    );
    let authors = authors.iter().map(|(fragment, index)| {
        (
            fragment.as_ref(),
            Some(u32::try_from(*index).expect("author stylesheet count fits in u32")),
        )
    });
    prepare_origin(
        authors,
        CascadeOrigin::Author,
        &mut rules,
        &mut scopes,
        &mut font_faces,
        &mut source_order,
        environment,
        initial_font_size,
        &mut media_queries,
        &mut media_path,
    );

    let rule_pseudo_masks = rules
        .iter()
        .map(|rule| selector_list_pseudo_mask(&rule.style_rule.selectors))
        .collect();

    let declaration_capacity = rules
        .iter()
        .map(|rule| {
            rule.style_rule.declarations.declarations.len()
                + rule.style_rule.declarations.important_declarations.len()
        })
        .sum();
    // Link precompiled declaration ranges into the active global rule order.
    // No parser values or shorthand targets are recomputed here.
    let mut declaration_target_ranges = Vec::with_capacity(declaration_capacity);
    let mut property_targets = Vec::with_capacity(declaration_capacity);
    let mut rule_target_starts = Vec::with_capacity(rules.len());
    for rule in &rules {
        let normal_start = u32::try_from(declaration_target_ranges.len())
            .expect("prepared declaration target ranges fit in u32");
        let (fragment, rule_index) = rule.fragment_rule;
        let masks = fragment.rule_declarations[rule_index];
        append_rule_declarations(
            fragment,
            masks.normal_start,
            rule.style_rule.declarations.declarations.len(),
            &mut property_targets,
            &mut declaration_target_ranges,
        );
        let important_start = u32::try_from(declaration_target_ranges.len())
            .expect("prepared declaration target ranges fit in u32");
        append_rule_declarations(
            fragment,
            masks.important_start,
            rule.style_rule.declarations.important_declarations.len(),
            &mut property_targets,
            &mut declaration_target_ranges,
        );
        rule_target_starts.push(PreparedRuleDeclarations {
            normal_start,
            important_start,
            normal_shadow_mask: masks.normal_shadow_mask,
            important_shadow_mask: masks.important_shadow_mask,
        });
    }

    let (style_rules, rules) = rules
        .into_iter()
        .map(|rule| (rule.style_rule, rule.metadata))
        .unzip();
    PreparedRuleSet {
        style_rules,
        rules,
        rule_pseudo_masks,
        scopes,
        rule_target_starts,
        declaration_target_ranges,
        property_targets,
        media_queries,
        font_faces: font_faces.into(),
        environment,
    }
}

fn append_rule_declarations(
    fragment: &PreparedStylesheetFragment<'_>,
    start: u32,
    count: usize,
    targets: &mut Vec<PropertyKey>,
    ranges: &mut Vec<PreparedTargetRange>,
) {
    for index in start as usize..start as usize + count {
        link_fragment_declaration(fragment, index, targets, ranges);
    }
}

#[derive(Clone, Copy)]
enum RuleWalkMode {
    Active,
    Dependencies,
    Skip,
}

struct FragmentCursor<'fragment, 'css> {
    fragment: &'fragment PreparedStylesheetFragment<'css>,
    next_style_rule: usize,
}

struct RuleWalk<'a, 'fragment, 'css> {
    origin: CascadeOrigin,
    layers: LayerRegistry,
    output: &'a mut Vec<PendingRule<'fragment, 'css>>,
    scopes: &'a mut Vec<ScopeDescriptor<'css>>,
    font_faces: &'a mut Vec<FontFace>,
    source_order: &'a mut u32,
    environment: MediaEnvironment,
    initial_font_size: f64,
    media_queries: &'a mut MediaQuerySet,
    media_path: &'a mut Vec<u32>,
}

impl<'fragment, 'css> RuleWalk<'_, 'fragment, 'css> {
    fn visit(
        &mut self,
        rules: &CssRuleList<'css>,
        cursor: &mut FragmentCursor<'fragment, 'css>,
        current_layer: Option<LayerId>,
        current_scope: ScopeId,
        implicit_root_index: Option<u32>,
        mode: RuleWalkMode,
    ) {
        for rule in &rules.0 {
            match rule {
                CssRule::Style(_) => {
                    let fragment_rule_index = cursor.next_style_rule;
                    cursor.next_style_rule += 1;
                    if matches!(mode, RuleWalkMode::Skip) {
                        continue;
                    }
                    let effective_rule = &cursor.fragment.style_rules[fragment_rule_index];
                    if !selector_list_is_web_valid(&effective_rule.selectors) {
                        continue;
                    }
                    match mode {
                        RuleWalkMode::Active => {
                            self.media_queries.record_dependency(self.media_path);
                            let layer =
                                current_layer.map_or(LayerOrder::UNLAYERED, |id| LayerOrder(id.0));
                            self.output.push(PendingRule {
                                style_rule: effective_rule.clone(),
                                metadata: PreparedRuleMetadata {
                                    priority: RulePriority {
                                        origin: self.origin,
                                        layer,
                                        source_order: *self.source_order,
                                    },
                                    scope: current_scope,
                                },
                                fragment_rule: (cursor.fragment, fragment_rule_index),
                            });
                            *self.source_order = self.source_order.checked_add(1).expect(
                                "a stylesheet cannot contain more than u32::MAX effective rules",
                            );
                        }
                        RuleWalkMode::Dependencies => {
                            self.media_queries.record_dependency(self.media_path)
                        }
                        RuleWalkMode::Skip => {}
                    }
                }
                CssRule::FontFace(rule) => {
                    if !matches!(mode, RuleWalkMode::Skip)
                        && let Some(face) = FontFace::from_rule(rule)
                    {
                        // A sheet containing only font faces still changes
                        // shaping when a media condition changes truth value.
                        self.media_queries.record_dependency(self.media_path);
                        if matches!(mode, RuleWalkMode::Active) {
                            self.font_faces.push(face);
                        }
                    }
                }
                CssRule::Media(media) => {
                    if matches!(mode, RuleWalkMode::Skip) {
                        self.visit(
                            &media.rules,
                            cursor,
                            current_layer,
                            current_scope,
                            implicit_root_index,
                            mode,
                        );
                        continue;
                    }
                    let applies = matches!(mode, RuleWalkMode::Active)
                        && evaluate_media_list(
                            &media.query,
                            self.environment,
                            self.initial_font_size,
                        )
                        .is_true();
                    let query_id = self.media_queries.register_query(&media.query);
                    self.media_path.push(query_id);
                    let child_mode = if matches!(mode, RuleWalkMode::Active) && applies {
                        RuleWalkMode::Active
                    } else {
                        RuleWalkMode::Dependencies
                    };
                    self.visit(
                        &media.rules,
                        cursor,
                        current_layer,
                        current_scope,
                        implicit_root_index,
                        child_mode,
                    );
                    self.media_path.pop();
                }
                CssRule::Supports(supports) => {
                    let child_mode = if matches!(mode, RuleWalkMode::Skip)
                        || !supports_condition_applies(&supports.condition)
                    {
                        RuleWalkMode::Skip
                    } else {
                        mode
                    };
                    self.visit(
                        &supports.rules,
                        cursor,
                        current_layer,
                        current_scope,
                        implicit_root_index,
                        child_mode,
                    );
                }
                CssRule::LayerStatement(statement) => match mode {
                    RuleWalkMode::Active => {
                        self.media_queries.record_dependency(self.media_path);
                        for name in &statement.names {
                            self.layers.register_named(current_layer, name);
                        }
                    }
                    RuleWalkMode::Dependencies => {
                        self.media_queries.record_dependency(self.media_path)
                    }
                    RuleWalkMode::Skip => {}
                },
                CssRule::LayerBlock(block) => {
                    let layer = if matches!(mode, RuleWalkMode::Active) {
                        self.media_queries.record_dependency(self.media_path);
                        Some(match &block.name {
                            Some(name) => self.layers.register_named(current_layer, name),
                            None => self.layers.register_anonymous(current_layer),
                        })
                    } else {
                        if matches!(mode, RuleWalkMode::Dependencies) {
                            self.media_queries.record_dependency(self.media_path);
                        }
                        current_layer
                    };
                    self.visit(
                        &block.rules,
                        cursor,
                        layer,
                        current_scope,
                        implicit_root_index,
                        mode,
                    );
                }
                CssRule::Scope(scope) => {
                    let mut child_scope = current_scope;
                    let mut child_mode = mode;
                    if matches!(mode, RuleWalkMode::Active) {
                        let start_valid = scope
                            .scope_start
                            .as_ref()
                            .is_none_or(selector_list_is_web_valid_for_scope);
                        let end_valid = scope
                            .scope_end
                            .as_ref()
                            .is_none_or(selector_list_is_web_valid_for_scope);
                        if start_valid && end_valid {
                            child_scope = ScopeId(
                                u32::try_from(self.scopes.len()).expect("scope IDs fit in u32"),
                            );
                            self.scopes.push(ScopeDescriptor {
                                parent: current_scope,
                                start: scope.scope_start.clone(),
                                end: scope.scope_end.clone(),
                                implicit_root_index,
                            });
                        } else {
                            child_mode = RuleWalkMode::Skip;
                        }
                    }
                    self.visit(
                        &scope.rules,
                        cursor,
                        current_layer,
                        child_scope,
                        implicit_root_index,
                        child_mode,
                    );
                }
                _ => {}
            }
        }
    }
}

fn prepare_origin<'sheet, 'css: 'sheet>(
    stylesheets: impl IntoIterator<Item = (&'sheet PreparedStylesheetFragment<'css>, Option<u32>)>,
    origin: CascadeOrigin,
    output: &mut Vec<PendingRule<'sheet, 'css>>,
    scopes: &mut Vec<ScopeDescriptor<'css>>,
    font_faces: &mut Vec<FontFace>,
    source_order: &mut u32,
    environment: MediaEnvironment,
    initial_font_size: f64,
    media_queries: &mut MediaQuerySet,
    media_path: &mut Vec<u32>,
) {
    let first_rule = output.len();
    let mut walk = RuleWalk {
        origin,
        layers: LayerRegistry::default(),
        output,
        scopes,
        font_faces,
        source_order,
        environment,
        initial_font_size,
        media_queries,
        media_path,
    };
    for (fragment, implicit_root_index) in stylesheets {
        let mut cursor = FragmentCursor {
            fragment,
            next_style_rule: 0,
        };
        walk.visit(
            &fragment.stylesheet.rules,
            &mut cursor,
            None,
            ScopeId::NONE,
            implicit_root_index,
            RuleWalkMode::Active,
        );
        debug_assert_eq!(cursor.next_style_rule, fragment.rule_declarations.len());
    }
    let ranks = walk.layers.ranks();
    for rule in &mut walk.output[first_rule..] {
        if !rule.metadata.priority.layer.is_unlayered() {
            rule.metadata.priority.layer = ranks[rule.metadata.priority.layer.0 as usize];
        }
    }
}

fn supports_condition_applies(condition: &SupportsCondition<'_>) -> bool {
    match condition {
        SupportsCondition::Not(condition) => !supports_condition_applies(condition),
        SupportsCondition::And(conditions) => conditions.iter().all(supports_condition_applies),
        SupportsCondition::Or(conditions) => conditions.iter().any(supports_condition_applies),
        SupportsCondition::Declaration { property_id, value } => {
            supports_declaration_is_supported(property_id, value)
        }
        SupportsCondition::Selector(selector) => supports_selector_syntax_is_valid(selector),
        SupportsCondition::Unknown(_) => false,
    }
}

#[cfg(test)]
pub(crate) fn prepare_stylesheets<'css>(
    user_agent: &StyleSheet<'css>,
    authors: &[StyleSheet<'css>],
    environment: MediaEnvironment,
    initial_font_size: f64,
) -> PreparedRuleSet<'css> {
    let user_agent = PreparedStylesheetFragment::compile(user_agent.clone());
    let authors = authors
        .iter()
        .enumerate()
        .map(|(index, sheet)| {
            (
                Arc::new(PreparedStylesheetFragment::compile(sheet.clone())),
                index,
            )
        })
        .collect::<Vec<_>>();
    prepare(&user_agent, &authors, environment, initial_font_size)
}

#[cfg(test)]
mod tests {
    use super::{
        ALL_DECLARATIONS_SHADOWABLE, CascadeOrigin, PreparedStylesheetFragment,
        PreparedTargetRange, prepare, prepare_stylesheets,
    };
    use crate::MediaEnvironment;
    use crate::allocation_test_support::count_allocations;
    use crate::style::compiled::{EffectiveRule, EffectiveRuleId, ScopeId};
    use crate::style::property_catalogue::PropertyKey;
    use lightningcss::stylesheet::{ParserOptions, StyleSheet};
    use lightningcss::traits::ToCss;
    use std::sync::Arc;

    #[test]
    fn preparation_shares_fragment_rules_and_filters_invalid_selectors() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse(
            "p { color: red } p > { color: blue }",
            ParserOptions {
                error_recovery: true,
                ..ParserOptions::default()
            },
        )
        .unwrap();
        let user_agent = PreparedStylesheetFragment::compile(user_agent);
        let authors = [(Arc::new(PreparedStylesheetFragment::compile(author)), 0)];
        let expected = Arc::as_ptr(&authors[0].0.style_rules[0]);
        let (prepared, allocations) =
            count_allocations(|| prepare(&user_agent, &authors, MediaEnvironment::default(), 16.0));
        assert_eq!(
            allocations, 8,
            "linking should allocate only effective metadata and handles"
        );
        assert_eq!(prepared.len(), 1);
        let (_, rule) = prepared.iter().next().unwrap();
        assert_eq!(
            rule.style_rule() as *const _,
            expected,
            "linking reuses the fragment's immutable rule"
        );
        let repeated = prepare(&user_agent, &authors, MediaEnvironment::default(), 16.0);
        assert_eq!(
            repeated.iter().next().unwrap().1.style_rule() as *const _,
            expected
        );
        assert_eq!(rule.priority().origin(), CascadeOrigin::Author);

        let (count, allocations) = count_allocations(|| prepared.iter().count());
        assert_eq!(count, 1);
        assert_eq!(
            allocations, 0,
            "iterating prepared rules must stay allocation-free"
        );
    }

    #[test]
    fn hot_path_handles_stay_compact() {
        assert_eq!(std::mem::size_of::<EffectiveRuleId>(), 4);
        assert!(std::mem::size_of::<EffectiveRule<'_, '_>>() <= 24);
        assert_eq!(std::mem::size_of::<PreparedTargetRange>(), 8);
    }

    #[test]
    fn shadow_masks_preserve_the_declaration_limit_and_independent_priorities() {
        for count in [0usize, 1, 62, 63, 64, 65] {
            for mixed in [false, true] {
                let mut css = format!("p {{ {}", "color:red;".repeat(count));
                if mixed {
                    css.push_str("width:1px;");
                }
                css.push_str(&"color:green!important;".repeat(count + 1));
                css.push('}');
                let stylesheet = StyleSheet::parse(&css, ParserOptions::default()).unwrap();
                let fragment = PreparedStylesheetFragment::compile(stylesheet);
                let metadata = fragment.rule_declarations[0];
                let normal_count = count + usize::from(mixed);
                let bits = |count: usize| (1u64 << count.min(63)) - 1;
                let all = |enabled| {
                    if enabled {
                        ALL_DECLARATIONS_SHADOWABLE
                    } else {
                        0
                    }
                };
                assert_eq!(metadata.normal_start, 0);
                assert_eq!(metadata.important_start as usize, normal_count);
                assert_eq!(
                    metadata.normal_shadow_mask,
                    bits(count) | all(!mixed && count < 63)
                );
                assert_eq!(
                    metadata.important_shadow_mask,
                    bits(count + 1) | all(count + 1 < 63)
                );
                assert_eq!(
                    fragment.declaration_target_ranges.len(),
                    normal_count + count + 1
                );
                for (index, range) in fragment.declaration_target_ranges.iter().enumerate() {
                    assert_eq!(range.is_always_computable(), !mixed || index != count);
                }
            }
        }
    }

    #[test]
    fn preparation_flattens_declaration_targets_for_all_priorities() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse(
            "p { margin: 1px; color: red !important }",
            ParserOptions::default(),
        )
        .unwrap();
        let authors = [author];
        let prepared =
            prepare_stylesheets(&user_agent, &authors, MediaEnvironment::default(), 16.0);
        let (id, _) = prepared.iter().next().unwrap();

        let (normal_keys, allocations) =
            count_allocations(|| prepared.declaration_targets(id, false, 0).to_vec());
        assert_eq!(normal_keys.len(), 4);
        assert!(normal_keys.contains(&PropertyKey::MarginLeft));
        assert_eq!(
            allocations, 1,
            "only the test's collected result should allocate"
        );
        assert!(!prepared.declaration_is_always_computable(id, false, 0));

        let important = prepared.declaration_targets(id, true, 0);
        assert_eq!(important.len(), 1);
        assert_eq!(important[0], PropertyKey::Color);
        assert!(prepared.declaration_is_always_computable(id, true, 0));
    }

    #[test]
    fn preparation_flattens_active_rules_and_assigns_layer_ranks() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse("@layer early, late; @layer late { p { color: red } } @supports (width: 1px) { @layer early { p { color: green } } } @media print { p { color: blue } } p { color: black }", ParserOptions::default()).unwrap();
        let authors = [author];
        let prepared =
            prepare_stylesheets(&user_agent, &authors, MediaEnvironment::default(), 16.0);
        let rules = prepared
            .iter()
            .map(|(_, rule)| rule.priority())
            .collect::<Vec<_>>();

        assert_eq!(rules.len(), 3);
        assert!(
            rules[1].layer() < rules[0].layer(),
            "the declared early layer must rank below the late layer"
        );
        assert!(
            rules[2].layer() > rules[0].layer(),
            "unlayered rules must rank above every normal layer"
        );
        assert_eq!(
            rules
                .iter()
                .map(|priority| priority.source_order())
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn nested_rule_modes_preserve_rule_order_scopes_layers_and_media_dependencies() {
        let css = r"
            @supports (opacity:0.5) {
                @media (min-width:1px) {
                    @layer hidden { @scope (.hidden) { .skip { color:red } } }
                }
            }
            @scope (::before) {
                @media (min-width:2px) { .invalid { color:red } }
            }
            @media (min-width:900px) {
                @scope (.future) { @layer dormant { .future { color:blue } } }
            }
            @layer active {
                @scope (.outer) {
                    @media (min-width:500px) {
                        @scope (.inner) { .inside { padding-left:7px } }
                    }
                    .outer { color:green }
                }
                .after-scope { color:red }
            }
            .after { color:black }
        ";
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse(css, ParserOptions::default()).unwrap();
        let prepared =
            prepare_stylesheets(&user_agent, &[author], MediaEnvironment::default(), 16.0);
        let rules = prepared.iter().map(|(_, rule)| rule).collect::<Vec<_>>();
        let selectors = rules
            .iter()
            .map(|rule| {
                rule.style_rule()
                    .selectors
                    .to_css_string(Default::default())
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(selectors, [".inside", ".outer", ".after-scope", ".after"]);
        assert_eq!(
            rules
                .iter()
                .map(|rule| rule.priority().source_order())
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert_eq!(prepared.scopes.len(), 2);
        assert_eq!(rules[0].scope(), ScopeId(1));
        assert_eq!(
            prepared.scope_descriptor(rules[0].scope()).parent,
            ScopeId(0)
        );
        assert_eq!(rules[1].scope(), ScopeId(0));
        assert_eq!(rules[2].scope(), ScopeId::NONE);
        assert_eq!(rules[3].scope(), ScopeId::NONE);
        assert_eq!(rules[0].priority().layer(), rules[2].priority().layer());
        assert!(rules[2].priority().layer() < rules[3].priority().layer());

        let key = |width| {
            prepared
                .media_queries()
                .match_key(MediaEnvironment::screen(width, None).unwrap(), 16.0)
        };
        assert_eq!(
            key(0.5),
            key(2.5),
            "skipped queries must not affect activation"
        );
        assert_ne!(key(400.0), key(600.0));
        assert_eq!(key(600.0), key(800.0));
        assert_ne!(
            key(800.0),
            key(1000.0),
            "inactive rules must retain dependencies"
        );
    }

    #[test]
    fn supports_declarations_allow_important_priority() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse(
            "@supports (color: green !important) { html { background-color: green } }",
            ParserOptions::default(),
        )
        .unwrap();
        let authors = [author];
        let prepared =
            prepare_stylesheets(&user_agent, &authors, MediaEnvironment::default(), 16.0);

        assert_eq!(prepared.len(), 1);
    }

    #[test]
    fn supports_declarations_reject_invalid_custom_property_priority() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse("@supports (--theme: green !bogus) { html { color: red } } @supports (--theme: fn(!bogus)) { html { color: green } }", ParserOptions::default()).unwrap();
        let authors = [author];
        let prepared =
            prepare_stylesheets(&user_agent, &authors, MediaEnvironment::default(), 16.0);

        assert_eq!(prepared.len(), 1);
    }

    #[test]
    fn supports_declarations_use_native_priority_and_token_boundaries() {
        for (declaration, expected) in [
            ("color: initial !important", true),
            ("font-family: inherit !important", true),
            ("float: initial !important", true),
            ("--theme: green /* !bogus */", true),
            ("--theme: green ! /* priority */ important", true),
            (r"--theme: green !\69mportant", true),
            (r"--a\:b: green !important", true),
            ("--theme: '!bogus'", true),
            ("--theme: fn(!bogus)", true),
            ("--theme: green !important /* trailing */", true),
            ("--theme: green !important !important", false),
            ("--theme: green; color: red", false),
            ("color: red !important junk", false),
            ("width: bogus", false),
            ("opacity: 0.5", false),
        ] {
            let css = format!("@supports ({declaration}) {{ html {{ color: green }} }}");
            let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
            let author = StyleSheet::parse(&css, ParserOptions::default()).unwrap();
            let prepared =
                prepare_stylesheets(&user_agent, &[author], MediaEnvironment::default(), 16.0);
            assert_eq!(prepared.len(), usize::from(expected), "{declaration}");
        }
    }

    #[test]
    fn supports_selector_uses_the_unforgiving_single_selector_grammar() {
        let user_agent = StyleSheet::parse("", ParserOptions::default()).unwrap();
        let author = StyleSheet::parse("@supports selector(div) { html { color: green } } @supports selector(div, div) { html { color: red } } @supports selector(:is(.ok, :unknown)) { html { background: red } }", ParserOptions::default()).unwrap();
        let authors = [author];
        let prepared =
            prepare_stylesheets(&user_agent, &authors, MediaEnvironment::default(), 16.0);

        assert_eq!(prepared.len(), 1);
    }
}
