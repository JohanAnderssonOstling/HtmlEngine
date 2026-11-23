//! Owned stylesheet programs reusable across document loads.

use crate::style::compiled::CompiledStyles;
use crate::style::document::{AuthorStylesheetInput, StyledDocument};
use crate::style::environment::{MediaEnvironment, MediaQuerySet};
use crate::style::rules::compiler;
use crate::style::source::inline::{InlineStyles, ParsedInlineStyleCache, cached_entry};
use crate::style::source::resource::{ResourceStylesheet, ResourceStylesheetEntry};
use crate::style::timings::StyleTimings;
use html_dom::{Document, DomNodeId};
use html_style_model::{ComputedStyles, FontFace, ReaderStyleOverrides};
use rustc_data_structures::fx::FxHashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use web_time::Instant;

/// Parsed, prepared, and indexed CSS that can be applied to multiple DOMs.
/// DOM-specific implicit `@scope` roots are supplied when the program runs.
pub struct StyleProgram {
    compiled: CompiledStyles,
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
    sources: Box<[Option<u64>]>,
    environment: MediaEnvironment,
    initial_font_size: u32,
}

struct CachedStyleProgram {
    key: StyleProgramCacheKey,
    program: Arc<StyleProgram>,
}

struct CachedSource {
    last_used: u64,
    complete: Option<Option<ResourceStylesheet>>,
    entries: Option<Vec<ResourceStylesheetEntry>>,
}

/// Small in-process cache intended to live for the lifetime of an open book.
/// Entries are bounded and oldest-first evicted.
pub struct StyleProgramCache {
    entries: VecDeque<CachedStyleProgram>,
    capacity: usize,
    hits: u64,
    misses: u64,
    resource_sources: FxHashMap<Arc<str>, CachedSource>,
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
            resource_sources: FxHashMap::default(),
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
        self.resource_sources.clear();
        self.parsed_hits = 0;
        self.parsed_misses = 0;
        self.inline_styles
            .lock()
            .expect("inline style cache lock")
            .clear();
    }

    pub fn stats(&self) -> StyleProgramCacheStats {
        let (inline_entries, inline_hits, inline_misses) = self
            .inline_styles
            .lock()
            .expect("inline style cache lock")
            .stats();
        StyleProgramCacheStats {
            entries: self.entries.len(),
            hits: self.hits,
            misses: self.misses,
            parsed_entries: self.resource_sources.len(),
            parsed_hits: self.parsed_hits,
            parsed_misses: self.parsed_misses,
            inline_entries,
            inline_hits,
            inline_misses,
        }
    }

    fn source(&mut self, css: &str) -> &mut CachedSource {
        let last_used = self.parsed_hits + self.parsed_misses;
        if self.resource_sources.contains_key(css) {
            self.parsed_hits += 1;
        } else {
            self.parsed_misses += 1;
            if self.resource_sources.len() == self.parsed_capacity {
                let oldest = self
                    .resource_sources
                    .iter()
                    .min_by_key(|(_, source)| source.last_used)
                    .map(|(css, _)| css.clone())
                    .expect("full source cache");
                self.resource_sources.remove(&oldest);
            }
            self.resource_sources.insert(
                Arc::from(css),
                CachedSource {
                    last_used,
                    complete: None,
                    entries: None,
                },
            );
        }
        let source = self.resource_sources.get_mut(css).expect("cached source");
        source.last_used = last_used;
        source
    }

    /// Reuse parser output before resolving resource-specific font URLs/imports.
    pub fn resource_entries(&mut self, css: &str) -> Vec<ResourceStylesheetEntry> {
        self.source(css)
            .entries
            .get_or_insert_with(|| ResourceStylesheet::parse(css))
            .clone()
    }

    pub fn get_or_compile(
        &mut self,
        inputs: &[AuthorStylesheetInput<'_>],
        environment: MediaEnvironment,
        initial_font_size: f32,
    ) -> (Arc<StyleProgram>, StyleTimings) {
        let sources: Vec<_> = inputs
            .iter()
            .map(|input| {
                self.source(input.css)
                    .complete
                    .get_or_insert_with(|| ResourceStylesheet::parse_complete(input.css))
                    .clone()
            })
            .collect();
        self.get_or_compile_with(
            sources.iter().map(Option::as_ref),
            environment,
            initial_font_size,
        )
    }

    /// Compile resource-expanded parser output without a CSS round trip.
    pub fn get_or_compile_resources(
        &mut self,
        sources: &[&ResourceStylesheet],
        environment: MediaEnvironment,
        initial_font_size: f32,
    ) -> (Arc<StyleProgram>, StyleTimings) {
        self.get_or_compile_with(
            sources.iter().copied().map(Some),
            environment,
            initial_font_size,
        )
    }

    fn get_or_compile_with<'a>(
        &mut self,
        inputs: impl ExactSizeIterator<Item = Option<&'a ResourceStylesheet>> + Clone,
        environment: MediaEnvironment,
        initial_font_size: f32,
    ) -> (Arc<StyleProgram>, StyleTimings) {
        let initial_font_size = initial_font_size.to_bits();
        if let Some(entry) = cached_entry(&mut self.entries, |entry| {
            entry.key.environment == environment
                && entry.key.initial_font_size == initial_font_size
                && entry.key.sources.iter().copied().eq(inputs
                    .clone()
                    .map(|source| source.map(ResourceStylesheet::id)))
        }) {
            self.hits += 1;
            return (entry.program.clone(), StyleTimings::default());
        }
        self.misses += 1;
        let input_count = inputs.len();
        let mut sources = Vec::with_capacity(input_count);
        let mut stylesheets = Vec::with_capacity(input_count);
        let parse_started = Instant::now();
        for (index, input) in inputs.enumerate() {
            sources.push(input.map(ResourceStylesheet::id));
            if let Some(source) = input {
                stylesheets.push((source.prepared(), index));
            }
        }
        let parse_author_css = parse_started.elapsed();
        let key = StyleProgramCacheKey {
            sources: sources.into_boxed_slice(),
            environment,
            initial_font_size,
        };
        let (compiled, timings) = compiler::compile_fragments(
            input_count,
            &stylesheets,
            environment,
            f32::from_bits(initial_font_size),
            StyleTimings {
                parse_author_css,
                ..StyleTimings::default()
            },
        );
        let program = StyleProgram {
            compiled,
            inline_styles: self.inline_styles.clone(),
        };
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
        self.compiled.rules.font_faces()
    }

    /// Retain active font declarations without copying their source metadata.
    pub fn shared_font_faces(&self) -> Arc<[FontFace]> {
        self.compiled.rules.font_faces().clone()
    }

    pub fn compile(
        css_chunks: &[&str],
        environment: MediaEnvironment,
        initial_font_size: f32,
    ) -> Self {
        Self::compile_with_timings(css_chunks.iter().copied(), environment, initial_font_size).0
    }

    pub(crate) fn compile_with_timings<'a>(
        css_chunks: impl ExactSizeIterator<Item = &'a str>,
        environment: MediaEnvironment,
        initial_font_size: f32,
    ) -> (Self, StyleTimings) {
        let (compiled, timings) = compiler::compile(css_chunks, environment, initial_font_size);
        (
            Self {
                compiled,
                inline_styles: Arc::new(Mutex::new(ParsedInlineStyleCache::default())),
            },
            timings,
        )
    }
}

