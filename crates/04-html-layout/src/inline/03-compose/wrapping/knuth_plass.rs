//! Compact Knuth–Plass optimization over cumulative token breakpoints.
//!
//! The solver is independent of glyphs, styles, and the layout tree. Inline
//! token construction supplies compact cumulative breakpoints, and the solver
//! returns their selected indices.

use crate::inline::justification::tracking_width_limit;
use std::cmp::Ordering;
use std::mem::size_of;

/// Penalties at or beyond this magnitude are never selected.
pub(super) const INF_PENALTY: f64 = 10_000.0;

/// Extra demerits charged when two consecutive lines differ sharply in
/// tightness (Knuth's adjacent-fitness penalty).
const FITNESS_PENALTY: f64 = 3000.0;
const PENULTIMATE_HYPHEN_PENALTY: f64 = 2_500.0;
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

/// Keep candidates in construction order without a compression pass or
/// indirect metric lookups in the solver.
pub(super) struct BreakpointStorage(Box<[CompactBreakpoint]>);

impl BreakpointStorage {
    pub(super) fn new(values: Vec<CompactBreakpoint>) -> Self {
        Self(values.into_boxed_slice())
    }

    pub(super) fn get(&self, index: usize) -> Option<CompactBreakpoint> {
        self.0.get(index).copied()
    }

    fn iter(&self) -> impl ExactSizeIterator<Item = CompactBreakpoint> + Clone + '_ {
        self.0.iter().copied()
    }

    pub(super) fn memory_usage_bytes(&self) -> usize {
        self.0.len() * size_of::<CompactBreakpoint>()
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

    pub(super) fn is_discretionary(&self) -> bool {
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

    fn next_line_totals(&self) -> (f64, f64, f64) {
        if self.is_discretionary() {
            (self.width, self.stretch, self.shrink)
        } else {
            (
                self.width + self.discard_width,
                self.stretch + self.discarded_stretch(),
                self.shrink + self.discarded_shrink(),
            )
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
    previous_hyphen: bool,
    // Occupies padding in the 48-byte active record.
    tracking_start: u32,
}

/// A winning slot needs only its score and the parent identifiers. The next
/// active node takes all geometry from the current breakpoint, not its parent.
#[derive(Clone, Copy)]
struct CompactBestNode {
    demerits: f64,
    previous_path: u32,
    previous_line: u32,
    previous_hyphen: bool,
}

#[derive(Clone, Copy)]
struct CompactBestParent {
    path: u32,
    line: u32,
}

/// Scores are the only column read while comparing a populated slot. Parent
/// identifiers and the previous-line hyphen flag are loaded only when
/// emitting a winning path.
struct CompactBestNodes<const N: usize> {
    demerits: [f64; N],
    parents: [CompactBestParent; N],
    hyphenated: [bool; N],
    occupied: u16,
}

const _: () = assert!(size_of::<CompactBestNodes<4>>() <= 80);
const _: () = assert!(size_of::<CompactBestNodes<8>>() <= 144);

impl<const N: usize> CompactBestNodes<N> {
    fn new() -> Self {
        assert!(N <= u16::BITS as usize);
        Self {
            demerits: [0.0; N],
            parents: [CompactBestParent { path: 0, line: 0 }; N],
            hyphenated: [false; N],
            occupied: 0,
        }
    }

    #[inline(always)]
    fn consider(
        &mut self,
        slot: usize,
        demerits: f64,
        previous_path: u32,
        previous_line: u32,
        previous_hyphen: bool,
    ) {
        let bit = 1u16 << slot;
        // Match Option::is_none_or with a strict comparison, including NaNs.
        if self.occupied & bit == 0 || demerits < self.demerits[slot] {
            self.occupied |= bit;
            self.demerits[slot] = demerits;
            self.parents[slot] = CompactBestParent {
                path: previous_path,
                line: previous_line,
            };
            self.hyphenated[slot] = previous_hyphen;
        }
    }

    fn iter(&self) -> impl Iterator<Item = (usize, CompactBestNode)> + '_ {
        let mut occupied = self.occupied;
        std::iter::from_fn(move || {
            if occupied == 0 {
                return None;
            }
            // Preserve ascending bucket order while skipping empty slots.
            let slot = occupied.trailing_zeros() as usize;
            occupied &= occupied - 1;
            let parent = self.parents[slot];
            Some((
                slot,
                CompactBestNode {
                    demerits: self.demerits[slot],
                    previous_path: parent.path,
                    previous_line: parent.line,
                    previous_hyphen: self.hyphenated[slot],
                },
            ))
        })
    }
}

struct CompactPathNode {
    breakpoint: u32,
    previous: u32,
}

const NO_COMPACT_PATH: u32 = u32::MAX;

/// Width-dependent solver storage retained between paragraphs and resizes.
#[derive(Default)]
pub(in crate::inline) struct CompactScratch {
    paths: Vec<CompactPathNode>,
    active: Vec<CompactActiveNode>,
    breaks: Vec<usize>,
}

impl CompactScratch {
    fn collect_best_breaks(&mut self) -> Option<&[usize]> {
        let best = self.active.iter().min_by(|a, b| {
            a.demerits
                .partial_cmp(&b.demerits)
                .unwrap_or(Ordering::Equal)
        })?;
        let mut path = best.path;
        while path != NO_COMPACT_PATH {
            let node = &self.paths[path as usize];
            self.breaks.push(node.breakpoint as usize);
            path = node.previous;
        }
        self.breaks.reverse();
        Some(self.breaks.as_slice())
    }

    #[cfg(any(test, feature = "testing"))]
    pub(in crate::inline) fn capacities(&self) -> (usize, usize, usize) {
        (
            self.paths.capacity(),
            self.active.capacity(),
            self.breaks.capacity(),
        )
    }
}

#[cfg(any(test, feature = "testing"))]
pub(super) fn break_compact_reusing<'s>(
    breakpoints: &BreakpointStorage,
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
    ragged: bool,
    quality: bool,
    scratch: &'s mut CompactScratch,
) -> Option<&'s [usize]> {
    break_compact_reusing_with_tracking(
        breakpoints,
        line_width,
        tolerance,
        ragged,
        quality,
        &mut |_, _, _, _| false,
        scratch,
    )
}

