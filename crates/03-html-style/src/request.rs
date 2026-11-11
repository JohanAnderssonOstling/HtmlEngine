//! Configurable entry point over stylesheet compilation and document resolution.
use crate::style::program::apply;
use crate::{
    AuthorStylesheetInput, MediaEnvironment, MediaQuerySet, StyleProgram, StyleProgramCache,
    StyleTimings, StyledDocument,
};
use html_dom::Document;
use html_style_model::ReaderStyleOverrides;

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
            inputs: inputs
                .iter()
                .map(|input| AuthorStylesheetInput {
                    css: input.css,
                    implicit_scope_root: input.implicit_scope_root,
                })
                .collect(),
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
        apply(
            self.document,
            program,
            &self.inputs,
            timings,
            self.reader_overrides
                .unwrap_or(&ReaderStyleOverrides::default()),
        )
    }
}

/// Resolve ordinary author CSS with default environment and reader options.
pub fn style_document(document: Document, css: &[&str]) -> StyledDocument {
    StyleRequest::from_css(document, css).compile().0
}
