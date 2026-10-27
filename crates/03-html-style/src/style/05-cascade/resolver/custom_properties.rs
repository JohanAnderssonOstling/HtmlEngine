//! Custom-property collection, cycle detection, inheritance, and substitution.

use super::*;
use lightningcss::properties::custom::{Function, UnparsedProperty};
use lightningcss::traits::ParseWithOptions;
use crate::style::syntax::substitution::{contains_lazy_function, contains_random_item, is_lazy_function};
pub(super) use crate::style::syntax::substitution::contains_random_math;
pub(super) use crate::style::syntax::substitution::needs_substitution;
use rustc_data_structures::fx::FxHasher;
use specified::{
    CandidateAction, CascadeBoundary, CascadeRollback, DeclarationEvent, candidate_action,
    rollback_keyword,
};
use static_self::IntoOwned;
use std::hash::{Hash, Hasher};

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

fn custom_declaration_identity<'sheet, 'css>(
    event: &DeclarationEvent<'sheet, 'css>,
    inline_style: Option<&StyleAttribute<'css>>,
) -> Option<CustomDeclarationIdentity> {
    let property = event.property(inline_style);
    custom_declaration(property)?;
    Some(CustomDeclarationIdentity {
        property_address: property as *const Property<'css> as usize,
        boundary: event.boundary,
    })
}

impl CustomCascadeCache {
    /// Look up a prior cascade without allocating on a hit. The hash narrows
    /// candidates only; declaration identity and ordering are compared again
    /// so hash collisions cannot reuse the wrong custom-property map.
    pub(super) fn probe<'sheet, 'css>(
        &self,
        events: &[DeclarationEvent<'sheet, 'css>],
        inline_style: Option<&StyleAttribute<'css>>,
        parent_map_id: u32,
    ) -> CustomCascadeProbe {
        // Custom-property random choices compute at their defining element.
        // Keep raw declaration/program reuse while avoiding a map frozen for
        // another element's auto or dynamically scoped random key.
        if events.iter().any(|event| {
            custom_declaration(event.property(inline_style))
                .is_some_and(|(_, value)| contains_random_item(value))
        }) {
            return CustomCascadeProbe::UncachedDeclarations;
        }
        let mut hasher = FxHasher::default();
        parent_map_id.hash(&mut hasher);
        let mut declaration_count = 0usize;
        for identity in events
            .iter()
            .filter_map(|event| custom_declaration_identity(event, inline_style))
        {
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
                    && events
                        .iter()
                        .filter_map(|event| custom_declaration_identity(event, inline_style))
                        .eq(entry.declarations.iter().copied())
                {
                    return CustomCascadeProbe::Hit(entry.map_id);
                }
            }
        }
        if self.entry_count >= MAX_CUSTOM_CASCADE_CACHE_ENTRIES {
            return CustomCascadeProbe::UncachedDeclarations;
        }

        let declarations = events
            .iter()
            .filter_map(|event| custom_declaration_identity(event, inline_style))
            .collect::<Vec<_>>()
            .into_boxed_slice();
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
        Property::Custom(custom)
            if custom.name.as_ref().starts_with("--")
                && !is_renderer_marker(custom.name.as_ref()) =>
        {
            Some((custom.name.as_ref(), &custom.value))
        }
        Property::Unparsed(unparsed)
            if unparsed.property_id.name().starts_with("--")
                && !is_renderer_marker(unparsed.property_id.name()) =>
        {
            Some((unparsed.property_id.name(), &unparsed.value))
        }
        _ => None,
    }
}

fn is_renderer_marker(name: &str) -> bool {
    matches!(
        name,
        WHITE_SPACE_CASCADE_MARKER
            | LETTER_SPACING_MARKER
            | WORD_SPACING_MARKER
            | TAB_SIZE_CASCADE_MARKER
    )
}

fn renderer_marker_declaration<'property, 'css>(
    property: &'property Property<'css>,
) -> Option<(&'property str, &'property TokenList<'css>)> {
    match property {
        Property::Custom(custom) if is_renderer_marker(custom.name.as_ref()) => {
            Some((custom.name.as_ref(), &custom.value))
        }
        Property::Unparsed(unparsed) if is_renderer_marker(unparsed.property_id.name()) => {
            Some((unparsed.property_id.name(), &unparsed.value))
        }
        _ => None,
    }
}