pub(super) fn break_compact_reusing_with_tracking<'s>(
    breakpoints: &BreakpointStorage,
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
    ragged: bool,
    quality: bool,
    admit_tracking: &mut impl FnMut(u32, u32, f64, f64) -> bool,
    scratch: &'s mut CompactScratch,
) -> Option<&'s [usize]> {
    break_compact_iter(
        breakpoints.iter(),
        line_width,
        tolerance,
        ragged,
        quality,
        admit_tracking,
        scratch,
    )
}

fn break_compact_iter<'s>(
    breakpoints: impl ExactSizeIterator<Item = CompactBreakpoint> + Clone,
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
    ragged: bool,
    quality: bool,
    admit_tracking: &mut impl FnMut(u32, u32, f64, f64) -> bool,
    scratch: &'s mut CompactScratch,
) -> Option<&'s [usize]> {
    if ragged {
        break_compact_ragged_with_mode(breakpoints, &line_width, quality, admit_tracking, scratch)
    } else {
        if break_compact_with_mode(
            breakpoints.clone(),
            &line_width,
            tolerance,
            false,
            quality,
            admit_tracking,
            scratch,
        )
        .is_some()
        {
            return Some(&scratch.breaks);
        }
        break_compact_with_mode(
            breakpoints,
            &line_width,
            tolerance,
            true,
            quality,
            admit_tracking,
            scratch,
        )
    }
}

/// Knuth–Plass over the compact legal-break stream used by inline layout.
///
/// The active records contain every value read by the inner loop and are
/// compacted in place. This avoids the old active-index gathers into a much
/// larger append-only `Node` arena and exposes contiguous numeric columns to
/// the optimizer.
#[cfg(any(test, feature = "testing"))]
pub(super) fn break_compact(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
) -> Option<Vec<usize>> {
    break_compact_with_mode(
        BreakpointStorage::new(breakpoints.to_vec()).iter(),
        line_width,
        tolerance,
        false,
        false,
        &mut |_, _, _, _| false,
        &mut CompactScratch::default(),
    )
    .map(<[usize]>::to_vec)
}

#[cfg(any(test, feature = "testing"))]
pub(super) fn break_compact_emergency(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
) -> Option<Vec<usize>> {
    break_compact_with_mode(
        BreakpointStorage::new(breakpoints.to_vec()).iter(),
        line_width,
        tolerance,
        true,
        false,
        &mut |_, _, _, _| false,
        &mut CompactScratch::default(),
    )
    .map(<[usize]>::to_vec)
}

#[cfg(any(test, feature = "testing"))]
pub(super) fn break_compact_book(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
) -> Option<Vec<usize>> {
    break_compact_with_mode(
        BreakpointStorage::new(breakpoints.to_vec()).iter(),
        line_width,
        tolerance,
        false,
        true,
        &mut |_, _, _, _| false,
        &mut CompactScratch::default(),
    )
    .map(<[usize]>::to_vec)
}

#[cfg(any(test, feature = "testing"))]
pub(super) fn break_compact_emergency_book(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
) -> Option<Vec<usize>> {
    break_compact_with_mode(
        BreakpointStorage::new(breakpoints.to_vec()).iter(),
        line_width,
        tolerance,
        true,
        true,
        &mut |_, _, _, _| false,
        &mut CompactScratch::default(),
    )
    .map(<[usize]>::to_vec)
}

