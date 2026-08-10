use crate::ElementRef;

const EPUB_NAMESPACE: &str = "http://www.idpf.org/2007/ops";

fn element_has_token(
    element: ElementRef<'_>,
    attribute: &str,
    namespace: Option<&str>,
    local_name: &str,
    expected: &str,
) -> bool {
    element
        .attr(attribute)
        .or_else(|| element.attr_expanded(namespace, local_name))
        .is_some_and(|value| {
            value
                .split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case(expected))
        })
}

pub fn element_is_note_reference(element: ElementRef<'_>) -> bool {
    element_has_token(
        element,
        "epub:type",
        Some(EPUB_NAMESPACE),
        "type",
        "noteref",
    ) || element_has_token(element, "role", None, "role", "doc-noteref")
}

pub fn element_is_note_target(element: ElementRef<'_>) -> bool {
    ["footnote", "endnote", "rearnote"].iter().any(|kind| {
        element_has_token(
            element,
            "epub:type",
            Some(EPUB_NAMESPACE),
            "type",
            kind,
        )
    }) || ["doc-footnote", "doc-endnote"]
        .iter()
        .any(|role| element_has_token(element, "role", None, "role", role))
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
