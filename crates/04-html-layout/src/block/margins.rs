use html_engine_model::{BoxFacts, BoxType, Children};
use html_style_model::{Clear, Float, UsedPreferredSize as PreferredSize, UsedStyleView};

use crate::engine::{LayoutReadContext, LayoutReader};
use crate::{BoxLayoutRequest, LayoutEngine, ResolvedBoxSizing};

/// One adjoining set of vertical margins.
///
/// CSS collapses the largest positive margin with the most-negative margin;
/// summing every margin would be incorrect for chains of three or more boxes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MarginStrut {
    largest_positive: f64,
    smallest_negative: f64,
}

impl MarginStrut {
    pub(crate) fn from_margin(margin: f64) -> Self {
        let mut result = Self::default();
        result.add(margin);
        result
    }

    pub(super) fn add(&mut self, margin: f64) {
        if margin >= 0.0 {
            self.largest_positive = self.largest_positive.max(margin);
        } else {
            self.smallest_negative = self.smallest_negative.min(margin);
        }
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.largest_positive = self.largest_positive.max(other.largest_positive);
        self.smallest_negative = self.smallest_negative.min(other.smallest_negative);
    }

    pub(super) fn resolve(self) -> f64 {
        self.largest_positive + self.smallest_negative
    }

    pub(super) fn is_empty(self) -> bool {
        self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FlowParticipation {
    InFlow,
    Float,
    OutOfFlow,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct MarginProfile {
    pub(super) before: MarginStrut,
    /// The before strut accumulated before the first leading clearance
    /// candidate. A matching active float selects this alternative.
    pub(super) before_clear: MarginStrut,
    pub(super) after: MarginStrut,
    pub(super) own_before: f64,
    pub(super) own_after: f64,
    pub(super) participation: FlowParticipation,
    pub(super) collapses_through: bool,
    /// Float sides whose source remains adjoining through this box's open
    /// before edge. Two bits are enough and let cached margin analysis stop a
    /// matching later `clear` before its margin escapes through ancestors.
    adjoining_float_sides: u8,
    /// Clear sides on this box or its first in-flow descendant through an
    /// open before edge. Ancestors compare this with preceding adjoining
    /// floats before accepting the exposed before-margin strut.
    leading_clear_sides: u8,
    /// First BFC reached through the open before edge, when a float remains
    /// adjoining before it. The box index avoids storing a per-profile list;
    /// layout can resolve this one candidate against the actual inline size.
    leading_bfc_box: u32,
    leading_bfc_float_sides: u8,
}

impl MarginProfile {
    fn own(before: f64, after: f64, participation: FlowParticipation) -> Self {
        Self {
            before: MarginStrut::from_margin(before),
            before_clear: MarginStrut::from_margin(before),
            after: MarginStrut::from_margin(after),
            own_before: before,
            own_after: after,
            participation,
            collapses_through: false,
            adjoining_float_sides: 0,
            leading_clear_sides: 0,
            leading_bfc_box: NO_LEADING_BFC,
            leading_bfc_float_sides: 0,
        }
    }

    pub(super) fn leading_clear(self) -> Clear {
        match self.leading_clear_sides {
            ADJOINING_LEFT_FLOAT => Clear::Left,
            ADJOINING_RIGHT_FLOAT => Clear::Right,
            sides if sides == ADJOINING_LEFT_FLOAT | ADJOINING_RIGHT_FLOAT => Clear::Both,
            _ => Clear::None,
        }
    }

    pub(super) fn leading_bfc_to_probe(self, has_active_float: bool) -> Option<usize> {
        (self.leading_bfc_box != NO_LEADING_BFC
            && (self.leading_bfc_float_sides != 0 || has_active_float))
            .then_some(self.leading_bfc_box as usize)
    }

    pub(super) fn adjoining_float_sides(self) -> u8 {
        self.adjoining_float_sides
    }
}

pub(super) fn clear_matches_float_sides(clear: Clear, sides: u8) -> bool {
    clear_side_mask(clear) & sides != 0
}

const ADJOINING_LEFT_FLOAT: u8 = 1;
const ADJOINING_RIGHT_FLOAT: u8 = 2;
const NO_LEADING_BFC: u32 = u32::MAX;

fn float_side_mask(float: Float) -> u8 {
    match float {
        Float::Left => ADJOINING_LEFT_FLOAT,
        Float::Right => ADJOINING_RIGHT_FLOAT,
        Float::None => 0,
    }
}

fn clear_side_mask(clear: Clear) -> u8 {
    match clear {
        Clear::None => 0,
        Clear::Left => ADJOINING_LEFT_FLOAT,
        Clear::Right => ADJOINING_RIGHT_FLOAT,
        Clear::Both => ADJOINING_LEFT_FLOAT | ADJOINING_RIGHT_FLOAT,
    }
}

fn inline_item_float_side_mask(
    read: &LayoutReadContext<'_>,
    box_idx: usize,
    runs: &std::ops::Range<u32>,
) -> u8 {
    let (left, right) = read.inline_item_float_sides(box_idx, runs);
    u8::from(left) * ADJOINING_LEFT_FLOAT | u8::from(right) * ADJOINING_RIGHT_FLOAT
}

#[derive(Clone, Copy)]
pub(super) struct ParentCollapseContext {
    pub(super) top_open: bool,
    bottom_open: bool,
}

pub(crate) fn physical_margin_edges(style: UsedStyleView<'_>) -> (bool, bool) {
    (
        style.padding_top().is_zero() && style.border_top_width() == 0.0,
        style.padding_bottom().is_zero() && style.border_bottom_width() == 0.0,
    )
}

pub(super) fn parent_collapse_context(
    reader: &LayoutReader<'_>,
    parent_idx: usize,
) -> ParentCollapseContext {
    let style = reader.style(parent_idx);
    let establishes_context = reader.box_facts(parent_idx).formatting_context;
    let (before, after) = physical_margin_edges(style);
    ParentCollapseContext {
        top_open: !establishes_context && before,
        bottom_open: !establishes_context
            && after
            && matches!(style.height(), PreferredSize::Auto)
            && style.aspect_ratio().preferred().is_none(),
    }
}

/// Returns the vertical margins that participate in stretch-fit sizing.
///
/// CSS Sizing treats a margin adjoining an open containing-block edge as
/// zero for this calculation. Inline formatting roots, floats, flex/grid
/// items, and formatting-context roots keep both margins because their
/// margins cannot adjoin the parent's content edges.
pub(crate) fn stretch_margin_inset(
    reader: &LayoutReader<'_>,
    box_idx: usize,
    cb_width: f64,
) -> f64 {
    let (before, after) = reader.vertical_margins(box_idx, cb_width);
    let both = before + after;
    if reader.box_facts(box_idx).absolute
        || reader.box_facts(box_idx).float != Float::None
        || reader.box_facts(box_idx).atomic_inline
        || reader.box_facts(box_idx).flex_grid_item
    {
        return both;
    }

    let Some(parent_idx) = reader.get_parent(box_idx) else {
        return both;
    };
    let parent = reader.style(parent_idx);
    if reader.box_facts(parent_idx).formatting_context {
        return both;
    }

    let (before_open, after_open) = physical_margin_edges(parent);
    let before = if before_open { 0.0 } else { before };
    let after = if after_open { 0.0 } else { after };
    before + after
}

pub(super) fn collapse_before_child(
    pending: &mut MarginStrut,
    profile: MarginProfile,
    margin_adjoins_parent_top: bool,
    has_clearance: bool,
) -> (bool, f64) {
    let collapses_with_parent = margin_adjoins_parent_top && !has_clearance;
    if !collapses_with_parent && !has_clearance {
        pending.merge(profile.before);
    }
    let offset = if !profile.collapses_through && !collapses_with_parent {
        pending.resolve()
    } else {
        0.0
    };
    if !profile.collapses_through {
        *pending = MarginStrut::default();
    }
    (collapses_with_parent, offset)
}

pub(super) fn remaining_parent_margin(
    pending: MarginStrut,
    parent: ParentCollapseContext,
    clearance_consumed_margin: Option<f64>,
) -> f64 {
    if let Some(consumed_margin) = clearance_consumed_margin {
        pending.resolve() - consumed_margin
    } else if pending.is_empty() || parent.bottom_open {
        0.0
    } else {
        pending.resolve()
    }
}

fn participation(facts: &BoxFacts) -> FlowParticipation {
    if facts.absolute {
        FlowParticipation::OutOfFlow
    } else if facts.float != Float::None {
        FlowParticipation::Float
    } else {
        FlowParticipation::InFlow
    }
}

pub(super) fn permits_zero_height_collapse(size: PreferredSize) -> bool {
    matches!(
        size,
        PreferredSize::Auto | PreferredSize::Px(0.0) | PreferredSize::Percent(0.0)
    )
}

#[derive(Clone, Copy)]
struct CachedMarginProfile {
    basis_key: (u64, Option<u64>),
    profile: MarginProfile,
}

#[derive(Default)]
pub(crate) struct MarginAnalysis {
    profiles: Vec<Option<CachedMarginProfile>>,
}

impl MarginAnalysis {
    pub(crate) fn reset(&mut self) {
        self.profiles.clear();
    }
}

/// Computes the vertical margin set exposed by this box to its parent.
/// Descendant margins cross an edge only when that edge is open; floats are
/// excluded from the adjoining set entirely. `basis` carries the containing
/// block's content width and optional definite height.
pub(super) fn margin_profile(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    basis: (f64, Option<f64>),
) -> MarginProfile {
    let basis_key = (basis.0.to_bits(), basis.1.map(f64::to_bits));
    if let Some(Some(cached)) = engine.scratch.margins.profiles.get(box_idx)
        && cached.basis_key == basis_key
    {
        return cached.profile;
    }

    let profile = compute_margin_profile(engine, box_idx, basis);
    if engine.scratch.margins.profiles.len() <= box_idx {
        engine.scratch.margins.profiles.resize(box_idx + 1, None);
    }
    engine.scratch.margins.profiles[box_idx] = Some(CachedMarginProfile { basis_key, profile });
    profile
}

fn compute_margin_profile(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    basis: (f64, Option<f64>),
) -> MarginProfile {
    let reader = engine.read.reader;
    let style = reader.style(box_idx);
    let (own_before, own_after) = reader.vertical_margins(box_idx, basis.0);
    let mut profile = MarginProfile::own(
        own_before,
        own_after,
        participation(reader.box_facts(box_idx)),
    );
    if profile.participation != FlowParticipation::OutOfFlow {
        profile.leading_clear_sides = clear_side_mask(style.clear());
    }
    let (children, runs, anonymous, empty) = match reader.box_layout_mode(box_idx) {
        Some(BoxType::Anonymous(_)) => (None, None, true, false),
        Some(BoxType::Block(Children::Blocks(children))) => (
            Some(reader.layout_tree().child_slice(children)),
            None,
            false,
            false,
        ),
        Some(BoxType::Block(Children::InlineItems(runs))) => (
            None,
            Some(runs),
            false,
            !engine.read.inline_items_establish_line_box(box_idx, runs),
        ),
        Some(BoxType::Block(Children::Empty)) => (None, None, false, true),
        _ => (None, None, false, false),
    };
    if anonymous {
        return MarginProfile::own(own_before, own_after, FlowParticipation::InFlow);
    }
    if profile.participation == FlowParticipation::Float {
        profile.before_clear = MarginStrut::default();
        profile.adjoining_float_sides = float_side_mask(reader.box_facts(box_idx).float);
        return profile;
    }
    let establishes_context = reader.box_facts(box_idx).formatting_context;
    let edges = parent_collapse_context(&reader, box_idx);
    let can_collapse = physical_margin_edges(style) == (true, true)
        && !establishes_context
        && permits_zero_height_collapse(style.height())
        && permits_zero_height_collapse(style.min_height())
        && !(matches!(style.height(), PreferredSize::Auto)
            && style.aspect_ratio().preferred().is_some())
        && (empty || children.is_some());
    // Only open block edges expose descendant margins. Use ordinary sizing
    // for their containing block, including constraints and ratio transfers.
    let descendant_basis = if children.is_some() && (edges.top_open || edges.bottom_open) {
        let resolved =
            engine.resolve_box_sizing(BoxLayoutRequest::normal(box_idx, basis.0, basis.1));
        (
            resolved.horizontal.content,
            resolved.vertical.descendant_basis,
        )
    } else {
        basis
    };
    if can_collapse {
        if let Some(runs) = runs {
            profile.adjoining_float_sides =
                inline_item_float_side_mask(&engine.read, box_idx, runs);
        }
        if let Some(collapsed) =
            collapse_through_profile(engine, descendant_basis, profile, children)
        {
            return collapsed;
        }
    }
    open_edge_profile(
        engine,
        box_idx,
        descendant_basis,
        profile,
        children,
        edges,
        establishes_context,
    )
}

fn collapse_through_profile(
    engine: &mut LayoutEngine<'_, '_>,
    descendant_basis: (f64, Option<f64>),
    mut profile: MarginProfile,
    children: Option<&[u32]>,
) -> Option<MarginProfile> {
    let mut preceding_float_sides = 0;
    let mut adjoining = profile.before;
    adjoining.merge(profile.after);
    let mut before_candidate = profile.before;
    let mut before_clear = (profile.leading_clear_sides != 0).then_some(MarginStrut::default());
    for &child_idx in children.unwrap_or_default() {
        let child = margin_profile(engine, child_idx as usize, descendant_basis);
        if child.participation == FlowParticipation::Float {
            profile.adjoining_float_sides |= child.adjoining_float_sides;
            preceding_float_sides |= child.adjoining_float_sides;
            continue;
        }
        if child.leading_clear_sides & preceding_float_sides != 0 || !child.collapses_through {
            return None;
        }
        preceding_float_sides |= child.adjoining_float_sides;
        if child.participation == FlowParticipation::InFlow {
            if profile.leading_clear_sides == 0 && child.leading_clear_sides != 0 {
                profile.leading_clear_sides = child.leading_clear_sides;
                let mut separated = before_candidate;
                separated.merge(child.before_clear);
                before_clear = Some(separated);
            }
            adjoining.merge(child.before);
            adjoining.merge(child.after);
            before_candidate.merge(child.before);
            before_candidate.merge(child.after);
            profile.adjoining_float_sides |= child.adjoining_float_sides;
        }
    }
    profile.before = adjoining;
    profile.after = adjoining;
    profile.before_clear = before_clear.unwrap_or(adjoining);
    profile.collapses_through = true;
    Some(profile)
}

fn open_edge_profile(
    engine: &mut LayoutEngine<'_, '_>,
    box_idx: usize,
    descendant_basis: (f64, Option<f64>),
    profile: MarginProfile,
    block_children: Option<&[u32]>,
    edges: ParentCollapseContext,
    establishes_context: bool,
) -> MarginProfile {
    let mut before = MarginStrut::from_margin(profile.own_before);
    let mut adjoining_float_sides = 0;
    let mut leading_clear_sides = profile.leading_clear_sides;
    let mut leading_bfc_box = if establishes_context {
        box_idx as u32
    } else {
        NO_LEADING_BFC
    };
    let mut leading_bfc_float_sides = 0;
    let mut before_clear =
        (profile.leading_clear_sides != 0 || establishes_context).then_some(MarginStrut::default());
    if edges.top_open
        && let Some(children) = block_children
    {
        for child in children {
            let child_idx = *child as usize;
            let child = margin_profile(engine, child_idx, descendant_basis);
            if matches!(child.participation, FlowParticipation::Float) {
                adjoining_float_sides |= child.adjoining_float_sides;
                continue;
            }
            if child.leading_clear_sides & adjoining_float_sides != 0 {
                break;
            }
            if leading_clear_sides == 0 {
                leading_clear_sides = child.leading_clear_sides;
                if leading_clear_sides != 0 {
                    let mut separated = before;
                    separated.merge(child.before_clear);
                    before_clear = Some(separated);
                }
            }
            if leading_bfc_box == NO_LEADING_BFC && child.leading_bfc_box != NO_LEADING_BFC {
                leading_bfc_box = child.leading_bfc_box;
                leading_bfc_float_sides = child.leading_bfc_float_sides | adjoining_float_sides;
                let mut separated = before;
                separated.merge(child.before_clear);
                before_clear = Some(separated);
            }
            before.merge(child.before);
            if child.collapses_through {
                before.merge(child.after);
                adjoining_float_sides |= child.adjoining_float_sides;
                continue;
            }
            break;
        }
    }

    let mut after = MarginStrut::from_margin(profile.own_after);
    if edges.bottom_open
        && let Some(children) = block_children
    {
        for child in children.iter().rev() {
            let child = margin_profile(engine, *child as usize, descendant_basis);
            if matches!(child.participation, FlowParticipation::Float) {
                continue;
            }
            after.merge(child.after);
            if child.collapses_through {
                after.merge(child.before);
                continue;
            }
            break;
        }
    }

    MarginProfile {
        before,
        before_clear: before_clear.unwrap_or(before),
        after,
        collapses_through: false,
        adjoining_float_sides,
        leading_clear_sides,
        leading_bfc_box,
        leading_bfc_float_sides,
        ..profile
    }
}

/// Resolve the parent's trailing margin from the completed flow result.
pub(super) fn used_after_margin(
    resolved: &ResolvedBoxSizing,
    parent: ParentCollapseContext,
    natural_height: f64,
    trailing_margin: MarginStrut,
) -> MarginStrut {
    let minimum_blocks_child = resolved
        .vertical
        .min
        .resolve(Some(natural_height))
        .is_some_and(|minimum| natural_height < minimum);
    let maximum_blocks_child = resolved
        .vertical
        .max
        .resolve(Some(natural_height))
        .is_some_and(|maximum| natural_height > maximum);
    let mut after = MarginStrut::from_margin(resolved.resolved_bottom_margin);
    if parent.bottom_open && !minimum_blocks_child && !maximum_blocks_child {
        after.merge(trailing_margin);
    }
    after
}

#[cfg(test)]
mod tests {
    use super::*;
    use html_style_model::{Display, OverflowMode};

    #[test]
    fn adjoining_margins_keep_only_the_extreme_positive_and_negative_values() {
        let mut strut = MarginStrut::default();
        for margin in [10.0, 30.0, -4.0, -12.0, 20.0] {
            strut.add(margin);
        }
        assert_eq!(strut.resolve(), 18.0);
    }

    #[test]
    fn overflow_clip_does_not_create_a_formatting_context_by_itself() {
        assert!(
            !Display::Block
                .establishes_formatting_context((OverflowMode::Clip, OverflowMode::Visible))
        );
        assert!(
            Display::Block
                .establishes_formatting_context((OverflowMode::Hidden, OverflowMode::Visible))
        );
    }

    #[test]
    fn explicit_zero_height_can_collapse_through_but_positive_height_cannot() {
        assert!(permits_zero_height_collapse(PreferredSize::Auto));
        assert!(permits_zero_height_collapse(PreferredSize::Px(0.0)));
        assert!(!permits_zero_height_collapse(PreferredSize::Px(1.0)));
    }
}