/// Paragraph-wide composition for fixed-spacing ragged text. Unlike justified
/// Knuth--Plass, unused line width is intentional and becomes the badness
/// being minimized; word spaces retain their natural advances.
#[cfg(any(test, feature = "testing"))]
pub(super) fn break_compact_ragged(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
) -> Option<Vec<usize>> {
    break_compact_reusing(
        &BreakpointStorage::new(breakpoints.to_vec()),
        line_width,
        1.0,
        true,
        true,
        &mut CompactScratch::default(),
    )
    .map(<[usize]>::to_vec)
}

fn break_compact_ragged_with_mode<'s>(
    breakpoints: impl ExactSizeIterator<Item = CompactBreakpoint>,
    line_width: &impl Fn(usize) -> f64,
    hyphenation_quality: bool,
    admit_overhang: &mut impl FnMut(u32, u32, f64, f64) -> bool,
    scratch: &'s mut CompactScratch,
) -> Option<&'s [usize]> {
    scratch.paths.clear();
    scratch.active.clear();
    scratch.breaks.clear();
    if breakpoints.len() == 0 {
        return Some(&scratch.breaks);
    }
    let CompactScratch { paths, active, .. } = scratch;
    active.push(CompactActiveNode {
        path: NO_COMPACT_PATH,
        line: 0,
        width: 0.0,
        stretch: 0.0,
        shrink: 0.0,
        demerits: 0.0,
        fitness: 1,
        previous_hyphen: false,
        tracking_start: 0,
    });

    let breakpoint_count = breakpoints.len();
    for (breakpoint_index, breakpoint) in breakpoints.enumerate() {
        let forced = breakpoint_index + 1 == breakpoint_count;
        // The first line can have a different indent. After it, candidates at
        // this breakpoint are geometrically equivalent, so retain only the
        // cheapest path in each category.
        let mut best = CompactBestNodes::<4>::new();
        let mut kept = 0usize;

        for read in 0..active.len() {
            let candidate = active[read];
            let available = line_width(candidate.line as usize).max(0.0);
            let width = breakpoint.width - candidate.width + breakpoint.penalty_width();
            let fits = width <= available + 1.0e-9
                || (forced
                    && admit_overhang(
                        candidate.tracking_start,
                        breakpoint.line_end() as u32,
                        available,
                        width - available,
                    ));
            let next_hyphen = hyphenation_quality && breakpoint.is_hyphen();
            if fits {
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
                if hyphenation_quality && forced && candidate.previous_hyphen {
                    demerits += PENULTIMATE_HYPHEN_PENALTY;
                }
                if hyphenation_quality && breakpoint.is_hyphen() {
                    demerits += short_hyphenated_line_demerits(width, available);
                }
                let penalty = breakpoint.penalty();
                if penalty > 0.0 && penalty < INF_PENALTY {
                    demerits += penalty * penalty;
                }
                let bucket = usize::from(candidate.line > 0) * 2 + usize::from(next_hyphen);
                best.consider(
                    bucket,
                    demerits,
                    candidate.path,
                    candidate.line,
                    next_hyphen,
                );
            }

            if fits && !forced {
                // Until a candidate is dropped, survivors are already in
                // their final slots. Avoid rewriting their full records.
                if kept != read {
                    active[kept] = candidate;
                }
                kept += 1;
            }
        }
        active.truncate(kept);

        let (next_width, next_stretch, next_shrink) = breakpoint.next_line_totals();
        for (_, candidate) in best.iter() {
            let path = u32::try_from(paths.len()).ok()?;
            paths.push(CompactPathNode {
                breakpoint: breakpoint_index as u32,
                previous: candidate.previous_path,
            });
            active.push(CompactActiveNode {
                path,
                line: candidate.previous_line + 1,
                width: next_width,
                stretch: next_stretch,
                shrink: next_shrink,
                demerits: candidate.demerits,
                fitness: 1,
                previous_hyphen: candidate.previous_hyphen,
                tracking_start: breakpoint.next_line_start,
            });
        }

        if active.is_empty() {
            return None;
        }
    }

    scratch.collect_best_breaks()
}

