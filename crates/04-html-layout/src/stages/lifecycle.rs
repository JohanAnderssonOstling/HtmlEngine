//! Prepared, shaped, and laid-out document lifecycle types.

use super::*;
use crate::layout::{InlinePlanGeneration, LayoutScratch, prepare_text_paint};
use crate::ReaderStyleOverrides;
use crate::shaping::PreparedText;
use html_dom::{NodeRef, RootFontSize};
use html_style_model::{ContentLanguage, Display, Visibility};
use std::sync::Arc;
use super::paint::PaintStyles;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutConstraints {
    viewport_width: f64,
    viewport_height: Option<f64>,
    line_height: f64,
    image_sizing_policy: ImageSizingPolicy,
    text_composition_policy: TextCompositionPolicy,
    parallel_workers: usize,
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
}

impl TextCompositionPolicy {
    pub fn is_book_optimized(self) -> bool {
        matches!(
            self,
            Self::BookOptimized | Self::BookOptimizedUnrestrictedHyphenation
        )
    }

    pub fn uses_hyphenation_quality(self) -> bool {
        self == Self::BookOptimized
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
            parallel_workers: 1,
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

    /// Enables worker-owned parallel subtree measurement. A value of one
    /// preserves deterministic sequential execution; larger values cap the
    /// dedicated layout pool used by dependency-independent work.
    pub fn with_parallel_workers(mut self, workers: usize) -> Self {
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        self.parallel_workers = workers.clamp(1, available);
        self
    }

    pub fn parallel_workers(self) -> usize {
        self.parallel_workers
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
    pub(super) document: Arc<Document>,
    pub(super) styles: Arc<ComputedStyles>,
    pub(super) layout_tree: Arc<LayoutTree>,
    pub(super) text: Arc<PreparedText>,
    pub(super) note_flow: NoteFlow,
    pub(crate) paint: Arc<PaintStyles>,
}

#[derive(Clone)]
pub struct PreparedDocument {
    pub(super) inputs: Arc<PreparedInputs>,
}

pub(crate) struct ShapedText {
    pub(super) resource_generation: crate::GlyphResourceGeneration,
    pub(super) geometry_tree: Arc<LayoutTree>,
    pub(super) glyphs: Vec<GlyphId>,
    pub(super) glyph_metrics: GlyphMetrics,
    pub(super) text_geometry: crate::shaping::ShapedTextGeometry,
    pub(super) shared: SharedShapedData,
    pub(super) inline_plan_generation: InlinePlanGeneration,
}

/// Immutable shaping tables shared by the base shape and first-line variants.
pub(super) struct SharedShapedData {
    pub(super) font_metrics: crate::shaping::ShapedFontMetrics,
    pub(super) ellipsis_glyphs: FxHashMap<u32, GlyphId>,
    pub(super) hyphen_glyphs: FxHashMap<u32, GlyphId>,
    pub(super) link_glyph_targets: FxHashMap<u32, LinkGlyphTarget>,
    pub(super) anchor_glyphs: FxHashMap<u16, u32>,
}

pub(crate) struct FirstLineRefinement {
    pub(super) glyph_patches: Vec<GlyphPatch>,
    pub(super) paint_ranges: Vec<TextPaintRange>,
    pub(super) glyph_metrics: GlyphMetrics,
    pub(super) text_geometry: ShapedTextGeometry,
    pub(super) inline_plan_generation: InlinePlanGeneration,
}

impl FirstLineRefinement {
    fn memory_usage_report(&self, seen: &mut FxHashSet<*const ()>) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<GlyphPatch>(
            "glyph_patches",
            self.glyph_patches.capacity(),
            self.glyph_patches.len(),
        );
        for patch in &self.glyph_patches {
            report.add_slice_storage::<GlyphId>(
                "glyphs.storage",
                patch.glyphs.capacity(),
                patch.glyphs.len(),
            );
        }
        report.add_slice_storage::<TextPaintRange>(
            "text_paint_ranges",
            self.paint_ranges.capacity(),
            self.paint_ranges.len(),
        );
        report.extend_prefixed(
            "glyph_metrics",
            self.glyph_metrics.memory_usage_report_with_shared(seen),
        );
        report.add(
            "text_geometry",
            self.text_geometry.memory_usage_bytes_with_shared(seen),
            self.paint_ranges.len(),
        );
        report
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ShapedTextView<'a> {
    pub(super) resource_generation: crate::GlyphResourceGeneration,
    pub(super) glyphs: GlyphSlice<'a>,
    pub(super) glyph_metrics: &'a GlyphMetrics,
    pub(super) text_geometry: TextGeometryView<'a>,
    pub(super) paint_ranges: &'a [TextPaintRange],
    pub(super) shared: &'a SharedShapedData,
    pub(super) inline_plan_generation: InlinePlanGeneration,
}

impl<'a> ShapedTextView<'a> {
    fn new(
        base: &'a ShapedText,
        refinement: Option<&'a FirstLineRefinement>,
        metrics: Option<&'a GlyphMetrics>,
    ) -> Self {
        Self {
            resource_generation: base.resource_generation,
            glyphs: GlyphSlice::new(
                &base.glyphs,
                refinement.map_or(&[], |refined| &refined.glyph_patches),
            ),
            glyph_metrics: metrics.unwrap_or_else(|| {
                refinement.map_or(&base.glyph_metrics, |refined| &refined.glyph_metrics)
            }),
            text_geometry: TextGeometryView::new(
                &base.text_geometry,
                refinement.map(|refined| &refined.text_geometry),
            ),
            paint_ranges: refinement.map_or(&[][..], |refined| &refined.paint_ranges),
            shared: &base.shared,
            inline_plan_generation: refinement.map_or(base.inline_plan_generation, |refined| {
                refined.inline_plan_generation
            }),
        }
    }
}

/// Sole owner of viewport-independent shaping. Layout consumes this stage
/// and moves its glyph storage into the published document.
///
/// ```compile_fail
/// fn duplicate(shape: html_layout::ShapedDocument) {
///     let _ = shape.clone();
/// }
/// ```
/// Owns the shaped text until it is moved into a layout document.
/// Independent layouts execute shaping separately; readers borrow publication.
///
/// ```compile_fail
/// fn copy_shape(document: &html_layout::ShapedDocument) {
///     let _: html_layout::ShapedDocument = document.clone();
/// }
/// ```
pub struct ShapedDocument {
    pub(super) inputs: Arc<PreparedInputs>,
    pub(super) shaped: Box<ShapedText>,
}

impl ShapedText {
    fn memory_usage_report(&self, seen: &mut FxHashSet<*const ()>) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<GlyphId>(
            "glyphs.storage",
            self.glyphs.capacity(),
            self.glyphs.len(),
        );
        report.extend_prefixed(
            "glyph_metrics",
            self.glyph_metrics.memory_usage_report_with_shared(seen),
        );
        report.add(
            "text_geometry",
            self.text_geometry.memory_usage_bytes_with_shared(seen),
            self.glyphs.len(),
        );
        if seen.insert((&self.shared as *const SharedShapedData).cast()) {
            report.add("shared_metadata", std::mem::size_of::<SharedShapedData>(), 1);
            report.add(
                "font_metrics",
                self.shared.font_metrics.memory_usage_bytes(),
                self.geometry_tree.box_count(),
            );
            report.add_slice_storage::<(u32, GlyphId)>(
                "ellipsis_glyphs.storage",
                self.shared.ellipsis_glyphs.capacity(),
                self.shared.ellipsis_glyphs.len(),
            );
            report.add_slice_storage::<(u32, GlyphId)>(
                "hyphen_glyphs.storage",
                self.shared.hyphen_glyphs.capacity(),
                self.shared.hyphen_glyphs.len(),
            );
            report.add_slice_storage::<(u32, LinkGlyphTarget)>(
                "link_glyph_targets.storage",
                self.shared.link_glyph_targets.capacity(),
                self.shared.link_glyph_targets.len(),
            );
            report.add_slice_storage::<(u16, u32)>(
                "anchor_glyphs.storage",
                self.shared.anchor_glyphs.capacity(),
                self.shared.anchor_glyphs.len(),
            );
        }
        report
    }
}

fn with_append_shaping<S: crate::GlyphShaper, T>(
    glyph_shaper: &mut S,
    work: impl FnOnce(&mut S) -> Result<T, crate::ShapeError>,
) -> Result<T, crate::ShapeError> {
    glyph_shaper.begin_append_shaping()?;
    let result = work(glyph_shaper);
    if result.is_ok() {
        glyph_shaper.commit_append_shaping();
    } else {
        glyph_shaper.rollback_append_shaping();
    }
    result
}

/// Wall time spent in the two renderer-resource phases of an explicitly timed
/// shape-and-layout operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShapeLayoutTimings {
    pub shape: Duration,
    pub layout: Duration,
}

