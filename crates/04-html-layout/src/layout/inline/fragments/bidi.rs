//! Visual placement after logical line breaking (UAX #9 rules L1–L3).
use crate::layout::text_placement::spaced_advance;
use html_text::is_bidi_control;

use super::*;
use html_style_model::TextDirection;
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};

pub(super) fn place_visual_line(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    span: &[InlineToken],
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
    line: &BrokenLine,
    direction: TextDirection,
    containing_width: f64,
    layout: &mut LineLayout,
) {
    profile_scope!("lines.bidi_placement");
    if direction == TextDirection::Rtl && line.align == TextAlign::Justify {
        layout.x_offset = (line.width_limit - layout.used_width).max(0.0);
    }
    if !span.iter().any(|token| {
        token.bidi_level() % 2 == 1
            || (direction == TextDirection::Rtl
                && matches!(token.kind(), InlineTokenKind::Ellipsis { .. }))
    }) {
        // Explicit positions (including reader word spacing) must consume
        // justification here, since each painted fragment owns its origin.
        if let Some(mut placements) = layout.placements.take() {
            let mut adjustment = 0.0;
            for (token, placement) in span.iter().zip(&mut placements) {
                placement.x += adjustment;
                if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
                    let character = engine.text.glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default()).ch();
                    let advance = layout.punctuation_space_advances.iter()
                        .find(|&&(index, _)| index == glyph_idx).map(|&(_, advance)| f64::from(advance))
                        .unwrap_or_else(|| spaced_advance(
                            placement.advance, character,
                            engine.text.has_tracking_boundary_after(glyph_idx, layout.glyph_range.end),
                            layout.word_spacing, layout.letter_spacing,
                        ));
                    adjustment += advance - placement.advance;
                    placement.advance = advance;
                }
            }
            layout.placements = Some(placements);
        }
        return;
    }
    let base = match direction {
        TextDirection::Ltr => Level::ltr(),
        TextDirection::Rtl => Level::rtl(),
    };
    let mut levels: Vec<_> = span
        .iter()
        .map(|token| Level::new(token.bidi_level()).unwrap_or(base))
        .collect();
    for (index, token) in span.iter().enumerate() {
        if matches!(token.kind(), InlineTokenKind::Ellipsis { .. }) {
            levels[index] = base;
        }
        if matches!(
            token.kind(),
            InlineTokenKind::Opportunity
                | InlineTokenKind::AbsoluteAnchor { .. }
                | InlineTokenKind::FloatAnchor { .. }
        ) {
            levels[index] = index
                .checked_sub(1)
                .map(|previous| levels[previous])
                .or_else(|| levels.get(index + 1).copied())
                .unwrap_or(base);
        }
    }
    let mut reset = true;
    for (index, token) in span.iter().enumerate().rev() {
        let class = match token.kind() {
            InlineTokenKind::Glyph { glyph_idx } => bidi_class(
                engine
                    .text
                    .glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default())
                    .ch(),
            ),
            InlineTokenKind::InlineBoundary { .. }
            | InlineTokenKind::Opportunity
            | InlineTokenKind::AbsoluteAnchor { .. }
            | InlineTokenKind::FloatAnchor { .. } => BidiClass::BN,
            InlineTokenKind::Break { .. } => BidiClass::S,
            _ => BidiClass::ON,
        };
        if matches!(class, BidiClass::B | BidiClass::S) {
            levels[index] = base;
            reset = true;
        } else if matches!(
            class,
            BidiClass::WS
                | BidiClass::BN
                | BidiClass::LRE
                | BidiClass::RLE
                | BidiClass::LRO
                | BidiClass::RLO
                | BidiClass::PDF
                | BidiClass::LRI
                | BidiClass::RLI
                | BidiClass::FSI
                | BidiClass::PDI
        ) {
            if reset {
                levels[index] = base;
            }
        } else {
            reset = false;
        }
    }
    let mut placements = layout.placements.take().unwrap_or_else(|| {
        build_token_placements(
            engine,
            span,
            runs,
            line.tab_origin,
            layout.line_height,
            layout.baseline,
            containing_width,
        )
    });
    // Shaping clusters are indivisible in visual ordering, including attached
    // combining marks. The font backend owns their internal glyph order.
    let mut clusters: Vec<Range<usize>> = Vec::new();
    for (index, token) in span.iter().enumerate() {
        if index + 1 == span.len() && layout.hyphen.is_some() {
            placements[index].advance += token.discretionary_width();
        }
        let combining = match token.kind() {
            InlineTokenKind::Glyph { glyph_idx } => engine
                .text
                .glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default())
                .ch()
                .is_mark(),
            _ => false,
        };
        let joins = matches!(token.kind(), InlineTokenKind::Glyph { .. })
            && (!token.is_cluster_boundary() || combining)
            && index > 0
            && matches!(span[index - 1].kind(), InlineTokenKind::Glyph { .. });
        if joins {
            clusters.last_mut().expect("preceding bidi cluster").end = index + 1;
        } else {
            clusters.push(index..index + 1);
        }
        if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
            let character = engine
                .text
                .glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default())
                .ch();
            let space = layout
                .punctuation_space_advances
                .iter()
                .find(|&&(index, _)| index == glyph_idx)
                .map(|&(_, advance)| f64::from(advance));
            placements[index].advance = space.unwrap_or_else(|| spaced_advance(
                placements[index].advance, character,
                engine.text.has_tracking_boundary_after(glyph_idx, layout.glyph_range.end),
                layout.word_spacing, layout.letter_spacing,
            ));
        }
    }
    let cluster_levels: Vec<_> = clusters.iter().map(|range| levels[range.start]).collect();
    let mut visual_order = BidiInfo::reorder_visual(&cluster_levels);
    place_inline_edges(engine, span, runs, replaced, &clusters, &mut visual_order);
    let mut x = 0.0;
    for cluster in visual_order {
        let range = clusters[cluster].clone();
        let width: f64 = placements[range.clone()]
            .iter()
            .map(|placement| placement.advance)
            .sum();
        let mut cursor = x;
        for index in range {
            placements[index].x = cursor;
            cursor += placements[index].advance;
        }
        x += width;
    }
    if let Some((_, offset)) = &mut layout.hyphen {
        *offset = placements.last().map_or(0.0, |placement| placement.x);
    }
    layout.placements = Some(placements);
}

