use super::{MemoryUsage, MemoryUsageReport};

#[derive(Clone, Debug)]
pub struct DocumentTocEntry {
    pub level: u8,
    pub title: u16,
    pub href: u16,
    pub node_idx: u32,
}

impl MemoryUsage for DocumentTocEntry {
    fn memory_usage_report(&self) -> MemoryUsageReport {
        MemoryUsageReport::new()
    }
}

#[derive(Clone, Debug)]
pub struct DocumentTocNode {
    pub level: u8,
    pub title: u16,
    pub href: u16,
    pub node_idx: u32,
    pub children: Vec<DocumentTocNode>,
}

impl MemoryUsage for DocumentTocNode {
    fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<DocumentTocNode>("DocumentTocNode.children.storage", self.children.capacity(), self.children.len());
        for child in &self.children {
            report.extend_prefixed("DocumentTocNode.children", child.memory_usage_report());
        }
        report
    }
}

pub(crate) fn build_document_toc_tree(entries: &[DocumentTocEntry]) -> Vec<DocumentTocNode> {
    fn node_mut<'a>(roots: &'a mut Vec<DocumentTocNode>, path: &[usize]) -> &'a mut DocumentTocNode {
        let mut node = &mut roots[path[0]];
        for &idx in &path[1..] {
            node = &mut node.children[idx];
        }
        node
    }

    let mut roots: Vec<DocumentTocNode> = Vec::new();
    let mut path: Vec<usize> = Vec::new();

    for entry in entries {
        let parent_depth = usize::from(entry.level.saturating_sub(1));
        while path.len() > parent_depth {
            path.pop();
        }

        let node = DocumentTocNode { level: entry.level, title: entry.title, href: entry.href, node_idx: entry.node_idx, children: Vec::new() };

        if path.is_empty() {
            roots.push(node);
            path.push(roots.len() - 1);
        } else {
            let parent = node_mut(&mut roots, &path);
            parent.children.push(node);
            let idx = parent.children.len() - 1;
            path.push(idx);
        }
    }

    roots
}