/// Rebuild derived preparation while the session exclusively owns its DOM.
/// The resolver borrows the canonical tree. A journal moves old derived state
/// aside until the complete shape/layout transaction commits.
fn with_reprepared<T>(
    owner: &mut Arc<PreparedInputs>,
    root_font_size: RootFontSize,
    note_flow: NoteFlow,
    overrides: &ReaderStyleOverrides,
    resolve: impl FnOnce(&Document) -> ComputedStyles,
    apply: impl FnOnce(&PreparedInputs) -> Result<T, crate::ShapeError>,
) -> Result<T, crate::ShapeError> {
    let inputs = Arc::get_mut(owner).ok_or_else(crate::ShapeError::document_is_shared)?;
    let document =
        Arc::get_mut(&mut inputs.document).ok_or_else(crate::ShapeError::document_is_shared)?;
    let old_font = RootFontSize::new(document.root_font_size()).expect("validated root font size");
    document.set_root_font_size(root_font_size);
    let styles = resolve(document);
    styles
        .validate_for(document)
        .expect("resolver styles must belong to the canonical DOM");
    let math_undo = super::mathml::prepare_with_undo(document, &styles);
    let mut tree = LayoutTree::default();
    let mut content = InlineContent::default();
    crate::layout::build_layout_inputs(document, &styles, &mut tree, &mut content, note_flow);
    let text = PreparedText::new(document, &styles, &tree, content);
    let paint =
        PaintStyles::new(document, &styles, &tree).with_overrides(&styles, &tree, overrides);
    let old_styles = std::mem::replace(&mut inputs.styles, Arc::new(styles));
    let old_tree = std::mem::replace(&mut inputs.layout_tree, Arc::new(tree));
    let old_text = std::mem::replace(&mut inputs.text, Arc::new(text));
    let old_paint = std::mem::replace(&mut inputs.paint, Arc::new(paint));
    let old_flow = std::mem::replace(&mut inputs.note_flow, note_flow);
    let result = apply(owner.as_ref());
    if result.is_err() {
        let inputs =
            Arc::get_mut(owner).expect("failed candidates release their borrowed preparation");
        inputs.styles = old_styles;
        inputs.layout_tree = old_tree;
        inputs.text = old_text;
        inputs.paint = old_paint;
        inputs.note_flow = old_flow;
        let document = Arc::get_mut(&mut inputs.document).expect("canonical DOM owner is retained");
        document.set_root_font_size(old_font);
        super::mathml::restore(document, math_undo);
    }
    result
}

impl PreparedDocument {
    /// Source text participating in visible content, with inherited language,
    /// before any platform shaping. Display suppression applies to subtrees;
    /// visibility can be restored by a descendant's own computed style.
    pub fn visible_text_runs(&self) -> impl Iterator<Item = (&str, Option<&ContentLanguage>)> { visible_text_runs(&self.inputs) }

