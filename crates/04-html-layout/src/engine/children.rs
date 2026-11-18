use super::LayoutEngine;
use html_engine_model::{Children, InlineItemKind, LayoutMode, TableCellBox};

impl LayoutEngine<'_, '_> {
    /// Enumerate layout participants; the caller selects which inline anchors count.
    /// Positioned row children are intentionally outside this flow traversal.
    pub(crate) fn child_boxes(
        &self,
        box_idx: usize,
        inline_child: fn(&InlineItemKind) -> Option<u32>,
    ) -> impl Iterator<Item = usize> + '_ {
        let mut groups: [&[u32]; 3] = [&[]; 3];
        let mut inline = 0..0;
        match self.read.reader.box_layout_mode(box_idx) {
            Some(LayoutMode::Block(children))
            | Some(LayoutMode::TableCell(TableCellBox { children, .. })) => match children {
                Children::Blocks(children) => {
                    groups[0] = self.read.reader.layout_tree().child_slice(children)
                }
                Children::InlineItems(range) => inline = range.clone(),
                Children::Empty => {}
            },
            Some(LayoutMode::Table(_)) => {
                let table = self.read.reader.box_table(box_idx).expect("table payload");
                groups = [&table.rows, &table.captions_top, &table.captions_bottom];
            }
            Some(LayoutMode::TableRow(row)) => groups[0] = &row.cells,
            Some(LayoutMode::Flex(container) | LayoutMode::Grid(container)) => {
                groups[0] = self.read.reader.layout_tree().child_slice(container);
            }
            Some(LayoutMode::Inline(range)) | Some(LayoutMode::Anonymous(range)) => {
                inline = range.clone();
            }
            None => {}
        }
        groups
            .into_iter()
            .flatten()
            .copied()
            .chain(inline.filter_map(move |index| {
                self.read
                    .text
                    .inline_item(index as usize)
                    .and_then(|item| inline_child(&item.kind))
            }))
            .map(|child| child as usize)
    }
}
