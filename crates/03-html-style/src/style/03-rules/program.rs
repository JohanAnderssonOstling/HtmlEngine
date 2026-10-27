//! Owned stylesheet programs reusable across document loads.

use super::prepared::{ParsedStylesheetSet, PreparedRuleSet, PreparedStylesheetFragment};
use crate::style::matching::selectors::SelectorIndex;
use crate::{
    AuthorStylesheetInput, DEFAULT_CSS, FontFace, MediaEnvironment, MediaQuerySet, MediaType, StyleTimings,
    StyledDocument,
};
use html_dom::Document;
use lightningcss::stylesheet::{ParserOptions, StyleAttribute, StyleSheet};
use static_self::IntoOwned;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use web_time::Instant;

/// The built-in source never changes. Share its immutable parser output across
/// programs and document workers; media evaluation and rule preparation still
/// run for each program's environment. Only the initializing call records parse
/// time, so a cache lookup is not reported as another CSS parse.
fn default_stylesheet(timings: &mut StyleTimings) -> &'static StyleSheet<'static> {
    static STYLESHEET: OnceLock<StyleSheet<'static>> = OnceLock::new();
    STYLESHEET.get_or_init(|| {
        let started = Instant::now();
        let stylesheet = StyleSheet::parse(DEFAULT_CSS, ParserOptions::default())
            .expect("default CSS must parse")
            .into_owned();
        timings.parse_default_css = started.elapsed();
        stylesheet
    })
}

/// Parsed, prepared, and indexed CSS that can be applied to multiple DOMs.
/// DOM-specific implicit `@scope` roots are supplied when the program runs.
pub struct StyleProgram {
    prepared: PreparedRuleSet<'static>,
    selector_index: SelectorIndex,
    input_count: usize,
    inline_styles: Arc<Mutex<ParsedInlineStyleCache>>,
}

#[derive(Clone, Debug, Default)]
pub struct StyleProgramCacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub parsed_entries: usize,
    pub parsed_hits: u64,
    pub parsed_misses: u64,
    pub inline_entries: usize,
    pub inline_hits: u64,
    pub inline_misses: u64,
}

struct StyleProgramCacheKey {
    css: Box<[String]>,
    media_type: u8,
    viewport_width: u64,
    viewport_height: Option<u64>,
    initial_font_size: u32,
}

struct CachedStyleProgram {
    key: StyleProgramCacheKey,
    program: Arc<StyleProgram>,
}

struct CachedParsedStylesheet {
    css: String,
    stylesheet: Option<Arc<PreparedStylesheetFragment<'static>>>,
}

struct CachedInlineStyle {
    css: String,
    style: Option<Arc<StyleAttribute<'static>>>,
}

pub(crate) struct ParsedInlineStyleCache {
    entries: VecDeque<CachedInlineStyle>,
    capacity: usize,
    hits: u64,
    misses: u64,
    revision: u64,
}

impl Default for ParsedInlineStyleCache {
    fn default() -> Self {
        Self { entries: VecDeque::new(), capacity: 256, hits: 0, misses: 0, revision: 0 }
    }
}

impl ParsedInlineStyleCache {
    fn lookup(&mut self, css: &str) -> Option<Option<Arc<StyleAttribute<'static>>>> {
        let position = self.entries.iter().position(|entry| entry.css == css)?;
        if position + 1 != self.entries.len() {
            let entry = self.entries.remove(position).expect("located inline style exists");
            self.entries.push_back(entry);
        }
        Some(self.entries.back().expect("cached inline style exists").style.clone())
    }

    /// Resolve the document's distinct inline sources with at most two locks.
    /// Parsing and disposal of redundant concurrent results happen outside
    /// the shared LRU's critical section.
    pub(crate) fn get_or_parse_batch(shared: &Mutex<Self>, sources: &[&str]) -> Vec<Option<Arc<StyleAttribute<'static>>>> {
        let mut styles = vec![None; sources.len()];
        if sources.is_empty() { return styles; }
        let mut misses = Vec::new();
        let revision = {
            let mut cache = shared.lock().expect("inline style cache lock");
            for (index, source) in sources.iter().enumerate() {
                if let Some(style) = cache.lookup(source) {
                    cache.hits += 1;
                    styles[index] = style;
                } else {
                    cache.misses += 1;
                    misses.push(index);
                }
            }
            cache.revision
        };
        if misses.is_empty() { return styles; }
        for &index in &misses {
            let normalized = crate::style::source::declarations::normalize_inline(sources[index]);
            styles[index] = StyleAttribute::parse(&normalized, ParserOptions { error_recovery: true, ..ParserOptions::default() }).ok().map(|style| Arc::new(style.into_owned()));
        }
        let mut discarded = Vec::new();
        let mut cache = shared.lock().expect("inline style cache lock");
        let raced = cache.revision != revision;
        for index in misses {
            if raced && let Some(cached) = cache.lookup(sources[index]) {
                discarded.push(std::mem::replace(&mut styles[index], cached));
                continue;
            }
            if cache.entries.len() == cache.capacity { cache.entries.pop_front(); }
            cache.entries.push_back(CachedInlineStyle { css: sources[index].to_owned(), style: styles[index].clone() });
            cache.revision = cache.revision.wrapping_add(1);
        }
        drop(cache);
        drop(discarded);
        styles
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.revision = self.revision.wrapping_add(1);
        self.hits = 0;
        self.misses = 0;
    }
}

