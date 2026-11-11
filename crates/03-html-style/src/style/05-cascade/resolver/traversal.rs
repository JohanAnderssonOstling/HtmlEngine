use html_dom::Document;
use html_style_model::ComputedStylesBuilder;

/// `rem` on the root element uses the initial root size. Once that element has
/// been computed, later elements use its computed font size. DOM style
/// resolution is parent-before-child, so the root handle is available here
/// without a second cascade pass.
pub(super) fn root_font_size_for_resolution(doc: &Document, styles: &ComputedStylesBuilder) -> f32 {
    doc.dom_root()
        .and_then(|root| styles.style_for_node(root))
        .and_then(|indices| styles.font_style(indices))
        .map(|font| font.font_size)
        .filter(|size| size.is_finite() && *size >= 0.0)
        .unwrap_or_else(|| doc.root_font_size())
}
