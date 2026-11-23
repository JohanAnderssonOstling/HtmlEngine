//! Build inline-box fragments and their decorating ancestor geometry.
use std::ptr;
use crate::LayoutEngine;
use crate::inline::TokenSpan;
use crate::inline::fragments::inline_relative_position_offset;
use crate::inline::fragments::{inline_ancestors, parent_font_content_extents};
use crate::inline::metrics::MetricsView;
use crate::inline::tokens::{
    InlineTokenKind, InlineTokenMetrics, ReplacedToken, TokenMetadata,
    vertical_align_metrics_offset,
};
use html_engine_model::LineInlineBoxFragment;
use html_style_model::{
    Float, TextBoxOverEdge, TextBoxTrim, TextBoxUnderEdge, TextDirection, computed_style,
};
use kurbo::Vec2;

#[allow(clippy::too_many_arguments)]
pub(super) fn positioned_inline_box_fragments_from<'m>(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    placements: impl IntoIterator<Item = ((TokenMetadata, &'m InlineTokenMetrics), (f64, f64, f32))>,
    bidi: bool,
    replaced: &[ReplacedToken],
    line_height: f64,
    baseline: f64,
    containing_width: f64,
) -> Vec<LineInlineBoxFragment> {
    // A line normally intersects only a handful of inline boxes. A compact
    // linear accumulator beats allocating a box-count-sized side table for
    // every line while still publishing one durable fragment per owner.
    let mut bounds = Vec::<LineInlineBoxFragment>::new();
    let mut cached_owner = None;
    let mut ancestors = Vec::new();
    let mut placements = placements.into_iter().enumerate().peekable();
    while let Some((paint_order, ((token, metrics), (mut x, advance, y_offset)))) =
        placements.next()
    {
        let paint_order = paint_order as u32;
        if advance <= 0.0 && !matches!(token.kind(), InlineTokenKind::InlineBoundary { .. }) {
            continue;
        }
        let mut end_x = x + advance;
        // Logical glyph runs share all ancestor geometry and fragment flags.
        // Reduce their horizontal extent before visiting the ancestor chain:
        // O(glyphs + runs * depth), rather than O(glyphs * depth). Keep source
        // paint order from the first glyph, and preserve each x + advance sum
        // instead of reconstructing the endpoint from a combined advance.
        if !bidi
            && matches!(token.kind(), InlineTokenKind::Glyph { .. })
            && x.is_finite()
            && end_x.is_finite()
        {
            while let Some((_, ((_, _), (next_x, next_advance, _)))) = placements.next_if(
                |(_, ((next, next_metrics), (next_x, next_advance, next_y)))| {
                    matches!(next.kind(), InlineTokenKind::Glyph { .. })
                        && ptr::eq(metrics, *next_metrics)
                        && next_y.to_bits() == y_offset.to_bits()
                        && *next_advance > 0.0
                        && next_x.is_finite()
                        && (next_x + next_advance).is_finite()
                },
            ) {
                x = x.min(next_x);
                end_x = end_x.max(next_x + next_advance);
            }
        }
        let token_top = (baseline - metrics.ascent as f64 - y_offset as f64) as f32;
        let token_bottom = (baseline + metrics.descent as f64 - y_offset as f64) as f32;
        let boundary = match token.kind() {
            InlineTokenKind::InlineBoundary { payload_idx } => {
                match replaced.get(payload_idx as usize) {
                    Some(ReplacedToken::InlineBoundary {
                        box_idx,
                        margin_left,
                        left_inset,
                        inline_start,
                        inline_end,
                    }) => Some((
                        *box_idx as usize,
                        x + margin_left + left_inset,
                        *inline_start,
                        *inline_end,
                    )),
                    _ => None,
                }
            }
            _ => None,
        };
        let replaced_border_fragment = match token.kind() {
            InlineTokenKind::Image { payload_idx } => match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::Image {
                    box_idx,
                    border_size,
                    margin_left,
                    margin_top,
                    ..
                }) => {
                    let border_left = x + margin_left;
                    let border_top =
                        baseline - metrics.ascent as f64 - y_offset as f64 + margin_top;
                    Some(LineInlineBoxFragment {
                        box_idx: *box_idx,
                        paint_order,
                        start_x: border_left as f32,
                        end_x: (border_left + border_size.width) as f32,
                        top: border_top as f32,
                        bottom: (border_top + border_size.height) as f32,
                        baseline: baseline as f32,
                        flags: LineInlineBoxFragment::INLINE_START
                            | LineInlineBoxFragment::INLINE_END
                            | LineInlineBoxFragment::BORDER_BOX_BOUNDS,
                    })
                }
                _ => None,
            },
            _ => None,
        };
        let owner = inline_fragment_owner(engine, &token, metrics, replaced);
        // Consecutive tokens usually share an owner. Resolve its ancestry and
        // line-relative inline geometry once; block decoration extents still
        // use each token's own ascent, descent, and placement below.
        if cached_owner != Some(owner) {
            resolve_inline_fragment_ancestors(
                engine,
                container_box_idx,
                owner,
                line_height,
                baseline,
                containing_width,
                &mut ancestors,
            );
            cached_owner = Some(owner);
        }
        // New fragments have monotonically increasing source positions. At
        // the same position, append enclosing boxes before their descendants
        // so logical-order lines already have their final paint order.
        let mut ancestor_iter = ancestors.iter_mut();
        while let Some(ancestor) = if bidi {
            ancestor_iter.next()
        } else {
            ancestor_iter.next_back()
        } {
            let box_idx = ancestor.box_idx;
            let relative_offset = ancestor.relative_offset;
            let fragment_baseline = ancestor.baseline;
            let (fragment_top, fragment_bottom) =
                ancestor.content_bounds.unwrap_or((token_top, token_bottom));
            let own_boundary = boundary.filter(|boundary| boundary.0 == box_idx);
            let fragment_start =
                (own_boundary.map_or(x, |boundary| boundary.1) + relative_offset.x) as f32;
            let fragment_end =
                (own_boundary.map_or(end_x, |boundary| boundary.1) + relative_offset.x) as f32;
            // LTR bounds are never reordered or removed during accumulation.
            // Resolve the slot once per owner transition, then update it directly.
            // Bidi fragments can be disjoint, so their overlap search stays dynamic.
            let fragment_index = ancestor.fragment_index.or_else(|| {
                bounds.iter().position(|fragment| {
                    fragment.box_idx as usize == box_idx
                        && (!bidi
                            || (fragment.start_x <= fragment_end + 0.001
                                && fragment.end_x >= fragment_start - 0.001))
                })
            });
            if let Some(index) = fragment_index {
                if !bidi {
                    ancestor.fragment_index = Some(index);
                }
                let fragment = &mut bounds[index];
                fragment.paint_order = fragment.paint_order.min(paint_order);
                fragment.start_x = fragment.start_x.min(fragment_start);
                fragment.end_x = fragment.end_x.max(fragment_end);
                fragment.top = fragment.top.min(fragment_top);
                fragment.bottom = fragment.bottom.max(fragment_bottom);
                if let Some((_, _, inline_start, inline_end)) = own_boundary {
                    fragment.flags |= if inline_start {
                        LineInlineBoxFragment::INLINE_START
                    } else {
                        0
                    } | if inline_end {
                        LineInlineBoxFragment::INLINE_END
                    } else {
                        0
                    };
                }
            } else {
                let (inline_start, inline_end) = own_boundary.map_or_else(
                    || {
                        if bidi && replaced.iter().any(|replaced| matches!(replaced, ReplacedToken::InlineBoundary { box_idx: owner, .. } if *owner as usize == box_idx)) {
                            return (false, false);
                        }
                        let edges = engine.read.reader.inline_fragment_edges(box_idx);
                        if engine.read.reader.style(box_idx).direction() == TextDirection::Rtl { (edges.1, edges.0) } else { edges }
                    },
                    |boundary| (boundary.2, boundary.3),
                );
                if !bidi {
                    ancestor.fragment_index = Some(bounds.len());
                }
                bounds.push(LineInlineBoxFragment {
                    box_idx: box_idx as u32,
                    paint_order,
                    start_x: fragment_start,
                    end_x: fragment_end,
                    top: fragment_top,
                    bottom: fragment_bottom,
                    baseline: fragment_baseline,
                    flags: if inline_start {
                        LineInlineBoxFragment::INLINE_START
                    } else {
                        0
                    } | if inline_end {
                        LineInlineBoxFragment::INLINE_END
                    } else {
                        0
                    },
                });
            }
        }
        // Enclosing inline backgrounds must precede the replaced element's
        // own background and border in paint order, so append this fragment
        // after accumulating its ancestor chain.
        if let Some(fragment) = replaced_border_fragment {
            bounds.push(fragment);
        }
    }
    if bidi {
        bounds.sort_by(|a, b| {
            a.box_idx
                .cmp(&b.box_idx)
                .then(a.start_x.total_cmp(&b.start_x))
        });
        let mut merged: Vec<LineInlineBoxFragment> = Vec::with_capacity(bounds.len());
        for fragment in bounds {
            if let Some(previous) = merged.last_mut().filter(|previous| {
                previous.box_idx == fragment.box_idx
                    && (previous.flags ^ fragment.flags) & LineInlineBoxFragment::BORDER_BOX_BOUNDS
                        == 0
                    && previous.end_x + 0.001 >= fragment.start_x
            }) {
                previous.end_x = previous.end_x.max(fragment.end_x);
                previous.top = previous.top.min(fragment.top);
                previous.bottom = previous.bottom.max(fragment.bottom);
                previous.paint_order = previous.paint_order.min(fragment.paint_order);
                previous.flags |= fragment.flags;
            } else {
                merged.push(fragment);
            }
        }
        bounds = merged;
        // Visual merging groups by box and x coordinate, so restore source
        // paint order here. Logical-order lines never need this sorting pass.
        bounds.sort_by_key(|fragment| {
            let mut depth = 0u32;
            let mut parent = engine.read.reader.get_parent(fragment.box_idx as usize);
            while let Some(parent_idx) = parent {
                depth += 1;
                parent = engine.read.reader.get_parent(parent_idx);
            }
            (fragment.paint_order, depth)
        });
    }
    bounds
}

