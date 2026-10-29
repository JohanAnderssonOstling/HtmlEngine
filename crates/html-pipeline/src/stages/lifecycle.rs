//! Prepared, shaped, and laid-out document lifecycle types.

use super::paint::{NotePaint, PaintPalette, PaintStyles};
use super::*;
use crate::ReaderStyleOverrides;
use crate::layout::{InlinePlanGeneration, LayoutScratch, prepare_text_paint};
use html_dom::{NodeRef, RootFontSize};
use html_engine_model::{PaintColorRun, PaintView, TextPaintRange};
use html_style_model::{ContentLanguage, Display, Visibility};
use html_text::PreparedText;
use std::sync::atomic::{AtomicU64, Ordering};

/// Identifies the preparation used by geometry without owning its box tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreparationGeneration(u64);

impl PreparationGeneration {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(
            NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("preparation generation space exhausted"),
        )
    }
}

/// Canonical inputs are owned once; derived stages may borrow each input.
pub(crate) enum Storage<'a, T> {
    Owned(Box<T>),
    Borrowed(&'a T),
}

impl<T> Storage<'_, T> {
    pub(super) fn owned(value: T) -> Self {
        Self::Owned(Box::new(value))
    }
    fn owned_mut(&mut self) -> Result<&mut T, crate::ShapeError> {
        match self {
            Self::Owned(value) => Ok(value),
            Self::Borrowed(_) => Err(crate::ShapeError::borrowed_document()),
        }
    }
    pub(super) fn into_owned(self) -> Option<T> {
        match self {
            Self::Owned(value) => Some(*value),
            Self::Borrowed(_) => None,
        }
    }
}

impl<T> std::ops::Deref for Storage<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Owned(value) => value,
            Self::Borrowed(value) => value,
        }
    }
}

impl<T> AsRef<T> for Storage<'_, T> {
    fn as_ref(&self) -> &T {
        self
    }
}

/// Styled content with formatting structure, before logical-text preparation.
///
/// This stage owns its DOM, styles, box topology and raw inline buffers. It
/// cannot be shaped or laid out. Consuming it prepares the canonical text and
/// moves every owned input into a PreparedDocument without copying buffers.
///
/// ```compile_fail
/// fn skip_preprocessing(document: html_pipeline::engine::FormattingDocument,
///                       shaper: &mut impl html_pipeline::engine::GlyphShaper) {
///     let _ = document.shape(shaper);
/// }
/// ```
pub struct FormattingDocument {
    document: Document,
    styles: ComputedStyles,
    layout_tree: LayoutTree,
    inline_content: InlineContent,
    note_flow: NoteFlow,
}

impl FormattingDocument {
    pub fn try_new(document: Document, styles: ComputedStyles) -> Result<Self, PrepareError> {
        Self::try_new_with_note_flow(document, styles, NoteFlow::default())
    }

    pub fn try_new_with_note_flow(
        document: Document,
        styles: ComputedStyles,
        note_flow: NoteFlow,
    ) -> Result<Self, PrepareError> {
        profile_scope!("prepare.formatting_structure");
        profile!("prepare.validate_styles", styles.validate_for(&document))
            .map_err(PrepareError::from)?;
        let mut document = document;
        profile!(
            "prepare.mathml",
            super::mathml::prepare(&mut document, &styles)
        );
        let mut layout_tree = LayoutTree::default();
        let mut inline_content = InlineContent::default();
        profile!(
            "prepare.build_box_tree",
            html_format::build_layout_inputs(
                &document,
                &styles,
                &mut layout_tree,
                &mut inline_content,
                note_flow
            )
        );
        Ok(Self {
            document,
            styles,
            layout_tree,
            inline_content,
            note_flow,
        })
    }

    /// Run html-text after formatting boundaries are known and before shaping.
    pub fn prepare_text(self) -> PreparedDocument<'static> {
        let Self {
            document,
            styles,
            layout_tree,
            inline_content,
            note_flow,
        } = self;
        let text = prepare_logical_text(&document, &styles, &layout_tree, inline_content);
        let paint = profile!(
            "prepare.paint_bindings",
            PaintStyles::new(&document, &styles, &layout_tree)
        );
        PreparedDocument {
            inputs: Box::new(PreparedInputs {
                generation: PreparationGeneration::new(),
                document: Storage::owned(document),
                styles: Storage::owned(styles),
                layout_tree: Storage::owned(layout_tree),
                text: Storage::owned(text),
                note_flow,
                paint: Storage::owned(paint),
            }),
        }
    }
}

fn prepare_logical_text(
    document: &Document,
    styles: &ComputedStyles,
    tree: &LayoutTree,
    content: InlineContent,
) -> PreparedText {
    #[cfg(feature = "profiling")]
    let observe = crate::profiling::scope;
    #[cfg(not(feature = "profiling"))]
    let observe = |_| ();
    PreparedText::with_observer(document, styles, tree, content, observe)
}

pub(crate) struct PreparedInputs<'a> {
    generation: PreparationGeneration,
    pub(super) document: Storage<'a, Document>,
    pub(super) styles: Storage<'a, ComputedStyles>,
    pub(super) layout_tree: Storage<'a, LayoutTree>,
    pub(super) text: Storage<'a, PreparedText>,
    pub(super) note_flow: NoteFlow,
    pub(crate) paint: Storage<'a, PaintStyles<'a>>,
}

impl PreparedInputs<'_> {
    fn require_static_text(&self) -> Result<(), crate::ShapeError> {
        if self.styles.has_first_line_styles() {
            return Err(crate::ShapeError::first_line_shaper_required());
        }
        Ok(())
    }

    fn borrowed(&self) -> PreparedInputs<'_> {
        PreparedInputs {
            generation: self.generation,
            document: Storage::Borrowed(&self.document),
            styles: Storage::Borrowed(&self.styles),
            layout_tree: Storage::Borrowed(&self.layout_tree),
            text: Storage::Borrowed(&self.text),
            note_flow: self.note_flow,
            paint: Storage::Borrowed(&self.paint),
        }
    }
}

/// Styled content with a stable box tree and unshaped inline text.
///
/// This is the only input accepted by [`PreparedDocument::shape`]. Its fields
/// are private so callers cannot pair layout inputs with a different DOM.
/// Skipping shaping is rejected by the type system:
///
/// ```compile_fail
/// fn skip_shaping(document: html_pipeline::engine::PreparedDocument) {
///     let _ = document.layout(800.0, 20.0);
/// }
/// ```
///
/// Raw style/layout storage is not part of the stage contract:
///
/// ```compile_fail
/// fn reach_through(document: &html_pipeline::engine::PreparedDocument) {
///     let _ = document.styles();
/// }
/// ```
pub struct PreparedDocument<'a> {
    pub(super) inputs: Box<PreparedInputs<'a>>,
}

pub(crate) struct ShapedText {
    pub(super) resource_generation: crate::GlyphResourceGeneration,
    preparation: PreparationGeneration,
    geometry_box_count: usize,
    pub(super) glyphs: Vec<GlyphId>,
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
    pub(super) text_geometry: TextGeometryView<'a>,
    pub(super) paint_ranges: &'a [TextPaintRange],
    pub(super) shared: &'a SharedShapedData,
    pub(super) inline_plan_generation: InlinePlanGeneration,
}

