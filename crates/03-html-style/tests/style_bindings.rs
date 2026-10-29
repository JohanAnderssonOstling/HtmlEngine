use html_parse::parse_dom_document;
use html_style::style_document;
use std::collections::HashSet;

#[test]
fn bound_style_handles_include_generated_text_and_both_text_pseudos() {
    let document =
        parse_dom_document("<html><body><div id='target'>body text</div></body></html>").unwrap();
    let styled = style_document(
        document,
        &[r#"
        #target { color:red }
        #target::before { content:'prefix'; color:blue; font-size:12px }
        #target::after { content:'suffix'; color:green }
        #target::first-line { font-size:20px }
        #target::first-letter { font-size:24px }
    "#],
    );
    let (document, styles) = styled.into_parts();
    let target = document
        .node_ids()
        .find(|&node| document.get_dom_id(node) == Some("target"))
        .unwrap();
    let handles = styles.bound_styles().collect::<HashSet<_>>();
    for handle in [
        styles.style_for_node(target).unwrap(),
        styles.before_style_for_node(target).unwrap().0,
        styles.after_style_for_node(target).unwrap().0,
        styles.first_line_style_for_node(target).unwrap(),
        styles.first_letter_style_for_node(target).unwrap(),
        styles.before_first_letter_style_for_node(target).unwrap(),
    ] {
        assert!(handles.contains(&handle));
    }
    assert!(
        handles
            .into_iter()
            .all(|handle| styles.view(handle).is_some())
    );
}
