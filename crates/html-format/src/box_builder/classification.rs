use crate::layout_model::{
    BlockBox, Children, FlexBox, GridBox, LayoutMode, TableBox, TableCellBox, TableRowBox,
};
use html_dom::{DomNodeId, ElementRef};
use html_style_model::{Clear, ComputedStyles, Display, Float, PositionMode, StyleIndices};
use std::ops::Range;

pub(super) fn display_for_element(styles: &ComputedStyles, element: ElementRef<'_>) -> Display {
    let display = styles
        .style_for_node(element.node_id())
        .map_or(Display::Block, |style| display_for_style(styles, style));
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
        Display::Table | Display::InlineTable => LayoutMode::Table(TableBox::default()),
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

/// Out-of-flow boxes are blockified for layout even when their authored
/// display is `inline`. Parent formatting-context construction must make the
/// same decision as the box itself, otherwise an absolutely positioned span
/// is incorrectly flattened into the surrounding inline-item stream.
pub(super) fn starts_new_block_child_for_element(
    styles: &ComputedStyles,
    element: ElementRef<'_>,
) -> bool {
    let display = display_for_element(styles, element);
    if display == Display::Contents {
        return false;
    }
    starts_new_block_child(display)
        || is_absolutely_positioned(styles, element)
        || is_clearing_break(styles, element)
}

pub(super) fn is_clearing_break(styles: &ComputedStyles, element: ElementRef<'_>) -> bool {
    element.tag().eq_ignore_ascii_case("br")
        && styles
            .style_for_node(element.node_id())
            .and_then(|style| styles.box_model_style(style))
            .is_some_and(|style| style.clear != Clear::None)
}

/// Table-internal boxes remain inline candidates until anonymous-table fixup
/// groups them. Ordinary block descendants and absolutely positioned boxes
/// instead split an inline formatting context.
pub(super) fn starts_inline_split_child(styles: &ComputedStyles, element: ElementRef<'_>) -> bool {
    let display = display_for_element(styles, element);
    starts_new_block_child(display)
        && !display.is_table_internal()
        && !is_absolutely_positioned(styles, element)
        && !is_floated(styles, element)
}

pub(super) fn is_absolutely_positioned(styles: &ComputedStyles, element: ElementRef<'_>) -> bool {
    styles
        .style_for_node(element.node_id())
        .and_then(|style| styles.layout_style(style))
        .is_some_and(|style| style.position == PositionMode::Absolute)
}

pub(super) fn is_floated(styles: &ComputedStyles, element: ElementRef<'_>) -> bool {
    styles
        .style_for_node(element.node_id())
        .and_then(|style| styles.box_model_style(style))
        .is_some_and(|style| matches!(style.float, Float::Left | Float::Right))
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
    styles.generated_pseudo_for_node(origin, before)
        .and_then(|(indices, _, _)| styles.view(indices))
        .is_some_and(|style| style.layout.position != PositionMode::Absolute
            && style.float() == Float::None
            && is_block_level_pseudo_display(style.display()))
}
