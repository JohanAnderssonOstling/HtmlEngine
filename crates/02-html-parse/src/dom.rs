use crate::ParsedHtml;
use html_dom::{
    Document, DocumentBuildError, DocumentBuilder, DomAttribute, DomElementAttributeCache,
    DomElementData, ImageResource, ImageSource};
use std::borrow::Cow;
use std::sync::Arc;

/// Parser-independent output for normal HTML rendering.
/// Title metadata is stored on the document; source encoding is kept for CSS decoding.
pub struct ParsedDomDocument {
    pub(crate) document: Document,
    pub(crate) source_encoding: Option<String>,
}

impl ParsedDomDocument {
    pub fn into_parts(self) -> (Document, Option<String>) {
        (self.document, self.source_encoding)
    }
}

/// Parse HTML directly into the engine DOM without constructing an inspection tree.
pub fn parse_dom_document(html: &str) -> Result<Document, DocumentBuildError> {
    parse_dom_document_with_metadata(html).map(|parsed| parsed.document)
}

/// Parse HTML into the engine DOM and the metadata needed during preparation.
pub fn parse_dom_document_with_metadata(
    html: &str,
) -> Result<ParsedDomDocument, DocumentBuildError> {
    crate::html::parse_dom_document(html)
}

/// Clone an already parsed engine DOM for callers that need another owned copy.
pub fn build_dom_document(parsed: &ParsedHtml) -> Result<Document, DocumentBuildError> {
    parsed.document.clone()
}

/// Borrowed attribute data at the parser/DOM boundary. No tree is copied.
pub(crate) struct SourceAttribute<'a> {
    pub name: Cow<'a, str>,
    pub local_name: &'a str,
    pub namespace: Option<&'a str>,
    pub value: &'a str,
}

pub(crate) fn build_element_data<'a>(
    document: &mut DocumentBuilder,
    tag_name: &str,
    namespace_name: Option<&str>,
    source_attributes: impl ExactSizeIterator<Item = SourceAttribute<'a>>,
    foreign_markup: impl FnOnce() -> String,
) -> Result<DomElementData, DocumentBuildError> {
    let tag = document.try_intern_string(tag_name)?;
    let namespace = namespace_name
        .map(|name| document.try_intern_string(name))
        .transpose()?;
    let mut id = None;
    let mut classes = 0..0;
    let (mut src, mut href, mut xlink_href) = (None, None, None);
    let (mut width, mut height) = (None, None);
    let mut attributes = Vec::with_capacity(source_attributes.len());
    for attribute in source_attributes {
        let name = document.try_intern_string(&attribute.name)?;
        let local_name = if attribute.name.as_ref() == attribute.local_name {
            name
        } else {
            document.try_intern_string(attribute.local_name)?
        };
        let namespace = attribute
            .namespace
            .map(|name| document.try_intern_string(name))
            .transpose()?;
        let value = document.try_intern_string(attribute.value)?;
        match attribute.name.as_ref() {
            "id" => id = Some(value),
            "class" => classes = document.try_intern_classes(Some(attribute.value))?,
            "src" => src = Some(attribute.value),
            "href" => href = Some(attribute.value),
            "xlink:href" => xlink_href = Some(attribute.value),
            "width" => width = attribute.value.parse().ok(),
            "height" => height = attribute.value.parse().ok(),
            _ => {}
        }
        attributes.push(DomAttribute {
            name,
            local_name,
            namespace,
            value,
        });
    }
    let is_mathml =
        tag_name == "math" && namespace_name == Some("http://www.w3.org/1998/Math/MathML");
    let is_inline_svg = tag_name == "svg" && namespace_name == Some("http://www.w3.org/2000/svg");
    let image_source = if is_mathml {
        Some(ImageSource::MathMl {
            markup: foreign_markup().into(),
            svg: Arc::from([]),
            baseline: 1.0,
        })
    } else if is_inline_svg {
        Some(ImageSource::InlineSvg {
            bytes: foreign_markup().into_bytes().into(),
            base_uri: String::new(),
        })
    } else if matches!(tag_name, "img" | "image") {
        src.or(href)
            .or(xlink_href)
            .map(str::trim)
            .filter(|source| !source.is_empty())
            .map(|source| ImageSource::Uri(source.to_owned()))
    } else {
        None
    };
    let image_idx = image_source.map(|source| {
        let (width_attr, height_attr) = if is_mathml || is_inline_svg {
            (None, None)
        } else {
            (width, height)
        };
        document.push_image(ImageResource {
            source,
            width: 0,
            height: 0,
            width_attr,
            height_attr,
        })
    });
    Ok(DomElementData {
        tag,
        namespace,
        attributes,
        attribute_cache: DomElementAttributeCache::new(id, classes),
        image_idx,
    })
}
