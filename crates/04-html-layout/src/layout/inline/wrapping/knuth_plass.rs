//! Compact Knuth–Plass optimization over cumulative token breakpoints.
//!
//! The solver is independent of glyphs, styles, and the layout tree. Inline
//! token construction supplies compact cumulative breakpoints, and the solver
//! returns their selected indices.

use std::cmp::Ordering;

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
const TRACKING_LINE_DEMERITS: f64 = 25_000.0;
pub(super) const MICRO_TRACKING_FRACTION: f64 = 0.005;

fn underfull_demerits(remaining: f64, available: f64) -> f64 {
    let fraction = remaining.max(0.0) / available.max(1.0);
    UNDERFULL_LINE_DEMERITS + (fraction * 1000.0).powi(2)
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
}

struct CompactPathNode {
    breakpoint: u32,
    previous: u32,
}

const NO_COMPACT_PATH: u32 = u32::MAX;

/// Knuth–Plass over the compact legal-break stream used by inline layout.
///
/// The active records contain every value read by the inner loop and are
/// compacted in place. This avoids the old active-index gathers into a much
/// larger append-only `Node` arena and exposes contiguous numeric columns to
/// the optimizer.
pub(super) fn break_compact(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
) -> Option<Vec<usize>> {
    break_compact_with_mode(breakpoints, line_width, tolerance, false, false)
}

pub(super) fn break_compact_emergency(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
) -> Option<Vec<usize>> {
    break_compact_with_mode(breakpoints, line_width, tolerance, true, false)
}

pub(super) fn break_compact_book(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
) -> Option<Vec<usize>> {
    break_compact_with_mode(breakpoints, line_width, tolerance, false, true)
}

pub(super) fn break_compact_emergency_book(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
) -> Option<Vec<usize>> {
    break_compact_with_mode(breakpoints, line_width, tolerance, true, true)
}

/// Paragraph-wide composition for fixed-spacing ragged text. Unlike justified
/// Knuth--Plass, unused line width is intentional and becomes the badness
/// being minimized; word spaces retain their natural advances.
pub(super) fn break_compact_ragged(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
) -> Option<Vec<usize>> {
    break_compact_ragged_with_mode(breakpoints, &line_width, false, true)
        .or_else(|| break_compact_ragged_with_mode(breakpoints, &line_width, true, true))
}

pub(super) fn break_compact_ragged_unrestricted(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
) -> Option<Vec<usize>> {
    break_compact_ragged_with_mode(breakpoints, &line_width, true, false)
}

fn break_compact_ragged_with_mode(
    breakpoints: &[CompactBreakpoint],
    line_width: &impl Fn(usize) -> f64,
    allow_excessive_hyphen_run: bool,
    hyphenation_quality: bool,
) -> Option<Vec<usize>> {
    if breakpoints.is_empty() {
        return Some(Vec::new());
    }

    let mut paths = Vec::<CompactPathNode>::new();
    let mut active = vec![CompactActiveNode {
        path: NO_COMPACT_PATH,
        line: 0,
        width: 0.0,
        stretch: 0.0,
        shrink: 0.0,
        demerits: 0.0,
        fitness: 1,
        hyphen_run: 0,
    }];

    for (breakpoint_index, breakpoint) in breakpoints.iter().enumerate() {
        let forced = breakpoint_index + 1 == breakpoints.len();
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
    let mut breaks = Vec::new();
    while path != NO_COMPACT_PATH {
        let node = &paths[path as usize];
        breaks.push(node.breakpoint as usize);
        path = node.previous;
    }
    breaks.reverse();
    Some(breaks)
}

fn break_compact_with_mode(
    breakpoints: &[CompactBreakpoint],
    line_width: impl Fn(usize) -> f64,
    tolerance: f64,
    allow_underfull: bool,
    book_hyphen_quality: bool,
) -> Option<Vec<usize>> {
    if breakpoints.is_empty() {
        return Some(Vec::new());
    }

    let mut paths = Vec::<CompactPathNode>::new();
    let mut active = vec![CompactActiveNode {
        path: NO_COMPACT_PATH,
        line: 0,
        width: 0.0,
        stretch: 0.0,
        shrink: 0.0,
        demerits: 0.0,
        fitness: 1,
        hyphen_run: 0,
    }];

    for (breakpoint_index, breakpoint) in breakpoints.iter().enumerate() {
        let forced = breakpoint_index + 1 == breakpoints.len();
        let mut best: [Option<(f64, CompactActiveNode, u8)>; 12] = [None; 12];
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
                width.max(0.0) * MICRO_TRACKING_FRACTION
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
            if ((ratio >= -1.0 && ratio <= tolerance) || emergency_underfull || emergency_tight)
                && (!excessive_hyphen_run || allow_underfull)
            {
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
                }
            }

            if ratio >= -1.0 && !forced {
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
        for (slot, candidate) in best.into_iter().enumerate() {
            if let Some((demerits, previous, hyphen_run)) = candidate {
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
                    fitness: (slot / 3) as i8,
                    hyphen_run,
                });
            }
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
    let mut breaks = Vec::new();
    while path != NO_COMPACT_PATH {
        let node = &paths[path as usize];
        breaks.push(node.breakpoint as usize);
        path = node.previous;
    }
    breaks.reverse();
    Some(breaks)
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
        assert_eq!(std::mem::size_of::<CompactBreakpoint>(), 48);
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
