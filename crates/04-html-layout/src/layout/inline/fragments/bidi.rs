//! Visual placement after logical line breaking (UAX #9 rules L1–L3).
use html_text::is_bidi_control;
use html_text::spaced_advance;

use super::*;
use crate::layout::LayoutEngine;
use html_style_model::TextDirection;
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};

/// Clusters partition the token span. Adjacent boundaries supply each range,
/// so the scratch array stores one word per cluster plus its initial zero.
#[derive(Clone, Copy)]
struct ClusterRanges<'a>(&'a [usize]);

impl ClusterRanges<'_> {
    #[inline]
    fn len(self) -> usize {
        self.0.len().saturating_sub(1)
    }

    #[inline]
    fn range(self, index: usize) -> Range<usize> {
        let bounds = self.0.windows(2).nth(index).expect("valid bidi cluster");
        bounds[0]..bounds[1]
    }

    #[inline]
    fn iter(self) -> impl DoubleEndedIterator<Item = Range<usize>> + ExactSizeIterator {
        self.0.windows(2).map(|bounds| bounds[0]..bounds[1])
    }
}

pub(super) fn place_visual_line(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
    line: &BrokenLine,
    direction: TextDirection,
    bidi: InlineBidiSummary,
    no_bidi_separators: bool,
    layout: &mut LineLayout,
) {
    profile_scope!("lines.bidi_placement");
    let has_odd_level = bidi.has_odd_level;
    if direction == TextDirection::Rtl && line.align == TextAlign::Justify {
        layout.x_offset += (line.width_limit - layout.used_width).max(0.0);
    }
    // Measurement already classified the selected span, including rewritten
    // lines. An RTL ellipsis can still require ordering at level zero.
    let has_rtl_ellipsis = !has_odd_level
        && direction == TextDirection::Rtl
        && span
            .metadata_iter()
            .any(|token| matches!(token.kind(), InlineTokenKind::Ellipsis { .. }));
    // RTL paragraphs still need L1's reset of trailing whitespace when all
    // characters on this particular line resolved to an even level.
    if !has_odd_level && !has_rtl_ellipsis && direction == TextDirection::Ltr {
        // Streamed text applies the same justification adjustments when its
        // fragments are published, without retaining per-token positions.
        if matches!(layout.placements, Some(LinePlacements::Sequential { .. })) {
            return;
        }
        // Explicit positions (including reader word spacing) must consume
        // justification here, since each painted fragment owns its origin.
        if let Some(mut placements) = layout.placements.take() {
            let LinePlacements::Full(values) = &mut placements else {
                unreachable!("measurement only creates full placements");
            };
            let mut adjustment = 0.0;
            for (token, placement) in span.metadata_iter().zip(values) {
                placement.x += adjustment;
                if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
                    let character = engine.text.character(glyph_idx);
                    let advance = layout
                        .punctuation_space_advances
                        .iter()
                        .find(|&&(index, _)| index == glyph_idx)
                        .map(|&(_, advance)| f64::from(advance))
                        .unwrap_or_else(|| {
                            spaced_advance(
                                placement.advance,
                                character,
                                layout.letter_spacing != 0.0
                                    && engine.text.has_tracking_boundary_after(
                                        glyph_idx,
                                        layout.glyph_range.end,
                                    ),
                                layout.word_spacing,
                                layout.letter_spacing,
                            )
                        });
                    adjustment += advance - placement.advance;
                    placement.advance = advance;
                }
            }
            placements.compact(line.tab_origin, false, None);
            layout.placements = Some(placements);
        }
        return;
    }
    let base = match direction {
        TextDirection::Ltr => Level::ltr(),
        TextDirection::Rtl => Level::rtl(),
    };
    let uniform_base = bidi.is_uniform_at(base.number());
    match span {
        TokenSpan::Wide(tokens) => {
            place_bidi_line(
                engine,
                tokens,
                span,
                runs,
                replaced,
                base,
                uniform_base,
                no_bidi_separators,
                line.tab_origin,
                layout,
            );
        }
        TokenSpan::Compact { source, start, len } => place_bidi_line(
            engine,
            CompactTokens::new(source, start, len),
            span,
            runs,
            replaced,
            base,
            uniform_base,
            no_bidi_separators,
            line.tab_origin,
            layout,
        ),
    }
}