/// Select an internal renderer declaration with the same origin/layer rollback
/// rules as custom properties. Its computed value lives in `WorkingStyle`, so
/// it does not need to be serialized back into the inherited variable map.
pub(super) fn selected_renderer_marker<'sheet, 'css>(
    events: &[DeclarationEvent<'sheet, 'css>],
    inline_style: Option<&StyleAttribute<'css>>,
    name: &str,
    custom_properties: &FxHashMap<String, TokenList<'css>>,
    parent_custom: &FxHashMap<String, TokenList<'css>>,
    random: &RandomContext,
) -> Option<TokenList<'css>> {
    let mut rollbacks = Vec::<CascadeRollback>::new();
    for event in events.iter().rev() {
        let Some((candidate, value)) = renderer_marker_declaration(event.property(inline_style))
        else {
            continue;
        };
        if candidate != name {
            continue;
        }
        let keyword = single_ident_keyword(value).map(str::to_ascii_lowercase);
        let rollback = keyword.as_deref().and_then(rollback_keyword);
        match candidate_action(&rollbacks, event.boundary, rollback) {
            CandidateAction::Excluded => continue,
            CandidateAction::Rollback(kind) => {
                rollbacks.push(CascadeRollback::new(event.boundary, kind));
                continue;
            }
            CandidateAction::Select => {}
        }
        match keyword.as_deref() {
            Some("initial" | "inherit" | "unset") => return None,
            _ => {
                let mut resolved = value.clone();
                if needs_substitution(&resolved) || contains_random_math(&resolved) {
                    let variables = custom_properties
                        .iter()
                        .map(|(name, value)| (name.as_str(), value.clone()))
                        .collect::<HashMap<_, _>>();
                    let property = match name {
                        WHITE_SPACE_CASCADE_MARKER => PropertyId::WhiteSpace.name(),
                        LETTER_SPACING_MARKER => PropertyId::LetterSpacing.name(),
                        WORD_SPACING_MARKER => PropertyId::WordSpacing.name(),
                        TAB_SIZE_CASCADE_MARKER => "tab-size",
                        _ => unreachable!("known renderer marker"),
                    };
                    if !substitute_property_tokens(&mut resolved, &variables, parent_custom, random, property) {
                        return None;
                    }
                    if single_ident_keyword(&resolved)
                        .is_some_and(|keyword| keyword.eq_ignore_ascii_case("initial"))
                    {
                        let initial = if name == TAB_SIZE_CASCADE_MARKER {
                            "8"
                        } else {
                            "normal"
                        };
                        return TokenList::parse_string_with_options(
                            initial,
                            ParserOptions::default(),
                        )
                        .ok();
                    }
                }
                return Some(resolved);
            }
        }
    }
    None
}

/// Cascade custom properties as specified token values. Rollback discards
/// candidates by origin/layer rather than cloning the whole map at every
/// boundary.
pub(super) fn cascade_custom_properties<'sheet, 'css>(
    events: &[DeclarationEvent<'sheet, 'css>],
    inline_style: Option<&StyleAttribute<'css>>,
    parent: &FxHashMap<String, TokenList<'css>>,
    random: &RandomContext,
) -> Option<FxHashMap<String, TokenList<'css>>> {
    if !events
        .iter()
        .any(|event| custom_declaration(event.property(inline_style)).is_some())
    {
        return None;
    }
    let mut values = parent.clone();
    let mut decided = HashSet::<&str>::new();
    let mut rollbacks = FxHashMap::<&str, Vec<CascadeRollback>>::default();

    for event in events.iter().rev() {
        let Some((name, value)) = custom_declaration(event.property(inline_style)) else {
            continue;
        };
        if decided.contains(&name) {
            continue;
        }
        let keyword = single_ident_keyword(value).map(str::to_ascii_lowercase);
        let rollback = keyword.as_deref().and_then(rollback_keyword);
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
        match keyword.as_deref() {
            Some("initial") => {
                values.remove(name);
                decided.insert(name);
            }
            Some("inherit" | "unset") => {
                // `values` began as the inherited map.
                decided.insert(name);
            }
            _ => {
                values.insert(name.to_string(), value.clone());
                decided.insert(name);
            }
        }
    }
    resolve_custom_properties(&mut values, parent, random);
    Some(values)
}

