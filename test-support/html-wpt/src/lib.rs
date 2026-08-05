//! Test-only adapters for consuming upstream Web Platform Tests unchanged.
//!
//! This crate deliberately contains no renderer knowledge. Owning stage crates
//! translate their public output contracts into the corresponding WPT form.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReftestRelation {
    Match,
    Mismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReftestReference {
    pub relation: ReftestRelation,
    pub href: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReftestFuzzyTolerance {
    pub reference: Option<String>,
    pub maximum_channel_delta: u8,
    pub maximum_different_pixels: usize,
}

/// Extract WPT `<meta name="fuzzy">` upper bounds. Both document-wide and
/// reference-scoped forms are supported; ranges use their upper endpoint.
pub fn extract_reftest_fuzzy_tolerances(source: &str) -> Vec<ReftestFuzzyTolerance> {
    let lower = source.to_ascii_lowercase();
    let mut tolerances = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find("<meta") {
        let start = cursor + offset;
        let Some(length) = lower[start..].find('>') else {
            break;
        };
        let end = start + length + 1;
        let attributes = html_tag_attributes(&source[start..end]);
        let is_fuzzy = attributes.iter().find(|(name, _)| name == "name").is_some_and(|(_, value)| value.eq_ignore_ascii_case("fuzzy"));
        if is_fuzzy
            && let Some((_, content)) = attributes.iter().find(|(name, _)| name == "content")
            && let Some(tolerance) = parse_fuzzy_content(content)
        {
            tolerances.push(tolerance);
        }
        cursor = start + "<meta".len();
    }
    tolerances
}

fn parse_fuzzy_content(content: &str) -> Option<ReftestFuzzyTolerance> {
    let marker = "maxDifference=";
    let marker_start = content.find(marker)?;
    let reference = content[..marker_start].trim().strip_suffix(':').map(str::trim).filter(|reference| !reference.is_empty()).map(str::to_owned);
    let settings = &content[marker_start..];
    let maximum_channel_delta = fuzzy_upper_bound(settings, "maxDifference=")?;
    let maximum_different_pixels = fuzzy_upper_bound(settings, "totalPixels=")?;
    Some(ReftestFuzzyTolerance { reference, maximum_channel_delta: u8::try_from(maximum_channel_delta).ok()?, maximum_different_pixels })
}

fn fuzzy_upper_bound(settings: &str, name: &str) -> Option<usize> {
    let value = settings.split(';').find_map(|setting| setting.trim().strip_prefix(name))?;
    value.rsplit_once('-').map_or(value, |(_, upper)| upper).trim().parse().ok()
}

/// Extract declarative WPT reftest links without executing the document.
/// The original HTML remains the test input; this only reads `<link rel>`
/// metadata used by the harness.
pub fn extract_reftest_references(source: &str) -> Vec<ReftestReference> {
    let lower = source.to_ascii_lowercase();
    let mut references = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find("<link") {
        let start = cursor + offset;
        let Some(length) = lower[start..].find('>') else {
            break;
        };
        let end = start + length + 1;
        let attributes = html_tag_attributes(&source[start..end]);
        let relation = attributes.iter().find(|(name, _)| name == "rel").and_then(|(_, value)| {
            value.split_ascii_whitespace().find_map(|token| {
                if token.eq_ignore_ascii_case("match") {
                    Some(ReftestRelation::Match)
                } else if token.eq_ignore_ascii_case("mismatch") {
                    Some(ReftestRelation::Mismatch)
                } else {
                    None
                }
            })
        });
        let href = attributes.iter().find(|(name, _)| name == "href").map(|(_, value)| value.trim_ascii().to_owned());
        if let (Some(relation), Some(href)) = (relation, href) {
            references.push(ReftestReference { relation, href });
        }
        // Do not jump past a malformed preceding tag's closing `>`. WPT has
        // recovery cases where another `<link` begins before that delimiter.
        cursor = start + "<link".len();
    }
    references
}

fn html_tag_attributes(tag: &str) -> Vec<(String, String)> {
    let bytes = tag.as_bytes();
    let mut cursor = tag.find(char::is_whitespace).unwrap_or(tag.len());
    let mut attributes = Vec::new();
    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(|byte| byte.is_ascii_whitespace()) {
            cursor += 1;
        }
        if matches!(bytes.get(cursor), None | Some(b'>') | Some(b'/')) {
            break;
        }
        let name_start = cursor;
        while bytes.get(cursor).is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':')) {
            cursor += 1;
        }
        if name_start == cursor {
            cursor += 1;
            continue;
        }
        let name = tag[name_start..cursor].to_ascii_lowercase();
        while bytes.get(cursor).is_some_and(|byte| byte.is_ascii_whitespace()) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            attributes.push((name, String::new()));
            continue;
        }
        cursor += 1;
        while bytes.get(cursor).is_some_and(|byte| byte.is_ascii_whitespace()) {
            cursor += 1;
        }
        let value = match bytes.get(cursor).copied() {
            Some(quote @ (b'\'' | b'"')) => {
                cursor += 1;
                let start = cursor;
                while bytes.get(cursor).is_some_and(|byte| *byte != quote) {
                    cursor += 1;
                }
                let value = tag[start..cursor].to_owned();
                cursor += usize::from(cursor < bytes.len());
                value
            }
            Some(_) => {
                let start = cursor;
                while bytes.get(cursor).is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>') {
                    cursor += 1;
                }
                tag[start..cursor].to_owned()
            }
            None => String::new(),
        };
        attributes.push((name, value));
    }
    attributes
}

