//! Compact Knuth–Plass optimization over cumulative token breakpoints.
//!
//! The solver is independent of glyphs, styles, and the layout tree. Inline
//! token construction supplies compact cumulative breakpoints, and the solver
//! returns their selected indices.

use std::cmp::Ordering;
use super::super::justification::tracking_width_limit;
use std::mem::size_of;
use rustc_data_structures::fx::FxHashMap;
#[cfg(feature = "profiling")]
use crate::profiling::{measurements_enabled, record_measurement, scope};
#[cfg(feature = "profiling")]
use std::time::Instant;

/// Penalties at or beyond this magnitude are never selected.
pub(super) const INF_PENALTY: f64 = 10_000.0;

/// Extra demerits charged when two consecutive lines differ sharply in
/// tightness (Knuth's adjacent-fitness penalty).
const FITNESS_PENALTY: f64 = 3000.0;
const FLAGGED_PENALTY: f64 = 100.0;
const PENULTIMATE_HYPHEN_PENALTY: f64 = 2_500.0;
const EXCESSIVE_HYPHEN_RUN_PENALTY: f64 = 100_000_000.0;
const MAX_CONSECUTIVE_HYPHENATED_LINES: u8 = 2;
const MIN_HYPHENATED_LINE_FILL: f64 = 0.5;
const SHORT_HYPHENATED_LINE_DEMERITS: f64 = 25_000_000.0;
const UNDERFULL_LINE_DEMERITS: f64 = 1_000_000.0;
// An emergency line is allowed only when the normal glue limits cannot make
// the paragraph fit.  A very short line must still be much more expensive
// than spreading a modest amount of slack over the remaining lines; otherwise
// the fallback can create one-word first lines in an otherwise ordinary
// paragraph.
const SEVERELY_UNDERFULL_LINE_DEMERITS: f64 = 100_000_000.0;
const TRACKING_LINE_DEMERITS: f64 = 25_000.0;

fn underfull_demerits(remaining: f64, available: f64) -> f64 {
    let fraction = remaining.max(0.0) / available.max(1.0);
    let severity = fraction.clamp(0.0, 1.0);
    UNDERFULL_LINE_DEMERITS
        + (fraction * 1000.0).powi(2)
        + SEVERELY_UNDERFULL_LINE_DEMERITS * severity.powi(4)
}

fn tracking_demerits(adjustment: f64, capacity: f64) -> f64 {
    let usage = if capacity > 0.0 {
        (adjustment.abs() / capacity).min(1.0)
    } else {
        0.0
    };
    TRACKING_LINE_DEMERITS * usage * usage
}

fn short_hyphenated_line_demerits(width: f64, available: f64) -> f64 {
    let fill = if available > 0.0 {
        (width / available).clamp(0.0, 1.0)
    } else {
        1.0
    };
    if fill >= MIN_HYPHENATED_LINE_FILL {
        0.0
    } else {
        let severity = (MIN_HYPHENATED_LINE_FILL - fill) / MIN_HYPHENATED_LINE_FILL;
        SHORT_HYPHENATED_LINE_DEMERITS * (1.0 + severity * severity)
    }
}

fn fitness_class(r: f64) -> i32 {
    if r < -0.5 {
        0
    } else if r <= 0.5 {
        1
    } else if r <= 1.0 {
        2
    } else {
        3
    }
}

/// One legal break in the compact word-oriented paragraph representation.
///
/// Prefix totals describe the line immediately before the breakable space.
/// `discard_*` is the following run of spaces, which becomes the next active
/// node's prefix. The final record has zero discard width and carries the
/// synthetic infinite stretch used to keep the last line ragged. Token targets
/// live here as `u32`s so inline layout needs no parallel item-indexed map.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct CompactBreakpoint {
    width: f64,
    stretch: f64,
    shrink: f64,
    discard_width: f64,
    discard_stretch: f32,
    discard_shrink: f32,
    line_end: u32,
    next_line_start: u32,
}

/// Cumulative geometry stays exact; repeated space/hyphen metrics and the
/// next-token delta are shared instead of occupying 16 bytes at every break.
pub(super) struct StoredBreakpoint {
    width: f64,
    stretch: f64,
    shrink: f64,
    line_end: u32,
    discard: u32,
}

pub(super) struct BreakpointDiscard {
    width: f64,
    stretch: f32,
    shrink: f32,
    next_token_delta: u32,
}

impl StoredBreakpoint {
    fn expand(&self, discards: &[BreakpointDiscard]) -> CompactBreakpoint {
        let discard = &discards[self.discard as usize];
        CompactBreakpoint {
            width: self.width,
            stretch: self.stretch,
            shrink: self.shrink,
            discard_width: discard.width,
            discard_stretch: discard.stretch,
            discard_shrink: discard.shrink,
            line_end: self.line_end,
            next_line_start: self.line_end.wrapping_add(discard.next_token_delta),
        }
    }
}

pub(super) enum BreakpointStorage {
    Direct(Box<[CompactBreakpoint]>),
    Shared {
        records: Box<[StoredBreakpoint]>,
        discards: Box<[BreakpointDiscard]>,
    },
}

impl BreakpointStorage {
    pub(super) fn new(values: Vec<CompactBreakpoint>) -> Self {
        let direct_bytes = values.len() * size_of::<CompactBreakpoint>();
        let record_bytes = values.len() * size_of::<StoredBreakpoint>();
        let extra_metadata = size_of::<Self>() - size_of::<Box<[CompactBreakpoint]>>();
        // Tiny segments cannot amortize a shared table and its metadata.
        if record_bytes + size_of::<BreakpointDiscard>() + extra_metadata >= direct_bytes {
            return Self::Direct(values.into_boxed_slice());
        }
        let mut records = Vec::with_capacity(values.len());
        let mut discards = Vec::new();
        let mut indices = FxHashMap::default();
        for value in &values {
            let delta = value.next_line_start.wrapping_sub(value.line_end);
            let key = (
                value.discard_width.to_bits(),
                value.discard_stretch.to_bits(),
                value.discard_shrink.to_bits(),
                delta,
            );
            let index = *indices.entry(key).or_insert_with(|| {
                let index = u32::try_from(discards.len()).expect("breakpoint discard table exceeds u32 capacity");
                discards.push(BreakpointDiscard {
                    width: value.discard_width,
                    stretch: value.discard_stretch,
                    shrink: value.discard_shrink,
                    next_token_delta: delta,
                });
                index
            });
            // Unusual paragraphs with many different spaces keep direct
            // records rather than growing a table larger than the original.
            if record_bytes + discards.len() * size_of::<BreakpointDiscard>() + extra_metadata >= direct_bytes {
                return Self::Direct(values.into_boxed_slice());
            }
            records.push(StoredBreakpoint {
                width: value.width,
                stretch: value.stretch,
                shrink: value.shrink,
                line_end: value.line_end,
                discard: index,
            });
        }
        Self::Shared { records: records.into_boxed_slice(), discards: discards.into_boxed_slice() }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        match self {
            Self::Direct(values) => values.len(),
            Self::Shared { records, .. } => records.len(),
        }
    }

