use html_style_model::{ComputedStylesBuilder, StyleStringId};

pub(in crate::style::cascade::resolver) fn map_list_style_position(
    pos: &lightningcss::properties::list::ListStylePosition,
) -> html_style_model::ListStylePosition {
    use html_style_model::ListStylePosition as Out;
    use lightningcss::properties::list::ListStylePosition as Lc;
    match pos {
        Lc::Inside => Out::Inside,
        Lc::Outside => Out::Outside,
    }
}

/// Intern the URL of a `list-style-image`. Gradients / `image-set()` / `none`
/// yield `None`. The interned string index (a `u16`) is widened into the
/// style's `u32` slot; it is resolved to an image resource when the marker box
/// is built.
pub(in crate::style::cascade::resolver) fn list_style_image_to_interned(
    styles: &mut ComputedStylesBuilder,
    image: &lightningcss::values::image::Image,
) -> Option<StyleStringId> {
    use lightningcss::values::image::Image;
    match image {
        Image::Url(url) => {
            let raw = url.url.as_ref().trim();
            if raw.is_empty() {
                None
            } else {
                Some(styles.intern_string(raw))
            }
        }
        _ => None,
    }
}
