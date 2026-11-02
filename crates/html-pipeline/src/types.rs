use crate::engine::{ImageMetrics, NoteFlow, ReaderStyleOverrides};
use html_dom::{DocumentBuildError, RootFontSize};
use html_engine_model::{LayoutConstraintError, PrepareError, ShapeError};
pub use html_engine_model::LayoutConstraints;
use html_parse::XmlParseError;
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

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct PaintSettingsRevision(u64);

impl PaintSettingsRevision {
    pub const INITIAL: PaintSettingsRevision = PaintSettingsRevision(1);

    pub fn next(self) -> PaintSettingsRevision {
        PaintSettingsRevision(self.0.saturating_add(1))
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct Reuse {
    pub reused: bool,
    pub stage: EarliestStage,
}

impl Reuse {
    pub const fn reused(stage: EarliestStage) -> Self {
        Self { reused: true, stage }
    }

    pub const fn recomputed(stage: EarliestStage) -> Self {
        Self { reused: false, stage }
    }

    pub const REUSED: Reuse = Reuse { reused: true, stage: EarliestStage::None };
    pub const RECOMPUTED: Reuse = Reuse { reused: false, stage: EarliestStage::None };
}

impl Default for Reuse {
    fn default() -> Self {
        Reuse::REUSED
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReuseReport {
    pub parsed: Reuse,
    pub styled: Reuse,
    pub prepared: Reuse,
    pub shaped: Reuse,
    pub laid_out: Reuse,
}

impl Default for ReuseReport {
    fn default() -> Self {
        Self { parsed: Reuse::REUSED, styled: Reuse::REUSED, prepared: Reuse::REUSED, shaped: Reuse::REUSED, laid_out: Reuse::RECOMPUTED }
    }
}

/// Complete owned inputs for loading or replacing a document. Once submitted,
/// borrow them through the session and use [`PipelineChanges`] for updates.
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
    pub source_revision: SourceRevision,
    pub base_uri: String,
    pub resource_revision: ResourceRevision,
    pub stylesheet_revision: StylesheetRevision,
    pub style_environment: StyleEnvironment,
    pub font_environment: FontEnvironmentRevision,
    /// Optional explicit invalidation; changed metric contents also invalidate layout.
    pub image_metrics_revision: ImageMetricsRevision,
    pub layout: LayoutConstraints,
    pub image_metrics: ImageMetrics,
    pub paint: PaintSettingsRevision,
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
    pub source_revision: Option<SourceRevision>,
    pub base_uri: Option<String>,
    pub resource_revision: Option<ResourceRevision>,
    pub stylesheet_revision: Option<StylesheetRevision>,
    pub style_environment: Option<StyleEnvironment>,
    pub font_environment: Option<FontEnvironmentRevision>,
    pub image_metrics_revision: Option<ImageMetricsRevision>,
    pub layout: Option<LayoutConstraints>,
    pub image_metrics: Option<ImageMetrics>,
    pub paint: Option<PaintSettingsRevision>,
}

impl PipelineChanges {
    pub fn is_empty(&self) -> bool {
        self.source.is_none()
            && self.markup_syntax.is_none()
            && self.user_styles.is_none()
            && self.reader_overrides.is_none()
            && self.note_flow.is_none()
            && self.source_revision.is_none()
            && self.base_uri.is_none()
            && self.resource_revision.is_none()
            && self.stylesheet_revision.is_none()
            && self.style_environment.is_none()
            && self.font_environment.is_none()
            && self.image_metrics_revision.is_none()
            && self.layout.is_none()
            && self.image_metrics.is_none()
            && self.paint.is_none()
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
        swap_if_present(&mut self.source_revision, &mut inputs.source_revision);
        swap_if_present(&mut self.base_uri, &mut inputs.base_uri);
        swap_if_present(&mut self.resource_revision, &mut inputs.resource_revision);
        swap_if_present(
            &mut self.stylesheet_revision,
            &mut inputs.stylesheet_revision,
        );
        swap_if_present(&mut self.style_environment, &mut inputs.style_environment);
        swap_if_present(&mut self.font_environment, &mut inputs.font_environment);
        swap_if_present(
            &mut self.image_metrics_revision,
            &mut inputs.image_metrics_revision,
        );
        swap_if_present(&mut self.layout, &mut inputs.layout);
        swap_if_present(&mut self.image_metrics, &mut inputs.image_metrics);
        swap_if_present(&mut self.paint, &mut inputs.paint);
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RenderOnlyCacheKey {
    pub layout: LayoutCacheKey,
    pub paint: PaintSettingsRevision,
    pub reader_paint: ReaderStyleOverrides,
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
            reader_overrides: Arc::new(self.reader_overrides.layout_overrides()),
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

    pub(crate) fn render_only_cache_key(&self) -> RenderOnlyCacheKey {
        RenderOnlyCacheKey { layout: self.layout_cache_key(), paint: self.paint, reader_paint: self.reader_overrides.paint_overrides() }
    }

    pub(crate) fn render_key_reusing(
        &self,
        previous: Option<&PreparedCacheKey>,
        same_styles: bool,
        same_overrides: bool,
    ) -> RenderOnlyCacheKey {
        let Some(previous) = previous else {
            return self.render_only_cache_key();
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
            prepared.reader_overrides = Arc::new(self.reader_overrides.layout_overrides());
        }
        RenderOnlyCacheKey {
            layout: LayoutCacheKey {
                shaped: ShapedCacheKey {
                    prepared,
                    font_environment: self.font_environment,
                },
                layout: self.layout,
                image_metrics_revision: self.image_metrics_revision,
                image_metrics_generation: self.image_metrics.cache_generation(),
            },
            paint: self.paint,
            reader_paint: self.reader_overrides.paint_overrides(),
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
            && self.paint == other.paint
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
    Resource(io::Error),
    Message(String),
    Context { message: String, source: PipelineError },
}

impl PipelineError {
    pub fn message(message: impl Into<String>) -> Self {
        Self { kind: Arc::new(PipelineErrorKind::Message(message.into())) }
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
            PipelineErrorKind::Resource(error) => error.fmt(f),
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
            PipelineErrorKind::Resource(error) => Some(error),
            PipelineErrorKind::Context { source, .. } => Some(source),
            PipelineErrorKind::Message(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct PipelineUpdate {
    pub stage: EarliestStage,
    pub report: ReuseReport,
    pub anchors_preserved: bool,
}

impl PipelineUpdate {
    pub fn full_rebuild() -> Self {
        Self {
            stage: EarliestStage::Parse,
            report: ReuseReport { parsed: Reuse::RECOMPUTED, styled: Reuse::RECOMPUTED, prepared: Reuse::RECOMPUTED, shaped: Reuse::RECOMPUTED, laid_out: Reuse::RECOMPUTED },
            anchors_preserved: false,
        }
    }
}
