use html_engine_model::{
    Children, FontRelativeMetrics, ImageMetrics, ImageSizingPolicy, InlineContent, InlineItem,
    InlineItemKind, LayoutMode, LayoutTree, ListItemMarker, PaintView, ShapedFontMetrics, TableBox};
use super::block::ParagraphSpacing;
use super::box_model::ResolvedBoxModel;
use super::engine::LayoutInputs;
use html_dom::{DomNodeId, Document};
use html_text::{is_mandatory_line_break, suppresses_glyph};
use html_style_model::{
    BoxSizing, ComputedStyles, Float, LayoutStyle, PositionMode, StyleIndices,
    UsedPreferredSize as PreferredSize, UsedStyleView,
};
use kurbo::Size;
use rustc_data_structures::fx::FxHashMap;

#[derive(Clone, Copy, Debug)]
pub struct ImageIntrinsic {
    pub size: Size,
    pub aspect_ratio: Option<f64>,
}

/// Shared read-only access to the document, styled layout topology, and
/// renderer-supplied metrics used during layout.
#[derive(Clone, Copy)]
pub struct LayoutReader<'input> {
    document: &'input Document,
    styles: &'input ComputedStyles,
    topology: &'input LayoutTree,
    inline_content: &'input InlineContent,
    font_metrics: &'input ShapedFontMetrics,
    image_metrics: &'input ImageMetrics,
    pass_context: &'input PassContext<'input>,
    paragraph_spacing: &'input ParagraphSpacing,
    tracks_overflow_clips: bool,
    has_floats: bool,
    paint: Option<PaintView<'input>>,
}