    pub(super) fn get(&self, index: usize) -> Option<CompactBreakpoint> {
        match self {
            Self::Direct(values) => values.get(index).copied(),
            Self::Shared { records, discards } => {
                Some(records.get(index)?.expand(discards))
            }
        }
    }

    #[cfg(test)]
    fn iter(&self) -> impl ExactSizeIterator<Item = CompactBreakpoint> + Clone + '_ {
        (0..self.len()).map(|index| self.get(index).expect("breakpoint index is valid"))
    }

    pub(super) fn memory_usage_bytes(&self) -> usize {
        match self {
            Self::Direct(values) => values.len() * size_of::<CompactBreakpoint>(),
            Self::Shared { records, discards } => {
                records.len() * size_of::<StoredBreakpoint>() + discards.len() * size_of::<BreakpointDiscard>()
            }
        }
    }
}

impl CompactBreakpoint {
    pub(super) fn soft(
        width: f64,
        stretch: f64,
        shrink: f64,
        discard_width: f64,
        discard_stretch: f64,
        discard_shrink: f64,
        line_end: u32,
        next_line_start: u32,
    ) -> Self {
        Self {
            width,
            stretch,
            shrink,
            discard_width,
            discard_stretch: discard_stretch as f32,
            discard_shrink: discard_shrink as f32,
            line_end,
            next_line_start,
        }
    }

    pub(super) fn discretionary(
        width: f64,
        stretch: f64,
        shrink: f64,
        penalty_width: f64,
        penalty: f64,
        line_end: u32,
        next_line_start: u32,
    ) -> Self {
        // Discretionary breaks discard no glue, so reuse discard width and
        // shrink for their (negated) penalty metrics and retain the compact
        // record's 48-byte cache footprint.
        Self {
            width,
            stretch,
            shrink,
            discard_width: -penalty_width,
            discard_stretch: 0.0,
            discard_shrink: -penalty as f32,
            line_end,
            next_line_start,
        }
    }

    pub(super) fn final_(width: f64, stretch: f64, shrink: f64, token_end: u32) -> Self {
        Self {
            width,
            stretch: stretch + 1.0e9,
            shrink,
            discard_width: 0.0,
            discard_stretch: 0.0,
            discard_shrink: 0.0,
            line_end: token_end,
            next_line_start: token_end,
        }
    }

    fn is_discretionary(&self) -> bool {
        self.discard_shrink < 0.0
    }

    fn is_hyphen(&self) -> bool {
        self.is_discretionary() && self.discard_width < 0.0
    }

    fn penalty_width(&self) -> f64 {
        if self.is_discretionary() {
            -self.discard_width
        } else {
            0.0
        }
    }

    fn penalty(&self) -> f64 {
        if self.is_discretionary() {
            -f64::from(self.discard_shrink)
        } else {
            0.0
        }
    }

    pub(super) fn discarded_stretch(&self) -> f64 {
        f64::from(self.discard_stretch)
    }

    pub(super) fn discarded_shrink(&self) -> f64 {
        f64::from(self.discard_shrink)
    }

    pub(super) fn line_end(&self) -> usize {
        self.line_end as usize
    }

    pub(super) fn next_line_start(&self) -> usize {
        self.next_line_start as usize
    }
}

#[derive(Clone, Copy)]
struct CompactActiveNode {
    path: u32,
    line: u32,
    width: f64,
    stretch: f64,
    shrink: f64,
    demerits: f64,
    fitness: i8,
    hyphen_run: u8,
    // Occupies padding in the 48-byte active record.
    tracking_start: u32,
}

struct CompactPathNode {
    breakpoint: u32,
    previous: u32,
}

const NO_COMPACT_PATH: u32 = u32::MAX;

/// Width-dependent solver storage retained between paragraphs and resizes.
#[derive(Default)]
pub(in crate::layout::inline) struct CompactScratch {
    paths: Vec<CompactPathNode>,
    active: Vec<CompactActiveNode>,
    breaks: Vec<usize>,
}

impl CompactScratch {
    #[cfg(test)]
    pub(in crate::layout::inline) fn capacities(&self) -> (usize, usize, usize) {
        (self.paths.capacity(), self.active.capacity(), self.breaks.capacity())
    }
}

#[cfg(test)]
pub(super) fn break_compact_reusing<'s>(breakpoints: &BreakpointStorage, line_width: impl Fn(usize) -> f64, tolerance: f64, ragged: bool, quality: bool, scratch: &'s mut CompactScratch) -> Option<&'s [usize]> {
    break_compact_reusing_with_tracking(breakpoints, line_width, tolerance, ragged, quality, &|_, _, _, _| false, scratch)
}

pub(super) fn break_compact_reusing_with_tracking<'s>(breakpoints: &BreakpointStorage, line_width: impl Fn(usize) -> f64, tolerance: f64, ragged: bool, quality: bool, admit_tracking: &impl Fn(u32, u32, f64, f64) -> bool, scratch: &'s mut CompactScratch) -> Option<&'s [usize]> {
    profile_scope!("lines.knuth_solver");
    // Select storage once, outside both the breakpoint loop and its retries.
    // Direct records keep the slice iterator used by tiny/uncached plans.
    match breakpoints {
        BreakpointStorage::Direct(values) => break_compact_iter(values.iter().copied(), line_width, tolerance, ragged, quality, admit_tracking, scratch),
        BreakpointStorage::Shared { records, discards } => {
            break_compact_iter(records.iter().map(|record| record.expand(discards)), line_width, tolerance, ragged, quality, admit_tracking, scratch)
        }
    }
}

fn break_compact_iter<'s>(breakpoints: impl ExactSizeIterator<Item = CompactBreakpoint> + Clone, line_width: impl Fn(usize) -> f64, tolerance: f64, ragged: bool, quality: bool, admit_tracking: &impl Fn(u32, u32, f64, f64) -> bool, scratch: &'s mut CompactScratch) -> Option<&'s [usize]> {
    if ragged {
        if quality && break_compact_ragged_with_mode(breakpoints.clone(), &line_width, false, true, scratch).is_some() {
            return Some(&scratch.breaks);
        }
        break_compact_ragged_with_mode(breakpoints, &line_width, true, quality, scratch)
    } else {
        if break_compact_with_mode(breakpoints.clone(), &line_width, tolerance, false, quality, admit_tracking, scratch).is_some() {
            return Some(&scratch.breaks);
        }
        #[cfg(feature = "profiling")]
        if measurements_enabled() {
            // Counterfactual probes run only after a strict failure. Their
            // choices are discarded; the original emergency pass runs last.
            macro_rules! probe {
                ($policy:expr) => {
                    let _ = break_compact_with_mode_impl::<true, $policy>(
                        breakpoints.clone(), &line_width, tolerance, false, quality, admit_tracking, scratch,
                    );
                };
            }
            probe!(1); // Micro-tracking within the existing 0.5% capacity.
            probe!(2); // Relax the consecutive-hyphen limit only.
            probe!(3); // Tracking and hyphen-limit relaxation together.
            probe!(4); // Permit underfull lines only.
            probe!(8); // Permit tight lines only.
            probe!(12); // Both spacing relaxations, keeping the hyphen limit.
            probe!(6); // Underfull lines and hyphen-limit relaxation.
            probe!(10); // Tight lines and hyphen-limit relaxation.
        }
        break_compact_with_mode(breakpoints, &line_width, tolerance, true, quality, admit_tracking, scratch)
    }
}

