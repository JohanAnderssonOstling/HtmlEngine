//! Readable blocks addressed by DOM and glyph array indices.
use crate::stages::text_paint;
use super::RenderAddressingView;
use super::box_queries;
use crate::layout_model::InlineItemKind;
use html_dom::{DomNodeId, ElementRef, element_is_note_reference, element_is_note_target};
use html_text::whitespace_context_root;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// A reading unit with private source identity and document-local character ranges.
/// Font shaping and pagination preserve its identity while text stays fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingTarget {
    element: u32,
    pub glyph_ranges: Vec<Range<u32>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingFragment {
    pub text: String,
    pub target: ReadingTarget,
}

/// Fragments up to a caller-supplied page boundary, adjusted so spoken words
/// remain whole. `next` is the start of the next request in this document.
pub struct ReadingPage {
    pub fragments: Vec<ReadingFragment>,
    pub next: u32,
}

pub fn reading_page(
    value: RenderAddressingView<'_>,
    interval: Range<u32>,
    page_end: u32,
) -> ReadingPage {
    let next = if interval.is_empty() {
        interval.start
    } else if page_end >= interval.end {
        interval.end
    } else {
        render_addressing_view_reading_boundary_after_word(value, page_end)
            .min(interval.end)
            .max(interval.start)
    };
    let fragments = if next > interval.start {
        render_addressing_view_reading_fragments_in(value, interval.start..next)
    } else {
        Vec::new()
    };
    ReadingPage { fragments, next }
}