pub(super) fn effective_custom_properties<'map, 'css>(
    properties: &'map Option<FxHashMap<String, TokenList<'css>>>,
    parent: &'map FxHashMap<String, TokenList<'css>>,
) -> &'map FxHashMap<String, TokenList<'css>> {
    properties.as_ref().unwrap_or(parent)
}

pub(super) fn single_ident_keyword<'tokens, 'css>(
    tokens: &'tokens TokenList<'css>,
) -> Option<&'tokens str> {
    let mut iter = tokens.0.iter().filter(|token| !is_ignorable_token(token));
    let first = iter.next()?;
    if iter.next().is_some() {
        return None;
    }
    match first {
        TokenOrValue::Token(Token::Ident(ident)) => Some(ident.as_ref()),
        _ => None,
    }
}

pub(super) fn is_ignorable_token(token: &TokenOrValue) -> bool {
    matches!(
        token,
        TokenOrValue::Token(Token::WhiteSpace(_)) | TokenOrValue::Token(Token::Comment(_))
    )
}

struct CustomPropertyDependencyCollector {
    names: Vec<String>,
}

impl<'i> Visitor<'i> for CustomPropertyDependencyCollector {
    type Error = Infallible;

    fn visit_types(&self) -> lightningcss::visitor::VisitTypes {
        lightningcss::visit_types!(VARIABLES | FUNCTIONS)
    }

    fn visit_variable(&mut self, variable: &mut Variable<'i>) -> Result<(), Self::Error> {
        let name = variable.name.ident.as_ref().to_string();
        if !self.names.contains(&name) {
            self.names.push(name);
        }
        variable.visit_children(self)
    }

    fn visit_function(&mut self, function: &mut Function<'i>) -> Result<(), Self::Error> {
        // Only evaluated names, keys and selected branches create local
        // dependencies. Unused choices must not create property cycles.
        if is_lazy_function(function.name.as_ref()) {
            Ok(())
        } else {
            function.visit_children(self)
        }
    }
}

fn custom_property_dependencies(tokens: &TokenList<'_>) -> Vec<String> {
    let mut tokens = tokens.clone();
    let mut collector = CustomPropertyDependencyCollector { names: Vec::new() };
    let result = tokens.visit(&mut collector);
    match result {
        Ok(()) => collector.names,
        Err(error) => match error {},
    }
}

struct CustomPropertyCycleDetector<'a> {
    dependencies: &'a FxHashMap<String, Vec<String>>,
    next_index: usize,
    indices: FxHashMap<String, usize>,
    lowlinks: FxHashMap<String, usize>,
    stack: Vec<String>,
    on_stack: HashSet<String>,
    cyclic: HashSet<String>,
}

impl CustomPropertyCycleDetector<'_> {
    fn visit(&mut self, name: &str) {
        let index = self.next_index;
        self.next_index += 1;
        self.indices.insert(name.to_string(), index);
        self.lowlinks.insert(name.to_string(), index);
        self.stack.push(name.to_string());
        self.on_stack.insert(name.to_string());

        for dependency in self.dependencies.get(name).cloned().unwrap_or_default() {
            if !self.indices.contains_key(&dependency) {
                self.visit(&dependency);
                let dependency_lowlink = self.lowlinks[&dependency];
                self.lowlinks
                    .entry(name.to_string())
                    .and_modify(|lowlink| *lowlink = (*lowlink).min(dependency_lowlink));
            } else if self.on_stack.contains(&dependency) {
                let dependency_index = self.indices[&dependency];
                self.lowlinks
                    .entry(name.to_string())
                    .and_modify(|lowlink| *lowlink = (*lowlink).min(dependency_index));
            }
        }

        if self.lowlinks[name] != self.indices[name] {
            return;
        }
        let mut component = Vec::new();
        while let Some(member) = self.stack.pop() {
            self.on_stack.remove(&member);
            let is_root = member == name;
            component.push(member);
            if is_root {
                break;
            }
        }
        let self_referential = component.len() == 1
            && self.dependencies.get(name).is_some_and(|dependencies| {
                dependencies.iter().any(|dependency| dependency == name)
            });
        if component.len() > 1 || self_referential {
            self.cyclic.extend(component);
        }
    }
}

