//! EPUB Canonical Fragment Identifier (CFI) support
//!
//! Format: epubcfi(/6/4[id]!/4/2/1:25)
//! - /6    = <spine> element in the OPF package document
//! - /4    = second <itemref> in the spine (even step = element index)
//! - !     = indirection into the referenced content document
//! - /4/2  = path through DOM element children (even steps only)
//! - /1:25 = first text node child (odd step), character offset 25

use super::{Document, DomNodeId};

/// Parse just the spine doc index from a CFI without needing the content document.
pub fn parse_cfi_spine_only(cfi: &str) -> Option<usize> {
    let inner = cfi.strip_prefix("epubcfi(")?.strip_suffix(')')?;
    let (spine_part, _) = inner.split_once('!')?;
    parse_spine_position(spine_part)
}

/// Parse /6/N into a zero-based doc index.
fn parse_spine_position(spine_part: &str) -> Option<usize> {
    // Expected: /6/N (spine element step then itemref step)
    let rest = spine_part.strip_prefix("/6/")?;
    // Strip any ID assertion on the itemref
    let step_str = rest.split('[').next()?;
    let step: usize = step_str.parse().ok()?;
    if !step.is_multiple_of(2) || step < 2 {
        return None;
    }
    Some((step / 2) - 1)
}

impl Document {
    /// Find which child index this node is in its parent's children list
    pub fn get_child_index(&self, node_id: DomNodeId) -> Option<usize> {
        let parent = self.element_ref(self.get_dom_parent(node_id)?)?;
        parent.children().position(|child| child == node_id)
    }

    /// Get the CFI step number for this node (counting only element siblings before it)
    /// CFI uses step indirection: step = (element_index + 1) * 2
    pub fn get_element_step(&self, node_id: DomNodeId) -> Option<usize> {
        let parent = self.element_ref(self.get_dom_parent(node_id)?)?;

        let mut element_count = 0;
        for child_idx in parent.children() {
            if child_idx == node_id {
                // CFI step indirection: (element_index + 1) * 2
                return Some((element_count + 1) * 2);
            }
            // Only count element nodes, not text nodes
            if self.is_element_node(child_idx) {
                element_count += 1;
            }
        }
        None
    }

    /// Returns the CFI odd step for a text node within its parent element.
    /// Step = 2 * (number of preceding element siblings) + 1.
    pub fn get_text_node_step(&self, text_node_id: DomNodeId) -> Option<usize> {
        let parent = self.text_ref(text_node_id)?.parent()?;
        self.text_children_with_steps(parent)
            .find(|(text, _)| text.node_id() == text_node_id)
            .map(|(_, step)| step)
    }

    /// Finds the text node at the given CFI odd step within a parent element.
    pub fn find_text_node_by_step(&self, parent_idx: DomNodeId, text_step: usize) -> Option<DomNodeId> {
        if text_step == 0 || text_step.is_multiple_of(2) {
            return None;
        }
        self.text_children_with_steps(parent_idx)
            .find(|(_, step)| *step == text_step)
            .map(|(text, _)| text.node_id())
    }

    /// Returns every adjacent character-data node represented by one odd CFI
    /// step. EPUB CFI deliberately treats adjacent DOM text nodes as a single
    /// logical chunk; browser `Range` offsets therefore span all of them.
    pub fn find_text_nodes_by_step(&self, parent_idx: DomNodeId, text_step: usize) -> Vec<DomNodeId> {
        if text_step == 0 || text_step.is_multiple_of(2) {
            return Vec::new();
        }
        self.text_children_with_steps(parent_idx)
            .filter(|(_, step)| *step == text_step)
            .map(|(text, _)| text.node_id())
            .collect()
    }

    /// Get all DOM nodes in the path from root to the given node
    /// Returns nodes in order from root to target
    pub fn get_dom_path_to_node(&self, node_idx: DomNodeId) -> Vec<DomNodeId> {
        let mut path = Vec::new();
        let mut current = Some(node_idx);

        // Build path from node to root
        while let Some(idx) = current {
            path.push(idx);
            current = self.dom_nodes.get(idx.index()).and_then(|n| n.parent());
        }

        // Reverse to get root-to-node order
        path.reverse();
        path
    }

    /// Find a DOM node given a CFI path of steps
    /// Steps are in CFI format (step indirection: step/2 - 1 = element_index)
    pub fn find_dom_node_by_steps(&self, steps: &[usize]) -> Option<DomNodeId> {
        let mut current = self.dom_root?;

        for &step in steps {
            if step == 0 || !step.is_multiple_of(2) {
                return None;
            }
            current = self
                .element_ref(current)?
                .child_elements()
                .nth(step / 2 - 1)?
                .node_id();
        }

        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spine_position() {
        assert_eq!(parse_spine_position("/6/2"), Some(0));
        assert_eq!(parse_spine_position("/6/4"), Some(1));
        assert_eq!(parse_spine_position("/6/10"), Some(4));
        assert_eq!(parse_spine_position("/6/2[chap01]"), Some(0));
        assert_eq!(parse_spine_position("/6/1"), None); // odd
        assert_eq!(parse_spine_position("/6/0"), None); // too small
        assert_eq!(parse_spine_position("/2"), None); // missing /6/ prefix
    }
}
