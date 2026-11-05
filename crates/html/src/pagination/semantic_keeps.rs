use html_pipeline::engine::pagination_queries;
use super::KeepContext;
use crate::layout::RenderView;
use crate::pagination::PaginationTarget;

use crate::layout::ContentRole;

const EPSILON: f64 = 0.01;

/// Returns soft keep ranges for compact book structures. These are
/// pagination preferences, not layout constraints: the normal keep event
/// path discards any range taller than a fragmentainer.
pub(super) fn semantic_keep_ranges(
    doc: RenderView<'_>,
    ranges: &mut Vec<(usize, f64, f64)>,
    context: &KeepContext,
    range_lines: &mut Vec<usize>,
) {
    ranges.clear();
    let root = doc;
    let lines = pagination_queries::render_view_pagination(root);
    for block in &context.blocks {
        let constraints = &block.constraints;
        let point = block.bounds.origin();
        let bottom = block.bounds.y1;
        context.lines_in_range(point.y, bottom, range_lines);
        if range_lines.is_empty() {
            continue;
        }
        let semantics = block.roles;
        let compact_line_limit = if semantics.has_role(ContentRole::ListItem) {
            Some(4)
        } else if semantics.has_role(ContentRole::CodeBlock) {
            Some(12)
        } else if semantics.has_role(ContentRole::Quote) {
            Some(6)
        } else if semantics.has_role(ContentRole::CompactStructure) {
            Some(8)
        } else {
            None
        };
        let Some(limit) = compact_line_limit else {
            continue;
        };
        let has_forced_inside = constraints.has_forced_descendant;
        if range_lines.len() <= limit
            && !has_forced_inside
            && let Some(trigger_line) = block.before
        {
            ranges.push((trigger_line, point.y, bottom));
        }
    }

    for pair in &context.siblings {
        if (!pair.definition_pair && !pair.statement_label_pair) || pair.forced_boundary {
            continue;
        }
        let label_point = pair.previous_bounds.origin();
        context.lines_in_range(pair.next_bounds.y0, pair.next_bounds.y1, range_lines);
        let required_bottom = range_lines
            .iter()
            .take(2)
            .filter_map(|&line_idx| {
                lines
                    .locate(PaginationTarget::Cursor(line_idx))
                    .map(|position| position.line)
                    .map(|line| line.point().y + line.height())
            })
            .fold(label_point.y, f64::max);
        if let Some(trigger_line) = pair.trigger_line
            && required_bottom > label_point.y + EPSILON
        {
            ranges.push((trigger_line, label_point.y, required_bottom));
        }
    }

    ranges.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.total_cmp(&right.1))
            .then_with(|| right.2.total_cmp(&left.2))
    });
    ranges.dedup_by(|left, right| {
        left.0 == right.0
            && (left.1 - right.1).abs() <= EPSILON
            && (left.2 - right.2).abs() <= EPSILON
    });
}
