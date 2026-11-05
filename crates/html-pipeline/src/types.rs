use crate::engine::{ImageMetrics, NoteFlow, ReaderStyleOverrides};
use html_dom::{DocumentBuildError, InvalidRootFontSize, RootFontSize};
use html_engine_model::{LayoutConstraintError, PrepareError, ShapeError};
pub(crate) use html_engine_model::LayoutConstraints;
use html_parse::XmlParseError;
use html_style_model::reader_style;
use std::{error::Error, fmt, io};
use std::mem::swap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EarliestStage {
    Parse,
    Style,
    Prepare,
    Shape,
    Layout,
    Paint,
    None,
}

#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct SourceRevision(u64);

#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct ResourceRevision(u64);

#[derive(Clone, Copy, Eq, PartialEq, Debug, Hash)]
pub struct StylesheetRevision(u64);

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct FontEnvironmentRevision(u64);

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct ImageMetricsRevision(u64);

impl SourceRevision {
    pub const INITIAL: SourceRevision = SourceRevision(1);

    pub fn next(self) -> SourceRevision {
        SourceRevision(self.0.saturating_add(1))
    }
}

impl ResourceRevision {
    pub const INITIAL: ResourceRevision = ResourceRevision(1);

    pub fn next(self) -> ResourceRevision {
        ResourceRevision(self.0.saturating_add(1))
    }
}

impl StylesheetRevision {
    pub const INITIAL: StylesheetRevision = StylesheetRevision(1);

    pub fn next(self) -> StylesheetRevision {
        StylesheetRevision(self.0.saturating_add(1))
    }
}

impl FontEnvironmentRevision {
    pub const INITIAL: FontEnvironmentRevision = FontEnvironmentRevision(1);

    pub fn next(self) -> FontEnvironmentRevision {
        FontEnvironmentRevision(self.0.saturating_add(1))
    }
}

impl ImageMetricsRevision {
    pub const INITIAL: ImageMetricsRevision = ImageMetricsRevision(1);

    pub fn next(self) -> ImageMetricsRevision {
        ImageMetricsRevision(self.0.saturating_add(1))
    }
}

/// Validated root typography and media inputs. Text direction comes from CSS.
#[derive(Clone, Copy, Default, Eq, PartialEq, Debug)]
pub struct StyleEnvironment {
    pub root_font_size: RootFontSize,
    pub media: html_style::MediaEnvironment,
}

/// Complete owned inputs for loading or replacing a document. Once submitted,
/// borrow them through the session and use [`PipelineChanges`] for updates.
/// Revisions are assigned by the session from the reported changes.
#[derive(Clone, Debug)]
pub struct PipelineInputs {
    /// Source text owned by the session after preparation or a successful update.
    pub source: String,
    pub markup_syntax: html_parse::MarkupSyntax,
    /// Reader-authored CSS applied after the publication stylesheets.
    pub user_styles: Vec<String>,
    pub reader_overrides: ReaderStyleOverrides,
    /// Whether note bodies generate boxes. Changing it rebuilds the prepared
    /// stage, since it decides box generation rather than geometry.
    pub note_flow: NoteFlow,
    pub base_uri: String,
    pub style_environment: StyleEnvironment,
    pub layout: LayoutConstraints,
    /// Changed metric contents invalidate layout when resolved sizing uses them.
    pub image_metrics: ImageMetrics,
    pub(crate) source_revision: SourceRevision,
    pub(crate) resource_revision: ResourceRevision,
    pub(crate) stylesheet_revision: StylesheetRevision,
    pub(crate) font_environment: FontEnvironmentRevision,
    pub(crate) image_metrics_revision: ImageMetricsRevision,
}

impl PipelineInputs {
    /// Inputs with no reader styles, in-flow notes and no image metrics.
    pub fn new(
        source: String,
        markup_syntax: html_parse::MarkupSyntax,
        base_uri: String,
        style_environment: StyleEnvironment,
        layout: LayoutConstraints,
    ) -> Self {
        Self {
            source,
            markup_syntax,
            user_styles: Vec::new(),
            reader_overrides: ReaderStyleOverrides::default(),
            note_flow: NoteFlow::default(),
            base_uri,
            style_environment,
            layout,
            image_metrics: ImageMetrics::default(),
            source_revision: SourceRevision::INITIAL,
            resource_revision: ResourceRevision::INITIAL,
            stylesheet_revision: StylesheetRevision::INITIAL,
            font_environment: FontEnvironmentRevision::INITIAL,
            image_metrics_revision: ImageMetricsRevision::INITIAL,
        }
    }

