//! Build immutable style and geometry inputs before execution.
use super::LayoutInputs;
use super::paragraph_spacing::ParagraphSpacing;
use crate::boxes::model::ResolvedBoxModel;
use html_dom::{Document, DomNodeId};
use html_engine_model::{LayoutMode, LayoutTree};
use html_style_model::{UsedPreferredSize as PreferredSize, UsedStyleView};
use html_text::{is_mandatory_line_break, suppresses_glyph};
use rustc_data_structures::fx::FxHashMap;

/// Complete immutable snapshots for one layout pass.
pub(crate) struct PassContext<'input> {
    pub(super) styles: Vec<UsedStyleView<'input>>,
    pub(super) box_models: Vec<ResolvedBoxModel>,
    pub(super) box_model_indices: Vec<u32>,
    pub(super) sizes: Vec<BoxSizeSnapshot>,
    pub(super) size_indices: Vec<u32>,
    pub(super) paragraph_spacing: ParagraphSpacing,
    pub(super) tracking_spacing: Option<FxHashMap<u32, f32>>,
    pub(super) percentage_height_images: FxHashMap<DomNodeId, Vec<usize>>,
}

/// Font-relative resolution is independent of containing-block dimensions.
/// Shared by identical styles and font ratios, rather than copied per box.
pub(crate) struct BoxSizeSnapshot {
    pub width: PreferredSize,
    pub height: PreferredSize,
    pub min_width: PreferredSize,
    pub min_height: PreferredSize,
    pub max_width: PreferredSize,
    pub max_height: PreferredSize,
    pub flex_basis: PreferredSize,
}

impl<'input> PassContext<'input> {
    pub(crate) fn new(inputs: &LayoutInputs<'input>) -> Self {
        let LayoutInputs {
            document,
            inline_content,
            styles,
            topology,
            font_metrics,
            ..
        } = inputs;
        let mut resolved = Vec::with_capacity(topology.box_count());
        let mut box_models = vec![ResolvedBoxModel::default()];
        let mut box_model_indices = Vec::with_capacity(topology.box_count());
        let mut sizes = Vec::new();
        let mut size_indices = Vec::with_capacity(topology.box_count());
        let mut style_keys = FxHashMap::default();
        let mut candidates = FxHashMap::default();
        for box_idx in 0..topology.box_count() {
            let indices = topology
                .get_box_style_indices(box_idx)
                .unwrap_or_else(|| styles.default_indices());
            let key = (indices, font_metrics.box_metric_id(box_idx));
            let (style, size_index, model_index) = *style_keys.entry(key).or_insert_with(|| {
                let style = font_metrics
                    .used_style(styles, indices, box_idx)
                    .expect("validated style handle");
                let model = ResolvedBoxModel::new(style, 0.0);
                let model_index = if model.is_zero() {
                    0
                } else {
                    let index = u32::try_from(box_models.len()).expect("box model index fits u32");
                    box_models.push(model);
                    index
                };

                let index = u32::try_from(sizes.len()).expect("style size index fits u32");
                sizes.push(BoxSizeSnapshot {
                    width: style.width(),
                    height: style.height(),
                    min_width: style.min_width(),
                    min_height: style.min_height(),
                    max_width: style.max_width(),
                    max_height: style.max_height(),
                    flex_basis: style.flex_basis(),
                });
                (style, index, model_index)
            });
            resolved.push(style);
            if matches!(style.height(), PreferredSize::Percent(_))
                && topology.get_box_image_idx(box_idx).is_some()
                && let Some(node) = topology.get_box_dom_element(box_idx)
            {
                candidates.entry(node).or_insert(box_idx);
            }
            size_indices.push(size_index);
            box_model_indices.push(model_index);
        }
        let paragraph_spacing =
            ParagraphSpacing::new(document, styles, topology, inline_content, &resolved);
        let tracking_spacing = prepare_tracking_spacing(inputs, &resolved);
        let percentage_height_images =
            prepare_percentage_height_images(document, topology, &resolved, candidates);
        Self {
            styles: resolved,
            box_models,
            box_model_indices,
            sizes,
            size_indices,
            paragraph_spacing,
            tracking_spacing,
            percentage_height_images,
        }
    }
}