/// Small in-process cache intended to live for the lifetime of an open book.
/// Entries are bounded and oldest-first evicted.
pub struct StyleProgramCache {
    entries: VecDeque<CachedStyleProgram>,
    capacity: usize,
    hits: u64,
    misses: u64,
    parsed_entries: VecDeque<CachedParsedStylesheet>,
    parsed_capacity: usize,
    parsed_hits: u64,
    parsed_misses: u64,
    inline_styles: Arc<Mutex<ParsedInlineStyleCache>>,
}

impl Default for StyleProgramCache {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: 8,
            hits: 0,
            misses: 0,
            parsed_entries: VecDeque::new(),
            parsed_capacity: 64,
            parsed_hits: 0,
            parsed_misses: 0,
            inline_styles: Arc::new(Mutex::new(ParsedInlineStyleCache::default())),
        }
    }
}

impl StyleProgramCache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.hits = 0;
        self.misses = 0;
        self.parsed_entries.clear();
        self.parsed_hits = 0;
        self.parsed_misses = 0;
        self.inline_styles.lock().expect("inline style cache lock").clear();
    }

    pub fn stats(&self) -> StyleProgramCacheStats {
        let inline_styles = self.inline_styles.lock().expect("inline style cache lock");
        StyleProgramCacheStats {
            entries: self.entries.len(),
            hits: self.hits,
            misses: self.misses,
            parsed_entries: self.parsed_entries.len(),
            parsed_hits: self.parsed_hits,
            parsed_misses: self.parsed_misses,
            inline_entries: inline_styles.entries.len(),
            inline_hits: inline_styles.hits,
            inline_misses: inline_styles.misses,
        }
    }

    fn get_or_parse(&mut self, css: &str) -> Option<Arc<PreparedStylesheetFragment<'static>>> {
        if let Some(position) = self.parsed_entries.iter().position(|entry| entry.css == css) {
            self.parsed_hits += 1;
            if position + 1 != self.parsed_entries.len() {
                let entry = self.parsed_entries.remove(position).expect("located parsed stylesheet exists");
                self.parsed_entries.push_back(entry);
            }
            return self.parsed_entries.back().expect("cached parsed stylesheet exists").stylesheet.clone();
        }
        self.parsed_misses += 1;
        let normalized = crate::style::source::declarations::normalize(css);
        let stylesheet = if normalized.trim().is_empty() {
            None
        } else {
            StyleSheet::parse(&normalized, ParserOptions { error_recovery: true, ..ParserOptions::default() })
                .map(|stylesheet| Arc::new(PreparedStylesheetFragment::compile(stylesheet.into_owned())))
                .ok()
        };
        if self.parsed_entries.len() == self.parsed_capacity {
            self.parsed_entries.pop_front();
        }
        self.parsed_entries.push_back(CachedParsedStylesheet { css: css.to_owned(), stylesheet: stylesheet.clone() });
        stylesheet
    }

    pub fn get_or_compile(
        &mut self,
        inputs: &[AuthorStylesheetInput<'_>],
        environment: MediaEnvironment,
        initial_font_size: f32,
    ) -> (Arc<StyleProgram>, StyleTimings) {
        let media_type = match environment.media_type() {
            MediaType::Screen => 0,
            MediaType::Print => 1,
        };
        let viewport_width = environment.viewport_width().to_bits();
        let viewport_height = environment.viewport_height().map(f64::to_bits);
        let initial_font_size = initial_font_size.to_bits();
        if let Some(position) = self.entries.iter().position(|entry| {
            entry.key.media_type == media_type
                && entry.key.viewport_width == viewport_width
                && entry.key.viewport_height == viewport_height
                && entry.key.initial_font_size == initial_font_size
                && entry
                    .key
                    .css
                    .iter()
                    .map(String::as_str)
                    .eq(inputs.iter().map(|input| input.css))
        }) {
            self.hits += 1;
            if position + 1 != self.entries.len() {
                let entry = self
                    .entries
                    .remove(position)
                    .expect("located style program exists");
                self.entries.push_back(entry);
            }
            return (
                self.entries
                    .back()
                    .expect("cached style program exists")
                    .program
                    .clone(),
                StyleTimings::default(),
            );
        }
        self.misses += 1;
        let key = StyleProgramCacheKey {
            css: inputs.iter().map(|input| input.css.to_owned()).collect(),
            media_type,
            viewport_width,
            viewport_height,
            initial_font_size,
        };
        let parse_started = Instant::now();
        let stylesheets = inputs.iter().enumerate().filter_map(|(index, input)| self.get_or_parse(input.css).map(|stylesheet| (stylesheet, index))).collect::<Vec<_>>();
        let parse_author_css = parse_started.elapsed();
        let (program, mut timings) = StyleProgram::compile_parsed_with_timings(inputs.len(), &stylesheets, environment, f32::from_bits(initial_font_size), self.inline_styles.clone());
        timings.parse_author_css = parse_author_css;
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        let program = Arc::new(program);
        self.entries.push_back(CachedStyleProgram {
            key,
            program: program.clone(),
        });
        (program, timings)
    }
}

