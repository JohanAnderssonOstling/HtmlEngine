//! Facts accumulated during the depth-first style pass. Summaries are built
//! lazily when a matcher first needs them and updated as frames enter/leave.
use super::dom::element_language;
use super::selectors::AncestorFilter;
use crate::style::compiled::{FastAlternative, FastTest};
use html_dom::{Document, DomNodeId, ElementRef};
use lightningcss::selector::Component;
use rustc_data_structures::fx::FxHashMap;

pub(super) enum SummaryTest<'a> {
    Name { name: &'a str, lower_name: &'a str },
    Id(&'a str),
    Class(&'a str),
    Universal,
    Any(&'a [FastAlternative]),
}

impl<'a> SummaryTest<'a> {
    pub(super) fn from_fast_test(test: &'a FastTest) -> Option<Self> {
        match test {
            FastTest::LocalName { name, lower_name } => Some(Self::Name {
                name: name.as_ref(),
                lower_name: lower_name.as_ref(),
            }),
            FastTest::Id(id) => Some(Self::Id(id.as_ref())),
            FastTest::Class(class) => Some(Self::Class(class.as_ref())),
            FastTest::Any(alternatives) => Some(Self::Any(alternatives)),
            _ => None,
        }
    }

    pub(super) fn from_components(components: &'a [Component<'_>]) -> Option<Self> {
        match components {
            [] | [Component::ExplicitUniversalType] => Some(Self::Universal),
            [Component::LocalName(name)] => Some(Self::Name {
                name: name.name.as_ref(),
                lower_name: name.lower_name.as_ref(),
            }),
            [Component::ID(id)] => Some(Self::Id(id.as_ref())),
            [Component::Class(class)] => Some(Self::Class(class.as_ref())),
            _ => None,
        }
    }
}

// HTML tag, other tag, ID and class counts share one lookup table. Keys borrow
// immutable document strings; duplicate class tokens balance on removal.
#[derive(Default)]
struct Facts<'a>(FxHashMap<&'a str, [u32; 4]>);

impl<'a> Facts<'a> {
    fn update(&mut self, element: ElementRef<'a>, add: bool) {
        let tag_kind = usize::from(!element.is_html_element_in_html_document());
        self.change(element.tag(), tag_kind, add);
        if let Some(id) = element.id() {
            self.change(id, 2, add);
        }
        for class in element.classes() {
            self.change(class, 3, add);
        }
    }

    fn change(&mut self, value: &'a str, kind: usize, add: bool) {
        if add {
            self.0.entry(value).or_default()[kind] += 1;
        } else {
            let counts = self.0.get_mut(value).expect("active ancestor fact");
            counts[kind] -= 1;
            if *counts == [0; 4] {
                self.0.remove(value);
            }
        }
    }

    fn matches(&self, test: SummaryTest<'_>) -> bool {
        let contains =
            |value: &str, kind: usize| self.0.get(value).is_some_and(|counts| counts[kind] != 0);
        match test {
            SummaryTest::Name { name, lower_name } => contains(lower_name, 0) || contains(name, 1),
            SummaryTest::Id(id) => contains(id, 2),
            SummaryTest::Class(class) => contains(class, 3),
            SummaryTest::Universal => !self.0.is_empty(),
            SummaryTest::Any(alternatives) => alternatives.iter().any(|test| match test {
                FastAlternative::LocalName { name, lower_name } => {
                    contains(lower_name.as_ref(), 0) || contains(name.as_ref(), 1)
                }
                FastAlternative::Id(id) => contains(id.as_ref(), 2),
                FastAlternative::Class(class) => contains(class.as_ref(), 3),
            }),
        }
    }
}

struct Frame<'a> {
    element: ElementRef<'a>,
    next_child: usize,
    child_position: i32,
    visited_children: i32,
    previous_child: Option<DomNodeId>,
    filter: AncestorFilter,
    active_ancestor: bool,
    earlier_siblings: Option<Facts<'a>>,
    child_count: Option<i32>,
    language: Option<Option<&'a str>>,
}

impl<'a> Frame<'a> {
    fn new(element: ElementRef<'a>, child_position: i32, filter: AncestorFilter) -> Self {
        Self {
            element,
            next_child: 0,
            child_position,
            visited_children: 0,
            previous_child: None,
            filter,
            active_ancestor: false,
            earlier_siblings: None,
            child_count: None,
            language: None,
        }
    }
}

pub(crate) struct StyleTraversal<'a> {
    document: &'a Document,
    root_pending: bool,
    frames: Vec<Frame<'a>>,
    ancestors: Option<Facts<'a>>,
}

impl<'a> StyleTraversal<'a> {
    pub(crate) fn new(document: &'a Document) -> Self {
        Self {
            document,
            root_pending: true,
            frames: Vec::new(),
            ancestors: None,
        }
    }

