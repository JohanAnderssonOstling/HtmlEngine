//! Retained token arenas and separately owned composition state. Cache hits
//! borrow token slices and update derived state without moving it through tokens.
//! Construction and prose segmentation can still recycle changed token buffers.

use super::arenas::{ArenaRange, TokenArena};
use crate::inline::s03_compose::CompositionState;
#[cfg(test)]
use crate::inline::s03_compose::wrapping::KpPlan;
use crate::inline::storage::TokenData;
use crate::inline::tokens::{
    INLINE_SUMMARY_BLOCK_TOKENS, InlineSummaryBlock, InlineTokenMetrics, InlineTokens,
    PreparedInlinePlanKey, PreparedTokenFeatures, ReplacedToken,
};
use html_dom::MemoryUsageReport;
use rustc_data_structures::fx::FxHashMap;
use std::mem::swap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InlinePlanGeneration(u64);

static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

impl InlinePlanGeneration {
    pub fn new() -> Self {
        Self(
            NEXT_GENERATION
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .expect("inline-plan generation space exhausted"),
        )
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct PlanRanges {
    runs: ArenaRange,
    summaries: ArenaRange,
    replaced: ArenaRange,
    metadata: ArenaRange,
    widths: ArenaRange,
    features: PreparedTokenFeatures,
}
impl PlanRanges {
    fn add_lengths_to(self, totals: &mut [usize; 5]) {
        for (total, range) in totals.iter_mut().zip([
            self.runs,
            self.summaries,
            self.replaced,
            self.metadata,
            self.widths,
        ]) {
            *total += range.len as usize;
        }
    }
}
#[derive(Default)]
struct TokenArenas {
    runs: TokenArena<InlineTokenMetrics>,
    summaries: TokenArena<InlineSummaryBlock>,
    replaced: TokenArena<ReplacedToken>,
    metadata: TokenArena<u64>,
    widths: TokenArena<f32>,
}
impl TokenArenas {
    fn store(
        &mut self,
        previous: PlanRanges,
        tokens: &InlineTokens<'_>,
        changed_only: bool,
    ) -> PlanRanges {
        PlanRanges {
            runs: self.runs.store(previous.runs, &tokens.runs, changed_only),
            summaries: self.summaries.store(
                previous.summaries,
                &tokens.summary_blocks,
                changed_only,
            ),
            replaced: self
                .replaced
                .store(previous.replaced, &tokens.replaced, changed_only),
            metadata: self
                .metadata
                .store(previous.metadata, &tokens.data.metadata, changed_only),
            widths: self
                .widths
                .store(previous.widths, &tokens.data.widths, changed_only),
            features: tokens.features,
        }
    }
    fn view(&self, ranges: PlanRanges) -> InlineTokens<'_> {
        InlineTokens {
            data: TokenData {
                metadata: self.metadata.view(ranges.metadata),
                widths: self.widths.view(ranges.widths),
            },
            runs: self.runs.view(ranges.runs),
            summary_blocks: self.summaries.view(ranges.summaries),
            replaced: self.replaced.view(ranges.replaced),
            features: ranges.features,
        }
    }
}

struct PreparedInlinePlan {
    key: PreparedInlinePlanKey,
    ranges: PlanRanges,
    composition: CompositionState,
    available: bool,
}

#[derive(Default)]
struct GenerationPlans {
    generation: Option<InlinePlanGeneration>,
    lookup: FxHashMap<u32, usize>,
    entries: Vec<PreparedInlinePlan>,
    arenas: TokenArenas,
    needs_compaction: bool,
    initial_token_capacity: u32,
}

impl GenerationPlans {
    fn insert(
        &mut self,
        key: PreparedInlinePlanKey,
        tokens: &InlineTokens,
        composition: CompositionState,
    ) {
        // Reserve the shared columns when the first nonempty plan is published.
        if self.initial_token_capacity != 0 && !tokens.data.metadata.is_empty() {
            // Keep the existing growth quantum: synthetic hyphen tokens can
            // exceed the character hint and must not double an almost-full
            // reservation into a larger allocation than ordinary growth.
            let mut capacity = tokens.data.metadata.len();
            while capacity < self.initial_token_capacity as usize {
                capacity = capacity.saturating_mul(2);
            }
            if self.arenas.metadata.capacity() == 0 {
                self.arenas
                    .metadata
                    .reserve_exact(capacity.max(tokens.data.metadata.len()));
            }
            if self.arenas.widths.capacity() == 0 {
                self.arenas
                    .widths
                    .reserve_exact(capacity.max(tokens.data.widths.len()));
            }
            self.initial_token_capacity = 0;
        }
        let previous = self.lookup.get(&key.container_box_idx).copied();
        let ranges = previous
            .map(|index| self.entries[index].ranges)
            .unwrap_or_default();
        let stored = self.arenas.store(ranges, tokens, false);
        self.needs_compaction |= previous.is_some() && stored != ranges;
        let plan = PreparedInlinePlan {
            key,
            ranges: stored,
            composition,
            available: true,
        };
        if let Some(index) = previous {
            self.entries[index] = plan;
        } else {
            self.lookup
                .insert(key.container_box_idx, self.entries.len());
            self.entries.push(plan);
        }
    }

