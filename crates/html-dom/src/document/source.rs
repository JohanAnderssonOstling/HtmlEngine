//! Source addresses and UTF-16 correspondence, independent of rendering.

use super::{Document, DomNodeId, NodeRef, TextRef};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceElementStep<'a> {
    step: usize,
    id: Option<&'a str>,
}

impl<'a> SourceElementStep<'a> {
    pub fn step(self) -> usize {
        self.step
    }

    pub fn id(self) -> Option<&'a str> {
        self.id
    }
}

/// A source position within a logical chunk of adjacent DOM text nodes.
/// Publication-specific syntaxes such as EPUB CFI are built outside layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePosition<'a> {
    elements: Vec<SourceElementStep<'a>>,
    text_step: usize,
    utf16_offset: usize,
}

impl<'a> SourcePosition<'a> {
    pub fn elements(&self) -> impl ExactSizeIterator<Item = SourceElementStep<'a>> + '_ {
        self.elements.iter().copied()
    }

    pub fn text_step(&self) -> usize {
        self.text_step
    }

    pub fn utf16_offset(&self) -> usize {
        self.utf16_offset
    }
}

/// One text child's position within its parent's source-addressed text chunk.
#[derive(Clone, Debug)]
pub struct TextChildPosition {
    pub node: DomNodeId,
    pub text_step: usize,
    pub utf16_range: Range<usize>,
}

impl Document {
    /// Element steps from the document root, with optional source ID assertions.
    pub fn element_source_path(&self, element: DomNodeId) -> Option<Vec<SourceElementStep<'_>>> {
        self.element_ref(element)?;
        self.get_dom_path_to_node(element)
            .into_iter()
            .skip(1)
            .map(|node| {
                Some(SourceElementStep {
                    step: self.get_element_step(node)?,
                    id: self.get_dom_id(node),
                })
            })
            .collect()
    }

    /// Text children with their source-addressed odd steps. Adjacent text
    /// nodes share a step; element children start a new chunk. This iterator
    /// does not scan text contents or allocate a collection.
    pub fn text_children_with_steps(
        &self,
        parent: DomNodeId,
    ) -> impl Iterator<Item = (TextRef<'_>, usize)> + '_ {
        let mut element_count = 0;
        self.element_ref(parent)
            .into_iter()
            .flat_map(|element| element.children())
            .filter_map(move |node| match self.node_ref(node)? {
                NodeRef::Element(_) => {
                    element_count += 1;
                    None
                }
                NodeRef::Text(text) => Some((text, element_count * 2 + 1)),
            })
    }

    /// Enumerate text children in source order. Adjacent character-data nodes
    /// share an odd step and accumulate UTF-16 offsets; element children start
    /// a new chunk. Consumers can cache this iterator per parent for bulk reads.
    pub fn text_child_positions(
        &self,
        parent: DomNodeId,
    ) -> impl Iterator<Item = TextChildPosition> + '_ {
        let mut previous_step = 0;
        let mut utf16_offset = 0;
        self.text_children_with_steps(parent)
            .map(move |(text, text_step)| {
                if previous_step != text_step {
                    previous_step = text_step;
                    utf16_offset = 0;
                }
                let start = utf16_offset;
                utf16_offset += text.text().encode_utf16().count();
                TextChildPosition {
                    node: text.node_id(),
                    text_step,
                    utf16_range: start..utf16_offset,
                }
            })
    }

    /// Translate a node-local UTF-16 offset to a document-relative source
    /// position. `after_character` advances over one complete Unicode scalar.
    pub fn source_position_for_text_node(
        &self,
        node: DomNodeId,
        local_offset: usize,
        after_character: bool,
    ) -> Option<SourcePosition<'_>> {
        let text = self.text_ref(node)?;
        let parent = text.parent()?;
        let position = self
            .text_child_positions(parent)
            .find(|position| position.node == node)?;
        let local_offset = if after_character {
            utf16_offset_after_character(text.text(), local_offset)?
        } else {
            local_offset
        };
        Some(SourcePosition {
            elements: self.element_source_path(parent)?,
            text_step: position.text_step,
            utf16_offset: position.utf16_range.start + local_offset,
        })
    }
}

fn utf16_offset_after_character(text: &str, wanted: usize) -> Option<usize> {
    let mut offset = 0usize;
    for character in text.chars() {
        if offset == wanted {
            return Some(offset + character.len_utf16());
        }
        if offset > wanted {
            return None;
        }
        offset += character.len_utf16();
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::{DocumentBuilder, DomElementAttributeCache, DomElementData};

    #[test]
    fn text_steps_cfi_lookups_and_utf16_positions_share_chunk_boundaries() {
        let mut builder = DocumentBuilder::new();
        let tag = builder.intern_string("div");
        let element = || DomElementData {
            tag,
            namespace: None,
            attributes: Vec::new(),
            attribute_cache: DomElementAttributeCache::new(None, 0..0),
            image_idx: None,
        };
        let (root, builder) = builder.append_dom_element(None, element()).unwrap();
        let mut builder = builder.set_dom_root(root).unwrap();
        let astral = builder.append_dom_text(root, "😀").unwrap();
        let empty = builder.append_dom_text(root, "").unwrap();
        let adjacent = builder.append_dom_text(root, "b").unwrap();
        let child = builder.append_dom_element(Some(root), element()).unwrap();
        let nested = builder.append_dom_text(child, "nested").unwrap();
        let after = builder.append_dom_text(root, "c").unwrap();
        builder.append_dom_element(Some(root), element()).unwrap();
        builder.append_dom_element(Some(root), element()).unwrap();
        let tail = builder.append_dom_text(root, "終").unwrap();
        let document = builder.finish().unwrap();

        let expected = [
            (astral, 1, 0..2),
            (empty, 1, 2..2),
            (adjacent, 1, 2..3),
            (after, 3, 0..1),
            (tail, 7, 0..1),
        ];
        assert_eq!(
            document
                .text_children_with_steps(root)
                .map(|(text, step)| (text.node_id(), step))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|(node, step, _)| (*node, *step))
                .collect::<Vec<_>>()
        );
        for (position, (node, step, utf16_range)) in
            document.text_child_positions(root).zip(&expected)
        {
            assert_eq!(
                (position.node, position.text_step, position.utf16_range),
                (*node, *step, utf16_range.clone())
            );
            assert_eq!(document.get_text_node_step(*node), Some(*step));
            let source = document
                .source_position_for_text_node(*node, 0, false)
                .unwrap();
            assert_eq!(
                (source.text_step(), source.utf16_offset()),
                (*step, utf16_range.start)
            );
        }
        assert_eq!(
            document.find_text_nodes_by_step(root, 1),
            [astral, empty, adjacent]
        );
        assert_eq!(document.find_text_node_by_step(root, 1), Some(astral));
        assert_eq!(document.find_text_nodes_by_step(root, 3), [after]);
        assert_eq!(document.find_text_node_by_step(root, 7), Some(tail));
        assert_eq!(document.find_text_node_by_step(child, 1), Some(nested));
        for step in [0, 2, 4, 5, usize::MAX] {
            assert!(document.find_text_nodes_by_step(root, step).is_empty());
            assert_eq!(document.find_text_node_by_step(root, step), None);
        }
        assert_eq!(document.get_text_node_step(child), None);
        assert_eq!(document.find_text_node_by_step(astral, 1), None);
    }
}
