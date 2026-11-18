use crate::layout_model::{Children, LayoutMode, TableCellBox, TableRowBox};
use html_dom::{Document, DomNodeId, ElementRef};
use html_style_model::{Clear, ComputedStyles, Display, Float, PositionMode, StyleIndices};
use std::ops::Range;

#[derive(Clone, Copy)]
pub(super) struct BoxClassification {
    generated: bool,
    pub(super) style: Option<StyleIndices>,
    pub(super) display: Display,
    pub(super) floated: bool,
    pub(super) absolute: bool,
    pub(super) clearing_break: bool,
    pub(super) has_block_child: bool,
    pub(super) splits_inline_descendant: bool,
    pub(super) table_internal_descendant: bool,
}

impl BoxClassification {
    pub(super) fn new(styles: &ComputedStyles, element: ElementRef<'_>) -> Self {
        let style = styles.style_for_node(element.node_id());
        let mut result = Self::for_style(styles, style, false);
        result.display = resolved_display(element, result.display);
        result.clearing_break = element.tag().eq_ignore_ascii_case("br")
            && style
                .and_then(|style| styles.box_model_style(style))
                .is_some_and(|style| style.clear != Clear::None);
        result
    }

    fn for_style(styles: &ComputedStyles, style: Option<StyleIndices>, generated: bool) -> Self {
        let box_style = style.and_then(|style| styles.box_model_style(style));
        Self {
            generated,
            style,
            display: box_style.map_or(Display::Block, |style| style.display),
            floated: box_style.is_some_and(|style| style.float != Float::None),
            absolute: style
                .and_then(|style| styles.layout_style(style))
                .is_some_and(|style| style.position == PositionMode::Absolute),
            clearing_break: false,
            has_block_child: false,
            splits_inline_descendant: false,
            table_internal_descendant: false,
        }
    }

    pub(super) fn pseudo(styles: &ComputedStyles, origin: DomNodeId, before: bool) -> Option<Self> {
        styles
            .generated_pseudo_for_node(origin, before)
            .map(|(style, _, _)| Self::for_style(styles, Some(style), true))
    }

    pub(super) fn starts_block(self) -> bool {
        self.display != Display::Contents
            && (starts_new_block_child(self.display) || self.absolute || self.clearing_break)
    }

    pub(super) fn splits_inline(self) -> bool {
        starts_new_block_child(self.display)
            && if self.generated {
                // Generated flex/grid and column roles retain their existing
                // construction policy; table rows/cells can split an inline.
                !matches!(
                    self.display,
                    Display::Flex
                        | Display::Grid
                        | Display::TableColumn
                        | Display::TableColumnGroup
                )
            } else {
                !self.display.is_table_internal()
            }
            && !self.absolute
            && !self.floated
    }
}

/// Build-local facts indexed by element ID; text nodes need no classification.
pub(super) struct BoxClassifications {
    values: Vec<Option<BoxClassification>>,
}

impl BoxClassifications {
    pub(super) fn new(document: &Document, styles: &ComputedStyles) -> Self {
        let mut result = Self {
            values: vec![None; document.element_count()],
        };
        // Explicit postorder follows child lists: numeric IDs are insertion
        // order, and do not guarantee that descendants follow their parents.
        let mut pending = Vec::new();
        if let Some(root) = document.dom_root() {
            pending.push((root, false));
        }
        while let Some((node, visited)) = pending.pop() {
            let element = document.element_ref(node);
            if !visited {
                pending.push((node, true));
                pending.extend(
                    element
                        .child_elements()
                        .map(|child| (child.node_id(), false)),
                );
                continue;
            }
            let mut classification = BoxClassification::new(styles, element);
            let inline_container =
                matches!(classification.display, Display::Inline | Display::Contents);
            let block_pseudo = BoxClassification::pseudo(styles, node, true)
                .is_some_and(BoxClassification::splits_inline)
                || BoxClassification::pseudo(styles, node, false)
                    .is_some_and(BoxClassification::splits_inline);
            classification.has_block_child = block_pseudo;
            classification.splits_inline_descendant = block_pseudo;
            for child in element.child_elements() {
                let child = result.get(child);
                classification.table_internal_descendant |= child.display.is_table_internal()
                    || (child.display == Display::Contents && child.table_internal_descendant);
                let nested_split = matches!(child.display, Display::Inline | Display::Contents)
                    && child.splits_inline_descendant;
                classification.splits_inline_descendant |= child.splits_inline() || nested_split;
                classification.has_block_child |= (!child.absolute
                    && !child.floated
                    && if inline_container {
                        child.splits_inline()
                    } else {
                        child.starts_block()
                    })
                    || (child.display == Display::Contents && child.table_internal_descendant)
                    || nested_split;
            }
            // Replaced content is atomic even when its resource has DOM children.
            if element.image_idx().is_some() {
                classification.has_block_child = false;
                classification.splits_inline_descendant = false;
            }
            result.values[node.index()] = Some(classification);
        }
        result
    }

    pub(super) fn get(&self, element: ElementRef<'_>) -> BoxClassification {
        self.values[element.node_id().index()].expect("classified document element")
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
        | Display::TableCaption => LayoutMode::Block(Children::default()),
        Display::Inline => LayoutMode::Inline(Range::default()),
        Display::Flex | Display::InlineFlex => LayoutMode::Flex(0..0),
        Display::Grid | Display::InlineGrid => LayoutMode::Grid(0..0),
        Display::Table | Display::InlineTable => LayoutMode::Table(Default::default()),
        Display::TableRow => LayoutMode::TableRow(TableRowBox::default()),
        Display::TableCell => LayoutMode::TableCell(TableCellBox::new(Children::Empty)),
        Display::Contents | Display::None => return None,
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

pub(super) fn pseudo_display(
    styles: &ComputedStyles,
    origin: DomNodeId,
    before: bool,
) -> Option<Display> {
    styles
        .generated_pseudo_for_node(origin, before)
        .map(|(style, _, _)| display_for_style(styles, style))
}