    fn trim(&mut self) {
        // Dropped/mismatched plans and replaced ranges must not accumulate
        // across policy changes or shaping refinements.
        if self.needs_compaction || self.entries.iter().any(|plan| !plan.available) {
            let mut arenas = TokenArenas::default();
            let mut totals = [0; 5];
            for plan in self.entries.iter().filter(|plan| plan.available) {
                plan.ranges.add_lengths_to(&mut totals);
            }
            arenas.runs.reserve_exact(totals[0]);
            arenas.summaries.reserve_exact(totals[1]);
            arenas.replaced.reserve_exact(totals[2]);
            arenas.metadata.reserve_exact(totals[3]);
            arenas.widths.reserve_exact(totals[4]);
            self.entries.retain_mut(|plan| {
                if !plan.available {
                    return false;
                }
                let tokens = self.arenas.view(plan.ranges);
                plan.ranges = arenas.store(PlanRanges::default(), &tokens, false);
                true
            });
            self.arenas = arenas;
            self.lookup.clear();
            for (index, plan) in self.entries.iter().enumerate() {
                self.lookup.insert(plan.key.container_box_idx, index);
            }
            self.needs_compaction = false;
        }
        // Keep spare capacity reusable across layouts.
        self.initial_token_capacity = 0;
    }
}

/// Retain at most two recent shaping generations for first-line refinement.
/// Working buffers are recycled separately from retained paragraph ranges.
#[derive(Default)]
pub(crate) struct InlinePlanCache {
    active: GenerationPlans,
    previous: GenerationPlans,
    working: Vec<InlineTokens<'static>>,
}

impl InlinePlanCache {
    pub(crate) fn set_token_capacity_hint(&mut self, capacity: usize) {
        if self.active.entries.is_empty() {
            self.active.initial_token_capacity = u32::try_from(capacity).unwrap_or(u32::MAX);
        }
    }

    pub(crate) fn begin_generation(&mut self, generation: InlinePlanGeneration) {
        if self.active.generation == Some(generation) {
            return;
        }
        swap(&mut self.active, &mut self.previous);
        if self.active.generation != Some(generation) {
            self.active = GenerationPlans {
                generation: Some(generation),
                ..Default::default()
            };
        }
    }

    pub(in crate::inline) fn working_tokens(&mut self, capacity: usize) -> InlineTokens<'static> {
        let mut tokens = self.working.pop().unwrap_or_default();
        tokens.data.reserve(capacity);
        tokens
            .summary_blocks
            .reserve(capacity.div_ceil(INLINE_SUMMARY_BLOCK_TOKENS));
        tokens
    }

    pub(super) fn is_empty(&self) -> bool {
        self.active.entries.is_empty()
            && self.previous.entries.is_empty()
            && self.working.is_empty()
    }

    pub(super) fn contains(&self, key: PreparedInlinePlanKey) -> bool {
        self.active
            .lookup
            .get(&key.container_box_idx)
            .is_some_and(|&index| {
                self.active.entries[index].available && self.active.entries[index].key == key
            })
    }

