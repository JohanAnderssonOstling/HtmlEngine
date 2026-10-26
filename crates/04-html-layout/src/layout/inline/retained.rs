//! Retained line-break choices; all measured geometry belongs to publication.

use super::*;
use crate::GlyphResourceGeneration;
use crate::layout::LayoutEngine;
use crate::layout_model::LineOutput;
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
    force_justify: bool,
    generation: GlyphResourceGeneration,
    shaping: InlinePlanGeneration,
    refined: bool,
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
            refined: !engine.text_style_ranges.is_empty(),
        }
    }
}

/// The source choices are independent of published line metrics and positions.
/// Width/indent constraints are borrowed from the paragraph dependency key.
#[derive(Clone, Copy)]
pub(super) struct CachedBreak {
    start: u32,
    end: u32,
    empty_break: u32,
    flags: u8,
}
impl CachedBreak {
    pub(super) fn new(line: &BrokenLine, paragraph: ParagraphLayout, width: f64) -> Self {
        debug_assert_eq!(line.tab_origin, line.indent);
        debug_assert_eq!(line.width_limit, (width - line.indent).max(0.0));
        debug_assert!(line.indent == 0.0 || line.indent == paragraph.indent.amount);
        let align = match line.align {
            TextAlign::Left => 0,
            TextAlign::Right => 1,
            TextAlign::Center => 2,
            TextAlign::Justify => 3,
        };
        Self {
            start: u32::try_from(line.start).expect("token index capacity"),
            end: u32::try_from(line.end).expect("token index capacity"),
            empty_break: line.empty_segment_break.map_or(u32::MAX, |index| {
                u32::try_from(index).expect("token index capacity")
            }),
            flags: align | (u8::from(line.is_last_line) << 2) | (u8::from(line.indent != 0.0) << 3),
        }
    }
    pub(super) fn expand(self, paragraph: ParagraphLayout, width: f64) -> BrokenLine {
        let indent = if self.flags & 8 != 0 {
            paragraph.indent.amount
        } else {
            0.0
        };
        BrokenLine {
            start: self.start as usize,
            end: self.end as usize,
            empty_segment_break: (self.empty_break != u32::MAX)
                .then_some(self.empty_break as usize),
            indent,
            tab_origin: indent,
            width_limit: (width - indent).max(0.0),
            align: match self.flags & 3 {
                0 => TextAlign::Left,
                1 => TextAlign::Right,
                2 => TextAlign::Center,
                _ => TextAlign::Justify,
            },
            is_last_line: self.flags & 4 != 0,
        }
    }
}
const _: () = assert!(size_of::<CachedBreak>() == 16);

/// Pass-local adjustments borrow the canonical tokens except for changed
/// overflow/pseudo spans. They are released after publishing the line.
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
    pub(super) lines: Vec<PreparedLine>,
    pub(super) adjustments: Vec<LineAdjustment>,
}
impl PreparedParagraph {
    pub(super) fn new(lines: Vec<PreparedLine>, adjustments: Vec<LineAdjustment>) -> Self {
        Self { lines, adjustments }
    }
}

/// A locator into the last publication, plus the paragraph's flow result.
/// Every line's geometry and spacing remain solely in `LineOutput`.
struct PublishedParagraph {
    epoch: u64,
    size: Size,
}

pub(super) struct CachedParagraph {
    pub(super) dependencies: ParagraphDependencies,
    pub(super) token_count: usize,
    pub(super) choices: Vec<CachedBreak>,
    used: u64,
    published: Option<PublishedParagraph>,
}
impl CachedParagraph {
    pub(super) fn new(
        dependencies: ParagraphDependencies,
        token_count: usize,
        choices: Vec<CachedBreak>,
    ) -> Self {
        Self {
            dependencies,
            token_count,
            choices,
            used: 0,
            published: None,
        }
    }
    fn source_line(
        choice: &CachedBreak,
        tokens: &InlineTokens,
        previous: &LineOutput,
    ) -> Option<usize> {
        let glyph = tokens[choice.start as usize..choice.end as usize]
            .iter()
            .find_map(|token| match token.kind() {
                InlineTokenKind::Glyph { glyph_idx } => Some(glyph_idx),
                _ => None,
            })?;
        let index = previous.glyph_line_indices.get(glyph as usize)?;
        let line = previous.lines.get(index)?;
        line.glyphs.contains(&glyph).then_some(index)
    }

    pub(super) fn capture_publication(
        &mut self,
        engine: &LayoutEngine<'_, '_>,
        input: &InlineFormattingInput,
        start: usize,
        size: Size,
    ) {
        self.published = None;
        if !engine.placement.retains_output() {
            return;
        }
        let output = &engine.fragments.state().line_output;
        let lines = &output.lines[start..];
        if lines.len() != self.choices.len()
            || lines.is_empty()
            || input.paragraph.direction != TextDirection::Ltr
            || self
                .choices
                .iter()
                .any(|choice| !matches!(choice.flags & 3, 0 | 3))
            || lines.iter().any(|line| {
                line.owner_box_idx as usize != input.container_box_idx
                    || line.glyphs.is_empty()
                    || line.details(&output.line_details).is_some_and(|detail| {
                        detail.text_fragments.is_some() || !detail.inline_box_fragments.is_empty()
                    })
            })
            || output
                .hyphen_fragments
                .iter()
                .any(|fragment| fragment.line_idx >= start)
            || output
                .ellipsis_fragments
                .iter()
                .any(|fragment| fragment.line_idx >= start)
        {
            return;
        }
        self.published = Some(PublishedParagraph {
            epoch: engine.publication_epoch,
            size,
        });
    }

