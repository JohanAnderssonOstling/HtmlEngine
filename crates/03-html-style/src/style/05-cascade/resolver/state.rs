/// Mutable accumulator the cascade writes into: the same split sub-structs the
/// `StyleStore` ultimately stores, bundled so resolution can mutate one value.
/// Inherited groups (`font`, `text`) are cloned from the parent; reset groups
/// start from their defaults.
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

impl Clone for WorkingStyle {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            font: self.font.clone(),
            text: self.text.clone(),
            box_model: self.box_model.clone(),
            border: self.border.clone(),
            radii: self.radii,
            background: self.background.clone(),
            layout: self.layout.clone(),
            line_height_spec: self.line_height_spec.clone(),
            generated_content: self.generated_content.clone(),
            counters: self.counters.clone(),
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
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
                Display::TableRowGroup
                | Display::TableHeaderGroup
                | Display::TableFooterGroup
                | Display::TableRow
                | Display::TableColumnGroup
                | Display::TableColumn
                | Display::TableCell
                | Display::TableCaption => Display::Block,
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
use super::*;
