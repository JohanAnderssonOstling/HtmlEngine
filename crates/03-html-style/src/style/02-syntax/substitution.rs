//! Queries over token lists parsed by Lightning CSS.
use lightningcss::properties::custom::{TokenList, TokenOrValue, UnresolvedColor};

pub(crate) fn needs_substitution(tokens: &TokenList<'_>) -> bool {
    tokens.0.iter().any(|token| match token {
        TokenOrValue::Var(_) => true,
        TokenOrValue::Function(function) => needs_substitution(&function.arguments),
        TokenOrValue::Env(environment) => environment.fallback.as_ref().is_some_and(needs_substitution),
        TokenOrValue::UnresolvedColor(color) => match color {
            UnresolvedColor::RGB { alpha, .. } | UnresolvedColor::HSL { alpha, .. } => needs_substitution(alpha),
            UnresolvedColor::LightDark { light, dark } => needs_substitution(light) || needs_substitution(dark),
        },
        _ => false,
    })
}

pub(crate) fn contains_function(
    tokens: &TokenList<'_>,
    test: impl Fn(&str) -> bool + Copy,
) -> bool {
    tokens.0.iter().any(|token| match token {
        TokenOrValue::Function(function) => {
            test(function.name.as_ref()) || contains_function(&function.arguments, test)
        }
        TokenOrValue::Var(variable) => variable
            .fallback
            .as_ref()
            .is_some_and(|fallback| contains_function(fallback, test)),
        _ => false,
    })
}
