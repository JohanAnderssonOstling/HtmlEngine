use html_layout::ImageMetrics;
use std::fmt;
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PipelineChange {
    SourceChanged(SourceRevision),
    ResourceBaseChanged(ResourceRevision),
    StylesheetsChanged(StylesheetRevision),
    StyleEnvironmentChanged(StyleEnvironment),
    FontEnvironmentChanged(FontEnvironmentRevision),
    ImageMetricsChanged(ImageMetricsRevision),
    LayoutConstraintsChanged(LayoutConstraints),
    PaintSettingsChanged(PaintSettingsRevision),
}

impl PipelineChange {
    pub const fn invalidated_stage(&self) -> EarliestStage {
        match self {
            Self::SourceChanged(_) => EarliestStage::Parse,
            Self::ResourceBaseChanged(_) => EarliestStage::Parse,
            Self::StylesheetsChanged(_) => EarliestStage::Style,
            Self::StyleEnvironmentChanged(_) => EarliestStage::Style,
            Self::FontEnvironmentChanged(_) => EarliestStage::Shape,
            Self::ImageMetricsChanged(_) => EarliestStage::Layout,
            Self::LayoutConstraintsChanged(_) => EarliestStage::Layout,
            Self::PaintSettingsChanged(_) => EarliestStage::Paint,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PipelineChangeMask {
    pub source_changed: bool,
    pub resource_base_changed: bool,
    pub stylesheets_changed: bool,
    pub style_environment_changed: bool,
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
            && !self.font_environment_changed
            && !self.image_metrics_changed
            && !self.layout_constraints_changed
            && !self.paint_settings_changed
    }

    pub const fn has_changes(self) -> bool {
        !self.is_empty()
    }

    pub fn earliest_stage(self) -> EarliestStage {
        let mut stage = EarliestStage::None;
        stage = stage.coalesce(if self.source_changed { EarliestStage::Parse } else { EarliestStage::None });
        stage = stage.coalesce(if self.resource_base_changed { EarliestStage::Parse } else { EarliestStage::None });
        stage = stage.coalesce(if self.stylesheets_changed { EarliestStage::Style } else { EarliestStage::None });
        stage = stage.coalesce(if self.style_environment_changed { EarliestStage::Style } else { EarliestStage::None });
        stage = stage.coalesce(if self.font_environment_changed { EarliestStage::Shape } else { EarliestStage::None });
        stage = stage.coalesce(if self.image_metrics_changed { EarliestStage::Layout } else { EarliestStage::None });
        stage = stage.coalesce(if self.layout_constraints_changed { EarliestStage::Layout } else { EarliestStage::None });
        stage.coalesce(if self.paint_settings_changed { EarliestStage::Paint } else { EarliestStage::None })
    }

    pub fn set(mut self, change: PipelineChange) -> Self {
        match change {
            PipelineChange::SourceChanged(_) => self.source_changed = true,
            PipelineChange::ResourceBaseChanged(_) => self.resource_base_changed = true,
            PipelineChange::StylesheetsChanged(_) => self.stylesheets_changed = true,
            PipelineChange::StyleEnvironmentChanged(_) => self.style_environment_changed = true,
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
            font_environment_changed: self.font_environment_changed || other.font_environment_changed,
            image_metrics_changed: self.image_metrics_changed || other.image_metrics_changed,
            layout_constraints_changed: self.layout_constraints_changed || other.layout_constraints_changed,
            paint_settings_changed: self.paint_settings_changed || other.paint_settings_changed,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
        use EarliestStage::*;
        let rank = |stage: EarliestStage| match stage {
            Parse => 0,
            Style => 1,
            Prepare => 2,
            Shape => 3,
            Layout => 4,
            Paint => 5,
            None => 6,
        };
        if rank(self) <= rank(other) { self } else { other }
    }
}

impl std::cmp::PartialOrd for EarliestStage {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for EarliestStage {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use EarliestStage::*;
        let rank = |stage: EarliestStage| match stage {
            Parse => 0,
            Style => 1,
            Prepare => 2,
            Shape => 3,
            Layout => 4,
            Paint => 5,
            None => 6,
        };
        rank(*self).cmp(&rank(*other))
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
    pub image_sizing_policy: html_layout::ImageSizingPolicy,
    pub text_composition_policy: html_layout::TextCompositionPolicy,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PipelineStageCounts {
    pub parsed: u32,
    pub styled: u32,
    pub prepared: u32,
    pub shaped: u32,
    pub laid_out: u32,
    pub paint: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PipelineRetainedBytes {
    pub parsed: usize,
    pub styled: usize,
    pub shaped: usize,
    pub laid_out: usize,
}

impl PipelineRetainedBytes {
    pub const fn total(self) -> usize {
        self.parsed + self.styled + self.shaped + self.laid_out
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PipelineTimings {
    pub change_cause: PipelineChangeMask,
    pub decision_time: Duration,
    pub stage_runs: PipelineStageCounts,
    pub stage_reuses: PipelineStageCounts,
    pub retained_bytes: PipelineRetainedBytes,
    pub shape_time: Duration,
    pub layout_time: Duration,
}

impl Default for PipelineTimings {
    fn default() -> Self {
        Self { change_cause: PipelineChangeMask::empty(), decision_time: Duration::ZERO, stage_runs: PipelineStageCounts::default(), stage_reuses: PipelineStageCounts::default(), retained_bytes: PipelineRetainedBytes::default(), shape_time: Duration::ZERO, layout_time: Duration::ZERO }
    }
}

#[derive(Clone, Debug)]
pub struct PipelineInputs {
    pub source: String,
    pub markup_syntax: html_parse::MarkupSyntax,
    /// Reader-authored CSS applied after the publication stylesheets.
    pub user_styles: Vec<String>,
    pub reader_overrides: html_layout::ReaderStyleOverrides,
    /// Whether note bodies generate boxes. Changing it rebuilds the prepared
    /// stage, since it decides box generation rather than geometry.
    pub note_flow: html_layout::NoteFlow,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedCacheKey {
    pub source_revision: SourceRevision,
    pub markup_syntax: html_parse::MarkupSyntax,
    pub resource_revision: ResourceRevision,
    pub base_uri: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedCacheKey {
    pub parsed: ParsedCacheKey,
    pub stylesheet_revision: StylesheetRevision,
    pub style_environment: StyleCacheEnvironment,
    pub user_styles: Vec<String>,
    pub reader_overrides: html_layout::ReaderStyleOverrides,
    pub note_flow: html_layout::NoteFlow,
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
}

impl PipelineInputs {
    pub fn change_mask(&self, previous: &Self) -> PipelineChangeMask {
        let mut mask = PipelineChangeMask::empty();
        if self.source != previous.source || self.source_revision != previous.source_revision || self.markup_syntax != previous.markup_syntax {
            mask.source_changed = true;
        }
        if self.base_uri != previous.base_uri || self.resource_revision != previous.resource_revision {
            mask.resource_base_changed = true;
        }
        if self.stylesheet_revision != previous.stylesheet_revision || self.user_styles != previous.user_styles || self.reader_overrides != previous.reader_overrides {
            mask.stylesheets_changed = true;
        }
        if !self.style_environment.same_non_media_part(&previous.style_environment) {
            mask.style_environment_changed = true;
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
        if self.paint != previous.paint {
            mask.paint_settings_changed = true;
        }
        mask
    }

    pub fn earliest_stage(&self, previous: &PipelineInputs) -> EarliestStage {
        self.change_mask(previous).earliest_stage()
    }

    pub fn changes_from(&self, previous: &Self) -> (Vec<PipelineChange>, EarliestStage) {
        let mut changes = Vec::new();

        if self.source != previous.source || self.source_revision != previous.source_revision || self.markup_syntax != previous.markup_syntax {
            changes.push(PipelineChange::SourceChanged(self.source_revision));
        }
        if self.base_uri != previous.base_uri || self.resource_revision != previous.resource_revision {
            changes.push(PipelineChange::ResourceBaseChanged(self.resource_revision));
        }
        if self.stylesheet_revision != previous.stylesheet_revision || self.user_styles != previous.user_styles || self.reader_overrides != previous.reader_overrides {
            changes.push(PipelineChange::StylesheetsChanged(self.stylesheet_revision));
        }
        if !self.style_environment.same_non_media_part(&previous.style_environment) {
            changes.push(PipelineChange::StyleEnvironmentChanged(self.style_environment));
        }
        if self.font_environment != previous.font_environment {
            changes.push(PipelineChange::FontEnvironmentChanged(self.font_environment));
        }
        if self.image_metrics_revision != previous.image_metrics_revision {
            changes.push(PipelineChange::ImageMetricsChanged(self.image_metrics_revision));
        }
        if !self.layout.same(previous.layout) {
            changes.push(PipelineChange::LayoutConstraintsChanged(self.layout));
        }

        if self.paint != previous.paint {
            changes.push(PipelineChange::PaintSettingsChanged(self.paint));
        }

        let earliest = changes.iter().fold(EarliestStage::None, |stage, change| stage.coalesce(change.invalidated_stage()));
        (changes, earliest)
    }

    pub fn parsed_cache_key(&self) -> ParsedCacheKey {
        ParsedCacheKey { source_revision: self.source_revision, markup_syntax: self.markup_syntax, resource_revision: self.resource_revision, base_uri: self.base_uri.clone() }
    }

    pub fn prepared_cache_key(&self) -> PreparedCacheKey {
        PreparedCacheKey {
            parsed: self.parsed_cache_key(),
            stylesheet_revision: self.stylesheet_revision,
            style_environment: self.style_environment.cache_key(),
            user_styles: self.user_styles.clone(),
            reader_overrides: self.reader_overrides.clone(),
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
        RenderOnlyCacheKey { layout: self.layout_cache_key(), paint: self.paint }
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
        self.source == other.source
            && self.source_revision == other.source_revision
            && self.markup_syntax == other.markup_syntax
            && self.user_styles == other.user_styles
            && self.reader_overrides == other.reader_overrides
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
    pub timings: PipelineTimings,
    pub anchors_preserved: bool,
}

impl PipelineUpdate {
    pub fn full_rebuild() -> Self {
        Self {
            stage: EarliestStage::Parse,
            timings: PipelineTimings {
                change_cause: PipelineChangeMask::empty(),
                decision_time: Duration::ZERO,
                stage_runs: PipelineStageCounts { parsed: 1, styled: 1, prepared: 1, shaped: 1, laid_out: 1, paint: 0 },
                stage_reuses: PipelineStageCounts::default(),
                retained_bytes: PipelineRetainedBytes::default(),
                shape_time: Duration::ZERO,
                layout_time: Duration::ZERO,
            },
            report: ReuseReport { parsed: Reuse::RECOMPUTED, styled: Reuse::RECOMPUTED, prepared: Reuse::RECOMPUTED, shaped: Reuse::RECOMPUTED, laid_out: Reuse::RECOMPUTED },
            anchors_preserved: false,
        }
    }
}
