//! Custom-property collection, cycle detection, inheritance, and substitution.

use super::*;
use specified::{CascadeBoundary, DeclarationEvent};
use static_self::IntoOwned;

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
    inline_style: Option<&StyleAttribute<'css>>,
    parent: &FxHashMap<String, TokenList<'css>>,
) -> Option<FxHashMap<String, TokenList<'css>>> {
    if !events
        .iter()
        .any(|event| custom_declaration(event.property(inline_style)).is_some())
    {
        return None;
    }
    let mut values = parent.clone();
    let mut decided = HashSet::<&str>::new();
    let mut rollbacks = FxHashMap::<&str, Vec<(CascadeBoundary, bool)>>::default();

    for event in events.iter().rev() {
        let Some((name, value)) = custom_declaration(event.property(inline_style)) else {
            continue;
        };
        if decided.contains(&name)
            || rollbacks.get(&name).is_some_and(|entries| {
                entries
                    .iter()
                    .any(|(boundary, layer)| boundary.rollback_excludes(event.boundary, *layer))
            })
        {
            continue;
        }
        let keyword = single_ident_keyword(value).map(str::to_ascii_lowercase);
        match keyword.as_deref() {
            Some("revert") => rollbacks
                .entry(name)
                .or_default()
                .push((event.boundary, false)),
            Some("revert-layer") => rollbacks
                .entry(name)
                .or_default()
                .push((event.boundary, true)),
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
    resolve_custom_properties(&mut values, parent);
    Some(values)
}

pub(super) fn effective_custom_properties<'map, 'css>(
    properties: &'map Option<FxHashMap<String, TokenList<'css>>>,
    parent: &'map FxHashMap<String, TokenList<'css>>,
) -> &'map FxHashMap<String, TokenList<'css>> {
    properties.as_ref().unwrap_or(parent)
}

pub(super) fn update_custom_property<'css>(
    properties: &mut Option<FxHashMap<String, TokenList<'css>>>,
    parent: &FxHashMap<String, TokenList<'css>>,
    name: &str,
    value: TokenList<'css>,
) {
    if effective_custom_properties(properties, parent).get(name) == Some(&value) {
        return;
    }
    properties
        .get_or_insert_with(|| parent.clone())
        .insert(name.to_string(), value);
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
        lightningcss::visit_types!(VARIABLES)
    }

    fn visit_variable(&mut self, variable: &mut Variable<'i>) -> Result<(), Self::Error> {
        let name = variable.name.ident.as_ref().to_string();
        if !self.names.contains(&name) {
            self.names.push(name);
        }
        variable.visit_children(self)
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
    cyclic: &HashSet<String>,
    resolving: &mut HashSet<String>,
    resolved: &mut FxHashMap<String, TokenList<'a>>,
) -> bool {
    if resolved.contains_key(name) {
        return true;
    }
    if cyclic.contains(name) || !resolving.insert(name.to_string()) {
        return false;
    }
    let Some(mut value) = specified.get(name).cloned() else {
        resolving.remove(name);
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
            );
        }
    }

    let variables = resolved
        .iter()
        .map(|(name, value)| (name.as_str(), value.clone()))
        .collect::<HashMap<_, _>>();
    mark_var_substitution_boundaries(&mut value);
    value.substitute_variables(&variables);
    resolving.remove(name);
    if token_list_contains_var(&value) {
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
) {
    // The common EPUB case is a flat set of literal custom properties. Those
    // values are already final after the cascade above: inherited entries came
    // from the parent's resolved map, and CSS-wide keywords were handled while
    // selecting each winner. Building a dependency graph and running Tarjan's
    // algorithm for such a map only clones every name and token list several
    // times per element.
    if custom_properties
        .values()
        .all(|value| !token_list_contains_var(value))
    {
        return;
    }
    let specified = custom_properties.clone();
    let (dependencies, cyclic) = cyclic_custom_properties(&specified);
    let mut resolving = HashSet::new();
    let mut resolved = FxHashMap::default();
    for name in specified.keys() {
        let _ = resolve_custom_property(
            name,
            &specified,
            parent_custom,
            &dependencies,
            &cyclic,
            &mut resolving,
            &mut resolved,
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
        .is_some_and(token_list_is_serializable)
}

pub(super) fn resolve_single_var_property<'a>(
    unparsed: &lightningcss::properties::custom::UnparsedProperty<'a>,
    custom_properties: &FxHashMap<String, TokenList<'a>>,
) -> Option<Property<'a>> {
    let [TokenOrValue::Var(var)] = unparsed.value.0.as_slice() else {
        return None;
    };
    let replacement = custom_properties.get(var.name.ident.as_ref())?;
    let css = token_list_to_css_string(replacement)?;
    Property::parse_string(
        unparsed.property_id.clone(),
        &css,
        ParserOptions::default(),
    )
    .ok()
    .map(IntoOwned::into_owned)
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
                let mut printer = Printer::new(out, PrinterOptions::default());
                length.to_css(&mut printer).ok()?;
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

pub(super) fn canonical_text_spacing_tokens<'a>(spacing: TextSpacing) -> Option<TokenList<'a>> {
    let absolute_px = spacing.absolute_px();
    let percentage = spacing.font_size_fraction() * 100.0;
    let css = if percentage == 0.0 {
        format!("{absolute_px}px")
    } else if absolute_px == 0.0 {
        format!("{percentage}%")
    } else if percentage.is_sign_negative() {
        format!("calc({absolute_px}px - {}%)", percentage.abs())
    } else {
        format!("calc({absolute_px}px + {percentage}%)")
    };
    let leaked: &'a str = Box::leak(css.into_boxed_str());
    TokenList::parse_string_with_options(leaked, ParserOptions::default()).ok()
}

pub(super) fn canonical_tab_size_tokens<'a>(tab_size: TabSize) -> Option<TokenList<'a>> {
    let css = match tab_size.kind() {
        html_style_model::TabSizeKind::Spaces => tab_size.value().to_string(),
        html_style_model::TabSizeKind::LengthPx => format!("{}px", tab_size.value()),
    };
    let leaked: &'a str = Box::leak(css.into_boxed_str());
    TokenList::parse_string_with_options(leaked, ParserOptions::default()).ok()
}

pub(super) fn token_list_contains_var(tokens: &TokenList<'_>) -> bool {
    tokens.0.iter().any(|token| match token {
        TokenOrValue::Var(_) => true,
        TokenOrValue::Function(function) => token_list_contains_var(&function.arguments),
        _ => false,
    })
}