    pub fn reprepare_and_layout(
        &mut self, root_font_size: RootFontSize, note_flow: NoteFlow, overrides: &ReaderStyleOverrides,
        resolve: impl FnOnce(&Document) -> ComputedStyles, constraints: LayoutConstraints,
        image_metrics: &ImageMetrics, glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<LaidOutDocument, crate::ShapeError> {
        let (shaped, candidate) = with_reprepared(&mut self.inputs, root_font_size, note_flow, overrides, resolve, |inputs| {
            shape_and_layout_candidate(inputs, constraints, image_metrics, glyph_shaper)
        })?;
        Ok(candidate.into_document(self.inputs.clone(), shaped, constraints, image_metrics))
    }

    pub fn try_new(document: Document, styles: ComputedStyles) -> Result<Self, PrepareError> {
        Self::try_new_with_note_flow(document, styles, NoteFlow::default())
    }

    pub fn try_new_with_note_flow(
        document: Document,
        styles: ComputedStyles,
        note_flow: NoteFlow,
    ) -> Result<Self, PrepareError> {
        profile_scope!("prepare.total");
        profile!("prepare.validate_styles", styles.validate_for(&document)).map_err(PrepareError)?;
        let mut document = document;
        profile!("prepare.mathml", super::mathml::prepare(&mut document, &styles));
        let mut layout_tree = LayoutTree::default();
        let mut inline_content = InlineContent::default();
        profile!("prepare.build_box_tree", crate::layout::build_layout_inputs(&document, &styles, &mut layout_tree, &mut inline_content, note_flow));
        let text = Arc::new(PreparedText::new(&document, &styles, &layout_tree, inline_content));
        let paint = Arc::new(profile!("prepare.paint_bindings", PaintStyles::new(&document, &styles, &layout_tree)));
        Ok(Self { inputs: Arc::new(PreparedInputs { document: Arc::new(document), styles: Arc::new(styles), layout_tree: Arc::new(layout_tree), text, note_flow, paint }) })
    }

    /// Apply color preferences to an unpainted prepared document. The cascade
    /// and typography are retained, including their publication colors.
    pub fn with_reader_paint_overrides(&self, overrides: &ReaderStyleOverrides) -> Result<Self, PrepareError> {
        let paint = self.inputs.paint.with_overrides(&self.inputs.styles, &self.inputs.layout_tree, overrides);
        Ok(Self { inputs: Arc::new(PreparedInputs {
            document: self.inputs.document.clone(), styles: self.inputs.styles.clone(),
            layout_tree: self.inputs.layout_tree.clone(), text: self.inputs.text.clone(),
            note_flow: self.inputs.note_flow, paint: Arc::new(paint),
        }) })
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
    pub fn scoped_to_element_id(&self, id: &str) -> Option<Self> { scoped_to_element_id(&self.inputs, id) }

    /// Ids of this document's note bodies, in document order. A note without an
    /// id cannot be referenced, so it is not listed.
    pub fn note_ids(&self) -> Vec<&str> { note_ids(&self.inputs) }

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
        self.inputs.text.content()
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
        report.extend_prefixed("PreparedDocument.paint", self.inputs.paint.memory_usage_report());
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
            self.inputs.text.memory_usage_report(),
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
        Some(self.inputs.paint.resolve(&self.inputs.styles, indices, box_idx).foreground)
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
    ) -> Result<LaidOutDocument, crate::ShapeError> {
        glyph_shaper.begin_document_shaping();
        let result = self.shape_active_document(glyph_shaper).and_then(|shaped| {
            let laid_out = shaped.layout_with_metrics_and_shaper_inner(
                constraints,
                image_metrics,
                glyph_shaper,
            )?;
            Ok(laid_out)
        });
        if result.is_ok() {
            glyph_shaper.commit_document_shaping();
        } else {
            glyph_shaper.rollback_document_shaping();
        }
        result
    }

    /// Timed form of [`Self::shape_and_layout_with_metrics_and_shaper`]. Clock
    /// reads are kept out of the ordinary layout API.
    pub fn shape_and_layout_with_metrics_and_shaper_timed(
        &self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<(LaidOutDocument, ShapeLayoutTimings), crate::ShapeError> {
        glyph_shaper.begin_document_shaping();
        let mut timings = ShapeLayoutTimings::default();
        let shape_started = Instant::now();
        let result = self.shape_active_document(glyph_shaper).and_then(|shaped| {
            timings.shape = shape_started.elapsed();
            let layout_started = Instant::now();
            let laid_out = shaped.layout_with_metrics_and_shaper_inner(
                constraints,
                image_metrics,
                glyph_shaper,
            )?;
            timings.layout = layout_started.elapsed();
            Ok((laid_out, timings))
        });
        if result.is_ok() {
            glyph_shaper.commit_document_shaping();
        } else {
            glyph_shaper.rollback_document_shaping();
        }
        result
    }

    /// Shapes and lays out a subtree of the document whose renderer resources
    /// are already active, using a rollback-capable append transaction.
    ///
    /// A document shaping transaction *replaces* the shaper's glyphs and runs
    /// on commit. That is right for a new document and wrong for a note, which
    /// belongs to the document it was taken from: committing the note's
    /// resources strands every glyph the page still refers to, and rolling
    /// them back discards the note's own. An append transaction retains both
    /// on success and restores the page's resources on failure.
    ///
    /// Glyph ids and metrics both append to the shaper's active resource
    /// store. A failed append restores that store to the previous dense ID.
    pub fn shape_and_layout_into_active_resources(
        &self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<LaidOutDocument, crate::ShapeError> {
        with_append_shaping(glyph_shaper, |glyph_shaper| {
            let shaped = self.shape_active_document(glyph_shaper)?;
            let laid_out = shaped.layout_with_metrics_and_shaper_inner(
                constraints,
                image_metrics,
                glyph_shaper,
            )?;
            Ok(laid_out)
        })
    }

    fn shape_active_document(
        &self, glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<ShapedDocument, crate::ShapeError> {
        Ok(ShapedDocument { inputs: self.inputs.clone(), shaped: shape_text(&self.inputs, glyph_shaper)? })
    }
}

fn shape_text(
    inputs: &PreparedInputs,
    glyph_shaper: &mut impl crate::GlyphShaper,
) -> Result<Box<ShapedText>, crate::ShapeError> {
    profile_scope!("shape.total");
    let mut glyph_metrics = profile!(
        "shape.snapshot_metrics",
        glyph_shaper.glyph_resources().snapshot()
    );
    let inline_content = inputs.text.content();
    let mut glyphs = profile!(
        "shape.allocate_glyphs",
        inline_content.characters().to_vec()
    );
    let (ellipsis_glyphs, hyphen_glyphs, text_geometry, font_metrics) =
        crate::shaping::shape_document(
            &inputs.styles,
            &inputs.layout_tree,
            inline_content,
            &mut glyphs,
            inputs.text.first_letter_styles(),
            glyph_shaper,
            &mut glyph_metrics,
        )?;
    let link_glyph_targets = profile!(
        "shape.link_targets",
        collect_link_glyph_targets(&inputs.document, &inputs.layout_tree, inline_content,)
            .into_iter()
            .collect()
    );
    let anchor_glyphs = profile!(
        "shape.anchor_targets",
        collect_anchor_glyphs(&inputs.document, &inputs.layout_tree, inline_content,)
            .into_iter()
            .collect()
    );
    let inline_plan_generation = InlinePlanGeneration::new();
    Ok(Box::new(ShapedText {
        resource_generation: glyph_shaper.glyph_resources().generation(),
        geometry_tree: inputs.layout_tree.clone(),
        glyphs,
        glyph_metrics,
        text_geometry,
        shared: SharedShapedData {
            font_metrics,
            ellipsis_glyphs,
            hyphen_glyphs,
            link_glyph_targets,
            anchor_glyphs,
        },
        inline_plan_generation,
    }))
}

fn shape_and_layout_candidate(
    inputs: &PreparedInputs,
    constraints: LayoutConstraints,
    image_metrics: &ImageMetrics,
    glyph_shaper: &mut impl crate::GlyphShaper,
) -> Result<(Box<ShapedText>, LayoutCandidate), crate::ShapeError> {
    glyph_shaper.begin_document_shaping();
    let result = (|| {
        let shaped = shape_text(inputs, glyph_shaper)?;
        let candidate = LayoutCandidate::new(
            inputs,
            &shaped,
            constraints,
            image_metrics,
            None,
            LayoutScratch::default(),
        );
        let seed = glyph_shaper.glyph_resources().snapshot();
        let candidate = refine_first_lines(
            candidate,
            inputs,
            &shaped,
            seed,
            constraints,
            image_metrics,
            glyph_shaper,
        )?;
        Ok((shaped, candidate))
    })();
    if result.is_ok() {
        glyph_shaper.commit_document_shaping();
    } else {
        glyph_shaper.rollback_document_shaping();
    }
    result
}

/// A prepared document whose inline text has concrete glyph IDs and metrics.
/// Layout constraints can change without repeating parsing, styling, box-tree
/// construction, or shaping.
impl ShapedDocument {
    pub(crate) fn document(&self) -> &Document {
        self.inputs.document.as_ref()
    }

    pub fn glyphs(&self) -> &[GlyphId] {
        &self.shaped.glyphs
    }

    #[cfg(test)]
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
            self.inputs.text.memory_usage_report(),
        );
        report.extend_prefixed("ShapedDocument", self.shaped.memory_usage_report(&mut FxHashSet::default()));
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
        if self.shaped.resource_generation != glyph_shaper.glyph_resources().generation() {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        let candidate = LayoutCandidate::new(&self.inputs, &self.shaped, constraints, image_metrics, None, LayoutScratch::default());
        let shaping_seed = glyph_shaper.glyph_resources().snapshot();
        let needs_append = !first_line_style_ranges(&self.inputs, &candidate.layout_state).is_empty();
        let refine = |glyph_shaper: &mut _| {
            refine_first_lines(candidate, &self.inputs, &self.shaped, shaping_seed, constraints, image_metrics, glyph_shaper)
        };
        let candidate = if needs_append { with_append_shaping(glyph_shaper, refine)? } else { refine(glyph_shaper)? };
        Ok(candidate.into_document(self.inputs, self.shaped, constraints, image_metrics))
    }

    fn layout_with_metrics_and_shaper_inner(
        self, constraints: LayoutConstraints, image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<LaidOutDocument, crate::ShapeError> {
        debug_assert_eq!(self.shaped.resource_generation, glyph_shaper.glyph_resources().generation());
        let candidate = LayoutCandidate::new(&self.inputs, &self.shaped, constraints, image_metrics, None, LayoutScratch::default());
        let shaping_seed = glyph_shaper.glyph_resources().snapshot();
        let candidate = refine_first_lines(candidate, &self.inputs, &self.shaped, shaping_seed, constraints, image_metrics, glyph_shaper)?;
        Ok(candidate.into_document(self.inputs, self.shaped, constraints, image_metrics))
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
        self.layout_impl_with_scratch(constraints, image_metrics, timings, LayoutScratch::default())
    }

    pub(super) fn layout_impl_with_scratch(
        self, constraints: LayoutConstraints, image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>, layout_scratch: LayoutScratch,
    ) -> LaidOutDocument {
        LayoutCandidate::new(&self.inputs, &self.shaped, constraints, image_metrics, timings, layout_scratch)
            .into_document(self.inputs, self.shaped, constraints, image_metrics)
    }
}

/// Candidate output owns only mutable layout state. Preparation and shaping
/// are borrowed for the operation, including each first-line refinement pass.
pub(crate) struct LayoutCandidate {
    geometry: BoxGeometry,
    placement: crate::layout::PlacementState,
    pub(super) refinement: Option<FirstLineRefinement>,
    pub(super) glyph_metrics_override: Option<GlyphMetrics>,
    pub(super) layout_state: LayoutState,
    layout_scratch: LayoutScratch,
}

impl LayoutCandidate {
    fn new(
        inputs: &PreparedInputs,
        base: &ShapedText,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>,
        layout_scratch: LayoutScratch,
    ) -> Self {
        let mut candidate = Self {
            geometry: BoxGeometry::default(),
            placement: crate::layout::PlacementState::default(),
            refinement: None,
            glyph_metrics_override: None,
            layout_state: LayoutState::default(),
            layout_scratch,
        };
        candidate.relayout(inputs, base, constraints, image_metrics, timings);
        candidate
    }

    pub(super) fn relayout(
        &mut self,
        inputs: &PreparedInputs,
        base: &ShapedText,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>,
    ) {
        crate::layout::layout_with_timings(
            layout_inputs(
                inputs,
                ShapedTextView::new(
                    base,
                    self.refinement.as_ref(),
                    self.glyph_metrics_override.as_ref(),
                ),
                image_metrics,
            ),
            crate::layout::LayoutOutputs {
                geometry: &mut self.geometry,
                state: &mut self.layout_state,
                placement: &mut self.placement,
                scratch: &mut self.layout_scratch,
            },
            constraints,
            timings,
        );
    }

    fn into_document(
        self,
        inputs: Arc<PreparedInputs>,
        base_shaped: Box<ShapedText>,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
    ) -> LaidOutDocument {
        let mut document = LaidOutDocument {
            inputs,
            base_shaped,
            geometry: self.geometry,
            placement: self.placement,
            refinement: self.refinement,
            glyph_metrics_override: self.glyph_metrics_override,
            layout_state: self.layout_state,
            layout_scratch: self.layout_scratch,
            paint_colors: Vec::new(),
            last_constraints: constraints,
            last_image_metrics: image_metrics.clone(),
        };
        document.rebuild_anchor_positions();
        document.rebuild_paint_colors();
        document
    }

    fn publish(
        self,
        document: &mut LaidOutDocument,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
    ) {
        document.geometry = self.geometry;
        document.placement = self.placement;
        document.refinement = self.refinement;
        document.glyph_metrics_override = self.glyph_metrics_override;
        document.layout_state = self.layout_state;
        document.layout_scratch = self.layout_scratch;
        document.last_constraints = constraints;
        document.last_image_metrics = image_metrics.clone();
        document.rebuild_anchor_positions();
        document.rebuild_paint_colors();
    }
}

fn layout_inputs<'a>(
    inputs: &'a PreparedInputs,
    shaped: ShapedTextView<'a>,
    image_metrics: &'a ImageMetrics,
) -> crate::layout::LayoutInputs<'a> {
    crate::layout::LayoutInputs {
        document: &inputs.document,
        styles: &inputs.styles,
        topology: &inputs.layout_tree,
        inline_content: inputs.text.content(),
        glyphs: shaped.glyphs,
        glyph_metrics: shaped.glyph_metrics,
        font_metrics: &shaped.shared.font_metrics,
        text_geometry: Some(shaped.text_geometry),
        ellipsis_glyphs: &shaped.shared.ellipsis_glyphs,
        hyphen_glyphs: &shaped.shared.hyphen_glyphs,
        inline_plan_generation: shaped.inline_plan_generation,
        image_metrics,
        paint: &inputs.paint,
        text_style_ranges: shaped.paint_ranges,
        resource_generation: shaped.resource_generation,
    }
}

/// A fully shaped and laid-out document.
///
/// No public constructor exists: values can only be produced from a
/// [`ShapedDocument`]. Relayout mutates only layout-owned data, retaining the
/// immutable shaping inputs and reusing top-level layout buffers.
pub struct LaidOutDocument {
    pub(super) inputs: Arc<PreparedInputs>,
    pub(super) geometry: BoxGeometry,
    /// Retained placement relationships. Publication updates the owning
    /// geometry and fragment coordinates in place.
    pub(super) placement: crate::layout::PlacementState,
    /// Viewport-independent shaping retained so a width change can recompute
    /// the dynamic extent of `::first-line` without accumulating old styles.
    pub(super) base_shaped: Box<ShapedText>,
    pub(super) refinement: Option<FirstLineRefinement>,
    pub(super) glyph_metrics_override: Option<GlyphMetrics>,
    pub(super) layout_state: LayoutState,
    pub(super) layout_scratch: LayoutScratch,
    pub(super) paint_colors: Vec<PaintColorRun>,
    pub(super) last_constraints: LayoutConstraints,
    pub(super) last_image_metrics: ImageMetrics,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct TextPaintRange {
    pub(crate) glyphs: std::ops::Range<u32>,
    pub(crate) style: StyleIndices,
}

#[derive(Clone)]
pub(crate) struct PaintColorRun {
    pub(crate) glyphs: std::ops::Range<u32>,
    pub(crate) color: u32,
}

impl LaidOutDocument {
    /// Move the retained shape out when starting an independent layout pass.
    /// Glyph storage keeps its address; prior layout buffers are released.
    pub fn into_shaped(self) -> ShapedDocument {
        ShapedDocument { inputs: self.inputs, shaped: self.base_shaped }
    }

    /// Borrow source text from this document's sole preparation owner.
    pub fn visible_text_runs(&self) -> impl Iterator<Item = (&str, Option<&ContentLanguage>)> {
        visible_text_runs(&self.inputs)
    }

    pub fn note_ids(&self) -> Vec<&str> { note_ids(&self.inputs) }

    /// Explicitly detached note documents share only immutable DOM/styles.
    pub fn scoped_to_element_id(&self, id: &str) -> Option<PreparedDocument> {
        scoped_to_element_id(&self.inputs, id)
    }

    /// Replace backend shaping while retaining preparation. Candidate layout
    /// borrows both inputs and the new shape; publication moves the shape once.
    pub fn reshape_and_layout_with_metrics_and_shaper(
        &mut self, constraints: LayoutConstraints, image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<(), crate::ShapeError> {
        let (shaped, candidate) = shape_and_layout_candidate(&self.inputs, constraints, image_metrics, glyph_shaper)?;
        self.base_shaped = shaped;
        candidate.publish(self, constraints, image_metrics);
        Ok(())
    }

    /// Update a palette without manufacturing another prepared-stage owner.
    pub fn repaint_with_reader_overrides(&mut self, overrides: &ReaderStyleOverrides) -> Result<(), crate::ShapeError> {
        let paint = self.inputs.paint.with_overrides(&self.inputs.styles, &self.inputs.layout_tree, overrides);
        Arc::get_mut(&mut self.inputs).ok_or_else(crate::ShapeError::document_is_shared)?.paint = Arc::new(paint);
        self.rebuild_paint_colors();
        crate::layout::engine::repaint(layout_inputs(&self.inputs, ShapedTextView::new(&self.base_shaped, self.refinement.as_ref(), self.glyph_metrics_override.as_ref()), &self.last_image_metrics), &mut self.geometry, &mut self.layout_state, self.last_constraints);
        Ok(())
    }

    /// Stage a palette together with fallible shaping/layout and restore it on
    /// failure. The previous published geometry remains intact until commit.
    pub fn update_with_reader_overrides(
        &mut self, overrides: &ReaderStyleOverrides, constraints: LayoutConstraints,
        image_metrics: &ImageMetrics, reshape: bool, glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<(), crate::ShapeError> {
        let paint = self.inputs.paint.with_overrides(&self.inputs.styles, &self.inputs.layout_tree, overrides);
        let owner = Arc::get_mut(&mut self.inputs).ok_or_else(crate::ShapeError::document_is_shared)?;
        let previous = std::mem::replace(&mut owner.paint, Arc::new(paint));
        let result = if reshape { self.reshape_and_layout_with_metrics_and_shaper(constraints, image_metrics, glyph_shaper) }
            else { self.relayout_with_metrics_and_shaper(constraints, image_metrics, glyph_shaper) };
        if result.is_err() {
            Arc::get_mut(&mut self.inputs).expect("candidate borrows preparation").paint = previous;
        }
        result
    }

    pub fn reprepare_and_layout(
        &mut self, root_font_size: RootFontSize, note_flow: NoteFlow, overrides: &ReaderStyleOverrides,
        resolve: impl FnOnce(&Document) -> ComputedStyles, constraints: LayoutConstraints,
        image_metrics: &ImageMetrics, glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<(), crate::ShapeError> {
        let (shaped, candidate) = with_reprepared(&mut self.inputs, root_font_size, note_flow, overrides, resolve, |inputs| {
            shape_and_layout_candidate(inputs, constraints, image_metrics, glyph_shaper)
        })?;
        self.base_shaped = shaped;
        candidate.publish(self, constraints, image_metrics);
        Ok(())
    }

    pub(super) fn shaped(&self) -> ShapedTextView<'_> {
        ShapedTextView::new(&self.base_shaped, self.refinement.as_ref(), self.glyph_metrics_override.as_ref())
    }
    /// Resolve paint over retained geometry, including specialized formatting
    /// contexts. A different prepared geometry epoch must be laid out first.
    pub fn repaint_from(&mut self, prepared: &PreparedDocument) -> bool {
        if !Arc::ptr_eq(&self.base_shaped.geometry_tree, &prepared.inputs.layout_tree) { return false; }
        self.inputs = prepared.inputs.clone();
        self.rebuild_paint_colors();
        crate::layout::engine::repaint(layout_inputs(&self.inputs, ShapedTextView::new(&self.base_shaped, self.refinement.as_ref(), self.glyph_metrics_override.as_ref()), &self.last_image_metrics), &mut self.geometry, &mut self.layout_state, self.last_constraints);
        true
    }

    /// Refresh specialized decoration geometry while retaining shaped text.
    /// The paint variant must come from this document's prepared publication.
    pub fn relayout_with_paint_from(
        &mut self, prepared: &PreparedDocument, glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<(), crate::ShapeError> {
        let image_metrics = self.last_image_metrics.clone();
        self.relayout_from(prepared, self.last_constraints, &image_metrics, glyph_shaper)
    }

    /// Update geometry and paint together while retaining shaping. A failed
    /// exact-line refinement leaves both the previous geometry and palette.
    pub fn relayout_from(
        &mut self, prepared: &PreparedDocument, constraints: LayoutConstraints,
        image_metrics: &ImageMetrics, glyph_shaper: &mut impl crate::GlyphShaper,
    ) -> Result<(), crate::ShapeError> {
        assert!(Arc::ptr_eq(&self.base_shaped.geometry_tree, &prepared.inputs.layout_tree));
        let previous_inputs = self.inputs.clone();
        self.inputs = prepared.inputs.clone();
        let result = self.relayout_with_metrics_and_shaper(constraints, image_metrics, glyph_shaper);
        if result.is_err() { self.inputs = previous_inputs; }
        result
    }

    pub(super) fn rebuild_paint_colors(&mut self) {
        for line in &self.layout_state.line_output.lines {
            let Some(detail_idx) = line.details else { continue; };
            let details = &mut self.layout_state.line_output.line_details[detail_idx.get() as usize - 1];
            if details.paint_color.is_some() {
                let root = crate::shaping::whitespace_context_root(&self.inputs.layout_tree, line.owner_box_idx as usize);
                details.paint_color = crate::shaping::first_line_style_for_inline_root(&self.inputs.document, &self.inputs.styles, &self.inputs.layout_tree, self.inputs.text.content(), root)
                    .map(|indices| self.inputs.paint.resolve(&self.inputs.styles, indices, root).foreground);
            }
        }

        if self.inputs.paint.is_default() {
            let had_updates = !self.paint_colors.is_empty();
            self.paint_colors.clear();
            if had_updates {
                prepare_text_paint(layout_inputs(&self.inputs, ShapedTextView::new(&self.base_shaped, self.refinement.as_ref(), self.glyph_metrics_override.as_ref()), &self.last_image_metrics), &mut self.layout_state, &[]);
            }
            return;
        }
        let mut colors: Vec<PaintColorRun> = Vec::new();
        for item in self.inputs.text.content().inline_items() {
            let (InlineItemKind::Text { glyphs } | InlineItemKind::Marker { glyphs }) = &item.kind else { continue; };
            let base = self.inputs.layout_tree.get_box_style_indices(item.box_idx as usize).unwrap_or_else(|| self.inputs.styles.default_indices());
            let mut start = glyphs.start;
            while start < glyphs.end {
                let (override_style, mut end) = self.inputs.text.first_letter_styles().segment(start, glyphs.end);
                let index = self.shaped().paint_ranges.partition_point(|range| range.glyphs.end <= start);
                let mut style = override_style.unwrap_or(base);
                if let Some(range) = self.shaped().paint_ranges.get(index) {
                    if range.glyphs.start <= start { style = range.style; end = end.min(range.glyphs.end); }
                    else { end = end.min(range.glyphs.start); }
                }
                let color = if style == base {
                    self.inputs.paint.box_colors(item.box_idx as usize).expect("active palette has every box binding").foreground
                } else {
                    self.inputs.paint.resolve(&self.inputs.styles, style, item.box_idx as usize).foreground
                };
                if let Some(last) = colors.last_mut().filter(|last| last.glyphs.end == start && last.color == color) {
                    last.glyphs.end = end;
                } else {
                    colors.push(PaintColorRun { glyphs: start..end, color });
                }
                start = end;
            }
        }
        self.paint_colors = colors;
        prepare_text_paint(layout_inputs(&self.inputs, ShapedTextView::new(&self.base_shaped, self.refinement.as_ref(), self.glyph_metrics_override.as_ref()), &self.last_image_metrics), &mut self.layout_state, &self.paint_colors);
    }

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
                || match style.position() {
                    html_style_model::PositionMode::Absolute => {
                        style.inset_bottom().is_some()
                            || style.inset_top().is_some_and(|top| top.has_percentage())
                    }
                    html_style_model::PositionMode::Relative => {
                        style.inset_top().is_some_and(|top| top.has_percentage())
                            || style
                                .inset_bottom()
                                .is_some_and(|bottom| bottom.has_percentage())
                    }
                    html_style_model::PositionMode::Static => false,
                }
        })
    }

    /// Borrow the published output without copying any document storage.
    /// A view cannot outlive a mutation of its owning layout:
    ///
    /// ```compile_fail,E0502
    /// use html_layout::{LaidOutDocument, LayoutConstraints};
    /// fn resize(document: &mut LaidOutDocument, constraints: LayoutConstraints) {
    ///     let view = document.render_view();
    ///     document.relayout(constraints);
    ///     let _ = view.text().line_count();
    /// }
    /// ```
    pub fn render_view(&self) -> RenderView<'_> {
        RenderView { doc: self }
    }

    pub(super) fn text_geometry(&self) -> crate::shaping::TextGeometryView<'_> {
        self.shaped().text_geometry
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
        if self.base_shaped.resource_generation != glyph_shaper.glyph_resources().generation() {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        if !self.inputs.styles.has_first_line_styles() {
            // Ordinary resizing cannot fail during shaping. Retain geometry,
            // line output, placement and scratch storage instead of allocating
            // a private refinement candidate for every viewport change.
            let resources = glyph_shaper.glyph_resources();
            if resources.len() > self.shaped().glyph_metrics.len() {
                // Notes and renderer-owned auxiliary glyphs can have appended
                // metrics since the document was shaped. Keep them addressable.
                self.glyph_metrics_override = Some(resources.snapshot());
            }
            self.relayout_with_metrics(constraints, image_metrics);
            return Ok(());
        }
        let candidate = LayoutCandidate::new(&self.inputs, &self.base_shaped, constraints, image_metrics,
            None, std::mem::take(&mut self.layout_scratch));
        let shaping_seed = glyph_shaper.glyph_resources().snapshot();
        let needs_append = !first_line_style_ranges(&self.inputs, &candidate.layout_state).is_empty();
        let refine = |glyph_shaper: &mut _| {
            refine_first_lines(candidate, &self.inputs, &self.base_shaped, shaping_seed, constraints, image_metrics, glyph_shaper)
        };
        let candidate = if needs_append { with_append_shaping(glyph_shaper, refine)? } else { refine(glyph_shaper)? };
        candidate.publish(self, constraints, image_metrics);
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

    pub(super) fn relayout_impl(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>,
    ) {
        crate::layout::layout_with_timings(
            layout_inputs(&self.inputs, ShapedTextView::new(&self.base_shaped, self.refinement.as_ref(), self.glyph_metrics_override.as_ref()), image_metrics),
            crate::layout::LayoutOutputs {
                geometry: &mut self.geometry,
                state: &mut self.layout_state,
                placement: &mut self.placement,
                scratch: &mut self.layout_scratch,
            },
            constraints,
            timings,
        );
        self.rebuild_anchor_positions();
        self.rebuild_paint_colors();
    }

    fn rebuild_anchor_positions(&mut self) {
        self.layout_state.semantic_indexes.anchor_positions.clear();
        for box_idx in 0..self.inputs.layout_tree.box_count() {
            let Some(id_idx) = box_id(self.document(), &self.inputs.layout_tree, box_idx) else {
                continue;
            };
            let mut y = self.anchor_glyph(id_idx).and_then(|glyph| {
                let line_idx = self
                    .layout_state
                    .line_output
                    .glyph_line_indices
                    .get(glyph as usize)?;
                self.layout_state
                    .line_output
                    .lines
                    .get(line_idx)
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

    #[cfg(test)]
    pub(crate) fn glyph_metrics(&self) -> &GlyphMetrics {
        self.shaped().glyph_metrics
    }

    pub(crate) fn link_href_for_glyph(&self, glyph_idx: u32) -> Option<u16> {
        self.base_shaped
            .shared
            .link_glyph_targets
            .get(&glyph_idx)
            .map(|target| target.href)
    }

    pub(crate) fn glyph_is_note_reference(&self, glyph_idx: u32) -> bool {
        self.base_shaped
            .shared
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
        self.base_shaped.shared.anchor_glyphs.get(&id_idx).copied()
    }

    pub(crate) fn anchor_glyphs(&self) -> &FxHashMap<u16, u32> {
        &self.base_shaped.shared.anchor_glyphs
    }

    /// Refreshes the immutable render snapshot after the backend registers
    /// auxiliary glyphs in its active resource store.
    pub fn refresh_glyph_metrics_from(&mut self, glyph_shaper: &mut impl crate::GlyphShaper) -> Result<(), crate::ShapeError> {
        if self.base_shaped.resource_generation != glyph_shaper.glyph_resources().generation() {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        self.glyph_metrics_override = Some(glyph_shaper.glyph_resources().snapshot());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn glyphs(&self) -> crate::shaping::GlyphSlice<'_> {
        self.shaped().glyphs
    }

    /// Returns the DOM text node and UTF-16 source offset for the glyph.
    pub(crate) fn get_dom_node_for_glyph(&self, glyph_idx: u32) -> Option<(u32, usize)> {
        for run in self.inputs.text.content().inline_items() {
            if let InlineItemKind::Text { glyphs } = &run.kind
                && glyphs.contains(&glyph_idx)
            {
                let node_idx = run.dom_text_node?;
                let source_offset =
                    self.inputs.text.content()
                        .glyph_source_offset(glyph_idx as usize)? as usize;
                return Some((node_idx, source_offset));
            }
        }
        None
    }

    pub(crate) fn glyph_source_offset(&self, glyph_idx: u32) -> Option<u32> {
        self.inputs.text.content()
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
        self.base_shaped
            .shared
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
            color: self.inputs.paint.box_colors(box_idx).map_or_else(|| style.color(), |paint| paint.foreground),
            font_family: style.font_family(),
            letter_spacing: style.letter_spacing(),
            word_spacing: style.word_spacing(),
            text_decoration: style.text_decoration_lines(),
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
        report.extend_prefixed("LaidOutDocument.paint", self.inputs.paint.memory_usage_report());
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
        report.add(
            "LaidOutDocument.local_placement",
            self.placement.memory_usage_bytes(),
            self.box_count(),
        );
        report.extend_prefixed(
            "LaidOutDocument.inline_content",
            self.inputs.text.memory_usage_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument.layout_state",
            self.layout_state.memory_usage_report(),
        );
        let mut seen = FxHashSet::default();
        report.extend_prefixed("LaidOutDocument", self.base_shaped.memory_usage_report(&mut seen));
        if let Some(refinement) = &self.refinement {
            report.extend_prefixed("LaidOutDocument.refinement", refinement.memory_usage_report(&mut seen));
        }
        if let Some(metrics) = &self.glyph_metrics_override {
            report.extend_prefixed("LaidOutDocument.auxiliary_metrics", metrics.memory_usage_report_with_shared(&mut seen));
        }
        report.add_slice_storage::<PaintColorRun>(
            "LaidOutDocument.paint_colors.storage",
            self.paint_colors.capacity(), self.paint_colors.len(),
        );
        report.extend_prefixed("LaidOutDocument.paragraph_layouts", self.layout_scratch.paragraph_layout_memory_report());
        report.extend_prefixed(
            "LaidOutDocument.inline_plans",
            self.layout_scratch.inline_plan_memory_report(),
        );
        report.add(
            "LaidOutDocument.rollback_image_metrics",
            self.last_image_metrics.memory_usage_bytes(),
            self.last_image_metrics.len(),
        );

        report
    }

    pub fn memory_usage_bytes(&self) -> usize {
        self.memory_usage_report().total_bytes()
    }
}

fn visible_text_runs(inputs: &PreparedInputs) -> impl Iterator<Item = (&str, Option<&ContentLanguage>)> {
        let document = inputs.document.as_ref();
        let styles = inputs.styles.as_ref();
        let mut pending: Vec<_> = document.dom_root().into_iter().collect();
        std::iter::from_fn(move || {
            while let Some(node) = pending.pop() {
                match document.node_ref(node) {
                    Some(NodeRef::Element(element)) => {
                        if styles
                            .style_for_node(node)
                            .and_then(|style| styles.box_model_style(style))
                            .is_some_and(|style| style.display == Display::None)
                        {
                            continue;
                        }
                        let first_child = pending.len();
                        pending.extend(element.children());
                        pending[first_child..].reverse();
                    }
                    Some(NodeRef::Text(text)) => {
                        let Some(style) = text
                            .parent()
                            .and_then(|parent| styles.style_for_node(parent))
                            .and_then(|style| styles.text_style(style))
                        else {
                            continue;
                        };
                        if style.visibility != Visibility::Visible {
                            continue;
                        }
                        let language = style.language.and_then(|language| styles.language(language));
                        return Some((text.text(), language));
                    }
                    None => {}
                }
            }
            None
        })
    }

fn scoped_to_element_id(inputs: &PreparedInputs, id: &str) -> Option<PreparedDocument> {
        let document = &inputs.document;
        let root = document
            .node_ids()
            .find(|node| document.get_dom_id(*node) == Some(id))?;
        document.element_ref(root)?;

        let mut layout_tree = LayoutTree::default();
        let mut inline_content = InlineContent::default();
        crate::layout::build_layout_inputs_from(
            document,
            &inputs.styles,
            &mut layout_tree,
            &mut inline_content,
            NoteFlow::InFlow,
            root,
        );
        let text = Arc::new(PreparedText::new(document, &inputs.styles, &layout_tree, inline_content));
        let paint = Arc::new(inputs.paint.for_tree(document, &inputs.styles, &layout_tree));
        Some(PreparedDocument {
            inputs: Arc::new(PreparedInputs {
                document: Arc::clone(&inputs.document),
                styles: Arc::clone(&inputs.styles),
                layout_tree: Arc::new(layout_tree),
                text,
                note_flow: NoteFlow::InFlow,
                paint,
            }),
        })
    }

fn note_ids(inputs: &PreparedInputs) -> Vec<&str> {
        let document = &inputs.document;
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