/// Returns the alignment of the fragment owner itself and its inline
/// ancestors, excluding shifts authored on descendants. Decoration geometry
/// is tied to the decorating box's baseline, not whichever descendant token
/// happened to create its accumulated fragment first.
pub(super) fn has_inline_fragment_owners(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    metrics: MetricsView<'_>,
    replaced: &[ReplacedToken],
) -> bool {
    let mut checked_owner = None;
    metrics.any(span, move |token, metrics| {
        // Images publish their own border-box fragment even when none of
        // their ancestors contributes an inline background.
        if matches!(token.kind(), InlineTokenKind::Image { .. }) {
            return true;
        }
        let owner = inline_fragment_owner(engine, &token, metrics, replaced);
        // A positive result ends the scan. Consecutive tokens with the same
        // negative owner can reuse its ancestry check without style reads.
        if checked_owner == Some(owner) {
            return false;
        }
        checked_owner = Some(owner);
        owner_has_inline_fragments(engine, owner)
    })
}

pub(super) fn owner_has_inline_fragments(
    engine: &LayoutEngine<'_, '_>,
    mut owner: Option<usize>,
) -> bool {
    while let Some(box_idx) = owner {
        if engine.read.reader.is_inline_box(box_idx)
            || !computed_style::text_decoration_lines_is_empty(
                engine.read.reader.style(box_idx).text_decoration_lines(),
            )
        {
            return true;
        }
        if stops_text_decoration_propagation(engine, box_idx) {
            break;
        }
        owner = engine.read.reader.get_parent(box_idx);
    }
    false
}