/// A declarative helper used by the CSS parsing Web Platform Tests.
///
/// The adapter intentionally understands helper calls and JavaScript string
/// literals, not JavaScript execution. Calls whose arguments are computed by
/// script are returned separately as unsupported instead of being discarded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CssParsingTestKind {
    ValidValue,
    InvalidValue,
    ComputedValue,
    ValidAndComputedValue,
    ShorthandValue,
    ValidSelector,
    InvalidSelector,
    ValidForgivingSelector,
    ValidRule,
    InvalidRule,
    ValidLayerImport,
    InvalidLayerImport,
    ValidSupportsImport,
    InvalidSupportsImport,
    UnsupportedSupportsImport,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CssParsingTest {
    pub kind: CssParsingTestKind,
    pub arguments: Vec<String>,
    pub line: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedCssParsingCall {
    pub helper: String,
    pub line: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CssParsingExtraction {
    pub tests: Vec<CssParsingTest>,
    pub unsupported_calls: Vec<UnsupportedCssParsingCall>,
}

/// Return relative script resources referenced by a WPT HTML file. Shared
/// browser harness paths and absolute/network resources are intentionally
/// excluded; owning tests may load the returned files as static preludes.
pub fn relative_script_references(source: &str) -> Vec<String> {
    let lower = source.to_ascii_lowercase();
    let mut references = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find("<script") {
        let tag_start = cursor + offset;
        let Some(tag_length) = lower[tag_start..].find('>') else { break };
        let tag_end = tag_start + tag_length;
        let tag_lower = &lower[tag_start..tag_end];
        let Some(src_offset) = tag_lower.find("src") else {
            cursor = tag_end + 1;
            continue;
        };
        let mut value_start = tag_start + src_offset + 3;
        skip_ascii_whitespace(source.as_bytes(), &mut value_start);
        if source.as_bytes().get(value_start) != Some(&b'=') {
            cursor = tag_end + 1;
            continue;
        }
        value_start += 1;
        skip_ascii_whitespace(source.as_bytes(), &mut value_start);
        let Some(quote @ (b'\'' | b'"')) = source.as_bytes().get(value_start).copied() else {
            cursor = tag_end + 1;
            continue;
        };
        value_start += 1;
        let Some(value_length) = source.as_bytes()[value_start..tag_end].iter().position(|byte| *byte == quote) else {
            cursor = tag_end + 1;
            continue;
        };
        let reference = &source[value_start..value_start + value_length];
        if !reference.is_empty() && !reference.starts_with('/') && !reference.starts_with("//") && !reference.contains("://") {
            references.push(reference.to_owned());
        }
        cursor = tag_end + 1;
    }
    references
}

/// Extract literal declarative CSS parsing tests from an original WPT HTML
/// file without running its JavaScript.
pub fn extract_css_parsing_tests(source: &str) -> CssParsingExtraction {
    extract_css_parsing_tests_with_prelude(source, "")
}

/// Extract tests from an unchanged WPT HTML file while making definitions
/// from its relative, local script resources available to the static adapter.
/// The prelude is context only: calls in it are not emitted and HTML line
/// numbers remain relative to `source`.
pub fn extract_css_parsing_tests_with_prelude(source: &str, prelude: &str) -> CssParsingExtraction {
    let combined = if prelude.is_empty() { source.to_owned() } else { format!("{prelude}\n{source}") };
    let source_start = if prelude.is_empty() { 0 } else { prelude.len() + 1 };
    let source = combined.as_str();
    let bytes = source.as_bytes();
    let static_loops = collect_static_loop_bindings(source);
    let static_functions = collect_static_functions(source);
    let mut extraction = CssParsingExtraction::default();
    let mut cursor = source_start;
    let mut line = 1;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' | b'`' => {
                let quote = bytes[cursor];
                skip_js_string(bytes, &mut cursor, &mut line, quote);
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'/') => {
                cursor += 2;
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor += 2;
                while cursor + 1 < bytes.len() && !(bytes[cursor] == b'*' && bytes[cursor + 1] == b'/') {
                    if bytes[cursor] == b'\n' {
                        line += 1;
                    }
                    cursor += 1;
                }
                cursor = (cursor + 2).min(bytes.len());
            }
            byte if is_js_identifier_start(byte) => {
                let start = cursor;
                let call_line = line;
                cursor += 1;
                while cursor < bytes.len() && is_js_identifier_continue(bytes[cursor]) {
                    cursor += 1;
                }
                let helper = &source[start..cursor];
                if preceding_word(source, start) == Some("function") {
                    skip_js_function_body(source, &mut cursor, &mut line);
                    continue;
                }
                let mut open = cursor;
                while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                    if bytes[open] == b'\n' {
                        line += 1;
                    }
                    open += 1;
                }
                if (!helper.starts_with("test_") && helper != "assert_valid" && !helper.starts_with("fuzzy_test_") && !static_functions.contains_key(helper)) || bytes.get(open) != Some(&b'(') {
                    cursor = open;
                    continue;
                }

                match collect_js_call_arguments(source, open, &mut line) {
                    Ok((raw_arguments, end)) => {
                        cursor = end;
                        if helper == "assert_valid" {
                            match expand_is_where_assertion(&raw_arguments, call_line) {
                                Ok(tests) => extraction.tests.extend(tests),
                                Err(reason) => extraction.unsupported_calls.push(UnsupportedCssParsingCall { helper: helper.to_owned(), line: call_line, reason }),
                            }
                            continue;
                        }
                        if let Some(expansion) = expand_custom_css_helper(source, helper, &raw_arguments, start, call_line, &static_loops) {
                            match expansion {
                                Ok(tests) => extraction.tests.extend(tests),
                                Err(reason) => extraction.unsupported_calls.push(UnsupportedCssParsingCall { helper: helper.to_owned(), line: call_line, reason }),
                            }
                            continue;
                        }
                        if let Some(function) = static_functions.get(helper) {
                            match expand_static_function(source, function, &raw_arguments, start, call_line, &static_loops) {
                                Ok(tests) if !tests.is_empty() => {
                                    extraction.tests.extend(tests);
                                    continue;
                                }
                                Ok(_) => {}
                                Err(reason) => {
                                    extraction.unsupported_calls.push(UnsupportedCssParsingCall { helper: helper.to_owned(), line: call_line, reason });
                                    continue;
                                }
                            }
                        }
                        let kind = css_test_kind(helper);
                        let required_arguments = kind.map(required_argument_count).unwrap_or(raw_arguments.len());
                        let arguments = expand_static_arguments(&raw_arguments, start, &static_loops, required_arguments);
                        match (kind, arguments) {
                            (Some(kind), Some(argument_sets)) => extraction.tests.extend(argument_sets.into_iter().map(|arguments| CssParsingTest { kind, arguments, line: call_line })),
                            (None, _) => extraction.unsupported_calls.push(UnsupportedCssParsingCall { helper: helper.to_owned(), line: call_line, reason: "helper is not implemented by the direct adapter".to_owned() }),
                            (Some(_), None) => {
                                extraction.unsupported_calls.push(UnsupportedCssParsingCall { helper: helper.to_owned(), line: call_line, reason: "one or more arguments are computed JavaScript rather than string literals".to_owned() })
                            }
                        }
                    }
                    Err(reason) => {
                        extraction.unsupported_calls.push(UnsupportedCssParsingCall { helper: helper.to_owned(), line: call_line, reason });
                        cursor = open + 1;
                    }
                }
            }
            b'\n' => {
                line += 1;
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }

    extraction
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StaticLoopBinding {
    name: String,
    values: Vec<Vec<String>>,
    body_start: usize,
    body_end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StaticFunction {
    parameters: Vec<String>,
    body_start: usize,
    body_end: usize,
}

/// Find the small, declarative `for (const name of ["a", "b"])` matrices
/// used by CSS parsing WPTs. This is data expansion, not JavaScript execution.
fn collect_static_loop_bindings(source: &str) -> Vec<StaticLoopBinding> {
    let bytes = source.as_bytes();
    let arrays = collect_static_string_arrays(source);
    let mut bindings = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes.get(cursor..cursor + 3) != Some(b"for")
            || cursor.checked_sub(1).and_then(|index| bytes.get(index)).is_some_and(|byte| is_js_identifier_continue(*byte))
            || bytes.get(cursor + 3).is_some_and(|byte| is_js_identifier_continue(*byte))
        {
            advance_js_token(bytes, &mut cursor);
            continue;
        }

        let Some(binding) = parse_static_loop_binding(source, cursor, &arrays) else {
            cursor += 3;
            continue;
        };
        cursor += 3;
        if !static_loop_binding_is_mutated(source, &binding) {
            bindings.push(binding);
        }
    }
    bindings
}

fn static_loop_binding_is_mutated(source: &str, binding: &StaticLoopBinding) -> bool {
    let bytes = source.as_bytes();
    let mut cursor = binding.body_start + 1;
    while cursor < binding.body_end {
        if !is_js_identifier_start(bytes[cursor]) {
            advance_js_token(bytes, &mut cursor);
            continue;
        }
        let start = cursor;
        cursor += 1;
        while cursor < binding.body_end && is_js_identifier_continue(bytes[cursor]) {
            cursor += 1;
        }
        if source.get(start..cursor) != Some(binding.name.as_str()) {
            continue;
        }
        skip_ascii_whitespace(bytes, &mut cursor);
        if bytes.get(cursor) == Some(&b'=') && bytes.get(cursor + 1) != Some(&b'=') && bytes.get(cursor + 1) != Some(&b'>') {
            return true;
        }
    }
    false
}

fn parse_static_loop_binding(source: &str, start: usize, arrays: &BTreeMap<String, Vec<Vec<String>>>) -> Option<StaticLoopBinding> {
    let bytes = source.as_bytes();
    let mut cursor = start + 3;
    skip_ascii_whitespace(bytes, &mut cursor);
    expect_byte(bytes, &mut cursor, b'(')?;
    skip_ascii_whitespace(bytes, &mut cursor);
    if bytes.get(cursor..cursor + 5) == Some(b"const") {
        cursor += 5;
    } else if bytes.get(cursor..cursor + 3) == Some(b"let") {
        cursor += 3;
    } else {
        return None;
    }
    skip_ascii_whitespace(bytes, &mut cursor);
    let name_start = cursor;
    while bytes.get(cursor).is_some_and(|byte| is_js_identifier_continue(*byte)) {
        cursor += 1;
    }
    let name = source.get(name_start..cursor)?.to_owned();
    if name.is_empty() {
        return None;
    }
    skip_ascii_whitespace(bytes, &mut cursor);
    if bytes.get(cursor..cursor + 2) != Some(b"of") {
        return None;
    }
    cursor += 2;
    skip_ascii_whitespace(bytes, &mut cursor);
    let values = if bytes.get(cursor) == Some(&b'[') {
        let array_start = cursor;
        let array_end = find_matching_delimiter(source, array_start, b'[', b']')?;
        let values = parse_static_string_array(source.get(array_start + 1..array_end)?, arrays)?;
        cursor = array_end + 1;
        values
    } else {
        let array_name_start = cursor;
        while bytes.get(cursor).is_some_and(|byte| is_js_identifier_continue(*byte)) {
            cursor += 1;
        }
        arrays.get(source.get(array_name_start..cursor)?)?.clone()
    };
    skip_ascii_whitespace(bytes, &mut cursor);
    expect_byte(bytes, &mut cursor, b')')?;
    skip_ascii_whitespace(bytes, &mut cursor);
    let body_start = cursor;
    expect_byte(bytes, &mut cursor, b'{')?;
    let body_end = find_matching_delimiter(source, body_start, b'{', b'}')?;
    Some(StaticLoopBinding { name, values, body_start, body_end })
}

fn collect_static_string_arrays(source: &str) -> BTreeMap<String, Vec<Vec<String>>> {
    let bytes = source.as_bytes();
    let mut arrays = BTreeMap::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if !is_js_identifier_start(bytes[cursor]) {
            advance_js_token(bytes, &mut cursor);
            continue;
        }
        let name_start = cursor;
        cursor += 1;
        while bytes.get(cursor).is_some_and(|byte| is_js_identifier_continue(*byte)) {
            cursor += 1;
        }
        let Some(name) = source.get(name_start..cursor).filter(|name| !name.is_empty()) else {
            continue;
        };
        skip_ascii_whitespace(bytes, &mut cursor);
        if bytes.get(cursor) != Some(&b'=') {
            continue;
        }
        cursor += 1;
        skip_ascii_whitespace(bytes, &mut cursor);
        if bytes.get(cursor) != Some(&b'[') {
            continue;
        }
        let array_start = cursor;
        let Some(array_end) = find_matching_delimiter(source, array_start, b'[', b']') else {
            continue;
        };
        if let Some(values) = source.get(array_start + 1..array_end).and_then(|body| parse_static_string_array(body, &arrays)) {
            arrays.insert(name.to_owned(), values);
        }
        cursor = array_end + 1;
    }
    arrays
}

fn parse_static_string_array(source: &str, arrays: &BTreeMap<String, Vec<Vec<String>>>) -> Option<Vec<Vec<String>>> {
    let mut values = Vec::new();
    for item in split_js_values(source)? {
        let uncommented = item.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        let item = uncommented.trim();
        if item.is_empty() {
            continue;
        }
        if let Some(name) = item.strip_prefix("...") {
            values.extend(arrays.get(name.trim())?.iter().cloned());
        } else if let Some(body) = item.trim().strip_prefix('[').and_then(|item| item.strip_suffix(']')) {
            let tuple = split_js_values(body)?.into_iter().map(parse_js_string_literal).collect::<Option<Vec<_>>>()?;
            values.push(tuple);
        } else {
            values.push(vec![parse_js_string_literal(item)?]);
        }
    }
    Some(values)
}

fn collect_static_functions(source: &str) -> BTreeMap<String, StaticFunction> {
    let bytes = source.as_bytes();
    let mut functions = BTreeMap::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes.get(cursor..cursor + 8) != Some(b"function")
            || cursor.checked_sub(1).and_then(|index| bytes.get(index)).is_some_and(|byte| is_js_identifier_continue(*byte))
            || bytes.get(cursor + 8).is_some_and(|byte| is_js_identifier_continue(*byte))
        {
            advance_js_token(bytes, &mut cursor);
            continue;
        }
        cursor += 8;
        skip_ascii_whitespace(bytes, &mut cursor);
        let name_start = cursor;
        while bytes.get(cursor).is_some_and(|byte| is_js_identifier_continue(*byte)) {
            cursor += 1;
        }
        let Some(name) = source.get(name_start..cursor).filter(|name| !name.is_empty()) else {
            continue;
        };
        skip_ascii_whitespace(bytes, &mut cursor);
        if bytes.get(cursor) != Some(&b'(') {
            continue;
        }
        let parameters_start = cursor;
        let Some(parameters_end) = find_matching_delimiter(source, parameters_start, b'(', b')') else {
            continue;
        };
        let Some(parameters) = source.get(parameters_start + 1..parameters_end).and_then(split_js_values) else {
            continue;
        };
        let parameters = parameters.into_iter().map(str::trim).map(str::to_owned).collect::<Vec<_>>();
        cursor = parameters_end + 1;
        skip_ascii_whitespace(bytes, &mut cursor);
        let body_start = cursor;
        if bytes.get(body_start) != Some(&b'{') {
            continue;
        }
        let Some(body_end) = find_matching_delimiter(source, body_start, b'{', b'}') else {
            continue;
        };
        functions.insert(name.to_owned(), StaticFunction { parameters, body_start, body_end });
        cursor = body_end + 1;
    }
    functions
}

fn expand_static_arguments(raw_arguments: &[&str], call_offset: usize, loops: &[StaticLoopBinding], required_arguments: usize) -> Option<Vec<Vec<String>>> {
    expand_static_arguments_with_environment(raw_arguments, call_offset, loops, required_arguments, &BTreeMap::new())
}

fn expand_static_arguments_with_environment(raw_arguments: &[&str], call_offset: usize, loops: &[StaticLoopBinding], required_arguments: usize, base_environment: &BTreeMap<String, String>) -> Option<Vec<Vec<String>>> {
    let raw_arguments = &raw_arguments[..raw_arguments.len().min(required_arguments)];
    let enclosing = loops.iter().filter(|binding| binding.body_start < call_offset && call_offset < binding.body_end).collect::<Vec<_>>();
    let mut environments = vec![base_environment.clone()];
    for binding in enclosing {
        let mut expanded = Vec::new();
        for environment in &environments {
            for tuple in &binding.values {
                let mut next = environment.clone();
                if let Some(value) = tuple.first() {
                    next.insert(binding.name.clone(), value.clone());
                }
                for (index, value) in tuple.iter().enumerate() {
                    next.insert(format!("{}[{index}]", binding.name), value.clone());
                }
                expanded.push(next);
            }
        }
        environments = expanded;
    }

    environments.into_iter().map(|environment| raw_arguments.iter().map(|argument| evaluate_static_string(argument, &environment)).collect()).collect()
}

fn evaluate_static_string(argument: &str, environment: &BTreeMap<String, String>) -> Option<String> {
    if let Some(value) = parse_js_string_literal(argument) {
        return Some(value);
    }
    let argument = argument.trim();
    if let Some(value) = environment.get(argument) {
        return Some(value.clone());
    }
    if let Some(terms) = split_static_string_concatenation(argument) {
        let mut value = String::new();
        for term in terms {
            value.push_str(&evaluate_static_string(term, environment)?);
        }
        return Some(value);
    }
    if let Some(spread) = argument.strip_prefix("...")
        && let Some(value) = environment.get(&format!("{}[0]", spread.trim()))
    {
        return Some(value.clone());
    }
    if !(argument.starts_with('`') && argument.ends_with('`')) {
        return None;
    }

    let mut expanded = argument[1..argument.len() - 1].to_owned();
    for (name, value) in environment {
        expanded = expanded.replace(&format!("${{{name}}}"), value);
    }
    (!expanded.contains("${")).then_some(expanded)
}

fn split_static_string_concatenation(source: &str) -> Option<Vec<&str>> {
    let bytes = source.as_bytes();
    let mut cursor = 0usize;
    let mut start = 0usize;
    let mut terms = Vec::new();
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    let mut ignored_line = 0usize;

    while cursor < bytes.len() {
        match bytes[cursor] {
            quote @ (b'\'' | b'"' | b'`') => skip_js_string(bytes, &mut cursor, &mut ignored_line, quote),
            b'(' => {
                parentheses += 1;
                cursor += 1;
            }
            b')' => {
                parentheses = parentheses.saturating_sub(1);
                cursor += 1;
            }
            b'[' => {
                brackets += 1;
                cursor += 1;
            }
            b']' => {
                brackets = brackets.saturating_sub(1);
                cursor += 1;
            }
            b'{' => {
                braces += 1;
                cursor += 1;
            }
            b'}' => {
                braces = braces.saturating_sub(1);
                cursor += 1;
            }
            b'+' if parentheses == 0 && brackets == 0 && braces == 0 => {
                let term = source[start..cursor].trim();
                if term.is_empty() {
                    return None;
                }
                terms.push(term);
                cursor += 1;
                start = cursor;
            }
            _ => cursor += 1,
        }
    }

    if terms.is_empty() {
        return None;
    }
    let final_term = source[start..].trim();
    if final_term.is_empty() {
        return None;
    }
    terms.push(final_term);
    Some(terms)
}

