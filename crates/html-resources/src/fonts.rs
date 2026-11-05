//! Shared font-byte decoding for engine preparation and integration caches.

use std::borrow::Cow;

const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

pub fn decode(bytes: Cow<'_, [u8]>) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_FONT_BYTES {
        return Err("font exceeds the size limit".into());
    }
    let signature = bytes.get(..4).ok_or("truncated font")?;
    let decoded = match signature {
        b"wOFF" | b"wOF2" => {
            let size = bytes
                .get(16..20)
                .and_then(|value| <[u8; 4]>::try_from(value).ok())
                .map(u32::from_be_bytes)
                .ok_or("truncated web font header")?;
            if size as usize > MAX_FONT_BYTES {
                return Err("decoded font exceeds the size limit".into());
            }
            if signature == b"wOFF" {
                wuff::decompress_woff1(&bytes)
            } else {
                wuff::decompress_woff2(&bytes)
            }
            .map_err(|error| format!("web font decoding failed: {error:?}"))?
        }
        _ => bytes.into_owned(),
    };
    if decoded.len() > MAX_FONT_BYTES {
        return Err("decoded font exceeds the size limit".into());
    }
    ttf_parser::Face::parse(&decoded, 0).map_err(|error| format!("invalid font: {error:?}"))?;
    Ok(decoded)
}