/// Knuth–Plass over the compact legal-break stream used by inline layout.
///
/// The active records contain every value read by the inner loop and are
/// compacted in place. This avoids the old active-index gathers into a much
/// larger append-only `Node` arena and exposes contiguous numeric columns to
/// the optimizer.
#[cfg(test)]
pub(super) fn break_compact(breakpoints: &[CompactBreakpoint], line_width: impl Fn(usize) -> f64, tolerance: f64) -> Option<Vec<usize>> {
    break_compact_with_mode(BreakpointStorage::new(breakpoints.to_vec()).iter(), line_width, tolerance, false, false, &|_, _, _, _| false, &mut CompactScratch::default()).map(<[usize]>::to_vec)
}

#[cfg(test)]
pub(super) fn break_compact_emergency(breakpoints: &[CompactBreakpoint], line_width: impl Fn(usize) -> f64, tolerance: f64) -> Option<Vec<usize>> {
    break_compact_with_mode(BreakpointStorage::new(breakpoints.to_vec()).iter(), line_width, tolerance, true, false, &|_, _, _, _| false, &mut CompactScratch::default()).map(<[usize]>::to_vec)
}

#[cfg(test)]
pub(super) fn break_compact_book(breakpoints: &[CompactBreakpoint], line_width: impl Fn(usize) -> f64, tolerance: f64) -> Option<Vec<usize>> {
    break_compact_with_mode(BreakpointStorage::new(breakpoints.to_vec()).iter(), line_width, tolerance, false, true, &|_, _, _, _| false, &mut CompactScratch::default()).map(<[usize]>::to_vec)
}

#[cfg(test)]
pub(super) fn break_compact_emergency_book(breakpoints: &[CompactBreakpoint], line_width: impl Fn(usize) -> f64, tolerance: f64) -> Option<Vec<usize>> {
    break_compact_with_mode(BreakpointStorage::new(breakpoints.to_vec()).iter(), line_width, tolerance, true, true, &|_, _, _, _| false, &mut CompactScratch::default()).map(<[usize]>::to_vec)
}

/// Paragraph-wide composition for fixed-spacing ragged text. Unlike justified
/// Knuth--Plass, unused line width is intentional and becomes the badness
/// being minimized; word spaces retain their natural advances.
#[cfg(test)]
pub(super) fn break_compact_ragged(breakpoints: &[CompactBreakpoint], line_width: impl Fn(usize) -> f64) -> Option<Vec<usize>> {
    break_compact_reusing(&BreakpointStorage::new(breakpoints.to_vec()), line_width, 1.0, true, true, &mut CompactScratch::default()).map(<[usize]>::to_vec)
}

fn break_compact_ragged_with_mode<'s>(breakpoints: impl ExactSizeIterator<Item = CompactBreakpoint>, line_width: &impl Fn(usize) -> f64, allow_excessive_hyphen_run: bool, hyphenation_quality: bool, scratch: &'s mut CompactScratch) -> Option<&'s [usize]> {
    profile_scope!("lines.ragged_solver");
    scratch.paths.clear();
    scratch.active.clear();
    scratch.breaks.clear();
    if breakpoints.len() == 0 {
        return Some(&scratch.breaks);
    }
    let CompactScratch { paths, active, breaks } = scratch;
    active.push(CompactActiveNode { path: NO_COMPACT_PATH, line: 0, width: 0.0, stretch: 0.0, shrink: 0.0, demerits: 0.0, fitness: 1, hyphen_run: 0, tracking_start: 0 });

    let breakpoint_count = breakpoints.len();
    for (breakpoint_index, breakpoint) in breakpoints.enumerate() {
        let forced = breakpoint_index + 1 == breakpoint_count;
        // The first line can have a different indent. After it, candidates at
        // this breakpoint are geometrically equivalent, so retain only the
        // cheapest path in each category.
        let mut best: [Option<(f64, CompactActiveNode, u8)>; 6] = [None; 6];
        let mut kept = 0usize;

        for read in 0..active.len() {
            let candidate = active[read];
            let available = line_width(candidate.line as usize).max(0.0);
            let width = breakpoint.width - candidate.width + breakpoint.penalty_width();
            let fits = width <= available + 1.0e-9;
            let next_hyphen_run = if breakpoint.is_hyphen() {
                candidate.hyphen_run.saturating_add(1)
            } else {
                0
            };
            let excessive_hyphen_run =
                hyphenation_quality && next_hyphen_run > MAX_CONSECUTIVE_HYPHENATED_LINES;
            if fits && (!excessive_hyphen_run || allow_excessive_hyphen_run) {
                let fill = if available > 0.0 {
                    (width / available).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let remaining = 1.0 - fill;
                let line_demerits = if forced {
                    // A naturally short last line is desirable. Penalize only
                    // conspicuously short endings, and never alter a paragraph
                    // that already fits on one line.
                    if candidate.line == 0 {
                        0.0
                    } else {
                        ((0.25 - fill).max(0.0) * 200.0).powi(2)
                    }
                } else {
                    (remaining * 100.0).powi(2)
                };
                let mut demerits = candidate.demerits + line_demerits;
                if hyphenation_quality && forced && candidate.hyphen_run > 0 {
                    demerits += PENULTIMATE_HYPHEN_PENALTY;
                }
                if hyphenation_quality && breakpoint.is_hyphen() {
                    demerits += short_hyphenated_line_demerits(width, available);
                }
                let penalty = breakpoint.penalty();
                if penalty > 0.0 && penalty < INF_PENALTY {
                    demerits += penalty * penalty;
                }
                if breakpoint.is_hyphen() && candidate.hyphen_run > 0 {
                    demerits += FLAGGED_PENALTY;
                }
                if excessive_hyphen_run {
                    demerits += EXCESSIVE_HYPHEN_RUN_PENALTY;
                }
                let bucket = usize::from(candidate.line > 0) * 3
                    + usize::from(next_hyphen_run.min(MAX_CONSECUTIVE_HYPHENATED_LINES));
                if best[bucket].is_none_or(|(best_demerits, _, _)| demerits < best_demerits) {
                    best[bucket] = Some((demerits, candidate, next_hyphen_run));
                }
            }

            if fits && !forced {
                active[kept] = candidate;
                kept += 1;
            }
        }
        active.truncate(kept);

        let (next_width, next_stretch, next_shrink) = if breakpoint.is_discretionary() {
            (breakpoint.width, breakpoint.stretch, breakpoint.shrink)
        } else {
            (
                breakpoint.width + breakpoint.discard_width,
                breakpoint.stretch + breakpoint.discarded_stretch(),
                breakpoint.shrink + breakpoint.discarded_shrink(),
            )
        };
        for candidate in best.into_iter().flatten() {
            let (demerits, previous, hyphen_run) = candidate;
            let path = u32::try_from(paths.len()).ok()?;
            paths.push(CompactPathNode {
                breakpoint: breakpoint_index as u32,
                previous: previous.path,
            });
            active.push(CompactActiveNode {
                path,
                line: previous.line + 1,
                width: next_width,
                stretch: next_stretch,
                shrink: next_shrink,
                demerits,
                fitness: 1,
                hyphen_run,
                tracking_start: breakpoint.next_line_start,
            });
        }

        if active.is_empty() {
            return None;
        }
    }

    let best = active.iter().min_by(|a, b| {
        a.demerits
            .partial_cmp(&b.demerits)
            .unwrap_or(Ordering::Equal)
    })?;
    let mut path = best.path;
    while path != NO_COMPACT_PATH {
        let node = &paths[path as usize];
        breaks.push(node.breakpoint as usize);
        path = node.previous;
    }
    breaks.reverse();
    Some(breaks.as_slice())
}

