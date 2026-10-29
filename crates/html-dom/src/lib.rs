#[path = "document/mod.rs"]
mod document;
mod markup;
mod semantics;
mod source;
mod text;
pub mod storage;

pub use document::*;
pub use markup::{write_escaped_html_attribute, write_escaped_html_text, write_escaped_xml};
pub use semantics::*;
pub use source::{ResourceMetadata, ResourceProvider, has_uri_scheme, media_type_essence, uri_path};
pub use text::collapse_whitespace;