fn cyclic_custom_properties<'a>(
    specified: &FxHashMap<String, TokenList<'a>>,
) -> (FxHashMap<String, Vec<String>>, HashSet<String>) {
    let dependencies = specified
        .iter()
        .map(|(name, value)| {
            let local_dependencies = custom_property_dependencies(value)
                .into_iter()
                .filter(|dependency| specified.contains_key(dependency))
                .collect();
            (name.clone(), local_dependencies)
        })
        .collect::<FxHashMap<_, _>>();
    let mut detector = CustomPropertyCycleDetector {
        dependencies: &dependencies,
        next_index: 0,
        indices: FxHashMap::default(),
        lowlinks: FxHashMap::default(),
        stack: Vec::new(),
        on_stack: HashSet::new(),
        cyclic: HashSet::new(),
    };
    for name in specified.keys() {
        if !detector.indices.contains_key(name) {
            detector.visit(name);
        }
    }
    let cyclic = detector.cyclic;
    (dependencies, cyclic)
}

fn resolve_custom_property<'a>(
    name: &str,
    specified: &FxHashMap<String, TokenList<'a>>,
    parent_custom: &FxHashMap<String, TokenList<'a>>,
    dependencies: &FxHashMap<String, Vec<String>>,
    cyclic: &mut HashSet<String>,
    resolving: &mut Vec<String>,
    resolved: &mut FxHashMap<String, TokenList<'a>>,
    random: &RandomContext,
) -> bool {
    if resolved.contains_key(name) {
        return true;
    }
    if cyclic.contains(name) {
        return false;
    }
    if let Some(start) = resolving.iter().position(|active| active == name) {
        cyclic.extend(resolving[start..].iter().cloned());
        return false;
    }
    resolving.push(name.to_string());
    let Some(mut value) = specified.get(name).cloned() else {
        resolving.pop();
        return false;
    };

    if let Some(names) = dependencies.get(name) {
        for dependency in names {
            let _ = resolve_custom_property(
                dependency,
                specified,
                parent_custom,
                dependencies,
                cyclic,
                resolving,
                resolved,
                random,
            );
        }
    }

    let valid = if contains_lazy_function(&value) {
        substitution::substitute(&mut value, parent_custom, random, name, |dependency| {
            let _ = resolve_custom_property(
                dependency,
                specified,
                parent_custom,
                dependencies,
                cyclic,
                resolving,
                resolved,
                random,
            );
            resolved.get(dependency).cloned()
        })
    } else {
        let variables = resolved
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect::<HashMap<_, _>>();
        mark_var_substitution_boundaries(&mut value);
        value.substitute_variables(&variables);
        !needs_substitution(&value)
    };
    resolving.pop();
    if !valid || cyclic.contains(name) {
        return false;
    }
    if let Some(keyword) = single_ident_keyword(&value).map(str::to_ascii_lowercase) {
        match keyword.as_str() {
            "initial" => return false,
            "inherit" | "unset" => {
                let Some(inherited) = parent_custom.get(name) else {
                    return false;
                };
                value = inherited.clone();
            }
            _ => {}
        }
    }
    resolved.insert(name.to_string(), value);
    true
}

pub(super) fn resolve_custom_properties<'a>(
    custom_properties: &mut FxHashMap<String, TokenList<'a>>,
    parent_custom: &FxHashMap<String, TokenList<'a>>,
    random: &RandomContext,
) {
    // The common EPUB case is a flat set of literal custom properties. Those
    // values are already final after the cascade above: inherited entries came
    // from the parent's resolved map, and CSS-wide keywords were handled while
    // selecting each winner. Building a dependency graph and running Tarjan's
    // algorithm for such a map only clones every name and token list several
    // times per element.
    if custom_properties
        .values()
        .all(|value| !needs_substitution(value))
    {
        return;
    }
    let specified = custom_properties.clone();
    let (dependencies, mut cyclic) = cyclic_custom_properties(&specified);
    let mut resolving = Vec::new();
    let mut resolved = FxHashMap::default();
    for name in specified.keys() {
        let _ = resolve_custom_property(
            name,
            &specified,
            parent_custom,
            &dependencies,
            &mut cyclic,
            &mut resolving,
            &mut resolved,
            random,
        );
    }
    *custom_properties = resolved;
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

pub(super) fn single_var_property_is_resolvable(
    unparsed: &lightningcss::properties::custom::UnparsedProperty<'_>,
    custom_properties: &FxHashMap<String, TokenList<'_>>,
) -> bool {
    let [TokenOrValue::Var(var)] = unparsed.value.0.as_slice() else {
        return false;
    };
    custom_properties
        .get(var.name.ident.as_ref())
        .is_some_and(|value| !contains_random_math(value) && token_list_is_serializable(value))
}

pub(super) fn resolve_single_var_property<'a>(
    unparsed: &UnparsedProperty<'a>,
    custom_properties: &FxHashMap<String, TokenList<'a>>,
) -> Option<Property<'a>> {
    let [TokenOrValue::Var(var)] = unparsed.value.0.as_slice() else {
        return None;
    };
    let replacement = custom_properties.get(var.name.ident.as_ref())?;
    if contains_random_math(replacement) { return None; }
    parse_substituted_property(&unparsed.property_id, replacement)
}