    /// Borrow a plan only for execution, then write back changed buffers.
    /// The result cannot retain references into the cache.
    pub(super) fn with_plan<R>(
        &mut self,
        key: PreparedInlinePlanKey,
        execute: impl FnOnce(&mut InlineTokens<'_>, &mut CompositionState) -> R,
    ) -> Option<R> {
        let index = *self.active.lookup.get(&key.container_box_idx)?;
        let plan = &mut self.active.entries[index];
        if !plan.available {
            return None;
        }
        plan.available = false;
        if plan.key != key {
            return None;
        }
        let (result, working) = {
            let mut tokens = self.active.arenas.view(plan.ranges);
            let result = execute(&mut tokens, &mut plan.composition);
            let working = InlineTokens {
                data: tokens.data.into_working(),
                runs: tokens.runs.into_working(),
                summary_blocks: tokens.summary_blocks.into_working(),
                replaced: tokens.replaced.into_working(),
                features: tokens.features,
            };
            (result, working)
        };
        let stored = self.active.arenas.store(plan.ranges, &working, true);
        self.active.needs_compaction |= stored != plan.ranges;
        plan.ranges = stored;
        plan.available = true;
        self.recycle(working);
        Some(result)
    }

    pub(super) fn insert(
        &mut self,
        key: PreparedInlinePlanKey,
        tokens: &InlineTokens<'_>,
        composition: CompositionState,
    ) {
        self.active.insert(key, tokens, composition);
    }

    pub(super) fn recycle(&mut self, mut tokens: InlineTokens<'static>) {
        tokens.data.clear();
        tokens.runs.clear();
        tokens.summary_blocks.clear();
        tokens.replaced.clear();
        tokens.features = PreparedTokenFeatures::default();
        if tokens.data.storage_bytes()
            + tokens.runs.capacity()
            + tokens.summary_blocks.capacity()
            + tokens.replaced.capacity()
            > 0
        {
            self.working.push(tokens);
        }
    }

    pub(crate) fn trim(&mut self) {
        self.active.trim();
        self.previous.trim();
        // Reuse working buffers between paragraphs, then release them at the
        // pass boundary so a single large/uncached paragraph is not retained
        // in addition to the retained arenas.
        self.working.clear();
    }

    pub(crate) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        for cache in [&self.active, &self.previous] {
            let available = cache.entries.iter().filter(|plan| plan.available);
            report.add_slice_storage::<PreparedInlinePlan>(
                "entries",
                cache.entries.capacity(),
                available.clone().count(),
            );
            report.add_hash_map_storage("entry_lookup", &cache.lookup);
            let mut counts = [0; 5];
            for plan in available {
                plan.ranges.add_lengths_to(&mut counts);
                if let Some(plan) = &plan.composition.kp_plan {
                    report.add("line_break_plans", plan.memory_usage_bytes(), 1);
                }
                if let Some(cache) = &plan.composition.deferred_hyphens {
                    report.add("deferred_hyphenation", cache.memory_usage_bytes(), 1);
                }
            }
            report.add_slice_storage::<InlineTokenMetrics>(
                "run_metrics",
                cache.arenas.runs.capacity(),
                counts[0],
            );
            report.add_slice_storage::<InlineSummaryBlock>(
                "summaries",
                cache.arenas.summaries.capacity(),
                counts[1],
            );
            report.add_slice_storage::<ReplacedToken>(
                "replaced",
                cache.arenas.replaced.capacity(),
                counts[2],
            );
            report.add_slice_storage::<u64>(
                "token_metadata",
                cache.arenas.metadata.capacity(),
                counts[3],
            );
            report.add(
                "token_widths",
                cache.arenas.widths.capacity() * size_of::<f32>(),
                counts[4],
            );
        }
        report.add_slice_storage::<InlineTokens<'static>>(
            "working_buffers",
            self.working.capacity(),
            self.working.len(),
        );
        for tokens in &self.working {
            report.add_slice_storage::<InlineTokenMetrics>(
                "working_run_metrics",
                tokens.runs.capacity(),
                0,
            );
            report.add_slice_storage::<InlineSummaryBlock>(
                "working_summaries",
                tokens.summary_blocks.capacity(),
                0,
            );
            report.add_slice_storage::<ReplacedToken>(
                "working_replaced",
                tokens.replaced.capacity(),
                0,
            );
            report.add_slice_storage::<u64>("working_metadata", tokens.data.metadata.capacity(), 0);
            report.add_slice_storage::<f32>("working_widths", tokens.data.widths.capacity(), 0);
        }
        report
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn len(&self) -> usize {
        self.active
            .entries
            .iter()
            .chain(&self.previous.entries)
            .filter(|plan| plan.available)
            .count()
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn first_token_pointer(&self) -> Option<usize> {
        self.active
            .entries
            .iter()
            .find(|plan| plan.available)
            .map(|plan| {
                self.active
                    .arenas
                    .metadata
                    .view(plan.ranges.metadata)
                    .as_ptr() as usize
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocation_test_support::count_allocations;
    use crate::inline::tokens::{BreakKind, InlineToken, InlineTokenKind, InlineVerticalAlign};
    use html_engine_model::TextCompositionPolicy;
    use html_style_model::WhiteSpace;

    fn key(owner: u32, book: bool) -> PreparedInlinePlanKey {
        PreparedInlinePlanKey {
            run_start: 0,
            run_end: 1,
            container_box_idx: owner,
            composition: if book {
                TextCompositionPolicy::BookOptimizedUnrestrictedHyphenation
            } else {
                TextCompositionPolicy::WebCompatible
            },
        }
    }

    fn fill(tokens: &mut InlineTokens, owner: u32, count: usize) {
        let metrics = InlineTokenMetrics {
            owner_box_idx: owner,
            ascent: 8.0,
            descent: 2.0,
            line_height: 12.0,
            font_size: 16.0,
            vertical_align: InlineVerticalAlign::Baseline,
            white_space: WhiteSpace::Normal,
            contributes_line_metrics: true,
            visible: true,
            placement_required: false,
        };
        for index in 0..count {
            tokens.push(
                InlineToken::new(
                    InlineTokenKind::Glyph {
                        glyph_idx: owner * 100 + index as u32,
                    },
                    8.0 + f64::from(owner),
                    BreakKind::None,
                    false,
                    true,
                ),
                metrics,
            );
        }
        tokens.rebuild_summary_blocks();
    }

    fn insert(cache: &mut InlinePlanCache, owner: u32, count: usize) {
        let mut tokens = cache.working_tokens(count);
        fill(&mut tokens, owner, count);
        cache.insert(key(owner, false), &tokens, CompositionState::default());
        cache.recycle(tokens);
    }

    #[test]
    fn borrowed_ranges_preserve_local_indices_and_reject_other_policies() {
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 1, 20);
        insert(&mut cache, 2, 3);
        cache.trim();
        let pointer = cache.first_token_pointer();
        cache
            .with_plan(key(1, false), |first, _composition| {
                assert_eq!(first.span().token_at(0).run_idx(), 0);
                assert_eq!(first.runs[0].owner_box_idx, 1);
                assert_eq!(first.summary_blocks.len(), 2);
                assert!(matches!(
                    first.span().token_at(0).kind(),
                    InlineTokenKind::Glyph { glyph_idx: 100 }
                ));
            })
            .unwrap();
        cache
            .with_plan(key(2, false), |second, _composition| {
                assert_eq!(second.span().token_at(0).run_idx(), 0);
                assert_eq!(second.runs[0].owner_box_idx, 2);
                assert_eq!(second.summary_blocks.len(), 1);
                assert!(matches!(
                    second.span().token_at(0).kind(),
                    InlineTokenKind::Glyph { glyph_idx: 200 }
                ));
            })
            .unwrap();
        cache.trim();
        assert_eq!(
            cache.first_token_pointer(),
            pointer,
            "warm replay preserves the retained arena"
        );
        assert!(
            cache
                .with_plan(key(1, true), |_, _composition| ())
                .is_none()
        );
        cache.trim();
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.active.arenas.metadata.len(),
            3,
            "rejected plans release their ranges"
        );
        cache
            .with_plan(key(2, false), |remaining, _composition| {
                assert_eq!(remaining.runs[0].owner_box_idx, 2);
                assert_eq!(remaining.data.len(), 3);
            })
            .unwrap();
    }

    #[test]
    fn changing_sizes_and_policies_compacts_without_accumulating_dead_ranges() {
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 1, 7);
        insert(&mut cache, 2, 9);
        for count in [30, 2, 19, 1] {
            cache
                .with_plan(key(1, false), |tokens, _composition| {
                    tokens.data.clear();
                    tokens.runs.clear();
                    fill(tokens, 1, count);
                })
                .unwrap();
            cache.trim();
            assert_eq!(cache.active.arenas.metadata.len(), count + 9);
            assert_eq!(cache.active.arenas.runs.len(), 2);
            cache
                .with_plan(key(2, false), |second, _composition| {
                    assert_eq!(second.data.len(), 9);
                    assert_eq!(second.runs[0].owner_box_idx, 2);
                })
                .unwrap();
        }
    }

    #[test]
    fn construction_uses_packed_growth_and_warm_replay_does_not_allocate() {
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(InlinePlanGeneration::new());
        let (_, allocations) = count_allocations(|| {
            for owner in 0..256 {
                insert(&mut cache, owner, 8);
            }
            cache.trim();
        });
        assert!(
            allocations < 150,
            "256 plans should grow packed buffers instead of allocating per paragraph: {allocations}"
        );
        assert_eq!(cache.active.arenas.metadata.len(), 256 * 8);
        let (_, allocations) = count_allocations(|| {
            for owner in 0..256 {
                cache
                    .with_plan(key(owner, false), |_, _composition| ())
                    .unwrap();
            }
            cache.trim();
        });
        assert_eq!(
            allocations, 0,
            "borrowed replay must not allocate working copies"
        );
    }

    #[test]
    fn uncached_and_recursive_working_buffers_are_recycled() {
        let mut cache = InlinePlanCache::default();
        let mut outer = cache.working_tokens(100);
        fill(&mut outer, 1, 100);
        let mut inner = cache.working_tokens(20);
        fill(&mut inner, 2, 20);
        cache.recycle(inner);
        assert_eq!(outer.data.len(), 100);
        cache.recycle(outer);
        let (_, allocations) = count_allocations(|| {
            let first = cache.working_tokens(100);
            let second = cache.working_tokens(20);
            assert!(first.is_empty() && second.is_empty());
            assert!(first.runs.is_empty() && second.runs.is_empty());
            cache.recycle(second);
            cache.recycle(first);
        });
        assert_eq!(allocations, 0);
        assert_eq!(cache.working.len(), 2);
    }

    #[test]
    fn payloads_and_breakpoint_plans_survive_generation_rotation() {
        let generation = InlinePlanGeneration::new();
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(generation);
        for owner in [1, 2] {
            let mut tokens = cache.working_tokens(4);
            fill(&mut tokens, owner, 3);
            tokens.replaced.push(ReplacedToken::InlineBoundary {
                box_idx: owner,
                margin_left: f64::from(owner),
                left_inset: 4.0,
                inline_start: true,
                inline_end: false,
            });
            let metrics = tokens.runs[0];
            tokens.push(
                InlineToken::new(
                    InlineTokenKind::InlineBoundary { payload_idx: 0 },
                    0.0,
                    BreakKind::None,
                    false,
                    true,
                ),
                metrics,
            );
            tokens.rebuild_summary_blocks();
            let composition = CompositionState {
                kp_plan: Some(KpPlan::build(tokens.span(), &tokens.runs, false)),
                ..CompositionState::default()
            };
            cache.insert(key(owner, false), &tokens, composition);
            cache.recycle(tokens);
        }
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 3, 10);
        cache.begin_generation(generation);
        cache
            .with_plan(key(2, false), |tokens, _composition| {
                assert!(
                    _composition.kp_plan.is_some(),
                    "line-break plans stay with their paragraph"
                );
                assert!(matches!(
                    tokens.span().token_at(3).kind(),
                    InlineTokenKind::InlineBoundary { payload_idx: 0 }
                ));
                assert!(matches!(
                    tokens.replaced[0],
                    ReplacedToken::InlineBoundary { box_idx: 2, .. }
                ));
                assert_eq!(tokens.runs[0].owner_box_idx, 2);
            })
            .unwrap();
        cache.trim();
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn pass_boundary_releases_large_uncached_working_buffers() {
        let mut cache = InlinePlanCache::default();
        let mut tokens = cache.working_tokens(100_000);
        fill(&mut tokens, 1, 100_000);
        cache.recycle(tokens);
        assert!(cache.working[0].data.metadata.capacity() >= 100_000);
        cache.trim();
        assert_eq!(cache.working.capacity(), 0);
        assert!(cache.active.arenas.metadata.is_empty());
        assert_eq!(cache.memory_usage_report().total_bytes(), 0);
    }

    #[test]
    fn cached_paragraphs_borrow_arena_storage_without_allocations_or_working_copies() {
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 1, 20);
        insert(&mut cache, 2, 30);
        cache.trim();
        let pointer = cache.first_token_pointer().unwrap();
        let (_, allocations) = count_allocations(|| {
            for _ in 0..100 {
                cache
                    .with_plan(key(1, false), |tokens, _composition| {
                        assert_eq!(tokens.data.metadata.as_ptr() as usize, pointer);
                        assert!(tokens.data.metadata.is_borrowed());
                        assert!(tokens.runs.is_borrowed());
                        assert!(tokens.summary_blocks.is_borrowed());
                        assert_eq!(tokens.span().token_at(0).width(), 9.0);
                    })
                    .unwrap();
            }
        });
        assert_eq!(
            allocations, 0,
            "even the first hit after a pass boundary borrows directly"
        );
        assert!(cache.working.is_empty());
    }

