use html_parse::{HtmlFragmentContext, HtmlParserOptions, HtmlScriptingMode, HtmlSyntaxAttribute, HtmlSyntaxElement, HtmlSyntaxNode, HtmlSyntaxTree, parse_html_document_with_options, parse_html_fragment};
use html_wpt_test_support::{ScriptingMode, TreeConstructionCase, parse_tree_construction_file};

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";
const XLINK_NAMESPACE: &str = "http://www.w3.org/1999/xlink";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

const CORPORA: [(&str, &str); 61] = [
    ("blocks.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/blocks.dat")),
    ("adoption01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/adoption01.dat")),
    ("entities01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/entities01.dat")),
    ("adoption02.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/adoption02.dat")),
    ("entities02.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/entities02.dat")),
    ("inbody01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/inbody01.dat")),
    ("isindex.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/isindex.dat")),
    ("main-element.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/main-element.dat")),
    ("menuitem-element.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/menuitem-element.dat")),
    ("namespace-sensitivity.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/namespace-sensitivity.dat")),
    ("pending-spec-changes-plain-text-unsafe.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/pending-spec-changes-plain-text-unsafe.dat")),
    ("pending-spec-changes.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/pending-spec-changes.dat")),
    ("quirks01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/quirks01.dat")),
    ("ruby.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/ruby.dat")),
    ("search-element.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/search-element.dat")),
    ("tables01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tables01.dat")),
    ("tests11.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests11.dat")),
    ("tests12.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests12.dat")),
    ("tests14.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests14.dat")),
    ("tests17.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests17.dat")),
    ("tests22.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests22.dat")),
    ("tests23.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests23.dat")),
    ("tests24.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests24.dat")),
    ("tests25.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests25.dat")),
    ("tricky01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tricky01.dat")),
    ("void-in-phrasing.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/void-in-phrasing.dat")),
    ("comments01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/comments01.dat")),
    ("doctype01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/doctype01.dat")),
    ("domjs-unsafe.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/domjs-unsafe.dat")),
    ("foreign-fragment.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/foreign-fragment.dat")),
    ("html5test-com.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/html5test-com.dat")),
    ("math.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/math.dat")),
    ("noscript01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/noscript01.dat")),
    ("plain-text-unsafe.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/plain-text-unsafe.dat")),
    ("processing-instructions.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/processing-instructions.dat")),
    ("scriptdata01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/scriptdata01.dat")),
    ("scripted_adoption01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/scripted_adoption01.dat")),
    ("scripted_ark.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/scripted_ark.dat")),
    ("scripted_webkit01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/scripted_webkit01.dat")),
    ("svg.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/svg.dat")),
    ("template.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/template.dat")),
    ("tests1.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests1.dat")),
    ("tests10.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests10.dat")),
    ("tests15.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests15.dat")),
    ("tests16.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests16.dat")),
    ("tests18.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests18.dat")),
    ("tests19.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests19.dat")),
    ("tests2.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests2.dat")),
    ("tests20.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests20.dat")),
    ("tests21.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests21.dat")),
    ("tests26.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests26.dat")),
    ("tests3.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests3.dat")),
    ("tests4.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests4.dat")),
    ("tests5.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests5.dat")),
    ("tests6.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests6.dat")),
    ("tests7.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests7.dat")),
    ("tests8.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests8.dat")),
    ("tests9.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests9.dat")),
    ("tests_innerHTML_1.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/tests_innerHTML_1.dat")),
    ("webkit01.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/webkit01.dat")),
    ("webkit02.dat", include_str!("../../../testdata/wpt/html/syntax/parsing/resources/webkit02.dat")),
];

