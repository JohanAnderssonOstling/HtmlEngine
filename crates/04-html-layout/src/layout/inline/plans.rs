//! Owned retained token arenas. Cache hits borrow scoped slices; only
//! construction and changed buffers need mutable working storage.

use super::storage::{ArenaRange, TokenArena};
use super::tokens::{
    INLINE_SUMMARY_BLOCK_TOKENS, InlineSummaryBlock, InlineToken, InlineTokenMetrics, InlineTokens,
    PreparedInlinePlanKey, ReplacedToken,
};
use super::wrapping::KpPlan;
use html_dom::MemoryUsageReport;
use rustc_data_structures::fx::FxHashMap;
use std::mem::{size_of, swap};
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
    dense: ArenaRange,
    runs: ArenaRange,
    summaries: ArenaRange,
    replaced: ArenaRange,
}

#[derive(Default)]
struct TokenArenas {
    dense: TokenArena<InlineToken>,
    runs: TokenArena<InlineTokenMetrics>,
    summaries: TokenArena<InlineSummaryBlock>,
    replaced: TokenArena<ReplacedToken>,
}

impl TokenArenas {
    fn store(&mut self, previous: PlanRanges, tokens: &InlineTokens<'_>) -> PlanRanges {
        self.store_changes(previous, tokens, [true; 4])
    }

    fn store_changes(
        &mut self,
        previous: PlanRanges,
        tokens: &InlineTokens<'_>,
        changed: [bool; 4],
    ) -> PlanRanges {
        PlanRanges {
            dense: if changed[0] {
                self.dense.store(previous.dense, &tokens.dense)
            } else {
                previous.dense
            },
            runs: if changed[1] {
                self.runs.store(previous.runs, &tokens.runs)
            } else {
                previous.runs
            },
            summaries: if changed[2] {
                self.summaries
                    .store(previous.summaries, &tokens.summary_blocks)
            } else {
                previous.summaries
            },
            replaced: if changed[3] {
                self.replaced.store(previous.replaced, &tokens.replaced)
            } else {
                previous.replaced
            },
        }
    }

    fn view(&self, ranges: PlanRanges) -> InlineTokens<'_> {
        InlineTokens {
            dense: self.dense.view(ranges.dense),
            runs: self.runs.view(ranges.runs),
            summary_blocks: self.summaries.view(ranges.summaries),
            replaced: self.replaced.view(ranges.replaced),
            kp_plan: None,
        }
    }

    fn shrink_to_fit(&mut self) {
        self.dense.shrink_to_fit();
        self.runs.shrink_to_fit();
        self.summaries.shrink_to_fit();
        self.replaced.shrink_to_fit();
    }
}

struct PreparedInlinePlan {
    key: PreparedInlinePlanKey,
    ranges: PlanRanges,
    kp_plan: Option<KpPlan>,
    available: bool,
}

#[derive(Default)]
struct GenerationPlans {
    generation: Option<InlinePlanGeneration>,
    lookup: FxHashMap<u32, usize>,
    entries: Vec<PreparedInlinePlan>,
    arenas: TokenArenas,
    needs_compaction: bool,
}

impl GenerationPlans {
    fn insert(&mut self, key: PreparedInlinePlanKey, tokens: &mut InlineTokens) {
        let previous = self.lookup.get(&key.container_box_idx).copied();
        let ranges = previous
            .map(|index| self.entries[index].ranges)
            .unwrap_or_default();
        let stored = self.arenas.store(ranges, tokens);
        self.needs_compaction |= previous.is_some() && stored != ranges;
        let plan = PreparedInlinePlan {
            key,
            ranges: stored,
            kp_plan: tokens.kp_plan.take(),
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
            let mut totals = [0; 4];
            for plan in self.entries.iter().filter(|plan| plan.available) {
                for (total, range) in totals.iter_mut().zip([
                    plan.ranges.dense,
                    plan.ranges.runs,
                    plan.ranges.summaries,
                    plan.ranges.replaced,
                ]) {
                    *total += range.len as usize;
                }
            }
            arenas.dense.reserve_exact(totals[0]);
            arenas.runs.reserve_exact(totals[1]);
            arenas.summaries.reserve_exact(totals[2]);
            arenas.replaced.reserve_exact(totals[3]);
            self.entries.retain_mut(|plan| {
                if !plan.available {
                    return false;
                }
                let tokens = self.arenas.view(plan.ranges);
                plan.ranges = arenas.store(PlanRanges::default(), &tokens);
                true
            });
            self.arenas = arenas;
            self.lookup.clear();
            for (index, plan) in self.entries.iter().enumerate() {
                self.lookup.insert(plan.key.container_box_idx, index);
            }
            self.needs_compaction = false;
        }
        self.arenas.shrink_to_fit();
        self.entries.shrink_to_fit();
    }
}

