//! Shared grammar and OpenType features for renderer-owned font switches.

use super::property_id::RendererPropertyId;
use super::value::{IntegerValue, normalize_math, parse_integer_value};
use cssparser::{ParseError, Parser, ParserInput, match_ignore_ascii_case};
use html_style_model::OpenTypeFeature;
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use std::iter::once;

pub(crate) fn parse_tokens(
    id: RendererPropertyId,
    tokens: &TokenList<'_>,
) -> Option<Vec<OpenTypeFeature>> {
    let mut tokens = tokens.0.iter().filter(|token| {
        !matches!(
            token,
            TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_))
        )
    });
    let first = ident(tokens.next()?)?;
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
    for keyword in once(Some(first)).chain(tokens.map(ident)) {
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

fn ident<'a, 'css>(token: &'a TokenOrValue<'css>) -> Option<&'a str> {
    match token {
        TokenOrValue::Token(Token::Ident(value)) => Some(value.as_ref()),
        _ => None,
    }
}

pub(crate) struct FontFeatureSetting {
    pub(crate) tag: [u8; 4],
    pub(crate) value: IntegerValue,
}

pub(crate) fn parse_settings(css: &str) -> Option<Vec<FontFeatureSetting>> {
    let normalized = normalize_math(css);
    let mut input = ParserInput::new(&normalized);
    let mut parser = Parser::new(&mut input);
    if parser
        .try_parse(|parser| {
            parser.expect_ident_matching("normal")?;
            parser.expect_exhausted()
        })
        .is_ok()
    {
        return Some(Vec::new());
    }
    let parsed: Result<_, ParseError<'_, ()>> = parser.parse_comma_separated(|parser| {
        let text = parser.expect_string_cloned()?;
        let bytes = text.as_bytes();
        if bytes.len() != 4 || !bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
            return Err(parser.new_custom_error(()));
        }
        let mut tag = [0; 4];
        tag.copy_from_slice(bytes);
        let value = if parser.is_exhausted() {
            IntegerValue::Integer(1)
        } else if parser
            .try_parse(|parser| parser.expect_ident_matching("on"))
            .is_ok()
        {
            IntegerValue::Integer(1)
        } else if parser
            .try_parse(|parser| parser.expect_ident_matching("off"))
            .is_ok()
        {
            IntegerValue::Integer(0)
        } else {
            parse_integer_value(parser, 0)?
        };
        Ok(FontFeatureSetting { tag, value })
    });
    parsed.ok()
}
