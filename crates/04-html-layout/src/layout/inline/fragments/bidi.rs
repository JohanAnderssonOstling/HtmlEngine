//! Visual placement after logical line breaking (UAX #9 rules L1–L3).

use super::*;
use html_style_model::TextDirection;
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};

pub(super) fn place_visual_line(
    engine: &crate::layout::LayoutEngine<'_, '_>,
    span: &[InlineToken],
    runs: &[InlineTokenMetrics],
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
                        .unwrap_or(placement.advance
                            + if character == ' ' { layout.word_spacing } else { 0.0 }
                            + if !crate::shaping::is_bidi_control(character)
                                && glyph_idx + 1 < layout.glyph_range.end
                                && engine.text.is_cluster_boundary((glyph_idx + 1) as usize)
                            { layout.letter_spacing } else { 0.0 });
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
            placements[index].advance = space.unwrap_or(
                placements[index].advance
                    + if character == ' ' {
                        layout.word_spacing
                    } else {
                        0.0
                    }
                    + if !crate::shaping::is_bidi_control(character)
                        && glyph_idx + 1 < layout.glyph_range.end
                        && engine.text.is_cluster_boundary((glyph_idx + 1) as usize)
                    {
                        layout.letter_spacing
                    } else {
                        0.0
                    },
            );
        }
    }
    let cluster_levels: Vec<_> = clusters.iter().map(|range| levels[range.start]).collect();
    let visual_order = BidiInfo::reorder_visual(&cluster_levels);
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
