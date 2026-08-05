use html_parse::{HtmlParserOptions, HtmlQuirksMode, parse_html_document_with_options};

const CASES: [(&str, &str, HtmlQuirksMode); 3] = [
    ("document-compatmode-01.html", include_str!("../../../testdata/wpt/html/dom/documents/resource-metadata-management/document-compatmode-01.html"), HtmlQuirksMode::NoQuirks),
    ("document-compatmode-02.html", include_str!("../../../testdata/wpt/html/dom/documents/resource-metadata-management/document-compatmode-02.html"), HtmlQuirksMode::LimitedQuirks),
    ("document-compatmode-03.html", include_str!("../../../testdata/wpt/html/dom/documents/resource-metadata-management/document-compatmode-03.html"), HtmlQuirksMode::Quirks),
];

#[test]
fn runs_wpt_document_compatibility_modes_directly() {
    for (file, source, expected) in CASES {
        let parsed = parse_html_document_with_options(source, HtmlParserOptions::default());
        assert_eq!(parsed.quirks_mode(), expected, "{file}");
    }
}