// Keep bidi scratch buffers and specialized loops out of ordinary LTR line
// preparation, where inlining them increases register and instruction pressure.
#[inline(never)]
fn place_bidi_line<S: TokenRead>(
    engine: &LayoutEngine<'_, '_>,
    tokens: S,
    span: TokenSpan<'_>,
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
    base: Level,
    uniform_base: bool,
    no_bidi_separators: bool,
    tab_origin: f64,
    layout: &mut LineLayout,
) {
    // Unadjusted plain glyphs need only visual x coordinates. Keep the full
    // path for hyphens and line spacing so its exact advance arithmetic stays
    // unchanged. Measurement already proved visibility and zero offsets.
    let glyph_x = layout.placements.is_none()
        && layout.hyphen.is_none()
        && layout.word_spacing == 0.0
        && layout.letter_spacing == 0.0
        && layout.punctuation_space_advances.is_empty();
    let has_inline_edges = replaced
        .iter()
        .any(|payload| matches!(payload, ReplacedToken::InlineBoundary { .. }));
    if uniform_base && base.is_rtl() && glyph_x && !has_inline_edges {
        // The visual order is just reversed clusters. Discover each cluster
        // while walking backward instead of materializing ranges and a map.
        layout.placements = Some(LinePlacements::GlyphX(
            uniform_rtl_glyph_positions(tokens).into_boxed_slice(),
        ));
        return;
    }
    let mut placement_storage = if glyph_x {
        None
    } else {
        Some(layout.placements.take().unwrap_or_else(|| {
            LinePlacements::Full(plain_text_placements(tokens).into_boxed_slice())
        }))
    };
    let mut placements = match placement_storage.as_mut() {
        Some(LinePlacements::Full(values)) => Some(values.as_mut()),
        None => None,
        Some(LinePlacements::GlyphX(_) | LinePlacements::Positioned(_)) => {
            unreachable!("freshly measured line")
        }
        Some(LinePlacements::Sequential { .. }) => {
            unreachable!("streamed positions require logical LTR text")
        }
    };
    // Shaping clusters are indivisible in visual ordering, including attached
    // combining marks. The font backend owns their internal glyph order.
    let mut cluster_boundaries: Vec<usize> = Vec::new();
    if tokens.len() != 0 {
        // Four initial clusters plus the origin avoid an extra growth step
        // at power-of-two cluster counts when adding the leading boundary.
        cluster_boundaries.reserve_exact(5);
        cluster_boundaries.push(0);
    }
    // Ordering consumes only each cluster's initial level. Keep these in a
    // contiguous column and carry the preceding token's unadjusted level for
    // synthetic-token inheritance, including glyphs that join a cluster.
    let mut cluster_levels = if uniform_base {
        Vec::new()
    } else {
        Vec::<Level>::with_capacity(tokens.len())
    };
    // A closing inline boundary can follow the chosen conditional hyphen.
    // Give its advance to the discretionary token so visual ordering keeps
    // the hyphen beside the word, including on the left of an RTL word.
    let hyphen_index = layout.hyphen.and_then(|_| {
        (0..tokens.len()).rev().find(|&index| {
            !matches!(tokens.metadata_at(index).kind(),
                InlineTokenKind::InlineBoundary { .. }
                    | InlineTokenKind::Opportunity
                    | InlineTokenKind::AbsoluteAnchor { .. }
                    | InlineTokenKind::FloatAnchor { .. })
        })
    });
    let mut previous_level = None;
    let mut previous_is_glyph = false;
    for index in 0..tokens.len() {
        let token = tokens.metadata_at(index);
        if Some(index) == hyphen_index {
            placements
                .as_mut()
                .expect("hyphen requires full placements")[index]
                .advance += token.discretionary_width(tokens.stored_width_at(index));
        }
        let kind = token.kind();
        let glyph = match kind {
            InlineTokenKind::Glyph { glyph_idx } => Some(glyph_idx),
            _ => None,
        };
        let joins = previous_is_glyph && glyph.is_some() && token.continues_bidi_cluster();
        previous_is_glyph = glyph.is_some();
        if joins {
            *cluster_boundaries.last_mut().expect("preceding bidi cluster") = index + 1;
        } else {
            cluster_boundaries.push(index + 1);
        }
        if !uniform_base {
            let level = match kind {
                InlineTokenKind::Ellipsis { .. } => base,
                // At the start of a paragraph an out-of-flow anchor occupies
                // the paragraph's insertion point, even when the following
                // text has a different embedding direction.
                InlineTokenKind::AbsoluteAnchor { .. } => previous_level.unwrap_or(base),
                InlineTokenKind::Opportunity
                | InlineTokenKind::Discretionary { .. }
                | InlineTokenKind::FloatAnchor { .. } => previous_level
                    .or_else(|| {
                        (index + 1 < tokens.len()).then(|| {
                            Level::new(tokens.metadata_at(index + 1).bidi_level()).unwrap_or(base)
                        })
                    })
                    .unwrap_or(base),
                _ => Level::new(token.bidi_level()).unwrap_or(base),
            };
            previous_level = Some(level);
            if !joins {
                cluster_levels.push(level);
            }
        }
        if let Some(glyph_idx) = glyph
            && let Some(placements) = placements.as_mut()
        {
            let character = engine.text.character(glyph_idx);
            let space = layout
                .punctuation_space_advances
                .iter()
                .find(|&&(index, _)| index == glyph_idx)
                .map(|&(_, advance)| f64::from(advance));
            placements[index].advance = space.unwrap_or_else(|| {
                spaced_advance(
                    placements[index].advance,
                    character,
                    layout.letter_spacing != 0.0
                        && engine
                            .text
                            .has_tracking_boundary_after(glyph_idx, layout.glyph_range.end),
                    layout.word_spacing,
                    layout.letter_spacing,
                )
            });
        }
    }
    let clusters = ClusterRanges(&cluster_boundaries);
    if !uniform_base {
        // L1 can inspect any token in a cluster, but only its start level is
        // used by L2. Uniform base levels already need no reset or level data.
        let mut reset = true;
        'reset_levels: for (cluster, range) in clusters.iter().enumerate().rev() {
            for index in range.clone().rev() {
                let token = tokens.metadata_at(index);
                let class = match token.kind() {
                    InlineTokenKind::Glyph { glyph_idx } => {
                        let character = engine.text.character(glyph_idx);
                        // Once ordinary text ends the trailing reset, only B/S
                        // can restart it. All other classes leave levels and the
                        // false reset state unchanged, so need no table lookup.
                        if !reset && !may_be_bidi_separator(character) {
                            continue;
                        }
                        bidi_class(character)
                    }
                    InlineTokenKind::InlineBoundary { .. }
                    | InlineTokenKind::Opportunity
                    | InlineTokenKind::AbsoluteAnchor { .. }
                    | InlineTokenKind::FloatAnchor { .. } => BidiClass::BN,
                    InlineTokenKind::Break { .. } => BidiClass::S,
                    _ => BidiClass::ON,
                };
                if matches!(class, BidiClass::B | BidiClass::S) {
                    if index == range.start {
                        cluster_levels[cluster] = base;
                    }
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
                    if reset && index == range.start {
                        cluster_levels[cluster] = base;
                    }
                } else {
                    // Without a glyph separator or Break token, no earlier
                    // token can restart L1 after ordinary text ends the reset.
                    if no_bidi_separators {
                        break 'reset_levels;
                    }
                    reset = false;
                }
            }
        }
    }
    let mut visual_order = if uniform_base {
        if base.is_rtl() {
            (0..clusters.len()).rev().collect()
        } else {
            (0..clusters.len()).collect()
        }
    } else {
        BidiInfo::reorder_visual(&cluster_levels)
    };
    if has_inline_edges {
        place_inline_edges(engine, span, runs, replaced, clusters, &mut visual_order);
    }
    if glyph_x {
        layout.placements = Some(LinePlacements::GlyphX(
            visual_glyph_positions(tokens, clusters, &visual_order).into_boxed_slice(),
        ));
        return;
    }
    let placements = placements.expect("adjusted placements");
    let mut x = 0.0;
    for cluster in visual_order {
        let range = clusters.range(cluster);
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
        *offset = hyphen_index.map_or(0.0, |index| placements[index].x);
    }
    if let Some(storage) = &mut placement_storage {
        storage.compact(tab_origin, true, hyphen_index);
    }
    layout.placements = placement_storage;
}

