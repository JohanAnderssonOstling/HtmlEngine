pub(crate) struct LayoutConfig {
    viewport_width: f64,
    viewport_height: Option<f64>,
    line_height: f64,
    image_sizing_policy: crate::ImageSizingPolicy,
    text_composition_policy: crate::TextCompositionPolicy,
    force_justify: bool,
}

impl LayoutConfig {
    pub(crate) fn new(constraints: crate::LayoutConstraints) -> Self {
        Self {
            viewport_width: constraints.viewport_width(),
            viewport_height: constraints.viewport_height(),
            line_height: constraints.line_height(),
            image_sizing_policy: constraints.image_sizing_policy(),
            text_composition_policy: constraints.text_composition_policy(),
            force_justify: false,
        }
    }

    pub(crate) fn viewport_width(&self) -> f64 {
        self.viewport_width
    }
    pub(crate) fn viewport_height(&self) -> Option<f64> {
        self.viewport_height
    }
    pub(crate) fn default_line_height(&self) -> f64 {
        self.line_height
    }
    pub(crate) fn force_justify(&self) -> bool {
        self.force_justify
    }
    pub(crate) fn book_optimized_text(&self) -> bool {
        self.text_composition_policy.is_book_optimized()
    }
    pub(crate) fn hyphenation_quality(&self) -> bool {
        self.text_composition_policy.uses_hyphenation_quality()
    }
    pub(crate) fn sentence_per_line(&self) -> bool {
        self.text_composition_policy.uses_sentence_per_line()
    }
    pub(crate) fn image_sizing_policy(&self) -> crate::ImageSizingPolicy {
        self.image_sizing_policy
    }
}