/// Immutable style snapshots for one layout pass. Paint-only replay
/// builds the style views and document flags without geometry caches or spacing.
pub(super) struct PassContext<'input> {
    styles: Vec<UsedStyleView<'input>>,
    box_models: Vec<ResolvedBoxModel>,
    box_model_indices: Vec<u32>,
    sizes: Vec<BoxSizeSnapshot>,
    size_indices: Vec<u32>,
    paragraph_spacing: ParagraphSpacing,
    tracking_spacing: Option<FxHashMap<u32, f32>>,
    percentage_height_images: FxHashMap<DomNodeId, Vec<usize>>,
    tracks_overflow_clips: bool,
    has_floats: bool,
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
    pub(super) fn new(inputs: &LayoutInputs<'input>) -> Self {
        Self::build(inputs, true)
    }

    pub(super) fn for_paint(inputs: &LayoutInputs<'input>) -> Self {
        Self::build(inputs, false)
    }

    fn build(inputs: &LayoutInputs<'input>, layout: bool) -> Self {
        let LayoutInputs {
            document,
            inline_content,
            styles,
            topology,
            font_metrics,
            ..
        } = inputs;
        let mut resolved = Vec::with_capacity(topology.box_count());
        let mut box_models = if layout {
            vec![ResolvedBoxModel::default()]
        } else {
            Vec::new()
        };
        let mut box_model_indices =
            Vec::with_capacity(if layout { topology.box_count() } else { 0 });
        let mut sizes = Vec::new();
        let mut size_indices = Vec::with_capacity(if layout { topology.box_count() } else { 0 });
        let mut size_keys = FxHashMap::default();
        let mut tracks_overflow_clips = false;
        let mut has_floats = false;
        let mut percentage_height_images = FxHashMap::default();
        for box_idx in 0..topology.box_count() {
            let indices = topology
                .get_box_style_indices(box_idx)
                .unwrap_or_else(|| styles.default_indices());
            let style = font_metrics
                .used_style(styles, indices, box_idx)
                .expect("validated style handle");
            tracks_overflow_clips |= style.overflow_x().clips() || style.overflow_y().clips();
            has_floats |= style.position() != PositionMode::Absolute
                && matches!(style.float(), Float::Left | Float::Right);
            resolved.push(style);
            if !layout {
                continue;
            }
            if matches!(style.height(), PreferredSize::Percent(_))
                && topology.get_box_image_idx(box_idx).is_some()
                && let Some(node) = topology.get_box_dom_element(box_idx)
            {
                percentage_height_images.entry(node).or_insert(box_idx);
            }
            let metrics = font_metrics.for_box(box_idx);
            let key = (
                indices,
                [
                    metrics.x_height_ratio.to_bits(),
                    metrics.ch_advance_ratio.to_bits(),
                    metrics.cap_height_ratio.to_bits(),
                ],
            );
            let (size_index, model_index) = *size_keys.entry(key).or_insert_with(|| {
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
                (index, model_index)
            });
            size_indices.push(size_index);
            box_model_indices.push(model_index);
        }
        let paragraph_spacing = if layout {
            ParagraphSpacing::new(document, styles, topology, inline_content, &resolved)
        } else {
            ParagraphSpacing::default()
        };
        let tracking_spacing = if layout {
            prepare_tracking_spacing(inputs, &resolved)
        } else {
            None
        };
        let percentage_height_images = prepare_percentage_height_images(
            document, topology, &resolved, percentage_height_images,
        );
        Self {
            styles: resolved,
            box_models,
            box_model_indices,
            sizes,
            size_indices,
            paragraph_spacing,
            tracking_spacing,
            percentage_height_images,
            tracks_overflow_clips,
            has_floats,
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
            && let Some(parent) = document.try_element_ref(node).and_then(|element| element.parent())
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
    let mut runs = content.text_runs().filter(|run| !run.glyphs.is_empty()).rev().peekable();
    let level = |index| inputs.text_geometry.map_or(0, |geometry| geometry.bidi().level(index));
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
        if !suppresses_glyph(character) {
            if let Some(run) = run {
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
        while matches!(topology.get_box_layout_mode(node), Some(LayoutMode::Inline(_))) {
            let Some(parent) = topology.get_box_parent(node) else { break };
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
        let Some(parent) = topology.get_box_parent(left) else { return styles[owner].letter_spacing() };
        left = parent;
    }
    for _ in left_depth..right_depth {
        let Some(parent) = topology.get_box_parent(right) else { return styles[owner].letter_spacing() };
        right = parent;
    }
    while left != right {
        let (Some(a), Some(b)) = (topology.get_box_parent(left), topology.get_box_parent(right)) else {
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

impl<'input> LayoutReader<'input> {
    pub(super) fn new(
        inputs: &LayoutInputs<'input>,
        pass_context: &'input PassContext<'input>,
    ) -> Self {
        Self {
            document: inputs.document,
            styles: inputs.styles,
            topology: inputs.topology,
            inline_content: inputs.inline_content,
            font_metrics: inputs.font_metrics,
            image_metrics: inputs.image_metrics,
            pass_context,
            paragraph_spacing: &pass_context.paragraph_spacing,
            tracks_overflow_clips: pass_context.tracks_overflow_clips,
            has_floats: pass_context.has_floats,
            paint: None,
        }
    }

    pub fn with_paint(mut self, paint: PaintView<'input>) -> Self {
        self.paint = Some(paint);
        self
    }

    pub fn document(&self) -> &'input Document {
        self.document
    }

    /// Geometry reads never consult the reader palette.
    pub fn style(&self, box_idx: usize) -> UsedStyleView<'input> {
        self.pass_context.styles[box_idx]
    }

    pub(super) fn has_tracking_spacing(&self) -> bool {
        self.pass_context.tracking_spacing.is_some()
    }

    /// Boundary values are shared by intrinsic sizing and final token construction.
    pub(super) fn tracking_spacing_after(&self, glyph: u32, owner: usize) -> f64 {
        self.pass_context.tracking_spacing.as_ref().map_or(0.0, |spacing| {
            f64::from(spacing.get(&glyph).copied().unwrap_or_else(|| self.style(owner).letter_spacing()))
        })
    }

    pub(super) fn style_ref(&self, box_idx: usize) -> &'input UsedStyleView<'input> {
        &self.pass_context.styles[box_idx]
    }

    pub(crate) fn sizes(&self, box_idx: usize) -> &'input BoxSizeSnapshot {
        &self.pass_context.sizes[self.pass_context.size_indices[box_idx] as usize]
    }

    pub fn box_model(&self, box_idx: usize, containing_width: f64) -> ResolvedBoxModel {
        if containing_width.to_bits() == 0 {
            self.pass_context.box_models[self.pass_context.box_model_indices[box_idx] as usize]
        } else {
            ResolvedBoxModel::new(self.style(box_idx), containing_width)
        }
    }

    pub(super) fn adjust_vertical_margins(
        &self,
        box_idx: usize,
        top: f64,
        bottom: f64,
    ) -> (f64, f64) {
        self.paragraph_spacing.resolve(box_idx, top, bottom)
    }

    pub fn reader_text_indent(&self, box_idx: usize) -> Option<f64> {
        self.paragraph_spacing.indent(box_idx)
    }

    pub fn reader_paragraph_gap(&self, box_idx: usize) -> Option<f64> {
        self.paragraph_spacing.gap(box_idx)
    }

    pub fn is_reader_prose(&self, box_idx: usize) -> bool {
        self.paragraph_spacing.is_prose(box_idx)
    }

    pub fn vertical_margins(&self, box_idx: usize, containing_width: f64) -> (f64, f64) {
        let style = self.style(box_idx);
        self.paragraph_spacing.resolve(box_idx, style.margin_top().resolve(containing_width), style.margin_bottom().resolve(containing_width))
    }

    pub fn paint_style(&self, box_idx: usize) -> UsedStyleView<'input> {
        let style = self.style(box_idx);
        self.paint.and_then(|paint| paint.box_colors(box_idx)).map_or(style, |colors| style.with_paint_colors(colors))
    }

    pub fn background_can_change(&self, box_idx: usize) -> bool {
        let indices = self.topology.get_box_style_indices(box_idx).unwrap_or_else(|| self.styles.default_indices());
        self.style(box_idx).background_color() & 0xff != 0
            || self.styles.background_style(indices).is_some_and(|background| background.background_color_current_color)
            || self.topology.root_box() == Some(box_idx) || self.topology.body_box() == Some(box_idx)
    }

    pub fn tracks_overflow_clips(&self) -> bool {
        self.tracks_overflow_clips
    }

    pub fn has_floats(&self) -> bool {
        self.has_floats
    }

    pub fn used_style(&self, indices: StyleIndices) -> UsedStyleView<'input> {
        self.font_metrics.used_non_box_style(self.styles, indices).expect("validated style handle")
    }

    pub fn paint_used_style(&self, indices: StyleIndices, context: usize) -> UsedStyleView<'input> {
        let style = self.used_style(indices);
        self.paint.and_then(|paint| paint.style_colors(indices, context)).map_or(style, |colors| style.with_paint_colors(colors))
    }

    pub fn font_metrics(&self, box_idx: usize) -> FontRelativeMetrics {
        self.font_metrics.for_box(box_idx)
    }

    pub fn style_font_metrics(&self, indices: StyleIndices) -> FontRelativeMetrics {
        self.font_metrics.for_non_box_style(indices)
    }

    pub fn styles(&self) -> &'input ComputedStyles {
        self.styles
    }

    pub fn layout_tree(&self) -> &'input LayoutTree {
        self.topology
    }

    pub fn box_dom_element(&self, box_idx: usize) -> Option<DomNodeId> {
        self.topology.get_box_dom_element(box_idx)
    }

    pub fn image_idx(&self, box_idx: usize) -> Option<u32> {
        self.topology.get_box_image_idx(box_idx)
    }

    pub fn image_display_size(&self, image_idx: u32) -> (f64, f64) {
        self.image_metrics.display_size(
            image_idx,
            self.document
                .image(image_idx)
                .expect("image index should be valid"),
        )
    }

    pub(super) fn percentage_height_image_children(&self, box_idx: usize) -> &[usize] {
        self.topology.get_box_dom_element(box_idx)
            .and_then(|node| self.pass_context.percentage_height_images.get(&node))
            .map_or(&[], Vec::as_slice)
    }

    pub fn image_intrinsic(&self, box_idx: usize) -> Option<ImageIntrinsic> {
        let image_idx = self.image_idx(box_idx)?;
        let (width, height) = self.image_display_size(image_idx);
        let size = Size::new(width, height);
        Some(ImageIntrinsic {
            size,
            aspect_ratio: self.image_intrinsic_ratio(box_idx, size),
        })
    }

    fn image_intrinsic_ratio(&self, box_idx: usize, intrinsic: Size) -> Option<f64> {
        if let Some(element) = self.topology.element_for_box(box_idx, self.document)
            && element.tag().eq_ignore_ascii_case("svg")
        {
            if let Some(view_box) = element.attr("viewBox").or_else(|| element.attr("viewbox")) {
                let mut parts = view_box
                    .split(|character: char| character.is_ascii_whitespace() || character == ',')
                    .filter(|value| !value.is_empty());
                let mut values = [0.0; 4];
                for value in &mut values {
                    *value = parts.next()?.parse::<f64>().ok()?;
                }
                if parts.next().is_some() || values[2] <= 0.0 || values[3] <= 0.0 {
                    return None;
                }
                return Some(values[2] / values[3]);
            }
            return None;
        }
        (intrinsic.height > 0.0).then_some(intrinsic.width / intrinsic.height)
    }

    /// Returns a renderer-supplied column-filling width for an eligible
    /// standalone image, capped so its outer height fits the viewport.
    pub fn smart_standalone_image_width(
        &self,
        policy: ImageSizingPolicy,
        box_idx: usize,
        intrinsic: Size,
        available_width: f64,
        viewport_height: Option<f64>,
        horizontal_padding_border: f64,
        vertical_outer_inset: f64,
        standalone: bool,
    ) -> Option<PreferredSize> {
        const MIN_SOURCE_WIDTH: f64 = 96.0;

        if self.image_idx(box_idx).and_then(|index| self.document.image(index))
            .is_some_and(|image| matches!(image.source, html_dom::ImageSource::MathMl { .. }))
        {
            return None;
        }

        if policy != ImageSizingPolicy::SmartStandalone
            || !standalone
            || intrinsic.width < MIN_SOURCE_WIDTH
        {
            return None;
        }
        let style = self.style(box_idx);
        if style.height() != PreferredSize::Auto
            || style.min_height() != PreferredSize::Auto
            || style.max_height() != PreferredSize::Auto
        {
            return None;
        }

        let element = self.topology.element_for_box(box_idx, self.document)?;
        if element.attr("height").is_some() {
            return None;
        }

        let mut expanded_content_width = (available_width - horizontal_padding_border).max(0.0);
        if intrinsic.height > 0.0
            && let Some(viewport_height) = viewport_height
        {
            let available_content_height = (viewport_height - vertical_outer_inset).max(0.0);
            expanded_content_width = expanded_content_width
                .min(available_content_height * intrinsic.width / intrinsic.height);
        }
        let preferred = match style.box_sizing() {
            BoxSizing::ContentBox => expanded_content_width,
            BoxSizing::BorderBox => {
                expanded_content_width + horizontal_padding_border
            }
        };
        Some(PreferredSize::Px(preferred as f32))
    }

    pub fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.topology.get_box_style_indices(box_idx)
    }

    pub fn layout_style(&self, box_idx: usize) -> &'input LayoutStyle {
        let indices = self
            .box_style_indices(box_idx)
            .unwrap_or_else(|| self.styles.default_indices());
        self.styles
            .layout_style(indices)
            .expect("validated style handle")
    }

    pub fn inline_fragment_edges(&self, box_idx: usize) -> (bool, bool) {
        self.topology.inline_fragment_edges(box_idx)
    }

    pub fn split_inline_position_ancestors(&self, box_idx: usize) -> &[StyleIndices] {
        self.topology.split_inline_position_ancestors(box_idx)
    }

    pub fn box_count(&self) -> usize {
        self.topology.box_count()
    }

    pub fn box_layout_mode(&self, box_idx: usize) -> Option<&'input LayoutMode> {
        self.topology.get_box_layout_mode(box_idx)
    }

    pub fn box_table(&self, box_idx: usize) -> Option<&'input TableBox> {
        self.topology.get_box_table(box_idx)
    }

    /// Whether an inline-run child range creates an in-flow line box. Floats
    /// and absolutely positioned descendants leave anchors in the run stream,
    /// but those anchors do not give their containing block a line box or
    /// prevent its vertical margins from collapsing through.
    pub fn inline_items_establish_line_box(
        &self,
        container_box_idx: usize,
        runs: &std::ops::Range<u32>,
    ) -> bool {
        self.inline_content.inline_items()[runs.start as usize..runs.end as usize]
            .iter()
            .any(|run| {
                if !self.inline_item_belongs_to_context(run, container_box_idx) {
                    return false;
                }
                match &run.kind {
                    InlineItemKind::FloatAnchor { .. } | InlineItemKind::AbsoluteAnchor { .. } => {
                        false
                    }
                    InlineItemKind::Text { glyphs } => {
                        let preserves_space = self
                            .style(run.box_idx as usize)
                            .white_space()
                            .preserves_spaces();
                        glyphs.clone().any(|glyph_idx| {
                            let character = self
                                .inline_content
                                .character_at(glyph_idx as usize)
                                .unwrap_or_default();
                            preserves_space || !html_text::is_document_white_space(character)
                        })
                    }
                    InlineItemKind::Image { .. }
                    | InlineItemKind::Break { .. }
                    | InlineItemKind::InlineBoundary { .. }
                    | InlineItemKind::AtomicBox { .. }
                    | InlineItemKind::Marker { .. } => true,
                }
            })
    }

    /// Float sides represented only by anchors in this block's inline-run
    /// stream. Anchor-only streams do not establish a line box, but their
    /// float sources still participate in adjoining-margin/clearance rules.
    pub fn inline_item_float_sides(
        &self,
        container_box_idx: usize,
        runs: &std::ops::Range<u32>,
    ) -> (bool, bool) {
        let mut left = false;
        let mut right = false;
        for run in &self.inline_content.inline_items()[runs.start as usize..runs.end as usize] {
            let InlineItemKind::FloatAnchor { box_idx } = run.kind else {
                continue;
            };
            if !self.inline_item_belongs_to_context(run, container_box_idx) {
                continue;
            }
            match self.style(box_idx as usize).float() {
                Float::Left => left = true,
                Float::Right => right = true,
                Float::None => {}
            }
        }
        (left, right)
    }

    /// Inline items stop at their formatting-context root rather than leaking
    /// into ancestor lines. An atomic box's parent owns its placeholder.
    pub fn inline_item_belongs_to_context(&self, item: &InlineItem, container_box_idx: usize) -> bool {
        let mut current = match item.kind {
            InlineItemKind::AtomicBox { box_idx } => self.get_parent(box_idx as usize),
            _ => Some(item.box_idx as usize),
        };
        while let Some(box_idx) = current {
            if box_idx == container_box_idx {
                return true;
            }
            if matches!(
                self.box_layout_mode(box_idx),
                Some(
                    LayoutMode::Block(_)
                        | LayoutMode::Table(_)
                        | LayoutMode::TableRow(_)
                        | LayoutMode::TableCell(_)
                        | LayoutMode::Flex(_)
                        | LayoutMode::Grid(_)
                )
            ) {
                return false;
            }
            current = self.get_parent(box_idx);
        }
        false
    }

    pub fn get_parent(&self, box_idx: usize) -> Option<usize> {
        self.topology.get_box_parent(box_idx)
    }

    pub fn root_box(&self) -> Option<usize> {
        self.topology.root_box()
    }

    pub fn body_box(&self) -> Option<usize> {
        self.topology.body_box()
    }

    /// CSS Overflow propagates the HTML body element's overflow to the
    /// viewport when both root axes are `visible`. The body's used overflow
    /// then becomes `visible`, so it must not create a block formatting
    /// context or clip its own descendants.
    pub fn effective_overflow_modes(
        &self,
        box_idx: usize,
    ) -> (
        html_style_model::OverflowMode,
        html_style_model::OverflowMode,
    ) {
        let style = self.style(box_idx);
        if self.body_box() == Some(box_idx)
            && self.root_box().is_some_and(|root_idx| {
                let root = self.style(root_idx);
                root.overflow_x() == html_style_model::OverflowMode::Visible
                    && root.overflow_y() == html_style_model::OverflowMode::Visible
            })
        {
            (
                html_style_model::OverflowMode::Visible,
                html_style_model::OverflowMode::Visible,
            )
        } else {
            (style.overflow_x(), style.overflow_y())
        }
    }

    /// Whether this box supplies the canvas background instead of painting a
    /// separate element background. CSS propagates the root background to the
    /// canvas; for HTML documents whose root has no background, the body
    /// background is propagated instead.
    pub fn background_paints_on_canvas(&self, box_idx: usize) -> bool {
        let Some(root_box) = self.root_box() else {
            return false;
        };
        let root = self.paint_style(root_box);
        if root.background_image_present() || root.background_color() & 0xFF != 0 {
            return box_idx == root_box;
        }
        let Some(body_box) = self.body_box() else {
            return false;
        };
        let body = self.paint_style(body_box);
        (body.background_image_present() || body.background_color() & 0xFF != 0)
            && box_idx == body_box
    }

    pub fn is_table_cell_box(&self, box_idx: usize) -> bool {
        matches!(
            self.box_layout_mode(box_idx),
            Some(LayoutMode::TableCell(_))
        )
    }

    pub fn is_block_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Block(_)))
    }

    pub fn is_flex_grid_box(&self, box_idx: usize) -> bool {
        matches!(
            self.box_layout_mode(box_idx),
            Some(LayoutMode::Flex(_) | LayoutMode::Grid(_))
        )
    }

    pub fn is_anonymous_box(&self, box_idx: usize) -> bool {
        matches!(
            self.box_layout_mode(box_idx),
            Some(LayoutMode::Anonymous(_))
        )
    }

    pub fn is_table_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Table(_)))
    }

    pub fn is_inline_box(&self, box_idx: usize) -> bool {
        matches!(self.box_layout_mode(box_idx), Some(LayoutMode::Inline(_)))
    }

    pub fn is_only_block_child(&self, box_idx: usize) -> bool {
        let Some(parent) = self.get_parent(box_idx) else {
            return false;
        };
        matches!(
            self.box_layout_mode(parent),
            Some(LayoutMode::Block(block)) if matches!(&block.children, Children::Blocks(children) if children.as_slice() == [box_idx as u32])
        )
    }

    pub fn list_marker(&self, box_idx: usize) -> Option<ListItemMarker> {
        self.topology.list_marker(box_idx)
    }

    pub fn establishes_formatting_context(&self, box_idx: usize) -> bool {
        let style = self.style(box_idx);
        let (overflow_x, overflow_y) = self.effective_overflow_modes(box_idx);
        super::block::establishes_formatting_context(
            style.display(),
            overflow_x,
            overflow_y,
            matches!(style.float(), Float::Left | Float::Right),
            self.root_box() == Some(box_idx),
            self.is_table_cell_box(box_idx),
            self.get_parent(box_idx)
                .is_some_and(|parent| self.is_flex_grid_box(parent)),
        )
    }

    pub fn box_uses_float_context(&self, box_idx: usize) -> bool {
        self.is_table_box(box_idx)
            || self.establishes_formatting_context(box_idx)
            || self.style(box_idx).position() == PositionMode::Absolute
    }

    pub fn box_uses_ahem(&self, box_idx: usize) -> bool {
        let indices = self
            .topology
            .get_box_style_indices(box_idx)
            .unwrap_or_else(|| self.styles.default_indices());
        self.styles
            .view(indices)
            .and_then(|style| style.font_family())
            .and_then(|family| self.styles.string(family))
            .is_some_and(|family| family.eq_ignore_ascii_case("Ahem"))
    }
}
