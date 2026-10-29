use crate::ElementRef;

const EPUB_NAMESPACE: &str = "http://www.idpf.org/2007/ops";

impl<'a> ElementRef<'a> {
    /// Borrow EPUB type tokens, accepting either the literal HTML attribute or
    /// an XML attribute bound to the EPUB namespace. Token case is preserved.
    pub fn epub_types(self) -> impl Iterator<Item = &'a str> + Clone + 'a {
        self.attr("epub:type")
            .or_else(|| self.attr_expanded(Some(EPUB_NAMESPACE), "type"))
            .into_iter()
            .flat_map(str::split_ascii_whitespace)
    }

    /// Borrow ASCII-whitespace-separated role tokens without normalizing case.
    pub fn aria_roles(self) -> impl Iterator<Item = &'a str> + Clone + 'a {
        self.attr("role")
            .or_else(|| self.attr_expanded(None, "role"))
            .into_iter()
            .flat_map(str::split_ascii_whitespace)
    }
}

pub fn element_is_note_reference(element: ElementRef<'_>) -> bool {
    element
        .epub_types()
        .any(|token| token.eq_ignore_ascii_case("noteref"))
        || element
            .aria_roles()
            .any(|token| token.eq_ignore_ascii_case("doc-noteref"))
}

pub fn element_is_note_target(element: ElementRef<'_>) -> bool {
    element.epub_types().any(|token| {
        ["footnote", "endnote", "rearnote"]
            .iter()
            .any(|kind| token.eq_ignore_ascii_case(kind))
    }) || element.aria_roles().any(|token| {
        ["doc-footnote", "doc-endnote"]
            .iter()
            .any(|role| token.eq_ignore_ascii_case(role))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocumentBuilder, DomAttribute, DomElementAttributeCache, DomElementData};

    fn element(attribute: Option<(&str, &str)>) -> crate::Document {
        let mut builder = DocumentBuilder::new();
        let aside = builder.intern_string("aside");
        let attributes = attribute
            .map(|(name, value)| DomAttribute {
                name: builder.intern_string(name),
                local_name: builder.intern_string(name),
                namespace: None,
                value: builder.intern_string(value),
            })
            .into_iter()
            .collect();
        let (root, builder) = builder
            .append_dom_element(
                None,
                DomElementData {
                    tag: aside,
                    namespace: None,
                    attributes,
                    attribute_cache: DomElementAttributeCache::new(None, 0..0),
                    image_idx: None,
                },
            )
            .expect("root append");
        builder
            .set_dom_root(root)
            .expect("root set")
            .finish()
            .expect("document build")
    }

    #[test]
    fn note_target_semantics_cover_epub_types_and_aria_roles() {
        let epub = element(Some(("epub:type", "footnote")));
        let role = element(Some(("role", "doc-endnote")));
        let ordinary = element(None);

        assert!(epub.node_ids().filter_map(|node| epub.element_ref(node)).any(element_is_note_target));
        assert!(role.node_ids().filter_map(|node| role.element_ref(node)).any(element_is_note_target));
        assert!(!ordinary.node_ids().filter_map(|node| ordinary.element_ref(node)).any(element_is_note_target));
    }
}
