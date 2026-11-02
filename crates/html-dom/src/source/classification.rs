//! Resource classification shared by loaders and document stages.

/// Resource identifier without its query or fragment. This preserves schemes,
/// authority components, and filesystem paths; it performs no URL resolution.
pub fn uri_path(uri: &str) -> &str {
    uri.split(['?', '#']).next().unwrap_or(uri)
}

/// Whether a resource identifier starts with an ASCII URI scheme.
pub fn has_uri_scheme(uri: &str) -> bool {
    uri.split_once(':').is_some_and(|(scheme, _)| {
        scheme
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
    })
}

/// Media type without parameters or surrounding whitespace. Case is preserved
/// so consumers can compare with `eq_ignore_ascii_case` without allocating.
pub fn media_type_essence(media_type: &str) -> &str {
    media_type.split(';').next().unwrap_or_default().trim()
}

#[cfg(test)]
mod tests {
    use super::{has_uri_scheme, media_type_essence, uri_path};

    #[test]
    fn uri_schemes_require_a_valid_prefix_before_any_path_or_suffix() {
        for uri in [
            "https://example.com/a",
            "data:image/png;base64,AA==",
            "urn:book:1",
            "custom+v1.test-2:value",
            "C:\\book.html",
        ] {
            assert!(has_uri_scheme(uri), "{uri}");
        }
        for uri in [
            "",
            ":file",
            "1http:file",
            "folder/image:name.png",
            "image.png?x:y",
            "image.png#x:y",
            "//example.com:80/image.png",
        ] {
            assert!(!has_uri_scheme(uri), "{uri}");
        }
    }

    #[test]
    fn resource_suffixes_do_not_change_the_path_or_extension() {
        for uri in [
            "book.xhtml?mode=1#part",
            "book.xhtml#part?mode=1",
            "book.xhtml",
        ] {
            assert_eq!(uri_path(uri), "book.xhtml");
        }
        assert_eq!(uri_path("#part"), "");
        assert_eq!(uri_path("?query"), "");
    }

    #[test]
    fn media_types_preserve_case_and_ignore_parameters() {
        assert_eq!(media_type_essence(" Text/CSS ; charset=utf-8 "), "Text/CSS");
        assert!(
            media_type_essence("IMAGE/SVG+XML;charset=utf-8").eq_ignore_ascii_case("image/svg+xml")
        );
        assert_eq!(media_type_essence("  "), "");
    }
}
