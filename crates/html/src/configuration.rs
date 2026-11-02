//! Hosts submit document inputs; the engine owns invalidation and stage reuse.
use crate::document::RootFontSize;
use crate::engine::{Engine, Error};
use crate::layout::{ImageSizingPolicy, NoteFlow, TextCompositionPolicy};
use crate::pipeline::{EarliestStage, PipelineChanges, PipelineInputs, PipelineUpdate};
use crate::style::ReaderStyleOverrides;
use crate::text::GlyphShaper;
use html_style::MediaEnvironment;

pub(crate) fn normalized_font_size(size: f32) -> Result<RootFontSize, Error> {
    let size = RootFontSize::new(size).map_err(|error| Error::message(error.to_string()))?;
    // Preserve whole-pixel rounding and u32 saturation at the input boundary.
    RootFontSize::new((size.get().max(1.0).round() as u32) as f32)
        .map_err(|error| Error::message(error.to_string()))
}

/// Changes to published document products, independent of internal stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Update {
    Unchanged,
    Paint,
    Layout { anchors_preserved: bool },
}

impl Update {
    pub(crate) fn from_pipeline(update: PipelineUpdate) -> Self {
        match update.stage {
            EarliestStage::None => Self::Unchanged,
            EarliestStage::Paint => Self::Paint,
            _ => Self::Layout {
                anchors_preserved: update.anchors_preserved,
            },
        }
    }
}

/// Borrowed committed configuration. Computed styles, cache keys and stage
/// revisions are owned by the engine and are not part of this view.
#[derive(Clone, Copy)]
pub struct DocumentConfiguration<'a> {
    pub(crate) inputs: &'a PipelineInputs,
}

impl<'a> DocumentConfiguration<'a> {
    pub fn width(self) -> f64 {
        self.inputs.layout.viewport_width()
    }
    pub fn height(self) -> Option<f64> {
        self.inputs.layout.viewport_height()
    }
    pub fn font_size(self) -> RootFontSize {
        self.inputs.style_environment.root_font_size
    }
    pub fn user_styles(self) -> &'a [String] {
        &self.inputs.user_styles
    }
    pub fn reader_overrides(self) -> &'a ReaderStyleOverrides {
        &self.inputs.reader_overrides
    }
    pub fn image_sizing_policy(self) -> ImageSizingPolicy {
        self.inputs.layout.image_sizing_policy()
    }
    pub fn text_composition_policy(self) -> TextCompositionPolicy {
        self.inputs.layout.text_composition_policy()
    }
    pub fn note_flow(self) -> NoteFlow {
        self.inputs.note_flow
    }
}

/// A batch of input changes. Omitted values retain the committed configuration.
/// Validation happens before applying any changes; failures retain prior inputs.
#[derive(Debug, Default)]
pub struct DocumentUpdate {
    width: Option<f64>,
    height: Option<Option<f64>>,
    font_size: Option<f32>,
    source: Option<String>,
    user_styles: Option<Vec<String>>,
    reader_overrides: Option<ReaderStyleOverrides>,
    image_sizing_policy: Option<ImageSizingPolicy>,
    text_composition_policy: Option<TextCompositionPolicy>,
    note_flow: Option<NoteFlow>,
    fonts_changed: bool,
    stylesheets_changed: bool,
    resources_changed: bool,
}

