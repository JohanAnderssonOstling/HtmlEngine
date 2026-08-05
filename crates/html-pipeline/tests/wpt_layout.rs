use html_dom::{Document, ImageSource};
use html_layout::{FontSlant, GlyphId, GlyphMetric, GlyphRegistry, GlyphShaper, ImageMetrics, LayoutConstraints, ShapeError};
use html_pipeline::{DocumentFactory, parse_html_document};
use html_resources::{ResourceProvider, probe_dimensions};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

const LAYOUT_TESTS: &str = include_str!("../../../testdata/wpt/layout-check-tests.txt");
const KNOWN_FAILURES: &str = include_str!("../../../testdata/wpt/known-layout-failures.txt");
const IGNORED_RUNS: &str = include_str!("../../../testdata/wpt/ignored-layout-runs.txt");
const VIEWPORT_WIDTH: f64 = 800.0;
const LINE_HEIGHT: f64 = 16.0;
const WPT_LAYOUT_TOLERANCE: f64 = 1.0;

#[derive(Default)]
struct DeterministicShaper {
    glyphs: HashMap<(char, u32, bool), GlyphId>,
}

impl GlyphShaper for DeterministicShaper {
    fn reset(&mut self) {
        self.glyphs.clear();
    }

    fn shape_glyph<'a>(&mut self, glyph_metrics: &mut GlyphRegistry<'a>, ch: char, font_size: f32, _font_weight: u16, _font_slant: FontSlant, _color: u32, family: Option<&str>) -> Result<GlyphId, ShapeError> {
        let is_ahem = family.and_then(|families| families.split(',').next()).map(str::trim).map(|family| family.trim_matches(['\'', '"'])).is_some_and(|family| family.eq_ignore_ascii_case("ahem"));
        let key = (ch, font_size.to_bits(), is_ahem);
        if let Some(&glyph) = self.glyphs.get(&key) {
            return Ok(glyph);
        }
        let advance = if is_ahem { font_size } else { font_size * 0.5 };
        let metric = GlyphMetric::try_new(ch, advance, font_size * 0.75, font_size * 0.25, font_size * 0.75).map_err(ShapeError::rejected_metric)?;
        let glyph = glyph_metrics.register(metric)?;
        self.glyphs.insert(key, glyph);
        Ok(glyph)
    }
}

struct WptResourceProvider {
    root: PathBuf,
}

impl ResourceProvider for WptResourceProvider {
    fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
        fs::read(self.root.join(uri.trim_start_matches('/')))
    }

    fn exists(&self, uri: &str) -> bool {
        self.root.join(uri.trim_start_matches('/')).is_file()
    }

    fn resolve(&self, base: &str, href: &str) -> String {
        if href.contains("://") {
            return href.to_owned();
        }
        let base = Path::new(base);
        let base_dir = if base.as_os_str().to_string_lossy().ends_with('/') { base } else { base.parent().unwrap_or_else(|| Path::new("")) };
        let path = if href.starts_with('/') { PathBuf::from(href.trim_start_matches('/')) } else { base_dir.join(href) };
        normalize_relative_path(&path)
    }

    fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