#[cfg(feature = "profiling")]
#[derive(Default)]
struct SolverCounts {
    requested: usize,
    processed: usize,
    evaluations: usize,
    geometry_reuses: usize,
    scored_geometry_reuses: usize,
    max_active: usize,
    first_line: usize,
    max_line: usize,
    scored: usize,
    underfull: usize,
    tight: usize,
    tracking: usize,
    best_updates: usize,
    dropped: usize,
    paths: usize,
    success: usize,
    lines: usize,
    rejected_loose: usize,
    rejected_tight: usize,
    rejected_hyphens: usize,
    tracking_feasible: usize,
    selected_loose: usize,
    selected_tight: usize,
    selected_hyphens: usize,
    selected_tracking_fit: usize,
    selected_ragged: usize,
    selected_ragged_total_millipx: usize,
    selected_ragged_max_millipx: usize,
    selected_ragged_over_1px: usize,
    selected_ragged_over_3px: usize,
    selected_ragged_over_10px: usize,
    selected_tracking_fit_total_millipx: usize,
    selected_tracking_fit_max_millipx: usize,
}


#[cfg(feature = "profiling")]
struct SolverMeasurements {
    name: &'static str,
    started: Instant,
    counts: SolverCounts,
    path_conditions: Vec<(u8, f64, f64)>,
}

#[cfg(feature = "profiling")]
impl Drop for SolverMeasurements {
    fn drop(&mut self) {
        let counts = &self.counts;
        record_measurement(self.name, &[
            ("breakpoints_requested", counts.requested),
            ("breakpoints_processed", counts.processed),
            ("candidate_evaluations", counts.evaluations),
            ("geometry_reuses", counts.geometry_reuses),
            ("scored_geometry_reuses", counts.scored_geometry_reuses),
            ("max_active", counts.max_active),
            ("first_line_candidates", counts.first_line),
            ("max_line", counts.max_line),
            ("scored_candidates", counts.scored),
            ("emergency_underfull", counts.underfull),
            ("emergency_tight", counts.tight),
            ("tracking_candidates", counts.tracking),
            ("best_updates", counts.best_updates),
            ("dropped_candidates", counts.dropped),
            ("paths_created", counts.paths),
            ("success", counts.success),
            ("selected_lines", counts.lines),
            ("rejected_loose", counts.rejected_loose),
            ("rejected_tight", counts.rejected_tight),
            ("rejected_hyphens_with_valid_spacing", counts.rejected_hyphens),
            ("tracking_feasible_candidates", counts.tracking_feasible),
            ("selected_loose", counts.selected_loose),
            ("selected_tight", counts.selected_tight),
            ("selected_excessive_hyphen_run", counts.selected_hyphens),
            ("selected_tracking_fits", counts.selected_tracking_fit),
            ("selected_requires_ragged", counts.selected_ragged),
            ("selected_ragged_total_millipx", counts.selected_ragged_total_millipx),
            ("selected_ragged_max_millipx", counts.selected_ragged_max_millipx),
            ("selected_ragged_over_1px", counts.selected_ragged_over_1px),
            ("selected_ragged_over_3px", counts.selected_ragged_over_3px),
            ("selected_ragged_over_10px", counts.selected_ragged_over_10px),
            ("selected_tracking_fit_total_millipx", counts.selected_tracking_fit_total_millipx),
            ("selected_tracking_fit_max_millipx", counts.selected_tracking_fit_max_millipx),
            ("wall_ns", self.started.elapsed().as_nanos() as usize),
        ]);
    }
}

fn break_compact_with_mode<'s>(breakpoints: impl ExactSizeIterator<Item = CompactBreakpoint>, line_width: impl Fn(usize) -> f64, tolerance: f64, allow_underfull: bool, book_hyphen_quality: bool, admit_tracking: &impl Fn(u32, u32, f64, f64) -> bool, scratch: &'s mut CompactScratch) -> Option<&'s [usize]> {
    profile_scope!("lines.justified_solver");
    #[cfg(feature = "profiling")]
    if measurements_enabled() {
        return break_compact_with_mode_impl::<true, 0>(breakpoints, line_width, tolerance, allow_underfull, book_hyphen_quality, admit_tracking, scratch);
    }
    break_compact_with_mode_impl::<false, 0>(breakpoints, line_width, tolerance, allow_underfull, book_hyphen_quality, admit_tracking, scratch)
}