    /// Validate the complete paragraph before moving any published records.
    /// Invalid or partially consumed output falls back to ordinary measurement.
    pub(super) fn reuse_publication(
        &mut self,
        engine: &mut LayoutEngine<'_, '_>,
        input: &InlineFormattingInput,
        tokens: &InlineTokens,
    ) -> Option<Size> {
        let published = self.published.as_ref()?;
        if !engine.placement.retains_output()
            || published.epoch.wrapping_add(1) != engine.publication_epoch
        {
            return None;
        }
        let previous = engine.previous_lines.as_ref()?;
        if self.choices.iter().any(|choice| {
            Self::source_line(choice, tokens, previous).is_none_or(|index| {
                previous.lines[index].owner_box_idx as usize != input.container_box_idx
            })
        }) {
            return None;
        }
        let size = published.size;
        let mut y = 0.0;
        for choice in &self.choices {
            let previous = engine
                .previous_lines
                .as_mut()
                .expect("validated publication");
            let index = Self::source_line(choice, tokens, previous).expect("validated source line");
            let mut line = std::mem::take(&mut previous.lines[index]);
            let output = &mut engine.fragments.state_mut().line_output;
            // Reconstruct the same additions used by fresh publication. An
            // absolute-position delta would accumulate floating-point drift.
            let indent = if choice.flags & 8 != 0 {
                input.paragraph.indent.amount
            } else {
                0.0
            };
            line.point = Point::new(input.area.origin.x + indent, input.area.origin.y + y);
            y += line.height;
            line.prepared_text_runs = 0..0;
            line.native_text_runs_complete = false;
            line.details = line.details.map(|old| {
                output.line_details.push(std::mem::take(
                    &mut previous.line_details[old.get() as usize - 1],
                ));
                (output.line_details.len() as u32)
                    .try_into()
                    .expect("line-detail capacity")
            });
            output
                .line_glyph_offsets
                .push(previous.line_glyph_offsets.take_group(index));
            output
                .line_glyph_advances
                .push(previous.line_glyph_advances.take_group(index));
            output.lines.push(line);
            engine.push_line_owner(input.container_box_idx as u32);
        }
        engine.record_timing(|timings| timings.reused_lines += self.choices.len());
        self.published.as_mut().unwrap().epoch = engine.publication_epoch;
        Some(size)
    }

    fn memory_usage_bytes(&self) -> usize {
        size_of::<Self>() + self.choices.capacity() * size_of::<CachedBreak>()
    }
}
pub(super) enum ParagraphLookup {
    Hit(CachedParagraph),
    Miss { choices: Vec<CachedBreak> },
}

/// One mutable owner per layout executor. Taking a result releases the cache
/// borrow before fragment publication can recurse into an atomic paragraph.
/// The two retained variants cover base and first-line shaping. Each keeps
/// only its current layout inputs, without accumulating resize history.
/// Intrinsic measurements use separate slots so their speculative widths do
/// not displace the paragraph result used by the published chapter.
#[derive(Default)]
pub(in crate::layout) struct ParagraphLayouts {
    entries: FxHashMap<ParagraphSlot, Vec<CachedParagraph>>,
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
    pub(in crate::layout) fn has_publication(&self) -> bool {
        self.entries
            .values()
            .flatten()
            .any(|entry| entry.published.is_some())
    }

    pub(super) fn take(
        &mut self,
        slot: ParagraphSlot,
        dependencies: &ParagraphDependencies,
        tokens: &InlineTokens,
        immutable_text: bool,
    ) -> ParagraphLookup {
        if !immutable_text {
            if let Some(entries) = self.entries.remove(&slot) {
                self.bytes -= entries
                    .iter()
                    .map(CachedParagraph::memory_usage_bytes)
                    .sum::<usize>();
            }
            #[cfg(test)]
            {
                self.misses += 1;
            }
            return ParagraphLookup::Miss {
                choices: Vec::new(),
            };
        }
        let entries = self.entries.get_mut(&slot);
        if let Some(entries) = entries {
            let exact = entries.iter().position(|entry| {
                &entry.dependencies == dependencies && entry.token_count == tokens.len()
            });
            let index = exact.or_else(|| {
                entries
                    .iter()
                    .position(|entry| entry.dependencies.refined == dependencies.refined)
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
                    choices: result.choices,
                };
            }
        }
        #[cfg(test)]
        {
            self.misses += 1;
        }
        ParagraphLookup::Miss {
            choices: Vec::new(),
        }
    }

    pub(super) fn insert(&mut self, slot: ParagraphSlot, mut result: CachedParagraph) {
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
            + self.entries.capacity() * size_of::<(ParagraphSlot, Vec<CachedParagraph>)>()
            + self
                .entries
                .values()
                .map(|entries| (entries.capacity() - entries.len()) * size_of::<CachedParagraph>())
                .sum::<usize>()
    }

    pub(in crate::layout) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        let mut entries =
            self.entries.capacity() * size_of::<(ParagraphSlot, Vec<CachedParagraph>)>();
        let mut choices = 0;
        for values in self.entries.values() {
            entries += values.capacity() * size_of::<CachedParagraph>();
            choices += values
                .iter()
                .map(|value| value.choices.capacity() * size_of::<CachedBreak>())
                .sum::<usize>();
        }
        for (label, bytes) in [
            ("entries", entries),
            ("lines", choices),
            ("tokens", 0),
            ("placements", 0),
            ("adjustments", 0),
            ("spacing", 0),
            ("dependencies", 0),
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
