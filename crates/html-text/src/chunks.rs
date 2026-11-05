//! Allocation-free text spans with independent Unicode boundary context.

/// A borrowed span separated from its neighbors at word and grapheme boundaries.
pub enum TextChunk<'a> {
    /// ASCII text whose byte offsets are also character offsets.
    Ascii(&'a str),
    /// Non-ASCII text together with its surrounding word context.
    Unicode(&'a str),
}

/// Visit coalesced ASCII stretches and Unicode spans without storing a list.
/// ASCII whitespace followed by an ASCII letter/number is always a word and
/// grapheme boundary. Keep non-ASCII text and its neighboring word together
/// until such a boundary, including combining marks, emoji and format controls.
#[inline]
pub fn for_each_text_chunk<'a>(text: &'a str, mut visit: impl FnMut(TextChunk<'a>)) {
    if text.is_empty() {
        return;
    }
    if text.is_ascii() {
        visit(TextChunk::Ascii(text));
        return;
    }
    let mut chunk_start = 0;
    let mut safe_boundary = 0;
    let mut in_unicode = false;
    let mut previous = 0;
    for (index, &byte) in text.as_bytes().iter().enumerate() {
        if matches!(previous, b'\t'..=b'\r' | b' ') && byte.is_ascii_alphanumeric() {
            if in_unicode {
                visit(TextChunk::Unicode(&text[chunk_start..index]));
                chunk_start = index;
                in_unicode = false;
            }
            safe_boundary = index;
        } else if !byte.is_ascii() && !in_unicode {
            if chunk_start < safe_boundary {
                visit(TextChunk::Ascii(&text[chunk_start..safe_boundary]));
            }
            chunk_start = safe_boundary;
            in_unicode = true;
        }
        previous = byte;
    }
    if in_unicode {
        visit(TextChunk::Unicode(&text[chunk_start..]));
    } else {
        visit(TextChunk::Ascii(&text[chunk_start..]));
    }
}
