use html_style::{PropertyCapability, PropertyValueSyntax, UnsupportedStyleFeature, declaration_support, property_value_syntax, selector_syntax_is_valid, stylesheet_syntax_is_valid};
use html_wpt_test_support::{CssParsingTestKind, extract_css_parsing_tests_with_prelude, relative_script_references, wpt_root};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

const KNOWN_FAILURES: &str = include_str!("../../../testdata/wpt/known-css-parsing-failures.txt");
const UNSUPPORTED_DEPENDENCY_PREFIXES: &str = include_str!("../../../testdata/wpt/unsupported-css-parsing-dependency-prefixes.txt");
const IGNORED_RUNS: &str = include_str!("../../../testdata/wpt/ignored-css-parsing-runs.txt");

#[test]
fn runs_vendored_declarative_css_parsing_tests_directly() {
    let root = wpt_root().join("css");
    let mut files = Vec::new();
    collect_html_files(&root, &mut files);
    files.sort();

    let mut known: BTreeSet<String> = KNOWN_FAILURES.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(str::to_owned).collect();
    let unsupported_dependency_prefixes: Vec<&str> =
        UNSUPPORTED_DEPENDENCY_PREFIXES.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(|line| line.split_once("|reason=").expect("unsupported dependency prefix includes a reason").0).collect();
    let mut exercised_unsupported_prefixes = BTreeSet::new();
    let mut ignored: BTreeSet<String> =
        IGNORED_RUNS.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(|line| line.split_once("|reason=").expect("ignored CSS run includes a reason").0.to_owned()).collect();
    let ignored_total = ignored.len();
    let mut unexpected = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut extracted = 0usize;
    let mut unsupported_calls = 0usize;
    let mut unsupported_details = Vec::new();
    let mut unusable_literal_calls = 0usize;
    let mut unsupported_declarations = 0usize;
    let mut executed = 0usize;
    let mut known_mismatches = 0usize;
    let mut unsupported_dependency_mismatches = 0usize;

    for file in &files {
        let source = fs::read_to_string(file).unwrap_or_else(|error| panic!("failed to read {}: {error}", file.display()));
        let relative = file.strip_prefix(&root).expect("file is under WPT CSS root").to_string_lossy().replace('\\', "/");
        let mut script_prelude = String::new();
        for reference in relative_script_references(&source) {
            let script = file.parent().expect("WPT file has a parent").join(&reference);
            if script.is_file() {
                let contents = fs::read_to_string(&script).unwrap_or_else(|error| panic!("failed to read local WPT script {}: {error}", script.display()));
                script_prelude.push_str(&contents);
                script_prelude.push('\n');
            }
        }
        let extraction = extract_css_parsing_tests_with_prelude(&source, &script_prelude);
        unsupported_calls += extraction.unsupported_calls.len();
        unsupported_details.extend(extraction.unsupported_calls.iter().map(|call| format!("{relative}|line={}|helper={}|reason={}", call.line, call.helper, call.reason)));
        extracted += extraction.tests.len();

        let mut source_line_ordinals = BTreeMap::new();
        for test in &extraction.tests {
            let ordinal = source_line_ordinals.entry((test.line, format!("{:?}", test.kind))).or_insert(0usize);
            let run_key = format!("{relative}|line={}|ordinal={ordinal}|kind={:?}", test.line, test.kind);
            *ordinal += 1;
            if ignored.remove(&run_key) {
                continue;
            }
            let expectation = match test.kind {
                CssParsingTestKind::ValidValue | CssParsingTestKind::ComputedValue | CssParsingTestKind::ValidAndComputedValue | CssParsingTestKind::ShorthandValue => Some(true),
                CssParsingTestKind::InvalidValue => Some(false),
                CssParsingTestKind::ValidSelector | CssParsingTestKind::ValidForgivingSelector => Some(true),
                CssParsingTestKind::InvalidSelector => Some(false),
                CssParsingTestKind::ValidRule | CssParsingTestKind::ValidLayerImport | CssParsingTestKind::InvalidLayerImport | CssParsingTestKind::ValidSupportsImport | CssParsingTestKind::UnsupportedSupportsImport => Some(true),
                CssParsingTestKind::InvalidRule | CssParsingTestKind::InvalidSupportsImport => Some(false),
            };
            let Some(expected_valid) = expectation else { continue };

            if std::env::var_os("HTML_WPT_TRACE").is_some() {
                eprintln!("running CSS adapter assertion: {run_key}|arguments={:?}", test.arguments);
            }

            let actual_valid = match test.kind {
                CssParsingTestKind::ValidSelector | CssParsingTestKind::InvalidSelector | CssParsingTestKind::ValidForgivingSelector => {
                    let Some(selector) = test.arguments.first() else {
                        unusable_literal_calls += 1;
                        continue;
                    };
                    let Ok(valid) = catch_unwind(AssertUnwindSafe(|| selector_syntax_is_valid(selector))) else {
                        unsupported_calls += 1;
                        unsupported_details.push(format!("{run_key}|helper={:?}|reason=dependency parser panicked", test.kind));
                        continue;
                    };
                    valid
                }
                CssParsingTestKind::ValidRule
                | CssParsingTestKind::InvalidRule
                | CssParsingTestKind::ValidLayerImport
                | CssParsingTestKind::InvalidLayerImport
                | CssParsingTestKind::ValidSupportsImport
                | CssParsingTestKind::InvalidSupportsImport
                | CssParsingTestKind::UnsupportedSupportsImport => {
                    let Some(rule) = test.arguments.first() else {
                        unusable_literal_calls += 1;
                        continue;
                    };
                    let Ok(valid) = catch_unwind(AssertUnwindSafe(|| stylesheet_syntax_is_valid(rule))) else {
                        unsupported_calls += 1;
                        unsupported_details.push(format!("{run_key}|helper={:?}|reason=dependency parser panicked", test.kind));
                        continue;
                    };
                    valid
                }
                _ => {
                    let [property, value, ..] = test.arguments.as_slice() else {
                        unusable_literal_calls += 1;
                        continue;
                    };
                    let Ok(syntax) = catch_unwind(AssertUnwindSafe(|| {
                        if matches!(declaration_support(property, value).capability, PropertyCapability::Unsupported(UnsupportedStyleFeature::Gradient)) {
                            return None;
                        }
                        Some(property_value_syntax(property, value))
                    })) else {
                        unsupported_calls += 1;
                        unsupported_details.push(format!("{run_key}|helper={:?}|reason=dependency parser panicked", test.kind));
                        continue;
                    };
                    let Some(syntax) = syntax else {
                        unsupported_declarations += 1;
                        continue;
                    };
                    match syntax {
                        PropertyValueSyntax::Valid => true,
                        PropertyValueSyntax::Invalid => false,
                        PropertyValueSyntax::UnsupportedProperty => {
                            unsupported_declarations += 1;
                            continue;
                        }
                    }
                }
            };
            let expected = if expected_valid { "valid" } else { "invalid" };
            let actual = if actual_valid { "valid" } else { "invalid" };
            let prefix = format!("{relative}|line={}|ordinal={ordinal}|kind={:?}|expected={expected}|", test.line, test.kind);
            let mismatch = format!("{prefix}actual={actual}");
            if actual_valid != expected_valid {
                if let Some(unsupported_prefix) = unsupported_dependency_prefixes.iter().find(|prefix| relative.starts_with(**prefix)).copied()
                    && known.remove(&mismatch)
                {
                    unsupported_dependency_mismatches += 1;
                    exercised_unsupported_prefixes.insert(unsupported_prefix);
                    continue;
                } else if known.remove(&mismatch) {
                    known_mismatches += 1;
                } else {
                    unexpected.push(mismatch);
                }
            } else {
                if let Some(stale) = known.iter().find(|entry| entry.starts_with(&prefix)).cloned() {
                    known.remove(&stale);
                    unexpected_passes.push(stale);
                }
            }
            executed += 1;
        }
    }

    eprintln!(
        "direct CSS WPT parsing: {} files, {extracted} literal calls, {executed} supported assertions, {} unsupported property/feature assertions, {unsupported_calls} dynamic/unimplemented calls, {unusable_literal_calls} unusable literal calls, {known_mismatches} known mismatches, {ignored_total} explicitly ignored, {} unexpected mismatches, {} unexpected passes",
        files.len(),
        unsupported_declarations + unsupported_dependency_mismatches,
        unexpected.len(),
        unexpected_passes.len()
    );
    if std::env::var_os("HTML_WPT_VERBOSE").is_some() {
        for detail in &unsupported_details {
            eprintln!("unsupported CSS adapter call: {detail}");
        }
    }

    assert_eq!(files.len(), 22_977, "the pinned CSS corpus changed");
    assert!(extracted >= 31_000, "too few direct CSS WPT calls were extracted: {extracted}");
    assert!(unusable_literal_calls == 0, "literal calls with unusable signatures: {unusable_literal_calls}");
    assert!(unexpected.is_empty(), "unexpected CSS parsing mismatches:\n{}", unexpected.join("\n"));
    assert!(unexpected_passes.is_empty(), "known CSS parsing failures now pass and should be removed:\n{}", unexpected_passes.join("\n"));
    assert_eq!(
        exercised_unsupported_prefixes.len(),
        unsupported_dependency_prefixes.len(),
        "unsupported dependency prefixes were not exercised: {:?}",
        unsupported_dependency_prefixes.iter().filter(|prefix| !exercised_unsupported_prefixes.contains(**prefix)).collect::<Vec<_>>()
    );
    assert!(known.is_empty(), "known CSS parsing failures were not exercised:\n{}", known.into_iter().collect::<Vec<_>>().join("\n"));
    assert!(ignored.is_empty(), "ignored CSS parsing runs were not exercised:\n{}", ignored.into_iter().collect::<Vec<_>>().join("\n"));
}

fn collect_html_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap_or_else(|error| panic!("failed to list {}: {error}", directory.display())) {
        let path = entry.expect("valid directory entry").path();
        if path.is_dir() {
            collect_html_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "html") {
            files.push(path);
        }
    }
}
