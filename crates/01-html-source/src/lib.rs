use std::{future::Future, io, pin::Pin};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceMetadata {
    /// Internet media type without requiring providers to parse it for the
    /// pipeline. Parameters may be present and are handled by consumers.
    pub media_type: Option<String>,
    /// Transport- or container-declared character encoding, when available.
    pub charset: Option<String>,
}

/// Supplies documents and related resources to the HTML pipeline.
///
/// Implementations may read from a filesystem, an EPUB archive, a network
/// cache, or any other backing store.
pub trait ResourceProvider: Send + Sync {
    fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>>;

    /// Read a resource on demand, including resources outside a prepared text
    /// snapshot. Synchronous pipeline reads keep using [`Self::read_bytes`].
    fn read_bytes_async<'a>(&'a self, uri: &'a str) -> Pin<Box<dyn Future<Output = io::Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move { self.read_bytes(uri) })
    }

    fn read_string(&self, uri: &str) -> io::Result<String> {
        let bytes = self.read_bytes(uri)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn metadata(&self, _uri: &str) -> io::Result<ResourceMetadata> {
        Ok(ResourceMetadata::default())
    }

    /// Primary publication language in metadata order. Used only as a
    /// hyphenation fallback when the content has no inherited language tag.
    fn primary_language(&self) -> Option<String> {
        None
    }

    fn exists(&self, uri: &str) -> bool;

    fn resolve(&self, base: &str, href: &str) -> String;

    fn list_html_candidates(&self, root: &str) -> io::Result<Vec<String>>;

}