fn place_inline_edges(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    span: &[InlineToken],
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
    clusters: &[Range<usize>],
    visual_order: &mut Vec<usize>,
) {
    // CSS assigns an inline's outer edges to its visual extremes on the
    // first/last line, independently of embedding controls inside its text.
    // Resolve descendants first so their edges remain inside ancestor edges.
    let mut edges = Vec::new();
    for (cluster_idx, cluster) in clusters.iter().enumerate() {
        let InlineTokenKind::InlineBoundary { payload_idx } = span[cluster.start].kind() else {
            continue;
        };
        let Some(ReplacedToken::InlineBoundary { box_idx, inline_start, inline_end, .. }) =
            replaced.get(payload_idx as usize)
        else {
            continue;
        };
        if inline_start == inline_end {
            continue;
        }
        let mut depth = 0;
        let mut ancestor = engine.reader.get_parent(*box_idx as usize);
        while let Some(owner) = ancestor {
            depth += 1;
            ancestor = engine.reader.get_parent(owner);
        }
        edges.push((cluster_idx, *box_idx as usize, *inline_start, depth));
    }
    edges.sort_by(|a, b| b.3.cmp(&a.3));
    for (edge, owner, left, _) in edges {
        let belongs = |cluster_idx: usize| {
            clusters[cluster_idx].clone().any(|index| {
                let token = &span[index];
                if let InlineTokenKind::InlineBoundary { payload_idx } = token.kind() {
                    if matches!(replaced.get(payload_idx as usize), Some(ReplacedToken::InlineBoundary { box_idx, .. }) if *box_idx as usize == owner) {
                        return false;
                    }
                } else if !matches!(token.kind(), InlineTokenKind::Glyph { .. } | InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }) {
                    return false;
                }
                if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
                    let character = engine.text.glyph_metric(engine.text.glyph_at(glyph_idx as usize).unwrap_or_default()).ch();
                    if is_bidi_control(character) {
                        return false;
                    }
                }
                let mut current = Some(token.run_metrics(runs).owner_box_idx as usize)
                    .filter(|&box_idx| box_idx != u32::MAX as usize);
                while let Some(box_idx) = current {
                    if box_idx == owner {
                        return true;
                    }
                    current = engine.reader.get_parent(box_idx);
                }
                false
            })
        };
        let anchor = if left {
            visual_order.iter().copied().find(|&index| belongs(index))
        } else {
            visual_order.iter().copied().rev().find(|&index| belongs(index))
        };
        let Some(anchor) = anchor else { continue };
        visual_order.retain(|&index| index != edge);
        let position = visual_order.iter().position(|&index| index == anchor)
            .expect("inline edge anchor remains in visual order");
        visual_order.insert(position + usize::from(!left), edge);
    }
}