#[derive(Clone, Copy)]
struct InlineFragmentAncestor {
    box_idx: usize,
    relative_offset: Vec2,
    baseline: f32,
    content_bounds: Option<(f32, f32)>,
    // Logical-order fragments keep their slot throughout this line.
    fragment_index: Option<usize>,
}

#[allow(clippy::too_many_arguments)]
fn resolve_inline_fragment_ancestors(
    engine: &LayoutEngine<'_, '_>,
    container_box_idx: usize,
    mut owner: Option<usize>,
    line_height: f64,
    baseline: f64,
    containing_width: f64,
    ancestors: &mut Vec<InlineFragmentAncestor>,
) {
    ancestors.clear();
    let mut outside_formatting_context = false;
    while let Some(box_idx) = owner {
        let is_inline = engine.read.reader.is_inline_box(box_idx);
        let has_propagated_decoration = !computed_style::text_decoration_lines_is_empty(
            engine.read.reader.style(box_idx).text_decoration_lines(),
        );
        if (!outside_formatting_context && is_inline) || has_propagated_decoration {
            let relative_offset =
                inline_relative_position_offset(engine, box_idx, containing_width);
            let fragment_baseline = (baseline
                - inline_fragment_vertical_align_offset(engine, box_idx, line_height, baseline)
                + relative_offset.y) as f32;
            // CSS 2.1 defines a non-replaced inline's vertical padding,
            // border, and margin edges from its content box. Half-leading
            // participates in line-box height, but is not part of that
            // content box. Use the decorating inline's own font metrics so
            // descendants with a different font do not resize its border.
            let content_bounds = if is_inline {
                let style = engine.read.reader.style(box_idx);
                let font_size = style.font_size() as f64;
                let font_metrics = engine.read.reader.font_metrics(box_idx);
                let (ascent, descent) = font_metrics.line_box_extents(font_size);
                let mut top = fragment_baseline - ascent as f32;
                let mut bottom = fragment_baseline + descent as f32;
                let trim = style.text_box_trim();
                let edge = style.text_box_edge();
                if matches!(trim, TextBoxTrim::Start | TextBoxTrim::Both) {
                    top = match edge.over {
                        TextBoxOverEdge::Text => top,
                        TextBoxOverEdge::Cap => {
                            fragment_baseline - font_size as f32 * font_metrics.cap_height_ratio
                        }
                        TextBoxOverEdge::Ex => {
                            fragment_baseline - font_size as f32 * font_metrics.x_height_ratio
                        }
                    };
                }
                if matches!(trim, TextBoxTrim::End | TextBoxTrim::Both)
                    && matches!(edge.under, TextBoxUnderEdge::Alphabetic)
                {
                    bottom = fragment_baseline;
                }
                Some((top, bottom))
            } else {
                None
            };
            ancestors.push(InlineFragmentAncestor {
                box_idx,
                relative_offset,
                baseline: fragment_baseline,
                content_bounds,
                fragment_index: None,
            });
        }
        if stops_text_decoration_propagation(engine, box_idx) {
            break;
        }
        if box_idx == container_box_idx {
            outside_formatting_context = true;
        }
        owner = engine.read.reader.get_parent(box_idx);
    }
}

