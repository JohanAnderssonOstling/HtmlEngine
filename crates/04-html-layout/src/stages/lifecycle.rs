//! Prepared, shaped, and laid-out document lifecycle types.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutConstraints {
    viewport_width: f64,
    viewport_height: Option<f64>,
    line_height: f64,
    image_sizing_policy: ImageSizingPolicy,
    text_composition_policy: TextCompositionPolicy,
}

/// Optional renderer-level adaptation for standalone images. Web-compatible
/// sizing remains the default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageSizingPolicy {
    #[default]
    WebCompatible,
    /// Make a sufficiently large image fill its column when it is the sole
    /// meaningful content of its block, overriding authored horizontal image
    /// sizing and margins while preserving its aspect ratio.
    SmartStandalone,
}

/// Whether note bodies generate boxes in the reading flow.
///
/// Notes are recognised by the same predicate that answers
/// [`RenderAddressingView::is_note_target`], so an embedder that holds notes
/// back for its own presentation never has to restate what a note is. Excluding
/// them suppresses box generation exactly as `display: none` would, leaving the
/// subtree in the DOM so link targets and scoped layout still resolve.
///
/// In-flow is the default: markup lays out as authored unless an embedder asks
/// otherwise.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum NoteFlow {
    #[default]
    InFlow,
    Excluded,
}

/// Selects browser-compatible first-fit wrapping or paragraph-wide book
/// composition. Web-compatible wrapping remains the public default so layout
/// tests and embedders do not silently acquire different line breaks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextCompositionPolicy {
    #[default]
    WebCompatible,
    /// Paragraph-wide book composition with conservative hyphenation quality
    /// controls enabled.
    BookOptimized,
    /// Paragraph-wide book composition without the optional consecutive-run,
    /// paragraph-ending, fragment-length, and short-line hyphen safeguards.
    BookOptimizedUnrestrictedHyphenation,
    /// Experimental book composition that starts every Unicode sentence on a
    /// new logical line without inserting characters into the source text.
    SentencePerLine,
}

impl TextCompositionPolicy {
    pub fn is_book_optimized(self) -> bool {
        matches!(
            self,
            Self::BookOptimized
                | Self::BookOptimizedUnrestrictedHyphenation
                | Self::SentencePerLine
        )
    }

    pub fn uses_hyphenation_quality(self) -> bool {
        matches!(self, Self::BookOptimized | Self::SentencePerLine)
    }

    pub fn uses_sentence_per_line(self) -> bool {
        self == Self::SentencePerLine
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutConstraintError {
    InvalidViewportWidth,
    InvalidViewportHeight,
    InvalidLineHeight,
}

impl LayoutConstraints {
    pub fn new(viewport_width: f64, line_height: f64) -> Result<Self, LayoutConstraintError> {
        if !viewport_width.is_finite() || viewport_width < 0.0 {
            return Err(LayoutConstraintError::InvalidViewportWidth);
        }
        if !line_height.is_finite() || line_height <= 0.0 {
            return Err(LayoutConstraintError::InvalidLineHeight);
        }
        Ok(Self {
            viewport_width,
            viewport_height: None,
            line_height,
            image_sizing_policy: ImageSizingPolicy::WebCompatible,
            text_composition_policy: TextCompositionPolicy::WebCompatible,
        })
    }

    pub fn viewport_width(self) -> f64 {
        self.viewport_width
    }

    pub fn with_viewport_height(
        mut self,
        viewport_height: Option<f64>,
    ) -> Result<Self, LayoutConstraintError> {
        if viewport_height.is_some_and(|height| !height.is_finite() || height < 0.0) {
            return Err(LayoutConstraintError::InvalidViewportHeight);
        }
        self.viewport_height = viewport_height;
        Ok(self)
    }

    pub fn viewport_height(self) -> Option<f64> {
        self.viewport_height
    }

    pub fn line_height(self) -> f64 {
        self.line_height
    }

    pub fn with_image_sizing_policy(mut self, policy: ImageSizingPolicy) -> Self {
        self.image_sizing_policy = policy;
        self
    }

    pub fn image_sizing_policy(self) -> ImageSizingPolicy {
        self.image_sizing_policy
    }

    pub fn with_text_composition_policy(mut self, policy: TextCompositionPolicy) -> Self {
        self.text_composition_policy = policy;
        self
    }

    pub fn text_composition_policy(self) -> TextCompositionPolicy {
        self.text_composition_policy
    }
}

impl fmt::Display for LayoutConstraintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidViewportWidth => {
                formatter.write_str("viewport width must be finite and non-negative")
            }
            Self::InvalidViewportHeight => {
                formatter.write_str("viewport height must be finite and non-negative")
            }
            Self::InvalidLineHeight => {
                formatter.write_str("line height must be finite and positive")
            }
        }
    }
}