fn uniform_rtl_glyph_positions<S: TokenRead>(tokens: S) -> Vec<f64> {
    let mut xs = vec![0.0; tokens.len()];
    let mut x = 0.0;
    let mut place_cluster = |range: Range<usize>| {
        // Match visual_glyph_positions: sum each cluster in logical order,
        // then place its glyphs with a separate sum from the visual origin.
        let width: f64 = range
            .clone()
            .map(|index| tokens.metadata_at(index).width(tokens.stored_width_at(index)))
            .sum();
        let mut cursor = x;
        for index in range {
            xs[index] = cursor;
            cursor += tokens.metadata_at(index).width(tokens.stored_width_at(index));
        }
        x += width;
    };
    let mut end = tokens.len();
    let mut next_is_glyph = false;
    let mut next_continues = false;
    for index in (0..tokens.len()).rev() {
        let token = tokens.metadata_at(index);
        let is_glyph = matches!(token.kind(), InlineTokenKind::Glyph { .. });
        // Forward clustering joins the next token to this one only when
        // both are glyphs and that next token carries the continuation bit.
        if index + 1 < end && !(is_glyph && next_is_glyph && next_continues) {
            place_cluster(index + 1..end);
            end = index + 1;
        }
        next_is_glyph = is_glyph;
        next_continues = token.continues_bidi_cluster();
    }
    if end != 0 {
        place_cluster(0..end);
    }
    xs
}

