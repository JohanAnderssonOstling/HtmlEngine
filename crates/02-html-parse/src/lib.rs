mod dom;
mod encoding;
mod metadata;
mod parser;
mod text;
mod xml;

pub use dom::{build_dom_document, parse_dom_document};
pub use encoding::{
    decode_html_bytes, encoding_for_label, parse_html_document_bytes, read_document_source,
};
pub use metadata::{document_stylesheets, style_type_is_css};
pub use parser::*;
pub use text::{DocumentTextIndex, SourceTextPosition, plain_text_from_fragment};
pub use xml::{XmlParseError, parse_xml_document};