#[test]
fn runs_vendored_wpt_tree_construction_files_directly() {
    let mut executed = 0usize;
    let mut known = known_failures();
    let known_total = known.len();
    let mut ignored = ignored_tree_runs();
    let ignored_total = ignored.len();
    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();

    for (file, source) in CORPORA {
        let cases = parse_tree_construction_file(source).unwrap_or_else(|error| panic!("could not read {file}: {error}"));
        for (offset, case) in cases.iter().enumerate() {
            let case_number = offset + 1;
            for scripting in scripting_modes(case.scripting) {
                let key = format!("{file}|{case_number}|{scripting:?}");
                if ignored.remove(&key) {
                    continue;
                }
                let tree = parse_case(case, *scripting);
                let actual = dump_tree(&tree);
                if actual != case.document {
                    if !known.remove(&key) {
                        unexpected_failures.push((key, format!("input: {:?}\nexpected:\n{}\nactual:\n{}", case.data, case.document, actual)));
                    }
                } else if known.remove(&key) {
                    unexpected_passes.push(key);
                }
                executed += 1;
            }
        }
    }

    eprintln!("direct WPT tree-construction runs: {executed} executed, {known_total} known mismatches, {ignored_total} explicitly ignored, {} unexpected mismatches, {} unexpected passes", unexpected_failures.len(), unexpected_passes.len());
    assert!(executed > 0, "the direct WPT adapter did not execute any cases");
    if !unexpected_failures.is_empty() || !unexpected_passes.is_empty() || !known.is_empty() || !ignored.is_empty() {
        let failure_keys = unexpected_failures.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>().join("\n");
        let details = unexpected_failures.iter().take(5).map(|(key, detail)| format!("--- {key} ---\n{detail}")).collect::<Vec<_>>().join("\n");
        panic!(
            "WPT expectation ledger changed\nunexpected mismatches:\n{failure_keys}\nunexpected passes:\n{}\nknown entries not executed:\n{}\nignored entries not encountered:\n{}\n\nfirst mismatches:\n{details}",
            unexpected_passes.join("\n"),
            known.into_iter().collect::<Vec<_>>().join("\n"),
            ignored.into_iter().collect::<Vec<_>>().join("\n")
        );
    }
}

#[test]
fn audits_vendored_wpt_parse_error_counts() {
    let mut executed = 0usize;
    let mut known = known_error_count_failures();
    let known_total = known.len();
    let mut ignored = ignored_error_count_runs();
    let ignored_total = ignored.len();
    let mut unexpected_mismatches = Vec::new();
    let mut unexpected_passes = Vec::new();

    for (file, source) in CORPORA {
        let cases = parse_tree_construction_file(source).unwrap_or_else(|error| panic!("could not read {file}: {error}"));
        for (offset, case) in cases.iter().enumerate() {
            let case_number = offset + 1;
            for scripting in scripting_modes(case.scripting) {
                let key = format!("{file}|{case_number}|{scripting:?}");
                if ignored.remove(&key) {
                    continue;
                }
                let actual = parse_case_error_count(case, *scripting);
                let expected = case.errors.len();
                let entry = format!("{key}|expected={expected}|actual={actual}");
                if actual != expected {
                    if !known.remove(&entry) {
                        unexpected_mismatches.push(entry);
                    }
                } else if let Some(stale) = known.iter().find(|entry| entry.starts_with(&format!("{key}|"))).cloned() {
                    known.remove(&stale);
                    unexpected_passes.push(stale);
                }
                executed += 1;
            }
        }
    }

    eprintln!(
        "direct WPT parse-error runs: {executed} executed, {known_total} known count mismatches, {ignored_total} explicitly ignored, {} unexpected mismatches, {} unexpected passes",
        unexpected_mismatches.len(),
        unexpected_passes.len()
    );
    assert!(
        unexpected_mismatches.is_empty() && unexpected_passes.is_empty() && known.is_empty() && ignored.is_empty(),
        "WPT parse-error ledger changed\nunexpected mismatches:\n{}\nunexpected passes:\n{}\nknown entries not executed:\n{}\nignored entries not encountered:\n{}",
        unexpected_mismatches.join("\n"),
        unexpected_passes.join("\n"),
        known.into_iter().collect::<Vec<_>>().join("\n"),
        ignored.into_iter().collect::<Vec<_>>().join("\n")
    );
}

fn known_error_count_failures() -> BTreeSet<String> {
    include_str!("../../../testdata/wpt/known-parse-error-count-failures.txt").lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(str::to_owned).collect()
}

fn known_failures() -> BTreeSet<String> {
    include_str!("../../../testdata/wpt/known-tree-construction-failures.txt").lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(str::to_owned).collect()
}

fn ignored_tree_runs() -> BTreeSet<String> {
    ignored_runs(include_str!("../../../testdata/wpt/ignored-tree-construction-runs.txt"))
}

fn ignored_error_count_runs() -> BTreeSet<String> {
    let mut ignored = ignored_runs(include_str!("../../../testdata/wpt/ignored-parse-error-count-runs.txt"));
    ignored.extend(ignored_count_runs(include_str!("../../../testdata/wpt/ignored-parser-stack-error-count-runs.txt")));
    ignored
}

fn ignored_runs(source: &str) -> BTreeSet<String> {
    source.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(|line| line.split_once("|reason=").expect("ignored WPT run includes a reason").0.to_owned()).collect()
}