    pub(crate) fn next(&mut self) -> Option<(ElementRef<'a>, AncestorFilter)> {
        if self.root_pending {
            self.root_pending = false;
            let root = self.document.element_ref(self.document.dom_root()?);
            self.frames
                .push(Frame::new(root, 1, AncestorFilter::default()));
            return Some((root, AncestorFilter::default()));
        }
        while let Some(parent) = self.frames.last_mut() {
            // children() wraps a slice iterator, whose nth is constant-time.
            if let Some(child) = parent.element.children().nth(parent.next_child) {
                parent.next_child += 1;
                let Some(child) = self.document.try_element_ref(child) else {
                    continue;
                };
                parent.visited_children += 1;
                if !parent.active_ancestor {
                    parent.filter.insert(parent.element.tag());
                    if let Some(id) = parent.element.id() {
                        parent.filter.insert(id);
                    }
                    for class in parent.element.classes() {
                        parent.filter.insert(class);
                    }
                    if let Some(facts) = self.ancestors.as_mut() {
                        facts.update(parent.element, true);
                    }
                    parent.active_ancestor = true;
                }
                let filter = parent.filter;
                let position = parent.visited_children;
                self.frames.push(Frame::new(child, position, filter));
                return Some((child, filter));
            }
            let finished = self.frames.pop().expect("current traversal frame");
            if finished.active_ancestor {
                if let Some(facts) = self.ancestors.as_mut() {
                    facts.update(finished.element, false);
                }
            }
            if let Some(parent) = self.frames.last_mut() {
                parent.previous_child = Some(finished.element.node_id());
                if let Some(facts) = parent.earlier_siblings.as_mut() {
                    facts.update(finished.element, true);
                }
            }
        }
        None
    }

    fn current(&self, node: DomNodeId) -> Option<&Frame<'a>> {
        // Recursive matching can inspect other nodes (for example in :has()).
        // Facts describe only the element currently receiving its style.
        self.frames
            .last()
            .filter(|frame| frame.element.node_id() == node)
    }

    /// Active ancestors share inherited language. Resolve each frame lazily,
    /// stopping at an already cached ancestor; off-path searches use the DOM.
    pub(super) fn effective_language(&mut self, node: DomNodeId) -> Option<Option<&'a str>> {
        let index = self
            .frames
            .iter()
            .rposition(|frame| frame.element.node_id() == node)?;
        if let Some(language) = self.frames[index].language.as_ref() {
            return Some(*language);
        }
        let mut start = index;
        while start > 0 && self.frames[start - 1].language.as_ref().is_none() {
            start -= 1;
        }
        let mut language = start
            .checked_sub(1)
            .and_then(|parent| self.frames[parent].language.as_ref().copied().flatten());
        for frame in &mut self.frames[start..=index] {
            language = *frame
                .language
                .get_or_insert_with(|| element_language(frame.element).or(language));
        }
        Some(language)
    }

    pub(super) fn ancestor_matches(
        &mut self,
        node: DomNodeId,
        test: SummaryTest<'_>,
    ) -> Option<bool> {
        self.current(node)?;
        let facts = self.ancestors.get_or_insert_with(|| {
            let mut facts = Facts::default();
            for frame in &self.frames[..self.frames.len() - 1] {
                facts.update(frame.element, true);
            }
            facts
        });
        Some(facts.matches(test))
    }

    pub(super) fn previous_sibling(&self, node: DomNodeId) -> Option<Option<DomNodeId>> {
        let index = self
            .frames
            .iter()
            .rposition(|frame| frame.element.node_id() == node)?;
        Some(
            index
                .checked_sub(1)
                .and_then(|parent| self.frames[parent].previous_child),
        )
    }

    pub(super) fn earlier_sibling_matches(
        &mut self,
        node: DomNodeId,
        test: SummaryTest<'_>,
    ) -> Option<bool> {
        self.current(node)?;
        let Some(parent) = self.frames.iter_mut().rev().nth(1) else {
            return Some(false);
        };
        let facts = parent.earlier_siblings.get_or_insert_with(|| {
            let mut facts = Facts::default();
            // next_child points just past the current child; only preceding
            // children have completed and can satisfy a sibling relation.
            for child in parent.element.children().take(parent.next_child - 1) {
                if let Some(child) = self.document.try_element_ref(child) {
                    facts.update(child, true);
                }
            }
            facts
        });
        Some(facts.matches(test))
    }

    pub(super) fn sibling_position(
        &mut self,
        node: DomNodeId,
        need_count: bool,
    ) -> Option<(i32, i32)> {
        let position = self.current(node)?.child_position;
        let count = if need_count {
            self.frames.iter_mut().rev().nth(1).map_or(1, |parent| {
                *parent
                    .child_count
                    .get_or_insert_with(|| parent.element.child_elements().count() as i32)
            })
        } else {
            0
        };
        Some((position, count))
    }
}
