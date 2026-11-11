/// Reset groups are owned by the current cascade. Inherited font/text groups
/// remain store handles until a property actually writes to them.
pub(super) struct WorkingStyle {
    font: ComputedStyleGroup<Font>,
    text: ComputedStyleGroup<InheritedText>,
    pub(super) box_model: BoxModel,
    pub(super) border: Border,
    pub(super) radii: BorderRadii,
    pub(super) background: Background,
    pub(super) layout: LayoutStyle,
    pub(super) line_height_spec: Option<Arc<PreparedLineHeight>>,
    pub(super) generated_content: Option<GeneratedContent>,
    pub(super) counters: CounterDirectives,
}

/// Read-only projection; borrowing a stored parent never clones its groups.
pub(super) struct StyleRef<'a> {
    pub(super) font: &'a Font,
    pub(super) text: &'a InheritedText,
    pub(super) box_model: &'a BoxModel,
    pub(super) border: &'a Border,
    pub(super) radii: &'a BorderRadii,
    pub(super) background: &'a Background,
    pub(super) layout: &'a LayoutStyle,
    pub(super) line_height_spec: &'a Option<Arc<PreparedLineHeight>>,
    pub(super) generated_content: &'a Option<GeneratedContent>,
    pub(super) counters: &'a CounterDirectives,
}

/// Resolve the handle only for the duration of a read, allowing the builder
/// to keep interning values between parent accesses.
#[derive(Clone, Copy)]
pub(super) struct ParentStyle {
    indices: Option<StyleIndices>,
    root_font_size: f32,
}

impl ParentStyle {
    pub(super) fn new(indices: Option<StyleIndices>, root_font_size: f32) -> Self {
        Self {
            indices,
            root_font_size,
        }
    }

    pub(super) fn with<R>(
        &self,
        styles: &ComputedStylesBuilder,
        read: impl FnOnce(StyleRef<'_>) -> R,
    ) -> R {
        let Some(indices) = self.indices else {
            return read(WorkingStyle::initial(self.root_font_size).view(styles));
        };
        read(StyleRef {
            font: styles
                .font_style(indices)
                .expect("builder-issued style handle"),
            text: styles
                .text_style(indices)
                .expect("builder-issued style handle"),
            box_model: styles
                .box_model_style(indices)
                .expect("builder-issued style handle"),
            border: styles
                .border_style(indices)
                .expect("builder-issued style handle"),
            radii: styles
                .border_radii_style(indices)
                .expect("builder-issued style handle"),
            background: styles
                .background_style(indices)
                .expect("builder-issued style handle"),
            layout: styles
                .layout_style(indices)
                .expect("builder-issued style handle"),
            line_height_spec: &None,
            generated_content: &None,
            counters: &CounterDirectives::default(),
        })
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
            font: ComputedStyleGroup::Owned(Font::default()),
            text: ComputedStyleGroup::Owned(InheritedText::default()),
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
    pub(super) fn view<'a>(&'a self, styles: &'a ComputedStylesBuilder) -> StyleRef<'a> {
        StyleRef {
            font: self.font(styles),
            text: self.text(styles),
            box_model: &self.box_model,
            border: &self.border,
            radii: &self.radii,
            background: &self.background,
            layout: &self.layout,
            line_height_spec: &self.line_height_spec,
            generated_content: &self.generated_content,
            counters: &self.counters,
        }
    }

    pub(super) fn initial(root_font_size: f32) -> Self {
        let mut style = Self::default();
        if let ComputedStyleGroup::Owned(font) = &mut style.font {
            font.font_size = root_font_size;
        }
        style
    }

    /// Inherit only the groups and box properties that CSS inherits by default.
    pub(super) fn inherited(parent: &ParentStyle, styles: &ComputedStylesBuilder) -> Self {
        let mut style = Self::default();
        if let Some(indices) = parent.indices {
            style.font = ComputedStyleGroup::Inherited(indices);
            style.text = ComputedStyleGroup::Inherited(indices);
        } else {
            style = Self::initial(parent.root_font_size);
        }
        parent.with(styles, |parent| {
            inherit_box_model_properties(&mut style.box_model, parent.box_model)
        });
        style
    }

    pub(super) fn font<'a>(&'a self, styles: &'a ComputedStylesBuilder) -> &'a Font {
        match &self.font {
            ComputedStyleGroup::Inherited(indices) => {
                styles.font_style(*indices).expect("inherited font")
            }
            ComputedStyleGroup::Owned(value) => value,
        }
    }

    pub(super) fn text<'a>(&'a self, styles: &'a ComputedStylesBuilder) -> &'a InheritedText {
        match &self.text {
            ComputedStyleGroup::Inherited(indices) => {
                styles.text_style(*indices).expect("inherited text")
            }
            ComputedStyleGroup::Owned(value) => value,
        }
    }

    pub(super) fn font_group(&self) -> ComputedStyleGroup<Font> {
        self.font.clone()
    }

    pub(super) fn font_mut(&mut self, styles: &ComputedStylesBuilder) -> &mut Font {
        if let ComputedStyleGroup::Inherited(indices) = self.font {
            self.font = ComputedStyleGroup::Owned(
                styles.font_style(indices).expect("inherited font").clone(),
            );
        }
        match &mut self.font {
            ComputedStyleGroup::Owned(value) => value,
            _ => unreachable!(),
        }
    }

    pub(super) fn text_mut(&mut self, styles: &ComputedStylesBuilder) -> &mut InheritedText {
        if let ComputedStyleGroup::Inherited(indices) = self.text {
            self.text = ComputedStyleGroup::Owned(
                styles.text_style(indices).expect("inherited text").clone(),
            );
        }
        match &mut self.text {
            ComputedStyleGroup::Owned(value) => value,
            _ => unreachable!(),
        }
    }

    /// Move the resolved sub-styles into the document's deduped style store.
    pub(super) fn intern(
        mut self,
        styles: &mut ComputedStylesBuilder,
    ) -> Result<StyleIndices, ComputedStyleValueError> {
        self.layout.static_position_inline = self.layout.position == PositionMode::Absolute
            && (self.layout.static_position_inline
                || matches!(
                    self.box_model.display,
                    Display::Inline
                        | Display::InlineBlock
                        | Display::InlineTable
                        | Display::InlineFlex
                        | Display::InlineGrid
                ));
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
        styles.push_with_inherited_groups(
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
use super::prepared_native::typography::PreparedLineHeight;
use html_style_model::{
    Background, Border, BorderRadii, BoxModel, ComputedStyleGroup, ComputedStyleValueError,
    ComputedStylesBuilder, CounterDirectives, Display, Float, Font, FontRelativeLength,
    GeneratedContent, InheritedText, LayoutStyle, PositionMode, StyleIndices,
};

pub(super) fn inherit_box_model_properties(style: &mut BoxModel, parent: &BoxModel) {
    style.border_collapse = parent.border_collapse;
    style.border_spacing_horizontal = parent.border_spacing_horizontal;
    style.border_spacing_vertical = parent.border_spacing_vertical;
    style.caption_side = parent.caption_side;
    style.empty_cells = parent.empty_cells;
}

use std::sync::Arc;