#[test]
fn runs_pinned_noninteractive_wpt_layout_assertions() {
    let wpt_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/wpt");
    let provider = Arc::new(WptResourceProvider { root: wpt_root.clone() });
    let tests = manifest_entries();
    let mut known = KNOWN_FAILURES.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(str::to_owned).collect::<BTreeSet<_>>();
    let mut ignored =
        IGNORED_RUNS.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(|line| line.split_once("|reason=").expect("ignored layout run includes a reason").0.to_owned()).collect::<BTreeSet<_>>();
    let ignored_total = ignored.len();
    let verbose = std::env::var_os("HTML_WPT_LAYOUT_VERBOSE").is_some();
    let mut infrastructure_failures = Vec::new();
    let mut unsupported_checks = BTreeMap::<String, usize>::new();
    let mut unexpected = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut assertion_count = 0usize;

    for relative in &tests {
        if verbose {
            eprintln!("layout WPT file: {relative}");
        }
        let path = wpt_root.join(relative);
        let source = fs::read_to_string(&path).unwrap_or_else(|error| panic!("failed to read pinned WPT {}: {error}", path.display()));
        let source = prepare_noninteractive_source(relative, &source);
        let expectation_document = parse_html_document(&source).build_dom();
        let expectations = collect_expectations(relative, &expectation_document, &mut infrastructure_failures, &mut unsupported_checks);
        assertion_count += expectations.iter().map(|expectation| usize::from(expectation.width.is_some()) + usize::from(expectation.height.is_some())).sum::<usize>();
        // Files with only geometry kinds that the semantic view cannot expose
        // are still parsed and audited, but need not pay for shape and layout.
        if expectations.is_empty() {
            continue;
        }

        let mut factory = DocumentFactory::new();
        factory.set_resource_context(provider.clone(), relative.clone());
        let mut shaper = DeterministicShaper::default();
        let prepared = factory.parse_with_new_pipeline(&source, None);
        let mut image_metrics = ImageMetrics::default();
        for (image_idx, image) in expectation_document.images().iter().enumerate() {
            let source = match &image.source {
                ImageSource::Uri(uri) => ImageSource::Uri(provider.resolve(relative, uri)),
                ImageSource::Inline(bytes) => ImageSource::Inline(bytes.clone()),
            };
            if let Some((width, height)) = probe_dimensions(provider.as_ref(), &source) {
                image_metrics.set(image_idx as u32, width, height);
            }
        }
        let document = prepared
            .shape(&mut shaper)
            .unwrap_or_else(|error| panic!("failed to shape pinned WPT {relative}: {error}"))
            .layout_with_metrics(LayoutConstraints::new(VIEWPORT_WIDTH, LINE_HEIGHT).expect("fixed WPT constraints are valid"), &image_metrics);
        let boxes = document.render_view().boxes();
        let boxes_by_dom_node = (0..boxes.len()).filter_map(|box_idx| boxes.dom_node_index(box_idx).map(|node_idx| (node_idx, box_idx))).collect::<HashMap<_, _>>();

        for expectation in expectations {
            let missing_box = format!("{relative}|node={}|tag={}|missing-layout-box", expectation.node_index, expectation.tag);
            let Some(&box_idx) = boxes_by_dom_node.get(&expectation.node_index) else {
                if !known.remove(&missing_box) {
                    unexpected.push(missing_box);
                }
                continue;
            };
            if known.remove(&missing_box) {
                unexpected_passes.push(missing_box);
            }
            let size = boxes.size(box_idx).expect("a correlated box has geometry");
            if let Some(expected) = expectation.width {
                let run_key = assertion_run_key(relative, &expectation, "width", expected);
                if verbose {
                    eprintln!("layout assertion-run: {run_key}|actual={}|box={box_idx}|parent={:?}|class={:?}|style={:?}", size.width, boxes.parent(box_idx), expectation.class, expectation.style);
                }
                if !ignored.remove(&run_key) {
                    reconcile_dimension(relative, &expectation, "width", size.width, expected, &mut known, &mut unexpected, &mut unexpected_passes);
                }
            }
            if let Some(expected) = expectation.height {
                let run_key = assertion_run_key(relative, &expectation, "height", expected);
                if verbose {
                    eprintln!("layout assertion-run: {run_key}|actual={}|box={box_idx}|parent={:?}|class={:?}|style={:?}", size.height, boxes.parent(box_idx), expectation.class, expectation.style);
                }
                if !ignored.remove(&run_key) {
                    reconcile_dimension(relative, &expectation, "height", size.height, expected, &mut known, &mut unexpected, &mut unexpected_passes);
                }
            }
        }
    }

    let known_mismatches = KNOWN_FAILURES.lines().filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#')).count() - known.len();
    let unsupported_check_count = unsupported_checks.values().sum::<usize>();
    eprintln!(
        "direct layout WPT: {} files, {assertion_count} size assertions, {} unsupported geometry assertions, {ignored_total} intentionally unsupported runs, {known_mismatches} known mismatches, {} unexpected mismatches, {} unexpected passes",
        tests.len(),
        unsupported_check_count,
        unexpected.len(),
        unexpected_passes.len()
    );
    if verbose && !unsupported_checks.is_empty() {
        eprintln!("unsupported WPT geometry assertions: {unsupported_checks:?}");
    }
    assert_eq!(tests.len(), 914, "the pinned WPT layout selection changed");
    assert!(assertion_count >= 21_000, "too few WPT layout assertions were exercised: {assertion_count}");
    assert!(unsupported_check_count >= 18_000, "too few unsupported geometry assertions were audited: {unsupported_check_count}");
    let mut audit_failures = Vec::new();
    if !infrastructure_failures.is_empty() {
        audit_failures.push(format!("WPT layout adapter failures:\n{}", infrastructure_failures.join("\n")));
    }
    if !unexpected.is_empty() {
        audit_failures.push(format!("unexpected WPT layout mismatches:\n{}", unexpected.join("\n")));
    }
    if !unexpected_passes.is_empty() {
        audit_failures.push(format!("known WPT layout failures now pass and should be removed:\n{}", unexpected_passes.join("\n")));
    }
    if !known.is_empty() {
        audit_failures.push(format!("known WPT layout failures were not exercised:\n{}", known.into_iter().collect::<Vec<_>>().join("\n")));
    }
    if !ignored.is_empty() {
        audit_failures.push(format!("ignored WPT layout runs were not exercised:\n{}", ignored.into_iter().collect::<Vec<_>>().join("\n")));
    }
    assert!(audit_failures.is_empty(), "{}", audit_failures.join("\n\n"));
}