    #[test]
    fn lazy_line_break_plans_survive_borrowed_execution() {
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 1, 256);
        cache.trim();
        cache
            .with_plan(key(1, false), |tokens, _composition| {
                assert!(_composition.kp_plan.is_none());
                _composition.prepare_kp_plan(tokens, false);
            })
            .unwrap();
        let (_, allocations) = count_allocations(|| {
            for _ in 0..100 {
                cache
                    .with_plan(key(1, false), |tokens, _composition| {
                        assert!(_composition.kp_plan.is_some());
                        _composition.prepare_kp_plan(tokens, false);
                    })
                    .unwrap();
            }
        });
        assert_eq!(
            allocations, 0,
            "line-break plans are retained independently of borrowed tokens"
        );
    }

    #[test]
    fn metrics_changes_do_not_copy_tokens_or_change_other_paragraphs() {
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 1, 20);
        insert(&mut cache, 2, 30);
        cache.trim();
        let pointer = cache.first_token_pointer().unwrap();
        cache
            .with_plan(key(1, false), |tokens, _composition| {
                tokens.runs[0].ascent = 17.0;
                assert!(!tokens.runs.is_borrowed());
                assert!(tokens.data.metadata.is_borrowed());
                assert!(tokens.summary_blocks.is_borrowed());
            })
            .unwrap();
        cache
            .with_plan(key(1, false), |tokens, _composition| {
                assert_eq!(tokens.data.metadata.as_ptr() as usize, pointer);
                assert_eq!(tokens.runs[0].ascent, 17.0);
            })
            .unwrap();
        cache
            .with_plan(key(2, false), |other, _composition| {
                assert_eq!(other.runs[0].ascent, 8.0)
            })
            .unwrap();
    }

    #[test]
    fn emptying_a_cached_paragraph_preserves_its_neighbor() {
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 1, 20);
        insert(&mut cache, 2, 3);
        cache.trim();
        cache
            .with_plan(key(1, false), |tokens, _composition| {
                tokens.data.clear();
                tokens.runs.clear();
                tokens.summary_blocks.clear();
                tokens.replaced.clear();
            })
            .unwrap();
        cache.trim();
        cache
            .with_plan(key(1, false), |empty, _composition| {
                assert!(empty.data.is_empty());
                assert!(empty.runs.is_empty());
                assert!(empty.summary_blocks.is_empty());
                assert!(empty.replaced.is_empty());
            })
            .unwrap();
        cache
            .with_plan(key(2, false), |neighbor, _composition| {
                assert_eq!(neighbor.data.len(), 3);
                assert_eq!(neighbor.span().token_at(0).width(), 10.0);
                assert_eq!(neighbor.runs[0].owner_box_idx, 2);
            })
            .unwrap();
        assert_eq!(cache.active.arenas.metadata.len(), 3);
        assert_eq!(cache.active.arenas.runs.len(), 1);
        assert_eq!(
            cache.len(),
            2,
            "an empty cached paragraph remains available"
        );
    }

    #[test]
    fn updates_and_growth_follow_the_end_of_a_borrowed_view() {
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 1, 20);
        insert(&mut cache, 2, 30);
        cache.trim();
        cache
            .with_plan(key(1, false), |outer, _composition| {
                let pointer = outer.data.metadata.as_ptr();
                assert_eq!(outer.span().token_at(0).width(), 9.0);
                assert_eq!(outer.data.metadata.as_ptr(), pointer);
            })
            .unwrap();
        cache
            .with_plan(key(2, false), |inner, _composition| {
                inner.data.widths.make_mut()[0] = 123.0
            })
            .unwrap();
        insert(&mut cache, 3, 1000);
        cache.trim();
        cache
            .with_plan(key(2, false), |inner, _composition| {
                assert_eq!(inner.span().token_at(0).width(), 123.0);
                assert_eq!(inner.runs[0].owner_box_idx, 2);
            })
            .unwrap();
        cache
            .with_plan(key(1, false), |outer, _composition| {
                assert_eq!(outer.span().token_at(0).width(), 9.0)
            })
            .unwrap();
        assert_eq!(cache.active.arenas.metadata.len(), 1050);
    }
}
