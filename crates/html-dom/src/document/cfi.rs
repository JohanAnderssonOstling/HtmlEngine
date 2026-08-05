//! EPUB Canonical Fragment Identifier (CFI) support
//!
//! Format: epubcfi(/6/4[id]!/4/2/1:25)
//! - /6    = <spine> element in the OPF package document
//! - /4    = second <itemref> in the spine (even step = element index)
//! - !     = indirection into the referenced content document
//! - /4/2  = path through DOM element children (even steps only)
//! - /1:25 = first text node child (odd step), character offset 25

use super::{Document, DomNode, DomNodeId};

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
        let parent_idx = self.dom_nodes.get(node_id.index())?.parent()?;
        let parent = self.dom_nodes.get(parent_idx.index())?.as_element()?;
        parent.children.iter().position(|&child| child == node_id)
    }

    /// Get the CFI step number for this node (counting only element siblings before it)
    /// CFI uses step indirection: step = (element_index + 1) * 2
    pub fn get_element_step(&self, node_id: DomNodeId) -> Option<usize> {
        let parent_idx = self.dom_nodes.get(node_id.index())?.parent()?;
        let parent = self.dom_nodes.get(parent_idx.index())?.as_element()?;

        let mut element_count = 0;
        for &child_idx in &parent.children {
            if child_idx == node_id {
                // CFI step indirection: (element_index + 1) * 2
                return Some((element_count + 1) * 2);
            }
            // Only count element nodes, not text nodes
            if self.dom_nodes.get(child_idx.index())?.as_element().is_some() {
                element_count += 1;
            }
        }
        None
    }

    /// Returns the CFI odd step for a text node within its parent element.
    /// Step = 2 * (number of preceding element siblings) + 1.
    pub fn get_text_node_step(&self, text_node_id: DomNodeId) -> Option<usize> {
        let parent_idx = self.dom_nodes.get(text_node_id.index())?.parent()?;
        let parent = self.dom_nodes.get(parent_idx.index())?.as_element()?;
        let mut elem_count = 0;
        for &child_idx in &parent.children {
            if child_idx == text_node_id {
                return Some(elem_count * 2 + 1);
            }
            if self.dom_nodes.get(child_idx.index())?.as_element().is_some() {
                elem_count += 1;
            }
        }
        None
    }

    /// Finds the text node at the given CFI odd step within a parent element.
    pub fn find_text_node_by_step(&self, parent_idx: DomNodeId, text_step: usize) -> Option<DomNodeId> {
        self.find_text_nodes_by_step(parent_idx, text_step).into_iter().next()
    }

    /// Returns every adjacent character-data node represented by one odd CFI
    /// step. EPUB CFI deliberately treats adjacent DOM text nodes as a single
    /// logical chunk; browser `Range` offsets therefore span all of them.
    pub fn find_text_nodes_by_step(&self, parent_idx: DomNodeId, text_step: usize) -> Vec<DomNodeId> {
        if text_step == 0 || text_step.is_multiple_of(2) {
            return Vec::new();
        }
        let target_gap = (text_step - 1) / 2;
        let Some(parent) = self.dom_nodes.get(parent_idx.index()).and_then(DomNode::as_element) else {
            return Vec::new();
        };
        let mut elem_count = 0;
        let mut result = Vec::new();
        for &child_idx in &parent.children {
            let Some(child) = self.dom_nodes.get(child_idx.index()) else { continue };
            match child {
                DomNode::Element(_) => elem_count += 1,
                DomNode::Text(_) => {
                    if elem_count == target_gap {
                        result.push(child_idx);
                    }
                }
            }
        }
        result
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
            let element = self.dom_nodes.get(current.index())?.as_element()?;

            // Convert CFI step to element index: (step / 2) - 1
            if step % 2 != 0 {
                // Odd steps point to text nodes, which we don't navigate through
                return None;
            }
            let target_element_index = (step / 2) - 1;

            // Count element children to find the target
            let mut element_count = 0;
            let mut found = None;
            for &child_idx in &element.children {
                if self.dom_nodes.get(child_idx.index())?.as_element().is_some() {
                    if element_count == target_element_index {
                        found = Some(child_idx);
                        break;
                    }
                    element_count += 1;
                }
            }

            current = found?;
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
