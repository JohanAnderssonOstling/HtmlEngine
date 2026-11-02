//! Optional font-data preparation. Hosts supply I/O and local-font discovery;
//! native registration, private family rewriting and cache policy stay outside.
use crate::style::{FontFace, FontFaceSource};
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

pub const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;

#[cfg(not(target_arch = "wasm32"))]
pub type FontLoadFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
pub type FontLoadFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + 'a>>;

pub trait FontSourceLoader: Sync {
    fn local_family(&self, name: &str) -> Option<String>;
    fn load<'a>(&'a self, url: &'a str) -> FontLoadFuture<'a>;
}

pub enum FontData {
    Local(String),
    Decoded(Vec<u8>),
}

/// Resolve in authored order, skipping unsupported formats/technologies and
/// falling back after invalid data or I/O failures. Data URLs need no host I/O.
pub async fn resolve_face(
    face: &FontFace,
    loader: &impl FontSourceLoader,
) -> Result<FontData, String> {
    prepare_face(face, loader, Ok).await
}

/// Preserve authored fallback when a native backend rejects otherwise valid data.
/// The engine does not interpret the backend's prepared output.
pub async fn prepare_face<T>(
    face: &FontFace,
    loader: &impl FontSourceLoader,
    mut prepare: impl FnMut(FontData) -> Result<T, String>,
) -> Result<T, String> {
    let mut errors = Vec::new();
    for source in &face.sources {
        match source {
            FontFaceSource::Local(name) => {
                if let Some(family) = loader.local_family(name) {
                    match prepare(FontData::Local(family)) {
                        Ok(font) => return Ok(font),
                        Err(error) => errors.push(format!("{name}: {error}")),
                    }
                }
            }
            FontFaceSource::Url {
                url,
                format,
                technologies,
            } => {
                if !supported_source(format.as_deref(), technologies) {
                    continue;
                }
                let resource = url.split('#').next().unwrap_or(url);
                let bytes = if resource.starts_with("data:") {
                    data_url::DataUrl::process(resource)
                        .map_err(|error| format!("invalid data URL: {error:?}"))
                        .and_then(|data| {
                            data.decode_to_vec()
                                .map(|(bytes, _)| bytes)
                                .map_err(|error| format!("invalid font data: {error:?}"))
                        })
                } else {
                    loader.load(resource).await
                };
                match bytes
                    .and_then(|bytes| decode_bytes(bytes.into()))
                    .and_then(|bytes| prepare(FontData::Decoded(bytes)))
                {
                    Ok(font) => return Ok(font),
                    Err(error) => errors.push(format!("{url}: {error}")),
                }
            }
        }
    }
    Err(format!(
        "no usable source for font {}: {}",
        face.family,
        errors.join("; ")
    ))
}

pub fn supported_source(format: Option<&str>, technologies: &[String]) -> bool {
    format.is_none_or(|format| {
        [
            "woff",
            "woff2",
            "truetype",
            "opentype",
            "collection",
            "woff-variations",
            "woff2-variations",
            "truetype-variations",
            "opentype-variations",
        ]
        .iter()
        .any(|supported| format.eq_ignore_ascii_case(supported))
    }) && technologies
        .iter()
        .all(|technology| matches!(technology.as_str(), "features-opentype" | "variations"))
}

/// Decode and validate SFNT/WOFF/WOFF2 with a bound on both encoded and decoded size.
pub fn decode(bytes: &[u8]) -> Result<Vec<u8>, String> {
    decode_bytes(bytes.into())
}

fn decode_bytes(bytes: Cow<'_, [u8]>) -> Result<Vec<u8>, String> {
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
