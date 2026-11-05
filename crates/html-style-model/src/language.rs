use std::num::NonZeroU32;

pub use language_tags::LanguageTag;

/// A language declaration resolved for HTML text. Well-formed BCP 47 tags use
/// the external parser's structured representation. HTML also requires unknown
/// declarations to retain their identity and original spelling for consumers.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ContentLanguage {
    Tag(LanguageTag),
    Unrecognized(String),
}

impl ContentLanguage {
    pub(crate) fn parse(value: &str) -> Self {
        match LanguageTag::parse(value) {
            Ok(tag) => Self::Tag(tag.canonicalize().unwrap_or(tag)),
            Err(_) => Self::Unrecognized(value.to_owned()),
        }
    }

}

/// Opaque reference into one computed-style result's structured language table.
/// Inherited styles copy this handle without cloning or reparsing a tag.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LanguageId {
    pub(crate) store_id: NonZeroU32,
    pub(crate) index: u32,
}

pub fn content_language_tag(value: &ContentLanguage) -> Option<&LanguageTag> {
    match value {
        ContentLanguage::Tag(tag) => Some(tag),
        ContentLanguage::Unrecognized(_) => None,
    }
}

pub fn content_language_as_str(value: &ContentLanguage) -> &str {
    match value {
        ContentLanguage::Tag(tag) => tag.as_str(),
        ContentLanguage::Unrecognized(value) => value,
    }
}
