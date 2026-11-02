use html_parse::parse_dom_document;
use html_style::style_document;
use html_style_model::resolve_used_preferred_size;

#[test]
fn size_comparisons_keep_branch_order_limits_and_percentage_dependence() {
    for (value, expected) in [
        ("min(10px,20%)", [2.0, 10.0]),
        ("max(30%,2ex,5px)", [20.0, 30.0]),
        ("clamp(10px,20%,30px)", [10.0, 20.0]),
        // Four unresolved branches exceed the existing renderer limit.
        ("min(10px,20%,2em,3ex)", [7.0, 7.0]),
    ] {
        let source = format!(
            "<p style='font-size:20px;width:7px;width:{value};text-indent:calc(10% + 2ex)'>Text</p>"
        );
        let styled = style_document(parse_dom_document(&source).unwrap(), &[]);
        let node = styled
            .document()
            .node_ids()
            .find(|&node| styled.document().get_dom_tag(node) == Some("p"))
            .unwrap();
        let used = styled
            .styles()
            .used_view(styled.style_for_node(node).unwrap(), 0.5, 0.6, 0.7)
            .unwrap();
        for (parent, expected) in [10.0, 100.0].into_iter().zip(expected) {
            let width = resolve_used_preferred_size(used.width(), -1.0, parent);
            assert!((width - expected).abs() < 0.00001, "{value}: {width}");
        }
        assert!(used.text_indent().has_percentage());
        assert!((used.text_indent().resolve(100.0) - 30.0).abs() < 0.00001);
    }
}