/// Applies setup performed by small WPT scripts that the noninteractive
/// runner intentionally does not execute. Keep these adaptations narrow and
/// semantic: the layout input should match the DOM at `checkLayout()` time.
fn prepare_noninteractive_source<'a>(relative: &str, source: &'a str) -> Cow<'a, str> {
    if relative == "css/css-sizing/percentage-height-replaced-content-in-auto-cb.html" {
        return Cow::Owned(source.replacen("<img id=\"img\"", "<img id=\"img\" src=\"/css/support/60x60-green.png\"", 1));
    }
    Cow::Borrowed(source)
}

fn assertion_run_key(relative: &str, expectation: &LayoutExpectation, dimension: &str, expected: f64) -> String {
    format!("{relative}|node={}|tag={}|{dimension}|expected={expected}", expectation.node_index, expectation.tag)
}

#[derive(Debug)]
struct LayoutExpectation {
    node_index: usize,
    tag: String,
    class: Option<String>,
    style: Option<String>,
    width: Option<f64>,
    height: Option<f64>,
}

fn collect_expectations(relative: &str, document: &Document, failures: &mut Vec<String>, unsupported_checks: &mut BTreeMap<String, usize>) -> Vec<LayoutExpectation> {
    let mut expectations = Vec::new();
    for node in document.node_ids() {
        let Some(element) = document.element_ref(node) else { continue };
        let mut width = None;
        let mut height = None;
        for attribute in element.attributes() {
            match attribute.name() {
                "data-expected-width" => width = parse_expected(relative, node.index(), attribute.name(), attribute.value(), failures),
                "data-expected-height" => height = parse_expected(relative, node.index(), attribute.name(), attribute.value(), failures),
                name if is_known_unsupported_geometry_check(name) => *unsupported_checks.entry(name.to_owned()).or_default() += 1,
                name if name.starts_with("data-expected-") || name.starts_with("data-offset-") => failures.push(format!("{relative}|node={}|unknown-check-layout-attribute={name}", node.index())),
                _ => {}
            }
        }
        if width.is_some() || height.is_some() {
            expectations.push(LayoutExpectation { node_index: node.index(), tag: element.tag().to_owned(), class: element.attr("class").map(str::to_owned), style: element.attr("style").map(str::to_owned), width, height });
        }
    }
    expectations
}

fn is_known_unsupported_geometry_check(name: &str) -> bool {
    matches!(
        name,
        "data-offset-x"
            | "data-offset-y"
            | "data-expected-client-height"
            | "data-expected-client-width"
            | "data-expected-scroll-height"
            | "data-expected-scroll-width"
            | "data-expected-margin-bottom"
            | "data-expected-margin-left"
            | "data-expected-margin-right"
            | "data-expected-margin-top"
            | "data-expected-padding-bottom"
            | "data-expected-padding-left"
            | "data-expected-padding-right"
            | "data-expected-padding-top"
            | "data-expected-bounding-client-rect-height"
            | "data-expected-bounding-client-rect-width"
            | "data-expected-display"
    )
}

fn parse_expected(relative: &str, node_index: usize, name: &str, value: &str, failures: &mut Vec<String>) -> Option<f64> {
    match value.parse::<f64>() {
        Ok(value) if value.is_finite() => Some(value),
        Ok(_) | Err(_) => {
            failures.push(format!("{relative}|node={node_index}|{name}=invalid({value:?})"));
            None
        }
    }
}

fn reconcile_dimension(relative: &str, expectation: &LayoutExpectation, dimension: &str, actual: f64, expected: f64, known: &mut BTreeSet<String>, unexpected: &mut Vec<String>, unexpected_passes: &mut Vec<String>) {
    // This matches check-layout-th.js: values less than one CSS pixel apart
    // pass, while a difference of one pixel or more uses exact equality.
    let prefix = format!("{relative}|node={}|tag={}|{dimension}|expected={expected}|", expectation.node_index, expectation.tag);
    if (actual - expected).abs() >= WPT_LAYOUT_TOLERANCE {
        let mismatch = format!("{prefix}actual={actual}");
        if !known.remove(&mismatch) {
            unexpected.push(mismatch);
        }
    } else if let Some(stale) = known.iter().find(|entry| entry.starts_with(&prefix)).cloned() {
        known.remove(&stale);
        unexpected_passes.push(stale);
    }
}

fn manifest_entries() -> Vec<String> {
    let mut entries = LAYOUT_TESTS.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')).map(str::to_owned).collect::<Vec<_>>();
    let mut sorted = entries.clone();
    sorted.sort();
    assert_eq!(entries, sorted, "WPT layout manifest must remain sorted");
    entries.dedup();
    assert_eq!(entries.len(), sorted.len(), "WPT layout manifest contains duplicate paths");
    entries
}

fn normalize_relative_path(path: &Path) -> String {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => components.push(value.to_string_lossy().into_owned()),
            Component::ParentDir => {
                components.pop();
            }
            Component::CurDir | Component::RootDir => {}
            Component::Prefix(_) => unreachable!("WPT resource paths are platform-independent"),
        }
    }
    components.join("/")
}
