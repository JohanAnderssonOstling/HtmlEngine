use html_style_model::UsedStyleView;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UsedBorderInsets {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl UsedBorderInsets {
    pub(crate) fn horizontal(self) -> f64 {
        self.left + self.right
    }

    pub(crate) fn vertical(self) -> f64 {
        self.top + self.bottom
    }
}

/// Physical used box-model values resolved against one containing-block width.
///
/// Keeping these values together prevents each formatting algorithm from
/// independently rebuilding padding, border, and margin sums. Callers still
/// decide whether a CSS rule suppresses a value (for example table-cell
/// margins) and which percentage basis applies.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResolvedBoxModel {
    pub margin_left: f64,
    pub margin_right: f64,
    pub margin_top: f64,
    pub margin_bottom: f64,
    pub padding_left: f64,
    pub padding_right: f64,
    pub padding_top: f64,
    pub padding_bottom: f64,
    pub border_left: f64,
    pub border_right: f64,
    pub border_top: f64,
    pub border_bottom: f64,
}

impl ResolvedBoxModel {
    pub(crate) fn new(style: UsedStyleView<'_>, containing_width: f64) -> Self {
        Self {
            margin_left: style.margin_left().resolve(containing_width),
            margin_right: style.margin_right().resolve(containing_width),
            margin_top: style.margin_top().resolve(containing_width),
            margin_bottom: style.margin_bottom().resolve(containing_width),
            padding_left: style.padding_left().resolve(containing_width),
            padding_right: style.padding_right().resolve(containing_width),
            padding_top: style.padding_top().resolve(containing_width),
            padding_bottom: style.padding_bottom().resolve(containing_width),
            border_left: style.border_left_width() as f64,
            border_right: style.border_right_width() as f64,
            border_top: style.border_top_width() as f64,
            border_bottom: style.border_bottom_width() as f64,
        }
    }

    pub(crate) fn with_used_borders(mut self, borders: UsedBorderInsets) -> Self {
        self.border_top = borders.top;
        self.border_right = borders.right;
        self.border_bottom = borders.bottom;
        self.border_left = borders.left;
        self
    }

    pub(crate) fn without_padding(mut self) -> Self {
        self.padding_top = 0.0;
        self.padding_right = 0.0;
        self.padding_bottom = 0.0;
        self.padding_left = 0.0;
        self
    }

    pub(crate) fn horizontal_margin(self) -> f64 {
        self.margin_left + self.margin_right
    }

    pub(super) fn vertical_margin(self) -> f64 {
        self.margin_top + self.margin_bottom
    }

    pub(super) fn horizontal_padding(self) -> f64 {
        self.padding_left + self.padding_right
    }

    pub(super) fn vertical_padding(self) -> f64 {
        self.padding_top + self.padding_bottom
    }

    pub(super) fn horizontal_border(self) -> f64 {
        self.border_left + self.border_right
    }

    pub(super) fn vertical_border(self) -> f64 {
        self.border_top + self.border_bottom
    }

    pub(crate) fn horizontal_padding_border(self) -> f64 {
        self.horizontal_padding() + self.horizontal_border()
    }

    pub(crate) fn vertical_padding_border(self) -> f64 {
        self.vertical_padding() + self.vertical_border()
    }

    pub(crate) fn horizontal_noncontent(self) -> f64 {
        self.horizontal_margin() + self.horizontal_padding_border()
    }
}