// RELAX is zero for the real solver. Diagnostic-only bits relax tracking
// (1), hyphen runs (2), underfull lines (4), or tight lines (8) in isolation.
fn break_compact_with_mode_impl<'s, const COLLECT: bool, const RELAX: u8>(breakpoints: impl ExactSizeIterator<Item = CompactBreakpoint>, line_width: impl Fn(usize) -> f64, tolerance: f64, allow_underfull: bool, book_hyphen_quality: bool, admit_tracking: &impl Fn(u32, u32, f64, f64) -> bool, scratch: &'s mut CompactScratch) -> Option<&'s [usize]> {
    #[cfg(feature = "profiling")]
    let mode_name = match RELAX {
        1 => "lines.justified.probe.tracking",
        2 => "lines.justified.probe.hyphens",
        3 => "lines.justified.probe.tracking_hyphens",
        4 => "lines.justified.probe.underfull",
        8 => "lines.justified.probe.tight",
        12 => "lines.justified.probe.spacing",
        6 => "lines.justified.probe.underfull_hyphens",
        10 => "lines.justified.probe.tight_hyphens",
        _ if allow_underfull => "lines.justified.emergency",
        _ => "lines.justified.normal",
    };
    #[cfg(feature = "profiling")]
    let mut measurements = COLLECT.then(|| SolverMeasurements {
        name: mode_name,
        started: Instant::now(),
        counts: SolverCounts { requested: breakpoints.len(), ..SolverCounts::default() },
        path_conditions: Vec::new(),
    });
    #[cfg(feature = "profiling")]
    let _mode_scope = scope(mode_name);
    scratch.paths.clear();
    scratch.active.clear();
    scratch.breaks.clear();
    if breakpoints.len() == 0 {
        #[cfg(feature = "profiling")]
        if let Some(measurements) = &mut measurements { measurements.counts.success = 1; }
        return Some(&scratch.breaks);
    }
    let CompactScratch { paths, active, breaks } = scratch;
    active.push(CompactActiveNode { path: NO_COMPACT_PATH, line: 0, width: 0.0, stretch: 0.0, shrink: 0.0, demerits: 0.0, fitness: 1, hyphen_run: 0, tracking_start: 0 });

    let breakpoint_count = breakpoints.len();
    for (breakpoint_index, breakpoint) in breakpoints.enumerate() {
        let forced = breakpoint_index + 1 == breakpoint_count;
        let mut best: [Option<(f64, CompactActiveNode, u8)>; 12] = [None; 12];
        let mut kept = 0usize;
        #[cfg(feature = "profiling")]
        let mut best_conditions = [(0u8, 0.0, 0.0); 12];
        #[cfg(feature = "profiling")]
        if let Some(measurements) = &mut measurements {
            let counts = &mut measurements.counts;
            counts.processed += 1;
            counts.evaluations += active.len();
            counts.max_active = counts.max_active.max(active.len());
        }

        #[cfg(feature = "profiling")]
        let mut previous_geometry = None;
        for read in 0..active.len() {
            let candidate = active[read];
            #[cfg(feature = "profiling")]
            if let Some(measurements) = &mut measurements {
                measurements.counts.first_line += usize::from(candidate.line == 0);
                measurements.counts.max_line = measurements.counts.max_line.max(candidate.line as usize);
            }
            let available = line_width(candidate.line as usize);
            #[cfg(feature = "profiling")]
            let mut geometry_reused = false;
            #[cfg(feature = "profiling")]
            if let Some(measurements) = &mut measurements {
                let geometry = (candidate.width.to_bits(), candidate.stretch.to_bits(), candidate.shrink.to_bits(), available.to_bits());
                geometry_reused = previous_geometry == Some(geometry);
                measurements.counts.geometry_reuses += usize::from(geometry_reused);
                previous_geometry = Some(geometry);
            }
            let width = breakpoint.width - candidate.width + breakpoint.penalty_width();
            let ratio = if width < available {
                let stretch = breakpoint.stretch - candidate.stretch;
                if stretch > 0.0 {
                    (available - width) / stretch
                } else {
                    f64::INFINITY
                }
            } else if width > available {
                let shrink = breakpoint.shrink - candidate.shrink;
                if shrink > 0.0 {
                    (available - width) / shrink
                } else {
                    f64::NEG_INFINITY
                }
            } else {
                0.0
            };

            let remaining_after_stretch = if width < available {
                (available - width - (breakpoint.stretch - candidate.stretch) * tolerance).max(0.0)
            } else {
                0.0
            };
            let remaining_after_shrink = if width > available {
                (width - available - (breakpoint.shrink - candidate.shrink)).max(0.0)
            } else {
                0.0
            };
            let tracking_capacity = if forced {
                0.0
            } else {
                tracking_width_limit(width)
            };
            let tracking_adjustment = if ratio > tolerance {
                remaining_after_stretch.min(tracking_capacity)
            } else if ratio < -1.0 {
                -remaining_after_shrink.min(tracking_capacity)
            } else {
                0.0
            };
            let ragged_remainder = (remaining_after_stretch - tracking_capacity).max(0.0);
            let emergency_underfull =
                (allow_underfull || RELAX & 4 != 0) && ratio > tolerance && remaining_after_stretch > 0.0;
            let emergency_tight = (allow_underfull || RELAX & 8 != 0)
                && ratio < -1.0
                && remaining_after_shrink > 0.0
                && remaining_after_shrink <= tracking_capacity;
            let counts_as_hyphen = if book_hyphen_quality {
                breakpoint.is_hyphen()
            } else {
                breakpoint.is_discretionary()
            };
            let next_hyphen_run = if counts_as_hyphen {
                candidate.hyphen_run.saturating_add(1)
            } else {
                0
            };
            let excessive_hyphen_run =
                book_hyphen_quality && next_hyphen_run > MAX_CONSECUTIVE_HYPHENATED_LINES;
            let tracking_fits = tracking_capacity > 0.0
                && ((ratio > tolerance && remaining_after_stretch <= tracking_capacity)
                    || (ratio < -1.0 && remaining_after_shrink <= tracking_capacity));
            let tracking_admitted = !allow_underfull
                && tracking_fits
                && (!excessive_hyphen_run || RELAX & 2 != 0)
                && admit_tracking(candidate.tracking_start, breakpoint.line_end, width,
                    if ratio > tolerance { remaining_after_stretch } else { remaining_after_shrink });
            #[cfg(feature = "profiling")]
            if let Some(measurements) = &mut measurements {
                let counts = &mut measurements.counts;
                counts.rejected_loose += usize::from(!allow_underfull && ratio > tolerance && !tracking_admitted);
                counts.rejected_tight += usize::from(!allow_underfull && ratio < -1.0 && !tracking_admitted);
                counts.rejected_hyphens += usize::from(!allow_underfull && excessive_hyphen_run && ratio >= -1.0 && ratio <= tolerance);
                counts.tracking_feasible += usize::from(tracking_fits);
            }
            if ((ratio >= -1.0 && ratio <= tolerance) || emergency_underfull || emergency_tight || tracking_admitted || (RELAX & 1 != 0 && tracking_fits))
                && (!excessive_hyphen_run || allow_underfull || RELAX & 2 != 0)
            {
                #[cfg(feature = "profiling")]
                if let Some(measurements) = &mut measurements {
                    let counts = &mut measurements.counts;
                    counts.scored += 1;
                    counts.scored_geometry_reuses += usize::from(geometry_reused);
                    counts.underfull += usize::from(emergency_underfull);
                    counts.tight += usize::from(emergency_tight);
                    counts.tracking += usize::from(tracking_adjustment != 0.0);
                }
                let effective_ratio = ratio.clamp(-1.0, tolerance);
                let badness = 100.0 * effective_ratio.abs().powi(3);
                let mut demerits = candidate.demerits + (1.0 + badness).powi(2);
                if book_hyphen_quality && forced && candidate.hyphen_run > 0 {
                    demerits += PENULTIMATE_HYPHEN_PENALTY;
                }
                if book_hyphen_quality && breakpoint.is_hyphen() {
                    demerits += short_hyphenated_line_demerits(width, available);
                }
                if tracking_adjustment != 0.0 {
                    demerits += tracking_demerits(tracking_adjustment, tracking_capacity);
                }
                if emergency_underfull && ragged_remainder > 0.0 {
                    demerits += underfull_demerits(ragged_remainder, available);
                }
                let fitness = fitness_class(effective_ratio);
                if (fitness - candidate.fitness as i32).abs() > 1 {
                    demerits += FITNESS_PENALTY;
                }
                let penalty = breakpoint.penalty();
                if penalty > 0.0 && penalty < INF_PENALTY {
                    demerits += penalty * penalty;
                }
                if counts_as_hyphen && candidate.hyphen_run > 0 {
                    demerits += FLAGGED_PENALTY;
                }
                if excessive_hyphen_run {
                    demerits += EXCESSIVE_HYPHEN_RUN_PENALTY;
                }
                let hyphen_slot = if book_hyphen_quality {
                    next_hyphen_run.min(MAX_CONSECUTIVE_HYPHENATED_LINES)
                } else {
                    0
                };
                let slot = fitness as usize * 3 + usize::from(hyphen_slot);
                if best[slot].is_none_or(|(best_demerits, _, _)| demerits < best_demerits) {
                    best[slot] = Some((demerits, candidate, next_hyphen_run));
                    #[cfg(feature = "profiling")]
                    if let Some(measurements) = &mut measurements {
                        measurements.counts.best_updates += 1;
                        best_conditions[slot] = (u8::from(ratio > tolerance)
                            | (u8::from(ratio < -1.0) << 1)
                            | (u8::from(excessive_hyphen_run) << 2)
                            | (u8::from(tracking_fits) << 3)
                            | (u8::from(ratio > tolerance && ragged_remainder > 0.0) << 4),
                            ragged_remainder,
                            if ratio > tolerance { remaining_after_stretch } else { remaining_after_shrink });
                    }
                }
            }

            if (ratio >= -1.0 || tracking_admitted || (RELAX & 1 != 0 && tracking_fits)) && !forced {
                active[kept] = candidate;
                kept += 1;
            }
        }
        #[cfg(feature = "profiling")]
        if let Some(measurements) = &mut measurements { measurements.counts.dropped += active.len() - kept; }
        active.truncate(kept);

        let (next_width, next_stretch, next_shrink) = if breakpoint.is_discretionary() {
            (breakpoint.width, breakpoint.stretch, breakpoint.shrink)
        } else {
            (
                breakpoint.width + breakpoint.discard_width,
                breakpoint.stretch + breakpoint.discarded_stretch(),
                breakpoint.shrink + breakpoint.discarded_shrink(),
            )
        };
        for (slot, candidate) in best.into_iter().enumerate() {
            if let Some((demerits, previous, hyphen_run)) = candidate {
                let path = u32::try_from(paths.len()).ok()?;
                paths.push(CompactPathNode {
                    breakpoint: breakpoint_index as u32,
                    previous: previous.path,
                });
                #[cfg(feature = "profiling")]
                if let Some(measurements) = &mut measurements {
                    measurements.path_conditions.push(best_conditions[slot]);
                }
                active.push(CompactActiveNode {
                    path,
                    line: previous.line + 1,
                    width: next_width,
                    stretch: next_stretch,
                    shrink: next_shrink,
                    demerits,
                    fitness: (slot / 3) as i8,
                    hyphen_run,
                    tracking_start: breakpoint.next_line_start,
                });
            }
        }

        #[cfg(feature = "profiling")]
        if let Some(measurements) = &mut measurements { measurements.counts.paths = paths.len(); }
        if active.is_empty() {
            return None;
        }
    }

    let best = active.iter().min_by(|a, b| {
        a.demerits
            .partial_cmp(&b.demerits)
            .unwrap_or(Ordering::Equal)
    })?;
    let mut path = best.path;
    while path != NO_COMPACT_PATH {
        let node = &paths[path as usize];
        #[cfg(feature = "profiling")]
        if let Some(measurements) = &mut measurements {
            let (conditions, residual, tracking_needed) = measurements.path_conditions[path as usize];
            let counts = &mut measurements.counts;
            counts.selected_loose += usize::from(conditions & 1 != 0);
            counts.selected_tight += usize::from(conditions & 2 != 0);
            counts.selected_hyphens += usize::from(conditions & 4 != 0);
            counts.selected_tracking_fit += usize::from(conditions & 8 != 0);
            counts.selected_ragged += usize::from(conditions & 16 != 0);
            if conditions & 16 != 0 {
                let millipx = (residual * 1000.0).round() as usize;
                counts.selected_ragged_total_millipx += millipx;
                counts.selected_ragged_max_millipx = counts.selected_ragged_max_millipx.max(millipx);
                counts.selected_ragged_over_1px += usize::from(residual > 1.0);
                counts.selected_ragged_over_3px += usize::from(residual > 3.0);
                counts.selected_ragged_over_10px += usize::from(residual > 10.0);
            }
            if conditions & 8 != 0 {
                let millipx = (tracking_needed * 1000.0).round() as usize;
                counts.selected_tracking_fit_total_millipx += millipx;
                counts.selected_tracking_fit_max_millipx = counts.selected_tracking_fit_max_millipx.max(millipx);
            }
        }
        breaks.push(node.breakpoint as usize);
        path = node.previous;
    }
    breaks.reverse();
    #[cfg(feature = "profiling")]
    if let Some(measurements) = &mut measurements {
        measurements.counts.success = 1;
        measurements.counts.lines = breaks.len();
    }
    Some(breaks.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordinary_soft(
        width: f64,
        stretch: f64,
        shrink: f64,
        space: f64,
        line_end: u32,
        next_line_start: u32,
    ) -> CompactBreakpoint {
        let (discard_stretch, discard_shrink) =
            crate::layout::inline::justification::glue_capacities(
                space,
                crate::layout::inline::justification::SpaceGlueClass::Ordinary,
                false,
            );
        CompactBreakpoint::soft(
            width,
            stretch,
            shrink,
            space,
            discard_stretch,
            discard_shrink,
            line_end,
            next_line_start,
        )
    }

    fn compact_paragraph(words: &[f64], space: f64) -> Vec<CompactBreakpoint> {
        let (mut width, mut stretch, mut shrink) = (0.0, 0.0, 0.0);
        let mut breakpoints = Vec::new();
        for (index, &word) in words.iter().enumerate() {
            width += word;
            if index + 1 < words.len() {
                breakpoints.push(ordinary_soft(
                    width,
                    stretch,
                    shrink,
                    space,
                    index as u32,
                    index as u32 + 1,
                ));
                width += space;
                stretch += space * 0.5;
                shrink += space * 0.33;
            }
        }
        breakpoints.push(CompactBreakpoint::final_(
            width,
            stretch,
            shrink,
            words.len() as u32,
        ));
        breakpoints
    }

    #[test]
    fn compact_breakpoint_stays_cache_dense() {
        assert_eq!(size_of::<CompactBreakpoint>(), 48);
        assert_eq!(size_of::<CompactActiveNode>(), 48);
        assert_eq!(size_of::<StoredBreakpoint>(), 32);
    }

    #[test]
    fn shared_breakpoint_metrics_preserve_precision_and_solver_choices() {
        let mut values = compact_paragraph(&[30.125; 64], 3.333333333333333);
        // Exercise discretionary and zero-width opportunities as well as
        // space discards, with cumulative totals beyond f32 precision.
        values[3] = CompactBreakpoint::discretionary(100_000_001.125, 0.0, 0.0, 2.125, 50.0, 3, 3);
        values[7] = CompactBreakpoint::soft(100_000_002.125, 0.0, 0.0, 0.0, -0.0, 0.0, 7, 7);
        let stored = BreakpointStorage::new(values.clone());
        assert!(matches!(stored, BreakpointStorage::Shared { .. }));
        assert!(stored.memory_usage_bytes() < values.len() * size_of::<CompactBreakpoint>());
        for (before, after) in values.iter().zip(stored.iter()) {
            assert_eq!(before.width.to_bits(), after.width.to_bits());
            assert_eq!(before.stretch.to_bits(), after.stretch.to_bits());
            assert_eq!(before.shrink.to_bits(), after.shrink.to_bits());
            assert_eq!(before.discard_width.to_bits(), after.discard_width.to_bits());
            assert_eq!(before.discard_stretch.to_bits(), after.discard_stretch.to_bits());
            assert_eq!(before.discard_shrink.to_bits(), after.discard_shrink.to_bits());
            assert_eq!(before.line_end, after.line_end);
            assert_eq!(before.next_line_start, after.next_line_start);
        }

        let values = compact_paragraph(&[30.125; 64], 3.333333333333333);
        let direct = BreakpointStorage::Direct(values.clone().into_boxed_slice());
        let shared = BreakpointStorage::new(values);
        for width in [32.0, 75.0, 180.0, 600.0] {
            for ragged in [false, true] {
                for quality in [false, true] {
                    let solve = |storage| break_compact_reusing(storage, |_| width, 1.0, ragged, quality, &mut CompactScratch::default()).map(<[usize]>::to_vec);
                    assert_eq!(solve(&direct), solve(&shared));
                }
            }
        }
    }

    #[test]
    fn distinct_breakpoint_metrics_keep_direct_storage() {
        let values: Vec<_> = (0..64).map(|index| {
            CompactBreakpoint::soft(index as f64 * 30.0, 0.0, 0.0, index as f64 + 0.125, 1.0, 0.5, index, index + 1)
        }).collect();
        let stored = BreakpointStorage::new(values);
        assert!(matches!(stored, BreakpointStorage::Direct(_)));
        assert_eq!(stored.memory_usage_bytes(), 64 * size_of::<CompactBreakpoint>());
    }

    #[cfg(feature = "profiling")]
    #[test]
    fn solver_measurements_match_width_queries_and_preserve_emergency_choices() {
        use crate::profiling::{capture, capture_with_measurements};
        use std::cell::Cell;
        use std::time::Duration;

        let plan = BreakpointStorage::new(compact_paragraph(&[2.0, 2.0, 2.0, 2.0], 2.0));
        let queries = Cell::new(0);
        let mut scratch = CompactScratch::default();
        let solve = |scratch: &mut CompactScratch| break_compact_reusing(
            &plan, |_| { queries.set(queries.get() + 1); 8.5 }, 1.0, false, true, scratch,
        ).map(<[usize]>::to_vec);
        let (result, entries) = capture_with_measurements(Duration::default, true, || solve(&mut scratch));
        assert_eq!(result, Some(vec![1, 3]));
        let normal = &entries.iter().find(|entry| entry.name == "lines.justified.normal").unwrap().measurements;
        let emergency = &entries.iter().find(|entry| entry.name == "lines.justified.emergency").unwrap().measurements;
        assert_eq!((normal.len(), emergency.len()), (1, 1));
        assert_eq!(normal[0]["success"], 0);
        assert_eq!(emergency[0]["success"], 1);
        assert_eq!(emergency[0]["selected_lines"], 2);
        assert_eq!(entries.iter().flat_map(|entry| &entry.measurements).map(|row| row["candidate_evaluations"]).sum::<usize>(), queries.get());
        let probe = |name| &entries.iter().find(|entry| entry.name == name).unwrap().measurements[0];
        assert_eq!(probe("lines.justified.probe.tracking")["success"], 0);
        assert_eq!(probe("lines.justified.probe.hyphens")["success"], 0);
        assert_eq!(probe("lines.justified.probe.underfull")["success"], 1);
        assert!(emergency[0]["selected_requires_ragged"] > 0);
        for row in normal.iter().chain(emergency) {
            assert!(row["scored_candidates"] <= row["candidate_evaluations"]);
            assert!(row["best_updates"] <= row["scored_candidates"]);
            assert!(row["geometry_reuses"] <= row["candidate_evaluations"]);
            assert!(row["scored_geometry_reuses"] <= row["scored_candidates"]);
            assert!(row["paths_created"] <= row["best_updates"]);
        }
        let (plain, entries) = capture(Duration::default, || solve(&mut scratch));
        assert_eq!(plain, result);
        assert!(entries.iter().all(|entry| entry.measurements.is_empty()));
    }

    #[cfg(feature = "profiling")]
    #[test]
    fn diagnostic_probes_isolate_tracking_and_hyphen_constraints() {
        use crate::profiling::capture_with_measurements;
        use std::time::Duration;

        // Both boundaries are just outside normal glue limits, but within
        // the existing 0.5% tracking allowance in opposite directions.
        for available in [101.1, 99.2] {
            let plan = BreakpointStorage::new(compact_paragraph(&[49.0, 49.0, 49.0, 40.0], 2.0));
            let (result, entries) = capture_with_measurements(Duration::default, true, || {
                break_compact_reusing(&plan, |_| available, 1.0, false, true, &mut CompactScratch::default()).map(<[usize]>::to_vec)
            });
            assert_eq!(result, Some(vec![1, 3]));
            let row = |name| &entries.iter().find(|entry| entry.name == name).unwrap().measurements[0];
            assert_eq!(row("lines.justified.normal")["success"], 0);
            assert_eq!(row("lines.justified.probe.tracking")["success"], 1);
            assert_eq!(row("lines.justified.probe.tracking")["selected_tracking_fits"], 1);
            assert_eq!(row("lines.justified.probe.hyphens")["success"], 0);
        }
        let plan = BreakpointStorage::new(vec![
            CompactBreakpoint::discretionary(30.0, 0.0, 0.0, 2.0, 50.0, 1, 1),
            CompactBreakpoint::discretionary(60.0, 0.0, 0.0, 2.0, 50.0, 2, 2),
            CompactBreakpoint::discretionary(90.0, 0.0, 0.0, 2.0, 50.0, 3, 3),
            CompactBreakpoint::final_(120.0, 0.0, 0.0, 4),
        ]);
        let (result, entries) = capture_with_measurements(Duration::default, true, || {
            break_compact_reusing(&plan, |_| 32.0, 1.0, false, true, &mut CompactScratch::default()).map(<[usize]>::to_vec)
        });
        assert_eq!(result, Some(vec![0, 1, 2, 3]));
        let row = |name| &entries.iter().find(|entry| entry.name == name).unwrap().measurements[0];
        assert_eq!(row("lines.justified.normal")["success"], 0);
        assert_eq!(row("lines.justified.probe.hyphens")["success"], 1);
        assert_eq!(row("lines.justified.probe.hyphens")["selected_excessive_hyphen_run"], 1);
        assert_eq!(row("lines.justified.probe.tracking")["success"], 0);
        assert_eq!(row("lines.justified.probe.spacing")["success"], 0);
    }

    #[test]
    fn compact_solver_handles_wide_narrow_and_infeasible_lines() {
        let paragraph = compact_paragraph(&[30.0, 30.0, 30.0], 10.0);
        assert_eq!(break_compact(&paragraph, |_| 1000.0, 8.0), Some(vec![2]));
        assert!(break_compact(&paragraph, |_| 75.0, 8.0).is_some());

        let overwide = compact_paragraph(&[200.0], 10.0);
        assert!(break_compact(&overwide, |_| 50.0, 8.0).is_none());
    }

    #[test]
    fn emergency_pass_prefers_a_short_line_to_excessive_space_expansion() {
        let compact = compact_paragraph(&[2.0, 2.0, 2.0, 2.0], 2.0);
        assert!(break_compact(&compact, |_| 8.5, 1.0).is_none());
        assert_eq!(
            break_compact_emergency(&compact, |_| 8.5, 1.0),
            Some(vec![1, 3])
        );
    }

    #[test]
    fn emergency_pass_does_not_create_a_one_word_first_line() {
        let compact = compact_paragraph(&[10.0, 10.0, 10.0], 1.0);

        assert_eq!(
            break_compact_emergency(&compact, |_| 25.0, 1.0),
            Some(vec![1, 2]),
            "when two words fit more closely, an emergency fallback must not strand the first word"
        );
    }

    #[test]
    fn ragged_composition_moves_a_word_to_avoid_an_orphaned_last_line() {
        let compact = compact_paragraph(&[20.0, 20.0, 5.0, 5.0], 10.0);
        assert_eq!(break_compact_ragged(&compact, |_| 70.0), Some(vec![1, 3]));
    }

    #[test]
    fn third_consecutive_hyphen_requires_the_emergency_pass() {
        let compact = vec![
            CompactBreakpoint::discretionary(30.0, 0.0, 0.0, 2.0, 50.0, 1, 1),
            CompactBreakpoint::discretionary(60.0, 0.0, 0.0, 2.0, 50.0, 2, 2),
            CompactBreakpoint::discretionary(90.0, 0.0, 0.0, 2.0, 50.0, 3, 3),
            CompactBreakpoint::final_(120.0, 0.0, 0.0, 4),
        ];

        assert_eq!(
            break_compact_book(&compact, |_| 32.0, 8.0),
            None,
            "the normal justified pass must reject a third consecutive hyphen"
        );
        assert_eq!(
            break_compact_emergency_book(&compact, |_| 32.0, 8.0),
            Some(vec![0, 1, 2, 3]),
            "the emergency pass must still make progress in a narrow measure"
        );
        assert_eq!(
            break_compact(&compact, |_| 32.0, 8.0),
            Some(vec![0, 1, 2, 3]),
            "web-compatible composition must retain its prior best-effort behavior"
        );
        assert_eq!(
            break_compact_ragged(&compact, |_| 32.0),
            Some(vec![0, 1, 2, 3]),
            "ragged composition must use the same best-effort fallback"
        );
    }

    #[test]
    fn book_emergency_interrupts_an_avoidable_long_hyphen_run() {
        // Monospace model of the visual fixture in a twenty-cell measure.
        // Continuing at every discretionary point fills each line, while the
        // thirteen-letter bridge words provide feasible normal boundaries.
        let compact = vec![
            CompactBreakpoint::discretionary(19.0, 0.5, 0.33, 1.0, 50.0, 1, 1),
            ordinary_soft(21.0, 0.5, 0.33, 1.0, 2, 3),
            ordinary_soft(35.0, 1.0, 0.66, 1.0, 4, 5),
            CompactBreakpoint::discretionary(38.0, 1.5, 0.99, 1.0, 50.0, 6, 6),
            ordinary_soft(40.0, 1.5, 0.99, 1.0, 7, 8),
            ordinary_soft(54.0, 2.0, 1.32, 1.0, 9, 10),
            CompactBreakpoint::discretionary(57.0, 2.5, 1.65, 1.0, 50.0, 11, 11),
            ordinary_soft(59.0, 2.5, 1.65, 1.0, 12, 13),
            ordinary_soft(73.0, 3.0, 1.98, 1.0, 14, 15),
            CompactBreakpoint::discretionary(76.0, 3.5, 2.31, 1.0, 50.0, 16, 16),
            CompactBreakpoint::final_(79.0, 3.5, 2.31, 17),
        ];

        let web = break_compact(&compact, |_| 20.0, 8.0).expect("web path");
        let book = break_compact_book(&compact, |_| 20.0, 8.0)
            .expect("the normal-boundary reset should make a strict book path feasible");
        assert_ne!(
            web, book,
            "book quality must alter a long but avoidable hyphen chain"
        );
        assert_eq!(web, vec![0, 3, 6, 9, 10]);
        assert!(
            book.windows(3)
                .all(|window| !(compact[window[0]].is_hyphen()
                    && compact[window[1]].is_hyphen()
                    && compact[window[2]].is_hyphen()))
        );
    }

    #[test]
    fn nonprinting_discretionary_opportunities_do_not_count_as_hyphens() {
        let opportunity = CompactBreakpoint::discretionary(20.0, 0.0, 0.0, 0.0, 50.0, 1, 1);
        let hyphen = CompactBreakpoint::discretionary(20.0, 0.0, 0.0, 2.0, 50.0, 1, 1);

        assert!(!opportunity.is_hyphen());
        assert!(hyphen.is_hyphen());
    }

    #[test]
    fn short_hyphenated_lines_receive_a_strong_book_quality_penalty() {
        assert_eq!(short_hyphenated_line_demerits(50.0, 100.0), 0.0);
        assert!(short_hyphenated_line_demerits(30.0, 100.0) >= SHORT_HYPHENATED_LINE_DEMERITS);
        assert!(
            short_hyphenated_line_demerits(10.0, 100.0)
                > short_hyphenated_line_demerits(30.0, 100.0)
        );
    }

    #[test]
    fn ragged_composition_avoids_a_hyphen_on_the_penultimate_line() {
        let compact = vec![
            CompactBreakpoint::discretionary(30.0, 0.0, 0.0, 2.0, 0.001, 1, 1),
            ordinary_soft(31.0, 0.0, 0.0, 0.0, 2, 2),
            CompactBreakpoint::final_(60.0, 0.0, 0.0, 3),
        ];

        assert_eq!(
            break_compact_ragged(&compact, |_| 40.0),
            Some(vec![1, 2]),
            "a nearby word boundary should beat a hyphen immediately before the final line"
        );
    }
}
