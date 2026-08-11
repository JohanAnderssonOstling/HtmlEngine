/// The parent element's interned computed style, used to resolve the CSS-wide
/// keywords `inherit` and `unset` (and `initial` via [`ParentStyle::initial`]).
pub(super) struct ParentStyle {
    pub(super) font: Font,
    pub(super) text: InheritedText,
    pub(super) box_model: BoxModel,
    pub(super) border: Border,
    pub(super) radii: BorderRadii,
    pub(super) background: Background,
    pub(super) layout: LayoutStyle,
}

impl ParentStyle {
    pub(super) fn from_working(style: &WorkingStyle) -> Self {
        Self {
            font: style.font.clone(),
            text: style.text.clone(),
            box_model: style.box_model.clone(),
            border: style.border.clone(),
            radii: style.radii,
            background: style.background.clone(),
            layout: style.layout.clone(),
        }
    }

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

    /// The initial value of every property (`font-size: medium` resolves to
    /// the document's root font size).
    pub(super) fn initial(root_font_size: f32) -> Self {
        let mut font = Font::default();
        font.font_size = root_font_size;
        Self {
            font,
            text: InheritedText::default(),
            box_model: initial_box_model(),
            border: initial_border(),
            radii: BorderRadii::default(),
            background: Background::default(),
            layout: LayoutStyle::default(),
        }
    }

    pub(super) fn to_working_style(&self) -> WorkingStyle {
        WorkingStyle {
            font: self.font.clone(),
            text: self.text.clone(),
            box_model: self.box_model.clone(),
            border: self.border.clone(),
            radii: self.radii,
            background: self.background.clone(),
            layout: self.layout.clone(),
            line_height_spec: None,
            generated_content: None,
            counters: CounterDirectives::default(),
        }
    }

    pub(super) fn unset_working_style(&self, root_font_size: f32) -> WorkingStyle {
        let mut style = Self::initial(root_font_size).to_working_style();
        style.font = self.font.clone();
        style.text = self.text.clone();
        inherit_box_model_properties(&mut style.box_model, &self.box_model);
        style
    }
}
use super::*;
