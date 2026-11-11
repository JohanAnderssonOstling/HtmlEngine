use html_engine_model::{ImageSizingPolicy, LayoutConstraints, TextCompositionPolicy};
use html_engine_model::constraints;
#[derive(Clone, Copy, PartialEq)]
pub struct LayoutConfig {
    viewport_width: f64,
    viewport_height: Option<f64>,
    image_sizing_policy: ImageSizingPolicy,
    text_composition_policy: TextCompositionPolicy,
}

impl LayoutConfig {
    pub fn new(constraints: LayoutConstraints) -> Self {
        Self {
            viewport_width: constraints.viewport_width(),
            viewport_height: constraints.viewport_height(),
            image_sizing_policy: constraints.image_sizing_policy(),
            text_composition_policy: constraints.text_composition_policy(),
        }
    }

    pub fn viewport_width(&self) -> f64 {
        self.viewport_width
    }
    pub fn viewport_height(&self) -> Option<f64> {
        self.viewport_height
    }

    pub fn book_optimized_text(&self) -> bool {
        self.text_composition_policy.is_book_optimized()
    }
    pub fn hyphenation_quality(&self) -> bool {
        constraints::text_composition_policy_uses_hyphenation_quality(self.text_composition_policy)
    }
    pub fn image_sizing_policy(&self) -> ImageSizingPolicy {
        self.image_sizing_policy
    }
}
