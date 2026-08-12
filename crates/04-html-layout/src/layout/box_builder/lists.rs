use html_dom::{Document, DomNodeId};
use html_style_model::{ComputedStyles, Display};
use rustc_data_structures::fx::FxHashMap;

use super::classification::display_for_element;

/// Ordinals for rendered list-item boxes, indexed by DOM node.
///
/// This table is deliberately temporary: it removes the previous repeated
/// sibling scans without adding retained data to `PreparedDocument`.
pub(super) struct ListItemOrdinals {
    values: Vec<i64>,
}

impl ListItemOrdinals {
    pub(super) fn new(document: &Document, styles: &ComputedStyles) -> Self {
        let mut groups = FxHashMap::<DomNodeId, Vec<DomNodeId>>::default();
        if let Some(root) = document.dom_root() {
            collect_rendered_list_items(document, styles, root, None, &mut groups);
        }

        let mut values = vec![1; document.node_count()];
        for (owner, items) in groups {
            assign_group_ordinals(document, owner, &items, &mut values);
        }
        Self { values }
    }

    pub(super) fn get(&self, item: DomNodeId) -> i64 {
        self.values.get(item.index()).copied().unwrap_or(1)
    }
}

fn collect_rendered_list_items(
    document: &Document,
    styles: &ComputedStyles,
    node: DomNodeId,
    nearest_list_owner: Option<DomNodeId>,
    groups: &mut FxHashMap<DomNodeId, Vec<DomNodeId>>,
) {
    let Some(element) = document.element_ref(node) else {
        return;
    };
    let display = display_for_element(styles, element);

    // A display:none subtree generates no list-item boxes and does not
    // participate in ordinal calculation.
    if display == Display::None {
        return;
    }

    if matches!(display, Display::ListItem | Display::FlowRootListItem)
        && let Some(owner) = nearest_list_owner.or_else(|| element.parent())
    {
        groups.entry(owner).or_default().push(node);
    }

    // An HTML list whose principal box is suppressed by `display: contents`
    // is skipped by the list-owner algorithm. Its CSS counter directives are
    // still handled by GeneratedContentResolver during box-tree traversal.
    let child_list_owner = if display != Display::Contents && resets_list_item_counter(styles, node)
    {
        Some(node)
    } else {
        nearest_list_owner
    };
    for child in element.children() {
        if document.element_ref(child).is_some() {
            collect_rendered_list_items(document, styles, child, child_list_owner, groups);
        }
    }
}

fn resets_list_item_counter(styles: &ComputedStyles, node: DomNodeId) -> bool {
    styles
        .counter_directives_for_node(node)
        .is_some_and(|directives| {
            directives
                .resets
                .iter()
                .any(|directive| styles.string(directive.name) == Some("list-item"))
        })
}

fn assign_group_ordinals(
    document: &Document,
    owner: DomNodeId,
    items: &[DomNodeId],
    values: &mut [i64],
) {
    let owner = document
        .element_ref(owner)
        .expect("list ordinal owner must be an element");
    let ordered = owner.namespace() == Some("http://www.w3.org/1999/xhtml")
        && owner.tag().eq_ignore_ascii_case("ol");
    let reversed = ordered && owner.html_list_reversed();
    let start = ordered.then(|| owner.html_list_start()).flatten();
    let mut current = start.unwrap_or_else(|| if reversed { items.len() as i64 } else { 1 });
    let step = if reversed { -1 } else { 1 };

    for &item in items {
        if ordered
            && let Some(value) = document
                .element_ref(item)
                .and_then(|element| element.html_list_item_value())
        {
            current = value;
        }
        if let Some(slot) = values.get_mut(item.index()) {
            *slot = current;
        }
        current = current.saturating_add(step);
    }
}

#[cfg(test)]
mod tests {
    use super::ListItemOrdinals;
    use crate::parser::DocumentFactory;
    use html_dom::{Document, DomNodeId};

    fn node_with_id(document: &Document, id: &str) -> DomNodeId {
        document
            .node_ids()
            .find(|&node| document.get_dom_id(node) == Some(id))
            .unwrap_or_else(|| panic!("missing element #{id}"))
    }

    fn ordinals(html: &str, css: Option<&str>, ids: &[&str]) -> Vec<i64> {
        let mut factory = DocumentFactory::new();
        let prepared = factory.parse_with_new_pipeline(html, css);
        let table = ListItemOrdinals::new(prepared.document(), prepared.styles());
        ids.iter()
            .map(|id| table.get(node_with_id(prepared.document(), id)))
            .collect()
    }

    #[test]
    fn nearest_html_list_ancestor_owns_indirect_list_items() {
        let html = "<html><body><ol start='4'><li id='a'>A</li><div><li id='b' value='9'>B</li><dir><li id='c'>C</li></dir></div><ol><li id='nested'>N</li></ol><li id='d'>D</li></ol></body></html>";
        assert_eq!(
            ordinals(
                html,
                Some("li { list-style-type: decimal }"),
                &["a", "b", "c", "nested", "d"]
            ),
            [4, 9, 10, 1, 11]
        );
    }

    #[test]
    fn reversed_lists_count_only_rendered_items_and_include_css_list_items() {
        let html = "<html><body><ol reversed><li id='a'>A</li><span id='b'>B</span><li id='hidden'>C</li><div id='c'>D</div></ol></body></html>";
        let css =
            "#b, #c { display: list-item; list-style-type: decimal } #hidden { display: none }";
        assert_eq!(ordinals(html, Some(css), &["a", "b", "c"]), [3, 2, 1]);
    }

    #[test]
    fn parent_owns_list_items_when_there_is_no_html_list_ancestor() {
        let html = "<html><body><li id='a'>A</li><li id='b'>B</li><div><li id='c'>C</li></div><li id='d'>D</li></body></html>";
        assert_eq!(
            ordinals(
                html,
                Some("li { list-style-type: decimal }"),
                &["a", "b", "c", "d"]
            ),
            [1, 2, 1, 3]
        );
    }

    #[test]
    fn authored_counter_reset_can_remove_an_html_lists_counter_scope() {
        let html = "<html><body><ol style='counter-reset: other'><li id='a'>A</li><li id='b'>B</li><div><li id='c'>C</li></div></ol></body></html>";
        assert_eq!(
            ordinals(
                html,
                Some("li { list-style-type: decimal }"),
                &["a", "b", "c"]
            ),
            [1, 2, 1]
        );
    }

    #[test]
    fn display_contents_html_list_is_not_a_list_owner() {
        let html = "<html><body><ol start='4'><ol style='display:contents'><li id='a'>A</li><li id='b'>B</li></ol><li id='c'>C</li></ol></body></html>";
        assert_eq!(
            ordinals(
                html,
                Some("li { list-style-type: decimal }"),
                &["a", "b", "c"]
            ),
            [4, 5, 6]
        );
    }
}