fn prepare_percentage_height_images(
    document: &Document,
    topology: &LayoutTree,
    styles: &[UsedStyleView<'_>],
    mut candidates: FxHashMap<DomNodeId, usize>,
) -> FxHashMap<DomNodeId, Vec<usize>> {
    let mut children: FxHashMap<DomNodeId, Vec<usize>> = FxHashMap::default();
    if candidates.is_empty() {
        return children;
    }
    // A DOM element can generate multiple boxes. Preserve the previous query's
    // first-box semantics, including when that first box is not an image.
    for box_idx in 0..topology.box_count() {
        if let Some(node) = topology.get_box_dom_element(box_idx)
            && let Some(first) = candidates.get_mut(&node)
        {
            *first = (*first).min(box_idx);
        }
    }
    for (node, box_idx) in candidates {
        if topology.get_box_image_idx(box_idx).is_some()
            && matches!(styles[box_idx].height(), PreferredSize::Percent(_))
            && let Some(parent) = document
                .try_element_ref(node)
                .and_then(|element| element.parent())
        {
            children.entry(parent).or_default().push(box_idx);
        }
    }
    children
}

// Only exceptional boundaries need storage. Most documents need neither this
// pass nor a map: all containing elements have zero letter-spacing.
fn prepare_tracking_spacing(
    inputs: &LayoutInputs<'_>,
    styles: &[UsedStyleView<'_>],
) -> Option<FxHashMap<u32, f32>> {
    if styles.iter().all(|style| style.letter_spacing() == 0.0) {
        return None;
    }
    let mut spacing = FxHashMap::default();
    let content = inputs.inline_content;
    let mut runs = content
        .text_runs()
        .filter(|run| !run.glyphs.is_empty())
        .rev()
        .peekable();
    let level = |index| {
        inputs
            .text_geometry
            .map_or(0, |geometry| geometry.bidi().level(index))
    };
    let mut next = content.characters().len() as u32;
    let mut next_owner = None;
    // Walking backwards carries the next visible character and its owner even
    // through format controls, without looking up either position again.
    for (index, &character) in content.characters().iter().enumerate().rev() {
        let glyph = index as u32;
        while runs.peek().is_some_and(|run| run.glyphs.start > glyph) {
            runs.next();
        }
        let run = runs.peek().filter(|run| run.glyphs.contains(&glyph));
        let owner = run.map(|run| run.box_idx as usize);
        if !suppresses_glyph(character)
            && let Some(run) = run
        {
            let owner = run.box_idx as usize;
            let fragmented = level(glyph) != level(next);
            let next_character = content.character_at(next as usize).unwrap_or('\0');
            let hard_break = next_character == '\n' || is_mandatory_line_break(next_character);
            if next >= run.glyphs.end || fragmented || hard_break {
                let value = next_owner.map_or(0.0, |other| {
                    if hard_break {
                        0.0
                    } else {
                        boundary_tracking_spacing(inputs.topology, styles, owner, other, fragmented)
                    }
                });
                if value != styles[owner].letter_spacing() {
                    spacing.insert(glyph, value);
                }
            }
        }
        if !suppresses_glyph(character) || character == '\u{00ad}' {
            next = glyph;
            next_owner = owner;
        }
    }
    Some(spacing)
}

// Cross-fragment gaps use the common containing element. A bidi level change
// fragments intervening inlines, so its spacing comes from their block context.
fn boundary_tracking_spacing(
    topology: &LayoutTree,
    styles: &[UsedStyleView<'_>],
    owner: usize,
    other: usize,
    fragmented: bool,
) -> f32 {
    let context = |mut node| {
        while matches!(
            topology.get_box_layout_mode(node),
            Some(LayoutMode::Inline(_))
        ) {
            let Some(parent) = topology.get_box_parent(node) else {
                break;
            };
            node = parent;
        }
        node
    };
    if context(owner) != context(other) {
        return 0.0;
    }
    let mut left = owner;
    let mut right = other;
    let left_depth = topology.ancestors(left).count();
    let right_depth = topology.ancestors(right).count();
    for _ in right_depth..left_depth {
        let Some(parent) = topology.get_box_parent(left) else {
            return styles[owner].letter_spacing();
        };
        left = parent;
    }
    for _ in left_depth..right_depth {
        let Some(parent) = topology.get_box_parent(right) else {
            return styles[owner].letter_spacing();
        };
        right = parent;
    }
    while left != right {
        let (Some(a), Some(b)) = (
            topology.get_box_parent(left),
            topology.get_box_parent(right),
        ) else {
            return styles[owner].letter_spacing();
        };
        left = a;
        right = b;
    }
    if fragmented {
        left = context(left);
    }
    styles[left].letter_spacing()
}