fn expand_static_function(source: &str, function: &StaticFunction, call_arguments: &[&str], call_offset: usize, call_line: usize, loops: &[StaticLoopBinding]) -> Result<Vec<CssParsingTest>, String> {
    let actual_sets = expand_static_arguments(call_arguments, call_offset, loops, function.parameters.len()).ok_or_else(|| "wrapper arguments are not a static literal expansion".to_owned())?;
    let bytes = source.as_bytes();
    let mut output = Vec::new();

    for actuals in actual_sets {
        let environment = function.parameters.iter().cloned().zip(actuals).collect::<BTreeMap<_, _>>();
        let mut cursor = function.body_start + 1;
        while cursor < function.body_end {
            match bytes[cursor] {
                b'\'' | b'"' | b'`' => {
                    let quote = bytes[cursor];
                    let mut ignored_line = 0;
                    skip_js_string(bytes, &mut cursor, &mut ignored_line, quote);
                }
                byte if is_js_identifier_start(byte) => {
                    let helper_start = cursor;
                    cursor += 1;
                    while cursor < function.body_end && is_js_identifier_continue(bytes[cursor]) {
                        cursor += 1;
                    }
                    let helper = &source[helper_start..cursor];
                    if preceding_word(source, helper_start) == Some("function") {
                        let mut ignored_line = 0;
                        skip_js_function_body(source, &mut cursor, &mut ignored_line);
                        continue;
                    }
                    let Some(kind) = css_test_kind(helper) else {
                        continue;
                    };
                    let mut open = cursor;
                    skip_ascii_whitespace(bytes, &mut open);
                    if bytes.get(open) != Some(&b'(') {
                        continue;
                    }
                    let mut ignored_line = 0;
                    let (arguments, end) = collect_js_call_arguments(source, open, &mut ignored_line)?;
                    cursor = end;
                    let required = required_argument_count(kind);
                    let expanded = expand_static_arguments_with_environment(&arguments, helper_start, loops, required, &environment).ok_or_else(|| format!("{helper} inside wrapper is not a static literal expansion"))?;
                    output.extend(expanded.into_iter().map(|arguments| CssParsingTest { kind, arguments, line: call_line }));
                }
                _ => cursor += 1,
            }
        }
    }

    let mut deduplicated = Vec::new();
    for test in output {
        if !deduplicated.contains(&test) {
            deduplicated.push(test);
        }
    }
    Ok(deduplicated)
}

fn expand_is_where_assertion(raw_arguments: &[&str], line: usize) -> Result<Vec<CssParsingTest>, String> {
    let [valid, pattern, expected, ..] = raw_arguments else {
        return Err("assert_valid requires boolean, pattern, and expected pattern arguments".to_owned());
    };
    let kind = match valid.trim() {
        "true" => CssParsingTestKind::ValidSelector,
        "false" => CssParsingTestKind::ValidForgivingSelector,
        _ => return Err("assert_valid validity is not a literal boolean".to_owned()),
    };
    let pattern = parse_js_string_literal(pattern).ok_or_else(|| "assert_valid pattern is not a string literal".to_owned())?;
    let expected = if expected.trim() == "null" { None } else { Some(parse_js_string_literal(expected).ok_or_else(|| "assert_valid expected pattern is not null or a string literal".to_owned())?) };

    Ok(["is", "where"]
        .into_iter()
        .map(|pseudo| {
            let selector = pattern.replace("{}", &format!(":{pseudo}"));
            let mut arguments = vec![selector.clone()];
            if let Some(expected) = &expected {
                arguments.push(expected.replace("{}", &format!(":{pseudo}")));
            } else {
                arguments.push(selector);
            }
            CssParsingTest { kind, arguments, line }
        })
        .collect())
}

