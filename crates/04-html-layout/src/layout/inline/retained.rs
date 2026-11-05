//! Owned paragraph-local results, independent of published fragment storage.

use super::paragraph::ParagraphLayoutResult;
use super::*;
use crate::GlyphResourceGeneration;
use crate::layout::LayoutEngine;
use html_dom::MemoryUsageReport;
use rustc_data_structures::fx::FxHashMap;

const CACHE_BYTES: usize = 4 * 1024 * 1024;
const NO_RECENCY_NODE: u32 = u32::MAX;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(super) struct ParagraphSlot {
    owner: u32,
    start: u32,
    end: u32,
    measurement: bool,
    refined: bool,
}
impl ParagraphSlot {
    pub(super) fn new(engine: &LayoutEngine<'_, '_>, input: &InlineFormattingInput) -> Self {
        Self {
            owner: input.container_box_idx as u32,
            start: input.run_range.start,
            end: input.run_range.end,
            measurement: !engine.placement.retains_output(),
            refined: !engine.text_style_ranges.is_empty(),
        }
    }
}

#[derive(PartialEq)]
pub(super) struct ParagraphDependencies {
    width: f64,
    percentage_height_basis: Option<f64>,
    paragraph: ParagraphLayout,
    overflow: InlineOverflow,
    first_line_height: Option<f64>,
    first_letter_height: Option<f64>,
    book: bool,
    hyphenation_quality: bool,
    force_justify: bool,
    generation: GlyphResourceGeneration,
    shaping: InlinePlanGeneration,
}
impl ParagraphDependencies {
    pub(super) fn new(
        engine: &LayoutEngine<'_, '_>,
        input: &InlineFormattingInput,
        overflow: InlineOverflow,
        first_line_height: Option<f64>,
        first_letter_height: Option<f64>,
    ) -> Self {
        Self {
            width: input.area.width,
            percentage_height_basis: input.area.percentage_height_basis,
            paragraph: input.paragraph,
            overflow,
            first_line_height,
            first_letter_height,
            book: engine.config.book_optimized_text(),
            hyphenation_quality: engine.config.hyphenation_quality(),
            force_justify: engine.config.force_justify(),
            generation: engine.resource_generation,
            shaping: engine.inline_plan_generation,
        }
    }
}

pub(super) struct CachedParagraph {
    pub(super) dependencies: ParagraphDependencies,
    pub(super) token_count: usize,
    pub(super) paragraph: ParagraphLayoutResult,
    /// Insertion timestamp before the first eviction, arena index afterwards.
    recency: u64,
}
impl CachedParagraph {
    pub(super) fn new(
        dependencies: ParagraphDependencies,
        token_count: usize,
        paragraph: ParagraphLayoutResult,
    ) -> Self {
        Self {
            dependencies,
            token_count,
            paragraph,
            recency: 0,
        }
    }

    fn memory_usage_bytes(&self) -> usize {
        size_of::<Self>() + self.paragraph.storage_bytes().into_iter().sum::<usize>()
    }
}
pub(super) enum ParagraphLookup {
    Hit(CachedParagraph),
    Miss { paragraph: ParagraphLayoutResult },
}

#[derive(Clone, Copy)]
enum RecencyNode {
    Cached {
        slot: ParagraphSlot,
        previous: u32,
        next: u32,
    },
    Free {
        next: u32,
    },
}

/// Small caches retain only timestamps. Once eviction is needed, stable
/// indices link their keys without walking the much larger payload buckets.
struct ParagraphRecency {
    nodes: Vec<RecencyNode>,
    oldest: u32,
    newest: u32,
    free: u32,
}

impl Default for ParagraphRecency {
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            oldest: NO_RECENCY_NODE,
            newest: NO_RECENCY_NODE,
            free: NO_RECENCY_NODE,
        }
    }
}

impl ParagraphRecency {
    fn active(&self) -> bool {
        !self.nodes.is_empty()
    }

