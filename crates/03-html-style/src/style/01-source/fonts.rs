//! Owned font declarations for host text systems. No font bytes are loaded here.

use html_dom::has_uri_scheme;
use lightningcss::printer::PrinterOptions;
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue};
use lightningcss::properties::font::{AbsoluteFontWeight, FontFamily, FontStretch, FontWeight};
use lightningcss::rules::font_face::{
    FontFaceProperty, FontFaceRule, FontFormat, FontStyle, Source,
};
use lightningcss::rules::{CssRule, CssRuleList};
use lightningcss::traits::{ParseWithOptions, ToCss};
use lightningcss::stylesheet::ParserOptions;
use lightningcss::values::percentage::Percentage;
use std::borrow::Cow;
use std::ops::RangeInclusive;

/// A usable `@font-face` declaration. Hosts load and register these faces under
/// `family`, which may differ from the font file's internal family name.
/// Faces retain stylesheet order, including multiple faces for the same family.
#[derive(Clone, Debug, PartialEq)]
pub struct FontFace {
    /// Decoded family name, without CSS quotes or escapes.
    pub family: String,
    /// Alternatives in authored order. Try the next source if one is unavailable
    /// or its format/technology is unsupported by the host.
    pub sources: Vec<FontFaceSource>,
    /// Inclusive CSS weight range; defaults to 400..=400.
    pub weight: RangeInclusive<f32>,
    pub style: FontFaceStyle,
    /// Inclusive percentages, where 100 is normal width.
    pub stretch: RangeInclusive<f32>,
    /// Inclusive Unicode code point ranges. Defaults to U+0-10FFFF.
    /// The host intersects this coverage with the font's actual character map.
    pub unicode_ranges: Vec<RangeInclusive<u32>>,
    /// Other descriptors, preserved as CSS for host policy (e.g. font-display).
    /// The engine does not apply these descriptors.
    pub additional_descriptors: Vec<FontFaceDescriptor>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FontFaceStyle {
    Normal,
    Italic,
    /// Inclusive angles in degrees; plain `oblique` is 14..=14.
    Oblique(RangeInclusive<f32>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FontFaceSource {
    /// A decoded local face name, without CSS quotes or escapes.
    Local(String),
    Url {
        /// Decoded URL. Pipeline APIs resolve this using ResourceProvider.
        url: String,
        /// Decoded format hint, e.g. "woff2". Unknown hints are preserved.
        format: Option<String>,
        /// Required technology keywords, e.g. "variations".
        technologies: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontFaceDescriptor {
    pub name: String,
    pub value: String,
}

impl FontFace {
    pub(crate) fn from_rule(rule: &FontFaceRule<'_>) -> Option<Self> {
        let mut family = None;
        let mut sources = None;
        let mut face = Self {
            family: String::new(),
            sources: Vec::new(),
            weight: 400.0..=400.0,
            style: FontFaceStyle::Normal,
            stretch: 100.0..=100.0,
            unicode_ranges: vec![0..=0x10ffff],
            additional_descriptors: Vec::new(),
        };
        // Later valid descriptors replace earlier ones. Parser-recognized but
        // descriptor-invalid values must not replace the last valid value.
        for property in &rule.properties {
            match property {
                FontFaceProperty::FontFamily(value) => {
                    if let FontFamily::FamilyName(_) = value
                        && let Some(name) = decoded_family(value).filter(|name| !name.is_empty())
                    {
                        family = Some(name);
                    }
                }
                FontFaceProperty::Source(values) => {
                    let values = values
                        .iter()
                        .filter_map(|source| match source {
                            Source::Local(name) => decoded_family(name).map(FontFaceSource::Local),
                            Source::Url(source) => Some(FontFaceSource::Url {
                                url: source.url.url.as_ref().to_owned(),
                                format: source.format.as_ref().map(font_format),
                                technologies: source
                                    .tech
                                    .iter()
                                    .filter_map(|tech| {
                                        tech.to_css_string(PrinterOptions::default()).ok()
                                    })
                                    .collect(),
                            }),
                        })
                        .collect::<Vec<_>>();
                    if !values.is_empty() {
                        sources = Some(values);
                    }
                }
                FontFaceProperty::FontWeight(value) => {
                    if let (Some(start), Some(end)) = (weight(&value.0), weight(&value.1))
                        && start <= end
                    {
                        face.weight = start..=end;
                    }
                }
                FontFaceProperty::FontStyle(value) => match value {
                    FontStyle::Normal => face.style = FontFaceStyle::Normal,
                    FontStyle::Italic => face.style = FontFaceStyle::Italic,
                    FontStyle::Oblique(angles) => {
                        let (start, end) = (angles.0.to_degrees(), angles.1.to_degrees());
                        if start.is_finite()
                            && end.is_finite()
                            && start >= -90.0
                            && end <= 90.0
                            && start <= end
                        {
                            face.style = FontFaceStyle::Oblique(start..=end);
                        }
                    }
                },
                FontFaceProperty::FontStretch(value) => {
                    let (start, end) = (stretch(&value.0), stretch(&value.1));
                    if start.is_finite() && end.is_finite() && start >= 0.0 && start <= end {
                        face.stretch = start..=end;
                    }
                }
                FontFaceProperty::UnicodeRange(ranges) => {
                    face.unicode_ranges =
                        ranges.iter().map(|range| range.start..=range.end).collect();
                }
                FontFaceProperty::Custom(custom) => {
                    let name = custom.name.as_ref().to_ascii_lowercase();
                    if matches!(
                        name.as_str(),
                        "font-family"
                            | "src"
                            | "font-weight"
                            | "font-style"
                            | "font-stretch"
                            | "unicode-range"
                    ) {
                        continue;
                    }
                    if let Ok(css) = property.to_css_string(PrinterOptions::default())
                        && let Some((_, value)) = css.split_once(':')
                    {
                        face.additional_descriptors.push(FontFaceDescriptor {
                            name,
                            value: value.trim().to_owned(),
                        });
                    }
                }
            }
        }
        face.family = family?;
        face.sources = sources?;
        Some(face)
    }
}

fn weight(value: &FontWeight) -> Option<f32> {
    let value = match value {
        FontWeight::Absolute(AbsoluteFontWeight::Normal) => 400.0,
        FontWeight::Absolute(AbsoluteFontWeight::Bold) => 700.0,
        FontWeight::Absolute(AbsoluteFontWeight::Weight(value)) => *value,
        FontWeight::Bolder | FontWeight::Lighter => return None,
    };
    (value.is_finite() && (1.0..=1000.0).contains(&value)).then_some(value)
}

fn stretch(value: &FontStretch) -> f32 {
    let percentage: Percentage = value.into();
    percentage.0 * 100.0
}

fn font_format(format: &FontFormat<'_>) -> String {
    match format {
        FontFormat::WOFF => "woff",
        FontFormat::WOFF2 => "woff2",
        FontFormat::TrueType => "truetype",
        FontFormat::OpenType => "opentype",
        FontFormat::EmbeddedOpenType => "embedded-opentype",
        FontFormat::Collection => "collection",
        FontFormat::SVG => "svg",
        FontFormat::String(value) => value.as_ref(),
    }
    .to_owned()
}

fn decoded_family(family: &FontFamily<'_>) -> Option<String> {
    // FamilyName's decoded string is private. Let Lightning CSS tokenize its
    // serialization, then convert the parsed name tokens into host data.
    let css = family.to_css_string(PrinterOptions::default()).ok()?;
    let tokens = TokenList::parse_string_with_options(&css, ParserOptions::default()).ok()?;
    let words: Vec<&str> = tokens
        .0
        .iter()
        .filter(|token| !token.is_whitespace())
        .map(|token| match token {
            TokenOrValue::Token(Token::Ident(value) | Token::String(value)) => Some(value.as_ref()),
            TokenOrValue::DashedIdent(value) => Some(value.0.as_ref()),
            _ => None,
        })
        .collect::<Option<_>>()?;
    (!words.is_empty()).then(|| words.join(" "))
}

/// Resolve only `@font-face` URLs before flattening imported stylesheets.
/// The callback receives decoded relative URLs; absolute URLs (including data URLs)
/// are preserved. Other URL-bearing CSS is untouched.
/// Unchanged sheets borrow the input; changed sheets are serialized as CSS.
/// This function performs no resource reads.
pub fn resolve_font_face_urls<'a>(
    css: &'a str,
    mut resolve: impl FnMut(&str) -> String,
) -> Cow<'a, str> {
    // Includes escaped at-rule names; ordinary declaration-only sheets need no
    // parsing at this boundary.
    if !css.contains('@')
        || (!css
            .as_bytes()
            .windows(10)
            .any(|token| token.eq_ignore_ascii_case(b"@font-face"))
            && !css.contains('\\'))
    {
        return Cow::Borrowed(css);
    }
    let Some(mut sheet) = super::parse_stylesheet(css) else {
        return Cow::Borrowed(css);
    };
    if !resolve_rule_urls(&mut sheet.rules, &mut resolve) {
        return Cow::Borrowed(css);
    }
    match sheet.to_css(PrinterOptions::default()) {
        Ok(result) => Cow::Owned(result.code),
        Err(_) => Cow::Borrowed(css),
    }
}

pub(super) fn resolve_rule_urls(
    rules: &mut CssRuleList<'_>,
    resolve: &mut impl FnMut(&str) -> String,
) -> bool {
    let mut changed = false;
    for rule in &mut rules.0 {
        match rule {
            CssRule::FontFace(face) => {
                for property in &mut face.properties {
                    if let FontFaceProperty::Source(sources) = property {
                        for source in sources {
                            if let Source::Url(source) = source {
                                let original = source.url.url.as_ref();
                                // Resource providers may implement filesystem
                                // paths only; absolute URIs need no rebasing.
                                if has_uri_scheme(original) {
                                    continue;
                                }
                                let url = resolve(original);
                                if url != source.url.url.as_ref() {
                                    source.url.url = url.into();
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
            CssRule::Media(rule) => changed |= resolve_rule_urls(&mut rule.rules, resolve),
            CssRule::Supports(rule) => changed |= resolve_rule_urls(&mut rule.rules, resolve),
            CssRule::LayerBlock(rule) => changed |= resolve_rule_urls(&mut rule.rules, resolve),
            CssRule::Scope(rule) => changed |= resolve_rule_urls(&mut rule.rules, resolve),
            _ => {}
        }
    }
    changed
}
