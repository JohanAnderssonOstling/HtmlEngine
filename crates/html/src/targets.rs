//! Document targets and annotation anchors, queried through
//! [`crate::layout::RenderAddressingView`]. Navigation and publication identity
//! belong to the embedding application.

use crate::resources::ResourceProvider;

pub use html_pipeline::engine::{
    DocumentTarget, ElementTarget, LinkTarget, ResolvedDestination, TextQuote,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLink {
    pub resource: String,
    pub fragment: Option<String>,
}

pub fn resolve_link(provider: &dyn ResourceProvider, base: &str, href: &str) -> ResolvedLink {
    let base = base.split_once('#').map_or(base, |(resource, _)| resource);
    let (path, fragment) = href.split_once('#').unwrap_or((href, ""));
    ResolvedLink {
        resource: if path.is_empty() {
            base.to_owned()
        } else {
            provider.resolve(base, path)
        },
        fragment: (!fragment.is_empty()).then(|| decode_fragment(fragment)),
    }
}

fn decode_fragment(fragment: &str) -> String {
    if !fragment.contains('%') {
        return fragment.to_owned();
    }
    let mut decoded = Vec::with_capacity(fragment.len());
    let mut bytes = fragment.as_bytes();
    while let Some((&first, rest)) = bytes.split_first() {
        if first == b'%'
            && bytes.len() >= 3
            && let Some(value) = char::from(bytes[1])
                .to_digit(16)
                .zip(char::from(bytes[2]).to_digit(16))
                .map(|(high, low)| (high * 16 + low) as u8)
        {
            decoded.push(value);
            bytes = &bytes[3..];
        } else {
            decoded.push(first);
            bytes = rest;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

impl crate::Document {
    /// Resolve a link through the document's resource provider. Whether to
    /// navigate, open a popup, or launch another application is a host decision.
    pub fn resolve_link(&self, href: &str) -> Option<ResolvedLink> {
        let base = self
            .document()?
            .document()
            .resource_base_uri()
            .unwrap_or(&self.inputs()?.base_uri);
        Some(resolve_link(self.provider.as_ref(), base, href))
    }
}
