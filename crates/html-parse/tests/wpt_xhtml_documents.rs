use html_parse::{MarkupSyntax, parse_document};
use std::fs;
use std::path::Path;

const GPUI_REFTESTS: &str = include_str!("../../../testdata/wpt/gpui-reftests.txt");

#[test]
fn parses_all_vendored_static_xhtml_reftests_as_xml() {
    let wpt_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/wpt");
    let mut parsed = 0usize;
    let mut failures = Vec::new();

    for relative in GPUI_REFTESTS.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).filter(|line| matches!(Path::new(line).extension().and_then(|extension| extension.to_str()), Some("xht" | "xhtml"))) {
        let source = match fs::read_to_string(wpt_root.join(relative)) {
            Ok(source) => source,
            Err(error) => {
                failures.push(format!("{relative}: could not read input: {error}"));
                continue;
            }
        };
        match parse_document(&source, MarkupSyntax::Xml) {
            Ok(_) => parsed += 1,
            Err(error) => failures.push(format!("{relative}: {error}")),
        }
    }

    assert!(parsed > 0, "the static WPT manifest contains no XHTML documents");
    assert!(failures.is_empty(), "{} static XHTML WPT documents failed strict XML parsing:\n{}", failures.len(), failures.into_iter().take(50).collect::<Vec<_>>().join("\n"));
}