fn ignored_count_runs(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut fields = line.split('|');
            let file = fields.next().expect("ignored count run includes a file");
            let case = fields.next().expect("ignored count run includes a case");
            let scripting = fields.next().expect("ignored count run includes a scripting mode");
            assert!(fields.next().is_some(), "ignored count run includes its observed counts");
            format!("{file}|{case}|{scripting}")
        })
        .collect()
}

fn scripting_modes(mode: ScriptingMode) -> &'static [HtmlScriptingMode] {
    match mode {
        ScriptingMode::Both => &[HtmlScriptingMode::Disabled, HtmlScriptingMode::Enabled],
        ScriptingMode::Off => &[HtmlScriptingMode::Disabled],
        ScriptingMode::On => &[HtmlScriptingMode::Enabled],
    }
}

fn parse_case(case: &TreeConstructionCase, scripting: HtmlScriptingMode) -> HtmlSyntaxTree {
    let options = HtmlParserOptions { scripting, exact_errors: false };
    match case.fragment_context.as_deref() {
        Some(context) => parse_html_fragment(&case.data, &fragment_context(context), options).syntax_tree(),
        None => parse_html_document_with_options(&case.data, options).syntax_tree(),
    }
}

fn parse_case_error_count(case: &TreeConstructionCase, scripting: HtmlScriptingMode) -> usize {
    let options = HtmlParserOptions { scripting, exact_errors: true };
    match case.fragment_context.as_deref() {
        Some(context) => parse_html_fragment(&case.data, &fragment_context(context), options).errors().len(),
        None => parse_html_document_with_options(&case.data, options).errors().len(),
    }
}

fn fragment_context(value: &str) -> HtmlFragmentContext {
    if let Some(local_name) = value.strip_prefix("svg ") {
        HtmlFragmentContext::svg(local_name)
    } else if let Some(local_name) = value.strip_prefix("math ") {
        HtmlFragmentContext::math_ml(local_name)
    } else {
        HtmlFragmentContext::html(value)
    }
}

fn dump_tree(tree: &HtmlSyntaxTree) -> String {
    let mut lines = Vec::new();
    for node in &tree.nodes {
        dump_node(node, 0, &mut lines);
    }
    lines.join("\n")
}

fn dump_node(node: &HtmlSyntaxNode, depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    match node {
        HtmlSyntaxNode::Doctype { name, public_id, system_id } => {
            if public_id.is_empty() && system_id.is_empty() {
                lines.push(format!("| {indent}<!DOCTYPE {name}>"));
            } else {
                lines.push(format!("| {indent}<!DOCTYPE {name} \"{public_id}\" \"{system_id}\">"));
            }
        }
        HtmlSyntaxNode::Comment(comment) => lines.push(format!("| {indent}<!-- {comment} -->")),
        HtmlSyntaxNode::Text(text) => lines.push(format!("| {indent}\"{text}\"")),
        HtmlSyntaxNode::ProcessingInstruction { target, data } => lines.push(format!("| {indent}<?{target} {data}?>")),
        HtmlSyntaxNode::TemplateContents(children) => {
            lines.push(format!("| {indent}content"));
            for child in children {
                dump_node(child, depth + 1, lines);
            }
        }
        HtmlSyntaxNode::Element(element) => {
            lines.push(format!("| {indent}<{}{}>", element_namespace_prefix(element), element.local_name));
            let mut attributes: Vec<_> = element.attributes.iter().map(wpt_attribute).collect();
            attributes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            for (name, value) in attributes {
                lines.push(format!("| {indent}  {name}=\"{value}\""));
            }
            for child in &element.children {
                dump_node(child, depth + 1, lines);
            }
        }
    }
}

fn element_namespace_prefix(element: &HtmlSyntaxElement) -> &'static str {
    match element.namespace.as_deref() {
        None | Some(HTML_NAMESPACE) => "",
        Some(SVG_NAMESPACE) => "svg ",
        Some(MATHML_NAMESPACE) => "math ",
        Some(namespace) => panic!("WPT adapter does not know element namespace {namespace:?}"),
    }
}

fn wpt_attribute(attribute: &HtmlSyntaxAttribute) -> (String, String) {
    let prefix = match attribute.namespace.as_deref() {
        None => "",
        Some(XLINK_NAMESPACE) => "xlink ",
        Some(XML_NAMESPACE) => "xml ",
        Some(XMLNS_NAMESPACE) => "xmlns ",
        Some(namespace) => panic!("WPT adapter does not know attribute namespace {namespace:?}"),
    };
    let name = if attribute.namespace.is_some() { &attribute.local_name } else { &attribute.name };
    (format!("{prefix}{name}"), attribute.value.clone())
}
use std::collections::BTreeSet;
