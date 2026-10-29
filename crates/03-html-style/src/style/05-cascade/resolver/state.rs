/// Owned computed groups shared by parent snapshots and cascade accumulators.
/// Inherited groups (`font`, `text`) are cloned from the parent; reset groups
/// start from their defaults. Declaration-local state is empty when loading
/// a parent from the style store.
pub(super) struct WorkingStyle {
    pub(super) font: Font,
    pub(super) text: InheritedText,
    pub(super) box_model: BoxModel,
    pub(super) border: Border,
    pub(super) radii: BorderRadii,
    pub(super) background: Background,
    pub(super) layout: LayoutStyle,
    pub(super) line_height_spec: Option<CssLineHeight>,
    pub(super) generated_content: Option<GeneratedContent>,
    pub(super) counters: CounterDirectives,
}

pub(super) fn initial_box_model() -> BoxModel {
    let mut box_model = BoxModel::default();
    box_model.display = Display::Inline;
    box_model
}

pub(super) fn initial_border() -> Border {
    let mut border = Border::default();
    let medium = FontRelativeLength::px(3.0).expect("CSS medium border width is finite");
    border.border_top_width = medium;
    border.border_right_width = medium;
    border.border_bottom_width = medium;
    border.border_left_width = medium;
    border.current_color_sides = 0b1111;
    border
}

impl Default for WorkingStyle {
    fn default() -> Self {
        Self {
            font: Font::default(),
            text: InheritedText::default(),
            box_model: initial_box_model(),
            border: initial_border(),
            radii: BorderRadii::default(),
            background: Background::default(),
            layout: LayoutStyle::default(),
            line_height_spec: None,
            generated_content: None,
            counters: CounterDirectives::default(),
        }
    }
}

impl WorkingStyle {
    #[inline]
    pub(super) fn clone_from(&mut self, source: &Self) {
        self.font.clone_from(&source.font);
        self.text.clone_from(&source.text);
        self.box_model.clone_from(&source.box_model);
        self.border.clone_from(&source.border);
        self.radii = source.radii;
        self.background.clone_from(&source.background);
        self.layout.clone_from(&source.layout);
        self.line_height_spec.clone_from(&source.line_height_spec);
        self.generated_content.clone_from(&source.generated_content);
        self.counters.clone_from(&source.counters);
    }

    /// Load computed values without inheriting authored line-height expressions
    /// or per-element generated-content and counter state.
    pub(super) fn from_indices(styles: &ComputedStylesBuilder, indices: StyleIndices) -> Self {
        Self {
            font: styles
                .font_style(indices)
                .expect("builder-issued style handle")
                .clone(),
            text: styles
                .text_style(indices)
                .expect("builder-issued style handle")
                .clone(),
            box_model: styles
                .box_model_style(indices)
                .expect("builder-issued style handle")
                .clone(),
            border: styles
                .border_style(indices)
                .expect("builder-issued style handle")
                .clone(),
            radii: *styles
                .border_radii_style(indices)
                .expect("builder-issued style handle"),
            background: styles
                .background_style(indices)
                .expect("builder-issued style handle")
                .clone(),
            layout: styles
                .layout_style(indices)
                .expect("builder-issued style handle")
                .clone(),
            ..Self::default()
        }
    }

    pub(super) fn for_node(
        doc: &Document,
        styles: &ComputedStylesBuilder,
        node_idx: DomNodeId,
    ) -> Self {
        if let Some(parent_idx) = doc.get_dom_parent(node_idx)
            && let Some(indices) = styles.style_for_node(parent_idx)
        {
            return Self::from_indices(styles, indices);
        }
        Self::initial(doc.root_font_size())
    }

    pub(super) fn initial(root_font_size: f32) -> Self {
        let mut style = Self::default();
        style.font.font_size = root_font_size;
        style
    }

    /// Inherit only the groups and box properties that CSS inherits by default.
    pub(super) fn inherited(font: &Font, text: &InheritedText, parent_box: &BoxModel) -> Self {
        let mut style = Self {
            font: font.clone(),
            text: text.clone(),
            ..Self::default()
        };
        inherit_box_model_properties(&mut style.box_model, parent_box);
        style
    }

    /// Move the resolved sub-styles into the document's deduped style store.
    pub(super) fn intern(
        mut self,
        styles: &mut ComputedStylesBuilder,
    ) -> Result<StyleIndices, ComputedStyleValueError> {
        // Absolute positioning and floating both blockify the principal box
        // at the computed-style boundary (CSS 2.1 section 9.7). The box
        // builder still recognizes a floated descendant from its `float`
        // value and leaves an anchor when it occurs inside inline content.
        if self.layout.position == PositionMode::Absolute
            || matches!(self.box_model.float, Float::Left | Float::Right)
        {
            self.box_model.display = match self.box_model.display {
                Display::Inline | Display::InlineBlock => Display::Block,
                Display::InlineTable => Display::Table,
                Display::InlineFlex => Display::Flex,
                Display::InlineGrid => Display::Grid,
                display if display.is_table_internal() => Display::Block,
                display => display,
            };
        }
        self.box_model.normalize_overflow_axes();
        styles.push_with_layout_and_radii(
            self.font,
            self.text,
            self.box_model,
            self.border,
            self.background,
            self.layout,
            self.radii,
        )
    }
}
use html_dom::{Document, DomNodeId};
use html_style_model::{
    Background, Border, BorderRadii, BoxModel, ComputedStyleValueError, ComputedStylesBuilder,
    CounterDirectives, Display, Float, Font, FontRelativeLength, GeneratedContent, InheritedText,
    LayoutStyle, PositionMode, StyleIndices,
};
use lightningcss::properties::font::LineHeight as CssLineHeight;

pub(super) fn inherit_box_model_properties(style: &mut BoxModel, parent: &BoxModel) {
    style.border_collapse = parent.border_collapse;
    style.border_spacing_horizontal = parent.border_spacing_horizontal;
    style.border_spacing_vertical = parent.border_spacing_vertical;
    style.caption_side = parent.caption_side;
    style.empty_cells = parent.empty_cells;
}
