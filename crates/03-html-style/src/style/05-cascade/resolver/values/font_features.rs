use super::*;

pub(in crate::style::cascade::resolver) fn feature(tag: &[u8; 4], value: u32) -> OpenTypeFeature {
    OpenTypeFeature::new(*tag, value)
}

pub(in crate::style::cascade::resolver) fn font_variant_caps_features(
    value: &lightningcss::properties::font::FontVariantCaps,
) -> Vec<OpenTypeFeature> {
    use lightningcss::properties::font::FontVariantCaps;
    match value {
        FontVariantCaps::Normal => Vec::new(),
        FontVariantCaps::SmallCaps => vec![feature(b"smcp", 1)],
        FontVariantCaps::AllSmallCaps => vec![feature(b"smcp", 1), feature(b"c2sc", 1)],
        FontVariantCaps::PetiteCaps => vec![feature(b"pcap", 1)],
        FontVariantCaps::AllPetiteCaps => vec![feature(b"pcap", 1), feature(b"c2pc", 1)],
        FontVariantCaps::Unicase => vec![feature(b"unic", 1)],
        FontVariantCaps::TitlingCaps => vec![feature(b"titl", 1)],
    }
}

pub(crate) fn parse_font_kerning(css: &str) -> Option<Vec<OpenTypeFeature>> {
    match css.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(Vec::new()),
        "normal" => Some(vec![feature(b"kern", 1)]),
        "none" => Some(vec![feature(b"kern", 0)]),
        _ => None,
    }
}

pub(in crate::style::cascade::resolver) fn parse_font_variant_numeric(
    css: &str,
) -> Option<Vec<OpenTypeFeature>> {
    let css = css.trim().to_ascii_lowercase();
    if css == "normal" {
        return Some(Vec::new());
    }
    let mut features = Vec::new();
    let mut figure_style = false;
    let mut spacing_style = false;
    let mut fraction_style = false;
    for keyword in css.split_ascii_whitespace() {
        let (tag, category) = match keyword {
            "lining-nums" => (b"lnum", Some(&mut figure_style)),
            "oldstyle-nums" => (b"onum", Some(&mut figure_style)),
            "proportional-nums" => (b"pnum", Some(&mut spacing_style)),
            "tabular-nums" => (b"tnum", Some(&mut spacing_style)),
            "diagonal-fractions" => (b"frac", Some(&mut fraction_style)),
            "stacked-fractions" => (b"afrc", Some(&mut fraction_style)),
            "ordinal" => (b"ordn", None),
            "slashed-zero" => (b"zero", None),
            _ => return None,
        };
        if let Some(category) = category {
            if *category {
                return None;
            }
            *category = true;
        }
        if features
            .iter()
            .any(|existing: &OpenTypeFeature| existing.tag() == *tag)
        {
            return None;
        }
        features.push(feature(tag, 1));
    }
    (!features.is_empty()).then_some(features)
}

pub(in crate::style::cascade::resolver) fn parse_font_variant_ligatures(
    css: &str,
) -> Option<Vec<OpenTypeFeature>> {
    let css = css.trim().to_ascii_lowercase();
    if css == "normal" {
        return Some(Vec::new());
    }
    if css == "none" {
        return Some(
            [b"liga", b"clig", b"dlig", b"hlig", b"calt"]
                .into_iter()
                .map(|tag| feature(tag, 0))
                .collect(),
        );
    }
    let mut features = Vec::new();
    for keyword in css.split_ascii_whitespace() {
        let (tags, value): (&[[u8; 4]], u32) = match keyword {
            "common-ligatures" => (&[*b"liga", *b"clig"], 1),
            "no-common-ligatures" => (&[*b"liga", *b"clig"], 0),
            "discretionary-ligatures" => (&[*b"dlig"], 1),
            "no-discretionary-ligatures" => (&[*b"dlig"], 0),
            "historical-ligatures" => (&[*b"hlig"], 1),
            "no-historical-ligatures" => (&[*b"hlig"], 0),
            "contextual" => (&[*b"calt"], 1),
            "no-contextual" => (&[*b"calt"], 0),
            _ => return None,
        };
        for tag in tags {
            if features
                .iter()
                .any(|existing: &OpenTypeFeature| existing.tag() == *tag)
            {
                return None;
            }
            features.push(OpenTypeFeature::new(*tag, value));
        }
    }
    (!features.is_empty()).then_some(features)
}

pub(in crate::style::cascade::resolver) fn parse_font_feature_settings(
    css: &str,
) -> Option<Vec<OpenTypeFeature>> {
    let css = css.trim();
    if css.eq_ignore_ascii_case("normal") {
        return Some(Vec::new());
    }
    let mut features = Vec::new();
    for item in css.split(',') {
        let item = item.trim();
        let quote = item.as_bytes().first().copied()?;
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        let end = item.as_bytes()[1..]
            .iter()
            .position(|byte| *byte == quote)?
            + 1;
        let tag_text = &item[1..end];
        if tag_text.len() != 4 || !tag_text.is_ascii() {
            return None;
        }
        let mut tag = [0; 4];
        tag.copy_from_slice(tag_text.as_bytes());
        let remainder = item[end + 1..].trim();
        let value = if remainder.is_empty() || remainder.eq_ignore_ascii_case("on") {
            1
        } else if remainder.eq_ignore_ascii_case("off") {
            0
        } else {
            remainder.parse::<u32>().ok()?
        };
        let parsed = OpenTypeFeature::new(tag, value);
        if let Some(index) = features
            .iter()
            .position(|existing: &OpenTypeFeature| existing.tag() == tag)
        {
            features[index] = parsed;
        } else {
            features.push(parsed);
        }
    }
    (!features.is_empty()).then_some(features)
}