fn expand_custom_css_helper(source: &str, helper: &str, arguments: &[&str], call_offset: usize, line: usize, loops: &[StaticLoopBinding]) -> Option<Result<Vec<CssParsingTest>, String>> {
    let declaration_tests = |kind, property: &str, value_indices: &[usize]| -> Result<Vec<CssParsingTest>, String> {
        value_indices
            .iter()
            .map(|index| {
                let value = literal_argument(arguments, *index, helper)?;
                Ok(CssParsingTest { kind, arguments: vec![property.to_owned(), value], line })
            })
            .collect()
    };
    let rule_test = |kind, rule: String| Ok(vec![CssParsingTest { kind, arguments: vec![rule], line }]);

    if !matches!(
        helper,
        "test_pseudo_computed_value"
            | "test_relative"
            | "test_relative_size"
            | "test_font_size"
            | "test_system_font"
            | "test_auto_duration"
            | "test_currentcolor_alpha"
            | "test_keyframes_name_valid"
            | "test_keyframes_name_invalid"
            | "test_valid_keyframe_selector"
            | "test_invalid_keyframe_selector"
            | "test_display_computed"
            | "test_display_affected"
            | "fuzzy_test_valid_color"
            | "fuzzy_test_computed_color"
            | "fuzzy_test_computed_color_property"
            | "fuzzy_test_computed_color_using_currentcolor"
            | "test_valid_color_layers_value"
            | "test_inset_longhand"
            | "test_inset_shorthand"
            | "grid_template_columns"
            | "grid_template_rows"
            | "testGridDefinitionsSetJSValues"
            | "testGridDefinitionsSetBadJSValues"
            | "testInherit"
            | "testInitial"
            | "testValidGta"
            | "testInvalidGta"
            | "testValidGridTemplate"
            | "test_valid_grid_lanes_value"
            | "testValidGridLanes"
            | "test_grid_lanes_serialization"
            | "test_shorthand_roundtrip"
            | "test_shorthand_value"
            | "test_each_interpolation_method"
            | "test_valid_rule"
            | "test_invalid_rule"
            | "test_computed_value"
            | "test_various"
            | "test_valid"
            | "test_invalid"
    ) {
        return None;
    }

    Some((|| -> Result<Vec<CssParsingTest>, String> {
        match helper {
            // computed-testcommon.js helper with an extra pseudo-element argument.
            "test_pseudo_computed_value" => {
                let property = literal_argument(arguments, 1, helper)?;
                declaration_tests(CssParsingTestKind::ComputedValue, &property, &[2])
            }
            // Local font WPT helpers still assign each selected value through the
            // corresponding CSS declaration before making their relational check.
            "test_relative" => declaration_tests(CssParsingTestKind::ComputedValue, "font-weight", &[0, 1]),
            "test_relative_size" => declaration_tests(CssParsingTestKind::ComputedValue, "font-size", &[0, 1]),
            "test_font_size" => declaration_tests(CssParsingTestKind::ComputedValue, "font-size", &[1]),
            "test_system_font" => declaration_tests(CssParsingTestKind::ComputedValue, "font", &[0]),
            "test_auto_duration" => {
                let mut tests = declaration_tests(CssParsingTestKind::ComputedValue, "animation-duration", &[0])?;
                tests.extend(declaration_tests(CssParsingTestKind::ComputedValue, "animation-timeline", &[1])?);
                Ok(tests)
            }
            "test_currentcolor_alpha" => {
                let mut tests = declaration_tests(CssParsingTestKind::ComputedValue, "background-color", &[0])?;
                tests.extend(declaration_tests(CssParsingTestKind::ComputedValue, "color", &[2])?);
                Ok(tests)
            }
            "test_valid_rule" | "test_invalid_rule" => {
                let kind = if helper == "test_valid_rule" { CssParsingTestKind::ValidRule } else { CssParsingTestKind::InvalidRule };
                if let Some(argument_sets) = expand_static_arguments(arguments, call_offset, loops, 1) {
                    return Ok(argument_sets.into_iter().map(|arguments| CssParsingTest { kind, arguments, line }).collect());
                }
                let rules = collect_static_string_arrays(source).get("ruleTypes").cloned().ok_or_else(|| format!("{helper} argument is computed JavaScript rather than a static rule table"))?;
                Ok(rules
                    .into_iter()
                    .filter_map(|values| values.into_iter().next())
                    .map(|rule| {
                        let rule = if helper == "test_valid_rule" { format!("@page {{ {rule} {{ }} }}") } else { format!("{rule}{{ }}") };
                        CssParsingTest { kind, arguments: vec![rule], line }
                    })
                    .collect())
            }
            "test_computed_value" => {
                if let Some(argument_sets) = expand_static_arguments(arguments, call_offset, loops, 2) {
                    return Ok(argument_sets.into_iter().map(|arguments| CssParsingTest { kind: CssParsingTestKind::ComputedValue, arguments, line }).collect());
                }
                let property = literal_argument(arguments, 0, helper)?;
                if arguments.get(1).map(|argument| argument.trim()) != Some("test[0]") {
                    return Err("test_computed_value arguments are computed JavaScript rather than a static tuple table".to_owned());
                }
                let values = static_tuple_array_prefixes(source, "tests", 1).ok_or_else(|| "could not read the tests tuple table".to_owned())?;
                Ok(values.into_iter().map(|values| CssParsingTest { kind: CssParsingTestKind::ComputedValue, arguments: vec![property.clone(), values[0].clone()], line }).collect())
            }
            "test_keyframes_name_valid" | "test_keyframes_name_invalid" => {
                let name = literal_argument(arguments, 0, helper)?;
                let kind = if helper.ends_with("_valid") { CssParsingTestKind::ValidRule } else { CssParsingTestKind::InvalidRule };
                rule_test(kind, format!("@keyframes {name} {{}}"))
            }
            "test_valid_keyframe_selector" | "test_invalid_keyframe_selector" => {
                let selector = literal_argument(arguments, 0, helper)?;
                let kind = if helper.starts_with("test_valid") { CssParsingTestKind::ValidRule } else { CssParsingTestKind::InvalidRule };
                rule_test(kind, format!("@keyframes --wpt {{ {selector} {{}} }}"))
            }
            "test_display_computed" => {
                let object = arguments.first().ok_or_else(|| "test_display_computed requires a property object".to_owned())?;
                let value = object_literal_string_field(object, "display").ok_or_else(|| "test_display_computed display field is not a string literal".to_owned())?;
                Ok(vec![CssParsingTest { kind: CssParsingTestKind::ComputedValue, arguments: vec!["display".to_owned(), value], line }])
            }
            "test_display_affected" => {
                let property = literal_argument(arguments, 0, helper)?;
                let value = literal_argument(arguments, 1, helper)?;
                Ok(vec![CssParsingTest { kind: CssParsingTestKind::ComputedValue, arguments: vec![property, value], line }])
            }
            "fuzzy_test_valid_color" | "fuzzy_test_computed_color" | "fuzzy_test_computed_color_using_currentcolor" => {
                let kind = if helper == "fuzzy_test_valid_color" { CssParsingTestKind::ValidValue } else { CssParsingTestKind::ComputedValue };
                let property = if helper == "fuzzy_test_computed_color_using_currentcolor" { "background-color" } else { "color" };
                let values = expand_static_arguments(arguments, call_offset, loops, 1).ok_or_else(|| format!("{helper} input is not a static string expansion"))?;
                Ok(values.into_iter().map(|values| CssParsingTest { kind, arguments: vec![property.to_owned(), values[0].clone()], line }).collect())
            }
            "fuzzy_test_computed_color_property" => {
                let values = expand_static_arguments(arguments, call_offset, loops, 2).ok_or_else(|| "fuzzy_test_computed_color_property arguments are not a static string expansion".to_owned())?;
                Ok(values.into_iter().map(|arguments| CssParsingTest { kind: CssParsingTestKind::ComputedValue, arguments, line }).collect())
            }
            "test_valid_color_layers_value" => {
                if !source.contains("function test_valid_color_layers_value(parsed, expected)") {
                    return Err("test_valid_color_layers_value is not the known CSS Color 6 generator".to_owned());
                }
                let template = arguments
                    .first()
                    .map(|argument| argument.trim())
                    .and_then(|argument| argument.strip_prefix('`'))
                    .and_then(|argument| argument.strip_suffix('`'))
                    .ok_or_else(|| "test_valid_color_layers_value input is not a template literal".to_owned())?;
                if !template.contains("${blendMode}") {
                    return Err("test_valid_color_layers_value template does not contain the known blendMode binding".to_owned());
                }
                let modes = [
                    "",
                    "normal, ",
                    "multiply, ",
                    "screen, ",
                    "overlay, ",
                    "darken, ",
                    "lighten, ",
                    "color-dodge, ",
                    "color-burn, ",
                    "hard-light, ",
                    "soft-light, ",
                    "difference, ",
                    "exclusion, ",
                    "hue, ",
                    "saturation, ",
                    "color, ",
                    "luminosity, ",
                ];
                Ok(modes.into_iter().map(|mode| CssParsingTest { kind: CssParsingTestKind::ValidValue, arguments: vec!["color".to_owned(), template.replace("${blendMode}", mode)], line }).collect())
            }
            "test_inset_longhand" | "test_inset_shorthand" => {
                let property = literal_argument(arguments, 0, helper)?;
                let mut values = vec!["0deg", "calc(20deg)", "10"];
                if helper == "test_inset_shorthand" {
                    values.extend(["inherit auto", "inherit inherit"]);
                }
                Ok(values.into_iter().map(|value| CssParsingTest { kind: CssParsingTestKind::InvalidValue, arguments: vec![property.clone(), value.to_owned()], line }).collect())
            }
            "grid_template_columns" | "grid_template_rows" => {
                let property = if helper.ends_with("columns") { "grid-template-columns" } else { "grid-template-rows" };
                declaration_tests(CssParsingTestKind::ComputedValue, property, &[0])
            }
            "testGridDefinitionsSetJSValues" | "testGridDefinitionsSetBadJSValues" => {
                let kind = if helper.contains("Bad") { CssParsingTestKind::InvalidValue } else { CssParsingTestKind::ComputedValue };
                let columns = literal_argument(arguments, 0, helper)?;
                let rows = literal_argument(arguments, 1, helper)?;
                Ok(vec![CssParsingTest { kind, arguments: vec!["grid-template-columns".to_owned(), columns], line }, CssParsingTest { kind, arguments: vec!["grid-template-rows".to_owned(), rows], line }])
            }
            "testInherit" | "testInitial" => {
                let value = if helper == "testInherit" { "inherit" } else { "initial" };
                Ok(["grid-template-columns", "grid-template-rows"].into_iter().map(|property| CssParsingTest { kind: CssParsingTestKind::ComputedValue, arguments: vec![property.to_owned(), value.to_owned()], line }).collect())
            }
            "testValidGta" | "testInvalidGta" => {
                let kind = if helper == "testValidGta" { CssParsingTestKind::ValidValue } else { CssParsingTestKind::InvalidValue };
                declaration_tests(kind, "grid-template-areas", &[0])
            }
            "testValidGridTemplate" => {
                if source.contains("function testValidGridTemplate(valueGridTemplate, valueGridAreas") {
                    let template = literal_argument(arguments, 0, helper)?;
                    let areas = literal_argument(arguments, 1, helper)?;
                    Ok(vec![
                        CssParsingTest { kind: CssParsingTestKind::ValidValue, arguments: vec!["grid-template".to_owned(), template], line },
                        CssParsingTest { kind: CssParsingTestKind::ValidValue, arguments: vec!["grid-template-areas".to_owned(), areas], line },
                    ])
                } else if source.contains("function testValidGridTemplate(valueGridTemplateRows, valueGridTemplateColumns, valueGridAreas") {
                    let rows = literal_argument(arguments, 0, helper)?;
                    let columns = literal_argument(arguments, 1, helper)?;
                    let areas = literal_argument(arguments, 2, helper)?;
                    Ok(vec![
                        CssParsingTest { kind: CssParsingTestKind::ValidValue, arguments: vec!["grid-template-rows".to_owned(), rows], line },
                        CssParsingTest { kind: CssParsingTestKind::ValidValue, arguments: vec!["grid-template-columns".to_owned(), columns], line },
                        CssParsingTest { kind: CssParsingTestKind::ValidValue, arguments: vec!["grid-template-areas".to_owned(), areas], line },
                    ])
                } else {
                    Err("testValidGridTemplate has an unknown helper signature".to_owned())
                }
            }
            "test_valid_grid_lanes_value" => {
                let property = literal_argument(arguments, 0, helper)?;
                let value = literal_argument(arguments, 1, helper)?;
                Ok(vec![CssParsingTest { kind: CssParsingTestKind::ValidValue, arguments: vec![property, value], line }])
            }
            "testValidGridLanes" => {
                let properties = ["grid-template-rows", "grid-template-columns", "grid-template-areas", "grid-lanes-direction"];
                properties.into_iter().enumerate().map(|(index, property)| Ok(CssParsingTest { kind: CssParsingTestKind::ValidValue, arguments: vec![property.to_owned(), literal_argument(arguments, index, helper)?], line })).collect()
            }
            "test_grid_lanes_serialization" => {
                let callback = arguments.get(1).ok_or_else(|| "test_grid_lanes_serialization requires a setup callback".to_owned())?;
                let assignments = static_style_assignments(callback);
                if assignments.is_empty() {
                    return Err("test_grid_lanes_serialization callback has no static style assignments".to_owned());
                }
                Ok(assignments.into_iter().map(|(property, value)| CssParsingTest { kind: CssParsingTestKind::ComputedValue, arguments: vec![property, value], line }).collect())
            }
            "test_shorthand_roundtrip" => {
                if !source.contains("function test_shorthand_roundtrip(cssText, properties, declarations)") {
                    return Err("test_shorthand_roundtrip is not the known grid CSSOM helper".to_owned());
                }
                let declarations = literal_argument(arguments, 0, helper)?;
                rule_test(CssParsingTestKind::ValidRule, format!("* {{ {declarations} }}"))
            }
            "test_shorthand_value" => {
                if let Some(argument_sets) = expand_static_arguments(arguments, call_offset, loops, 2) {
                    return Ok(argument_sets.into_iter().map(|arguments| CssParsingTest { kind: CssParsingTestKind::ShorthandValue, arguments, line }).collect());
                }
                expand_gap_shorthand_matrix(source, call_offset, line)
            }
            "test_each_interpolation_method" => expand_gradient_interpolation_methods(source, arguments, line),
            // font-valid.html and font-computed.html deliberately generate all
            // permutations recursively. Reproduce that finite data generator
            // so the upstream files remain unchanged and no JS engine is needed.
            "test_various" => expand_font_various(source, arguments, line),
            // This helper exists only in starting-style-parsing.html and inserts
            // the supplied at-rule prefix followed by an empty block.
            "test_valid" | "test_invalid" => {
                let prefix = literal_argument(arguments, 0, helper)?;
                let kind = if helper == "test_valid" { CssParsingTestKind::ValidRule } else { CssParsingTestKind::InvalidRule };
                rule_test(kind, format!("{prefix}{{}}"))
            }
            _ => unreachable!("custom helper guard and expansion match stay synchronized"),
        }
    })())
}

