//! Owned stylesheet programs reusable across document loads.

use super::prepared::{ParsedStylesheetSet, PreparedRuleSet};
use crate::style::matching::selectors::SelectorIndex;
use crate::{
    AuthorStylesheetInput, DEFAULT_CSS, MediaEnvironment, MediaQuerySet, MediaType, StyleTimings,
    StyledDocument,
};
use html_dom::Document;
use lightningcss::stylesheet::{ParserOptions, StyleSheet};
use static_self::IntoOwned;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

/// Parsed, prepared, and indexed CSS that can be applied to multiple DOMs.
/// DOM-specific implicit `@scope` roots are supplied when the program runs.
pub struct StyleProgram {
    prepared: PreparedRuleSet<'static>,
    selector_index: SelectorIndex,
    input_count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct StyleProgramCacheStats {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
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

/// Small in-process cache intended to live for the lifetime of an open book.
/// Entries are bounded and oldest-first evicted.
pub struct StyleProgramCache {
    entries: VecDeque<CachedStyleProgram>,
    capacity: usize,
    hits: u64,
    misses: u64,
}

impl Default for StyleProgramCache {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: 8,
            hits: 0,
            misses: 0,
        }
    }
}

impl StyleProgramCache {
    pub fn clear(&mut self) {
        self.entries.clear();
        self.hits = 0;
        self.misses = 0;
    }

    pub fn stats(&self) -> StyleProgramCacheStats {
        StyleProgramCacheStats {
            entries: self.entries.len(),
            hits: self.hits,
            misses: self.misses,
        }
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
        let css = inputs.iter().map(|input| input.css).collect::<Vec<_>>();
        let (program, timings) = StyleProgram::compile_with_timings(
            &css,
            environment,
            f32::from_bits(initial_font_size),
        );
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
        let started = Instant::now();
        let user_agent = StyleSheet::parse(DEFAULT_CSS, ParserOptions::default())
            .expect("default CSS must parse")
            .into_owned();
        timings.parse_default_css = started.elapsed();

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
                    author_root_indices
                        .push(u32::try_from(index).expect("author stylesheet count fits in u32"));
                }
                Err(error) => eprintln!("Skipping CSS chunk {index}: {error}"),
            }
        }
        timings.parse_author_css = started.elapsed();

        let started = Instant::now();
        let prepared = ParsedStylesheetSet::with_author_root_indices(
            &user_agent,
            &stylesheets,
            &author_root_indices,
        )
        .prepare(environment, f64::from(initial_font_size));
        timings.prepare_rules = started.elapsed();
        let started = Instant::now();
        let selector_index = SelectorIndex::from_prepared(&prepared);
        timings.selector_index = started.elapsed();
        (
            Self {
                prepared,
                selector_index,
                input_count: css_chunks.len(),
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
    apply(document, &program, inputs, timings)
}

pub fn style_document_with_cached_program_and_timings(
    cache: &mut StyleProgramCache,
    document: Document,
    inputs: &[AuthorStylesheetInput<'_>],
    environment: MediaEnvironment,
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    let root_font_size = document.root_font_size();
    let (program, timings) = cache.get_or_compile(inputs, environment, root_font_size);
    apply(document, &program, inputs, timings)
}

pub fn style_document_with_program_and_timings(
    document: Document,
    program: &StyleProgram,
    inputs: &[AuthorStylesheetInput<'_>],
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
    apply(document, program, inputs, StyleTimings::default())
}

fn apply(
    document: Document,
    program: &StyleProgram,
    inputs: &[AuthorStylesheetInput<'_>],
    mut timings: StyleTimings,
) -> (StyledDocument, StyleTimings, MediaQuerySet) {
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
        &document,
        &program.prepared,
        &program.selector_index,
        &author_roots,
    );
    let media_queries = program.prepared.media_queries().clone();
    timings.resolve_styles = started.elapsed();
    timings.resolver_setup = resolution.resolver_setup;
    timings.selector_matching = resolution.selector_matching;
    timings.cascade = resolution.cascade;
    timings.style_store = resolution.style_store;
    (StyledDocument { document, styles }, timings, media_queries)
}

#[cfg(test)]
mod tests {
    use super::StyleProgram;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn compiled_program_can_move_between_document_workers() {
        assert_send_sync::<StyleProgram>();
    }
}