impl<'a> ShapedTextView<'a> {
    fn new(base: &'a ShapedText, refinement: Option<&'a FirstLineRefinement>) -> Self {
        Self {
            resource_generation: base.resource_generation,
            glyphs: GlyphSlice::new(
                &base.glyphs,
                refinement.map_or(&[], |refined| &refined.glyph_patches),
            ),
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
/// fn duplicate(shape: html_pipeline::engine::ShapedDocument) {
///     let _ = shape.clone();
/// }
/// ```
/// Owns the shaped text until it is moved into a layout document.
/// Independent layouts execute shaping separately; readers borrow publication.
///
/// ```compile_fail
/// fn copy_shape(document: &html_pipeline::engine::ShapedDocument) {
///     let _: html_pipeline::engine::ShapedDocument = document.clone();
/// }
/// ```
pub struct ShapedDocument<'a> {
    pub(super) inputs: Box<PreparedInputs<'a>>,
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
        report.add(
            "text_geometry",
            self.text_geometry.memory_usage_bytes_with_shared(seen),
            self.glyphs.len(),
        );
        if seen.insert((&self.shared as *const SharedShapedData).cast()) {
            report.add(
                "shared_metadata",
                std::mem::size_of::<SharedShapedData>(),
                1,
            );
            report.add(
                "font_metrics",
                self.shared.font_metrics.memory_usage_bytes(),
                self.geometry_box_count,
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
    work: impl FnOnce(&mut S, &mut GlyphResourceStore) -> Result<T, crate::ShapeError>,
    resources: &mut GlyphResourceStore,
) -> Result<T, crate::ShapeError> {
    if glyph_shaper.resource_generation() != resources.generation() {
        return Err(crate::ShapeError::resource_generation_mismatch());
    }
    glyph_shaper.begin_append_shaping()?;
    let checkpoint = resources.checkpoint();
    let result = work(glyph_shaper, resources);
    if result.is_ok() {
        glyph_shaper.commit_append_shaping();
    } else {
        glyph_shaper.rollback_append_shaping();
        assert!(resources.rollback_to(checkpoint));
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
    inputs: &mut PreparedInputs<'_>,
    root_font_size: RootFontSize,
    note_flow: NoteFlow,
    overrides: &ReaderStyleOverrides,
    resolve: impl FnOnce(&Document) -> ComputedStyles,
    apply: impl FnOnce(&PreparedInputs<'_>) -> Result<T, crate::ShapeError>,
) -> Result<T, crate::ShapeError> {
    let document = inputs.document.owned_mut()?;
    let old_font = RootFontSize::new(document.root_font_size()).expect("validated root font size");
    document.set_root_font_size(root_font_size);
    let styles = resolve(document);
    styles
        .validate_for(document)
        .expect("resolver styles must belong to the canonical DOM");
    let math_undo = super::mathml::prepare_with_undo(document, &styles);
    let mut tree = LayoutTree::default();
    let mut content = InlineContent::default();
    html_format::build_layout_inputs(document, &styles, &mut tree, &mut content, note_flow);
    let text = prepare_logical_text(document, &styles, &tree, content);
    let mut paint = PaintStyles::new(document, &styles, &tree);
    let palette = paint
        .with_overrides(&styles, &tree, overrides)
        .into_palette();
    paint.replace_palette(palette);
    let old_generation = std::mem::replace(&mut inputs.generation, PreparationGeneration::new());
    let old_styles = std::mem::replace(&mut inputs.styles, Storage::owned(styles));
    let old_tree = std::mem::replace(&mut inputs.layout_tree, Storage::owned(tree));
    let old_text = std::mem::replace(&mut inputs.text, Storage::owned(text));
    let old_paint = std::mem::replace(&mut inputs.paint, Storage::owned(paint));
    let old_flow = std::mem::replace(&mut inputs.note_flow, note_flow);
    let result = apply(inputs);
    if result.is_err() {
        inputs.generation = old_generation;
        inputs.styles = old_styles;
        inputs.layout_tree = old_tree;
        inputs.text = old_text;
        inputs.paint = old_paint;
        inputs.note_flow = old_flow;
        let document = inputs
            .document
            .owned_mut()
            .expect("canonical DOM owner is retained");
        document.set_root_font_size(old_font);
        super::mathml::restore(document, math_undo);
    }
    result
}

impl<'a> PreparedDocument<'a> {
    pub fn shape_and_layout(
        self,
        constraints: LayoutConstraints,
        shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, crate::ShapeError> {
        let metrics = ImageMetrics::from_document(self.inputs.document.as_ref());
        let (shaped, candidate) =
            shape_and_layout_candidate(&self.inputs, constraints, &metrics, shaper, resources)?;
        Ok(candidate.into_document(self.inputs, shaped, constraints, &metrics, resources))
    }

    /// Source text participating in visible content, with inherited language,
    /// before any platform shaping. Display suppression applies to subtrees;
    /// visibility can be restored by a descendant's own computed style.
    pub fn visible_text_runs(&self) -> impl Iterator<Item = (&str, Option<&ContentLanguage>)> {
        visible_text_runs(&self.inputs)
    }

    pub fn reprepare_and_layout(
        &mut self,
        root_font_size: RootFontSize,
        note_flow: NoteFlow,
        overrides: &ReaderStyleOverrides,
        resolve: impl FnOnce(&Document) -> ComputedStyles,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'_>, crate::ShapeError> {
        let (shaped, candidate) = with_reprepared(
            &mut self.inputs,
            root_font_size,
            note_flow,
            overrides,
            resolve,
            |inputs| {
                shape_and_layout_candidate(
                    inputs,
                    constraints,
                    image_metrics,
                    glyph_shaper,
                    resources,
                )
            },
        )?;
        Ok(candidate.into_document(
            Box::new(self.inputs.borrowed()),
            shaped,
            constraints,
            image_metrics,
            resources,
        ))
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
        Ok(FormattingDocument::try_new_with_note_flow(document, styles, note_flow)?.prepare_text())
    }

    /// Apply color preferences to an unpainted prepared document. The cascade
    /// and typography are retained, including their publication colors.
    pub fn into_reader_paint_overrides(
        mut self,
        overrides: &ReaderStyleOverrides,
    ) -> Result<Self, PrepareError> {
        let paint = self
            .inputs
            .paint
            .with_overrides(&self.inputs.styles, &self.inputs.layout_tree, overrides)
            .into_palette();
        self.inputs
            .paint
            .owned_mut()
            .expect("canonical paint owner")
            .replace_palette(paint);
        Ok(self)
    }

    pub fn with_reader_paint_overrides(
        &self,
        overrides: &ReaderStyleOverrides,
    ) -> Result<PreparedDocument<'_>, PrepareError> {
        let paint = self.inputs.paint.with_overrides(
            &self.inputs.styles,
            &self.inputs.layout_tree,
            overrides,
        );
        let mut inputs = self.inputs.borrowed();
        inputs.paint = Storage::owned(paint);
        Ok(PreparedDocument {
            inputs: Box::new(inputs),
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
    pub fn scoped_to_element_id(&self, id: &str) -> Option<PreparedDocument<'_>> {
        scoped_to_element_id(&self.inputs, id)
    }

    /// Ids of this document's note bodies, in document order. A note without an
    /// id cannot be referenced, so it is not listed.
    pub fn note_ids(&self) -> Vec<&str> {
        note_ids(&self.inputs)
    }

    pub fn document(&self) -> &Document {
        self.inputs.document.as_ref()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test_styles(&self) -> &ComputedStyles {
        self.styles()
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn styles(&self) -> &ComputedStyles {
        self.inputs.styles.as_ref()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn style_view(&self, indices: StyleIndices) -> html_style_model::StyleView<'_> {
        self.inputs
            .styles
            .view(indices)
            .expect("validated style handle")
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn layout_tree(&self) -> &LayoutTree {
        self.inputs.layout_tree.as_ref()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn inline_content(&self) -> &InlineContent {
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
        report.extend_prefixed(
            "PreparedDocument.paint",
            self.inputs.paint.memory_usage_report(),
        );
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

    #[cfg(any(test, feature = "testing"))]
    pub fn box_at(&self, box_idx: usize) -> Option<&crate::layout_model::LayoutBox> {
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

    pub fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.inputs.layout_tree.box_at(box_idx)?.style()
    }

    pub fn box_text_color(&self, box_idx: usize) -> Option<u32> {
        let indices = self.box_style_indices(box_idx)?;
        Some(
            self.inputs
                .paint
                .resolve(&self.inputs.styles, indices, box_idx)
                .foreground,
        )
    }

    /// Move preparation into a shaped document with no shared ownership.
    pub fn shape(
        self,
        shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<ShapedDocument<'a>, crate::ShapeError> {
        shaper.begin_document_shaping();
        let previous_resources = std::mem::replace(
            resources,
            GlyphResourceStore::new(shaper.resource_generation()),
        );
        let result = shape_text(&self.inputs, shaper, resources);
        match result {
            Ok(shaped) => {
                shaper.commit_document_shaping();
                Ok(ShapedDocument {
                    inputs: self.inputs,
                    shaped,
                })
            }
            Err(error) => {
                shaper.rollback_document_shaping();
                *resources = previous_resources;
                Err(error)
            }
        }
    }

    /// Attach a completed layout to its sole preparation owner.
    pub fn with_layout(mut self, layout: DetachedLayout) -> LaidOutDocument<'a> {
        assert_eq!(self.inputs.generation, layout.base_shaped.preparation);
        if let Some(paint) = layout.paint {
            self.inputs
                .paint
                .owned_mut()
                .expect("canonical paint owner")
                .replace_palette(paint);
        }
        LaidOutDocument {
            inputs: self.inputs,
            geometry: layout.geometry,
            placement: layout.placement,
            base_shaped: layout.base_shaped,
            refinement: layout.refinement,
            layout_state: layout.layout_state,
            layout_scratch: layout.layout_scratch,
            paint_colors: layout.paint_colors,
            last_constraints: layout.last_constraints,
            last_image_metrics: layout.last_image_metrics,
        }
    }

    pub fn shape_borrowed(
        &self,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<ShapedDocument<'_>, crate::ShapeError> {
        glyph_shaper.begin_document_shaping();
        let previous_resources = std::mem::replace(
            resources,
            GlyphResourceStore::new(glyph_shaper.resource_generation()),
        );
        let result = self.shape_active_document(glyph_shaper, resources);
        if result.is_ok() {
            glyph_shaper.commit_document_shaping();
        } else {
            glyph_shaper.rollback_document_shaping();
            *resources = previous_resources;
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
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'_>, crate::ShapeError> {
        glyph_shaper.begin_document_shaping();
        let previous_resources = std::mem::replace(
            resources,
            GlyphResourceStore::new(glyph_shaper.resource_generation()),
        );
        let result = self
            .shape_active_document(glyph_shaper, resources)
            .and_then(|shaped| {
                let laid_out = shaped.layout_with_metrics_and_shaper_inner(
                    constraints,
                    image_metrics,
                    glyph_shaper,
                    resources,
                )?;
                Ok(laid_out)
            });
        if result.is_ok() {
            glyph_shaper.commit_document_shaping();
        } else {
            glyph_shaper.rollback_document_shaping();
            *resources = previous_resources;
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
        resources: &mut GlyphResourceStore,
    ) -> Result<(LaidOutDocument<'_>, ShapeLayoutTimings), crate::ShapeError> {
        glyph_shaper.begin_document_shaping();
        let previous_resources = std::mem::replace(
            resources,
            GlyphResourceStore::new(glyph_shaper.resource_generation()),
        );
        let mut timings = ShapeLayoutTimings::default();
        let shape_started = Instant::now();
        let result = self
            .shape_active_document(glyph_shaper, resources)
            .and_then(|shaped| {
                timings.shape = shape_started.elapsed();
                let layout_started = Instant::now();
                let laid_out = shaped.layout_with_metrics_and_shaper_inner(
                    constraints,
                    image_metrics,
                    glyph_shaper,
                    resources,
                )?;
                timings.layout = layout_started.elapsed();
                Ok((laid_out, timings))
            });
        if result.is_ok() {
            glyph_shaper.commit_document_shaping();
        } else {
            glyph_shaper.rollback_document_shaping();
            *resources = previous_resources;
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
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'_>, crate::ShapeError> {
        with_append_shaping(
            glyph_shaper,
            |glyph_shaper, resources| {
                let shaped = self.shape_active_document(glyph_shaper, resources)?;
                let laid_out = shaped.layout_with_metrics_and_shaper_inner(
                    constraints,
                    image_metrics,
                    glyph_shaper,
                    resources,
                )?;
                Ok(laid_out)
            },
            resources,
        )
    }

    fn shape_active_document(
        &self,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<ShapedDocument<'_>, crate::ShapeError> {
        Ok(ShapedDocument {
            inputs: Box::new(self.inputs.borrowed()),
            shaped: shape_text(&self.inputs, glyph_shaper, resources)?,
        })
    }
}

fn shape_text(
    inputs: &PreparedInputs,
    glyph_shaper: &mut impl crate::GlyphShaper,
    resources: &mut GlyphResourceStore,
) -> Result<Box<ShapedText>, crate::ShapeError> {
    profile_scope!("shape.total");

    let inline_content = inputs.text.content();
    let mut glyphs = profile!(
        "shape.allocate_glyphs",
        inline_content
            .characters()
            .iter()
            .map(|&character| character as GlyphId)
            .collect::<Vec<_>>()
    );
    let (ellipsis_glyphs, hyphen_glyphs, text_geometry, font_metrics) =
        crate::shaping::shape_document(
            &inputs.styles,
            &inputs.layout_tree,
            inline_content,
            &mut glyphs,
            inputs.text.first_letter_styles(),
            glyph_shaper,
            resources,
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
        resource_generation: resources.generation(),
        preparation: inputs.generation,
        geometry_box_count: inputs.layout_tree.box_count(),
        glyphs,
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
    resources: &mut GlyphResourceStore,
) -> Result<(Box<ShapedText>, LayoutCandidate), crate::ShapeError> {
    glyph_shaper.begin_document_shaping();
    let previous_resources = std::mem::replace(
        resources,
        GlyphResourceStore::new(glyph_shaper.resource_generation()),
    );
    let result = (|| {
        let shaped = shape_text(inputs, glyph_shaper, resources)?;
        let candidate = LayoutCandidate::new(
            inputs,
            &shaped,
            constraints,
            image_metrics,
            None,
            LayoutScratch::default(),
            resources,
        );

        let candidate = refine_first_lines(
            candidate,
            inputs,
            &shaped,
            constraints,
            image_metrics,
            glyph_shaper,
            None,
            resources,
        )?;
        Ok((shaped, candidate))
    })();
    if result.is_ok() {
        glyph_shaper.commit_document_shaping();
    } else {
        glyph_shaper.rollback_document_shaping();
        *resources = previous_resources;
    }
    result
}

/// A prepared document whose inline text has concrete glyph IDs and metrics.
/// Layout constraints can change without repeating parsing, styling, box-tree
/// construction, or shaping.
impl<'a> ShapedDocument<'a> {
    pub fn document(&self) -> &Document {
        self.inputs.document.as_ref()
    }

    pub fn glyphs(&self) -> &[GlyphId] {
        &self.shaped.glyphs
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn glyph_metrics<'r>(&self, resources: &'r GlyphResourceStore) -> &'r GlyphMetrics {
        assert_eq!(self.shaped.resource_generation, resources.generation());
        resources.metrics()
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
        report.extend_prefixed(
            "ShapedDocument",
            self.shaped.memory_usage_report(&mut FxHashSet::default()),
        );
        report
    }

    /// Lay out text whose styles do not require first-line reshaping.
    /// Use layout_with_shaper when first-line styles are present.
    pub fn layout(
        self,
        constraints: LayoutConstraints,
        resources: &GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, crate::ShapeError> {
        let image_metrics = ImageMetrics::from_document(self.inputs.document.as_ref());
        self.layout_with_metrics(constraints, &image_metrics, resources)
    }

    pub fn layout_with_timings(
        self,
        constraints: LayoutConstraints,
        resources: &GlyphResourceStore,
    ) -> Result<(LaidOutDocument<'a>, crate::LayoutTimings), crate::ShapeError> {
        let image_metrics = ImageMetrics::from_document(self.inputs.document.as_ref());
        self.layout_with_timings_and_metrics(constraints, &image_metrics, resources)
    }

    pub fn layout_with_metrics(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, crate::ShapeError> {
        self.inputs.require_static_text()?;
        if self.shaped.resource_generation != resources.generation() {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        Ok(self.layout_impl(constraints, image_metrics, None, resources))
    }

    /// Coordinate layout and any width-dependent first-line reshaping.
    pub fn layout_with_shaper(
        self,
        constraints: LayoutConstraints,
        shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, crate::ShapeError> {
        let metrics = ImageMetrics::from_document(self.inputs.document.as_ref());
        self.layout_with_metrics_and_shaper(constraints, &metrics, shaper, resources)
    }

    /// Compatibility entry point. Document shaping is authoritative, so width
    /// changes only invoke the backend shaper for width-dependent pseudo text.
    pub fn layout_with_metrics_and_shaper(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, crate::ShapeError> {
        self.layout_coordinated(constraints, image_metrics, glyph_shaper, None, resources)
    }

    /// Timed layout follows the same first-line coordination as ordinary layout.
    pub fn layout_with_timings_and_metrics_and_shaper(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(LaidOutDocument<'a>, crate::LayoutTimings), crate::ShapeError> {
        let mut timings = crate::LayoutTimings::default();
        let document = self.layout_coordinated(
            constraints,
            image_metrics,
            shaper,
            Some(&mut timings),
            resources,
        )?;
        Ok((document, timings))
    }

    fn layout_coordinated(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
        mut timings: Option<&mut crate::LayoutTimings>,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, crate::ShapeError> {
        if self.shaped.resource_generation != resources.generation()
            || glyph_shaper.resource_generation() != resources.generation()
        {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        let candidate = LayoutCandidate::new(
            &self.inputs,
            &self.shaped,
            constraints,
            image_metrics,
            timings.as_deref_mut(),
            LayoutScratch::default(),
            resources,
        );

        let needs_append =
            !first_line_style_ranges(&self.inputs, &candidate.layout_state).is_empty();
        let refine = |glyph_shaper: &mut _, resources: &mut GlyphResourceStore| {
            refine_first_lines(
                candidate,
                &self.inputs,
                &self.shaped,
                constraints,
                image_metrics,
                glyph_shaper,
                timings,
                resources,
            )
        };
        let candidate = if needs_append {
            with_append_shaping(glyph_shaper, refine, resources)?
        } else {
            refine(glyph_shaper, resources)?
        };
        Ok(candidate.into_document(
            self.inputs,
            self.shaped,
            constraints,
            image_metrics,
            resources,
        ))
    }

    fn layout_with_metrics_and_shaper_inner(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, crate::ShapeError> {
        debug_assert_eq!(self.shaped.resource_generation, resources.generation());
        let candidate = LayoutCandidate::new(
            &self.inputs,
            &self.shaped,
            constraints,
            image_metrics,
            None,
            LayoutScratch::default(),
            resources,
        );

        let candidate = refine_first_lines(
            candidate,
            &self.inputs,
            &self.shaped,
            constraints,
            image_metrics,
            glyph_shaper,
            None,
            resources,
        )?;
        Ok(candidate.into_document(
            self.inputs,
            self.shaped,
            constraints,
            image_metrics,
            resources,
        ))
    }

    pub fn layout_with_timings_and_metrics(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) -> Result<(LaidOutDocument<'a>, crate::LayoutTimings), crate::ShapeError> {
        self.inputs.require_static_text()?;
        if self.shaped.resource_generation != resources.generation() {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        let mut timings = crate::LayoutTimings::default();
        let document = self.layout_impl(constraints, image_metrics, Some(&mut timings), resources);
        Ok((document, timings))
    }

    pub(super) fn layout_impl(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>,
        resources: &GlyphResourceStore,
    ) -> LaidOutDocument<'a> {
        self.layout_impl_with_scratch(
            constraints,
            image_metrics,
            timings,
            LayoutScratch::default(),
            resources,
        )
    }

    pub(super) fn layout_impl_with_scratch(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>,
        layout_scratch: LayoutScratch,
        resources: &GlyphResourceStore,
    ) -> LaidOutDocument<'a> {
        LayoutCandidate::new(
            &self.inputs,
            &self.shaped,
            constraints,
            image_metrics,
            timings,
            layout_scratch,
            resources,
        )
        .into_document(
            self.inputs,
            self.shaped,
            constraints,
            image_metrics,
            resources,
        )
    }
}

/// Candidate output owns only mutable layout state. Preparation and shaping
/// are borrowed for the operation, including each first-line refinement pass.
pub(crate) struct LayoutCandidate {
    geometry: BoxGeometry,
    placement: crate::layout::PlacementState,
    pub(super) refinement: Option<FirstLineRefinement>,
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
        resources: &GlyphResourceStore,
    ) -> Self {
        let mut candidate = Self {
            geometry: BoxGeometry::default(),
            placement: crate::layout::PlacementState::default(),
            refinement: None,
            layout_state: LayoutState::default(),
            layout_scratch,
        };
        candidate.relayout(inputs, base, constraints, image_metrics, timings, resources);
        candidate
    }

    pub(super) fn relayout(
        &mut self,
        inputs: &PreparedInputs,
        base: &ShapedText,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>,
        resources: &GlyphResourceStore,
    ) {
        crate::layout::layout_with_timings(
            layout_inputs(
                inputs,
                ShapedTextView::new(base, self.refinement.as_ref()),
                image_metrics,
                resources,
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

    fn into_document<'a>(
        self,
        inputs: Box<PreparedInputs<'a>>,
        base_shaped: Box<ShapedText>,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) -> LaidOutDocument<'a> {
        let mut document = LaidOutDocument {
            inputs,
            base_shaped,
            geometry: self.geometry,
            placement: self.placement,
            refinement: self.refinement,
            layout_state: self.layout_state,
            layout_scratch: self.layout_scratch,
            paint_colors: Vec::new(),
            last_constraints: constraints,
            last_image_metrics: image_metrics.clone(),
        };
        document.rebuild_anchor_positions();
        document.rebuild_paint_colors(resources);
        document
    }

    fn publish(
        self,
        document: &mut LaidOutDocument,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) {
        document.geometry = self.geometry;
        document.placement = self.placement;
        document.refinement = self.refinement;

        document.layout_state = self.layout_state;
        document.layout_scratch = self.layout_scratch;
        document.last_constraints = constraints;
        document.last_image_metrics = image_metrics.clone();
        document.rebuild_anchor_positions();
        document.rebuild_paint_colors(resources);
    }
}

fn layout_inputs<'a>(
    inputs: &'a PreparedInputs<'a>,
    shaped: ShapedTextView<'a>,
    image_metrics: &'a ImageMetrics,
    resources: &'a GlyphResourceStore,
) -> crate::layout::LayoutInputs<'a> {
    crate::layout::LayoutInputs {
        document: &inputs.document,
        styles: &inputs.styles,
        topology: &inputs.layout_tree,
        inline_content: inputs.text.content(),
        first_letter_styles: inputs.text.first_letter_styles(),
        glyphs: shaped.glyphs,
        glyph_metrics: resources.metrics(),
        font_metrics: &shaped.shared.font_metrics,
        text_geometry: Some(shaped.text_geometry),
        ellipsis_glyphs: &shaped.shared.ellipsis_glyphs,
        hyphen_glyphs: &shaped.shared.hyphen_glyphs,
        inline_plan_generation: shaped.inline_plan_generation,
        image_metrics,
        paint: inputs.paint.view(),
        text_style_ranges: shaped.paint_ranges,
        resource_generation: shaped.resource_generation,
    }
}

/// A fully shaped and laid-out document.
///
/// No public constructor exists: values can only be produced from a
/// [`ShapedDocument`]. Relayout mutates only layout-owned data, retaining the
/// immutable shaping inputs and reusing top-level layout buffers.
pub struct LaidOutDocument<'a> {
    pub(super) inputs: Box<PreparedInputs<'a>>,
    pub(super) geometry: BoxGeometry,
    /// Retained placement relationships. Publication updates the owning
    /// geometry and fragment coordinates in place.
    pub(super) placement: crate::layout::PlacementState,
    /// Viewport-independent shaping retained so a width change can recompute
    /// the dynamic extent of `::first-line` without accumulating old styles.
    pub(super) base_shaped: Box<ShapedText>,
    pub(super) refinement: Option<FirstLineRefinement>,
    pub(super) layout_state: LayoutState,
    pub(super) layout_scratch: LayoutScratch,
    pub(super) paint_colors: Vec<PaintColorRun>,
    pub(super) last_constraints: LayoutConstraints,
    pub(super) last_image_metrics: ImageMetrics,
}

#[derive(Clone, Copy)]
pub(super) struct InputRefs<'a> {
    pub(super) document: &'a Document,
    pub(super) styles: &'a ComputedStyles,
    pub(super) layout_tree: &'a LayoutTree,
    pub(super) text: &'a PreparedText,
    pub(super) paint: PaintView<'a>,
}

#[derive(Clone, Copy)]
pub(super) struct DocumentRead<'a> {
    pub(super) inputs: InputRefs<'a>,
    pub(super) geometry: &'a BoxGeometry,
    pub(super) base_shaped: &'a ShapedText,
    pub(super) refinement: Option<&'a FirstLineRefinement>,
    pub(super) layout_state: &'a LayoutState,
    pub(super) paint_colors: &'a [PaintColorRun],
}

impl PreparedInputs<'_> {
    fn refs(&self) -> InputRefs<'_> {
        InputRefs {
            document: &self.document,
            styles: &self.styles,
            layout_tree: &self.layout_tree,
            text: &self.text,
            paint: self.paint.view(),
        }
    }
}

/// Layout products awaiting publication by their preparation owner.
pub struct DetachedLayout {
    paint: Option<PaintPalette>,
    pub(super) geometry: BoxGeometry,
    /// Retained placement relationships. Publication updates the owning
    /// geometry and fragment coordinates in place.
    pub(super) placement: crate::layout::PlacementState,
    /// Viewport-independent shaping retained so a width change can recompute
    /// the dynamic extent of `::first-line` without accumulating old styles.
    pub(super) base_shaped: Box<ShapedText>,
    pub(super) refinement: Option<FirstLineRefinement>,
    pub(super) layout_state: LayoutState,
    pub(super) layout_scratch: LayoutScratch,
    pub(super) paint_colors: Vec<PaintColorRun>,
    pub(super) last_constraints: LayoutConstraints,
    pub(super) last_image_metrics: ImageMetrics,
}

/// Scoped layout products. DOM and styles remain in the parent document.
/// Querying requires that same preparation owner; replacing it invalidates the note.
pub struct NoteLayout {
    owner: PreparationGeneration,
    layout_tree: LayoutTree,
    text: PreparedText,
    paint: NotePaint,
    layout: DetachedLayout,
}

impl NoteLayout {
    pub fn render_view<'a>(
        &'a self,
        owner: &'a LaidOutDocument<'_>,
        resources: &'a GlyphResourceStore,
    ) -> Option<RenderView<'a>> {
        if self.layout.base_shaped.resource_generation != resources.generation()
            || self.owner != owner.inputs.generation
            || self.layout.base_shaped.resource_generation != owner.base_shaped.resource_generation
        {
            return None;
        }
        Some(RenderView {
            glyph_metrics: resources.metrics(),
            doc: DocumentRead {
                inputs: InputRefs {
                    document: &owner.inputs.document,
                    styles: &owner.inputs.styles,
                    layout_tree: &self.layout_tree,
                    text: &self.text,
                    paint: self.paint.view(&owner.inputs.paint)?,
                },
                geometry: &self.layout.geometry,
                base_shaped: &self.layout.base_shaped,
                refinement: self.layout.refinement.as_ref(),
                layout_state: &self.layout.layout_state,
                paint_colors: &self.layout.paint_colors,
            },
        })
    }
}

impl<'a> LaidOutDocument<'a> {
    /// Detach scoped products while retaining only the parent's preparation identity.
    pub fn into_note(self, owner: &LaidOutDocument<'_>) -> NoteLayout {
        assert!(std::ptr::eq(
            self.inputs.document.as_ref(),
            owner.inputs.document.as_ref()
        ));
        assert!(std::ptr::eq(
            self.inputs.styles.as_ref(),
            owner.inputs.styles.as_ref()
        ));
        NoteLayout {
            owner: owner.inputs.generation,
            layout_tree: self
                .inputs
                .layout_tree
                .into_owned()
                .expect("scoped tree has one owner"),
            text: self
                .inputs
                .text
                .into_owned()
                .expect("scoped text has one owner"),
            paint: self
                .inputs
                .paint
                .into_owned()
                .expect("scoped paint has one owner")
                .into_note_paint(),
            layout: DetachedLayout {
                paint: None,
                geometry: self.geometry,
                placement: self.placement,
                base_shaped: self.base_shaped,
                refinement: self.refinement,
                layout_state: self.layout_state,
                layout_scratch: self.layout_scratch,
                paint_colors: self.paint_colors,
                last_constraints: self.last_constraints,
                last_image_metrics: self.last_image_metrics,
            },
        }
    }

    fn read(&self) -> DocumentRead<'_> {
        DocumentRead {
            inputs: self.inputs.refs(),
            geometry: &self.geometry,
            base_shaped: &self.base_shaped,
            refinement: self.refinement.as_ref(),
            layout_state: &self.layout_state,
            paint_colors: &self.paint_colors,
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn shape_borrowed(
        &self,
        shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<ShapedDocument<'_>, crate::ShapeError> {
        shaper.begin_document_shaping();
        let previous_resources = std::mem::replace(
            resources,
            GlyphResourceStore::new(shaper.resource_generation()),
        );
        let result = shape_text(&self.inputs, shaper, resources);
        match result {
            Ok(shaped) => {
                shaper.commit_document_shaping();
                Ok(ShapedDocument {
                    inputs: Box::new(self.inputs.borrowed()),
                    shaped,
                })
            }
            Err(error) => {
                shaper.rollback_document_shaping();
                *resources = previous_resources;
                Err(error)
            }
        }
    }

    pub fn into_detached(self) -> DetachedLayout {
        DetachedLayout {
            paint: self
                .inputs
                .paint
                .into_owned()
                .map(PaintStyles::into_palette),
            geometry: self.geometry,
            placement: self.placement,
            base_shaped: self.base_shaped,
            refinement: self.refinement,
            layout_state: self.layout_state,
            layout_scratch: self.layout_scratch,
            paint_colors: self.paint_colors,
            last_constraints: self.last_constraints,
            last_image_metrics: self.last_image_metrics,
        }
    }

    /// Move the retained shape out when starting an independent layout pass.
    /// Glyph storage keeps its address; prior layout buffers are released.
    pub fn into_shaped(self) -> ShapedDocument<'a> {
        ShapedDocument {
            inputs: self.inputs,
            shaped: self.base_shaped,
        }
    }

    /// Borrow source text from this document's sole preparation owner.
    pub fn visible_text_runs(&self) -> impl Iterator<Item = (&str, Option<&ContentLanguage>)> {
        visible_text_runs(&self.inputs)
    }

    pub fn note_ids(&self) -> Vec<&str> {
        note_ids(&self.inputs)
    }

    /// Explicitly detached note documents share only immutable DOM/styles.
    pub fn scoped_to_element_id(&self, id: &str) -> Option<PreparedDocument<'_>> {
        scoped_to_element_id(&self.inputs, id)
    }

    /// Replace backend shaping while retaining preparation. Candidate layout
    /// borrows both inputs and the new shape; publication moves the shape once.
    pub fn reshape_and_layout_with_metrics_and_shaper(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        let (shaped, candidate) = shape_and_layout_candidate(
            &self.inputs,
            constraints,
            image_metrics,
            glyph_shaper,
            resources,
        )?;
        self.base_shaped = shaped;
        candidate.publish(self, constraints, image_metrics, resources);
        Ok(())
    }

    /// Update a palette without manufacturing another prepared-stage owner.
    pub fn repaint_with_reader_overrides(
        &mut self,
        overrides: &ReaderStyleOverrides,
        resources: &GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        let paint = self
            .inputs
            .paint
            .with_overrides(&self.inputs.styles, &self.inputs.layout_tree, overrides)
            .into_palette();
        self.inputs.paint.owned_mut()?.replace_palette(paint);
        self.rebuild_paint_colors(resources);
        html_layout::repaint(
            layout_inputs(
                &self.inputs,
                ShapedTextView::new(&self.base_shaped, self.refinement.as_ref()),
                &self.last_image_metrics,
                resources,
            ),
            &mut self.geometry,
            &mut self.layout_state,
            self.last_constraints,
        );
        Ok(())
    }

    /// Stage a palette together with fallible shaping/layout and restore it on
    /// failure. The previous published geometry remains intact until commit.
    pub fn update_with_reader_overrides(
        &mut self,
        overrides: &ReaderStyleOverrides,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        reshape: bool,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        let paint = self
            .inputs
            .paint
            .with_overrides(&self.inputs.styles, &self.inputs.layout_tree, overrides)
            .into_palette();
        let owner = self.inputs.as_mut();
        let previous = owner.paint.owned_mut()?.replace_palette(paint);
        let result = if reshape {
            self.reshape_and_layout_with_metrics_and_shaper(
                constraints,
                image_metrics,
                glyph_shaper,
                resources,
            )
        } else {
            self.relayout_with_metrics_and_shaper(
                constraints,
                image_metrics,
                glyph_shaper,
                resources,
            )
        };
        if result.is_err() {
            self.inputs.paint.owned_mut()?.replace_palette(previous);
        }
        result
    }

    pub fn reprepare_and_layout(
        &mut self,
        root_font_size: RootFontSize,
        note_flow: NoteFlow,
        overrides: &ReaderStyleOverrides,
        resolve: impl FnOnce(&Document) -> ComputedStyles,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        let (shaped, candidate) = with_reprepared(
            self.inputs.as_mut(),
            root_font_size,
            note_flow,
            overrides,
            resolve,
            |inputs| {
                shape_and_layout_candidate(
                    inputs,
                    constraints,
                    image_metrics,
                    glyph_shaper,
                    resources,
                )
            },
        )?;
        self.base_shaped = shaped;
        candidate.publish(self, constraints, image_metrics, resources);
        Ok(())
    }

    pub(super) fn shaped(&self) -> ShapedTextView<'_> {
        self.read().shaped()
    }
    /// Resolve paint over retained geometry, including specialized formatting
    /// contexts. A different prepared geometry epoch must be laid out first.
    pub fn repaint_from(
        &mut self,
        prepared: &'a PreparedDocument,
        resources: &GlyphResourceStore,
    ) -> bool {
        if self.base_shaped.preparation != prepared.inputs.generation {
            return false;
        }
        self.inputs = Box::new(prepared.inputs.borrowed());
        self.rebuild_paint_colors(resources);
        html_layout::repaint(
            layout_inputs(
                &self.inputs,
                ShapedTextView::new(&self.base_shaped, self.refinement.as_ref()),
                &self.last_image_metrics,
                resources,
            ),
            &mut self.geometry,
            &mut self.layout_state,
            self.last_constraints,
        );
        true
    }

    /// Refresh specialized decoration geometry while retaining shaped text.
    /// The paint variant must come from this document's prepared publication.
    pub fn relayout_with_paint_from(
        &mut self,
        prepared: &'a PreparedDocument,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        let image_metrics = self.last_image_metrics.clone();
        self.relayout_from(
            prepared,
            self.last_constraints,
            &image_metrics,
            glyph_shaper,
            resources,
        )
    }

    /// Update geometry and paint together while retaining shaping. A failed
    /// exact-line refinement leaves both the previous geometry and palette.
    pub fn relayout_from(
        &mut self,
        prepared: &'a PreparedDocument,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        assert_eq!(self.base_shaped.preparation, prepared.inputs.generation);
        let previous_inputs =
            std::mem::replace(&mut self.inputs, Box::new(prepared.inputs.borrowed()));
        let result = self.relayout_with_metrics_and_shaper(
            constraints,
            image_metrics,
            glyph_shaper,
            resources,
        );
        if result.is_err() {
            self.inputs = previous_inputs;
        }
        result
    }

    pub(super) fn rebuild_paint_colors(&mut self, resources: &GlyphResourceStore) {
        for line in &self.layout_state.line_output.lines {
            let Some(detail_idx) = line.details else {
                continue;
            };
            let details =
                &mut self.layout_state.line_output.line_details[detail_idx.get() as usize - 1];
            if details.paint_color.is_some() {
                let root = html_text::whitespace_context_root(
                    self.inputs.layout_tree.as_ref(),
                    line.owner_box_idx as usize,
                );
                details.paint_color = self.inputs.text.content().pseudo_styles().first_line(root)
                .map(|indices| {
                    self.inputs
                        .paint
                        .resolve(&self.inputs.styles, indices, root)
                        .foreground
                });
            }
        }

        let mut colors: Vec<PaintColorRun> = Vec::new();
        for item in self.inputs.text.content().text_runs() {
            let glyphs = item.glyphs;
            let base = self
                .inputs
                .layout_tree
                .get_box_style_indices(item.box_idx as usize)
                .unwrap_or_else(|| self.inputs.styles.default_indices());
            let mut start = glyphs.start;
            while start < glyphs.end {
                let (override_style, mut end) = self
                    .inputs
                    .text
                    .first_letter_styles()
                    .segment(start, glyphs.end);
                let index = self
                    .shaped()
                    .paint_ranges
                    .partition_point(|range| range.glyphs.end <= start);
                let mut style = override_style.unwrap_or(base);
                if let Some(range) = self.shaped().paint_ranges.get(index) {
                    if range.glyphs.start <= start {
                        style = range.style;
                        end = end.min(range.glyphs.end);
                    } else {
                        end = end.min(range.glyphs.start);
                    }
                }
                let color = self
                    .inputs
                    .paint
                    .box_colors(item.box_idx as usize)
                    .filter(|_| style == base)
                    .map(|colors| colors.foreground)
                    .unwrap_or_else(|| {
                        self.inputs
                            .paint
                            .resolve(&self.inputs.styles, style, item.box_idx as usize)
                            .foreground
                    });
                if let Some(last) = colors
                    .last_mut()
                    .filter(|last| last.glyphs.end == start && last.color == color)
                {
                    last.glyphs.end = end;
                } else {
                    colors.push(PaintColorRun {
                        glyphs: start..end,
                        color,
                    });
                }
                start = end;
            }
        }
        self.paint_colors = colors;
        self.prepare_text_paint(resources);
    }

    fn prepare_text_paint(&mut self, resources: &GlyphResourceStore) {
        // Geometry finalization leaves native fragments unprepared. Resolve
        // the palette first, then build the final colored slices once.
        prepare_text_paint(
            layout_inputs(
                &self.inputs,
                ShapedTextView::new(&self.base_shaped, self.refinement.as_ref()),
                &self.last_image_metrics,
                resources,
            ),
            &mut self.layout_state,
            // Native runs retain authored colors; only palette overrides replace them.
            if self.inputs.paint.is_default() {
                &[]
            } else {
                &self.paint_colors
            },
        );
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
    /// use html_pipeline::engine::{LaidOutDocument, LayoutConstraints};
    /// fn resize(document: &mut LaidOutDocument, constraints: LayoutConstraints) {
    ///     let view = document.render_view();
    ///     document.relayout(constraints);
    ///     let _ = view.text().line_count();
    /// }
    /// ```
    pub fn render_view<'r>(&'r self, resources: &'r GlyphResourceStore) -> RenderView<'r> {
        self.try_render_view(resources)
            .expect("document and glyph resources must belong to the same generation")
    }

    pub fn try_render_view<'r>(
        &'r self,
        resources: &'r GlyphResourceStore,
    ) -> Option<RenderView<'r>> {
        (self.base_shaped.resource_generation == resources.generation()).then(|| RenderView {
            doc: self.read(),
            glyph_metrics: resources.metrics(),
        })
    }

    #[cfg(test)]
    pub(super) fn text_geometry(&self) -> crate::shaping::TextGeometryView<'_> {
        self.read().text_geometry()
    }

    pub fn relayout(
        &mut self,
        constraints: LayoutConstraints,
        resources: &GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        self.inputs.require_static_text()?;
        if self.base_shaped.resource_generation != resources.generation() {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        let image_metrics = ImageMetrics::from_document(self.document());
        self.relayout_impl(constraints, &image_metrics, None, resources);
        self.last_constraints = constraints;
        self.last_image_metrics = image_metrics;
        Ok(())
    }

    pub fn relayout_with_timings(
        &mut self,
        constraints: LayoutConstraints,
        resources: &GlyphResourceStore,
    ) -> Result<crate::LayoutTimings, crate::ShapeError> {
        self.inputs.require_static_text()?;
        if self.base_shaped.resource_generation != resources.generation() {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        let mut timings = crate::LayoutTimings::default();
        let image_metrics = ImageMetrics::from_document(self.document());
        self.relayout_impl(constraints, &image_metrics, Some(&mut timings), resources);
        self.last_constraints = constraints;
        self.last_image_metrics = image_metrics;
        Ok(timings)
    }

    pub fn relayout_with_metrics(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        self.inputs.require_static_text()?;
        if self.base_shaped.resource_generation != resources.generation() {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        self.relayout_impl(constraints, image_metrics, None, resources);
        self.last_constraints = constraints;
        self.last_image_metrics = image_metrics.clone();
        Ok(())
    }

    /// Resize through the coordinator, including first-line refinement.
    pub fn relayout_with_shaper(
        &mut self,
        constraints: LayoutConstraints,
        shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        let metrics = ImageMetrics::from_document(self.document());
        self.relayout_with_metrics_and_shaper(constraints, &metrics, shaper, resources)
    }

    pub fn relayout_with_metrics_and_shaper(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl crate::GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), crate::ShapeError> {
        if self.base_shaped.resource_generation != resources.generation()
            || glyph_shaper.resource_generation() != resources.generation()
        {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        if !self.inputs.styles.has_first_line_styles() {
            // Ordinary resizing cannot fail during shaping. Retain geometry,
            // line output, placement and scratch storage instead of allocating
            // a private refinement candidate for every viewport change.
            self.relayout_with_metrics(constraints, image_metrics, resources)?;
            return Ok(());
        }
        let candidate = LayoutCandidate::new(
            &self.inputs,
            &self.base_shaped,
            constraints,
            image_metrics,
            None,
            std::mem::take(&mut self.layout_scratch),
            resources,
        );

        let needs_append =
            !first_line_style_ranges(&self.inputs, &candidate.layout_state).is_empty();
        let refine = |glyph_shaper: &mut _, resources: &mut GlyphResourceStore| {
            refine_first_lines(
                candidate,
                &self.inputs,
                &self.base_shaped,
                constraints,
                image_metrics,
                glyph_shaper,
                None,
                resources,
            )
        };
        let candidate = if needs_append {
            with_append_shaping(glyph_shaper, refine, resources)?
        } else {
            refine(glyph_shaper, resources)?
        };
        candidate.publish(self, constraints, image_metrics, resources);
        Ok(())
    }

    pub fn relayout_with_metrics_and_timings(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) -> Result<crate::LayoutTimings, crate::ShapeError> {
        self.inputs.require_static_text()?;
        if self.base_shaped.resource_generation != resources.generation() {
            return Err(crate::ShapeError::resource_generation_mismatch());
        }
        let mut timings = crate::LayoutTimings::default();
        self.relayout_impl(constraints, image_metrics, Some(&mut timings), resources);
        self.last_constraints = constraints;
        self.last_image_metrics = image_metrics.clone();
        Ok(timings)
    }

    pub(super) fn relayout_impl(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut crate::LayoutTimings>,
        resources: &GlyphResourceStore,
    ) {
        crate::layout::layout_with_timings(
            layout_inputs(
                &self.inputs,
                ShapedTextView::new(&self.base_shaped, self.refinement.as_ref()),
                image_metrics,
                resources,
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
        self.rebuild_anchor_positions();
        self.rebuild_paint_colors(resources);
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

    pub fn document(&self) -> &Document {
        self.read().document()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn glyph_metrics<'r>(&self, resources: &'r GlyphResourceStore) -> &'r GlyphMetrics {
        assert_eq!(self.base_shaped.resource_generation, resources.generation());
        resources.metrics()
    }

    pub fn anchor_glyph(&self, id_idx: u16) -> Option<u32> {
        self.read().anchor_glyph(id_idx)
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn glyphs(&self) -> crate::shaping::GlyphSlice<'_> {
        self.read().glyphs()
    }

    /// Returns the DOM text node and UTF-16 source offset for the glyph.

    #[cfg(any(test, feature = "testing"))]
    pub fn glyph_source_offset(&self, glyph_idx: u32) -> Option<u32> {
        self.read().glyph_source_offset(glyph_idx)
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn box_layout_mode(&self, box_idx: usize) -> Option<&LayoutMode> {
        self.read().box_layout_mode(box_idx)
    }

    pub fn box_used_style(&self, box_idx: usize) -> Option<html_style_model::UsedStyleView<'_>> {
        self.read().box_used_style(box_idx)
    }

    pub fn box_count(&self) -> usize {
        self.read().box_count()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn line_count(&self) -> usize {
        self.layout_state.line_output.lines.len()
    }

    /// Report document allocations together with its borrowed glyph ledger,
    /// counting shared shaping advances once.
    pub fn memory_usage_report_with_resources(
        &self,
        resources: &GlyphResourceStore,
    ) -> MemoryUsageReport {
        assert_eq!(self.base_shaped.resource_generation, resources.generation());
        let mut seen = FxHashSet::default();
        let mut report = self.memory_usage_report_with_shared(&mut seen);
        report.extend_prefixed(
            "GlyphResourceStore",
            resources.memory_usage_report_with_shared(&mut seen),
        );
        report
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        self.memory_usage_report_with_shared(&mut FxHashSet::default())
    }

    fn memory_usage_report_with_shared(
        &self,
        seen: &mut FxHashSet<*const ()>,
    ) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.extend_prefixed(
            "LaidOutDocument.paint",
            self.inputs.paint.memory_usage_report(),
        );
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
        report.extend_prefixed(
            "LaidOutDocument",
            self.base_shaped.memory_usage_report(seen),
        );
        if let Some(refinement) = &self.refinement {
            report.extend_prefixed(
                "LaidOutDocument.refinement",
                refinement.memory_usage_report(seen),
            );
        }
        report.add_slice_storage::<PaintColorRun>(
            "LaidOutDocument.paint_colors.storage",
            self.paint_colors.capacity(),
            self.paint_colors.len(),
        );
        report.extend_prefixed(
            "LaidOutDocument.paragraph_layouts",
            self.layout_scratch.paragraph_layout_memory_report(),
        );
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

fn visible_text_runs<'a>(
    inputs: &'a PreparedInputs<'_>,
) -> impl Iterator<Item = (&'a str, Option<&'a ContentLanguage>)> {
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
                    let language = style
                        .language
                        .and_then(|language| styles.language(language));
                    return Some((text.text(), language));
                }
                None => {}
            }
        }
        None
    })
}

fn scoped_to_element_id<'a>(
    inputs: &'a PreparedInputs<'_>,
    id: &str,
) -> Option<PreparedDocument<'a>> {
    let document = &inputs.document;
    let root = document
        .node_ids()
        .find(|node| document.get_dom_id(*node) == Some(id))?;
    document.element_ref(root)?;

    let mut layout_tree = LayoutTree::default();
    let mut inline_content = InlineContent::default();
    html_format::build_layout_inputs_from(
        document,
        &inputs.styles,
        &mut layout_tree,
        &mut inline_content,
        NoteFlow::InFlow,
        root,
    );
    let text = Storage::owned(PreparedText::new(
        document,
        &inputs.styles,
        &layout_tree,
        inline_content,
    ));
    let paint = Storage::owned(
        inputs
            .paint
            .for_tree(document, &inputs.styles, &layout_tree),
    );
    Some(PreparedDocument {
        inputs: Box::new(PreparedInputs {
            generation: PreparationGeneration::new(),
            document: Storage::Borrowed(&inputs.document),
            styles: Storage::Borrowed(&inputs.styles),
            layout_tree: Storage::owned(layout_tree),
            text,
            note_flow: NoteFlow::InFlow,
            paint,
        }),
    })
}

fn note_ids<'a>(inputs: &'a PreparedInputs<'_>) -> Vec<&'a str> {
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

impl<'a> DocumentRead<'a> {
    pub(super) fn render_view(self, glyph_metrics: &'a GlyphMetrics) -> RenderView<'a> {
        RenderView {
            doc: self,
            glyph_metrics,
        }
    }
    pub fn document(&self) -> &'a Document {
        self.inputs.document
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

    pub fn anchor_glyph(&self, id_idx: u16) -> Option<u32> {
        self.base_shaped.shared.anchor_glyphs.get(&id_idx).copied()
    }

    pub(crate) fn anchor_glyphs(&self) -> &'a FxHashMap<u16, u32> {
        &self.base_shaped.shared.anchor_glyphs
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn glyphs(&self) -> crate::shaping::GlyphSlice<'a> {
        self.shaped().glyphs
    }

    pub(crate) fn get_dom_node_for_glyph(&self, glyph_idx: u32) -> Option<(u32, usize)> {
        for run in self.inputs.text.content().inline_items() {
            if let InlineItemKind::Text { glyphs } = &run.kind
                && glyphs.contains(&glyph_idx)
            {
                let node_idx = run.dom_text_node?;
                let source_offset =
                    self.inputs
                        .text
                        .content()
                        .glyph_source_offset(glyph_idx as usize)? as usize;
                return Some((node_idx, source_offset));
            }
        }
        None
    }

    pub(crate) fn glyph_source_offset(&self, glyph_idx: u32) -> Option<u32> {
        self.inputs
            .text
            .content()
            .glyph_source_offset(glyph_idx as usize)
    }

    pub(crate) fn box_dom_element_idx(&self, box_idx: usize) -> Option<u32> {
        self.inputs.layout_tree.box_at(box_idx)?.dom_element()
    }

    pub(crate) fn box_layout_mode(&self, box_idx: usize) -> Option<&'a LayoutMode> {
        self.inputs
            .layout_tree
            .box_at(box_idx)
            .map(|layout_box| layout_box.layout_mode())
    }

    pub fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.inputs.layout_tree.box_at(box_idx)?.style()
    }

    pub fn box_used_style(&self, box_idx: usize) -> Option<html_style_model::UsedStyleView<'a>> {
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
            color: self
                .inputs
                .paint
                .box_colors(box_idx)
                .map_or_else(|| style.color(), |paint| paint.foreground),
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

    pub fn box_count(&self) -> usize {
        self.inputs.layout_tree.box_count()
    }

    pub(crate) fn is_block_container_box(&self, box_idx: usize) -> bool {
        matches!(
            self.box_layout_mode(box_idx),
            Some(LayoutMode::Block(_) | LayoutMode::Table(_) | LayoutMode::TableCell(_))
        )
    }

    pub(super) fn shaped(&self) -> ShapedTextView<'a> {
        ShapedTextView::new(&self.base_shaped, self.refinement)
    }

    pub(super) fn text_geometry(&self) -> crate::shaping::TextGeometryView<'a> {
        self.shaped().text_geometry
    }
}
