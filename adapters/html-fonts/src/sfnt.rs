use crate::{LoadedFontFace, PRIVATE_FAMILY_PREFIX};
use html::style::FontFace;
use html::text::FontSlant;
use std::collections::BTreeMap;

pub(crate) fn prepare(bytes: &[u8], declaration: &FontFace) -> Result<LoadedFontFace, String> {
    let face =
        ttf_parser::Face::parse(bytes, 0).map_err(|error| format!("invalid font: {error:?}"))?;
    let weight = face.weight().to_number();
    let slant = if face.is_italic() {
        FontSlant::Italic
    } else if face.is_oblique() {
        FontSlant::Oblique
    } else {
        FontSlant::Normal
    };
    let mut hasher = blake3::Hasher::new();
    hasher.update(bytes);
    for range in &declaration.unicode_ranges {
        hasher.update(&range.start().to_be_bytes());
        hasher.update(&range.end().to_be_bytes());
    }
    let family = format!(
        "{PRIVATE_FAMILY_PREFIX}{}",
        &hasher.finalize().to_hex()[..32]
    );
    let subfamily = match (weight >= 600, slant) {
        (true, FontSlant::Italic | FontSlant::Oblique) => "Bold Italic",
        (true, _) => "Bold",
        (false, FontSlant::Italic | FontSlant::Oblique) => "Italic",
        _ => "Regular",
    };
    let mut tables = BTreeMap::new();
    for record in face.raw_face().table_records {
        if record.tag.to_bytes() == *b"DSIG" {
            continue;
        }
        let start = record.offset as usize;
        let end = start
            .checked_add(record.length as usize)
            .ok_or("invalid table length")?;
        tables.insert(
            record.tag.to_bytes(),
            bytes
                .get(start..end)
                .ok_or("truncated font table")?
                .to_vec(),
        );
    }
    tables.insert(*b"name", name_table(&family, subfamily));
    // Restrict the font's actual cmap so native fallback respects CSS subsets.
    // The full-range default keeps variation-selector mappings unchanged.
    if declaration.unicode_ranges != [0..=0x10ffff] {
        tables.insert(*b"cmap", subset_cmap(&face, declaration));
    }
    let signature = if tables.contains_key(b"CFF ") || tables.contains_key(b"CFF2") {
        *b"OTTO"
    } else {
        [0, 1, 0, 0]
    };
    let rewritten = build_sfnt(signature, tables)?;
    ttf_parser::Face::parse(&rewritten, 0)
        .map_err(|error| format!("font rewriting failed: {error:?}"))?;
    Ok(LoadedFontFace {
        declaration: declaration.clone(),
        family,
        bytes: Some(rewritten.into()),
        weight,
        slant,
        variable: face.is_variable(),
    })
}

fn name_table(family: &str, subfamily: &str) -> Vec<u8> {
    let records = [
        (1, family),
        (2, subfamily),
        (3, family),
        (4, family),
        (6, family),
        (16, family),
        (17, subfamily),
    ];
    let mut table = Vec::new();
    put_u16(&mut table, 0);
    put_u16(&mut table, records.len() as u16);
    put_u16(&mut table, (6 + records.len() * 12) as u16);
    let mut strings = Vec::new();
    for (id, value) in records {
        let encoded = value
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>();
        for value in [3, 1, 0x409, id, encoded.len() as u16, strings.len() as u16] {
            put_u16(&mut table, value);
        }
        strings.extend(encoded);
    }
    table.extend(strings);
    table
}

