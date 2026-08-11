#[cfg(test)]
mod tests {
    use super::{MAX_COMPUTED_FLEX_FACTOR, MatchedRule, system_color_to_u32};
    use crate::document::{BorderCollapseMode, BorderStyle, CaptionSide, Clear, Display, Document, ElementRef, EmptyCellsMode, Float, FontRelativeLength, LengthPct, PositionMode, PreferredSize, TableLayoutMode, TextAlign, TextDirection, WhiteSpace};
    use crate::parser::DocumentFactory;

    #[path = "02-box-and-hints.rs"]
    mod box_and_hints;
    #[path = "01-cascade-and-custom.rs"]
    mod cascade_and_custom;
    #[path = "04-integration-and-paint.rs"]
    mod integration_and_paint;
    #[path = "05-selectors-and-pseudo.rs"]
    mod selectors_and_pseudo;
    #[path = "03-values-and-layout.rs"]
    mod values_and_layout;

    fn find_body(document: &Document) -> ElementRef<'_> {
        let html_idx = document.dom_root().expect("html root should exist");
        let html = document.element_ref(html_idx).expect("html should be element");
        let body_idx = html.children().find(|&child_idx| document.element_ref(child_idx).is_some_and(|element| element.tag() == "body")).expect("body should be child of html");
        document.element_ref(body_idx).expect("body should be element")
    }
}
