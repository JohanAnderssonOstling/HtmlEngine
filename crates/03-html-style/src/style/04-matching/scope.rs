//! Match compiled scope boundaries against a document.
use crate::style::compiled::{PreparedRuleSet, ScopeId};
use crate::style::matching::dom::{TraversalContext, selector_matches_dom_node_in_scope};
use html_dom::{Document, DomNodeId};

#[derive(Clone, Copy)]
pub(crate) struct ScopeMatch {
    pub(crate) root: DomNodeId,
    pub(crate) proximity: u32,
}

pub(crate) fn scope_match<'css>(
    prepared: &PreparedRuleSet<'css>,
    scope: ScopeId,
    document: &Document,
    author_roots: &[Option<DomNodeId>],
    subject: DomNodeId,
    siblings: &mut TraversalContext<'_, '_, 'css>,
) -> Option<ScopeMatch> {
    if scope == ScopeId::NONE {
        return Some(ScopeMatch {
            root: document.dom_root()?,
            proximity: u32::MAX,
        });
    }
    let descriptor = prepared.scope_descriptor(scope);
    let parent = scope_match(
        prepared,
        descriptor.parent,
        document,
        author_roots,
        subject,
        siblings,
    )?;
    let match_for_root = |root, siblings: &mut TraversalContext<'_, '_, 'css>| {
        let mut proximity = 0u32;
        for candidate in std::iter::once(subject).chain(document.dom_ancestors(subject)) {
            if descriptor.end.as_ref().is_some_and(|end| {
                end.0.iter().any(|selector| {
                    selector_matches_dom_node_in_scope(
                        selector, document, candidate, root, siblings,
                    )
                })
            }) {
                return None;
            }
            if candidate == root {
                return Some(ScopeMatch { root, proximity });
            }
            proximity = proximity.saturating_add(1);
        }
        None
    };
    if let Some(start) = &descriptor.start {
        for candidate in std::iter::once(subject).chain(document.dom_ancestors(subject)) {
            if !document.dom_contains(parent.root, candidate) {
                break;
            }
            if start.0.iter().any(|selector| {
                selector_matches_dom_node_in_scope(
                    selector,
                    document,
                    candidate,
                    parent.root,
                    siblings,
                )
            }) && let Some(matched) = match_for_root(candidate, siblings)
            {
                return Some(matched);
            }
        }
        None
    } else {
        let root = author_roots
            .get(descriptor.implicit_root_index? as usize)
            .copied()
            .flatten()?;
        document
            .dom_contains(parent.root, root)
            .then(|| match_for_root(root, siblings))
            .flatten()
    }
}
