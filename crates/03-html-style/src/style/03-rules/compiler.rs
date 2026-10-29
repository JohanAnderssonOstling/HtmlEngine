//! Compile stylesheets and selector instructions without consulting a document.
use crate::style::compiled::{CompiledStyles, SelectorIndex};
use crate::style::environment::MediaEnvironment;
use crate::style::rules::prepared::{self, PreparedStylesheetFragment};
use crate::style::source::DEFAULT_CSS;
use crate::style::timings::StyleTimings;
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use static_self::IntoOwned;
use std::sync::{Arc, OnceLock};
use web_time::Instant;

/// Share the immutable built-in fragment across programs and workers. Linking
/// evaluates its media conditions and cascade order for each environment.
pub(crate) fn default_stylesheet(
    timings: &mut StyleTimings,
) -> &'static PreparedStylesheetFragment<'static> {
    static STYLESHEET: OnceLock<PreparedStylesheetFragment<'static>> = OnceLock::new();
    STYLESHEET.get_or_init(|| {
        let started = Instant::now();
        let stylesheet = StyleSheet::parse(DEFAULT_CSS, ParserOptions::default())
            .expect("default CSS must parse")
            .into_owned();
        let fragment = PreparedStylesheetFragment::compile(stylesheet);
        timings.parse_default_css = started.elapsed();
        fragment
    })
}

/// Parsing and declaration preparation have one path; caches only reuse its result.
pub(crate) fn parse_fragment(css: &str) -> Option<Arc<PreparedStylesheetFragment<'static>>> {
    if css.trim().is_empty() {
        return None;
    }
    crate::style::source::parse_stylesheet(css)
        .map(|stylesheet| Arc::new(PreparedStylesheetFragment::compile(stylesheet)))
}

pub(crate) fn compile(
    css_chunks: &[&str],
    environment: MediaEnvironment,
    initial_font_size: f32,
) -> (CompiledStyles, StyleTimings) {
    let started = Instant::now();
    let stylesheets = css_chunks
        .iter()
        .enumerate()
        .filter_map(|(index, css)| parse_fragment(css).map(|fragment| (fragment, index)))
        .collect::<Vec<_>>();
    let timings = StyleTimings {
        parse_author_css: started.elapsed(),
        ..StyleTimings::default()
    };
    compile_fragments(
        css_chunks.len(),
        &stylesheets,
        environment,
        initial_font_size,
        timings,
    )
}

pub(crate) fn compile_fragments(
    input_count: usize,
    stylesheets: &[(Arc<PreparedStylesheetFragment<'static>>, usize)],
    environment: MediaEnvironment,
    initial_font_size: f32,
    mut timings: StyleTimings,
) -> (CompiledStyles, StyleTimings) {
    let user_agent = default_stylesheet(&mut timings);
    let started = Instant::now();
    let rules = prepared::prepare(
        user_agent,
        stylesheets,
        environment,
        f64::from(initial_font_size),
    );
    timings.prepare_rules = started.elapsed();
    let started = Instant::now();
    let selectors = SelectorIndex::from_prepared(&rules);
    timings.selector_index = started.elapsed();
    (
        CompiledStyles {
            rules,
            selectors,
            input_count,
        },
        timings,
    )
}