impl std::error::Error for LayoutConstraintError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrepareError(ComputedStylesValidationError);

impl PrepareError {
    pub fn reason(self) -> ComputedStylesValidationError {
        self.0
    }
}

impl fmt::Display for PrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "cannot prepare layout input: {}", self.0)
    }
}

impl std::error::Error for PrepareError {}

/// Styled content with a stable box tree and unshaped inline text.
///
/// This is the only input accepted by [`PreparedDocument::shape`]. Its fields
/// are private so callers cannot pair layout inputs with a different DOM.
/// Skipping shaping is rejected by the type system:
///
/// ```compile_fail
/// fn skip_shaping(document: html_layout::PreparedDocument) {
///     let _ = document.layout(800.0, 20.0);
/// }
/// ```
///
/// Raw style/layout storage is not part of the stage contract:
///
/// ```compile_fail
/// fn reach_through(document: &html_layout::PreparedDocument) {
///     let _ = document.styles();
/// }
/// ```
#[derive(Clone)]
pub(crate) struct PreparedInputs {
    pub(super) document: std::sync::Arc<Document>,
    pub(super) styles: std::sync::Arc<ComputedStyles>,
    pub(super) layout_tree: std::sync::Arc<LayoutTree>,
    pub(super) inline_content: InlineContent,
}

#[derive(Clone)]
pub struct PreparedDocument {
    pub(super) inputs: std::sync::Arc<PreparedInputs>,
}

#[derive(Clone)]
pub(crate) struct ShapedText {
    pub(super) inline_content: InlineContent,
    pub(super) glyph_metrics: GlyphMetrics,
    pub(super) font_metrics: crate::shaping::ShapedFontMetrics,
    pub(super) text_geometry: crate::shaping::ShapedTextGeometry,
    pub(super) ellipsis_glyphs: FxHashMap<u32, GlyphId>,
    pub(super) hyphen_glyphs: FxHashMap<u32, GlyphId>,
    pub(super) link_glyph_targets: FxHashMap<u32, LinkGlyphTarget>,
    pub(super) anchor_glyphs: FxHashMap<u16, u32>,
    pub(super) inline_plans: crate::layout::PreparedInlinePlans,
}

#[derive(Clone)]
pub struct ShapedDocument {
    pub(super) inputs: std::sync::Arc<PreparedInputs>,
    pub(super) shaped: std::sync::Arc<ShapedText>,
}

impl PreparedDocument {
    pub fn try_new(document: Document, styles: ComputedStyles) -> Result<Self, PrepareError> {
        Self::try_new_with_note_flow(document, styles, NoteFlow::default())
    }

    pub fn try_new_with_note_flow(
        document: Document,
        styles: ComputedStyles,
        note_flow: NoteFlow,
    ) -> Result<Self, PrepareError> {
        styles.validate_for(&document).map_err(PrepareError)?;
        let mut layout_tree = LayoutTree::default();
        let mut inline_content = InlineContent::default();
        crate::layout::build_layout_inputs(
            &document,
            &styles,
            &mut layout_tree,
            &mut inline_content,
            note_flow,
        );
        Ok(Self {
            inputs: std::sync::Arc::new(PreparedInputs {
                document: std::sync::Arc::new(document),
                styles: std::sync::Arc::new(styles),
                layout_tree: std::sync::Arc::new(layout_tree),
                inline_content,
            }),
        })
    }

