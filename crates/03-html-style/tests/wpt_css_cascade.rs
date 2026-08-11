use html_dom::DomNodeId;
use html_style::StyledDocument;
use html_style_model::{Display, StyleIndices};
use html_wpt_test_support::wpt_root;
use std::fs;

fn upstream_source(relative: &str) -> String {
    let path = wpt_root().join("css/css-cascade").join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read pinned WPT {}: {error}", path.display()))
}

fn embedded_styles(source: &str) -> String {
    let lower = source.to_ascii_lowercase();
    let mut styles = String::new();
    let mut cursor = 0;
    while let Some(open) = lower[cursor..].find("<style") {
        let open = cursor + open;
        let Some(start_offset) = lower[open..].find('>') else {
            break;
        };
        let start = open + start_offset + 1;
        let Some(end_offset) = lower[start..].find("</style>") else {
            break;
        };
        let end = start + end_offset;
        styles.push_str(&source[start..end]);
        styles.push('\n');
        cursor = end + "</style>".len();
    }
    styles
}

fn styled_upstream(relative: &str) -> StyledDocument {
    let source = upstream_source(relative);
    let css = embedded_styles(&source);
    let document = html_parse::parse_dom_document(&source).expect("pinned WPT HTML parses");
    html_style::style_document(document, &[&css])
}

fn node_by_id(document: &StyledDocument, id: &str) -> DomNodeId {
    document
        .document()
        .node_ids()
        .find(|&node| document.document().get_dom_id(node) == Some(id))
        .unwrap_or_else(|| panic!("WPT element #{id}"))
}

fn style_by_id(document: &StyledDocument, id: &str) -> StyleIndices {
    document
        .style_for_node(node_by_id(document, id))
        .unwrap_or_else(|| panic!("computed style for WPT element #{id}"))
}

#[test]
fn pinned_wpt_all_revert_allows_a_later_color() {
    let document = styled_upstream("all-prop-revert-color.html");
    let inner = document
        .document()
        .node_ids()
        .find(|&node| {
            document
                .document()
                .get_dom_classes(node)
                .any(|class| class == "inner")
        })
        .expect("WPT .inner element");
    let style = document.style_for_node(inner).expect("computed WPT style");
    assert_eq!(
        document.text_style(style).expect("text style").color,
        0x008000ff
    );
}

#[test]
fn pinned_wpt_revert_layer_cases_resolve_to_green() {
    // Static reftests only: cases such as 004 require executing Shadow DOM
    // mutation scripts and belong in a browser harness.
    for relative in [
        "revert-layer-001.html",
        "revert-layer-002.html",
        "revert-layer-005.html",
        "revert-layer-007.html",
        "revert-layer-009.html",
        "revert-layer-012.html",
    ] {
        let document = styled_upstream(relative);
        let style = style_by_id(&document, "target");
        assert_eq!(
            document
                .background_style(style)
                .expect("background style")
                .background_color,
            0x008000ff,
            "{relative}"
        );
    }
}

#[test]
fn pinned_wpt_revert_restores_the_div_ua_display() {
    let document = styled_upstream("revert-val-001.html");
    let style = style_by_id(&document, "inner");
    assert_eq!(
        document.box_model_style(style).expect("box style").display,
        Display::Block
    );
}

#[test]
fn pinned_wpt_important_font_size_controls_em_line_height() {
    let document = styled_upstream("important-vs-inline-002.html");
    let style = style_by_id(&document, "el");
    assert_eq!(
        document.font_style(style).expect("font style").font_size,
        18.0
    );
    assert_eq!(
        document.text_style(style).expect("text style").line_height,
        36.0
    );
}