impl StyleProgram {
    /// Active font declarations for this program's media environment.
    pub fn font_faces(&self) -> &[FontFace] {
        self.prepared.font_faces()
    }

    pub fn compile(
        css_chunks: &[&str],
        environment: MediaEnvironment,
        initial_font_size: f32,
    ) -> Self {
        Self::compile_with_timings(css_chunks, environment, initial_font_size).0
    }

    fn compile_with_timings(
        css_chunks: &[&str],
        environment: MediaEnvironment,
        initial_font_size: f32,
    ) -> (Self, StyleTimings) {
        let mut timings = StyleTimings::default();
        let user_agent = default_stylesheet(&mut timings);
        let started = Instant::now();
        let mut stylesheets = Vec::with_capacity(css_chunks.len());
        let mut author_root_indices = Vec::with_capacity(css_chunks.len());
        for (index, css) in css_chunks.iter().enumerate() {
            let normalized = crate::style::source::declarations::normalize(css);
            if normalized.trim().is_empty() {
                continue;
            }
            let options = ParserOptions {
                error_recovery: true,
                ..ParserOptions::default()
            };
            match StyleSheet::parse(&normalized, options) {
                Ok(stylesheet) => {
                    stylesheets.push(stylesheet.into_owned());
                    author_root_indices.push(u32::try_from(index).expect("author stylesheet count fits in u32"));
                }
                Err(_) => {}
            }
        }
        timings.parse_author_css = started.elapsed();
        let prepared = ParsedStylesheetSet::with_contiguous_author_root_indices(user_agent, &stylesheets, &author_root_indices);
        Self::finish_compile(prepared, css_chunks.len(), environment, initial_font_size, timings, Arc::new(Mutex::new(ParsedInlineStyleCache::default())))
    }

