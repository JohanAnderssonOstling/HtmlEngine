//! Paragraph-local results, separate from chapter coordinates and output arenas.

use super::*;
use crate::GlyphResourceGeneration;
use crate::layout::{FloatBand, LayoutEngine};
use crate::stages::TextPaintRange;
use html_dom::MemoryUsageReport;
use rustc_data_structures::fx::FxHashMap;

const CACHE_BYTES: usize = 4 * 1024 * 1024;
const VARIANTS_PER_PARAGRAPH: usize = 2;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(super) struct ParagraphSlot {
    owner: u32,
    start: u32,
    end: u32,
    measurement: bool,
}

impl ParagraphSlot {
    pub(super) fn new(input: &InlineFormattingInput, measurement: bool) -> Self {
        Self {
            owner: input.container_box_idx as u32,
            start: input.run_range.start,
            end: input.run_range.end,
            measurement,
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
    sentence_per_line: bool,
    force_justify: bool,
    generation: GlyphResourceGeneration,
    text_styles: Vec<TextPaintRange>,
    float_wrapping: bool,
    exclusions: Vec<FloatBand>,
}

impl ParagraphDependencies {
    pub(super) fn new(
        engine: &LayoutEngine<'_, '_>,
        input: &InlineFormattingInput,
        overflow: InlineOverflow,
        first_line_height: Option<f64>,
        first_letter_height: Option<f64>,
        floats: bool,
    ) -> Self {
        let mut glyphs = u32::MAX..0;
        for run in &engine.text.inline_items()
            [input.run_range.start as usize..input.run_range.end as usize]
        {
            if let InlineItemKind::Text { glyphs: range }
            | InlineItemKind::Marker { glyphs: range } = &run.kind
            {
                glyphs.start = glyphs.start.min(range.start);
                glyphs.end = glyphs.end.max(range.end);
            }
        }
        let ranges = engine.text_style_ranges;
        let first = ranges.partition_point(|range| range.glyphs.end <= glyphs.start);
        let text_styles = ranges[first..]
            .iter()
            .take_while(|range| range.glyphs.start < glyphs.end)
            .cloned()
            .collect();
        Self {
            width: input.area.width,
            percentage_height_basis: input.area.percentage_height_basis,
            paragraph: input.paragraph,
            overflow,
            first_line_height,
            first_letter_height,
            book: engine.config.book_optimized_text(),
            hyphenation_quality: engine.config.hyphenation_quality(),
            sentence_per_line: engine.config.sentence_per_line(),
            force_justify: engine.config.force_justify(),
            generation: engine.resource_generation.clone(),
            text_styles,
            float_wrapping: floats,
            exclusions: if floats {
                engine
                    .floats
                    .current_float_context()
                    .map_or_else(Vec::new, |context| {
                        context.paragraph_bands(input.area.origin)
                    })
            } else {
                Vec::new()
            },
        }
    }
}

/// Only uncommon overflow/pseudo spans need a private token copy. Ordinary
/// lines address the paragraph's existing immutable token storage by range.
pub(super) struct LineAdjustment {
    pub(super) line: usize,
    pub(super) tokens: Box<[InlineToken]>,
    pub(super) runs: Option<Box<[InlineTokenMetrics]>>,
}

pub(super) struct PreparedLine {
    pub(super) tokens: Range<usize>,
    pub(super) origin: Point,
    pub(super) layout: LineLayout,
}

pub(super) struct PreparedParagraph {
    pub(super) dependencies: ParagraphDependencies,
    pub(super) tokens: ParagraphTokenInput,
    pub(super) lines: Vec<PreparedLine>,
    pub(super) adjustments: Vec<LineAdjustment>,
    pub(super) size: Size,
    used: u64,
}

impl PreparedParagraph {
    pub(super) fn new(
        dependencies: ParagraphDependencies,
        tokens: &InlineTokens,
        lines: Vec<PreparedLine>,
        adjustments: Vec<LineAdjustment>,
        size: Size,
    ) -> Self {
        Self {
            dependencies,
            tokens: ParagraphTokenInput::new(tokens),
            lines,
            adjustments,
            size,
            used: 0,
        }
    }

    fn memory_usage_bytes(&self) -> usize {
        size_of::<Self>()
            + self.tokens.retained_bytes()
            + self.dependencies.text_styles.capacity() * size_of::<TextPaintRange>()
            + self.dependencies.exclusions.capacity() * size_of::<FloatBand>()
            + self.lines.capacity() * size_of::<PreparedLine>()
            + self.adjustments.capacity() * size_of::<LineAdjustment>()
            + self
                .lines
                .iter()
                .map(|line| {
                    line.layout.punctuation_space_advances.capacity() * size_of::<(u32, f32)>()
                        + line.layout.placements.as_ref().map_or(0, |placements| {
                            placements.capacity() * size_of::<TokenPlacement>()
                        })
                })
                .sum::<usize>()
            + self
                .adjustments
                .iter()
                .map(|adjustment| {
                    adjustment.tokens.len() * size_of::<InlineToken>()
                        + adjustment
                            .runs
                            .as_ref()
                            .map_or(0, |runs| runs.len() * size_of::<InlineTokenMetrics>())
                })
                .sum::<usize>()
    }
}

pub(super) enum ParagraphLookup {
    Hit(PreparedParagraph),
    Miss {
        lines: Vec<PreparedLine>,
        adjustments: Vec<LineAdjustment>,
    },
}

/// One mutable owner per layout executor. Taking a result releases the cache
/// borrow before fragment publication can recurse into an atomic paragraph.
/// The two retained variants cover base and first-line shaping. Each keeps
/// only its current layout inputs, without accumulating resize history.
/// Intrinsic measurements use separate slots so their speculative widths do
/// not displace the paragraph result used by the published chapter.
#[derive(Default)]
pub(in crate::layout) struct ParagraphLayouts {
    entries: FxHashMap<ParagraphSlot, Vec<PreparedParagraph>>,
    bytes: usize,
    clock: u64,
    #[cfg(test)]
    hits: usize,
    #[cfg(test)]
    misses: usize,
    #[cfg(test)]
    hit_owners: FxHashMap<(u32, bool), usize>,
}

impl ParagraphLayouts {
    pub(super) fn take(
        &mut self,
        slot: ParagraphSlot,
        dependencies: &ParagraphDependencies,
        tokens: &InlineTokens,
    ) -> ParagraphLookup {
        let entries = self.entries.get_mut(&slot);
        if let Some(entries) = entries {
            let exact = entries.iter().position(|entry| {
                &entry.dependencies == dependencies && entry.tokens.matches(tokens)
            });
            let index = exact.or_else(|| {
                entries.iter().position(|entry| {
                    entry.dependencies.text_styles.is_empty() == dependencies.text_styles.is_empty()
                })
            });
            if let Some(index) = index {
                let result = entries.swap_remove(index);
                self.bytes -= result.memory_usage_bytes();
                if exact.is_some() {
                    #[cfg(test)]
                    {
                        self.hits += 1;
                        *self
                            .hit_owners
                            .entry((slot.owner, slot.measurement))
                            .or_default() += 1;
                    }
                    return ParagraphLookup::Hit(result);
                }
                #[cfg(test)]
                {
                    self.misses += 1;
                }
                return ParagraphLookup::Miss {
                    lines: result.lines,
                    adjustments: result.adjustments,
                };
            }
        }
        #[cfg(test)]
        {
            self.misses += 1;
        }
        ParagraphLookup::Miss {
            lines: Vec::new(),
            adjustments: Vec::new(),
        }
    }

    pub(super) fn insert(&mut self, slot: ParagraphSlot, mut result: PreparedParagraph) {
        let bytes = result.memory_usage_bytes();
        if bytes > CACHE_BYTES {
            return;
        }
        self.clock = self.clock.wrapping_add(1);
        result.used = self.clock;
        let entries = self.entries.entry(slot).or_default();
        if entries.len() == VARIANTS_PER_PARAGRAPH {
            let oldest = entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.used)
                .unwrap()
                .0;
            self.bytes -= entries.swap_remove(oldest).memory_usage_bytes();
        }
        if entries.len() == entries.capacity() {
            entries.reserve_exact(1);
        }
        entries.push(result);
        self.bytes += bytes;
        while self.bytes > CACHE_BYTES {
            let oldest = self
                .entries
                .iter()
                .flat_map(|(&slot, entries)| {
                    entries
                        .iter()
                        .enumerate()
                        .map(move |(index, entry)| (entry.used, slot, index))
                })
                .min_by_key(|entry| entry.0);
            let Some((_, slot, index)) = oldest else {
                break;
            };
            let entries = self.entries.get_mut(&slot).unwrap();
            self.bytes -= entries.swap_remove(index).memory_usage_bytes();
            if entries.is_empty() {
                self.entries.remove(&slot);
            }
        }
    }

    #[cfg(test)]
    pub(in crate::layout) fn memory_usage_bytes(&self) -> usize {
        self.bytes
            + self.entries.capacity() * size_of::<(ParagraphSlot, Vec<PreparedParagraph>)>()
            + self
                .entries
                .values()
                .map(|entries| {
                    (entries.capacity() - entries.len()) * size_of::<PreparedParagraph>()
                })
                .sum::<usize>()
    }

    pub(in crate::layout) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        let mut tokens = 0;
        let mut geometry = 0;
        let mut placements = 0;
        let mut adjustments = 0;
        let mut spacing = 0;
        let mut dependencies = 0;
        let mut entries =
            self.entries.capacity() * size_of::<(ParagraphSlot, Vec<PreparedParagraph>)>();
        for values in self.entries.values() {
            entries += values.capacity() * size_of::<PreparedParagraph>();
            for value in values {
                tokens += value.tokens.retained_bytes();
                geometry += value.lines.capacity() * size_of::<PreparedLine>();
                dependencies += value.dependencies.text_styles.capacity()
                    * size_of::<TextPaintRange>()
                    + value.dependencies.exclusions.capacity() * size_of::<FloatBand>();
                adjustments += value.adjustments.capacity() * size_of::<LineAdjustment>()
                    + value
                        .adjustments
                        .iter()
                        .map(|entry| {
                            entry.tokens.len() * size_of::<InlineToken>()
                                + entry
                                    .runs
                                    .as_ref()
                                    .map_or(0, |runs| runs.len() * size_of::<InlineTokenMetrics>())
                        })
                        .sum::<usize>();
                for line in &value.lines {
                    placements += line
                        .layout
                        .placements
                        .as_ref()
                        .map_or(0, |values| values.capacity() * size_of::<TokenPlacement>());
                    spacing +=
                        line.layout.punctuation_space_advances.capacity() * size_of::<(u32, f32)>();
                }
            }
        }
        for (label, bytes) in [
            ("entries", entries),
            ("tokens", tokens),
            ("lines", geometry),
            ("placements", placements),
            ("adjustments", adjustments),
            ("spacing", spacing),
            ("dependencies", dependencies),
        ] {
            report.add(label, bytes, self.entries.len());
        }
        report
    }

    #[cfg(test)]
    pub(in crate::layout) fn published_hits(&self, box_idx: usize) -> usize {
        self.hit_owners
            .get(&(box_idx as u32, false))
            .copied()
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(in crate::layout) fn reuse_counts(&self) -> (usize, usize) {
        (self.hits, self.misses)
    }
}