fn expand_gap_shorthand_matrix(source: &str, call_offset: usize, line: usize) -> Result<Vec<CssParsingTest>, String> {
    if !source.contains("const rule_properties = {") {
        return Err("test_shorthand_value arguments are not a static literal expansion".to_owned());
    }
    let prefix = &source[..call_offset];
    let array_name = prefix
        .rfind("const test_cases =")
        .and_then(|offset| {
            let assignment = &prefix[offset + "const test_cases =".len()..];
            assignment.split(';').next().map(str::trim)
        })
        .filter(|name| !name.is_empty())
        .unwrap_or("testCases");
    let inputs = static_object_array_string_fields(source, array_name, "input").ok_or_else(|| format!("could not read {array_name} input table"))?;
    let mut properties = static_object_keys(source, "rule_properties").ok_or_else(|| "could not read rule_properties table".to_owned())?;

    let nearby = &source[call_offset.saturating_sub(500)..call_offset];
    if let Some(condition) = nearby.rfind("if (rule_property === 'rule')") {
        if nearby.rfind("} else {").is_some_and(|otherwise| otherwise > condition) {
            properties.retain(|property| property != "rule");
        } else {
            properties.retain(|property| property == "rule");
        }
    }
    if properties.is_empty() || inputs.is_empty() {
        return Err("gap shorthand matrix contains no static property/value pairs".to_owned());
    }
    Ok(properties.into_iter().flat_map(|property| inputs.iter().cloned().map(move |value| CssParsingTest { kind: CssParsingTestKind::ShorthandValue, arguments: vec![property.clone(), value], line })).collect())
}

fn static_object_array_string_fields(source: &str, name: &str, field: &str) -> Option<Vec<String>> {
    let body = static_assignment_body(source, name, b'[', b']')?;
    let mut values = Vec::new();
    for item in split_js_values(body)? {
        let item = item.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        if let Some(value) = object_literal_string_field(item.trim(), field) {
            values.push(value);
        }
    }
    Some(values)
}

fn static_tuple_array_prefixes(source: &str, name: &str, required: usize) -> Option<Vec<Vec<String>>> {
    let body = static_assignment_body(source, name, b'[', b']')?;
    let mut tuples = Vec::new();
    for item in split_js_values(body)? {
        let item = item.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        let body = item.trim().strip_prefix('[')?.strip_suffix(']')?;
        let members = split_js_values(body)?;
        if members.len() < required {
            return None;
        }
        tuples.push(members[..required].iter().map(|member| parse_js_string_literal(member.trim())).collect::<Option<Vec<_>>>()?);
    }
    Some(tuples)
}

fn static_object_keys(source: &str, name: &str) -> Option<Vec<String>> {
    let body = static_assignment_body(source, name, b'{', b'}')?;
    let mut keys = Vec::new();
    for item in split_js_values(body)? {
        let item = item.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        let (key, _) = item.trim().split_once(':')?;
        keys.push(parse_js_string_literal(key.trim()).unwrap_or_else(|| key.trim().to_owned()));
    }
    Some(keys)
}

fn static_assignment_body<'a>(source: &'a str, name: &str, open: u8, close: u8) -> Option<&'a str> {
    let assignment = source.find(&format!("{name} ="))?;
    let open_at = source.as_bytes()[assignment..].iter().position(|byte| *byte == open)? + assignment;
    let close_at = find_matching_delimiter(source, open_at, open, close)?;
    source.get(open_at + 1..close_at)
}

fn static_style_assignments(source: &str) -> Vec<(String, String)> {
    let bytes = source.as_bytes();
    let mut assignments = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = source[cursor..].find(".style.") {
        cursor += offset + ".style.".len();
        let property_start = cursor;
        while bytes.get(cursor).is_some_and(|byte| is_js_identifier_continue(*byte)) {
            cursor += 1;
        }
        let Some(property) = source.get(property_start..cursor).filter(|property| !property.is_empty()) else {
            continue;
        };
        skip_ascii_whitespace(bytes, &mut cursor);
        if bytes.get(cursor) != Some(&b'=') {
            continue;
        }
        cursor += 1;
        skip_ascii_whitespace(bytes, &mut cursor);
        let value_start = cursor;
        let Some(quote @ (b'\'' | b'"' | b'`')) = bytes.get(cursor).copied() else {
            continue;
        };
        let mut ignored_line = 0usize;
        skip_js_string(bytes, &mut cursor, &mut ignored_line, quote);
        let Some(value) = source.get(value_start..cursor).and_then(parse_js_string_literal) else {
            continue;
        };
        let property = property.chars().fold(String::new(), |mut output, character| {
            if character.is_ascii_uppercase() {
                output.push('-');
                output.push(character.to_ascii_lowercase());
            } else {
                output.push(character);
            }
            output
        });
        assignments.push((property, value));
    }
    assignments
}

