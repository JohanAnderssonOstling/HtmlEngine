use super::*;

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

/// Map Lightning CSS `list-style-type` onto the renderer's exact supported
/// set. Values requiring custom counter-style or marker-string rendering are
/// kept out of computed state instead of being silently approximated.
pub(in crate::style::cascade::resolver) fn map_list_style_type(
    lst: &lightningcss::properties::list::ListStyleType,
) -> Option<html_style_model::ListStyleType> {
    use html_style_model::ListStyleType as Out;
    use lightningcss::properties::list::{
        CounterStyle, ListStyleType as Lc, PredefinedCounterStyle as P,
    };
    Some(match lst {
        Lc::None => Out::None,
        Lc::String(_) => return None,
        Lc::CounterStyle(CounterStyle::Predefined(p)) => match p {
            P::Disc => Out::Disc,
            P::Circle => Out::Circle,
            P::Square => Out::Square,
            P::Decimal => Out::Decimal,
            P::DecimalLeadingZero => Out::DecimalLeadingZero,
            P::LowerAlpha | P::LowerLatin => Out::LowerAlpha,
            P::UpperAlpha | P::UpperLatin => Out::UpperAlpha,
            P::LowerRoman => Out::LowerRoman,
            P::UpperRoman => Out::UpperRoman,
            _ => return None,
        },
        Lc::CounterStyle(CounterStyle::Name(_) | CounterStyle::Symbols { .. }) => return None,
    })
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
