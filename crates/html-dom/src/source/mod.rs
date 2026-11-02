use std::{future::Future, io, pin::Pin};

mod classification;
pub use classification::{has_uri_scheme, media_type_essence, uri_path};

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

    /// Read UTF-8 text, replacing malformed byte sequences.
    fn read_string(&self, uri: &str) -> io::Result<String> {
        let bytes = self.read_bytes(uri)?;
        Ok(String::from_utf8(bytes)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned()))
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

#[cfg(test)]
mod tests {
    use super::ResourceProvider;
    use std::{io, sync::Mutex};

    struct SingleReadProvider(Mutex<Option<Vec<u8>>>);

    impl ResourceProvider for SingleReadProvider {
        fn read_bytes(&self, _: &str) -> io::Result<Vec<u8>> {
            self.0
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "resource already read"))
        }

        fn exists(&self, _: &str) -> bool {
            unreachable!()
        }

        fn resolve(&self, _: &str, _: &str) -> String {
            unreachable!()
        }

        fn list_html_candidates(&self, _: &str) -> io::Result<Vec<String>> {
            unreachable!()
        }
    }

    #[test]
    fn string_reads_reuse_valid_utf8_buffers_and_propagate_read_errors() {
        for source in ["", "ASCII", "café 🙂"] {
            let mut bytes = Vec::with_capacity(source.len() + 64);
            bytes.extend_from_slice(source.as_bytes());
            let pointer = bytes.as_ptr();
            let capacity = bytes.capacity();
            let provider = SingleReadProvider(Mutex::new(Some(bytes)));
            let text = provider.read_string("resource").unwrap();
            assert_eq!(text, source);
            assert_eq!(text.as_ptr(), pointer);
            assert_eq!(text.capacity(), capacity);
            assert_eq!(
                provider.read_string("resource").unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
        }
    }

    #[test]
    fn string_reads_keep_lossy_replacement_rules_for_malformed_utf8() {
        let cases: &[(&[u8], &str)] = &[
            (b"caf\xe9", "caf�"),
            (b"a\xf0\x9f", "a�"),
            (b"\xff\xff", "��"),
        ];
        for &(bytes, expected) in cases {
            let provider = SingleReadProvider(Mutex::new(Some(bytes.to_vec())));
            assert_eq!(provider.read_string("resource").unwrap(), expected);
        }
    }
}