    fn initialize(&mut self, entries: &mut FxHashMap<ParagraphSlot, CachedParagraph>) {
        debug_assert!(!self.active());
        let capacity = entries.capacity();
        let mut ordered: Vec<_> = entries.iter_mut().collect();
        ordered.sort_unstable_by_key(|(_, entry)| entry.recency);
        self.nodes.reserve_exact(capacity);
        for (slot, entry) in ordered {
            entry.recency = u64::from(self.push(*slot, capacity));
        }
    }

    fn push(&mut self, slot: ParagraphSlot, capacity: usize) -> u32 {
        let node = RecencyNode::Cached {
            slot,
            previous: self.newest,
            next: NO_RECENCY_NODE,
        };
        let index = if self.free != NO_RECENCY_NODE {
            let index = self.free;
            let RecencyNode::Free { next } = self.nodes[index as usize] else {
                unreachable!("free recency slot");
            };
            self.free = next;
            self.nodes[index as usize] = node;
            index
        } else {
            let index = u32::try_from(self.nodes.len()).expect("paragraph recency arena exhausted");
            // Follow the payload map's capacity instead of independently
            // doubling another allocation. Vacant slots are reused above.
            if self.nodes.len() == self.nodes.capacity() {
                self.nodes
                    .reserve_exact(capacity.saturating_sub(self.nodes.len()).max(1));
            }
            self.nodes.push(node);
            index
        };
        if self.newest == NO_RECENCY_NODE {
            self.oldest = index;
        } else {
            let RecencyNode::Cached { next, .. } = &mut self.nodes[self.newest as usize] else {
                unreachable!("newest cached paragraph");
            };
            *next = index;
        }
        self.newest = index;
        index
    }

    fn remove(&mut self, index: u32) {
        let RecencyNode::Cached { previous, next, .. } = self.nodes[index as usize] else {
            unreachable!("live cached paragraph");
        };
        if previous == NO_RECENCY_NODE {
            self.oldest = next;
        } else {
            let RecencyNode::Cached { next: link, .. } = &mut self.nodes[previous as usize] else {
                unreachable!("previous cached paragraph");
            };
            *link = next;
        }
        if next == NO_RECENCY_NODE {
            self.newest = previous;
        } else {
            let RecencyNode::Cached { previous: link, .. } = &mut self.nodes[next as usize] else {
                unreachable!("next cached paragraph");
            };
            *link = previous;
        }
        self.nodes[index as usize] = RecencyNode::Free { next: self.free };
        self.free = index;
    }

    fn oldest_slot(&self) -> ParagraphSlot {
        let RecencyNode::Cached { slot, .. } = self.nodes[self.oldest as usize] else {
            unreachable!("oldest cached paragraph");
        };
        slot
    }

    fn storage_bytes(&self) -> usize {
        self.nodes.capacity() * size_of::<RecencyNode>()
    }
}

/// One mutable owner per layout executor. Taking a result releases the cache
/// borrow before fragment publication can recurse into an atomic paragraph.
/// Base and first-line shaping have distinct keys, each retaining one current
/// local result without accumulating resize history or per-paragraph lists.
/// Intrinsic measurements use separate slots so their speculative widths do
/// not displace the paragraph result used by the published chapter.
#[derive(Default)]
pub(in crate::layout) struct ParagraphLayouts {
    entries: FxHashMap<ParagraphSlot, CachedParagraph>,
    bytes: usize,
    clock: u64,
    recency: ParagraphRecency,
    #[cfg(any(test, feature = "testing"))]
    hits: usize,
    #[cfg(any(test, feature = "testing"))]
    misses: usize,
    #[cfg(any(test, feature = "testing"))]
    hit_owners: FxHashMap<(u32, bool), usize>,
}

impl ParagraphLayouts {
    fn remove(&mut self, slot: &ParagraphSlot) -> Option<CachedParagraph> {
        let mut entry = self.entries.remove(slot)?;
        self.bytes -= entry.memory_usage_bytes();
        if self.recency.active() {
            self.recency.remove(entry.recency as u32);
        }
        entry.recency = 0;
        Some(entry)
    }