    /// Prepares the subtree rooted at the element carrying `id` as a document
    /// in its own right, so it can be shaped and laid out under constraints of
    /// the caller's choosing -- a popup's width rather than the column's.
    ///
    /// The parse and the computed styles are shared with `self` rather than
    /// recomputed, so this costs a box tree over one subtree. Inherited values
    /// are already resolved, so the subtree keeps the typography it would have
    /// had in place. Notes are in flow here whatever the containing document
    /// asked for: the caller has explicitly asked for this one.
    pub fn scoped_to_element_id(&self, id: &str) -> Option<Self> {
        let document = &self.inputs.document;
        let root = document
            .node_ids()
            .find(|node| document.get_dom_id(*node) == Some(id))?;
        document.element_ref(root)?;

        let mut layout_tree = LayoutTree::default();
        let mut inline_content = InlineContent::default();
        crate::layout::build_layout_inputs_from(
            document,
            &self.inputs.styles,
            &mut layout_tree,
            &mut inline_content,
            NoteFlow::InFlow,
            root,
        );
        Some(Self {
            inputs: std::sync::Arc::new(PreparedInputs {
                document: std::sync::Arc::clone(&self.inputs.document),
                styles: std::sync::Arc::clone(&self.inputs.styles),
                layout_tree: std::sync::Arc::new(layout_tree),
                inline_content,
            }),
        })
    }

    /// Ids of this document's note bodies, in document order. A note without an
    /// id cannot be referenced, so it is not listed.
    pub fn note_ids(&self) -> Vec<&str> {
        let document = &self.inputs.document;
        document
            .node_ids()
            .filter(|node| {
                document
                    .element_ref(*node)
                    .is_some_and(element_is_note_target)
            })
            .filter_map(|node| document.get_dom_id(node))
            .collect()
    }

    pub(crate) fn document(&self) -> &Document {
        self.inputs.document.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn styles(&self) -> &ComputedStyles {
        self.inputs.styles.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn style_view(&self, indices: StyleIndices) -> html_style_model::StyleView<'_> {
        self.inputs
            .styles
            .view(indices)
            .expect("validated style handle")
    }

    #[cfg(test)]
    pub(crate) fn layout_tree(&self) -> &LayoutTree {
        self.inputs.layout_tree.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn inline_content(&self) -> &InlineContent {
        &self.inputs.inline_content
    }

    pub fn box_count(&self) -> usize {
        self.inputs.layout_tree.box_count()
    }

    /// Estimate bytes retained for the prepared stage cache.
    pub fn memory_usage_bytes(&self) -> usize {
        self.memory_usage_report().total_bytes()
    }

    /// Report bytes retained for the prepared stage cache.
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.extend_prefixed(
            "PreparedDocument.document",
            self.document().memory_usage_report(),
        );
        report.extend_prefixed(
            "PreparedDocument.styles",
            self.inputs.styles.memory_usage_report(),
        );
        report.extend_prefixed(
            "PreparedDocument.layout_tree",
            self.inputs.layout_tree.memory_usage_report(),
        );
        report.extend_prefixed(
            "PreparedDocument.inline_content",
            self.inputs.inline_content.memory_usage_report(),
        );
        report
    }

    #[cfg(test)]
    pub(crate) fn box_at(&self, box_idx: usize) -> Option<&crate::layout_model::LayoutBox> {
        self.inputs.layout_tree.box_at(box_idx)
    }

    pub fn get_tag(&self, box_idx: usize) -> &str {
        self.inputs
            .layout_tree
            .box_at(box_idx)
            .and_then(|layout_box| layout_box.get_element(self.document()))
            .map(|element| element.tag())
            .unwrap_or("")
    }

    pub fn get_id(&self, box_idx: usize) -> Option<&str> {
        self.inputs
            .layout_tree
            .box_at(box_idx)?
            .get_element(self.document())?
            .id_idx()
            .map(|id| self.document().string(id))
    }

    pub(crate) fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.inputs.layout_tree.box_at(box_idx)?.style()
    }

    pub fn box_text_color(&self, box_idx: usize) -> Option<u32> {
        let indices = self.box_style_indices(box_idx)?;
        Some(self.inputs.styles.text_style(indices)?.color)
    }