/// Retain at most two recent shaping generations for first-line refinement.
/// Working buffers are recycled separately from retained paragraph ranges.
#[derive(Default)]
pub(in crate::layout) struct InlinePlanCache {
    active: GenerationPlans,
    previous: GenerationPlans,
    working: Vec<InlineTokens<'static>>,
}

impl InlinePlanCache {
    pub(in crate::layout) fn begin_generation(&mut self, generation: InlinePlanGeneration) {
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

    pub(super) fn working_tokens(&mut self, capacity: usize) -> InlineTokens<'static> {
        let mut tokens = self.working.pop().unwrap_or_default();
        tokens.dense.reserve(capacity);
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
        execute: impl FnOnce(&mut InlineTokens<'_>) -> R,
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
        let (result, changed, mut working) = {
            let mut tokens = self.active.arenas.view(plan.ranges);
            tokens.kp_plan = plan.kp_plan.take();
            let result = execute(&mut tokens);
            let dense = tokens.dense.into_owned();
            let runs = tokens.runs.into_owned();
            let summaries = tokens.summary_blocks.into_owned();
            let replaced = tokens.replaced.into_owned();
            let changed = [
                dense.is_some(),
                runs.is_some(),
                summaries.is_some(),
                replaced.is_some(),
            ];
            let working = InlineTokens {
                dense: dense.unwrap_or_default().into(),
                runs: runs.unwrap_or_default().into(),
                summary_blocks: summaries.unwrap_or_default().into(),
                replaced: replaced.unwrap_or_default().into(),
                kp_plan: tokens.kp_plan,
            };
            (result, changed, working)
        };
        let stored = self
            .active
            .arenas
            .store_changes(plan.ranges, &working, changed);
        self.active.needs_compaction |= stored != plan.ranges;
        plan.ranges = stored;
        plan.kp_plan = working.kp_plan.take();
        plan.available = true;
        self.restore(None, working);
        Some(result)
    }

    pub(super) fn restore(
        &mut self,
        key: Option<PreparedInlinePlanKey>,
        mut tokens: InlineTokens<'static>,
    ) {
        if let Some(key) = key {
            self.active.insert(key, &mut tokens);
        }
        tokens.dense.clear();
        tokens.runs.clear();
        tokens.summary_blocks.clear();
        tokens.replaced.clear();
        tokens.kp_plan = None;
        if tokens.dense.capacity()
            + tokens.runs.capacity()
            + tokens.summary_blocks.capacity()
            + tokens.replaced.capacity()
            > 0
        {
            self.working.push(tokens);
        }
    }

    pub(in crate::layout) fn trim(&mut self) {
        self.active.trim();
        self.previous.trim();
        // Reuse working buffers between paragraphs, then release them at the
        // pass boundary so a single large/uncached paragraph is not retained
        // in addition to the retained arenas.
        self.working.clear();
        self.working.shrink_to_fit();
    }

    pub(in crate::layout) fn merge(&mut self, mut worker: Self) {
        for plans in [worker.active, worker.previous] {
            let destination = if plans.generation == self.active.generation {
                &mut self.active
            } else if plans.generation == self.previous.generation {
                &mut self.previous
            } else {
                continue;
            };
            if destination.entries.is_empty() {
                *destination = plans;
                continue;
            }
            for mut plan in plans.entries.into_iter().filter(|plan| plan.available) {
                let mut tokens = plans.arenas.view(plan.ranges);
                tokens.kp_plan = plan.kp_plan.take();
                destination.insert(plan.key, &mut tokens);
            }
        }
        // Worker count must not multiply idle working-buffer retention. Keep
        // the largest useful buffer at each executor's existing nesting depth.
        for tokens in worker.working.drain(..) {
            if let Some(smallest) = self
                .working
                .iter_mut()
                .min_by_key(|tokens| tokens.dense.capacity())
            {
                if tokens.dense.capacity() > smallest.dense.capacity() {
                    *smallest = tokens;
                }
            } else {
                self.working.push(tokens);
            }
        }
    }

    pub(in crate::layout) fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        for cache in [&self.active, &self.previous] {
            let available = cache.entries.iter().filter(|plan| plan.available);
            report.add_slice_storage::<PreparedInlinePlan>(
                "entries",
                cache.entries.capacity(),
                available.clone().count(),
            );
            report.add(
                "entry_lookup",
                cache.lookup.capacity() * size_of::<(u32, usize)>(),
                cache.lookup.len(),
            );
            let mut counts = [0; 4];
            for plan in available {
                for (count, range) in counts.iter_mut().zip([
                    plan.ranges.dense,
                    plan.ranges.runs,
                    plan.ranges.summaries,
                    plan.ranges.replaced,
                ]) {
                    *count += range.len as usize;
                }
                if let Some(plan) = &plan.kp_plan {
                    report.add("line_break_plans", plan.memory_usage_bytes(), 1);
                }
            }
            report.add_slice_storage::<InlineToken>(
                "tokens",
                cache.arenas.dense.capacity(),
                counts[0],
            );
            report.add_slice_storage::<InlineTokenMetrics>(
                "run_metrics",
                cache.arenas.runs.capacity(),
                counts[1],
            );
            report.add_slice_storage::<InlineSummaryBlock>(
                "summaries",
                cache.arenas.summaries.capacity(),
                counts[2],
            );
            report.add_slice_storage::<ReplacedToken>(
                "replaced",
                cache.arenas.replaced.capacity(),
                counts[3],
            );
        }
        report.add_slice_storage::<InlineTokens<'static>>(
            "working_buffers",
            self.working.capacity(),
            self.working.len(),
        );
        for tokens in &self.working {
            report.add_slice_storage::<InlineToken>("working_tokens", tokens.dense.capacity(), 0);
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
        }
        report
    }

    #[cfg(any(test, feature = "testing"))]
    pub(in crate::layout) fn len(&self) -> usize {
        self.active
            .entries
            .iter()
            .chain(&self.previous.entries)
            .filter(|plan| plan.available)
            .count()
    }

    #[cfg(any(test, feature = "testing"))]
    pub(in crate::layout) fn first_token_pointer(&self) -> Option<usize> {
        self.active
            .entries
            .iter()
            .find(|plan| plan.available)
            .map(|plan| self.active.arenas.dense.view(plan.ranges.dense).as_ptr() as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tokens::{BreakKind, InlineTokenKind, InlineVerticalAlign, TokenWrap};
    use super::*;
    use crate::allocation_test_support::count_allocations;
    use html_style_model::WhiteSpace;

    fn key(owner: u32, book: bool) -> PreparedInlinePlanKey {
        PreparedInlinePlanKey {
            run_start: 0,
            run_end: 1,
            container_box_idx: owner,
            high_quality_hyphenation: false,
            book_optimized: book,
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
                    TokenWrap::Normal,
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
        cache.restore(Some(key(owner, false)), tokens);
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
            .with_plan(key(1, false), |first| {
                assert_eq!(first.dense[0].run_idx(), 0);
                assert_eq!(first.runs[0].owner_box_idx, 1);
                assert_eq!(first.summary_blocks.len(), 2);
                assert!(matches!(
                    first.dense[0].kind(),
                    InlineTokenKind::Glyph { glyph_idx: 100 }
                ));
            })
            .unwrap();
        cache
            .with_plan(key(2, false), |second| {
                assert_eq!(second.dense[0].run_idx(), 0);
                assert_eq!(second.runs[0].owner_box_idx, 2);
                assert_eq!(second.summary_blocks.len(), 1);
                assert!(matches!(
                    second.dense[0].kind(),
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
        assert!(cache.with_plan(key(1, true), |_| ()).is_none());
        cache.trim();
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.active.arenas.dense.len(),
            3,
            "rejected plans release their ranges"
        );
        cache
            .with_plan(key(2, false), |remaining| {
                assert_eq!(remaining.runs[0].owner_box_idx, 2);
                assert_eq!(remaining.dense.len(), 3);
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
                .with_plan(key(1, false), |tokens| {
                    tokens.dense.clear();
                    tokens.runs.clear();
                    fill(tokens, 1, count);
                })
                .unwrap();
            cache.trim();
            assert_eq!(cache.active.arenas.dense.len(), count + 9);
            assert_eq!(cache.active.arenas.runs.len(), 2);
            cache
                .with_plan(key(2, false), |second| {
                    assert_eq!(second.dense.len(), 9);
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
        assert_eq!(cache.active.arenas.dense.len(), 256 * 8);
        let (_, allocations) = count_allocations(|| {
            for owner in 0..256 {
                cache.with_plan(key(owner, false), |_| ()).unwrap();
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
        cache.restore(None, inner);
        assert_eq!(outer.dense.len(), 100);
        cache.restore(None, outer);
        let (_, allocations) = count_allocations(|| {
            let first = cache.working_tokens(100);
            let second = cache.working_tokens(20);
            assert!(first.is_empty() && second.is_empty());
            assert!(first.runs.is_empty() && second.runs.is_empty());
            cache.restore(None, second);
            cache.restore(None, first);
        });
        assert_eq!(allocations, 0);
        assert_eq!(cache.working.len(), 2);
    }

    #[test]
    fn worker_transfer_rebases_ranges_and_generations_remain_bounded() {
        let generation = InlinePlanGeneration::new();
        let mut parent = InlinePlanCache::default();
        parent.begin_generation(generation);
        insert(&mut parent, 1, 7);
        let mut worker = InlinePlanCache::default();
        worker.begin_generation(generation);
        insert(&mut worker, 2, 11);
        parent.merge(worker);
        assert_eq!(parent.len(), 2);
        parent
            .with_plan(key(2, false), |tokens| {
                assert_eq!(tokens.dense.len(), 11);
                assert_eq!(tokens.runs[0].owner_box_idx, 2);
                assert_eq!(tokens.dense[0].run_idx(), 0);
            })
            .unwrap();
        assert!(
            parent.working.len() <= 1,
            "workers must not multiply idle working buffers"
        );
        parent.begin_generation(InlinePlanGeneration::new());
        insert(&mut parent, 3, 5);
        parent.begin_generation(InlinePlanGeneration::new());
        insert(&mut parent, 4, 9);
        assert_eq!(parent.len(), 2, "only two shaping generations retain plans");
        parent.begin_generation(generation);
        assert!(parent.with_plan(key(1, false), |_| ()).is_none());
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
                    TokenWrap::Normal,
                    true,
                ),
                metrics,
            );
            tokens.rebuild_summary_blocks();
            tokens.kp_plan = Some(KpPlan::build(&tokens.dense, &tokens.runs, false));
            cache.restore(Some(key(owner, false)), tokens);
        }
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 3, 10);
        cache.begin_generation(generation);
        cache
            .with_plan(key(2, false), |tokens| {
                assert!(
                    tokens.kp_plan.is_some(),
                    "line-break plans stay with their paragraph"
                );
                assert!(matches!(
                    tokens.dense[3].kind(),
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
        cache.restore(None, tokens);
        assert!(cache.working[0].dense.capacity() >= 100_000);
        cache.trim();
        assert_eq!(cache.working.capacity(), 0);
        assert!(cache.active.arenas.dense.is_empty());
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
                    .with_plan(key(1, false), |tokens| {
                        assert_eq!(tokens.dense.as_ptr() as usize, pointer);
                        assert!(tokens.dense.is_borrowed());
                        assert!(tokens.runs.is_borrowed());
                        assert!(tokens.summary_blocks.is_borrowed());
                        assert_eq!(tokens.dense[0].width(), 9.0);
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
            .with_plan(key(1, false), |tokens| {
                assert!(tokens.kp_plan.is_none());
                tokens.prepare_kp_plan(false);
            })
            .unwrap();
        let (_, allocations) = count_allocations(|| {
            for _ in 0..100 {
                cache
                    .with_plan(key(1, false), |tokens| {
                        assert!(tokens.kp_plan.is_some());
                        tokens.prepare_kp_plan(false);
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
            .with_plan(key(1, false), |tokens| {
                tokens.runs[0].ascent = 17.0;
                assert!(!tokens.runs.is_borrowed());
                assert!(tokens.dense.is_borrowed());
                assert!(tokens.summary_blocks.is_borrowed());
            })
            .unwrap();
        cache
            .with_plan(key(1, false), |tokens| {
                assert_eq!(tokens.dense.as_ptr() as usize, pointer);
                assert_eq!(tokens.runs[0].ascent, 17.0);
            })
            .unwrap();
        cache
            .with_plan(key(2, false), |other| assert_eq!(other.runs[0].ascent, 8.0))
            .unwrap();
    }

    #[test]
    fn updates_and_growth_follow_the_end_of_a_borrowed_view() {
        let mut cache = InlinePlanCache::default();
        cache.begin_generation(InlinePlanGeneration::new());
        insert(&mut cache, 1, 20);
        insert(&mut cache, 2, 30);
        cache.trim();
        cache
            .with_plan(key(1, false), |outer| {
                let pointer = outer.dense.as_ptr();
                assert_eq!(outer.dense[0].width(), 9.0);
                assert_eq!(outer.dense.as_ptr(), pointer);
            })
            .unwrap();
        cache
            .with_plan(key(2, false), |inner| inner.dense[0].width = 123.0)
            .unwrap();
        insert(&mut cache, 3, 1000);
        cache.trim();
        cache
            .with_plan(key(2, false), |inner| {
                assert_eq!(inner.dense[0].width(), 123.0);
                assert_eq!(inner.runs[0].owner_box_idx, 2);
            })
            .unwrap();
        cache
            .with_plan(key(1, false), |outer| {
                assert_eq!(outer.dense[0].width(), 9.0)
            })
            .unwrap();
        assert_eq!(cache.active.arenas.dense.len(), 1050);
    }
}
