use crate::engine::{ImageMetrics, NoteFlow, ReaderStyleOverrides};
use std::fmt;
use std::mem::swap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineChange {
    SourceChanged(SourceRevision),
    ResourceBaseChanged(ResourceRevision),
    StylesheetsChanged(StylesheetRevision),
    StyleEnvironmentChanged(StyleEnvironment),
    NoteFlowChanged(NoteFlow),
    FontEnvironmentChanged(FontEnvironmentRevision),
    ImageMetricsChanged(ImageMetricsRevision),
    LayoutConstraintsChanged(LayoutConstraints),
    PaintSettingsChanged(PaintSettingsRevision),
}

impl PipelineChange {
    pub const fn invalidated_stage(&self) -> EarliestStage {
        PipelineChangeMask::empty().set(*self).earliest_stage()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PipelineChangeMask {
    pub source_changed: bool,
    pub resource_base_changed: bool,
    pub stylesheets_changed: bool,
    pub style_environment_changed: bool,
    pub note_flow_changed: bool,
    pub font_environment_changed: bool,
    pub image_metrics_changed: bool,
    pub layout_constraints_changed: bool,
    pub paint_settings_changed: bool,
}

impl PipelineChangeMask {
    pub const fn empty() -> Self {
        Self {
            source_changed: false,
            resource_base_changed: false,
            stylesheets_changed: false,
            style_environment_changed: false,
            note_flow_changed: false,
            font_environment_changed: false,
            image_metrics_changed: false,
            layout_constraints_changed: false,
            paint_settings_changed: false,
        }
    }

    pub const fn is_empty(self) -> bool {
        !self.source_changed
            && !self.resource_base_changed
            && !self.stylesheets_changed
            && !self.style_environment_changed
            && !self.note_flow_changed
            && !self.font_environment_changed
            && !self.image_metrics_changed
            && !self.layout_constraints_changed
            && !self.paint_settings_changed
    }

    pub const fn has_changes(self) -> bool {
        !self.is_empty()
    }

    pub const fn earliest_stage(self) -> EarliestStage {
        if self.source_changed || self.resource_base_changed {
            EarliestStage::Parse
        } else if self.stylesheets_changed || self.style_environment_changed {
            EarliestStage::Style
        } else if self.note_flow_changed {
            EarliestStage::Prepare
        } else if self.font_environment_changed {
            EarliestStage::Shape
        } else if self.image_metrics_changed || self.layout_constraints_changed {
            EarliestStage::Layout
        } else if self.paint_settings_changed {
            EarliestStage::Paint
        } else {
            EarliestStage::None
        }
    }

    pub const fn set(mut self, change: PipelineChange) -> Self {
        match change {
            PipelineChange::SourceChanged(_) => self.source_changed = true,
            PipelineChange::ResourceBaseChanged(_) => self.resource_base_changed = true,
            PipelineChange::StylesheetsChanged(_) => self.stylesheets_changed = true,
            PipelineChange::StyleEnvironmentChanged(_) => self.style_environment_changed = true,
            PipelineChange::NoteFlowChanged(_) => self.note_flow_changed = true,
            PipelineChange::FontEnvironmentChanged(_) => self.font_environment_changed = true,
            PipelineChange::ImageMetricsChanged(_) => self.image_metrics_changed = true,
            PipelineChange::LayoutConstraintsChanged(_) => self.layout_constraints_changed = true,
            PipelineChange::PaintSettingsChanged(_) => self.paint_settings_changed = true,
        }
        self
    }

    pub const fn merge(self, other: Self) -> Self {
        Self {
            source_changed: self.source_changed || other.source_changed,
            resource_base_changed: self.resource_base_changed || other.resource_base_changed,
            stylesheets_changed: self.stylesheets_changed || other.stylesheets_changed,
            style_environment_changed: self.style_environment_changed || other.style_environment_changed,
            note_flow_changed: self.note_flow_changed || other.note_flow_changed,
            font_environment_changed: self.font_environment_changed || other.font_environment_changed,
            image_metrics_changed: self.image_metrics_changed || other.image_metrics_changed,
            layout_constraints_changed: self.layout_constraints_changed || other.layout_constraints_changed,
            paint_settings_changed: self.paint_settings_changed || other.paint_settings_changed,
        }
    }
}

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

impl EarliestStage {
    pub fn coalesce(self, other: EarliestStage) -> EarliestStage {
        self.min(other)
    }
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

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct StyleEnvironment {
    pub root_font_size: u32,
    pub media: html_style::MediaEnvironment,
    pub direction: u8,
}

impl StyleEnvironment {
    pub fn same_non_media_part(&self, other: &Self) -> bool {
        self.root_font_size == other.root_font_size && self.direction == other.direction
    }

    pub fn cache_key(self) -> StyleCacheEnvironment {
        StyleCacheEnvironment { root_font_size: self.root_font_size, direction: self.direction }
    }
}

/// Style inputs that always require recascading. Media dimensions are omitted:
/// the session compares their retained media-query match key instead.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub struct StyleCacheEnvironment {
    pub root_font_size: u32,
    pub direction: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutConstraints {
    pub viewport_width: f64,
    pub viewport_height: Option<f64>,
    pub line_height: f64,
    pub image_sizing_policy: crate::engine::ImageSizingPolicy,
    pub text_composition_policy: crate::engine::TextCompositionPolicy,
}

impl Eq for LayoutConstraints {}

impl LayoutConstraints {
    pub fn same(self, other: Self) -> bool {
        self.viewport_width.to_bits() == other.viewport_width.to_bits()
            && self.viewport_height.map(f64::to_bits) == other.viewport_height.map(f64::to_bits)
            && self.line_height.to_bits() == other.line_height.to_bits()
            && self.image_sizing_policy == other.image_sizing_policy
            && self.text_composition_policy == other.text_composition_policy
    }
}

impl PartialEq for LayoutConstraints {
    fn eq(&self, other: &Self) -> bool {
        self.same(*other)
    }
}

impl Default for StyleEnvironment {
    fn default() -> Self {
        Self { root_font_size: 16, media: html_style::MediaEnvironment::default(), direction: 0 }
    }
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
        if let Some(value) = &mut self.source {
            swap(value, &mut inputs.source);
        }
        if let Some(value) = &mut self.markup_syntax {
            swap(value, &mut inputs.markup_syntax);
        }
        if let Some(value) = &mut self.user_styles {
            swap(value, &mut inputs.user_styles);
        }
        if let Some(value) = &mut self.reader_overrides {
            swap(value, &mut inputs.reader_overrides);
        }
        if let Some(value) = &mut self.note_flow {
            swap(value, &mut inputs.note_flow);
        }
        if let Some(value) = &mut self.source_revision {
            swap(value, &mut inputs.source_revision);
        }
        if let Some(value) = &mut self.base_uri {
            swap(value, &mut inputs.base_uri);
        }
        if let Some(value) = &mut self.resource_revision {
            swap(value, &mut inputs.resource_revision);
        }
        if let Some(value) = &mut self.stylesheet_revision {
            swap(value, &mut inputs.stylesheet_revision);
        }
        if let Some(value) = &mut self.style_environment {
            swap(value, &mut inputs.style_environment);
        }
        if let Some(value) = &mut self.font_environment {
            swap(value, &mut inputs.font_environment);
        }
        if let Some(value) = &mut self.image_metrics_revision {
            swap(value, &mut inputs.image_metrics_revision);
        }
        if let Some(value) = &mut self.layout {
            swap(value, &mut inputs.layout);
        }
        if let Some(value) = &mut self.image_metrics {
            swap(value, &mut inputs.image_metrics);
        }
        if let Some(value) = &mut self.paint {
            swap(value, &mut inputs.paint);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedCacheKey {
    pub source_revision: SourceRevision,
    pub markup_syntax: html_parse::MarkupSyntax,
    pub resource_revision: ResourceRevision,
    /// Retained URI snapshot shared by nested stage keys.
    pub base_uri: Arc<str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedCacheKey {
    pub parsed: ParsedCacheKey,
    pub stylesheet_revision: StylesheetRevision,
    pub style_environment: StyleCacheEnvironment,
    /// Cloned stage keys retain the same stylesheet and preference snapshots.
    pub user_styles: Arc<[String]>,
    pub reader_overrides: Arc<ReaderStyleOverrides>,
    pub note_flow: NoteFlow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShapedCacheKey {
    pub prepared: PreparedCacheKey,
    pub font_environment: FontEnvironmentRevision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutCacheKey {
    pub shaped: ShapedCacheKey,
    pub layout: LayoutConstraints,
    pub image_metrics_revision: ImageMetricsRevision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderOnlyCacheKey {
    pub layout: LayoutCacheKey,
    pub paint: PaintSettingsRevision,
    pub reader_paint: ReaderStyleOverrides,
}

impl PipelineInputs {
    /// Replacing source without advancing its revision still invalidates it.
    pub fn same_source(&self, other: &Self) -> bool {
        self.source == other.source
    }

    pub fn change_mask(&self, previous: &Self) -> PipelineChangeMask {
        let mut mask = PipelineChangeMask::empty();
        if !self.same_source(previous) || self.source_revision != previous.source_revision || self.markup_syntax != previous.markup_syntax {
            mask.source_changed = true;
        }
        if self.base_uri != previous.base_uri || self.resource_revision != previous.resource_revision {
            mask.resource_base_changed = true;
        }
        if self.stylesheet_revision != previous.stylesheet_revision || self.user_styles != previous.user_styles || !self.reader_overrides.same_layout_preferences(&previous.reader_overrides) {
            mask.stylesheets_changed = true;
        }
        if !self.style_environment.same_non_media_part(&previous.style_environment) {
            mask.style_environment_changed = true;
        }
        if self.note_flow != previous.note_flow {
            mask.note_flow_changed = true;
        }
        if self.font_environment != previous.font_environment {
            mask.font_environment_changed = true;
        }
        if self.image_metrics_revision != previous.image_metrics_revision {
            mask.image_metrics_changed = true;
        }
        if !self.layout.same(previous.layout) {
            mask.layout_constraints_changed = true;
        }
        if self.paint != previous.paint
            || !self.reader_overrides.same_paint_preferences(&previous.reader_overrides)
        {
            mask.paint_settings_changed = true;
        }
        mask
    }

    pub fn earliest_stage(&self, previous: &PipelineInputs) -> EarliestStage {
        self.change_mask(previous).earliest_stage()
    }

    pub fn changes_from(&self, previous: &Self) -> (Vec<PipelineChange>, EarliestStage) {
        let mask = self.change_mask(previous);
        let mut changes = Vec::new();

        if mask.source_changed {
            changes.push(PipelineChange::SourceChanged(self.source_revision));
        }
        if mask.resource_base_changed {
            changes.push(PipelineChange::ResourceBaseChanged(self.resource_revision));
        }
        if mask.stylesheets_changed {
            changes.push(PipelineChange::StylesheetsChanged(self.stylesheet_revision));
        }
        if mask.style_environment_changed {
            changes.push(PipelineChange::StyleEnvironmentChanged(self.style_environment));
        }
        if mask.note_flow_changed {
            changes.push(PipelineChange::NoteFlowChanged(self.note_flow));
        }
        if mask.font_environment_changed {
            changes.push(PipelineChange::FontEnvironmentChanged(self.font_environment));
        }
        if mask.image_metrics_changed {
            changes.push(PipelineChange::ImageMetricsChanged(self.image_metrics_revision));
        }
        if mask.layout_constraints_changed {
            changes.push(PipelineChange::LayoutConstraintsChanged(self.layout));
        }

        if mask.paint_settings_changed {
            changes.push(PipelineChange::PaintSettingsChanged(self.paint));
        }

        (changes, mask.earliest_stage())
    }

    pub fn parsed_cache_key(&self) -> ParsedCacheKey {
        ParsedCacheKey { source_revision: self.source_revision, markup_syntax: self.markup_syntax, resource_revision: self.resource_revision, base_uri: Arc::from(self.base_uri.as_str()) }
    }

    pub fn prepared_cache_key(&self) -> PreparedCacheKey {
        PreparedCacheKey {
            parsed: self.parsed_cache_key(),
            stylesheet_revision: self.stylesheet_revision,
            style_environment: self.style_environment.cache_key(),
            user_styles: self.user_styles.clone().into(),
            reader_overrides: Arc::new(self.reader_overrides.layout_overrides()),
            note_flow: self.note_flow,
        }
    }

    pub fn shaped_cache_key(&self) -> ShapedCacheKey {
        ShapedCacheKey { prepared: self.prepared_cache_key(), font_environment: self.font_environment }
    }

    pub fn layout_cache_key(&self) -> LayoutCacheKey {
        LayoutCacheKey { shaped: self.shaped_cache_key(), layout: self.layout, image_metrics_revision: self.image_metrics_revision }
    }

    pub fn render_only_cache_key(&self) -> RenderOnlyCacheKey {
        RenderOnlyCacheKey { layout: self.layout_cache_key(), paint: self.paint, reader_paint: self.reader_overrides.paint_overrides() }
    }

    pub fn cache_key(&self) -> PipelineCacheKey {
        PipelineCacheKey {
            source: self.source_revision,
            resource: self.resource_revision,
            stylesheets: self.stylesheet_revision,
            style_environment: self.style_environment,
            font_environment: self.font_environment,
            image_metrics: self.image_metrics_revision,
            layout: self.layout,
            paint: self.paint,
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
            && self.layout.same(other.layout)
            && self.paint == other.paint
    }
}

#[derive(Clone, Debug)]
pub struct PipelineError(pub String);

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PipelineError {}

#[derive(Clone, Debug)]
pub struct PipelineCacheKey {
    pub source: SourceRevision,
    pub resource: ResourceRevision,
    pub stylesheets: StylesheetRevision,
    pub style_environment: StyleEnvironment,
    pub font_environment: FontEnvironmentRevision,
    pub image_metrics: ImageMetricsRevision,
    pub layout: LayoutConstraints,
    pub paint: PaintSettingsRevision,
}

impl PartialEq for PipelineCacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
            && self.resource == other.resource
            && self.stylesheets == other.stylesheets
            && self.style_environment == other.style_environment
            && self.font_environment == other.font_environment
            && self.image_metrics == other.image_metrics
            && self.layout == other.layout
            && self.paint == other.paint
    }
}

impl Eq for PipelineCacheKey {}

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