    fn compile_parsed_with_timings(input_count: usize, stylesheets: &[(Arc<PreparedStylesheetFragment<'static>>, usize)], environment: MediaEnvironment, initial_font_size: f32, inline_styles: Arc<Mutex<ParsedInlineStyleCache>>) -> (Self, StyleTimings) {
        let mut timings = StyleTimings::default();
        let user_agent = default_stylesheet(&mut timings);
        let author_refs = stylesheets.iter().map(|(stylesheet, _)| stylesheet.as_ref()).collect::<Vec<_>>();
        let author_root_indices = stylesheets.iter().map(|(_, index)| u32::try_from(*index).expect("author stylesheet count fits in u32")).collect::<Vec<_>>();
        let prepared = ParsedStylesheetSet::with_fragments(user_agent, &author_refs, &author_root_indices);
        Self::finish_compile(prepared, input_count, environment, initial_font_size, timings, inline_styles)
    }

    fn finish_compile(parsed: ParsedStylesheetSet<'_, 'static>, input_count: usize, environment: MediaEnvironment, initial_font_size: f32, mut timings: StyleTimings, inline_styles: Arc<Mutex<ParsedInlineStyleCache>>) -> (Self, StyleTimings) {
        let started = Instant::now();
        let prepared = parsed.prepare(environment, f64::from(initial_font_size));
        timings.prepare_rules = started.elapsed();
        let started = Instant::now();
        let selector_index = SelectorIndex::from_prepared(&prepared);
        timings.selector_index = started.elapsed();
        (
            Self {
                prepared,
                selector_index,
                input_count,
                inline_styles,
            },
            timings,
        )
    }
}

pub(crate) fn compile_and_apply(
    document: Document,
    inputs: &[AuthorStylesheetInput<'_>],
    environment: MediaEnvironment,
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    let css = inputs.iter().map(|input| input.css).collect::<Vec<_>>();
    let (program, timings) =
        StyleProgram::compile_with_timings(&css, environment, document.root_font_size());
    apply(document, &program, inputs, timings, &html_style_model::ReaderStyleOverrides::default())
}

pub fn style_document_with_cached_program_and_timings(
    cache: &mut StyleProgramCache,
    document: Document,
    inputs: &[AuthorStylesheetInput<'_>],
    environment: MediaEnvironment,
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    style_document_with_cached_program_and_reader_overrides(cache, document, inputs, environment, &html_style_model::ReaderStyleOverrides::default())
}

pub fn style_document_with_cached_program_and_reader_overrides(
    cache: &mut StyleProgramCache,
    document: Document,
    inputs: &[AuthorStylesheetInput<'_>],
    environment: MediaEnvironment,
    reader_overrides: &html_style_model::ReaderStyleOverrides,
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    let root_font_size = document.root_font_size();
    let (program, timings) = cache.get_or_compile(inputs, environment, root_font_size);
    apply(document, &program, inputs, timings, reader_overrides)
}

pub fn style_document_with_program_and_timings(
    document: Document,
    program: &StyleProgram,
    inputs: &[AuthorStylesheetInput<'_>],
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    style_document_with_program_and_reader_overrides(document, program, inputs, &html_style_model::ReaderStyleOverrides::default())
}

pub fn style_document_with_program_and_reader_overrides(
    document: Document,
    program: &StyleProgram,
    inputs: &[AuthorStylesheetInput<'_>],
    reader_overrides: &html_style_model::ReaderStyleOverrides,
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    apply(document, program, inputs, StyleTimings::default(), reader_overrides)
}

fn apply(
    document: Document,
    program: &StyleProgram,
    inputs: &[AuthorStylesheetInput<'_>],
    timings: StyleTimings,
    reader_overrides: &html_style_model::ReaderStyleOverrides,
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    let (styles, timings, queries) = resolve_document_with_program(&document, program, inputs, timings, reader_overrides);
    (StyledDocument { document, styles, font_faces: program.prepared.font_faces().clone() }, timings, queries)
}

/// Resolve a new cascade while borrowing the canonical DOM owned by the session.
pub fn resolve_document_with_program(
    document: &Document,
    program: &StyleProgram,
    inputs: &[AuthorStylesheetInput<'_>],
    mut timings: StyleTimings,
    reader_overrides: &html_style_model::ReaderStyleOverrides,
) -> (html_style_model::ComputedStyles, StyleTimings, MediaQuerySet) {
    assert_eq!(
        inputs.len(),
        program.input_count,
        "style program inputs must correspond to its compiled stylesheet chunks"
    );
    let author_roots = inputs
        .iter()
        .map(|input| input.implicit_scope_root)
        .collect::<Vec<_>>();
    let started = Instant::now();
    let (styles, resolution) = crate::style::cascade::resolver::resolve_styles_for_dom_timed(
        document,
        &program.prepared,
        &program.selector_index,
        &author_roots,
        &program.inline_styles,
        reader_overrides,
    );
    let media_queries = program.prepared.media_queries().clone();
    timings.resolve_styles = started.elapsed();
    timings.resolver_setup = resolution.resolver_setup;
    timings.selector_matching = resolution.selector_matching;
    timings.cascade = resolution.cascade;
    timings.style_store = resolution.style_store;
    (styles, timings, media_queries)
}

#[cfg(test)]
mod tests {
    use super::{StyleProgram, StyleProgramCache, default_stylesheet, style_document_with_program_and_timings};
    use crate::{AuthorStylesheetInput, MediaEnvironment, StyleTimings};
    use html_style_model::Display;
    use std::ptr;
    use std::time::Duration;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn compiled_program_can_move_between_document_workers() {
        assert_send_sync::<StyleProgram>();
        assert_send_sync::<StyleProgramCache>();
    }

    #[test]
    fn default_stylesheet_is_shared_across_document_workers() {
        let shared = default_stylesheet(&mut StyleTimings::default());
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8).map(|_| scope.spawn(|| {
                let mut timings = StyleTimings::default();
                let reused = default_stylesheet(&mut timings);
                assert!(ptr::eq(shared, reused));
                assert_eq!(timings.parse_default_css, Duration::ZERO);
            })).collect();
            for worker in workers { worker.join().unwrap(); }
        });
    }

    #[test]
    fn fresh_programs_reuse_default_css_and_keep_environment_specific_styles() {
        default_stylesheet(&mut StyleTimings::default());
        let css = "p { color: red } @media (min-width: 600px) { p { color: blue } }";
        for cached in [false, true] {
            let mut cache = StyleProgramCache::default();
            for (width, expected_color) in [(400.0, 0xff0000ff), (800.0, 0x0000ffff), (400.0, 0xff0000ff)] {
                let document = html_parse::parse_dom_document("<!doctype html><html><body><p>text<span>span</span></p></body></html>").unwrap();
                let inputs = [AuthorStylesheetInput { css, implicit_scope_root: document.dom_root() }];
                let environment = MediaEnvironment::screen(width, None).unwrap();
                let (styled, timings) = if cached {
                    // Clearing a book cache must not discard the shared UA AST.
                    cache.clear();
                    let (program, timings) = cache.get_or_compile(&inputs, environment, 16.0);
                    (style_document_with_program_and_timings(document, &program, &inputs).0, timings)
                } else {
                    let (program, timings) = StyleProgram::compile_with_timings(&[css], environment, 16.0);
                    (style_document_with_program_and_timings(document, &program, &inputs).0, timings)
                };
                assert_eq!(timings.parse_default_css, Duration::ZERO);
                let paragraph = styled.document().node_ids().find(|&node| styled.document().get_dom_tag(node) == Some("p")).unwrap();
                let paragraph_style = styled.style_for_node(paragraph).unwrap();
                assert_eq!(styled.text_style(paragraph_style).unwrap().color, expected_color);
                assert_eq!(styled.style_view(paragraph_style).unwrap().display(), Display::Block);
                let span = styled.document().node_ids().find(|&node| styled.document().get_dom_tag(node) == Some("span")).unwrap();
                assert_eq!(styled.style_view(styled.style_for_node(span).unwrap()).unwrap().display(), Display::Inline);
            }
        }
    }
}



#[cfg(test)]
mod inline_cache_tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn simultaneous_inline_cache_misses_publish_one_shared_style() {
        let cache = Mutex::new(ParsedInlineStyleCache::default());
        let ready = Barrier::new(4);
        let styles = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..4).map(|_| scope.spawn(|| {
                ready.wait();
                ParsedInlineStyleCache::get_or_parse_batch(&cache, &["color: red; padding: 1px"]).pop().flatten().unwrap()
            })).collect();
            jobs.into_iter().map(|job| job.join().unwrap()).collect::<Vec<_>>()
        });
        assert!(styles.iter().all(|style| Arc::ptr_eq(style, &styles[0])));
        let cache = cache.lock().unwrap();
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.hits + cache.misses, 4);
    }

    #[test]
    fn a_batch_pins_hits_before_new_sources_evict_them() {
        let cache = Mutex::new(ParsedInlineStyleCache { capacity: 2, ..Default::default() });
        let first = ParsedInlineStyleCache::get_or_parse_batch(&cache, &["color: red", "color: blue"]);
        let second = ParsedInlineStyleCache::get_or_parse_batch(&cache, &["color: green", "color: red", "color: blue"]);
        assert!(Arc::ptr_eq(first[0].as_ref().unwrap(), second[1].as_ref().unwrap()));
        assert!(Arc::ptr_eq(first[1].as_ref().unwrap(), second[2].as_ref().unwrap()));
        let cache = cache.lock().unwrap();
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.hits, 2);
        assert_eq!(cache.misses, 3);
    }
}