fn subset_cmap(face: &ttf_parser::Face<'_>, declaration: &FontFace) -> Vec<u8> {
    let mut mapping = BTreeMap::new();
    if let Some(cmap) = face.tables().cmap {
        for subtable in cmap.subtables {
            if !subtable.is_unicode() {
                continue;
            }
            subtable.codepoints(|codepoint| {
                if declaration
                    .unicode_ranges
                    .iter()
                    .any(|range| range.contains(&codepoint))
                    && let Some(glyph) = subtable.glyph_index(codepoint)
                {
                    mapping.entry(codepoint).or_insert(glyph.0 as u32);
                }
            });
        }
    }
    let mut groups: Vec<(u32, u32, u32)> = Vec::new();
    for (codepoint, glyph) in mapping {
        if let Some((start, end, first)) = groups.last_mut()
            && codepoint == *end + 1
            && glyph == *first + codepoint - *start
        {
            *end = codepoint;
        } else {
            groups.push((codepoint, codepoint, glyph));
        }
    }
    let mut table = Vec::new();
    // A Unicode-platform and a Windows UCS-4 record share the format-12 table.
    for value in [0, 2, 0, 4] {
        put_u16(&mut table, value);
    }
    put_u32(&mut table, 20);
    for value in [3, 10] {
        put_u16(&mut table, value);
    }
    put_u32(&mut table, 20);
    for value in [12, 0] {
        put_u16(&mut table, value);
    }
    for value in [(16 + groups.len() * 12) as u32, 0, groups.len() as u32] {
        put_u32(&mut table, value);
    }
    for (start, end, glyph) in groups {
        for value in [start, end, glyph] {
            put_u32(&mut table, value);
        }
    }
    table
}

fn build_sfnt(
    signature: [u8; 4],
    mut tables: BTreeMap<[u8; 4], Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let head = tables
        .get_mut(b"head")
        .ok_or("font is missing its head table")?;
    head.get_mut(8..12).ok_or("truncated head table")?.fill(0);
    let count = u16::try_from(tables.len()).map_err(|_| "too many font tables")?;
    if count == 0 || count > 4095 {
        return Err("invalid font table count".into());
    }
    let power = 1u16 << (15 - count.leading_zeros());
    let mut output = signature.to_vec();
    for value in [
        count,
        power * 16,
        power.ilog2() as u16,
        count * 16 - power * 16,
    ] {
        put_u16(&mut output, value);
    }
    output.resize(12 + usize::from(count) * 16, 0);
    let mut head_offset = 0;
    for (index, (tag, table)) in tables.into_iter().enumerate() {
        let offset = output.len();
        if tag == *b"head" {
            head_offset = offset;
        }
        let mut record = tag.to_vec();
        for value in [checksum(&table), offset as u32, table.len() as u32] {
            put_u32(&mut record, value);
        }
        output[12 + index * 16..28 + index * 16].copy_from_slice(&record);
        output.extend(table);
        output.resize(output.len().next_multiple_of(4), 0);
    }
    let adjustment = 0xb1b0afbau32.wrapping_sub(checksum(&output));
    output[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    Ok(output)
}

fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

fn put_u16(output: &mut Vec<u8>, value: u16) {
    output.extend(value.to_be_bytes());
}
fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend(value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use html::style::FontFaceStyle;

    fn declaration() -> FontFace {
        FontFace {
            family: "Publisher alias".into(),
            sources: vec![],
            weight: 400.0..=400.0,
            style: FontFaceStyle::Normal,
            stretch: 100.0..=100.0,
            unicode_ranges: vec![0..=0x10ffff],
            additional_descriptors: vec![],
        }
    }

    #[test]
    fn fonts_get_private_names_valid_checksums_and_preserved_glyphs() {
        let original = include_bytes!("../tests/fixtures/NV_Garamond-Regular.ttf");
        let loaded = prepare(original, &declaration()).unwrap();
        let bytes = loaded.bytes.unwrap();
        assert_eq!(checksum(&bytes), 0xb1b0afba);
        let face = ttf_parser::Face::parse(&bytes, 0).unwrap();
        let original = ttf_parser::Face::parse(original, 0).unwrap();
        assert_eq!(face.glyph_index('A'), original.glyph_index('A'));
        assert!(
            face.names()
                .into_iter()
                .filter(|name| name.name_id == 1 || name.name_id == 16)
                .all(|name| name.to_string().as_deref() == Some(loaded.family.as_str()))
        );
    }

    #[test]
    fn unicode_subsets_have_distinct_identities_and_native_coverage() {
        let original = include_bytes!("../tests/fixtures/NV_Garamond-Regular.ttf");
        let complete = prepare(original, &declaration()).unwrap();
        let mut subset = declaration();
        subset.unicode_ranges = vec![u32::from('A')..=u32::from('Z')];
        let loaded = prepare(original, &subset).unwrap();
        assert_ne!(loaded.family, complete.family);
        let face = ttf_parser::Face::parse(loaded.bytes.as_ref().unwrap(), 0).unwrap();
        assert!(face.glyph_index('A').is_some());
        assert!(face.glyph_index('a').is_none());
    }
}