pub(super) fn parse_substituted_property<'a>(
    id: &PropertyId<'_>,
    tokens: &TokenList<'_>,
) -> Option<Property<'a>> {
    use crate::style::syntax::{box_syntax, value};

    let css = token_list_to_css_string(tokens).or_else(|| {
        Property::Unparsed(UnparsedProperty { property_id: id.clone(), value: tokens.clone() })
            .value_to_css_string(PrinterOptions::default()).ok()
    })?;
    let normalized = value::normalize_property_value(id, &css);
    if box_syntax::property_value_is_valid(id, &normalized) == Some(false) {
        return None;
    }
    value::parsed_property(id.clone(), &normalized).map(IntoOwned::into_owned)
}

pub(super) fn token_list_to_css_string(tokens: &TokenList<'_>) -> Option<String> {
    let mut out = String::new();
    write_token_list(tokens, &mut out)?;
    Some(out)
}

fn token_list_is_serializable(tokens: &TokenList<'_>) -> bool {
    tokens.0.iter().all(|token| match token {
        TokenOrValue::Color(_)
        | TokenOrValue::Length(_)
        | TokenOrValue::Angle(_)
        | TokenOrValue::Time(_)
        | TokenOrValue::Resolution(_)
        | TokenOrValue::DashedIdent(_)
        | TokenOrValue::AnimationName(_)
        | TokenOrValue::Token(_) => true,
        TokenOrValue::Function(function) => token_list_is_serializable(&function.arguments),
        _ => false,
    })
}

fn write_token_list(tokens: &TokenList<'_>, out: &mut String) -> Option<()> {
    for token in &tokens.0 {
        match token {
            TokenOrValue::Color(color) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                color.to_css(&mut printer).ok()?;
            }
            TokenOrValue::Function(function) => {
                out.push_str(function.name.as_ref());
                out.push('(');
                write_token_list(&function.arguments, out)?;
                out.push(')');
            }
            TokenOrValue::Length(length) => {
                let (value, unit) = length.to_unit_value();
                write_dimension(value,unit,out)?;
            }
            TokenOrValue::Angle(angle) => write_dimension(angle.to_degrees(),"deg",out)?,
            TokenOrValue::Time(time) => write_dimension(time.to_ms(),"ms",out)?,
            TokenOrValue::Resolution(value) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                value.to_css(&mut printer).ok()?;
            }
            TokenOrValue::DashedIdent(ident) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                ident.to_css(&mut printer).ok()?;
            }
            TokenOrValue::AnimationName(name) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                name.to_css(&mut printer).ok()?;
            }
            TokenOrValue::Token(token) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                token.to_css(&mut printer).ok()?;
            }
            _ => return None,
        }
    }
    Some(())
}

fn write_dimension(value:f32,unit:&str,out:&mut String)->Option<()> {
    // Numeric functions must retain the dimension of zero.
    let token=Token::Dimension {has_sign:value.is_sign_negative(),value,int_value:None,unit:unit.into()};
    token.to_css(&mut Printer::new(out,PrinterOptions::default())).ok()
}

pub(super) fn substitute_property_tokens<'css>(
    tokens: &mut TokenList<'css>,
    variables: &HashMap<&str, TokenList<'css>>,
    parent: &FxHashMap<String, TokenList<'css>>,
    random: &RandomContext,
    property: &str,
) -> bool {
    if contains_lazy_function(tokens) || contains_random_math(tokens) || variables.values().any(contains_random_math) {
        substitution::substitute(tokens, parent, random, property, |name| variables.get(name).cloned())
    } else {
        mark_var_substitution_boundaries(tokens);
        tokens.substitute_variables(variables);
        !needs_substitution(tokens)
    }
}