impl DocumentUpdate {
    pub fn width(mut self, width: f64) -> Self {
        self.width = Some(width);
        self
    }
    /// `None` clears a previous height constraint; omitting this setter keeps it.
    pub fn height(mut self, height: Option<f64>) -> Self {
        self.height = Some(height);
        self
    }
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = Some(size);
        self
    }
    /// Replace markup at the current URI, retaining its document syntax.
    pub fn source(mut self, source: String) -> Self {
        self.source = Some(source);
        self
    }
    pub fn user_styles(mut self, styles: Vec<String>) -> Self {
        self.user_styles = Some(styles);
        self
    }
    pub fn reader_overrides(mut self, overrides: ReaderStyleOverrides) -> Self {
        self.reader_overrides = Some(overrides);
        self
    }
    pub fn image_sizing_policy(mut self, policy: ImageSizingPolicy) -> Self {
        self.image_sizing_policy = Some(policy);
        self
    }
    pub fn text_composition_policy(mut self, policy: TextCompositionPolicy) -> Self {
        self.text_composition_policy = Some(policy);
        self
    }
    pub fn note_flow(mut self, flow: NoteFlow) -> Self {
        self.note_flow = Some(flow);
        self
    }
    /// Notify the engine that the host's available fonts or font metrics changed.
    pub fn fonts_changed(mut self) -> Self {
        self.fonts_changed = true;
        self
    }
    /// Notify the engine that provider-backed CSS changed at existing URIs.
    pub fn stylesheets_changed(mut self) -> Self {
        self.stylesheets_changed = true;
        self
    }
    /// Notify the engine that provider-backed resources changed at existing URIs.
    pub fn resources_changed(mut self) -> Self {
        self.resources_changed = true;
        self
    }
    /// Lets a text backend adopt proposed font preferences before shaping.
    pub fn overrides(&self) -> Option<&ReaderStyleOverrides> {
        self.reader_overrides.as_ref()
    }

    fn into_changes(self, inputs: &PipelineInputs) -> Result<PipelineChanges, Error> {
        let mut layout = inputs.layout;
        let mut environment = inputs.style_environment;
        if let Some(width) = self.width {
            layout = layout.with_viewport_width(width)?;
        }
        if let Some(height) = self.height {
            layout = layout.with_viewport_height(height)?;
        }
        if self.width.is_some() || self.height.is_some() {
            environment.media = MediaEnvironment::new(
                environment.media.media_type(),
                layout.viewport_width(),
                layout.viewport_height(),
            )
            .ok_or_else(|| Error::message("document dimensions must be finite and nonnegative"))?;
        }
        if let Some(size) = self.font_size {
            environment.root_font_size = normalized_font_size(size)?;
            layout = layout.with_line_height(f64::from(environment.root_font_size.get() as u32))?;
        }
        if let Some(policy) = self.image_sizing_policy {
            layout = layout.with_image_sizing_policy(policy);
        }
        if let Some(policy) = self.text_composition_policy {
            layout = layout.with_text_composition_policy(policy);
        }
        let source_changed = self
            .source
            .as_ref()
            .is_some_and(|source| source != &inputs.source);
        Ok(PipelineChanges {
            layout: (layout != inputs.layout).then_some(layout),
            style_environment: (environment != inputs.style_environment).then_some(environment),
            source: self.source.filter(|_| source_changed),
            source_revision: source_changed.then(|| inputs.source_revision.next()),
            user_styles: self.user_styles,
            reader_overrides: self.reader_overrides,
            note_flow: self.note_flow,
            font_environment: self.fonts_changed.then(|| inputs.font_environment.next()),
            stylesheet_revision: self
                .stylesheets_changed
                .then(|| inputs.stylesheet_revision.next()),
            resource_revision: self
                .resources_changed
                .then(|| inputs.resource_revision.next()),
            ..Default::default()
        })
    }
}

impl Engine {
    pub fn configuration(&self) -> DocumentConfiguration<'_> {
        DocumentConfiguration {
            inputs: self.inputs().expect("loaded engine owns document inputs"),
        }
    }

    /// Apply a batch, or finish a prepared document with a default batch.
    /// The engine synchronizes CSS media inputs and chooses
    /// the minimum required work, including media-query-dependent restyling.
    pub fn apply(
        &mut self,
        update: DocumentUpdate,
        shaper: &mut impl GlyphShaper,
    ) -> Result<Update, Error> {
        let mut changes =
            update.into_changes(self.inputs().expect("loaded engine owns document inputs"))?;
        // Metrics remain engine-owned, so deferred resource work can join a
        // viewport/style batch without exposing a stale input snapshot.
        if changes.source_revision.is_none() && changes.resource_revision.is_none() {
            changes.image_metrics = self.pending_image_snapshot.clone();
        }
        let result = self.apply_changes(changes, shaper)?;
        self.pending_image_snapshot = None;
        self.pending_image_metrics.clear();
        Ok(Update::from_pipeline(result))
    }
}
