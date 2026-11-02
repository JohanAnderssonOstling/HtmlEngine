//! Document-local annotation anchors and resolved ranges. IDs are opaque host
//! values; publication filtering, persistence and visual styling stay outside.

use crate::geometry::Geometry;
use crate::layout::RenderView;
use crate::targets::{Targets, TextAnchor, TextQuote};
use kurbo::Point;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnnotationAnchor {
    Text(TextAnchor),
    /// A host-decoded address, validated against the quote before use.
    Range {
        preferred: Option<Range<u32>>,
        quote: TextQuote,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentAnnotation<Id = String> {
    pub id: Id,
    pub anchor: AnnotationAnchor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedAnnotation<Id = String> {
    pub id: Id,
    pub range: Range<u32>,
}

/// Resolution and queries preserve source order. Hosts choose precedence when
/// overlapping annotations are activated or painted.
pub struct AnnotationIndex<Id = String> {
    source: Vec<DocumentAnnotation<Id>>,
    resolved: Vec<ResolvedAnnotation<Id>>,
}

impl<Id> Default for AnnotationIndex<Id> {
    fn default() -> Self {
        Self {
            source: Vec::new(),
            resolved: Vec::new(),
        }
    }
}

impl<Id: Clone + Eq> AnnotationIndex<Id> {
    pub fn replace(&mut self, source: impl IntoIterator<Item = DocumentAnnotation<Id>>) {
        self.source = source.into_iter().collect();
        self.resolved.clear();
    }

    pub fn upsert(&mut self, annotation: DocumentAnnotation<Id>) {
        self.remove(&annotation.id);
        self.source.push(annotation);
    }

    pub fn remove(&mut self, id: &Id) {
        self.source.retain(|entry| &entry.id != id);
        self.resolved.retain(|entry| &entry.id != id);
    }

    /// Re-resolve after document edits; source addresses are checked against
    /// quotes, with context-based recovery when the address has become stale.
    pub fn resolve(&mut self, view: RenderView<'_>) {
        let targets = Targets::new(view);
        self.resolved = self
            .source
            .iter()
            .filter_map(|entry| {
                let range = match &entry.anchor {
                    AnnotationAnchor::Text(anchor) => targets.resolve(anchor),
                    AnnotationAnchor::Range { preferred, quote } => {
                        targets.resolve_range(preferred.clone(), quote)
                    }
                }?;
                Some(ResolvedAnnotation {
                    id: entry.id.clone(),
                    range,
                })
            })
            .collect();
    }

    pub fn resolved(&self) -> &[ResolvedAnnotation<Id>] {
        &self.resolved
    }

    pub fn range(&self, id: &Id) -> Option<Range<u32>> {
        self.resolved
            .iter()
            .find(|entry| &entry.id == id)
            .map(|entry| entry.range.clone())
    }

    pub fn at(&self, glyph: u32) -> impl DoubleEndedIterator<Item = &ResolvedAnnotation<Id>> {
        self.resolved
            .iter()
            .filter(move |entry| entry.range.contains(&glyph))
    }

    pub fn at_point(
        &self,
        geometry: Geometry<'_>,
        point: Point,
    ) -> impl DoubleEndedIterator<Item = &ResolvedAnnotation<Id>> {
        let glyph = geometry
            .content_at(point)
            .and_then(|hit| hit.text)
            .map(|hit| hit.glyph);
        self.resolved
            .iter()
            .filter(move |entry| glyph.is_some_and(|glyph| entry.range.contains(&glyph)))
    }

    pub fn overlapping(&self, range: Range<u32>) -> impl Iterator<Item = &ResolvedAnnotation<Id>> {
        self.resolved.iter().filter(move |entry| {
            range.start < range.end
                && entry.range.start < range.end
                && range.start < entry.range.end
        })
    }
}