/// A document's stylesheet bindings and resolution options.
/// Compilation can be fresh, cached, or supplied by the caller.
pub struct StyleRequest<'a> {
    document: Document,
    inputs: Vec<AuthorStylesheetInput<'a>>,
    environment: MediaEnvironment,
    reader_overrides: Option<&'a ReaderStyleOverrides>,
}

impl<'a> StyleRequest<'a> {
    pub fn new(document: Document, inputs: &[AuthorStylesheetInput<'a>]) -> Self {
        Self {
            document,
            inputs: inputs.to_vec(),
            environment: MediaEnvironment::default(),
            reader_overrides: None,
        }
    }

    pub fn from_css(document: Document, css: &[&'a str]) -> Self {
        let root = document.dom_root();
        Self {
            document,
            inputs: css
                .iter()
                .map(|css| AuthorStylesheetInput {
                    css,
                    implicit_scope_root: root,
                })
                .collect(),
            environment: MediaEnvironment::default(),
            reader_overrides: None,
        }
    }

    pub fn environment(mut self, environment: MediaEnvironment) -> Self {
        self.environment = environment;
        self
    }

    pub fn reader_overrides(mut self, overrides: &'a ReaderStyleOverrides) -> Self {
        self.reader_overrides = Some(overrides);
        self
    }

    pub fn compile(self) -> (StyledDocument, StyleTimings, MediaQuerySet) {
        let (program, timings) = StyleProgram::compile_with_timings(
            self.inputs.iter().map(|input| input.css),
            self.environment,
            self.document.root_font_size(),
        );
        self.apply(&program, timings)
    }

    pub fn compile_cached(
        self,
        cache: &mut StyleProgramCache,
    ) -> (StyledDocument, StyleTimings, MediaQuerySet) {
        let (program, timings) = cache.get_or_compile(
            &self.inputs,
            self.environment,
            self.document.root_font_size(),
        );
        self.apply(&program, timings)
    }

    /// Resolve using the supplied program's already compiled environment.
    pub fn resolve(self, program: &StyleProgram) -> (StyledDocument, StyleTimings, MediaQuerySet) {
        self.apply(program, StyleTimings::default())
    }

    fn apply(
        self,
        program: &StyleProgram,
        timings: StyleTimings,
    ) -> (StyledDocument, StyleTimings, MediaQuerySet) {
        let author_roots = self
            .inputs
            .iter()
            .map(|input| input.implicit_scope_root)
            .collect::<Vec<_>>();
        let (styles, timings, queries) = resolve_document_with_program(
            &self.document,
            program,
            &author_roots,
            timings,
            self.reader_overrides
                .unwrap_or(&ReaderStyleOverrides::default()),
        );
        (
            StyledDocument {
                document: self.document,
                styles,
                font_faces: program.compiled.rules.font_faces().clone(),
            },
            timings,
            queries,
        )
    }
}

/// Resolve ordinary author CSS with default environment and reader options.
pub fn style_document(document: Document, css: &[&str]) -> StyledDocument {
    StyleRequest::from_css(document, css).compile().0
}

/// Resolve a new cascade while borrowing the canonical DOM owned by the session.
pub fn resolve_document_with_program(
    document: &Document,
    program: &StyleProgram,
    author_roots: &[Option<DomNodeId>],
    mut timings: StyleTimings,
    reader_overrides: &ReaderStyleOverrides,
) -> (ComputedStyles, StyleTimings, MediaQuerySet) {
    assert_eq!(
        author_roots.len(),
        program.compiled.input_count,
        "style program inputs must correspond to its compiled stylesheet chunks"
    );
    let started = Instant::now();
    let inline_styles = InlineStyles::parse(document, &program.inline_styles);
    let inline_time = started.elapsed();
    let (styles, resolution, media_queries) =
        crate::style::cascade::resolver::resolve_styles_for_dom_timed(
            document,
            &program.compiled,
            author_roots,
            &inline_styles,
            reader_overrides,
        );
    timings.resolve_styles = started.elapsed();
    timings.resolver_setup = resolution.resolver_setup + inline_time;
    timings.selector_matching = resolution.selector_matching;
    timings.cascade = resolution.cascade;
    timings.style_store = resolution.style_store;
    (styles, timings, media_queries)
}

#[cfg(test)]
mod tests {
    use super::{StyleProgram, StyleProgramCache};
    use crate::style::rules::compiler::default_stylesheet;

    use crate::{AuthorStylesheetInput, MediaEnvironment, StyleTimings};
    use html_style_model::Display;
    use std::ptr;
    use std::sync::Arc;
    use std::time::Duration;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn compiled_program_can_move_between_document_workers() {
        assert_send_sync::<StyleProgram>();
        assert_send_sync::<StyleProgramCache>();
    }

    #[test]
    fn cached_programs_keep_viewport_dependencies_document_local() {
        let mut cache = StyleProgramCache::default();
        let inputs = [AuthorStylesheetInput {
            css: "p { width:50vw }",
            implicit_scope_root: None,
        }];
        for (source, expected) in [
            ("<p>viewport</p>", true),
            ("<div>fixed</div>", false),
            ("<p>viewport again</p>", true),
        ] {
            let document = html_parse::parse_dom_document(source).unwrap();
            let (_, _, queries) = crate::StyleRequest::new(document, &inputs)
                .environment(MediaEnvironment::default())
                .compile_cached(&mut cache);
            assert_eq!(queries.uses_viewport_units(), expected, "{source}");
        }
        assert_eq!((cache.stats().misses, cache.stats().hits), (1, 2));
    }

    #[test]
    fn cached_programs_share_css_across_environments_and_repeated_inputs() {
        let css = "p { color:red }";
        let inputs = [css, "", css].map(|css| AuthorStylesheetInput {
            css,
            implicit_scope_root: None,
        });
        let mut cache = StyleProgramCache::default();
        let mut sources = None;
        for width in [400.0, 800.0] {
            cache.get_or_compile(
                &inputs,
                MediaEnvironment::screen(width, None).unwrap(),
                16.0,
            );
            let key = &cache.entries.back().unwrap().key;
            assert_eq!(key.sources.len(), inputs.len());
            assert_eq!(key.sources[0], key.sources[2]);
            if let Some(previous) = &sources {
                assert_eq!(&key.sources, previous);
            }
            sources = Some(key.sources.clone());
        }
        assert_eq!(
            (
                cache.stats().misses,
                cache.stats().parsed_misses,
                cache.stats().parsed_hits
            ),
            (2, 2, 4)
        );
        cache.clear();
        let sources = sources.unwrap();
        assert_eq!(sources[0], sources[2]);
        assert_eq!(sources.len(), inputs.len());
    }

    #[test]
    fn reused_compilation_keeps_scope_roots_and_inline_values_document_local() {
        use html_style_model::LengthPct;

        let mut cache = StyleProgramCache::default();
        let mut previous = None;
        for (prefix, padding) in [("", 3.0), ("<aside><span>prefix</span></aside>", 9.0)] {
            let document = html_parse::parse_dom_document(&format!(
                "<!doctype html><html><body>{prefix}<main id='scope'><p id='target' style='padding-left:{padding}px'>inside</p></main><p id='outside'>outside</p></body></html>"
            )).unwrap();
            let scope = document
                .node_ids()
                .find(|&node| {
                    document
                        .try_element_ref(node)
                        .and_then(|element| element.id())
                        == Some("scope")
                })
                .unwrap();
            let inputs = [AuthorStylesheetInput {
                css: "@scope { p { color:red } }",
                implicit_scope_root: Some(scope),
            }];
            let (program, _) = cache.get_or_compile(&inputs, MediaEnvironment::default(), 16.0);
            if let Some(previous) = previous {
                assert!(Arc::ptr_eq(&program, &previous));
            }
            let (styled, _, _) = crate::StyleRequest::new(document, &inputs).resolve(&program);
            let style_for = |id| {
                let node = styled
                    .document()
                    .node_ids()
                    .find(|&node| {
                        styled
                            .document()
                            .try_element_ref(node)
                            .and_then(|element| element.id())
                            == Some(id)
                    })
                    .unwrap();
                styled
                    .style_view(styled.style_for_node(node).unwrap())
                    .unwrap()
            };
            assert_eq!(style_for("target").text.color, 0xff0000ff);
            assert_eq!(
                style_for("target").box_model.padding_left,
                LengthPct::Px(padding)
            );
            assert_eq!(style_for("outside").text.color, 0x000000ff);
            previous = Some(program);
        }
        assert_eq!((cache.stats().misses, cache.stats().hits), (1, 1));
        assert_eq!(cache.stats().inline_misses, 2);
    }

    #[test]
    fn default_stylesheet_is_shared_across_document_workers() {
        let shared = default_stylesheet(&mut StyleTimings::default());
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        let mut timings = StyleTimings::default();
                        let reused = default_stylesheet(&mut timings);
                        assert!(ptr::eq(shared, reused));
                        assert_eq!(timings.parse_default_css, Duration::ZERO);
                    })
                })
                .collect();
            for worker in workers {
                worker.join().unwrap();
            }
        });
    }

    #[test]
    fn fresh_programs_reuse_default_css_and_keep_environment_specific_styles() {
        default_stylesheet(&mut StyleTimings::default());
        let css = "p { color: red } @media (min-width: 600px) { p { color: blue } }";
        for cached in [false, true] {
            let mut cache = StyleProgramCache::default();
            for (width, expected_color) in [
                (400.0, 0xff0000ff),
                (800.0, 0x0000ffff),
                (400.0, 0xff0000ff),
            ] {
                let document = html_parse::parse_dom_document(
                    "<!doctype html><html><body><p>text<span>span</span></p></body></html>",
                )
                .unwrap();
                let inputs = [AuthorStylesheetInput {
                    css,
                    implicit_scope_root: document.dom_root(),
                }];
                let environment = MediaEnvironment::screen(width, None).unwrap();
                let (styled, timings) = if cached {
                    // Clearing a book cache must not discard the shared UA AST.
                    cache.clear();
                    let (program, timings) = cache.get_or_compile(&inputs, environment, 16.0);
                    (
                        crate::StyleRequest::new(document, &inputs)
                            .resolve(&program)
                            .0,
                        timings,
                    )
                } else {
                    let (program, timings) =
                        StyleProgram::compile_with_timings([css].into_iter(), environment, 16.0);
                    (
                        crate::StyleRequest::new(document, &inputs)
                            .resolve(&program)
                            .0,
                        timings,
                    )
                };
                assert_eq!(timings.parse_default_css, Duration::ZERO);
                let paragraph = styled
                    .document()
                    .node_ids()
                    .find(|&node| {
                        styled
                            .document()
                            .try_element_ref(node)
                            .map(|element| element.tag())
                            == Some("p")
                    })
                    .unwrap();
                let paragraph_style = styled.style_for_node(paragraph).unwrap();
                assert_eq!(
                    styled.text_style(paragraph_style).unwrap().color,
                    expected_color
                );
                assert_eq!(
                    styled.style_view(paragraph_style).unwrap().display(),
                    Display::Block
                );
                let span = styled
                    .document()
                    .node_ids()
                    .find(|&node| {
                        styled
                            .document()
                            .try_element_ref(node)
                            .map(|element| element.tag())
                            == Some("span")
                    })
                    .unwrap();
                assert_eq!(
                    styled
                        .style_view(styled.style_for_node(span).unwrap())
                        .unwrap()
                        .display(),
                    Display::Inline
                );
            }
        }
    }

    #[test]
    fn cached_and_fresh_fragments_keep_scope_indices_through_inactive_rules() {
        use html_style_model::LengthPct;

        let sheets = [
            " ",
            "@supports (width: bogus) { p { color:yellow } } @media (max-width:600px) { p { color:red } } @scope { p { background:green } }",
            "@scope { p { padding-left:7px } } @media (min-width:600px) { p { color:blue } }",
            "p > { color:purple } @layer base, later; @layer base { p { margin-left:3px } } @layer later { p { margin-left:5px!important } }",
        ];
        let mut cache = StyleProgramCache::default();
        for width in [400.0, 800.0, 400.0] {
            for cached in [false, true] {
                let document = html_parse::parse_dom_document("<!doctype html><section id='a'><p id='one'>one</p></section><section id='b'><p id='two'>two</p></section><p id='outside'>outside</p>").unwrap();
                let node = |id| {
                    document
                        .node_ids()
                        .find(|&node| {
                            document
                                .try_element_ref(node)
                                .and_then(|element| element.id())
                                == Some(id)
                        })
                        .unwrap()
                };
                let inputs = [
                    AuthorStylesheetInput {
                        css: sheets[0],
                        implicit_scope_root: None,
                    },
                    AuthorStylesheetInput {
                        css: sheets[1],
                        implicit_scope_root: Some(node("a")),
                    },
                    AuthorStylesheetInput {
                        css: sheets[2],
                        implicit_scope_root: Some(node("b")),
                    },
                    AuthorStylesheetInput {
                        css: sheets[3],
                        implicit_scope_root: None,
                    },
                ];
                let environment = MediaEnvironment::screen(width, None).unwrap();
                let program = if cached {
                    cache.get_or_compile(&inputs, environment, 16.0).0
                } else {
                    Arc::new(StyleProgram::compile(&sheets, environment, 16.0))
                };
                let (styled, _, _) = crate::StyleRequest::new(document, &inputs).resolve(&program);
                for id in ["one", "two", "outside"] {
                    let node = styled
                        .document()
                        .node_ids()
                        .find(|&node| {
                            styled
                                .document()
                                .try_element_ref(node)
                                .and_then(|element| element.id())
                                == Some(id)
                        })
                        .unwrap();
                    let style = styled
                        .style_view(styled.style_for_node(node).unwrap())
                        .unwrap();
                    assert_eq!(
                        style.text.color,
                        if width < 600.0 {
                            0xff0000ff
                        } else {
                            0x0000ffff
                        }
                    );
                    assert_eq!(
                        style.background.background_color,
                        if id == "one" { 0x008000ff } else { 0 }
                    );
                    assert_eq!(
                        style.box_model.padding_left,
                        LengthPct::Px(if id == "two" { 7.0 } else { 0.0 })
                    );
                    assert_eq!(style.box_model.margin_left, LengthPct::Px(5.0));
                }
            }
        }
        assert_eq!((cache.stats().misses, cache.stats().hits), (2, 1));
        assert_eq!(
            // The text entry point resolves all four source identities on each load.
            (cache.stats().parsed_misses, cache.stats().parsed_hits),
            (4, 8)
        );
    }
}
