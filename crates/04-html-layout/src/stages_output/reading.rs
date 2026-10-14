//! Readable blocks addressed by DOM and glyph array indices.
use super::RenderAddressingView;
use crate::layout_model::InlineItemKind;
use crate::shaping::whitespace_context_root;
use html_dom::{DomNodeId, ElementRef, element_is_note_reference, element_is_note_target};
use html_style_model::Visibility;
use std::collections::HashMap;
use std::ops::Range;

/// Indices into the current document's DOM and normalized character arrays.
/// Font shaping and pagination preserve these indices while text stays fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingTarget {
    pub element: u32,
    pub glyph_ranges: Vec<Range<u32>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingFragment {
    pub text: String,
    pub target: ReadingTarget,
}

impl RenderAddressingView<'_> {
    /// Returns non-overlapping readable blocks in source order. `start` clips
    /// the first block to a current layout position, including mid-paragraph
    /// page starts. Inline markup remains inside its owning block.
    pub fn reading_fragments(self, start: u32) -> Vec<ReadingFragment> {
        self.reading_fragments_in(start..u32::MAX)
    }

    pub fn reading_fragments_in(self, interval: Range<u32>) -> Vec<ReadingFragment> {
        self.reading_targets_in(interval)
            .into_iter()
            .filter_map(|target| {
                let text = self.reading_text(&target);
                (!text.is_empty()).then_some(ReadingFragment { text, target })
            })
            .collect()
    }

    /// Index readable items without copying chapter text or consulting pagination.
    pub fn reading_targets_in(self, interval: Range<u32>) -> Vec<ReadingTarget> {
        let (start, end) = (interval.start, interval.end);
        let document = self.doc.document();
        let boxes = self.doc.render_view().boxes();
        let mut rendered = HashMap::<DomNodeId, Vec<Range<u32>>>::new();
        for run in self.doc.render_view().text().text_runs() {
            if run.is_marker()
                || run.glyphs().end <= start
                || run.glyphs().start >= end
                || self
                    .doc
                    .box_used_style(run.box_idx())
                    .is_some_and(|style| style.visibility() == Visibility::Hidden)
                || boxes
                    .ancestors(run.box_idx())
                    .any(|index| boxes.is_table(index))
            {
                continue;
            }
            if let Some(node) = run
                .dom_text_node()
                .and_then(|raw| document.node_id_from_raw(raw))
            {
                rendered
                    .entry(node)
                    .or_default()
                    .push(run.glyphs().start.max(start)..run.glyphs().end.min(end));
            }
        }
        let mut targets = Vec::<ReadingTarget>::new();
        for node in document.node_ids() {
            let Some(text) = document.text_ref(node) else {
                continue;
            };
            let Some(glyph_ranges) = rendered.get(&node) else {
                continue;
            };
            let mut parent = document.get_dom_parent(node);
            let mut owner = None;
            let mut excluded = false;
            while let Some(ancestor) = parent {
                let Some(element) = document.element_ref(ancestor) else {
                    break;
                };
                if excluded_element(element) {
                    excluded = true;
                    break;
                }
                if owner.is_none() && block_element(element.tag()) {
                    owner = Some(ancestor);
                }
                parent = element.parent();
            }
            if excluded {
                continue;
            }
            let Some(owner) = owner else { continue };
            let value = text.text();
            if value.trim().starts_with("https://")
                || value.trim().starts_with("http://")
                || value.trim().starts_with("www.")
            {
                continue;
            }
            if targets
                .last()
                .is_none_or(|target| target.element != owner.raw())
            {
                targets.push(ReadingTarget {
                    element: owner.raw(),
                    glyph_ranges: Vec::new(),
                });
            }
            targets
                .last_mut()
                .expect("reading item")
                .glyph_ranges
                .extend(glyph_ranges.iter().cloned());
        }
        targets
    }

    /// Materialize only the requested item's normalized visible text.
    pub fn reading_text(self, target: &ReadingTarget) -> String {
        self.reading_characters(target)
            .map(|(character, _)| character)
            .collect()
    }

    /// Extract one bounded synthesis chunk together with its source glyph ranges.
    /// `select` chooses a UTF-8 span from at most `max_chars + 1` normalized
    /// characters. The extra character preserves sentence/word boundary lookahead.
    /// Continue at the returned glyph, without scanning the item's earlier text.
    pub fn reading_chunk(
        self,
        target: &ReadingTarget,
        start: u32,
        max_chars: usize,
        select: impl FnOnce(&str) -> Option<Range<usize>>,
    ) -> Option<(ReadingFragment, Option<u32>)> {
        let mut characters = self.reading_characters_from(target, start);
        let mut text = String::new();
        let positions: Vec<_> = characters
            .by_ref()
            .take(max_chars.saturating_add(1))
            .map(|(character, glyph)| {
                let offset = text.len();
                text.push(character);
                (offset, character, glyph)
            })
            .collect();
        let span = select(&text)?;
        if span.is_empty() || text.get(span.clone()).is_none() {
            return None;
        }
        let mut chunk_target = ReadingTarget {
            element: target.element,
            glyph_ranges: Vec::new(),
        };
        for glyph in positions
            .iter()
            .filter_map(|(offset, _, glyph)| span.contains(offset).then_some(*glyph).flatten())
        {
            match chunk_target.glyph_ranges.last_mut() {
                Some(range) if range.end == glyph => range.end = glyph + 1,
                _ => chunk_target.glyph_ranges.push(glyph..glyph + 1),
            }
        }
        let next = positions
            .iter()
            .find(|(offset, character, _)| *offset >= span.end && !character.is_whitespace())
            .and_then(|(_, _, glyph)| *glyph)
            .or_else(|| {
                characters
                    .find(|(character, _)| !character.is_whitespace())
                    .and_then(|(_, glyph)| glyph)
            });
        text.truncate(span.end);
        text.drain(..span.start);
        Some((
            ReadingFragment {
                text,
                target: chunk_target,
            },
            next,
        ))
    }

    fn reading_characters<'a>(
        self,
        target: &'a ReadingTarget,
    ) -> impl Iterator<Item = (char, Option<u32>)> + 'a
    where
        Self: 'a,
    {
        self.reading_characters_from(target, 0)
    }

    fn reading_characters_from<'a>(
        self,
        target: &'a ReadingTarget,
        start: u32,
    ) -> impl Iterator<Item = (char, Option<u32>)> + 'a
    where
        Self: 'a,
    {
        let text = self.doc.render_view().text();
        let mut started = false;
        let mut pending_space = None;
        let mut separators = self
            .reading_separators()
            .skip_while(move |&index| index < start)
            .peekable();
        target
            .glyph_ranges
            .iter()
            .flat_map(move |range| range.start.max(start)..range.end)
            .flat_map(move |index| {
                while separators
                    .peek()
                    .is_some_and(|&separator| separator <= index)
                {
                    separators.next();
                    if started {
                        // A structural space has no source glyph to highlight.
                        pending_space = Some(None);
                    }
                }
                let character = text
                    .glyph_at(index as usize)
                    .and_then(|glyph| text.glyph_metric(glyph))
                    .map(|metric| metric.ch());
                match character {
                    Some(character) if character.is_whitespace() => {
                        pending_space = Some(Some(index));
                        [None, None]
                    }
                    Some(character) => {
                        let space = pending_space
                            .take()
                            .filter(|_| started)
                            .map(|glyph| (' ', glyph));
                        started = true;
                        [space, Some((character, Some(index)))]
                    }
                    None => [None, None],
                }
            })
            .flatten()
    }

    /// Glyph boundaries separating inline text contexts or explicit line
    /// breaks. This scans run metadata lazily and never copies chapter text.
    fn reading_separators(self) -> impl Iterator<Item = u32> {
        let tree = &self.doc.inputs.layout_tree;
        let mut previous_context = None;
        let mut explicit_break = false;
        self.doc
            .shaped
            .inline_content
            .inline_items()
            .iter()
            .filter_map(move |item| {
                let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) =
                    &item.kind
                else {
                    if matches!(item.kind, InlineItemKind::Break { .. }) {
                        explicit_break = true;
                    }
                    return None;
                };
                if glyphs.is_empty() {
                    return None;
                }
                let context = whitespace_context_root(tree, item.box_idx as usize);
                let separate =
                    explicit_break || previous_context.is_some_and(|previous| previous != context);
                previous_context = Some(context);
                explicit_break = false;
                separate.then_some(glyphs.start)
            })
    }

    /// Avoid cutting a spoken word at a visual hyphenation/page boundary.
    pub fn reading_boundary_after_word(self, mut boundary: u32) -> u32 {
        let text = self.doc.render_view().text();
        let character = |index: u32| {
            text.glyph_at(index as usize)
                .and_then(|glyph| text.glyph_metric(glyph))
                .map(|metric| metric.ch())
        };
        let limit = self
            .reading_separators()
            .find(|&separator| separator >= boundary)
            .unwrap_or(u32::MAX);
        if boundary > 0
            && character(boundary - 1).is_some_and(char::is_alphanumeric)
            && character(boundary).is_some_and(char::is_alphanumeric)
        {
            while boundary < limit && character(boundary).is_some_and(|ch| !ch.is_whitespace()) {
                boundary += 1;
            }
        }
        while boundary < limit && character(boundary).is_some_and(char::is_whitespace) {
            boundary += 1;
        }
        boundary
    }
}

fn block_element(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
            | "blockquote"
            | "pre"
            | "div"
            | "section"
            | "article"
            | "body"
    )
}

fn excluded_element(element: ElementRef<'_>) -> bool {
    matches!(
        element.tag(),
        "table" | "nav" | "script" | "style" | "head" | "template" | "noscript"
    ) || element_is_note_target(element)
        || element_is_note_reference(element)
        || element.attr("hidden").is_some()
        || element
            .attr("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        || element.attr("role").is_some_and(|value| {
            value.split_ascii_whitespace().any(|role| {
                matches!(
                    role,
                    "doc-toc" | "doc-endnotes" | "doc-backlink" | "navigation"
                )
            })
        })
        || element
            .attr("epub:type")
            .or_else(|| element.attr_expanded(Some("http://www.idpf.org/2007/ops"), "type"))
            .is_some_and(|value| {
                value.split_ascii_whitespace().any(|kind| {
                    matches!(
                        kind,
                        "toc" | "landmarks" | "page-list" | "endnotes" | "rearnotes" | "backlink"
                    )
                })
            })
}
