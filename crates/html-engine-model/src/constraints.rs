use html_style_model::ComputedStylesValidationError;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutConstraints {
    viewport_width: f64,
    viewport_height: Option<f64>,
    line_height: f64,
    image_sizing_policy: ImageSizingPolicy,
    text_composition_policy: TextCompositionPolicy,
    parallel_workers: usize,
}

/// Optional renderer-level adaptation for standalone images. Web-compatible
/// sizing remains the default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageSizingPolicy {
    #[default]
    WebCompatible,
    /// Make a sufficiently large image fill its column when it is the sole
    /// meaningful content of its block, overriding authored horizontal image
    /// sizing and margins while preserving its aspect ratio.
    SmartStandalone,
}

/// Whether note bodies generate boxes in the reading flow.
///
/// Notes are recognised by [`html_dom::element_is_note_target`], so an embedder
/// that holds notes back for its own presentation never has to restate what a
/// note is. Excluding them suppresses box generation exactly as `display: none`
/// would, leaving the subtree in the DOM so link targets and scoped layout still
/// resolve.
///
/// In-flow is the default: markup lays out as authored unless an embedder asks
/// otherwise.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NoteFlow {
    #[default]
    InFlow,
    Excluded,
}

/// Selects browser-compatible first-fit wrapping or paragraph-wide book
/// composition. Web-compatible wrapping remains the public default so layout
/// tests and embedders do not silently acquire different line breaks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextCompositionPolicy {
    #[default]
    WebCompatible,
    /// Paragraph-wide book composition with conservative hyphenation quality
    /// controls enabled.
    BookOptimized,
    /// Paragraph-wide book composition without the optional consecutive-run,
    /// paragraph-ending, fragment-length, and short-line hyphen safeguards.
    BookOptimizedUnrestrictedHyphenation,
}

impl TextCompositionPolicy {
    pub fn is_book_optimized(self) -> bool {
        matches!(
            self,
            Self::BookOptimized | Self::BookOptimizedUnrestrictedHyphenation
        )
    }

    pub fn uses_hyphenation_quality(self) -> bool {
        self == Self::BookOptimized
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutConstraintError {
    InvalidViewportWidth,
    InvalidViewportHeight,
    InvalidLineHeight,
}

impl LayoutConstraints {
    pub fn new(viewport_width: f64, line_height: f64) -> Result<Self, LayoutConstraintError> {
        if !viewport_width.is_finite() || viewport_width < 0.0 {
            return Err(LayoutConstraintError::InvalidViewportWidth);
        }
        if !line_height.is_finite() || line_height <= 0.0 {
            return Err(LayoutConstraintError::InvalidLineHeight);
        }
        Ok(Self {
            viewport_width,
            viewport_height: None,
            line_height,
            image_sizing_policy: ImageSizingPolicy::WebCompatible,
            text_composition_policy: TextCompositionPolicy::WebCompatible,
            parallel_workers: 1,
        })
    }

    pub fn viewport_width(self) -> f64 {
        self.viewport_width
    }

    pub fn with_viewport_height(
        mut self,
        viewport_height: Option<f64>,
    ) -> Result<Self, LayoutConstraintError> {
        if viewport_height.is_some_and(|height| !height.is_finite() || height < 0.0) {
            return Err(LayoutConstraintError::InvalidViewportHeight);
        }
        self.viewport_height = viewport_height;
        Ok(self)
    }

    pub fn viewport_height(self) -> Option<f64> {
        self.viewport_height
    }

    pub fn line_height(self) -> f64 {
        self.line_height
    }

    pub fn with_image_sizing_policy(mut self, policy: ImageSizingPolicy) -> Self {
        self.image_sizing_policy = policy;
        self
    }

    pub fn image_sizing_policy(self) -> ImageSizingPolicy {
        self.image_sizing_policy
    }

    pub fn with_text_composition_policy(mut self, policy: TextCompositionPolicy) -> Self {
        self.text_composition_policy = policy;
        self
    }

    pub fn text_composition_policy(self) -> TextCompositionPolicy {
        self.text_composition_policy
    }

    /// Enables worker-owned parallel subtree measurement. A value of one
    /// preserves deterministic sequential execution; larger values cap the
    /// dedicated layout pool used by dependency-independent work.
    pub fn with_parallel_workers(mut self, workers: usize) -> Self {
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        self.parallel_workers = workers.clamp(1, available);
        self
    }

    pub fn parallel_workers(self) -> usize {
        self.parallel_workers
    }
}

impl fmt::Display for LayoutConstraintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidViewportWidth => {
                formatter.write_str("viewport width must be finite and non-negative")
            }
            Self::InvalidViewportHeight => {
                formatter.write_str("viewport height must be finite and non-negative")
            }
            Self::InvalidLineHeight => {
                formatter.write_str("line height must be finite and positive")
            }
        }
    }
}

impl std::error::Error for LayoutConstraintError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrepareError(ComputedStylesValidationError);

impl From<ComputedStylesValidationError> for PrepareError {
    fn from(error: ComputedStylesValidationError) -> Self {
        Self(error)
    }
}

impl PrepareError {
    pub fn reason(self) -> ComputedStylesValidationError {
        self.0
    }
}

impl fmt::Display for PrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "cannot prepare layout input: {}", self.0)
    }
}

impl std::error::Error for PrepareError {}
