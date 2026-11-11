use crate::layout_model::{
    BlockBox, Children, FlexBox, GridBox, LayoutMode, TableCellBox, TableRowBox,
};
use html_dom::{DomNodeId, ElementRef};
use html_style_model::{Clear, ComputedStyles, Display, Float, PositionMode, StyleIndices};
use std::ops::Range;

#[derive(Clone, Copy)]
pub(super) struct ElementClassification {
    pub(super) style: Option<StyleIndices>,
    pub(super) display: Display,
    pub(super) floated: bool,
    pub(super) absolute: bool,
    pub(super) clearing_break: bool,
}

impl ElementClassification {
    pub(super) fn new(styles: &ComputedStyles, element: ElementRef<'_>) -> Self {
        let style = styles.style_for_node(element.node_id());
        let box_style = style.and_then(|style| styles.box_model_style(style));
        let display = box_style.map_or(Display::Block, |style| style.display);
        let display = resolved_display(element, display);
        Self {
            style,
            display,
            floated: box_style
                .is_some_and(|style| matches!(style.float, Float::Left | Float::Right)),
            absolute: style
                .and_then(|style| styles.layout_style(style))
                .is_some_and(|style| style.position == PositionMode::Absolute),
            clearing_break: element.tag().eq_ignore_ascii_case("br")
                && box_style.is_some_and(|style| style.clear != Clear::None),
        }
    }

    pub(super) fn starts_block(self) -> bool {
        self.display != Display::Contents
            && (starts_new_block_child(self.display) || self.absolute || self.clearing_break)
    }

    pub(super) fn splits_inline(self) -> bool {
        starts_new_block_child(self.display)
            && !self.display.is_table_internal()
            && !self.absolute
            && !self.floated
    }
}

pub(super) fn display_for_element(styles: &ComputedStyles, element: ElementRef<'_>) -> Display {
    let display = styles
        .style_for_node(element.node_id())
        .and_then(|style| styles.box_model_style(style))
        .map_or(Display::Block, |style| style.display);
    resolved_display(element, display)
}

fn resolved_display(element: ElementRef<'_>, display: Display) -> Display {
    if display == Display::Contents && contents_computes_to_none(element) {
        Display::None
    } else if element.image_idx().is_some() && display.is_table_internal() {
        Display::Inline
    } else {
        display
    }
}

fn contents_computes_to_none(element: ElementRef<'_>) -> bool {
    if element.image_idx().is_some() {
        return true;
    }
    element.is_html_element_in_html_document()
        && [
            "br", "wbr", "meter", "progress", "embed", "object", "audio", "img", "input",
            "textarea", "select",
        ]
        .iter()
        .any(|tag| element.tag().eq_ignore_ascii_case(tag))
}

pub(super) fn display_for_style(styles: &ComputedStyles, style: StyleIndices) -> Display {
    styles
        .box_model_style(style)
        .expect("validated style handle")
        .display
}

pub(super) fn layout_mode_for_display(display: Display) -> Option<LayoutMode> {
    Some(match display {
        Display::Block
        | Display::FlowRoot
        | Display::InlineBlock
        | Display::ListItem
        | Display::FlowRootListItem
        | Display::TableRowGroup
        | Display::TableHeaderGroup
        | Display::TableFooterGroup
        | Display::TableColumnGroup
        | Display::TableColumn
        | Display::TableCaption => LayoutMode::Block(BlockBox::default()),
        Display::Inline => LayoutMode::Inline(Range::default()),
        Display::Flex | Display::InlineFlex => LayoutMode::Flex(FlexBox::default()),
        Display::Grid | Display::InlineGrid => LayoutMode::Grid(GridBox::default()),
        Display::Table | Display::InlineTable => LayoutMode::Table(Default::default()),
        Display::TableRow => LayoutMode::TableRow(TableRowBox::default()),
        Display::TableCell => LayoutMode::TableCell(TableCellBox::new(Children::Empty)),
        Display::Contents | Display::None => return None,
    })
}

pub(super) fn has_table_internal_descendant(
    styles: &ComputedStyles,
    element: ElementRef<'_>,
) -> bool {
    element.child_elements().any(|child| {
        let display = display_for_element(styles, child);
        display.is_table_internal()
            || (display == Display::Contents && has_table_internal_descendant(styles, child))
    })
}

pub(super) fn starts_new_block_child(display: Display) -> bool {
    matches!(
        display,
        Display::Block
            | Display::FlowRoot
            | Display::Flex
            | Display::Grid
            | Display::ListItem
            | Display::FlowRootListItem
            | Display::Table
    ) || display.is_table_internal()
}

pub(super) fn is_block_level_pseudo_display(display: Display) -> bool {
    matches!(
        display,
        Display::Block
            | Display::FlowRoot
            | Display::ListItem
            | Display::FlowRootListItem
            | Display::Table
            | Display::TableRowGroup
            | Display::TableHeaderGroup
            | Display::TableFooterGroup
            | Display::TableRow
            | Display::TableCell
            | Display::TableCaption
    )
}

pub(super) fn pseudo_display(
    styles: &ComputedStyles,
    origin: DomNodeId,
    before: bool,
) -> Option<Display> {
    styles
        .generated_pseudo_for_node(origin, before)
        .map(|(style, _, _)| display_for_style(styles, style))
}

pub(super) fn pseudo_is_block_level(
    styles: &ComputedStyles,
    origin: DomNodeId,
    before: bool,
) -> bool {
    styles
        .generated_pseudo_for_node(origin, before)
        .and_then(|(indices, _, _)| styles.view(indices))
        .is_some_and(|style| {
            style.layout.position != PositionMode::Absolute
                && style.float() == Float::None
                && is_block_level_pseudo_display(style.display())
        })
}
