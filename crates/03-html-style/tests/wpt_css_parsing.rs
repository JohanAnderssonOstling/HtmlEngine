use html_style::{
    PropertyCapability, PropertyValueSyntax, StylesheetEntry, UnsupportedStyleFeature,
    declaration_support, property_value_syntax, selector_syntax_is_valid, stylesheet_entries,
    stylesheet_syntax_is_valid,
};
use html_wpt_test_support::{
    CssParsingTestKind, extract_css_parsing_tests_with_prelude, relative_script_references,
    wpt_root,
};
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
    let mut computed_helpers_checked_for_syntax = 0usize;
    let mut selector_serialization_helpers_checked_for_syntax = 0usize;
    let mut executed = 0usize;
    let mut known_mismatches = 0usize;
    let mut unsupported_dependency_mismatches = 0usize;
    let mut superseded_easing_expectations = 0usize;
    let mut superseded_shadow_expectations = 0usize;
    let mut superseded_outline_expectations = 0usize;
    let mut superseded_alpha_expectations = 0usize;
    let mut superseded_symbols_expectations = 0usize;
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
            // These pinned assertions predate the current easing/math rules.
            // Easing 2 now permits a single linear() stop; Values 4 censors
            // NaN to zero and defers calculated integer range clamping.
            // Keep testing all four values, with their corrected expectation.
            // https://drafts.csswg.org/css-easing-2/#linear-easing-function
            // https://drafts.csswg.org/css-values-4/#infinity
            let corrected_easing_value = match run_key.as_str() {
                "css-easing/linear-timing-functions-syntax.html|line=32|ordinal=0|kind=InvalidValue" => Some("linear(0)"),
                "css-easing/linear-timing-functions-syntax.html|line=35|ordinal=0|kind=InvalidValue" => Some("linear(0 0% 100%)"),
                "css-easing/linear-timing-functions-syntax.html|line=36|ordinal=0|kind=InvalidValue" => Some("linear(0% 100% 0)"),
                "css-easing/step-timing-functions-syntax.html|line=32|ordinal=0|kind=InvalidValue" => Some("steps(calc(0/0), jump-none)"),
                _ => None,
            };
            let expected_valid = if let Some(value) = corrected_easing_value {
                assert!(!expected_valid);
                assert_eq!(test.arguments.as_slice(), ["animation-timing-function", value]);
                superseded_easing_expectations += 1;
                true
            } else {
                expected_valid
            };

            // This pinned Text Decoration 3 assertion predates the spread
            // length added by Text Decoration 4. Keep executing the same
            // value against the current grammar; changes to the fixture fail.
            // https://drafts.csswg.org/css-text-decor-4/#text-shadow-property
            let expected_valid = if run_key == "css-text-decor/text-shadow/parsing/text-shadow-invalid.html|line=15|ordinal=0|kind=InvalidValue" {
                assert!(!expected_valid);
                assert_eq!(test.arguments.as_slice(), ["text-shadow", "10px 20px 30px 40px"]);
                superseded_shadow_expectations += 1;
                true
            } else {
                expected_valid
            };

            // UI 4 removed invert after UI 3 already allowed rejecting it on
            // platforms without pixel inversion. These exact pinned fixtures
            // still expect it to parse. Keep checking all eleven inputs for
            // rejection, rather than implementing the obsolete keyword.
            // https://drafts.csswg.org/css-ui-4/#outline-color
            let obsolete_outline_value = run_key.strip_prefix(
                "css-ui/parsing/canonical-order-outline-sub-properties-001.html|line=158|ordinal=",
            ).and_then(|suffix| match suffix {
                "1|kind=ComputedValue" => Some("invert"),
                "7|kind=ComputedValue" => Some("dotted invert"),
                "9|kind=ComputedValue" => Some("invert solid"),
                "11|kind=ComputedValue" => Some("invert 4px"),
                "13|kind=ComputedValue" => Some("5px invert"),
                "15|kind=ComputedValue" => Some("invert solid 6px"),
                "17|kind=ComputedValue" => Some("invert 4px dotted"),
                "19|kind=ComputedValue" => Some("dashed 5px invert"),
                "21|kind=ComputedValue" => Some("solid invert 6px"),
                "23|kind=ComputedValue" => Some("4px dotted invert"),
                "25|kind=ComputedValue" => Some("5px invert dashed"),
                _ => None,
            });
            let expected_valid = if let Some(value) = obsolete_outline_value {
                assert!(expected_valid);
                assert_eq!(test.arguments.as_slice(), ["outline", value]);
                superseded_outline_expectations += 1;
                false
            } else {
                expected_valid
            };

            // The current Color 5 alpha() grammar requires slash plus alpha;
            // these exact pinned assertions use the older optional form.
            // Keep executing them as rejections, with fixture identity guards.
            // https://drafts.csswg.org/css-color-5/#relative-alpha
            let obsolete_alpha_value = match run_key.as_str() {
                "css-color/parsing/alpha-color-computed.html|line=30|ordinal=0|kind=ComputedValue"
                | "css-color/parsing/alpha-color-parsing-valid.html|line=27|ordinal=0|kind=ValidValue" => Some(("color", "alpha(from red)")),
                "css-color/parsing/alpha-color-computed.html|line=31|ordinal=0|kind=ComputedValue"
                | "css-color/parsing/alpha-color-parsing-valid.html|line=58|ordinal=0|kind=ValidValue" => Some(("color", "alpha(from rgba(255, 0, 0, 0.3))")),
                "css-color/parsing/alpha-color-computed.html|line=79|ordinal=0|kind=ComputedValue" => Some(("background-color", "alpha(from currentcolor)")),
                "css-color/parsing/alpha-color-parsing-valid.html|line=28|ordinal=0|kind=ValidValue" => Some(("color", "alpha(from currentcolor)")),
                _ => None,
            };
            let expected_valid = if let Some((property, value)) = obsolete_alpha_value {
                assert!(expected_valid);
                assert_eq!(test.arguments.as_slice(), [property, value]);
                superseded_alpha_expectations += 1;
                false
            } else {
                expected_valid
            };

            // Counter Styles permits images in symbols(). Keep the pinned
            // image-symbol assertion active with its corrected expectation.
            // https://drafts.csswg.org/css-counter-styles-3/#symbols-function
            let expected_valid = if run_key == "css-lists/parsing/list-style-type-invalid.html|line=21|ordinal=0|kind=InvalidValue" {
                assert!(!expected_valid);
                assert_eq!(test.arguments.as_slice(), ["list-style-type", "symbols(fixed url(\"https://example.com\"))"]);
                superseded_symbols_expectations += 1;
                true
            } else {
                expected_valid
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
                        if !stylesheet_syntax_is_valid(&rule) {
                            return false;
                        }
                        if matches!(
                            test.kind,
                            CssParsingTestKind::ValidLayerImport
                                | CssParsingTestKind::InvalidLayerImport
                        ) {
                            let entries = stylesheet_entries(&rule);
                            let [StylesheetEntry::Import(import)] = entries.as_slice() else {
                                return false;
                            };
                            return if test.kind == CssParsingTestKind::ValidLayerImport {
                                import.layer.is_some() && import.media.is_none()
                            } else {
                                import.layer.is_none() && import.media.is_some()
                            };
                        }
                        true
                    })) else {
                        parser_panics.push(run_key);
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
                        let capability = declaration_support(property, value).capability;
                        if matches!(capability, PropertyCapability::Unsupported(_)) {
                            renderer_unsupported_declarations += 1;
                        }
                        if matches!(
                            capability,
                            PropertyCapability::Unsupported(
                                UnsupportedStyleFeature::Gradient
                                    | UnsupportedStyleFeature::GeneratedContent
                            )
                        ) {
                            return None;
                        }
                        Some(property_value_syntax(property, value))
                    })) else {
                        parser_panics.push(run_key);
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
        "direct CSS WPT parsing: {} files, {} non-UTF-8 encoding fixtures, {extracted} literal calls, {executed} non-excluded syntax assertions, {unsupported_declarations} unrepresented syntax assertions, {unsupported_dependency_mismatches} known dependency/scope mismatches, {renderer_unsupported_declarations} declarations with unsupported renderer capabilities, {computed_helpers_checked_for_syntax} computed/shorthand helpers checked for syntax only, {selector_serialization_helpers_checked_for_syntax} selector serialization helpers checked for syntax only, {unsupported_calls} dynamic/unimplemented calls, {unusable_literal_calls} unusable literal calls, {known_mismatches} other known syntax mismatches, {ignored_total} explicitly ignored, {superseded_easing_expectations} superseded easing expectations checked, {superseded_shadow_expectations} superseded shadow expectations checked, {superseded_outline_expectations} superseded outline expectations checked, {superseded_alpha_expectations} superseded alpha expectations checked, {superseded_symbols_expectations} superseded symbols expectations checked, {} unexpected mismatches, {} unexpected passes, {} dependency parser panics",
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
    assert_eq!(superseded_easing_expectations, 4);
    assert_eq!(superseded_shadow_expectations, 1);
    assert_eq!(superseded_outline_expectations, 11);
    assert_eq!(superseded_alpha_expectations, 6);
    assert_eq!(superseded_symbols_expectations, 1);
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