fn expand_gradient_interpolation_methods(source: &str, arguments: &[&str], line: usize) -> Result<Vec<CssParsingTest>, String> {
    let gradient = literal_argument(arguments, 0, "test_each_interpolation_method")?;
    let specifiers: &[&str] = match gradient.as_str() {
        "linear-gradient" => &["30deg", "to right bottom"],
        "radial-gradient" => &["50px", "ellipse 50% 40em", "at right center"],
        "conic-gradient" => &["from 30deg", "at left 10px top 50em"],
        _ => return Err("test_each_interpolation_method has an unknown gradient function".to_owned()),
    };
    let kind = if source.contains("function test_each_interpolation_method(gradientFunction) {") && source.contains("test_invalid_value(`background-image`") {
        CssParsingTestKind::InvalidValue
    } else if source.contains("test_computed_value(`background-image`") {
        CssParsingTestKind::ComputedValue
    } else if source.contains("test_valid_value(`background-image`") {
        CssParsingTestKind::ValidValue
    } else {
        return Err("test_each_interpolation_method is not a known gradient parsing generator".to_owned());
    };
    let mut tests = Vec::new();
    let mut push = |value: String| tests.push(CssParsingTest { kind, arguments: vec!["background-image".to_owned(), value], line });

    if kind == CssParsingTestKind::InvalidValue {
        push(format!("{gradient}(, red, blue)"));
        for color_space in ["lab", "oklab", "srgb", "srgb-linear", "xyz", "xyz-d50", "xyz-d65"] {
            push(format!("{gradient}(red, blue, {color_space})"));
            push(format!("{gradient}({color_space} {color_space}, red, blue)"));
            push(format!("{gradient}({color_space} shorter hue, red, blue)"));
        }
        for color_space in ["hsl", "hwb", "lch", "oklch"] {
            push(format!("{gradient}({color_space} foo hue, red, blue)"));
            push(format!("{gradient}({color_space} hue, red, blue)"));
            push(format!("{gradient}({color_space} {color_space}, red, blue)"));
            for hue in ["shorter", "longer", "increasing", "decreasing", "specified"] {
                push(format!("{gradient}({color_space} {hue}, red, blue)"));
                push(format!("{gradient}({hue} hue {color_space}, red, blue)"));
                push(format!("{gradient}(red, blue, {color_space} {hue} hue)"));
            }
        }
        return Ok(tests);
    }

    let stops: &[&str] = if kind == CssParsingTestKind::ComputedValue { &["red, blue", "color(srgb 1 0 0), blue"] } else { &["red, blue", "red, 50%, blue", "color(srgb 1 0 0), blue"] };
    for stops in stops {
        for specifier in specifiers {
            push(format!("{gradient}({specifier}, {stops})"));
        }
    }
    let mut methods = vec!["lab".to_owned(), "oklab".to_owned(), "srgb".to_owned(), "srgb-linear".to_owned(), "xyz".to_owned(), "xyz-d50".to_owned(), "xyz-d65".to_owned()];
    for color_space in ["hsl", "hwb", "lch", "oklch"] {
        for hue in ["", " shorter hue", " longer hue", " increasing hue", " decreasing hue"] {
            methods.push(format!("{color_space}{hue}"));
        }
    }
    for method in methods {
        for stops in stops {
            push(format!("{gradient}(in {method}, {stops})"));
            for specifier in specifiers {
                push(format!("{gradient}({specifier} in {method}, {stops})"));
                push(format!("{gradient}(in {method} {specifier}, {stops})"));
            }
        }
    }
    Ok(tests)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FontPrefixPart {
    Normal,
    Style,
    Variant,
    Weight,
    Stretch,
}

#[derive(Default)]
struct FontGeneratorCounters {
    style: usize,
    weight: usize,
    stretch: usize,
    size: usize,
    line_height: usize,
    family: usize,
}

fn expand_font_various(source: &str, arguments: &[&str], line: usize) -> Result<Vec<CssParsingTest>, String> {
    if arguments.len() != 1 || arguments[0].trim() != "[]" {
        return Err("test_various is only supported for the literal empty-prefix entry point".to_owned());
    }
    let kind = if source.contains("test_valid_value('font', parts.join(' ')") {
        CssParsingTestKind::ValidValue
    } else if source.contains("test_computed_value('font', parts.join(' ')") {
        CssParsingTestKind::ComputedValue
    } else {
        return Err("test_various is not the known finite font shorthand generator".to_owned());
    };
    let computed = kind == CssParsingTestKind::ComputedValue;
    let mut tests = Vec::new();
    let mut counters = FontGeneratorCounters::default();
    generate_font_permutations(&mut Vec::new(), computed, kind, line, &mut counters, &mut tests);
    Ok(tests)
}

fn generate_font_permutations(prefix: &mut Vec<FontPrefixPart>, computed: bool, kind: CssParsingTestKind, line: usize, counters: &mut FontGeneratorCounters, tests: &mut Vec<CssParsingTest>) {
    fn alternating<'a>(values: &'a [&str], counter: &mut usize) -> &'a str {
        let value = values[*counter % values.len()];
        *counter += 1;
        value
    }
    let mut parts = Vec::new();
    for entry in prefix.iter().copied() {
        let value = match entry {
            FontPrefixPart::Normal => "normal",
            FontPrefixPart::Style => alternating(if computed { &["italic"] } else { &["italic", "oblique"] }, &mut counters.style),
            FontPrefixPart::Variant => "small-caps",
            FontPrefixPart::Weight => alternating(&["bold", "bolder", "lighter", "100", "900"], &mut counters.weight),
            FontPrefixPart::Stretch => alternating(&["ultra-condensed", "extra-condensed", "condensed", "semi-condensed", "semi-expanded", "expanded", "extra-expanded", "ultra-expanded"], &mut counters.stretch),
        };
        parts.push(value.to_owned());
    }
    let size = alternating(&["xx-small", "medium", "xx-large", "larger", "smaller", "10px", "20%", "calc(30% - 40px)"], &mut counters.size);
    let line_height = alternating(&["", "normal", "1.2", "calc(120% + 1.2em)"], &mut counters.line_height);
    parts.push(if line_height.is_empty() { size.to_owned() } else { format!("{size}/{line_height}") });
    let families = if computed { &["serif", "sans-serif", "cursive", "fantasy", "monospace", "Menu", "Non-Generic Example Family Name"][..] } else { &["serif", "sans-serif", "cursive", "fantasy", "monospace", "Menu", "FB Armada"][..] };
    parts.push(alternating(families, &mut counters.family).to_owned());
    tests.push(CssParsingTest { kind, arguments: vec!["font".to_owned(), parts.join(" ")], line });

    if prefix.len() == 4 {
        return;
    }
    for alternative in [FontPrefixPart::Normal, FontPrefixPart::Style, FontPrefixPart::Variant, FontPrefixPart::Weight, FontPrefixPart::Stretch] {
        if alternative == FontPrefixPart::Normal || !prefix.contains(&alternative) {
            prefix.push(alternative);
            generate_font_permutations(prefix, computed, kind, line, counters, tests);
            prefix.pop();
        }
    }
}

fn literal_argument(arguments: &[&str], index: usize, helper: &str) -> Result<String, String> {
    arguments.get(index).and_then(|argument| parse_js_string_literal(argument)).ok_or_else(|| format!("{helper} argument {} is not a string literal", index + 1))
}

fn object_literal_string_field(object: &str, requested: &str) -> Option<String> {
    let object = object.trim();
    let body = object.strip_prefix('{')?.strip_suffix('}')?;
    for field in split_js_values(body)? {
        let (name, value) = field.split_once(':')?;
        let name = parse_js_string_literal(name.trim()).unwrap_or_else(|| name.trim().to_owned());
        if name == requested {
            return parse_js_string_literal(value.trim());
        }
    }
    None
}

fn skip_js_function_body(source: &str, cursor: &mut usize, line: &mut usize) {
    let bytes = source.as_bytes();
    let Some(open) = bytes.get(*cursor..).and_then(|tail| tail.iter().position(|byte| *byte == b'{')).map(|offset| *cursor + offset) else {
        return;
    };
    let Some(close) = find_matching_delimiter(source, open, b'{', b'}') else {
        return;
    };
    *line += bytes[open..=close].iter().filter(|byte| **byte == b'\n').count();
    *cursor = close + 1;
}

fn find_matching_delimiter(source: &str, open_at: usize, open: u8, close: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open_at) != Some(&open) {
        return None;
    }
    let mut cursor = open_at + 1;
    let mut depth = 1usize;
    let mut ignored_line = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' | b'`' => {
                let quote = bytes[cursor];
                skip_js_string(bytes, &mut cursor, &mut ignored_line, quote);
            }
            byte if byte == open => {
                depth += 1;
                cursor += 1;
            }
            byte if byte == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor);
                }
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }
    None
}

fn split_js_values(source: &str) -> Option<Vec<&str>> {
    let bytes = source.as_bytes();
    let mut cursor = 0;
    let mut start = 0;
    let mut values = Vec::new();
    let mut ignored_line = 0;
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' | b'`' => {
                let quote = bytes[cursor];
                skip_js_string(bytes, &mut cursor, &mut ignored_line, quote);
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'/') => {
                cursor += 2;
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor += 2;
                while cursor + 1 < bytes.len() && !(bytes[cursor] == b'*' && bytes[cursor + 1] == b'/') {
                    cursor += 1;
                }
                cursor = (cursor + 2).min(bytes.len());
            }
            b'(' => {
                parentheses += 1;
                cursor += 1;
            }
            b')' => {
                parentheses = parentheses.saturating_sub(1);
                cursor += 1;
            }
            b'[' => {
                brackets += 1;
                cursor += 1;
            }
            b']' => {
                brackets = brackets.saturating_sub(1);
                cursor += 1;
            }
            b'{' => {
                braces += 1;
                cursor += 1;
            }
            b'}' => {
                braces = braces.saturating_sub(1);
                cursor += 1;
            }
            b',' if parentheses == 0 && brackets == 0 && braces == 0 => {
                values.push(source[start..cursor].trim());
                cursor += 1;
                start = cursor;
            }
            _ => cursor += 1,
        }
    }
    let final_value = source[start..].trim();
    if !final_value.is_empty() {
        values.push(final_value);
    }
    Some(values)
}

fn skip_ascii_whitespace(bytes: &[u8], cursor: &mut usize) {
    while bytes.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
        *cursor += 1;
    }
}

fn expect_byte(bytes: &[u8], cursor: &mut usize, expected: u8) -> Option<()> {
    (bytes.get(*cursor) == Some(&expected)).then(|| *cursor += 1)
}

fn advance_js_token(bytes: &[u8], cursor: &mut usize) {
    let mut ignored_line = 0;
    match bytes.get(*cursor).copied() {
        Some(quote @ (b'\'' | b'"' | b'`')) => skip_js_string(bytes, cursor, &mut ignored_line, quote),
        Some(b'/') if bytes.get(*cursor + 1) == Some(&b'/') => {
            *cursor += 2;
            while *cursor < bytes.len() && bytes[*cursor] != b'\n' {
                *cursor += 1;
            }
        }
        Some(b'/') if bytes.get(*cursor + 1) == Some(&b'*') => {
            *cursor += 2;
            while *cursor + 1 < bytes.len() && !(bytes[*cursor] == b'*' && bytes[*cursor + 1] == b'/') {
                *cursor += 1;
            }
            *cursor = (*cursor + 2).min(bytes.len());
        }
        Some(_) => *cursor += 1,
        None => {}
    }
}

fn css_test_kind(helper: &str) -> Option<CssParsingTestKind> {
    Some(match helper {
        "test_valid_value" => CssParsingTestKind::ValidValue,
        "test_invalid_value" => CssParsingTestKind::InvalidValue,
        "test_computed_value" => CssParsingTestKind::ComputedValue,
        "verifyComputedStyle" => CssParsingTestKind::ComputedValue,
        "test_valid_and_computed_value" => CssParsingTestKind::ValidAndComputedValue,
        "test_shorthand_value" => CssParsingTestKind::ShorthandValue,
        "test_valid_selector" => CssParsingTestKind::ValidSelector,
        "test_invalid_selector" => CssParsingTestKind::InvalidSelector,
        "assert_selector_serializes_to" => CssParsingTestKind::ValidSelector,
        "assert_invalid_selector" => CssParsingTestKind::InvalidSelector,
        "test_valid_forgiving_selector" => CssParsingTestKind::ValidForgivingSelector,
        "test_valid_rule" => CssParsingTestKind::ValidRule,
        "test_invalid_rule" => CssParsingTestKind::InvalidRule,
        "test_valid_layer_import" => CssParsingTestKind::ValidLayerImport,
        "test_invalid_layer_import" => CssParsingTestKind::InvalidLayerImport,
        "test_valid_supports_import" => CssParsingTestKind::ValidSupportsImport,
        "test_invalid_supports_import" => CssParsingTestKind::InvalidSupportsImport,
        "test_unsupported_supports_import" => CssParsingTestKind::UnsupportedSupportsImport,
        _ => return None,
    })
}

fn required_argument_count(kind: CssParsingTestKind) -> usize {
    match kind {
        CssParsingTestKind::ValidValue | CssParsingTestKind::InvalidValue | CssParsingTestKind::ComputedValue | CssParsingTestKind::ValidAndComputedValue | CssParsingTestKind::ShorthandValue => 2,
        CssParsingTestKind::ValidSelector
        | CssParsingTestKind::InvalidSelector
        | CssParsingTestKind::ValidForgivingSelector
        | CssParsingTestKind::ValidRule
        | CssParsingTestKind::InvalidRule
        | CssParsingTestKind::ValidLayerImport
        | CssParsingTestKind::InvalidLayerImport
        | CssParsingTestKind::ValidSupportsImport
        | CssParsingTestKind::InvalidSupportsImport
        | CssParsingTestKind::UnsupportedSupportsImport => 1,
    }
}