    pub fn shape(
        &self,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<ShapedDocument, crate::ShapeError> {
        glyph_shaper.begin_document_shaping();
        let result = self.shape_active_document(glyph_shaper);
        if result.is_ok() {
            glyph_shaper.commit_document_shaping();
        } else {
            glyph_shaper.rollback_document_shaping();
        }
        result
    }

    /// Shapes and lays out a document as one renderer-resource transaction.
    /// A shaping or exact-line-refinement failure leaves the previous renderer
    /// resources active.
    pub fn shape_and_layout_with_metrics_and_shaper(
        &self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<(ShapedDocument, LaidOutDocument), crate::ShapeError> {
        glyph_shaper.begin_document_shaping();
        let result = self.shape_active_document(glyph_shaper).and_then(|shaped| {
            let laid_out = shaped.clone().layout_with_metrics_and_shaper(
                constraints,
                image_metrics,
                glyph_shaper,
            )?;
            Ok((shaped, laid_out))
        });
        if result.is_ok() {
            glyph_shaper.commit_document_shaping();
        } else {
            glyph_shaper.rollback_document_shaping();
        }
        result
    }

    /// Shapes and lays out a subtree of the document whose renderer resources
    /// are already active, appending to them instead of opening a transaction.
    ///
    /// A document shaping transaction *replaces* the shaper's glyphs and runs
    /// on commit. That is right for a new document and wrong for a note, which
    /// belongs to the document it was taken from: committing the note's
    /// resources strands every glyph the page still refers to, and rolling
    /// them back discards the note's own. Appending leaves both usable.
    ///
    /// Glyph ids continue past `base`'s, because the shaper keeps appending to
    /// one store, so metrics start from `base`'s table -- indexed by id, a
    /// fresh table would put every id this produces out of range.
    ///
    /// There is no rollback. A failure can leave the note's glyphs behind,
    /// which is harmless: they are unreferenced and the page's are untouched.
    pub fn shape_and_layout_into_active_resources(
        &self,
        base: &ShapedDocument,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<(ShapedDocument, LaidOutDocument), crate::ShapeError> {
        let shaped =
            self.shape_active_document_with_metrics(base.glyph_metrics().clone(), glyph_shaper)?;
        let laid_out = shaped.clone().layout_with_metrics_and_shaper(
            constraints,
            image_metrics,
            glyph_shaper,
        )?;
        Ok((shaped, laid_out))
    }

    fn shape_active_document(
        &self,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<ShapedDocument, crate::ShapeError> {
        self.shape_active_document_with_metrics(GlyphMetrics::default(), glyph_shaper)
    }

    fn shape_active_document_with_metrics(
        &self,
        mut glyph_metrics: GlyphMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<ShapedDocument, crate::ShapeError> {
        let mut inline_content = self.inputs.inline_content.clone();
        let (ellipsis_glyphs, hyphen_glyphs, text_geometry, font_metrics) =
            crate::shaping::shape_document(
                &self.inputs.document,
                &self.inputs.styles,
                &self.inputs.layout_tree,
                &mut inline_content,
                glyph_shaper,
                &mut glyph_metrics,
            )?;
        let link_glyph_targets = collect_link_glyph_targets(
            &self.inputs.document,
            &self.inputs.layout_tree,
            &inline_content,
        )
        .into_iter()
        .collect();
        let anchor_glyphs = collect_anchor_glyphs(
            &self.inputs.document,
            &self.inputs.layout_tree,
            &inline_content,
        )
        .into_iter()
        .collect();
        Ok(ShapedDocument {
            inputs: self.inputs.clone(),
            shaped: std::sync::Arc::new(ShapedText {
                inline_content,
                glyph_metrics,
                font_metrics,
                text_geometry,
                ellipsis_glyphs,
                hyphen_glyphs,
                link_glyph_targets,
                anchor_glyphs,
                inline_plans: crate::layout::PreparedInlinePlans::new(self.inputs.layout_tree.box_count()),
            }),
        })
    }
}

/// A prepared document whose inline text has concrete glyph IDs and metrics.
/// Layout constraints can change without repeating parsing, styling, box-tree
/// construction, or shaping.
impl ShapedDocument {
    pub(crate) fn document(&self) -> &Document {
        self.inputs.document.as_ref()
    }

    pub fn glyphs(&self) -> &[GlyphId] {
        self.shaped.inline_content.glyphs()
    }

    pub(crate) fn glyph_metrics(&self) -> &GlyphMetrics {
        &self.shaped.glyph_metrics
    }

    /// Estimate bytes retained for the shaped stage cache.
    pub fn memory_usage_bytes(&self) -> usize {
        self.memory_usage_report().total_bytes()
    }

    /// Report bytes retained for the shaped stage cache.
    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.extend_prefixed(
            "ShapedDocument.document",
            self.document().memory_usage_report(),
        );
        report.extend_prefixed(
            "ShapedDocument.styles",
            self.inputs.styles.memory_usage_report(),
        );
        report.extend_prefixed(
            "ShapedDocument.layout_tree",
            self.inputs.layout_tree.memory_usage_report(),
        );
        report.extend_prefixed(
            "ShapedDocument.inline_content",
            self.shaped.inline_content.memory_usage_report(),
        );
        report.extend_prefixed(
            "ShapedDocument.glyph_metrics",
            self.shaped.glyph_metrics.memory_usage_report(),
        );
        report.add(
            "ShapedDocument.text_geometry",
            self.shaped.text_geometry.memory_usage_bytes(),
            self.shaped.inline_content.glyphs().len(),
        );
        report.add(
            "ShapedDocument.font_metrics",
            self.shaped.font_metrics.memory_usage_bytes(),
            self.inputs.layout_tree.box_count(),
        );
        report.add(
            "ShapedDocument.inline_plans",
            self.shaped.inline_plans.memory_usage_bytes(),
            self.shaped.inline_plans.len(),
        );
        report.add_slice_storage::<(u32, GlyphId)>(
            "ShapedDocument.ellipsis_glyphs.storage",
            self.shaped.ellipsis_glyphs.capacity(),
            self.shaped.ellipsis_glyphs.len(),
        );
        report.add_slice_storage::<(u32, GlyphId)>(
            "ShapedDocument.hyphen_glyphs.storage",
            self.shaped.hyphen_glyphs.capacity(),
            self.shaped.hyphen_glyphs.len(),
        );
        report.add_slice_storage::<(u32, LinkGlyphTarget)>(
            "ShapedDocument.link_glyph_targets.storage",
            self.shaped.link_glyph_targets.capacity(),
            self.shaped.link_glyph_targets.len(),
        );
        report.add_slice_storage::<(u16, u32)>(
            "ShapedDocument.anchor_glyphs.storage",
            self.shaped.anchor_glyphs.capacity(),
            self.shaped.anchor_glyphs.len(),
        );
        report
    }

    pub fn layout(self, constraints: LayoutConstraints) -> LaidOutDocument {
        let image_metrics = ImageMetrics::from_document(self.inputs.document.as_ref());
        self.layout_with_metrics(constraints, &image_metrics)
    }

    pub fn layout_with_timings(
        self,
        constraints: LayoutConstraints,
    ) -> (LaidOutDocument, crate::LayoutTimings) {
        let image_metrics = ImageMetrics::from_document(self.inputs.document.as_ref());
        self.layout_with_timings_and_metrics(constraints, &image_metrics)
    }

    pub fn layout_with_metrics(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
    ) -> LaidOutDocument {
        self.layout_impl(constraints, image_metrics, None)
    }

    /// Compatibility entry point. Document shaping is authoritative, so width
    /// changes only invoke the backend shaper for width-dependent pseudo text.
    pub fn layout_with_metrics_and_shaper(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<LaidOutDocument, crate::ShapeError> {
        let base_shaped = self.shaped.clone();
        let initial = self.layout_impl(constraints, image_metrics, None);
        refine_first_lines(
            initial,
            base_shaped.clone(),
            base_shaped,
            constraints,
            image_metrics,
            glyph_shaper,
        )
    }

    pub fn layout_with_timings_and_metrics(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
    ) -> (LaidOutDocument, crate::LayoutTimings) {
        let mut timings = crate::LayoutTimings::default();
        let document = self.layout_impl(constraints, image_metrics, Some(&mut timings));
        (document, timings)
    }

    pub(super) fn layout_impl(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>,
    ) -> LaidOutDocument {
        let mut layout_state = LayoutState::default();
        let mut geometry = BoxGeometry::default();
        let mut layout_scratch = crate::layout::LayoutScratch::default();
        crate::layout::layout_with_timings(
            crate::layout::LayoutInputs {
                document: &self.inputs.document,
                styles: &self.inputs.styles,
                topology: &self.inputs.layout_tree,
                inline_content: &self.shaped.inline_content,
                glyph_metrics: &self.shaped.glyph_metrics,
                font_metrics: &self.shaped.font_metrics,
                text_geometry: Some(&self.shaped.text_geometry),
                ellipsis_glyphs: &self.shaped.ellipsis_glyphs,
                hyphen_glyphs: &self.shaped.hyphen_glyphs,
                inline_plans: &self.shaped.inline_plans,
                image_metrics,
            },
            crate::layout::LayoutOutputs {
                geometry: &mut geometry,
                state: &mut layout_state,
                scratch: &mut layout_scratch,
            },
            constraints,
            timings,
        );
        let mut laid_out = LaidOutDocument {
            inputs: self.inputs,
            geometry,
            base_shaped: self.shaped.clone(),
            shaped: self.shaped,
            layout_state,
            layout_scratch,
            last_constraints: constraints,
            last_image_metrics: image_metrics.clone(),
        };
        laid_out.rebuild_anchor_positions();
        laid_out
    }
}

/// A fully shaped and laid-out document.
///
/// No public constructor exists: values can only be produced from a
/// [`ShapedDocument`]. Relayout mutates only layout-owned data, retaining the
/// immutable shaping inputs and reusing top-level layout buffers.
#[derive(Clone)]
pub struct LaidOutDocument {
    pub(super) inputs: std::sync::Arc<PreparedInputs>,
    pub(super) geometry: BoxGeometry,
    /// Viewport-independent shaping retained so a width change can recompute
    /// the dynamic extent of `::first-line` without accumulating old styles.
    pub(super) base_shaped: std::sync::Arc<ShapedText>,
    pub(super) shaped: std::sync::Arc<ShapedText>,
    pub(super) layout_state: LayoutState,
    pub(super) layout_scratch: crate::layout::LayoutScratch,
    pub(super) last_constraints: LayoutConstraints,
    pub(super) last_image_metrics: ImageMetrics,
}

impl LaidOutDocument {
    /// Whether changing only the viewport height can alter document geometry.
    /// Viewport-relative style values are tracked by the style pipeline; this
    /// covers layout-time dependencies that remain percentage based.
    pub fn layout_depends_on_viewport_height(&self) -> bool {
        (0..self.box_count()).any(|box_idx| {
            let Some(style) = self.box_used_style(box_idx) else {
                return false;
            };
            style.height().percentage_dependent()
                || style.min_height().percentage_dependent()
                || style.max_height().percentage_dependent()
                || (style.position() == html_style_model::PositionMode::Absolute
                    && style.inset_bottom().is_some())
        })
    }

    pub fn render_view(&self) -> RenderView<'_> {
        RenderView { doc: self }
    }

    pub(super) fn text_geometry(&self) -> &crate::shaping::ShapedTextGeometry {
        &self.shaped.text_geometry
    }

    pub fn relayout(&mut self, constraints: LayoutConstraints) {
        let image_metrics = ImageMetrics::from_document(self.document());
        self.relayout_impl(constraints, &image_metrics, None);
        self.last_constraints = constraints;
        self.last_image_metrics = image_metrics;
    }

    pub fn relayout_with_timings(
        &mut self,
        constraints: LayoutConstraints,
    ) -> crate::LayoutTimings {
        let mut timings = crate::LayoutTimings::default();
        let image_metrics = ImageMetrics::from_document(self.document());
        self.relayout_impl(constraints, &image_metrics, Some(&mut timings));
        self.last_constraints = constraints;
        self.last_image_metrics = image_metrics;
        timings
    }

    pub fn relayout_with_metrics(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
    ) {
        self.relayout_impl(constraints, image_metrics, None);
        self.last_constraints = constraints;
        self.last_image_metrics = image_metrics.clone();
    }

    pub fn relayout_with_metrics_and_shaper(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<(), crate::ShapeError> {
        let shaping_seed = self.shaped.clone();
        self.shaped = self.base_shaped.clone();
        self.relayout_impl(constraints, image_metrics, None);
        let initial = self.clone();
        *self = refine_first_lines(
            initial,
            self.base_shaped.clone(),
            shaping_seed,
            constraints,
            image_metrics,
            glyph_shaper,
        )?;
        self.last_constraints = constraints;
        self.last_image_metrics = image_metrics.clone();
        Ok(())
    }

    pub fn relayout_with_metrics_and_timings(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
    ) -> crate::LayoutTimings {
        let mut timings = crate::LayoutTimings::default();
        self.relayout_impl(constraints, image_metrics, Some(&mut timings));
        self.last_constraints = constraints;
        self.last_image_metrics = image_metrics.clone();
        timings
    }

    fn relayout_impl(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>,
    ) {
        let text_geometry = &self.shaped.text_geometry;
        crate::layout::layout_with_timings(
            crate::layout::LayoutInputs {
                document: &self.inputs.document,
                styles: &self.inputs.styles,
                topology: &self.inputs.layout_tree,
                inline_content: &self.shaped.inline_content,
                glyph_metrics: &self.shaped.glyph_metrics,
                font_metrics: &self.shaped.font_metrics,
                text_geometry: Some(text_geometry),
                ellipsis_glyphs: &self.shaped.ellipsis_glyphs,
                hyphen_glyphs: &self.shaped.hyphen_glyphs,
                inline_plans: &self.shaped.inline_plans,
                image_metrics,
            },
            crate::layout::LayoutOutputs {
                geometry: &mut self.geometry,
                state: &mut self.layout_state,
                scratch: &mut self.layout_scratch,
            },
            constraints,
            timings,
        );
        self.rebuild_anchor_positions();
    }

    fn rebuild_anchor_positions(&mut self) {
        self.layout_state.semantic_indexes.anchor_positions.clear();
        for box_idx in 0..self.inputs.layout_tree.box_count() {
            let Some(id_idx) = box_id(self.document(), &self.inputs.layout_tree, box_idx) else {
                continue;
            };
            let mut y = self.anchor_glyph(id_idx).and_then(|glyph| {
                let line_idx = *self
                    .layout_state
                    .line_output
                    .glyph_line_indices
                    .get(glyph as usize)?;
                if line_idx == u32::MAX {
                    return None;
                }
                self.layout_state
                    .line_output
                    .lines
                    .get(line_idx as usize)
                    .map(|line| line.point.y)
            });
            if y.is_none() {
                let mut current = Some(box_idx);
                while let Some(idx) = current {
                    let layout_box = self.inputs.layout_tree.box_at(idx);
                    if let Some(layout_box) = layout_box {
                        let size = self.geometry.size(idx);
                        let point = self.geometry.point(idx);
                        if size.height > 0.0 || point.y != 0.0 {
                            y = Some(point.y);
                            break;
                        }
                        current = layout_box.parent().map(|parent| parent as usize);
                    } else {
                        break;
                    }
                }
            }
            let Some(y) = y else { continue };
            let order = self.anchor_glyph(id_idx).unwrap_or(box_idx as u32);
            self.layout_state
                .semantic_indexes
                .anchor_positions
                .entry(id_idx)
                .and_modify(|existing| {
                    if y < existing.y || (y == existing.y && order < existing.order) {
                        *existing = AnchorPosition { y, order };
                    }
                })
                .or_insert(AnchorPosition { y, order });
        }
    }

    pub(crate) fn document(&self) -> &Document {
        &self.inputs.document
    }

    pub(crate) fn glyph_metrics(&self) -> &GlyphMetrics {
        &self.shaped.glyph_metrics
    }

    pub(crate) fn link_href_for_glyph(&self, glyph_idx: u32) -> Option<u16> {
        self.shaped
            .link_glyph_targets
            .get(&glyph_idx)
            .map(|target| target.href)
    }

    pub(crate) fn glyph_is_note_reference(&self, glyph_idx: u32) -> bool {
        self.shaped
            .link_glyph_targets
            .get(&glyph_idx)
            .is_some_and(|target| target.note_reference)
    }

    pub(crate) fn target_is_note(&self, id: &str) -> bool {
        self.document()
            .node_ids()
            .find(|node| self.document().get_dom_id(*node) == Some(id))
            .and_then(|node| self.document().element_ref(node))
            .is_some_and(element_is_note_target)
    }

    pub(crate) fn anchor_glyph(&self, id_idx: u16) -> Option<u32> {
        self.shaped.anchor_glyphs.get(&id_idx).copied()
    }

    pub(crate) fn anchor_glyphs(&self) -> &FxHashMap<u16, u32> {
        &self.shaped.anchor_glyphs
    }

    /// Creates a scoped, append-only registry for renderer-owned auxiliary glyphs.
    /// Existing glyph IDs and document layout remain unchanged.
    pub fn auxiliary_glyph_registry(&mut self) -> crate::GlyphRegistry<'_> {
        crate::GlyphRegistry::new(&mut std::sync::Arc::make_mut(&mut self.shaped).glyph_metrics)
    }

    #[cfg(test)]
    pub(crate) fn glyphs(&self) -> &[GlyphId] {
        self.shaped.inline_content.glyphs()
    }

    /// Returns the DOM text node and UTF-16 source offset for the glyph.
    pub(crate) fn get_dom_node_for_glyph(&self, glyph_idx: u32) -> Option<(u32, usize)> {
        for run in self.shaped.inline_content.inline_items() {
            if let InlineItemKind::Text { glyphs } = &run.kind
                && glyphs.contains(&glyph_idx)
            {
                let node_idx = run.dom_text_node?;
                let source_offset =
                    self.shaped
                        .inline_content
                        .glyph_source_offset(glyph_idx as usize)? as usize;
                return Some((node_idx, source_offset));
            }
        }
        None
    }

    pub(crate) fn glyph_source_offset(&self, glyph_idx: u32) -> Option<u32> {
        self.shaped
            .inline_content
            .glyph_source_offset(glyph_idx as usize)
    }

    pub(crate) fn box_dom_element_idx(&self, box_idx: usize) -> Option<u32> {
        self.inputs.layout_tree.box_at(box_idx)?.dom_element()
    }

    pub(crate) fn box_layout_mode(&self, box_idx: usize) -> Option<&LayoutMode> {
        self.inputs
            .layout_tree
            .box_at(box_idx)
            .map(|layout_box| layout_box.layout_mode())
    }

    pub(crate) fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.inputs.layout_tree.box_at(box_idx)?.style()
    }

    pub(crate) fn box_used_style(
        &self,
        box_idx: usize,
    ) -> Option<html_style_model::UsedStyleView<'_>> {
        let indices = self
            .box_style_indices(box_idx)
            .unwrap_or_else(|| self.inputs.styles.default_indices());
        self.shaped
            .font_metrics
            .used_style(&self.inputs.styles, indices, box_idx)
    }

    pub(crate) fn box_text_format(&self, box_idx: usize) -> BoxTextFormat {
        let style = self
            .box_used_style(box_idx)
            .expect("shaping stores valid font metrics for every layout box");
        BoxTextFormat {
            font_size: style.font_size(),
            font_weight: style.font_weight(),
            font_style: style.font_style(),
            color: style.color(),
            font_family: style.font_family(),
            letter_spacing: style.letter_spacing(),
            word_spacing: style.word_spacing(),
            text_decoration: style.text_decoration().lines,
        }
    }

    pub(crate) fn list_marker(&self, box_idx: usize) -> Option<RenderListItemMarker> {
        self.inputs
            .layout_tree
            .list_marker(box_idx)
            .map(RenderListItemMarker::from_model)
    }

    pub(crate) fn box_count(&self) -> usize {
        self.inputs.layout_tree.box_count()
    }

    pub(crate) fn is_block_container_box(&self, box_idx: usize) -> bool {
        matches!(
            self.box_layout_mode(box_idx),
            Some(LayoutMode::Block(_) | LayoutMode::Table(_) | LayoutMode::TableCell(_))
        )
    }

    #[cfg(test)]
    pub(crate) fn line_count(&self) -> usize {
        self.layout_state.line_output.lines.len()
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.extend_prefixed(
            "LaidOutDocument.document",
            self.document().memory_usage_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument.styles",
            self.inputs.styles.memory_usage_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument.layout_tree",
            self.inputs.layout_tree.memory_usage_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument.geometry",
            self.geometry.memory_usage_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument.inline_content",
            self.shaped.inline_content.memory_usage_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument.layout_state",
            self.layout_state.memory_usage_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument.glyph_metrics",
            self.glyph_metrics().memory_usage_report(),
        );
        report.add(
            "LaidOutDocument.inline_plans",
            self.shaped.inline_plans.memory_usage_bytes(),
            self.shaped.inline_plans.len(),
        );
        report.add(
            "LaidOutDocument.rollback_image_metrics",
            self.last_image_metrics.memory_usage_bytes(),
            self.last_image_metrics.len(),
        );
        report.add_slice_storage::<(u32, LinkGlyphTarget)>(
            "LaidOutDocument.link_glyph_targets.storage",
            self.shaped.link_glyph_targets.capacity(),
            self.shaped.link_glyph_targets.len(),
        );
        report.add_slice_storage::<(u16, u32)>(
            "LaidOutDocument.anchor_glyphs.storage",
            self.shaped.anchor_glyphs.capacity(),
            self.shaped.anchor_glyphs.len(),
        );
        report
    }

    pub fn memory_usage_bytes(&self) -> usize {
        self.memory_usage_report().total_bytes()
    }
}
