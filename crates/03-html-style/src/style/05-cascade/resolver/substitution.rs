//! Lazy arbitrary-value functions and current-element variable arguments.

use super::custom_properties::token_list_to_css_string;
use super::random::RandomContext;
use crate::style::syntax::random_math;
use crate::style::syntax::substitution::{
    arguments, contains_random_math, custom_name, is_custom_function, is_inherit, is_random_item,
    is_random_math, random_item_arguments,
};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::traits::ParseWithOptions;
use rustc_data_structures::fx::FxHashMap;
use static_self::IntoOwned;

pub(super) fn substitute<'css>(
    tokens: &mut TokenList<'css>,
    parent: &FxHashMap<String, TokenList<'css>>,
    random: &RandomContext,
    property: &str,
    lookup: impl FnMut(&str) -> Option<TokenList<'css>>,
) -> bool {
    Substitution {
        parent,
        random,
        property,
        index: 1,
        numeric_context: !property.starts_with("--"),
        lookup,
    }
    .resolve(tokens, 0)
    .is_ok()
}

struct Substitution<'a, 'css, Lookup> {
    parent: &'a FxHashMap<String, TokenList<'css>>,
    random: &'a RandomContext,
    property: &'a str,
    index: usize,
    numeric_context: bool,
    lookup: Lookup,
}

impl<'css, Lookup: FnMut(&str) -> Option<TokenList<'css>>> Substitution<'_, 'css, Lookup> {
    fn resolve(&mut self, tokens: &mut TokenList<'css>, depth: usize) -> Result<(), ()> {
        if depth >= 128 {
            return Err(());
        }
        let mut result = Vec::with_capacity(tokens.0.len());
        for mut token in std::mem::take(&mut tokens.0) {
            let replacement = match &mut token {
                TokenOrValue::Var(variable) => {
                    if let Some(mut value) = (self.lookup)(variable.name.ident.as_ref()) {
                        if self.numeric_context && contains_random_math(&value) {
                            self.resolve(&mut value, depth + 1)?;
                        }
                        Some(value)
                    } else {
                        let mut fallback = variable.fallback.take().ok_or(())?;
                        self.resolve(&mut fallback, depth + 1)?;
                        Some(fallback)
                    }
                }
                TokenOrValue::Function(function) if is_inherit(function.name.as_ref()) => {
                    let (name, fallback) = arguments(&function.arguments)?;
                    let mut name = TokenList(name.to_vec());
                    let valid_name = self.resolve(&mut name, depth + 1).is_ok();
                    if let Some(value) = valid_name
                        .then(|| custom_name(&name.0).and_then(|name| self.parent.get(name)))
                        .flatten()
                    {
                        Some(value.clone())
                    } else {
                        let mut fallback = TokenList(fallback.ok_or(())?.to_vec());
                        self.resolve(&mut fallback, depth + 1)?;
                        Some(fallback)
                    }
                }
                TokenOrValue::Function(function) if is_random_item(function.name.as_ref()) => {
                    let arguments = random_item_arguments(&function.arguments)?;
                    let index = self.index;
                    self.index += 1;
                    let mut key = TokenList(arguments[0].to_vec());
                    let numeric_context = self.numeric_context;
                    self.numeric_context = true;
                    let result = self.resolve(&mut key, depth + 1);
                    self.numeric_context = numeric_context;
                    result?;
                    let css = token_list_to_css_string(&key).ok_or(())?;
                    let key = random_math::for_computation(&css, |key| {
                        self.random.base(key, self.property, index)
                    })?;
                    let base = self.random.base(&key, self.property, index).ok_or(())?;
                    let count = arguments.len() - 1;
                    let selected = ((base * count as f64).floor() as usize).min(count - 1);
                    let mut value = TokenList(arguments[selected + 1].to_vec());
                    self.resolve(&mut value, depth + 1)?;
                    Some(value)
                }
                TokenOrValue::Function(function)
                    if is_random_math(function.name.as_ref()) && self.numeric_context =>
                {
                    let index = self.index;
                    self.index += 1;
                    self.resolve(&mut function.arguments, depth + 1)?;
                    let original = token_list_to_css_string(&function.arguments).ok_or(())?;
                    let css = random_math::for_computation(&original, |key| {
                        self.random.base(key, self.property, index)
                    })?;
                    let args = random_math::arguments(&css).ok_or(())?;
                    let base = self.random.base(args.key, self.property, index).ok_or(())?;
                    let step = args.step.map(|step| format!(",{step}")).unwrap_or_default();
                    let frozen = format!("fixed {base},{},{}{step}", args.minimum, args.maximum);
                    function.arguments =
                        TokenList::parse_string_with_options(&frozen, ParserOptions::default())
                            .map_err(|_| ())?
                            .into_owned();
                    None
                }
                TokenOrValue::Function(function) if is_custom_function(function.name.as_ref()) => {
                    // No @function definitions are installed by this engine.
                    // Missing names produce the guaranteed-invalid value
                    // before their arguments are evaluated.
                    return Err(());
                }
                TokenOrValue::Function(function) => {
                    self.resolve(&mut function.arguments, depth + 1)?;
                    None
                }
                _ => None,
            };
            if let Some(value) = replacement {
                // Keep component boundaries when the computed tokens are printed.
                result.push(TokenOrValue::Token(Token::Comment("".into())));
                result.extend(value.0);
                result.push(TokenOrValue::Token(Token::Comment("".into())));
            } else {
                result.push(token);
            }
            if result.len() > 1 << 20 {
                return Err(());
            }
        }
        tokens.0 = result;
        Ok(())
    }
}