impl RenderAddressingView<'_> {
    /// All reading units and the first unit extending beyond `glyph`. The index
    /// equals the unit count when the position is past all units.
    pub fn reading_targets_at(self, glyph: u32) -> (usize, Vec<ReadingTarget>) {
        let targets = render_addressing_view_reading_targets_in(self, 0..u32::MAX);
        let index = targets
            .iter()
            .position(|target| target.glyph_ranges.iter().any(|range| range.end > glyph))
            .unwrap_or(targets.len());
        (index, targets)
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
                text.push(character);
                glyph
            })
            .collect();
        let span = select(&text)?;
        if span.is_empty() || text.get(span.clone()).is_none() {
            return None;
        }
        let mut chunk_target = reading_target_with_glyph_ranges(&target, Vec::new());
        let mut next = None;
        for ((offset, character), &glyph) in text.char_indices().zip(&positions) {
            if span.contains(&offset)
                && let Some(glyph) = glyph
            {
                match chunk_target.glyph_ranges.last_mut() {
                    Some(range) if range.end == glyph => range.end = glyph + 1,
                    _ => chunk_target.glyph_ranges.push(glyph..glyph + 1),
                }
            }
            if offset >= span.end && !character.is_whitespace() {
                next = glyph;
                break;
            }
        }
        let next = next.or_else(|| {
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

    fn reading_characters_from<'a>(
        self,
        target: &'a ReadingTarget,
        start: u32,
    ) -> impl Iterator<Item = (char, Option<u32>)> + 'a
    where
        Self: 'a,
    {
        let text = self.doc.render_view(self.glyph_metrics).text();
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
                let character = text_paint::render_text_view_character_at(text, index as usize);
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
        let tree = self.doc.inputs.layout_tree;
        let mut previous_context = None;
        let mut explicit_break = false;
        self.doc
            .inputs
            .text
            .content()
            .inline_items()
            .iter()
            .filter_map(move |item| {
                let Some(text) = item.text_run() else {
                    if matches!(item.kind, InlineItemKind::Break { .. }) {
                        explicit_break = true;
                    }
                    return None;
                };
                let glyphs = text.glyphs;
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

fn note_reference_class(classes: &str) -> bool {
    classes
        .split_ascii_whitespace()
        .any(note_reference_class_token)
}

fn note_reference_class_token(class: &str) -> bool {
    ["noteref", "footnote-ref", "footnote-reference", "fnanchor"]
        .iter()
        .any(|name| class.eq_ignore_ascii_case(name))
}

fn note_class(classes: &str) -> bool {
    classes.split_ascii_whitespace().any(|class| {
        note_reference_class_token(class)
            || [
                "footnote",
                "footnotes",
                "endnote",
                "endnotes",
                "rearnote",
                "rearnotes",
            ]
            .iter()
            .any(|name| class.eq_ignore_ascii_case(name))
    })
}

fn excluded_element(element: ElementRef<'_>) -> bool {
    matches!(
        element.tag(),
        "table" | "nav" | "script" | "style" | "head" | "template" | "noscript"
    ) || element.attr("class").is_some_and(note_class)
        || element_is_note_target(element)
        || element_is_note_reference(element)
        || element.attr("hidden").is_some()
        || element
            .attr("aria-hidden")
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        || element.aria_roles().any(|role| {
            matches!(
                role,
                "doc-toc" | "doc-endnotes" | "doc-backlink" | "navigation"
            )
        })
        || element.epub_types().any(|kind| {
            matches!(
                kind,
                "toc" | "landmarks" | "page-list" | "endnotes" | "rearnotes" | "backlink"
            )
        })
}

/// Index readable items without copying chapter text or consulting pagination.
pub fn render_addressing_view_reading_targets_in(
    value: RenderAddressingView<'_>,
    interval: Range<u32>,
) -> Vec<ReadingTarget> {
    let (start, end) = (interval.start, interval.end);
    let document = value.doc.document();
    let boxes = value.doc.render_view(value.glyph_metrics).boxes();
    // Explicit references also identify note bodies without EPUB/ARIA
    // attributes. Only local fragments belong to this document.
    let note_ids: HashSet<_> = document
        .node_ids()
        .filter_map(|node| document.element_ref(node))
        .filter(|element| {
            element_is_note_reference(*element)
                || element.attr("class").is_some_and(note_reference_class)
        })
        .filter_map(|element| element.attr("href").and_then(|href| href.strip_prefix('#')))
        .filter(|id| !id.is_empty())
        .collect();
    let mut rendered = HashMap::<DomNodeId, Vec<Range<u32>>>::new();
    for run in text_paint::render_text_view_visible_source_runs(
        value.doc.render_view(value.glyph_metrics).text(),
    ) {
        let glyphs = run.glyphs();
        if glyphs.end <= start
            || glyphs.start >= end
            || box_queries::ancestors(boxes, text_paint::render_text_run_box_id(&run))
                .any(|index| box_queries::is_table(boxes, index))
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
                .push(glyphs.start.max(start)..glyphs.end.min(end));
        }
    }
    let mut targets = Vec::<ReadingTarget>::new();
    for node in document.node_ids() {
        let Some(text) = document.text_ref(node) else {
            continue;
        };
        let Some(glyph_ranges) = rendered.remove(&node) else {
            continue;
        };
        let mut owner = None;
        let mut excluded = false;
        for ancestor in document.dom_ancestors(node) {
            let Some(element) = document.element_ref(ancestor) else {
                break;
            };
            if excluded_element(element)
                || element.attr("id").is_some_and(|id| note_ids.contains(id))
            {
                excluded = true;
                break;
            }
            if owner.is_none() && block_element(element.tag()) {
                owner = Some(ancestor);
            }
        }
        if excluded {
            continue;
        }
        let Some(owner) = owner else { continue };
        let value = text.text().trim();
        if value.starts_with("https://")
            || value.starts_with("http://")
            || value.starts_with("www.")
        {
            continue;
        }
        if let Some(target) = targets
            .last_mut()
            .filter(|target| target.element == owner.raw())
        {
            target.glyph_ranges.extend(glyph_ranges);
        } else {
            targets.push(ReadingTarget {
                element: owner.raw(),
                glyph_ranges,
            });
        }
    }
    targets
}

/// Materialize only the requested item's normalized visible text.
pub fn render_addressing_view_reading_text(
    value: RenderAddressingView<'_>,
    target: &ReadingTarget,
) -> String {
    value
        .reading_characters_from(target, 0)
        .map(|(character, _)| character)
        .collect()
}

/// Returns non-overlapping readable blocks in source order. `start` clips
/// the first block to a current layout position, including mid-paragraph
/// page starts. Inline markup remains inside its owning block.
pub fn render_addressing_view_reading_fragments(
    value: RenderAddressingView<'_>,
    start: u32,
) -> Vec<ReadingFragment> {
    render_addressing_view_reading_fragments_in(value, start..u32::MAX)
}

/// Derive a passage or speech-chunk target while preserving its source identity.
/// Ranges belong to the same document; the caller's vector is moved without copying.
#[inline]
pub fn reading_target_with_glyph_ranges(
    value: &ReadingTarget,
    glyph_ranges: Vec<Range<u32>>,
) -> ReadingTarget {
    ReadingTarget {
        element: value.element,
        glyph_ranges,
    }
}

pub(crate) fn render_addressing_view_reading_fragments_in(
    value: RenderAddressingView<'_>,
    interval: Range<u32>,
) -> Vec<ReadingFragment> {
    render_addressing_view_reading_targets_in(value, interval)
        .into_iter()
        .filter_map(|target| {
            let text = render_addressing_view_reading_text(value, &target);
            (!text.is_empty()).then_some(ReadingFragment { text, target })
        })
        .collect()
}

pub(crate) fn render_addressing_view_reading_boundary_after_word(
    value: RenderAddressingView<'_>,
    mut boundary: u32,
) -> u32 {
    let text = value.doc.render_view(value.glyph_metrics).text();
    let character = |index: u32| text_paint::render_text_view_character_at(text, index as usize);
    let limit = value
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
