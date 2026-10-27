use html_style::{
    PropertyCapability, PropertySyntax, StylesheetEntry, declaration_support,
    selector_syntax_is_valid, stylesheet_entries, stylesheet_syntax_is_valid,
};
use html_wpt_test_support::{
    CssParsingTestKind, extract_css_parsing_tests_with_prelude, relative_script_references,
    wpt_root,
};
use lightningcss::rules::CssRule;
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use lightningcss::visitor::{Visit, VisitTypes, Visitor};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

const KNOWN_FAILURES: &str = include_str!("../../../testdata/wpt/known-css-parsing-failures.txt");
const UNSUPPORTED_DEPENDENCY_PREFIXES: &str =
    include_str!("../../../testdata/wpt/unsupported-css-parsing-dependency-prefixes.txt");
const IGNORED_RUNS: &str = include_str!("../../../testdata/wpt/ignored-css-parsing-runs.txt");

#[test]
fn runs_vendored_declarative_css_parsing_tests_directly() {
    let root = wpt_root().join("css");
    let mut files = Vec::new();
    collect_html_files(&root, &mut files);
    files.sort();

    let mut known: BTreeSet<String> = KNOWN_FAILURES
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    let unsupported_dependency_prefixes: Vec<&str> = UNSUPPORTED_DEPENDENCY_PREFIXES
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.split_once("|reason=")
                .expect("unsupported dependency prefix includes a reason")
                .0
        })
        .collect();
    let mut exercised_unsupported_prefixes = BTreeSet::new();
    let mut ignored: BTreeSet<String> = IGNORED_RUNS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.split_once("|reason=")
                .expect("ignored CSS run includes a reason")
                .0
                .to_owned()
        })
        .collect();
    let ignored_total = ignored.len();
    let mut unexpected = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut parser_panics = Vec::new();
    let mut extracted = 0usize;
    let mut unsupported_calls = 0usize;
    let mut unsupported_details = Vec::new();
    let mut unusable_literal_calls = 0usize;
    let mut unsupported_declarations = 0usize;
    let mut renderer_unsupported_declarations = 0usize;
    let mut unsupported_rules = 0usize;
    let mut computed_helpers_checked_for_syntax = 0usize;
    let mut selector_serialization_helpers_checked_for_syntax = 0usize;
    let mut executed = 0usize;
    let mut known_mismatches = 0usize;
    let mut unsupported_dependency_mismatches = 0usize;
    let mut non_utf8_files = Vec::new();

    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .expect("file is under WPT CSS root")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(file)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", file.display()));
        let Ok(source) = String::from_utf8(bytes) else {
            // WPT deliberately contains non-UTF-8 encoding fixtures. They do
            // not contain declarative JavaScript parser assertions and cannot
            // be interpreted as Rust UTF-8 strings.
            non_utf8_files.push(relative);
            continue;
        };
        let mut script_prelude = String::new();
        for reference in relative_script_references(&source) {
            let script = file
                .parent()
                .expect("WPT file has a parent")
                .join(&reference);
            if script.is_file() {
                let contents = fs::read_to_string(&script).unwrap_or_else(|error| {
                    panic!(
                        "failed to read local WPT script {}: {error}",
                        script.display()
                    )
                });
                script_prelude.push_str(&contents);
                script_prelude.push('\n');
            }
        }
        let extraction = extract_css_parsing_tests_with_prelude(&source, &script_prelude);
        unsupported_calls += extraction.unsupported_calls.len();
        unsupported_details.extend(extraction.unsupported_calls.iter().map(|call| {
            format!(
                "{relative}|line={}|helper={}|reason={}",
                call.line, call.helper, call.reason
            )
        }));
        extracted += extraction.tests.len();

        let mut source_line_ordinals = BTreeMap::new();
        for test in &extraction.tests {
            let next_ordinal = source_line_ordinals
                .entry((test.line, format!("{:?}", test.kind)))
                .or_insert(0usize);
            let ordinal = *next_ordinal;
            *next_ordinal += 1;
            let run_key = format!(
                "{relative}|line={}|ordinal={ordinal}|kind={:?}",
                test.line, test.kind
            );
            if ignored.remove(&run_key) {
                continue;
            }
            let expectation = match test.kind {
                CssParsingTestKind::ValidValue
                | CssParsingTestKind::ComputedValue
                | CssParsingTestKind::ValidAndComputedValue
                | CssParsingTestKind::ShorthandValue => Some(true),
                CssParsingTestKind::InvalidValue => Some(false),
                CssParsingTestKind::ValidSelector
                | CssParsingTestKind::ValidForgivingSelector
                | CssParsingTestKind::SelectorSerialization => Some(true),
                CssParsingTestKind::InvalidSelector => Some(false),
                CssParsingTestKind::ValidRule
                | CssParsingTestKind::ValidLayerImport
                | CssParsingTestKind::InvalidLayerImport
                | CssParsingTestKind::ValidSupportsImport
                | CssParsingTestKind::UnsupportedSupportsImport => Some(true),
                CssParsingTestKind::InvalidRule | CssParsingTestKind::InvalidSupportsImport => {
                    Some(false)
                }
            };
            let Some(expected_valid) = expectation else {
                continue;
            };
            if std::env::var_os("HTML_WPT_TRACE").is_some() {
                eprintln!(
                    "running CSS adapter assertion: {run_key}|arguments={:?}",
                    test.arguments
                );
            }

            // In this pinned fixture the helper only creates an empty sheet
            // and checks its empty rule list. It never inserts its argument.
            // Treating the argument as a parser assertion invents a test.
            if relative == "css-cascade/parsing/supports-import-parsing.html"
                && test.kind == CssParsingTestKind::InvalidSupportsImport
            {
                unsupported_calls += 1;
                unsupported_details.push(format!(
                    "{run_key}|reason=pinned helper does not supply its rule to the stylesheet"
                ));
                continue;
            }

            let actual_valid = match test.kind {
                CssParsingTestKind::ValidSelector
                | CssParsingTestKind::InvalidSelector
                | CssParsingTestKind::ValidForgivingSelector => {
                    let Some(selector) = test.arguments.first() else {
                        unusable_literal_calls += 1;
                        continue;
                    };
                    let Ok(valid) =
                        catch_unwind(AssertUnwindSafe(|| selector_syntax_is_valid(selector)))
                    else {
                        parser_panics.push(run_key);
                        continue;
                    };
                    valid
                }
                CssParsingTestKind::ValidRule
                | CssParsingTestKind::InvalidRule
                | CssParsingTestKind::SelectorSerialization
                | CssParsingTestKind::ValidLayerImport
                | CssParsingTestKind::InvalidLayerImport
                | CssParsingTestKind::ValidSupportsImport
                | CssParsingTestKind::InvalidSupportsImport
                | CssParsingTestKind::UnsupportedSupportsImport => {
                    let Some(rule) = test.arguments.first() else {
                        unusable_literal_calls += 1;
                        continue;
                    };
                    let Ok(valid) = catch_unwind(AssertUnwindSafe(|| {
                        // The serialization helper assigns source + a block to
                        // a stylesheet, including any @namespace prelude. This
                        // checks input syntax only, without comparing selectorText.
                        let rule = if test.kind == CssParsingTestKind::SelectorSerialization {
                            Cow::Owned(format!("{rule} {{font-size:1em}}"))
                        } else {
                            Cow::Borrowed(rule.as_str())
                        };
                        if stylesheet_uses_unsupported_rules(&rule) {
                            return None;
                        }
                        if !stylesheet_syntax_is_valid(&rule) {
                            return Some(false);
                        }
                        if matches!(
                            test.kind,
                            CssParsingTestKind::ValidLayerImport
                                | CssParsingTestKind::InvalidLayerImport
                        ) {
                            let entries = stylesheet_entries(&rule);
                            let [StylesheetEntry::Import(import)] = entries.as_slice() else {
                                return Some(false);
                            };
                            return Some(if test.kind == CssParsingTestKind::ValidLayerImport {
                                import.layer.is_some() && import.media.is_none()
                            } else {
                                import.layer.is_none() && import.media.is_some()
                            });
                        }
                        Some(true)
                    })) else {
                        parser_panics.push(run_key);
                        continue;
                    };
                    let Some(valid) = valid else {
                        unsupported_rules += 1;
                        unsupported_details
                            .push(format!("{run_key}|reason=rule is outside renderer scope"));
                        continue;
                    };
                    valid
                }
                _ => {
                    let [property, value, ..] = test.arguments.as_slice() else {
                        unusable_literal_calls += 1;
                        continue;
                    };
                    let Ok(support) =
                        catch_unwind(AssertUnwindSafe(|| declaration_support(property, value)))
                    else {
                        parser_panics.push(run_key);
                        continue;
                    };
                    if let PropertyCapability::Unsupported(reason) = support.capability {
                        renderer_unsupported_declarations += 1;
                        unsupported_declarations += 1;
                        unsupported_details.push(format!(
                            "{run_key}|reason=renderer does not implement {reason:?}"
                        ));
                        continue;
                    }
                    match support.syntax {
                        PropertySyntax::Valid => true,
                        PropertySyntax::Invalid => false,
                        PropertySyntax::Unsupported => {
                            unreachable!("unsupported capabilities are reported above")
                        }
                    }
                }
            };
            if test.kind == CssParsingTestKind::SelectorSerialization {
                selector_serialization_helpers_checked_for_syntax += 1;
            }
            if matches!(
                test.kind,
                CssParsingTestKind::ComputedValue
                    | CssParsingTestKind::ValidAndComputedValue
                    | CssParsingTestKind::ShorthandValue
            ) {
                computed_helpers_checked_for_syntax += 1;
            }
            let expected = if expected_valid { "valid" } else { "invalid" };
            let actual = if actual_valid { "valid" } else { "invalid" };
            let prefix = format!(
                "{relative}|line={}|ordinal={ordinal}|kind={:?}|expected={expected}|",
                test.line, test.kind
            );
            let mismatch = format!("{prefix}actual={actual}");
            if actual_valid != expected_valid {
                if let Some(unsupported_prefix) = unsupported_dependency_prefixes
                    .iter()
                    .find(|prefix| relative.starts_with(**prefix))
                    .copied()
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
                if let Some(stale) = known
                    .iter()
                    .find(|entry| entry.starts_with(&prefix))
                    .cloned()
                {
                    known.remove(&stale);
                    unexpected_passes.push(stale);
                }
            }
            executed += 1;
        }
    }

    eprintln!(
        "direct CSS WPT parsing: {} files, {} non-UTF-8 encoding fixtures, {extracted} literal calls, {executed} non-excluded syntax assertions, {unsupported_declarations} unrepresented syntax assertions, {unsupported_dependency_mismatches} known dependency/scope mismatches, {renderer_unsupported_declarations} declarations with unsupported renderer capabilities, {unsupported_rules} rules outside renderer scope, {computed_helpers_checked_for_syntax} computed/shorthand helpers checked for syntax only, {selector_serialization_helpers_checked_for_syntax} selector serialization helpers checked for syntax only, {unsupported_calls} dynamic/unimplemented calls, {unusable_literal_calls} unusable literal calls, {known_mismatches} other known syntax mismatches, {ignored_total} explicitly ignored, {} unexpected mismatches, {} unexpected passes, {} dependency parser panics",
        files.len(),
        non_utf8_files.len(),
        unexpected.len(),
        unexpected_passes.len(),
        parser_panics.len()
    );
    if std::env::var_os("HTML_WPT_VERBOSE").is_some() {
        for detail in &unsupported_details {
            eprintln!("unsupported CSS adapter call: {detail}");
        }
    }

    assert!(
        parser_panics.is_empty(),
        "dependency parser panics must not be skipped:\n{}",
        parser_panics.join("\n")
    );
    assert_eq!(files.len(), 35_417, "the pinned CSS corpus changed");
    assert_eq!(
        non_utf8_files.len(),
        4,
        "the pinned non-UTF-8 fixture set changed: {non_utf8_files:?}"
    );
    assert!(
        extracted >= 31_000,
        "too few direct CSS WPT calls were extracted: {extracted}"
    );
    assert!(
        unusable_literal_calls == 0,
        "literal calls with unusable signatures: {unusable_literal_calls}"
    );
    assert!(
        unexpected.is_empty(),
        "unexpected CSS parsing mismatches:\n{}",
        unexpected.join("\n")
    );
    assert!(
        unexpected_passes.is_empty(),
        "known CSS parsing failures now pass and should be removed:\n{}",
        unexpected_passes.join("\n")
    );
    assert_eq!(
        exercised_unsupported_prefixes.len(),
        unsupported_dependency_prefixes.len(),
        "unsupported dependency prefixes were not exercised: {:?}",
        unsupported_dependency_prefixes
            .iter()
            .filter(|prefix| !exercised_unsupported_prefixes.contains(**prefix))
            .collect::<Vec<_>>()
    );
    assert!(
        known.is_empty(),
        "known CSS parsing failures were not exercised:\n{}",
        known.into_iter().collect::<Vec<_>>().join("\n")
    );
    assert!(
        ignored.is_empty(),
        "ignored CSS parsing runs were not exercised:\n{}",
        ignored.into_iter().collect::<Vec<_>>().join("\n")
    );
}

