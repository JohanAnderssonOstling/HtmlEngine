use crate::{ParsedHtml, parse_html_document};
use encoding_rs::{Encoding, UTF_8};

/// Decodes and parses an HTML byte stream before invalid sequences can be
/// replaced by a resource adapter. Transport metadata takes precedence over
/// an in-document declaration.
pub fn parse_html_document_bytes(bytes: &[u8], transport_encoding: Option<&str>) -> ParsedHtml {
    let (source, encoding) = decode_html_bytes_with_encoding(bytes, transport_encoding);
    let mut parsed = parse_html_document(&source);
    parsed.source_encoding = Some(encoding.name().to_owned());
    parsed
}

pub fn decode_html_bytes(bytes: &[u8], transport_encoding: Option<&str>) -> String {
    decode_html_bytes_with_encoding(bytes, transport_encoding)
        .0
        .into_owned()
}

fn decode_html_bytes_with_encoding<'a>(
    bytes: &'a [u8],
    transport_encoding: Option<&str>,
) -> (std::borrow::Cow<'a, str>, &'static Encoding) {
    let (encoding, bom_len) = Encoding::for_bom(bytes)
        .or_else(|| {
            transport_encoding
                .and_then(|label| Encoding::for_label(label.trim().as_bytes()))
                .map(|encoding| (encoding, 0))
        })
        .or_else(|| prescan_html_encoding(bytes).map(|encoding| (encoding, 0)))
        .unwrap_or((UTF_8, 0));
    (
        encoding.decode_without_bom_handling(&bytes[bom_len..]).0,
        encoding,
    )
}

fn prescan_html_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    let mut cursor = 0usize;
    let mut in_noscript = false;
    let mut head_closed = false;
    let mut landed_on_boundary_from_tag = false;
    let end = bytes.len().min(1024);
    while cursor <= end && cursor < bytes.len() {
        if starts_ascii_case_insensitive(bytes, cursor, b"<!--") {
            cursor = find_bytes(bytes, cursor + 4, b"-->").map_or(bytes.len(), |end| end + 3);
            continue;
        }
        if starts_raw_text_element(bytes, cursor, b"style")
            || starts_raw_text_element(bytes, cursor, b"title")
        {
            let name = if starts_raw_text_element(bytes, cursor, b"style") {
                b"style".as_slice()
            } else {
                b"title".as_slice()
            };
            let close = [b"</".as_slice(), name].concat();
            cursor = find_ascii_case_insensitive(
                bytes,
                tag_end(bytes, cursor + 1).saturating_add(1),
                &close,
            )
            .map_or(bytes.len(), |start| {
                tag_end(bytes, start + close.len()).saturating_add(1)
            });
            continue;
        }
        if starts_ascii_case_insensitive(bytes, cursor, b"<meta")
            && bytes
                .get(cursor + 5)
                .is_none_or(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
        {
            if cursor == end && (landed_on_boundary_from_tag || in_noscript || head_closed) {
                break;
            }
            let end = tag_end(bytes, cursor + 5);
            if let Some(label) = meta_charset_label(&bytes[cursor + 5..end]) {
                if let Some(encoding) = Encoding::for_label(label) {
                    return Some(encoding);
                }
                if let Some(decoded) = decode_ascii_numeric_references(label)
                    && let Some(encoding) = Encoding::for_label(&decoded)
                {
                    return Some(encoding);
                }
            }
            cursor = end.saturating_add(1);
            landed_on_boundary_from_tag = false;
            continue;
        }
        if bytes[cursor] == b'<' {
            if starts_ascii_case_insensitive(bytes, cursor, b"<noscript") {
                in_noscript = true;
            } else if starts_ascii_case_insensitive(bytes, cursor, b"</noscript") {
                in_noscript = false;
            }
            if starts_ascii_case_insensitive(bytes, cursor, b"</head") {
                head_closed = true;
            }
            cursor = tag_end(bytes, cursor + 1).saturating_add(1);
            landed_on_boundary_from_tag = cursor == end;
        } else {
            cursor += 1;
            landed_on_boundary_from_tag = false;
        }
    }
    None
}

fn decode_ascii_numeric_references(label: &[u8]) -> Option<Vec<u8>> {
    if !label.contains(&b'&') {
        return None;
    }
    let mut decoded = Vec::with_capacity(label.len());
    let mut cursor = 0usize;
    while cursor < label.len() {
        if label.get(cursor..cursor + 2) == Some(b"&#") {
            let hex = label
                .get(cursor + 2)
                .is_some_and(|byte| matches!(byte, b'x' | b'X'));
            let digits_start = cursor + if hex { 3 } else { 2 };
            let relative_end = label
                .get(digits_start..)?
                .iter()
                .position(|byte| *byte == b';')?;
            let digits_end = digits_start + relative_end;
            let digits = std::str::from_utf8(&label[digits_start..digits_end]).ok()?;
            let value = u32::from_str_radix(digits, if hex { 16 } else { 10 }).ok()?;
            decoded.push(u8::try_from(value).ok()?);
            cursor = digits_end + 1;
        } else {
            decoded.push(label[cursor]);
            cursor += 1;
        }
    }
    Some(decoded)
}

fn starts_raw_text_element(bytes: &[u8], cursor: usize, name: &[u8]) -> bool {
    let start = cursor.saturating_add(1);
    starts_ascii_case_insensitive(bytes, start, name)
        && bytes
            .get(start + name.len())
            .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
}

fn meta_charset_label(attributes: &[u8]) -> Option<&[u8]> {
    let mut cursor = 0usize;
    while cursor < attributes.len() {
        while attributes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'/')
        {
            cursor += 1;
        }
        let name_start = cursor;
        while attributes
            .get(cursor)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'/' | b'>'))
        {
            cursor += 1;
        }
        let name = &attributes[name_start..cursor];
        while attributes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if attributes.get(cursor) != Some(&b'=') {
            while attributes
                .get(cursor)
                .is_some_and(|byte| !byte.is_ascii_whitespace())
            {
                cursor += 1;
            }
            continue;
        }
        cursor += 1;
        while attributes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let (value_start, value_end) = match attributes.get(cursor).copied() {
            Some(quote @ (b'\'' | b'"')) => {
                cursor += 1;
                let start = cursor;
                while attributes.get(cursor).is_some_and(|byte| *byte != quote) {
                    cursor += 1;
                }
                (start, cursor)
            }
            Some(_) => {
                let start = cursor;
                while attributes
                    .get(cursor)
                    .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'/' | b'>'))
                {
                    cursor += 1;
                }
                (start, cursor)
            }
            None => return None,
        };
        if name.eq_ignore_ascii_case(b"charset") {
            return Some(&attributes[value_start..value_end]);
        }
        cursor = cursor.saturating_add(1);
    }
    None
}

fn starts_ascii_case_insensitive(bytes: &[u8], start: usize, needle: &[u8]) -> bool {
    bytes
        .get(start..start.saturating_add(needle.len()))
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle))
}

fn find_bytes(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(start..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn find_ascii_case_insensitive(bytes: &[u8], start: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(start..)?
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
        .map(|offset| start + offset)
}

fn tag_end(bytes: &[u8], start: usize) -> usize {
    let mut cursor = start;
    let mut quote = None;
    while cursor < bytes.len() {
        match (quote, bytes[cursor]) {
            (Some(expected), byte) if byte == expected => quote = None,
            (None, byte @ (b'\'' | b'"')) => quote = Some(byte),
            (None, b'>') => return cursor,
            _ => {}
        }
        cursor += 1;
    }
    bytes.len()
}