fn inline_fragment_vertical_align_offset(
    engine: &LayoutEngine<'_, '_>,
    box_idx: usize,
    line_height: f64,
    baseline: f64,
) -> f64 {
    let mut offset = 0.0;
    for current_idx in inline_ancestors(&engine.read.reader, Some(box_idx)) {
        let style = engine.read.reader.style(current_idx);
        let value = style.vertical_align();
        if !value.is_initial() {
            let own_line_height = style.resolved_line_height();
            let font_size = style.font_size() as f64;
            let (ascent, descent) =
                (engine.read.reader.font_metrics(current_idx)).line_box_extents(font_size);
            let half_leading = (own_line_height - ascent - descent) * 0.5;
            let (parent_ascent, parent_descent, parent_x_height) =
                parent_font_content_extents(&engine.read.reader, current_idx);
            offset += vertical_align_metrics_offset(
                value,
                ascent,
                descent,
                own_line_height,
                font_size,
                half_leading,
                line_height,
                baseline,
                parent_ascent,
                parent_descent,
                parent_x_height,
            );
        }
    }
    offset
}

fn inline_fragment_owner(
    engine: &LayoutEngine<'_, '_>,
    token: &TokenMetadata,
    metrics: &InlineTokenMetrics,
    replaced: &[ReplacedToken],
) -> Option<usize> {
    match token.kind() {
        // Replaced boxes paint themselves; their parent owns enclosing fragments.
        InlineTokenKind::Image { payload_idx } => match replaced.get(payload_idx as usize) {
            Some(ReplacedToken::Image { box_idx, .. }) => {
                engine.read.reader.get_parent(*box_idx as usize)
            }
            _ => None,
        },
        InlineTokenKind::AtomicBox { payload_idx } => match replaced.get(payload_idx as usize) {
            Some(ReplacedToken::AtomicBox { box_idx, .. }) => {
                engine.read.reader.get_parent(*box_idx as usize)
            }
            _ => None,
        },
        InlineTokenKind::InlineBoundary { payload_idx } => {
            match replaced.get(payload_idx as usize) {
                Some(ReplacedToken::InlineBoundary { box_idx, .. }) => Some(*box_idx as usize),
                _ => None,
            }
        }
        _ => {
            let owner = metrics.owner_box_idx;
            (owner != u32::MAX).then_some(owner as usize)
        }
    }
}

/// Text decorations propagate through ordinary in-flow block and table boxes;
/// they are not inherited, but each ancestor that establishes a decoration
/// paints it across descendant inline content. Atomic inline formatting
/// contexts and out-of-flow descendants form the CSS propagation boundary.
fn stops_text_decoration_propagation(engine: &LayoutEngine<'_, '_>, box_idx: usize) -> bool {
    engine.read.reader.box_facts(box_idx).atomic_inline
        || engine.read.reader.box_facts(box_idx).float != Float::None
        || engine.read.reader.box_facts(box_idx).absolute
}
