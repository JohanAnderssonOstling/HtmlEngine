//! Conversion of native raw font tokens into OpenType feature settings.

use super::value::{significant_tokens, token_ident};
use crate::style::syntax::property_id::RendererPropertyId;
use cssparser::match_ignore_ascii_case;
use html_style_model::OpenTypeFeature;
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::properties::font::FontVariantCaps;
use lightningcss::traits::Parse;
use std::iter::once;

/// The supported caps, numeric and ligature portions of `font-variant`.
/// Unknown or conflicting components invalidate the complete declaration.
pub(crate) fn parse_variant(
    tokens: &TokenList<'_>,
) -> Option<(FontVariantCaps, Vec<OpenTypeFeature>, Vec<OpenTypeFeature>)> {
    let values = significant_tokens(tokens).collect::<Vec<_>>();
    if values.len() == 1 {
        let value = token_ident(values[0])?;
        if value.eq_ignore_ascii_case("normal") {
            return Some((FontVariantCaps::Normal, Vec::new(), Vec::new()));
        }
        if value.eq_ignore_ascii_case("none") {
            return Some((
                FontVariantCaps::Normal,
                Vec::new(),
                parse_tokens(RendererPropertyId::FontVariantLigatures, tokens)?,
            ));
        }
    }
    let mut caps = None;
    let mut numeric = TokenList(Vec::new());
    let mut ligatures = TokenList(Vec::new());
    for token in values {
        let value = token_ident(token)?;
        if value.eq_ignore_ascii_case("normal") || value.eq_ignore_ascii_case("none") {
            return None;
        }
        if let Ok(value) = FontVariantCaps::parse_string(value) {
            if caps.replace(value).is_some() {
                return None;
            }
        } else {
            let single = TokenList(vec![token.clone()]);
            if parse_tokens(RendererPropertyId::FontVariantNumeric, &single).is_some() {
                numeric.0.push(token.clone());
            } else if parse_tokens(RendererPropertyId::FontVariantLigatures, &single).is_some() {
                ligatures.0.push(token.clone());
            } else {
                return None;
            }
        }
    }
    if caps.is_none() && numeric.0.is_empty() && ligatures.0.is_empty() {
        return None;
    }
    Some((
        caps.unwrap_or(FontVariantCaps::Normal),
        if numeric.0.is_empty() {
            Vec::new()
        } else {
            parse_tokens(RendererPropertyId::FontVariantNumeric, &numeric)?
        },
        if ligatures.0.is_empty() {
            Vec::new()
        } else {
            parse_tokens(RendererPropertyId::FontVariantLigatures, &ligatures)?
        },
    ))
}

pub(crate) fn parse_tokens(
    id: RendererPropertyId,
    tokens: &TokenList<'_>,
) -> Option<Vec<OpenTypeFeature>> {
    let mut tokens = significant_tokens(tokens);
    let first = token_ident(tokens.next()?)?;
    if tokens.clone().next().is_none() {
        if first.eq_ignore_ascii_case("normal") {
            return match id {
                RendererPropertyId::FontKerning => Some(vec![OpenTypeFeature::new(*b"kern", 1)]),
                RendererPropertyId::FontVariantNumeric
                | RendererPropertyId::FontVariantLigatures => Some(Vec::new()),
                _ => None,
            };
        }
        if first.eq_ignore_ascii_case("none") {
            return match id {
                RendererPropertyId::FontKerning => Some(vec![OpenTypeFeature::new(*b"kern", 0)]),
                RendererPropertyId::FontVariantLigatures => Some(
                    [*b"liga", *b"clig", *b"dlig", *b"hlig", *b"calt"]
                        .into_iter()
                        .map(|tag| OpenTypeFeature::new(tag, 0))
                        .collect(),
                ),
                _ => None,
            };
        }
        if id == RendererPropertyId::FontKerning && first.eq_ignore_ascii_case("auto") {
            return Some(Vec::new());
        }
    }

    let mut seen = 0u8;
    let mut features = Vec::new();
    for keyword in once(Some(first)).chain(tokens.map(token_ident)) {
        let keyword = keyword?;
        let (category, tags, value): (u8, &[[u8; 4]], u32) = match id {
            RendererPropertyId::FontVariantNumeric => match_ignore_ascii_case! { keyword,
                "lining-nums" => (1, &[*b"lnum"], 1),
                "oldstyle-nums" => (1, &[*b"onum"], 1),
                "proportional-nums" => (2, &[*b"pnum"], 1),
                "tabular-nums" => (2, &[*b"tnum"], 1),
                "diagonal-fractions" => (4, &[*b"frac"], 1),
                "stacked-fractions" => (4, &[*b"afrc"], 1),
                "ordinal" => (8, &[*b"ordn"], 1),
                "slashed-zero" => (16, &[*b"zero"], 1),
                _ => return None,
            },
            RendererPropertyId::FontVariantLigatures => match_ignore_ascii_case! { keyword,
                "common-ligatures" => (1, &[*b"liga", *b"clig"], 1),
                "no-common-ligatures" => (1, &[*b"liga", *b"clig"], 0),
                "discretionary-ligatures" => (2, &[*b"dlig"], 1),
                "no-discretionary-ligatures" => (2, &[*b"dlig"], 0),
                "historical-ligatures" => (4, &[*b"hlig"], 1),
                "no-historical-ligatures" => (4, &[*b"hlig"], 0),
                "contextual" => (8, &[*b"calt"], 1),
                "no-contextual" => (8, &[*b"calt"], 0),
                _ => return None,
            },
            _ => return None,
        };
        if seen & category != 0 {
            return None;
        }
        seen |= category;
        features.extend(tags.iter().map(|tag| OpenTypeFeature::new(*tag, value)));
    }
    Some(features)
}

pub(crate) fn parse_settings(tokens: &TokenList<'_>) -> Option<Vec<OpenTypeFeature>> {
    let tokens: Vec<_> = significant_tokens(tokens).collect();
    if matches!(tokens.as_slice(), [token] if token_ident(token).is_some_and(|word| word.eq_ignore_ascii_case("normal")))
    {
        return Some(Vec::new());
    }
    tokens
        .split(|token| matches!(token, TokenOrValue::Token(Token::Comma)))
        .map(|setting| {
            let [TokenOrValue::Token(Token::String(tag)), rest @ ..] = setting else {
                return None;
            };
            let bytes: [u8; 4] = tag.as_bytes().try_into().ok()?;
            if !bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
                return None;
            }
            let value = match rest {
                [] => 1,
                [
                    TokenOrValue::Token(Token::Number {
                        int_value: Some(value),
                        ..
                    }),
                ] => u32::try_from(*value).ok()?,
                [token]
                    if token_ident(token).is_some_and(|word| word.eq_ignore_ascii_case("on")) =>
                {
                    1
                }
                [token]
                    if token_ident(token).is_some_and(|word| word.eq_ignore_ascii_case("off")) =>
                {
                    0
                }
                _ => return None,
            };
            Some(OpenTypeFeature::new(bytes, value))
        })
        .collect()
}