fn is_js_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

fn is_js_identifier_continue(byte: u8) -> bool {
    is_js_identifier_start(byte) || byte.is_ascii_digit()
}

fn preceding_word(source: &str, start: usize) -> Option<&str> {
    let bytes = source.as_bytes();
    let mut end = start;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut word_start = end;
    while word_start > 0 && is_js_identifier_continue(bytes[word_start - 1]) {
        word_start -= 1;
    }
    (word_start < end).then_some(&source[word_start..end])
}

fn skip_js_string(bytes: &[u8], cursor: &mut usize, line: &mut usize, quote: u8) {
    *cursor += 1;
    while *cursor < bytes.len() {
        match bytes[*cursor] {
            b'\\' => {
                *cursor += 1;
                if bytes.get(*cursor) == Some(&b'\n') {
                    *line += 1;
                }
                *cursor = (*cursor + 1).min(bytes.len());
            }
            byte if byte == quote => {
                *cursor += 1;
                return;
            }
            b'\n' => {
                *line += 1;
                *cursor += 1;
            }
            _ => *cursor += 1,
        }
    }
}

fn collect_js_call_arguments<'a>(source: &'a str, open: usize, line: &mut usize) -> Result<(Vec<&'a str>, usize), String> {
    let bytes = source.as_bytes();
    let mut cursor = open + 1;
    let mut argument_start = cursor;
    let mut arguments = Vec::new();
    let mut parentheses = 1usize;
    let mut brackets = 0usize;
    let mut braces = 0usize;

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\'' | b'"' | b'`' => {
                let quote = bytes[cursor];
                skip_js_string(bytes, &mut cursor, line, quote);
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'/') => {
                cursor += 2;
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor += 2;
                while cursor + 1 < bytes.len() && !(bytes[cursor] == b'*' && bytes[cursor + 1] == b'/') {
                    if bytes[cursor] == b'\n' {
                        *line += 1;
                    }
                    cursor += 1;
                }
                cursor = (cursor + 2).min(bytes.len());
            }
            b'(' => {
                parentheses += 1;
                cursor += 1;
            }
            b')' => {
                parentheses -= 1;
                if parentheses == 0 {
                    let final_argument = source[argument_start..cursor].trim();
                    if !final_argument.is_empty() {
                        arguments.push(final_argument);
                    }
                    return Ok((arguments, cursor + 1));
                }
                cursor += 1;
            }
            b'[' => {
                brackets += 1;
                cursor += 1;
            }
            b']' => {
                brackets = brackets.saturating_sub(1);
                cursor += 1;
            }
            b'{' => {
                braces += 1;
                cursor += 1;
            }
            b'}' => {
                braces = braces.saturating_sub(1);
                cursor += 1;
            }
            b',' if parentheses == 1 && brackets == 0 && braces == 0 => {
                arguments.push(source[argument_start..cursor].trim());
                cursor += 1;
                argument_start = cursor;
            }
            b'\n' => {
                *line += 1;
                cursor += 1;
            }
            _ => cursor += 1,
        }
    }

    Err("unterminated helper call".to_owned())
}

fn parse_js_string_literal(argument: &str) -> Option<String> {
    let bytes = argument.as_bytes();
    let quote = *bytes.first()?;
    if !matches!(quote, b'\'' | b'"' | b'`') || bytes.last().copied()? != quote || (quote == b'`' && argument.contains("${")) {
        return None;
    }

    let mut output = String::new();
    let mut cursor = 1;
    while cursor + 1 < bytes.len() {
        if bytes[cursor] != b'\\' {
            if bytes[cursor] == quote {
                return None;
            }
            let character = argument[cursor..bytes.len() - 1].chars().next()?;
            output.push(character);
            cursor += character.len_utf8();
            continue;
        }

        cursor += 1;
        let escaped = *bytes.get(cursor)?;
        cursor += 1;
        match escaped {
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'b' => output.push('\u{0008}'),
            b'f' => output.push('\u{000c}'),
            b'v' => output.push('\u{000b}'),
            b'0' => output.push('\0'),
            b'\n' => {}
            b'x' => {
                let value = u8::from_str_radix(std::str::from_utf8(bytes.get(cursor..cursor + 2)?).ok()?, 16).ok()?;
                output.push(char::from(value));
                cursor += 2;
            }
            b'u' => {
                let digits = std::str::from_utf8(bytes.get(cursor..cursor + 4)?).ok()?;
                output.push(char::from_u32(u32::from_str_radix(digits, 16).ok()?)?);
                cursor += 4;
            }
            other => output.push(char::from(other)),
        }
    }
    Some(output)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScriptingMode {
    Both,
    Off,
    On,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeConstructionCase {
    pub data: String,
    pub errors: Vec<String>,
    pub new_errors: Vec<String>,
    pub fragment_context: Option<String>,
    pub scripting: ScriptingMode,
    pub document: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeConstructionParseError {
    pub case_index: usize,
    pub message: String,
}

impl fmt::Display for TreeConstructionParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "WPT tree-construction case {}: {}", self.case_index, self.message)
    }
}

impl std::error::Error for TreeConstructionParseError {}

/// Parse the original html5lib/WPT tree-construction `.dat` format.
///
/// Test data and expected document dumps are retained verbatim apart from the
/// section-terminating newline specified by the upstream format.
pub fn parse_tree_construction_file(source: &str) -> Result<Vec<TreeConstructionCase>, TreeConstructionParseError> {
    let lines: Vec<&str> = source.lines().collect();
    let mut cursor = 0;
    let mut cases = Vec::new();

    while cursor < lines.len() {
        while cursor < lines.len() && lines[cursor].is_empty() {
            cursor += 1;
        }
        if cursor == lines.len() {
            break;
        }

        let case_index = cases.len() + 1;
        expect_marker(&lines, &mut cursor, "#data", case_index)?;
        let data = collect_until(&lines, &mut cursor, "#errors", case_index)?.join("\n");
        cursor += 1;

        let mut errors = Vec::new();
        while cursor < lines.len() && !is_control_marker(lines[cursor]) {
            if !lines[cursor].is_empty() {
                errors.push(lines[cursor].to_owned());
            }
            cursor += 1;
        }
        let mut new_errors = Vec::new();
        if cursor < lines.len() && lines[cursor] == "#new-errors" {
            cursor += 1;
            while cursor < lines.len() && !is_control_marker(lines[cursor]) {
                if !lines[cursor].is_empty() {
                    new_errors.push(lines[cursor].to_owned());
                }
                cursor += 1;
            }
        }

        let fragment_context = if cursor < lines.len() && lines[cursor] == "#document-fragment" {
            cursor += 1;
            let context = lines.get(cursor).ok_or_else(|| parse_error(case_index, "missing fragment context"))?.to_string();
            cursor += 1;
            Some(context)
        } else {
            None
        };

        let scripting = match lines.get(cursor).copied() {
            Some("#script-off") => {
                cursor += 1;
                ScriptingMode::Off
            }
            Some("#script-on") => {
                cursor += 1;
                ScriptingMode::On
            }
            _ => ScriptingMode::Both,
        };

        expect_marker(&lines, &mut cursor, "#document", case_index)?;
        let document_start = cursor;
        while cursor < lines.len() && lines[cursor] != "#data" {
            cursor += 1;
        }
        let mut document_end = cursor;
        while document_end > document_start && lines[document_end - 1].is_empty() {
            document_end -= 1;
        }
        if document_start == document_end {
            return Err(parse_error(case_index, "missing expected document dump"));
        }

        cases.push(TreeConstructionCase { data, errors, new_errors, fragment_context, scripting, document: lines[document_start..document_end].join("\n") });
    }

    Ok(cases)
}

fn collect_until<'a>(lines: &'a [&str], cursor: &mut usize, marker: &str, case_index: usize) -> Result<Vec<&'a str>, TreeConstructionParseError> {
    let start = *cursor;
    while *cursor < lines.len() && lines[*cursor] != marker {
        *cursor += 1;
    }
    if *cursor == lines.len() {
        return Err(parse_error(case_index, &format!("missing {marker} section")));
    }
    Ok(lines[start..*cursor].to_vec())
}

fn expect_marker(lines: &[&str], cursor: &mut usize, marker: &str, case_index: usize) -> Result<(), TreeConstructionParseError> {
    match lines.get(*cursor).copied() {
        Some(actual) if actual == marker => {
            *cursor += 1;
            Ok(())
        }
        Some(actual) => Err(parse_error(case_index, &format!("expected {marker}, found {actual:?}"))),
        None => Err(parse_error(case_index, &format!("expected {marker}, found end of file"))),
    }
}

fn is_control_marker(line: &str) -> bool {
    matches!(line, "#new-errors" | "#document-fragment" | "#script-off" | "#script-on" | "#document")
}