fn break_compact_with_mode<'s>(
    breakpoints: impl ExactSizeIterator<Item = CompactBreakpoint>,
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
    allow_underfull: bool,
    book_hyphen_quality: bool,
    admit_tracking: &mut impl FnMut(u32, u32, f64, f64) -> bool,
    scratch: &'s mut CompactScratch,
) -> Option<&'s [usize]> {
    scratch.paths.clear();
    scratch.active.clear();
    scratch.breaks.clear();
    if breakpoints.len() == 0 {
        return Some(&scratch.breaks);
    }
    let CompactScratch { paths, active, .. } = scratch;
    active.push(CompactActiveNode {
        path: NO_COMPACT_PATH,
        line: 0,
        width: 0.0,
        stretch: 0.0,
        shrink: 0.0,
        demerits: 0.0,
        fitness: 1,
        previous_hyphen: false,
        tracking_start: 0,
    });

    let breakpoint_count = breakpoints.len();
    for (breakpoint_index, breakpoint) in breakpoints.enumerate() {
        let forced = breakpoint_index + 1 == breakpoint_count;
        let mut best = CompactBestNodes::<8>::new();
        let mut kept = 0usize;

        for read in 0..active.len() {
            let candidate = active[read];
            let available = line_width(candidate.line as usize);
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
                allow_underfull && ratio > tolerance && remaining_after_stretch > 0.0;
            let emergency_tight = allow_underfull
                && ratio < -1.0
                && remaining_after_shrink > 0.0
                && remaining_after_shrink <= tracking_capacity;
            let next_hyphen = book_hyphen_quality && breakpoint.is_hyphen();
            let tracking_fits = tracking_capacity > 0.0
                && ((ratio > tolerance && remaining_after_stretch <= tracking_capacity)
                    || (ratio < -1.0 && remaining_after_shrink <= tracking_capacity));
            let tracking_admitted = !allow_underfull
                && tracking_fits
                && admit_tracking(
                    candidate.tracking_start,
                    breakpoint.line_end,
                    width,
                    if ratio > tolerance {
                        remaining_after_stretch
                    } else {
                        remaining_after_shrink
                    },
                );
            if (ratio >= -1.0 && ratio <= tolerance)
                || emergency_underfull
                || emergency_tight
                || tracking_admitted
            {
                let effective_ratio = ratio.clamp(-1.0, tolerance);
                let badness = 100.0 * effective_ratio.abs().powi(3);
                let mut demerits = candidate.demerits + (1.0 + badness).powi(2);
                if book_hyphen_quality && forced && candidate.previous_hyphen {
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
                let slot = fitness as usize * 2 + usize::from(next_hyphen);
                best.consider(slot, demerits, candidate.path, candidate.line, next_hyphen);
            }

            if (ratio >= -1.0 || tracking_admitted) && !forced {
                if kept != read {
                    active[kept] = candidate;
                }
                kept += 1;
            }
        }
        active.truncate(kept);

        let (next_width, next_stretch, next_shrink) = breakpoint.next_line_totals();
        for (slot, candidate) in best.iter() {
            let path = u32::try_from(paths.len()).ok()?;
            paths.push(CompactPathNode {
                breakpoint: breakpoint_index as u32,
                previous: candidate.previous_path,
            });
            active.push(CompactActiveNode {
                path,
                line: candidate.previous_line + 1,
                width: next_width,
                stretch: next_stretch,
                shrink: next_shrink,
                demerits: candidate.demerits,
                fitness: (slot / 2) as i8,
                previous_hyphen: candidate.previous_hyphen,
                tracking_start: breakpoint.next_line_start,
            });
        }

        if active.is_empty() {
            return None;
        }
    }

    scratch.collect_best_breaks()
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
        let (discard_stretch, discard_shrink) = crate::inline::justification::glue_capacities(
            space,
            crate::inline::justification::SpaceGlueClass::Ordinary,
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
    }

    #[test]
    fn distinct_breakpoint_metrics_keep_direct_storage() {
        let values: Vec<_> = (0..64)
            .map(|index| {
                CompactBreakpoint::soft(
                    index as f64 * 30.0,
                    0.0,
                    0.0,
                    index as f64 + 0.125,
                    1.0,
                    0.5,
                    index,
                    index + 1,
                )
            })
            .collect();
        let stored = BreakpointStorage::new(values);
        assert_eq!(
            stored.memory_usage_bytes(),
            64 * size_of::<CompactBreakpoint>()
        );
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
    fn consecutive_hyphens_remain_feasible_in_all_composition_paths() {
        let compact = vec![
            CompactBreakpoint::discretionary(30.0, 0.0, 0.0, 2.0, 50.0, 1, 1),
            CompactBreakpoint::discretionary(60.0, 0.0, 0.0, 2.0, 50.0, 2, 2),
            CompactBreakpoint::discretionary(90.0, 0.0, 0.0, 2.0, 50.0, 3, 3),
            CompactBreakpoint::final_(120.0, 0.0, 0.0, 4),
        ];

        assert_eq!(
            break_compact_book(&compact, |_| 32.0, 8.0),
            Some(vec![0, 1, 2, 3]),
            "the normal justified pass must allow consecutive hyphens"
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
            "ragged composition must allow consecutive hyphens"
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