fn visual_glyph_positions<S: TokenRead>(
    tokens: S,
    clusters: ClusterRanges<'_>,
    visual_order: &[usize],
) -> Vec<f64> {
    let mut xs = vec![0.0; tokens.len()];
    let mut x = 0.0;
    for &cluster in visual_order {
        let range = clusters.range(cluster);
        // Preserve the full placement path's two sums and source order within
        // each shaping cluster, including its floating-point rounding.
        let width: f64 = range
            .clone()
            .map(|index| tokens.metadata_at(index).width(tokens.stored_width_at(index)))
            .sum();
        let mut cursor = x;
        for index in range {
            xs[index] = cursor;
            cursor += tokens.metadata_at(index).width(tokens.stored_width_at(index));
        }
        x += width;
    }
    xs
}

fn plain_text_placements<S: TokenRead>(tokens: S) -> Vec<TokenPlacement> {
    let mut x = 0.0;
    (0..tokens.len())
        .map(move |index| {
            let advance = tokens.metadata_at(index).width(tokens.stored_width_at(index));
            let placement = TokenPlacement {
                x,
                advance,
                y_offset: 0.0,
                relative_offset: Vec2::ZERO,
            };
            x += advance;
            placement
        })
        .collect()
}

fn place_inline_edges(
    engine: &LayoutEngine<'_, '_>,
    span: TokenSpan<'_>,
    runs: &[InlineTokenMetrics],
    replaced: &[ReplacedToken],
    clusters: ClusterRanges<'_>,
    visual_order: &mut Vec<usize>,
) {
    // CSS assigns an inline's outer edges to its visual extremes on the
    // first/last line, independently of embedding controls inside its text.
    // Resolve descendants first so their edges remain inside ancestor edges.
    let mut edges = Vec::new();
    for (cluster_idx, cluster) in clusters.iter().enumerate() {
        let InlineTokenKind::InlineBoundary { payload_idx } =
            span.metadata_at(cluster.start).kind()
        else {
            continue;
        };
        let Some(ReplacedToken::InlineBoundary {
            box_idx,
            inline_start,
            inline_end,
            ..
        }) = replaced.get(payload_idx as usize)
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
            clusters.range(cluster_idx).any(|index| {
                let token = span.metadata_at(index);
                if let InlineTokenKind::InlineBoundary { payload_idx } = token.kind() {
                    if matches!(replaced.get(payload_idx as usize), Some(ReplacedToken::InlineBoundary { box_idx, .. }) if *box_idx as usize == owner) {
                        return false;
                    }
                } else if !matches!(token.kind(), InlineTokenKind::Glyph { .. } | InlineTokenKind::Image { .. } | InlineTokenKind::AtomicBox { .. }) {
                    return false;
                }
                if let InlineTokenKind::Glyph { glyph_idx } = token.kind() {
                    let character = engine.text.character(glyph_idx);
                    if is_bidi_control(character) {
                        return false;
                    }
                }
                let mut current = Some(span.metrics_view(runs).at(span, index).owner_box_idx as usize)
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
            visual_order
                .iter()
                .copied()
                .rev()
                .find(|&index| belongs(index))
        };
        let Some(anchor) = anchor else { continue };
        visual_order.retain(|&index| index != edge);
        let position = visual_order
            .iter()
            .position(|&index| index == anchor)
            .expect("inline edge anchor remains in visual order");
        visual_order.insert(position + usize::from(!left), edge);
    }
}
