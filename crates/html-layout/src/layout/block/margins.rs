use crate::layout::box_constraints::resolve_vertical_size;
use crate::layout_model::{BoxType, Children};
use html_style_model::{BoxSizing, Clear, Display, Float, OverflowMode, PositionMode, UsedPreferredSize as PreferredSize};

use super::margin_cache::MarginCache;

/// One adjoining set of vertical margins.
///
/// CSS collapses the largest positive margin with the most-negative margin;
/// summing every margin would be incorrect for chains of three or more boxes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct MarginStrut {
    largest_positive: f64,
    smallest_negative: f64,
}

impl MarginStrut {
    pub(super) fn from_margin(margin: f64) -> Self {
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
    pub(super) fn leading_clear(self) -> Clear {
        match self.leading_clear_sides {
            ADJOINING_LEFT_FLOAT => Clear::Left,
            ADJOINING_RIGHT_FLOAT => Clear::Right,
            sides if sides == ADJOINING_LEFT_FLOAT | ADJOINING_RIGHT_FLOAT => Clear::Both,
            _ => Clear::None,
        }
    }

    pub(super) fn leading_bfc_to_probe(self, has_active_float: bool) -> Option<usize> {
        (self.leading_bfc_box != NO_LEADING_BFC && (self.leading_bfc_float_sides != 0 || has_active_float)).then_some(self.leading_bfc_box as usize)
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

fn inline_item_float_side_mask(reader: &crate::layout::read_context::LayoutReader<'_>, box_idx: usize, runs: &std::ops::Range<u32>) -> u8 {
    let (left, right) = reader.inline_item_float_sides(box_idx, runs);
    u8::from(left) * ADJOINING_LEFT_FLOAT | u8::from(right) * ADJOINING_RIGHT_FLOAT
}

#[derive(Clone, Copy)]
pub(super) struct ParentCollapseContext {
    pub(super) top_open: bool,
    bottom_open: bool,
}

pub(super) fn parent_collapse_context(reader: &crate::layout::read_context::LayoutReader<'_>, parent_idx: usize) -> ParentCollapseContext {
    let style = reader.style(parent_idx);
    let is_flex_grid_item = reader.get_parent(parent_idx).is_some_and(|ancestor| matches!(reader.box_layout_mode(ancestor), Some(BoxType::Flex(_) | BoxType::Grid(_))));
    let establishes_context = establishes_formatting_context(
        style.display(),
        style.overflow_x(),
        style.overflow_y(),
        matches!(style.float(), Float::Left | Float::Right),
        reader.root_box() == Some(parent_idx),
        reader.is_table_cell_box(parent_idx),
        is_flex_grid_item,
    );
    ParentCollapseContext {
        top_open: !establishes_context && style.padding_top().is_zero() && style.border_top_width() == 0.0,
        bottom_open: !establishes_context && style.padding_bottom().is_zero() && style.border_bottom_width() == 0.0 && matches!(style.height(), PreferredSize::Auto),
    }
}

pub(super) fn collapse_before_child(pending: &mut MarginStrut, profile: MarginProfile, started_flow: bool, has_clearance: bool, clearance_consumed_before_margin: bool, parent: ParentCollapseContext, is_anonymous: bool) -> (bool, f64) {
    let collapses_with_parent = !started_flow && !has_clearance && parent.top_open && !is_anonymous;
    if !collapses_with_parent && !clearance_consumed_before_margin {
        pending.merge(profile.before);
    }
    let offset = if !profile.collapses_through && !collapses_with_parent { pending.resolve() } else { 0.0 };
    if !profile.collapses_through {
        *pending = MarginStrut::default();
    }
    (collapses_with_parent, offset)
}

pub(super) fn remaining_parent_margin(pending: MarginStrut, parent: ParentCollapseContext, clearance_barrier: bool, clearance_consumed_margin: f64) -> f64 {
    if clearance_barrier {
        pending.resolve() - clearance_consumed_margin
    } else if pending.is_empty() || parent.bottom_open {
        0.0
    } else {
        pending.resolve()
    }
}

pub(super) fn participation(float: Float, position: PositionMode) -> FlowParticipation {
    if position == PositionMode::Absolute {
        FlowParticipation::OutOfFlow
    } else if matches!(float, Float::Left | Float::Right) {
        FlowParticipation::Float
    } else {
        FlowParticipation::InFlow
    }
}

/// A new block formatting context prevents descendant margins from escaping
/// through the box. Its own outer margins may still collapse with in-flow
/// siblings, so this is deliberately separate from `FlowParticipation`.
pub(in crate::layout) fn establishes_formatting_context(display: Display, overflow_x: OverflowMode, overflow_y: OverflowMode, is_float: bool, is_root: bool, is_table_cell: bool, is_flex_grid_item: bool) -> bool {
    is_root
        || is_float
        || is_table_cell
        || is_flex_grid_item
        || matches!(display, Display::FlowRoot | Display::FlowRootListItem | Display::InlineBlock | Display::InlineTable | Display::Flex | Display::InlineFlex | Display::Grid | Display::InlineGrid)
        || establishes_overflow_context(overflow_x)
        || establishes_overflow_context(overflow_y)
}

fn establishes_overflow_context(overflow: OverflowMode) -> bool {
    matches!(overflow, OverflowMode::Hidden | OverflowMode::Scroll | OverflowMode::Auto)
}

pub(super) fn permits_zero_height_collapse(size: PreferredSize) -> bool {
    matches!(size, PreferredSize::Auto | PreferredSize::Px(0.0) | PreferredSize::Percent(0.0))
}

#[derive(Default)]
pub(in crate::layout) struct MarginAnalysis {
    cache: MarginCache,
    natural_content_heights: Vec<Option<f64>>,
}

impl MarginAnalysis {
    pub(in crate::layout) fn reset(&mut self) {
        self.cache.clear();
        self.natural_content_heights.clear();
    }

    pub(crate) fn record_natural_content_height(&mut self, box_idx: usize, height: f64) {
        if self.natural_content_heights.len() <= box_idx {
            self.natural_content_heights.resize(box_idx + 1, None);
        }
        self.natural_content_heights[box_idx] = Some(height);
    }

    pub(crate) fn natural_content_height(&self, box_idx: usize) -> f64 {
        self.natural_content_heights.get(box_idx).and_then(|height| *height).unwrap_or(0.0)
    }

    pub(super) fn record_used_after_margin(&mut self, box_idx: usize, cb_width: f64, parent_content_height: Option<f64>, margin: MarginStrut) {
        let height_key = parent_content_height.map(f64::to_bits).unwrap_or(u64::MAX);
        self.cache.insert_after_margin(box_idx, cb_width.to_bits(), height_key, margin);
    }

    /// Computes the vertical margin set exposed by this box to its parent.
    /// Descendant margins cross an edge only when that edge is open; floats are
    /// excluded from the adjoining set entirely.
    pub(super) fn margin_profile(&mut self, reader: &crate::layout::read_context::LayoutReader<'_>, box_idx: usize, cb_width: f64) -> MarginProfile {
        let width_key = cb_width.to_bits();
        if let Some(profile) = self.cache.profile(box_idx, width_key) {
            return profile;
        }

        let profile = self.compute_margin_profile(reader, box_idx, cb_width);
        self.cache.insert_profile(box_idx, width_key, profile);
        profile
    }

    fn compute_margin_profile(&mut self, reader: &crate::layout::read_context::LayoutReader<'_>, box_idx: usize, cb_width: f64) -> MarginProfile {
        let style = reader.style(box_idx);
        let own_before = style.margin_top().resolve(cb_width);
        let own_after = style.margin_bottom().resolve(cb_width);
        let participation = participation(style.float(), style.position());
        let own_clear_sides = if matches!(participation, FlowParticipation::OutOfFlow) { 0 } else { clear_side_mask(style.clear()) };
        let is_anonymous = matches!(reader.box_layout_mode(box_idx), Some(BoxType::Anonymous(_)));
        if is_anonymous {
            return MarginProfile {
                before: MarginStrut::default(),
                before_clear: MarginStrut::default(),
                after: MarginStrut::default(),
                own_before: 0.0,
                own_after: 0.0,
                participation: FlowParticipation::InFlow,
                collapses_through: false,
                adjoining_float_sides: 0,
                leading_clear_sides: 0,
                leading_bfc_box: NO_LEADING_BFC,
                leading_bfc_float_sides: 0,
            };
        }
        if matches!(participation, FlowParticipation::Float) {
            return MarginProfile {
                before: MarginStrut::from_margin(own_before),
                before_clear: MarginStrut::default(),
                after: MarginStrut::from_margin(own_after),
                own_before,
                own_after,
                participation,
                collapses_through: false,
                adjoining_float_sides: float_side_mask(style.float()),
                leading_clear_sides: own_clear_sides,
                leading_bfc_box: NO_LEADING_BFC,
                leading_bfc_float_sides: 0,
            };
        }

        let is_flex_grid_item = reader.get_parent(box_idx).is_some_and(|parent_idx| matches!(reader.box_layout_mode(parent_idx), Some(BoxType::Flex(_) | BoxType::Grid(_))));
        let establishes_context = establishes_formatting_context(
            style.display(),
            style.overflow_x(),
            style.overflow_y(),
            matches!(style.float(), Float::Left | Float::Right),
            reader.root_box() == Some(box_idx),
            reader.is_table_cell_box(box_idx),
            is_flex_grid_item,
        );
        let physical_before_open = style.padding_top().is_zero() && style.border_top_width() == 0.0;
        let physical_after_open = style.padding_bottom().is_zero() && style.border_bottom_width() == 0.0;
        let children_before_open = physical_before_open && !establishes_context;
        // A parent's used min/max-height does not prevent its last child's
        // bottom margin from adjoining; only a non-auto authored height closes
        // this edge (CSS 2.1 section 8.3.1).
        let children_after_open = physical_after_open && !establishes_context && matches!(style.height(), PreferredSize::Auto);
        let block_children = match reader.box_layout_mode(box_idx) {
            Some(BoxType::Block(block)) => match &block.children {
                Children::Blocks(children) => Some(children.as_slice()),
                _ => None,
            },
            _ => None,
        };
        let inline_item_float_sides = match reader.box_layout_mode(box_idx) {
            Some(BoxType::Block(block)) => match &block.children {
                Children::InlineItems(runs) => inline_item_float_side_mask(reader, box_idx, runs),
                _ => 0,
            },
            _ => 0,
        };

        let children_all_collapse_through = block_children.is_some_and(|children| {
            let mut preceding_float_sides = 0;
            for child in children {
                let profile = self.margin_profile(reader, *child as usize, cb_width);
                if matches!(profile.participation, FlowParticipation::Float) {
                    preceding_float_sides |= profile.adjoining_float_sides;
                    continue;
                }
                // A matching float followed by a clear candidate can insert
                // real clearance at runtime, so the containing box cannot be
                // represented as one uninterrupted collapse-through chain.
                if profile.leading_clear_sides & preceding_float_sides != 0 || !profile.collapses_through {
                    return false;
                }
                preceding_float_sides |= profile.adjoining_float_sides;
            }
            true
        });
        let is_empty_block = matches!(
            reader.box_layout_mode(box_idx),
            Some(BoxType::Block(block))
                if matches!(&block.children, Children::Empty)
                    || matches!(&block.children, Children::InlineItems(runs) if !reader.inline_items_establish_line_box(box_idx, runs))
        );
        let has_zero_collapsible_height = permits_zero_height_collapse(style.height()) && permits_zero_height_collapse(style.min_height());
        let collapses_through = physical_before_open && physical_after_open && has_zero_collapsible_height && !establishes_context && (is_empty_block || children_all_collapse_through);

        if collapses_through {
            let mut adjoining = MarginStrut::from_margin(own_before);
            adjoining.add(own_after);
            let mut adjoining_float_sides = inline_item_float_sides;
            let mut leading_clear_sides = own_clear_sides;
            let mut before_clear = (own_clear_sides != 0).then_some(MarginStrut::default());
            let mut before_candidate = MarginStrut::from_margin(own_before);
            if let Some(children) = block_children {
                for child in children {
                    let child = self.margin_profile(reader, *child as usize, cb_width);
                    if matches!(child.participation, FlowParticipation::Float) {
                        adjoining_float_sides |= child.adjoining_float_sides;
                    } else if matches!(child.participation, FlowParticipation::InFlow) {
                        if leading_clear_sides == 0 && child.leading_clear_sides != 0 {
                            leading_clear_sides = child.leading_clear_sides;
                            let mut separated = before_candidate;
                            separated.merge(child.before_clear);
                            before_clear = Some(separated);
                        }
                        adjoining.merge(child.before);
                        adjoining.merge(child.after);
                        before_candidate.merge(child.before);
                        before_candidate.merge(child.after);
                        adjoining_float_sides |= child.adjoining_float_sides;
                    }
                }
            }
            return MarginProfile {
                before: adjoining,
                before_clear: before_clear.unwrap_or(adjoining),
                after: adjoining,
                own_before,
                own_after,
                participation,
                collapses_through: true,
                adjoining_float_sides,
                leading_clear_sides,
                leading_bfc_box: NO_LEADING_BFC,
                leading_bfc_float_sides: 0,
            };
        }

        let mut before = MarginStrut::from_margin(own_before);
        let mut adjoining_float_sides = 0;
        let mut leading_clear_sides = own_clear_sides;
        let mut leading_bfc_box = if establishes_context { box_idx as u32 } else { NO_LEADING_BFC };
        let mut leading_bfc_float_sides = 0;
        let mut before_clear = (own_clear_sides != 0 || establishes_context).then_some(MarginStrut::default());
        if children_before_open && let Some(children) = block_children {
            for child in children {
                let child_idx = *child as usize;
                let child = self.margin_profile(reader, child_idx, cb_width);
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

        let mut after = MarginStrut::from_margin(own_after);
        if children_after_open && let Some(children) = block_children {
            for child in children.iter().rev() {
                let child = self.margin_profile(reader, *child as usize, cb_width);
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

        MarginProfile { before, before_clear: before_clear.unwrap_or(before), after, own_before, own_after, participation, collapses_through: false, adjoining_float_sides, leading_clear_sides, leading_bfc_box, leading_bfc_float_sides }
    }

    /// Resolves the trailing margin after the box has been laid out. CSS bases
    /// min/max-height exceptions on used values: a constraint only stops the
    /// last child's bottom margin from escaping when it actually changes the
    /// parent's natural content height.
    pub(super) fn used_after_margin(&mut self, reader: &crate::layout::read_context::LayoutReader<'_>, box_idx: usize, cb_width: f64, parent_content_height: Option<f64>) -> MarginStrut {
        let width_key = cb_width.to_bits();
        let height_key = parent_content_height.map(f64::to_bits).unwrap_or(u64::MAX);
        if let Some(margin) = self.cache.after_margin(box_idx, width_key, height_key) {
            return margin;
        }

        let style = reader.style(box_idx);
        let own_after = style.margin_bottom().resolve(cb_width);
        let is_flex_grid_item = reader.get_parent(box_idx).is_some_and(|parent_idx| matches!(reader.box_layout_mode(parent_idx), Some(BoxType::Flex(_) | BoxType::Grid(_))));
        let establishes_context = establishes_formatting_context(
            style.display(),
            style.overflow_x(),
            style.overflow_y(),
            matches!(style.float(), Float::Left | Float::Right),
            reader.root_box() == Some(box_idx),
            reader.is_table_cell_box(box_idx),
            is_flex_grid_item,
        );
        let edge_open = !establishes_context && style.padding_bottom().is_zero() && style.border_bottom_width() == 0.0 && matches!(style.height(), PreferredSize::Auto);
        let vertical_inset = if matches!(style.box_sizing(), BoxSizing::BorderBox) { style.get_vertical_padding(cb_width) + style.border_top_width() as f64 + style.border_bottom_width() as f64 } else { 0.0 };
        let minimum = crate::layout::box_constraints::resolve_vertical_min_size(style.min_height(), parent_content_height, vertical_inset);
        let maximum = resolve_vertical_size(style.max_height(), parent_content_height, vertical_inset).map(|maximum| maximum.max(minimum.unwrap_or(0.0)));
        let descendant_height_basis = resolve_vertical_size(style.height(), parent_content_height, vertical_inset);
        let natural_height = self.natural_content_height(box_idx);
        let minimum_blocks_child = minimum.is_some_and(|minimum| natural_height < minimum);
        let maximum_blocks_child = maximum.is_some_and(|maximum| natural_height > maximum);

        let mut after = MarginStrut::from_margin(own_after);
        let trailing_children = (edge_open && !minimum_blocks_child && !maximum_blocks_child)
            .then(|| match reader.box_layout_mode(box_idx) {
                Some(BoxType::Block(block)) => match &block.children {
                    Children::Blocks(children) => Some(children.as_slice()),
                    _ => None,
                },
                _ => None,
            })
            .flatten();
        if let Some(children) = trailing_children {
            for child in children.iter().rev() {
                let child_idx = *child as usize;
                let profile = self.margin_profile(reader, child_idx, cb_width);
                if matches!(profile.participation, FlowParticipation::Float) {
                    continue;
                }
                after.merge(self.used_after_margin(reader, child_idx, cb_width, descendant_height_basis));
                if profile.collapses_through {
                    after.merge(profile.before);
                    continue;
                }
                break;
            }
        }

        self.cache.insert_after_margin(box_idx, width_key, height_key, after);
        after
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!establishes_formatting_context(Display::Block, OverflowMode::Clip, OverflowMode::Visible, false, false, false, false));
        assert!(establishes_formatting_context(Display::Block, OverflowMode::Hidden, OverflowMode::Visible, false, false, false, false));
    }

    #[test]
    fn explicit_zero_height_can_collapse_through_but_positive_height_cannot() {
        assert!(permits_zero_height_collapse(PreferredSize::Auto));
        assert!(permits_zero_height_collapse(PreferredSize::Px(0.0)));
        assert!(!permits_zero_height_collapse(PreferredSize::Px(1.0)));
    }
}