    pub(super) fn take(
        &mut self,
        slot: ParagraphSlot,
        dependencies: &ParagraphDependencies,
        tokens: &InlineTokens,
        immutable_text: bool,
    ) -> ParagraphLookup {
        let mut paragraph = ParagraphLayoutResult::default();
        if !immutable_text {
            for refined in [false, true] {
                drop(self.remove(&ParagraphSlot { refined, ..slot }));
            }
        } else if let Some(entry) = self.remove(&slot) {
            if &entry.dependencies == dependencies && entry.token_count == tokens.len() {
                #[cfg(any(test, feature = "testing"))]
                {
                    self.hits += 1;
                    *self
                        .hit_owners
                        .entry((slot.owner, slot.measurement))
                        .or_default() += 1;
                }
                return ParagraphLookup::Hit(entry);
            }
            paragraph = entry.paragraph;
        }
        #[cfg(any(test, feature = "testing"))]
        {
            self.misses += 1;
        }
        ParagraphLookup::Miss { paragraph }
    }

    pub(super) fn insert(&mut self, slot: ParagraphSlot, mut result: CachedParagraph) {
        let bytes = result.memory_usage_bytes();
        if bytes > CACHE_BYTES {
            return;
        }
        if self.recency.active() {
            drop(self.remove(&slot));
            self.entries.reserve(1);
            result.recency = u64::from(self.recency.push(slot, self.entries.capacity()));
            let previous = self.entries.insert(slot, result);
            debug_assert!(previous.is_none());
        } else {
            self.clock = self.clock.wrapping_add(1);
            result.recency = self.clock;
            if let Some(previous) = self.entries.insert(slot, result) {
                self.bytes -= previous.memory_usage_bytes();
            }
        }
        self.bytes += bytes;
        if self.bytes > CACHE_BYTES {
            if !self.recency.active() {
                self.recency.initialize(&mut self.entries);
            }
            while self.bytes > CACHE_BYTES {
                self.remove(&self.recency.oldest_slot())
                    .expect("over-budget cache has an oldest paragraph");
            }
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub(in crate::layout) fn memory_usage_bytes(&self) -> usize {
        self.memory_usage_report().total_bytes()
    }

    pub(in crate::layout) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        let entries = self.entries.capacity() * size_of::<(ParagraphSlot, CachedParagraph)>()
            + self.recency.storage_bytes();
        let mut storage = [0; 5];
        for value in self.entries.values() {
            for (bytes, retained) in storage.iter_mut().zip(value.paragraph.storage_bytes()) {
                *bytes += retained;
            }
        }
        report.add("entries", entries, self.entries.len());
        for (label, bytes) in ["lines", "placements", "adjustments", "tokens", "spacing"]
            .into_iter()
            .zip(storage)
        {
            report.add(label, bytes, self.entries.len());
        }
        report
    }

    #[cfg(any(test, feature = "testing"))]
    pub(in crate::layout) fn layout_hits(&self, box_idx: usize) -> usize {
        self.hit_owners
            .get(&(box_idx as u32, false))
            .copied()
            .unwrap_or(0)
    }

    #[cfg(any(test, feature = "testing"))]
    pub(in crate::layout) fn reuse_counts(&self) -> (usize, usize) {
        (self.hits, self.misses)
    }
}

#[cfg(test)]
mod tests {
    use super::super::paragraph::PreparedLine;
    use super::*;

    fn slot(owner: u32, refined: bool, measurement: bool) -> ParagraphSlot {
        ParagraphSlot {
            owner,
            start: 0,
            end: 1,
            measurement,
            refined,
        }
    }

    fn dependencies(width: f64, shaping: InlinePlanGeneration) -> ParagraphDependencies {
        ParagraphDependencies {
            width,
            percentage_height_basis: None,
            paragraph: ParagraphLayout {
                direction: TextDirection::Ltr,
                indent: ResolvedTextIndent {
                    amount: 0.0,
                    hanging: false,
                    each_line: false,
                    first_line: true,
                },
                text_align: TextAlign::Left,
                text_align_last: TextAlign::Left,
                wrap_style: TextWrapStyle::Auto,
                hanging_punctuation: HangingPunctuation::default(),
            },
            overflow: InlineOverflow {
                clips: false,
                ellipsis: None,
            },
            first_line_height: None,
            first_letter_height: None,
            book: false,
            hyphenation_quality: false,
            force_justify: false,
            generation: GlyphResourceGeneration::UNBOUND,
            shaping,
        }
    }

