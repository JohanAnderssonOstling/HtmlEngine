//! Consistent document layout, typography and media updates. Hosts choose values.
use crate::engine::{Engine, Error};
use crate::pipeline::{MediaEnvironment, PipelineChanges};

#[derive(Clone, Copy, Debug, Default)]
pub struct DocumentUpdate {
    width: Option<f64>,
    height: Option<Option<f64>>,
    font_size: Option<f32>,
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
}

impl Engine {
    /// Build changes without applying them, so hosts can preserve their own
    /// reading position or merge additional resource/style changes first.
    pub fn configuration_changes(&self, update: DocumentUpdate) -> Result<PipelineChanges, Error> {
        let inputs = self
            .inputs()
            .ok_or_else(|| Error("document has not been prepared".into()))?;
        let mut layout = inputs.layout;
        let mut environment = inputs.style_environment;
        if let Some(width) = update.width {
            layout.viewport_width = width;
        }
        if let Some(height) = update.height {
            if height.is_some_and(|height| !height.is_finite() || height < 0.0) {
                return Err(Error(
                    "document height must be finite and nonnegative".into(),
                ));
            }
            layout.viewport_height = height;
        }
        if update.width.is_some() || update.height.is_some() {
            environment.media = MediaEnvironment::new(
                environment.media.media_type(),
                layout.viewport_width,
                layout.viewport_height.filter(|height| *height > 0.0),
            )
            .ok_or_else(|| Error("document width must be finite and positive".into()))?;
        }
        if let Some(size) = update.font_size {
            if !size.is_finite() || size <= 0.0 {
                return Err(Error("font size must be finite and positive".into()));
            }
            environment.root_font_size = size.max(1.0).round() as u32;
            layout.line_height = f64::from(environment.root_font_size);
        }
        Ok(PipelineChanges {
            layout: Some(layout),
            style_environment: Some(environment),
            ..Default::default()
        })
    }
}
