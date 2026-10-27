//! Lazy DOM sibling positions for the element currently being resolved.

use crate::style::syntax::tree_counting::{self, TreeFunction};
use html_dom::{Document, DomNodeId};
use rustc_data_structures::fx::FxHashMap;
use std::borrow::Cow;

#[derive(Clone, Copy)]
struct Position {
    index: u32,
    count: u32,
}

#[derive(Default)]
pub(super) struct TreeCountingContext {
    node: Option<DomNodeId>,
    positions: FxHashMap<DomNodeId, Position>,
    used: bool,
}

impl TreeCountingContext {
    pub(super) fn begin_element(&mut self, node: DomNodeId) {
        self.node = Some(node);
        self.used = false;
    }

    pub(super) fn used(&self) -> bool {
        self.used
    }

    pub(super) fn lower<'a>(
        &mut self,
        doc: &Document,
        source: &'a str,
    ) -> Result<Cow<'a, str>, ()> {
        tree_counting::lower(source, |function| {
            self.used = true;
            let node = self
                .node
                .expect("tree counting requires an originating element");
            if !self.positions.contains_key(&node) {
                if let Some(parent) = doc
                    .get_dom_parent(node)
                    .and_then(|node| doc.element_ref(node))
                {
                    let count = parent
                        .children()
                        .filter(|node| doc.element_ref(*node).is_some())
                        .count() as u32;
                    for (index, sibling) in parent
                        .children()
                        .filter(|node| doc.element_ref(*node).is_some())
                        .enumerate()
                    {
                        self.positions.insert(
                            sibling,
                            Position {
                                index: index as u32 + 1,
                                count,
                            },
                        );
                    }
                } else {
                    self.positions.insert(node, Position { index: 1, count: 1 });
                }
            }
            let position = self.positions[&node];
            match function {
                TreeFunction::Index => position.index,
                TreeFunction::Count => position.count,
            }
        })
    }
}
