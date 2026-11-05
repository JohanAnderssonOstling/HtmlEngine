//! Prepare authored font sources using host-provided I/O and local discovery.
//! Hosts register the prepared result in their native text backend; rejecting a
//! source continues through the remaining authored alternatives.
use html::text::FontFace;
use html_style_model::FontFaceSource;
use html_resources::fonts::decode;
use std::future::Future;
use std::pin::Pin;

#[cfg(not(target_arch = "wasm32"))]
pub type FontLoadFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
pub type FontLoadFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + 'a>>;

/// Host-owned font discovery and byte loading, including any I/O or cache policy.
pub trait FontSourceLoader: Sync {
    /// Return the registered family name when a local source is available.
    fn local_family(&self, name: &str) -> Option<String>;
    /// Load encoded font bytes; the preparation service handles decoding.
    fn load<'a>(&'a self, url: &'a str) -> FontLoadFuture<'a>;
}

/// One usable source passed to the host's native preparation callback.
pub enum FontData {
    Local(String),
    Decoded(Vec<u8>),
}

/// Try the next authored source when native preparation rejects font data.
pub(crate) async fn prepare_face<T>(
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
                    .and_then(|bytes| decode(bytes.into()))
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

fn supported_source(format: Option<&str>, technologies: &[String]) -> bool {
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
