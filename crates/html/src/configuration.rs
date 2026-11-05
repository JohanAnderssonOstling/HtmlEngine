//! Hosts submit document inputs; the engine owns invalidation and stage reuse.
use html_dom as dom_values;
use crate::document::RootFontSize;
use crate::{Document, Error};
use crate::layout::{ImageSizingPolicy, NoteFlow, TextCompositionPolicy};
use crate::pipeline::{EarliestStage, PipelineChanges, PipelineInputs};
use crate::style::ReaderStyleOverrides;
use crate::text::GlyphShaper;
use html_style::MediaEnvironment;

pub(crate) fn normalized_font_size(size: f32) -> Result<RootFontSize, Error> {
    let size = dom_values::root_font_size_new(size)?;
    // Preserve whole-pixel rounding and u32 saturation at the input boundary.
    Ok(dom_values::root_font_size_new((size.get().max(1.0).round() as u32) as f32)?)
}

/// Changes to published document products, independent of internal stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Update {
    Unchanged,
    Paint,
    Layout { anchors_preserved: bool },
}

impl Update {
    pub(crate) fn from_pipeline(stage: EarliestStage) -> Self {
        match stage {
            EarliestStage::None => Self::Unchanged,
            EarliestStage::Paint => Self::Paint,
            _ => Self::Layout {
                anchors_preserved: stage == EarliestStage::Layout,
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
    pub(crate) fn height(self) -> Option<f64> {
        self.inputs.layout.viewport_height()
    }
    pub fn font_size(self) -> RootFontSize {
        self.inputs.style_environment.root_font_size
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
    pub(crate) fn source(mut self, source: String) -> Self {
        self.source = Some(source);
        self
    }
    pub(crate) fn user_styles(mut self, styles: Vec<String>) -> Self {
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
            .expect("validated layout dimensions form a media environment");
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
            user_styles: self.user_styles,
            reader_overrides: self.reader_overrides,
            note_flow: self.note_flow,
            fonts_changed: self.fonts_changed,
            ..Default::default()
        })
    }
}

impl Document {
    pub fn configuration(&self) -> DocumentConfiguration<'_> {
        DocumentConfiguration {
            inputs: self.inputs().expect("loaded engine owns document inputs"),
        }
    }

    /// Apply a batch of input changes to the document.
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
        if changes.source.is_none() && !changes.resources_changed {
            changes.image_metrics = self.images.pending_metrics().cloned();
        }
        let result = self.apply_changes(changes, shaper)?;
        self.images.clear_pending();
        Ok(Update::from_pipeline(result))
    }
}
