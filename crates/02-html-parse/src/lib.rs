mod dom;
mod encoding;
mod metadata;
mod parser;
mod text;
mod xml;

pub use dom::{build_dom_document, parse_dom_document};
pub use encoding::{decode_html_bytes, parse_html_document_bytes};
pub use parser::*;
pub use text::{DocumentTextIndex, SourceTextPosition, plain_text_from_fragment};
pub use xml::{XmlParseError, parse_xml_document};