    /// Identity of the provider resources used by the committed document.
    pub fn resource_revision(&self) -> ResourceRevision {
        self.resource_revision
    }

    pub(crate) fn revisions(&self) -> InputRevisions {
        InputRevisions {
            source: self.source_revision,
            resource: self.resource_revision,
            stylesheet: self.stylesheet_revision,
            font_environment: self.font_environment,
            image_metrics: self.image_metrics_revision,
        }
    }

    pub(crate) fn set_revisions(&mut self, revisions: InputRevisions) {
        self.source_revision = revisions.source;
        self.resource_revision = revisions.resource;
        self.stylesheet_revision = revisions.stylesheet;
        self.font_environment = revisions.font_environment;
        self.image_metrics_revision = revisions.image_metrics;
    }
}

/// Session-assigned revisions, advanced together from one batch of changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InputRevisions {
    source: SourceRevision,
    resource: ResourceRevision,
    stylesheet: StylesheetRevision,
    font_environment: FontEnvironmentRevision,
    image_metrics: ImageMetricsRevision,
}

/// Proposed changes to an existing session. Omitted fields keep their values.
///
/// Values move into the session on success. Failed updates restore the previous
/// inputs without cloning the source, stylesheets, or image metrics.
#[derive(Debug, Default)]
pub struct PipelineChanges {
    pub source: Option<String>,
    pub markup_syntax: Option<html_parse::MarkupSyntax>,
    pub user_styles: Option<Vec<String>>,
    pub reader_overrides: Option<ReaderStyleOverrides>,
    pub note_flow: Option<NoteFlow>,
    pub base_uri: Option<String>,
    pub style_environment: Option<StyleEnvironment>,
    pub layout: Option<LayoutConstraints>,
    pub image_metrics: Option<ImageMetrics>,
    /// Provider resources changed: reparse and reload stylesheets and images.
    pub resources_changed: bool,
    /// Linked stylesheet contents changed: restyle without reparsing.
    pub stylesheets_changed: bool,
    /// Available fonts or font metrics changed: reshape.
    pub fonts_changed: bool,
}

impl PipelineChanges {
    pub fn is_empty(&self) -> bool {
        self.source.is_none()
            && self.markup_syntax.is_none()
            && self.user_styles.is_none()
            && self.reader_overrides.is_none()
            && self.note_flow.is_none()
            && self.base_uri.is_none()
            && self.style_environment.is_none()
            && self.layout.is_none()
            && self.image_metrics.is_none()
            && !self.resources_changed
            && !self.stylesheets_changed
            && !self.fonts_changed
    }

    /// Revisions after applying these changes. Replaced source text always
    /// advances its revision; unchanged text keeps it.
    pub(crate) fn advance_revisions(&self, inputs: &PipelineInputs) -> InputRevisions {
        let mut revisions = inputs.revisions();
        if self
            .source
            .as_ref()
            .is_some_and(|source| source != &inputs.source)
        {
            revisions.source = revisions.source.next();
        }
        if self.resources_changed {
            revisions.resource = revisions.resource.next();
            revisions.image_metrics = revisions.image_metrics.next();
        }
        if self.resources_changed || self.stylesheets_changed {
            revisions.stylesheet = revisions.stylesheet.next();
        }
        if self.fonts_changed {
            revisions.font_environment = revisions.font_environment.next();
        }
        revisions
    }