fn parse_error(case_index: usize, message: &str) -> TreeConstructionParseError {
    TreeConstructionParseError { case_index, message: message.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::{
        CssParsingTestKind, ReftestFuzzyTolerance, ReftestReference, ReftestRelation, ScriptingMode, extract_css_parsing_tests, extract_css_parsing_tests_with_prelude, extract_reftest_fuzzy_tolerances, extract_reftest_references,
        parse_tree_construction_file, relative_script_references,
    };

    #[test]
    fn extracts_match_and_mismatch_references() {
        let references = extract_reftest_references(
            r#"
                <link href='same.html' rel='help match'>
                <LINK REL=mismatch HREF="different.xhtml">
                <link rel=match href=../../reference.html>
                <link rel=match href="trimmed.html ">
                <link rel=help href="spec.html"
                <link rel=match href="recovered.html">
            "#,
        );
        assert_eq!(
            references,
            [
                ReftestReference { relation: ReftestRelation::Match, href: "same.html".to_owned() },
                ReftestReference { relation: ReftestRelation::Mismatch, href: "different.xhtml".to_owned() },
                ReftestReference { relation: ReftestRelation::Match, href: "../../reference.html".to_owned() },
                ReftestReference { relation: ReftestRelation::Match, href: "trimmed.html".to_owned() },
                ReftestReference { relation: ReftestRelation::Match, href: "recovered.html".to_owned() },
            ]
        );
    }

    #[test]
    fn extracts_document_and_reference_scoped_fuzzy_tolerances() {
        let tolerances = extract_reftest_fuzzy_tolerances(
            r#"
                <meta name="fuzzy" content="maxDifference=0-2; totalPixels=0-100">
                <META CONTENT='same.html:maxDifference=3;totalPixels=4-5' NAME=fuzzy>
            "#,
        );
        assert_eq!(
            tolerances,
            [ReftestFuzzyTolerance { reference: None, maximum_channel_delta: 2, maximum_different_pixels: 100 }, ReftestFuzzyTolerance { reference: Some("same.html".to_owned()), maximum_channel_delta: 3, maximum_different_pixels: 5 },]
        );
    }

    #[test]
    fn extracts_literal_css_helpers_and_reports_dynamic_calls() {
        let source = r#"
            test_valid_value("margin", "10px");
            // test_invalid_value("ignored", "comment")
            test_computed_value('white-space', 'pre\nwrap', "pre wrap");
            test_invalid_value(property, "bad");
        "#;
        let extraction = extract_css_parsing_tests(source);

        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(extraction.tests[0].kind, CssParsingTestKind::ValidValue);
        assert_eq!(extraction.tests[0].arguments, ["margin", "10px"]);
        assert_eq!(extraction.tests[1].kind, CssParsingTestKind::ComputedValue);
        assert_eq!(extraction.tests[1].arguments, ["white-space", "pre\nwrap"]);
        assert_eq!(extraction.unsupported_calls.len(), 1);
        assert_eq!(extraction.unsupported_calls[0].helper, "test_invalid_value");
    }

    #[test]
    fn expands_literal_selector_generators_without_javascript() {
        let source = r#"
            function assert_valid(valid, pattern, expected_pattern, description) {
                test_valid_selector(pattern, expected_pattern);
            }
            assert_valid(true, "{}(div )", "{}(div)", "trailing whitespace");
            assert_valid(false, "{}(::before)", null, "forgiving selector");
        "#;
        let extraction = extract_css_parsing_tests(source);

        assert_eq!(extraction.unsupported_calls, []);
        assert_eq!(extraction.tests.len(), 4);
        assert_eq!(extraction.tests[0].kind, CssParsingTestKind::ValidSelector);
        assert_eq!(extraction.tests[0].arguments[0], ":is(div )");
        assert_eq!(extraction.tests[1].arguments[0], ":where(div )");
        assert_eq!(extraction.tests[2].kind, CssParsingTestKind::ValidForgivingSelector);
    }

    #[test]
    fn expands_static_keyword_matrices_and_ignores_optional_serialization_arrays() {
        let source = r#"
            const first_values = ['none', 'auto'];
            const second_values = ['trim-start', 'space-first'];
            const combined = [...first_values, 'normal'];
            for (const first of first_values) {
                for (const second of second_values) {
                    test_invalid_value("text-spacing", `${first} ${second}`);
                }
            }
            for (const value of combined) {
                test_valid_value("text-spacing", value);
            }
            test_valid_selector('*#id', ['*#id', '#id']);
        "#;
        let extraction = extract_css_parsing_tests(source);

        assert_eq!(extraction.unsupported_calls, []);
        assert_eq!(extraction.tests.len(), 8);
        assert_eq!(extraction.tests[0].arguments, ["text-spacing", "none trim-start"]);
        assert_eq!(extraction.tests[3].arguments, ["text-spacing", "auto space-first"]);
        assert_eq!(extraction.tests[4].arguments, ["text-spacing", "none"]);
        assert_eq!(extraction.tests[6].arguments, ["text-spacing", "normal"]);
        assert_eq!(extraction.tests[7].arguments, ["*#id"]);
    }

    #[test]
    fn loads_relative_script_data_as_context_without_emitting_its_calls() {
        let html = r#"
            <script src="values.js"></script>
            <script src="/resources/testharness.js"></script>
            <script>
              for (const value of values) {
                test_valid_value("width", value);
              }
            </script>
        "#;
        let prelude = r#"const values = ["1px", "2px"]; test_invalid_value("width", "bad");"#;
        assert_eq!(relative_script_references(html), ["values.js"]);

        let extraction = extract_css_parsing_tests_with_prelude(html, prelude);
        assert_eq!(extraction.unsupported_calls, []);
        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(extraction.tests[0].arguments, ["width", "1px"]);
        assert_eq!(extraction.tests[1].arguments, ["width", "2px"]);
    }

    #[test]
    fn expands_simple_declarative_wrapper_functions() {
        let source = r#"
            function pair(property, value) {
                test_valid_value(property, value);
                test_valid_value(property, `${value} extra`);
            }
            pair("content", "open-quote");
        "#;
        let extraction = extract_css_parsing_tests(source);

        assert_eq!(extraction.unsupported_calls, []);
        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(extraction.tests[0].arguments, ["content", "open-quote"]);
        assert_eq!(extraction.tests[1].arguments, ["content", "open-quote extra"]);
    }

    #[test]
    fn expands_static_string_concatenation_in_selector_wrappers() {
        let source = r#"
            function run(source) {
                assert_selector_serializes_to(source + '(n)', source + '(n)');
                assert_invalid_selector(source + '(n-b1)');
            }
            run(':nth-child');
        "#;
        let extraction = extract_css_parsing_tests(source);

        assert_eq!(extraction.unsupported_calls, []);
        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(extraction.tests[0].kind, CssParsingTestKind::ValidSelector);
        assert_eq!(extraction.tests[0].arguments, [":nth-child(n)"]);
        assert_eq!(extraction.tests[1].kind, CssParsingTestKind::InvalidSelector);
        assert_eq!(extraction.tests[1].arguments, [":nth-child(n-b1)"]);
    }

    #[test]
    fn distinguishes_javascript_concatenation_from_one_literal() {
        assert_eq!(super::parse_js_string_literal(r#""red" + "blue""#), None);
        let extraction = extract_css_parsing_tests(r#"test_valid_value("color", "red" + "blue");"#);
        assert_eq!(extraction.unsupported_calls, []);
        assert_eq!(extraction.tests[0].arguments, ["color", "redblue"]);
    }

    #[test]
    fn expands_the_margin_at_rule_table() {
        let source = r#"
            const ruleTypes = ["@top-left", "@bottom-right"];
            for (let t in ruleTypes) {
                test_invalid_rule(ruleTypes[t] + "{ }");
                test_valid_rule("@page { " + ruleTypes[t] + " { } }");
            }
        "#;
        let extraction = extract_css_parsing_tests(source);

        assert_eq!(extraction.unsupported_calls, []);
        assert_eq!(extraction.tests.len(), 4);
        assert_eq!(extraction.tests[0].arguments, ["@top-left{ }"]);
        assert_eq!(extraction.tests[2].arguments, ["@page { @top-left { } }"]);
    }

    #[test]
    fn expands_the_finite_font_shorthand_permutation_generator() {
        let source = r#"
            function test_specific(prefix) {
                test_valid_value('font', parts.join(' '), canonical.join(' '));
            }
            function test_various(prefix) {}
            test_various([]);
        "#;
        let extraction = extract_css_parsing_tests(source);

        assert_eq!(extraction.unsupported_calls, []);
        assert_eq!(extraction.tests.len(), 309);
        assert_eq!(extraction.tests[0].arguments, ["font", "xx-small serif"]);
        assert_eq!(extraction.tests[1].arguments, ["font", "normal medium/normal sans-serif"]);
        assert!(extraction.tests.iter().all(|test| test.kind == CssParsingTestKind::ValidValue));
    }

    #[test]
    fn expands_fuzzy_color_helpers_inside_static_loops() {
        let source = r#"
            // This doesn't require executing JavaScript.
            for (const colorSpace of ["srgb", "display-p3"]) {
                const resultColorSpace = colorSpace == "xyz" ? "xyz-d65" : colorSpace;
                fuzzy_test_valid_color(`color-mix(in ${colorSpace}, red, blue)`, `ignored ${resultColorSpace}`);
            }
        "#;
        let extraction = extract_css_parsing_tests(source);

        assert_eq!(extraction.unsupported_calls, []);
        assert_eq!(extraction.tests.len(), 2);
        assert_eq!(extraction.tests[0].arguments, ["color", "color-mix(in srgb, red, blue)"]);
        assert_eq!(extraction.tests[1].arguments, ["color", "color-mix(in display-p3, red, blue)"]);
    }

    #[test]
    fn expands_the_finite_gradient_interpolation_matrix() {
        let valid = extract_css_parsing_tests(
            r#"
                function test_each_interpolation_method(gradientFunction, specifiers) {
                    test_valid_value(`background-image`, value);
                }
                test_each_interpolation_method("linear-gradient", LINEAR_GRADIENT_SPECIFIERS);
            "#,
        );
        let invalid = extract_css_parsing_tests(
            r#"
                function test_each_interpolation_method(gradientFunction) {
                    test_invalid_value(`background-image`, value);
                }
                test_each_interpolation_method("radial-gradient");
            "#,
        );

        assert_eq!(valid.unsupported_calls, []);
        assert_eq!(valid.tests.len(), 411);
        assert_eq!(invalid.unsupported_calls, []);
        assert_eq!(invalid.tests.len(), 94);
        assert_eq!(invalid.tests[0].arguments, ["background-image", "radial-gradient(, red, blue)"]);
    }

    #[test]
    fn parses_multiple_cases_without_rewriting_payloads() {
        let source = "#data\n<p>One<p>Two\n#errors\nold error\n#new-errors\nnew error\n#document\n| <html>\n|   <head>\n|   <body>\n|     <p>\n|       \"One\"\n\n#data\n<b>x\n#errors\n#document-fragment\nsvg svg\n#script-off\n#document\n| <b>\n|   \"x\"\n";
        let cases = parse_tree_construction_file(source).unwrap();

        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].data, "<p>One<p>Two");
        assert_eq!(cases[0].errors, ["old error"]);
        assert_eq!(cases[0].new_errors, ["new error"]);
        assert_eq!(cases[0].scripting, ScriptingMode::Both);
        assert_eq!(cases[1].fragment_context.as_deref(), Some("svg svg"));
        assert_eq!(cases[1].scripting, ScriptingMode::Off);
        assert_eq!(cases[1].document, "| <b>\n|   \"x\"");
    }

    #[test]
    fn reports_the_case_containing_a_malformed_section() {
        let error = parse_tree_construction_file("#data\nx\n#document\n| <html>\n").unwrap_err();
        assert_eq!(error.case_index, 1);
        assert!(error.message.contains("#errors"));
    }
}