fn collect_html_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to list {}: {error}", directory.display()))
    {
        let path = entry.expect("valid directory entry").path();
        if path.is_dir() {
            collect_html_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "html")
        {
            files.push(path);
        }
    }
}

fn stylesheet_uses_unsupported_rules(css: &str) -> bool {
    struct RendererRules;
    impl<'i> Visitor<'i> for RendererRules {
        type Error = ();
        fn visit_types(&self) -> VisitTypes {
            lightningcss::visit_types!(RULES)
        }
        fn visit_rule(&mut self, rule: &mut CssRule<'i>) -> Result<(), ()> {
            match rule {
                CssRule::Style(_)
                | CssRule::Media(_)
                | CssRule::Supports(_)
                | CssRule::LayerBlock(_)
                | CssRule::LayerStatement(_)
                | CssRule::Scope(_)
                | CssRule::FontFace(_)
                | CssRule::Import(_)
                | CssRule::Namespace(_)
                | CssRule::Ignored => rule.visit_children(self),
                _ => Err(()),
            }
        }
    }
    StyleSheet::parse(
        css,
        ParserOptions {
            error_recovery: true,
            ..ParserOptions::default()
        },
    )
    .is_ok_and(|mut sheet| sheet.visit(&mut RendererRules).is_err())
}

#[test]
fn conformance_adapter_reports_unsupported_features_without_validating_them() {
    for (property, value) in [
        ("filter", "blur(2px)"),
        ("filter", "blur(bad)"),
        ("animation", "name 1s"),
        ("animation", "invalid invalid invalid"),
    ] {
        assert_eq!(
            declaration_support(property, value).syntax,
            PropertySyntax::Unsupported
        );
    }
    for css in [
        "@keyframes fade { from {opacity:0} }",
        "@position-try --fallback {top:10px}",
        "@media screen { @page {margin:0} }",
        "@custom-media --small (width < 20em);",
    ] {
        assert!(stylesheet_uses_unsupported_rules(css), "{css}");
    }
    for css in [
        "p {color:red}",
        "@media screen {@supports (width:10px) {p {width:10px}}}",
        "p {--opaque: {@page {color:red}}}",
        "@layer base {p {color:red}}",
    ] {
        assert!(!stylesheet_uses_unsupported_rules(css), "{css}");
    }
}