    /// Swapping a second time restores every changed field after failure.
    pub(crate) fn swap_with(&mut self, inputs: &mut PipelineInputs) {
        fn swap_if_present<T>(change: &mut Option<T>, current: &mut T) {
            if let Some(value) = change {
                swap(value, current);
            }
        }

        swap_if_present(&mut self.source, &mut inputs.source);
        swap_if_present(&mut self.markup_syntax, &mut inputs.markup_syntax);
        swap_if_present(&mut self.user_styles, &mut inputs.user_styles);
        swap_if_present(&mut self.reader_overrides, &mut inputs.reader_overrides);
        swap_if_present(&mut self.note_flow, &mut inputs.note_flow);
        swap_if_present(&mut self.base_uri, &mut inputs.base_uri);
        swap_if_present(&mut self.style_environment, &mut inputs.style_environment);
        swap_if_present(&mut self.layout, &mut inputs.layout);
        swap_if_present(&mut self.image_metrics, &mut inputs.image_metrics);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParsedCacheKey {
    pub source_revision: SourceRevision,
    pub markup_syntax: html_parse::MarkupSyntax,
    pub resource_revision: ResourceRevision,
    /// Retained URI snapshot shared by nested stage keys.
    pub base_uri: Arc<str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedCacheKey {
    pub parsed: ParsedCacheKey,
    pub stylesheet_revision: StylesheetRevision,
    // Media dimensions use the retained media-query match key instead.
    pub root_font_size: RootFontSize,
    /// Cloned stage keys retain the same stylesheet and preference snapshots.
    pub user_styles: Arc<[String]>,
    pub reader_overrides: Arc<ReaderStyleOverrides>,
    pub note_flow: NoteFlow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShapedCacheKey {
    pub font_environment: FontEnvironmentRevision,
    pub prepared: PreparedCacheKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LayoutCacheKey {
    // Reject cheap layout-only changes before comparing nested stage inputs.
    pub image_metrics_revision: ImageMetricsRevision,
    /// Assigned by metric mutation, without retaining another snapshot owner.
    pub image_metrics_generation: u64,
    pub layout: LayoutConstraints,
    pub shaped: ShapedCacheKey,
}

impl PipelineInputs {
    /// Replacing source without advancing its revision still invalidates it.
    pub(crate) fn same_source(&self, other: &Self) -> bool {
        self.source == other.source
    }

    pub(crate) fn parsed_cache_key(&self) -> ParsedCacheKey {
        ParsedCacheKey { source_revision: self.source_revision, markup_syntax: self.markup_syntax, resource_revision: self.resource_revision, base_uri: Arc::from(self.base_uri.as_str()) }
    }

    pub(crate) fn prepared_cache_key(&self) -> PreparedCacheKey {
        PreparedCacheKey {
            parsed: self.parsed_cache_key(),
            stylesheet_revision: self.stylesheet_revision,
            root_font_size: self.style_environment.root_font_size,
            user_styles: self.user_styles.clone().into(),
            reader_overrides: Arc::new(reader_style::layout_overrides(&self.reader_overrides)),
            note_flow: self.note_flow,
        }
    }

    pub(crate) fn shaped_cache_key(&self) -> ShapedCacheKey {
        ShapedCacheKey { prepared: self.prepared_cache_key(), font_environment: self.font_environment }
    }

    pub(crate) fn layout_cache_key(&self) -> LayoutCacheKey {
        LayoutCacheKey {
            shaped: self.shaped_cache_key(),
            layout: self.layout,
            image_metrics_revision: self.image_metrics_revision,
            image_metrics_generation: self.image_metrics.cache_generation(),
        }
    }

    /// The layout key, sharing unchanged style payloads with the previous key.
    pub(crate) fn layout_key_reusing(
        &self,
        previous: Option<&PreparedCacheKey>,
        same_styles: bool,
        same_overrides: bool,
    ) -> LayoutCacheKey {
        let Some(previous) = previous else {
            return self.layout_cache_key();
        };
        let mut prepared = previous.clone();
        prepared.parsed.source_revision = self.source_revision;
        prepared.parsed.markup_syntax = self.markup_syntax;
        prepared.parsed.resource_revision = self.resource_revision;
        if prepared.parsed.base_uri.as_ref() != self.base_uri {
            prepared.parsed.base_uri = Arc::from(self.base_uri.as_str());
        }
        prepared.stylesheet_revision = self.stylesheet_revision;
        prepared.root_font_size = self.style_environment.root_font_size;
        prepared.note_flow = self.note_flow;
        if !same_styles {
            prepared.user_styles = self.user_styles.clone().into();
        }
        if !same_overrides {
            prepared.reader_overrides =
                Arc::new(reader_style::layout_overrides(&self.reader_overrides));
        }
        LayoutCacheKey {
            shaped: ShapedCacheKey {
                prepared,
                font_environment: self.font_environment,
            },
            layout: self.layout,
            image_metrics_revision: self.image_metrics_revision,
            image_metrics_generation: self.image_metrics.cache_generation(),
        }
    }
}

impl PartialEq for PipelineInputs {
    fn eq(&self, other: &Self) -> bool {
        self.same_source(other)
            && self.source_revision == other.source_revision
            && self.markup_syntax == other.markup_syntax
            && self.user_styles == other.user_styles
            && self.reader_overrides == other.reader_overrides
            && self.note_flow == other.note_flow
            && self.base_uri == other.base_uri
            && self.resource_revision == other.resource_revision
            && self.stylesheet_revision == other.stylesheet_revision
            && self.style_environment == other.style_environment
            && self.font_environment == other.font_environment
            && self.image_metrics_revision == other.image_metrics_revision
            && self.image_metrics == other.image_metrics
            && self.layout == other.layout
    }
}

/// A failed pipeline operation, retaining its machine-readable cause.
/// Error payloads allocate only on failure; successful updates carry no diagnostic state.
#[derive(Clone, Debug)]
pub struct PipelineError {
    kind: Arc<PipelineErrorKind>,
}

#[derive(Debug)]
pub enum PipelineErrorKind {
    Xml(XmlParseError),
    Dom(DocumentBuildError),
    Prepare(PrepareError),
    Shape(ShapeError),
    Constraints(LayoutConstraintError),
    RootFontSize(InvalidRootFontSize),
    Resource(io::Error),
    /// An update or rehydration was requested before a document was loaded.
    EmptySession,
    Message(String),
    Context { message: String, source: PipelineError },
}

impl PipelineError {
    pub fn message(message: impl Into<String>) -> Self {
        Self { kind: Arc::new(PipelineErrorKind::Message(message.into())) }
    }

    pub(crate) fn from_kind(kind: PipelineErrorKind) -> Self {
        Self { kind: Arc::new(kind) }
    }

    pub fn kind(&self) -> &PipelineErrorKind {
        &self.kind
    }

    pub fn with_context(self, message: impl Into<String>) -> Self {
        Self { kind: Arc::new(PipelineErrorKind::Context { message: message.into(), source: self }) }
    }
}

macro_rules! pipeline_error_from {
    ($($error:ty => $variant:ident),* $(,)?) => {
        $(impl From<$error> for PipelineError {
            fn from(error: $error) -> Self {
                Self { kind: Arc::new(PipelineErrorKind::$variant(error)) }
            }
        })*
    };
}

pipeline_error_from! {
    XmlParseError => Xml,
    DocumentBuildError => Dom,
    PrepareError => Prepare,
    ShapeError => Shape,
    LayoutConstraintError => Constraints,
    InvalidRootFontSize => RootFontSize,
    io::Error => Resource,
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            PipelineErrorKind::Xml(error) => error.fmt(f),
            PipelineErrorKind::Dom(error) => error.fmt(f),
            PipelineErrorKind::Prepare(error) => error.fmt(f),
            PipelineErrorKind::Shape(error) => error.fmt(f),
            PipelineErrorKind::Constraints(error) => error.fmt(f),
            PipelineErrorKind::RootFontSize(error) => error.fmt(f),
            PipelineErrorKind::Resource(error) => error.fmt(f),
            PipelineErrorKind::EmptySession => f.write_str("the pipeline session has no document"),
            PipelineErrorKind::Message(message) => f.write_str(message),
            PipelineErrorKind::Context { message, source } => write!(f, "{message}: {source}"),
        }
    }
}

impl Error for PipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.kind() {
            PipelineErrorKind::Xml(error) => Some(error),
            PipelineErrorKind::Dom(error) => Some(error),
            PipelineErrorKind::Prepare(error) => Some(error),
            PipelineErrorKind::Shape(error) => Some(error),
            PipelineErrorKind::Constraints(error) => Some(error),
            PipelineErrorKind::RootFontSize(error) => Some(error),
            PipelineErrorKind::Resource(error) => Some(error),
            PipelineErrorKind::Context { source, .. } => Some(source),
            PipelineErrorKind::EmptySession | PipelineErrorKind::Message(_) => None,
        }
    }
}