    fn result(shaping: InlinePlanGeneration, capacity: usize) -> CachedParagraph {
        CachedParagraph::new(
            dependencies(100.0, shaping),
            0,
            ParagraphLayoutResult {
                lines: Vec::with_capacity(capacity),
                ..ParagraphLayoutResult::default()
            },
        )
    }

    #[test]
    fn variants_and_measurements_are_independent_and_stale_results_reuse_storage() {
        let mut cache = ParagraphLayouts::default();
        let shaping = InlinePlanGeneration::new();
        let tokens = InlineTokens::default();
        for refined in [false, true] {
            for measurement in [false, true] {
                cache.insert(slot(0, refined, measurement), result(shaping, 4));
            }
        }
        for refined in [false, true] {
            for measurement in [false, true] {
                let key = slot(0, refined, measurement);
                let ParagraphLookup::Hit(entry) =
                    cache.take(key, &dependencies(100.0, shaping), &tokens, true)
                else {
                    panic!("each shaping/measurement variant retains its own result");
                };
                let storage = entry.paragraph.lines.as_ptr();
                cache.insert(key, entry);
                let ParagraphLookup::Miss { paragraph } =
                    cache.take(key, &dependencies(200.0, shaping), &tokens, true)
                else {
                    panic!("changing width must recompose");
                };
                assert_eq!(paragraph.lines.as_ptr(), storage);
            }
        }
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn uncacheable_content_discards_both_shapes_but_keeps_other_slots() {
        let mut cache = ParagraphLayouts::default();
        let shaping = InlinePlanGeneration::new();
        for key in [
            slot(0, false, false),
            slot(0, true, false),
            slot(0, false, true),
            slot(1, false, false),
        ] {
            cache.insert(key, result(shaping, 4));
        }
        assert!(matches!(
            cache.take(
                slot(0, true, false),
                &dependencies(100.0, shaping),
                &InlineTokens::default(),
                false
            ),
            ParagraphLookup::Miss { .. }
        ));
        assert!(!cache.entries.contains_key(&slot(0, false, false)));
        assert!(!cache.entries.contains_key(&slot(0, true, false)));
        assert!(cache.entries.contains_key(&slot(0, false, true)));
        assert!(cache.entries.contains_key(&slot(1, false, false)));
        assert_eq!(
            cache.bytes,
            cache
                .entries
                .values()
                .map(CachedParagraph::memory_usage_bytes)
                .sum::<usize>()
        );
    }

    #[test]
    fn budget_evicts_the_least_recent_result_and_replacement_keeps_accounting_exact() {
        let mut cache = ParagraphLayouts::default();
        let shaping = InlinePlanGeneration::new();
        let capacity = CACHE_BYTES / (3 * size_of::<PreparedLine>());
        cache.insert(slot(0, false, false), result(shaping, capacity));
        cache.insert(slot(1, false, false), result(shaping, capacity));
        let ParagraphLookup::Hit(recent) = cache.take(
            slot(0, false, false),
            &dependencies(100.0, shaping),
            &InlineTokens::default(),
            true,
        ) else {
            panic!("the first result is still cached");
        };
        cache.insert(slot(0, false, false), recent);
        cache.insert(slot(2, false, false), result(shaping, capacity));
        assert!(cache.entries.contains_key(&slot(0, false, false)));
        assert!(!cache.entries.contains_key(&slot(1, false, false)));
        assert!(cache.entries.contains_key(&slot(2, false, false)));
        assert!(cache.bytes <= CACHE_BYTES);

        let bytes = cache.bytes;
        cache.insert(
            slot(3, false, false),
            result(shaping, CACHE_BYTES / size_of::<PreparedLine>() + 1),
        );
        assert_eq!(cache.bytes, bytes, "oversized results are not retained");
        cache.insert(slot(0, false, false), result(shaping, 0));
        assert_eq!(
            cache.bytes,
            cache
                .entries
                .values()
                .map(CachedParagraph::memory_usage_bytes)
                .sum::<usize>()
        );
    }
}
