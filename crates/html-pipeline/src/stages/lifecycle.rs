//! Prepared, shaped, and laid-out document lifecycle types.

use html_dom as dom_values;
use html_dom::storage;
use super::mathml::{self, MathMlUndo};
use super::output::{WordIndex, build_word_index};
use super::paint::{NotePaint, PaintPalette, PaintStyles};
use super::semantics::is_block_container;
use super::*;
use crate::layout::{InlinePlanGeneration, LayoutScratch, PlacementState, prepare_text_paint};
use crate::{GlyphShaper, LayoutTimings, ReaderStyleOverrides, ShapeError};
use html_dom::{ElementRef, NodeRef, RootFontSize};
use html_engine_model::shaping::{
    borrowed_document, first_line_shaper_required, resource_generation_mismatch,
};
use html_engine_model::{GlyphResourceCheckpoint, PaintColorRun, PaintView, TextPaintRange};
use html_style_model::{
    ContentLanguage, Display, FontFace, ObjectFit, UsedPreferredSize, Visibility,
};
use html_text::PreparedText;
use std::iter::once;
use std::mem::{replace, take};
use std::sync::Arc;
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
    fn owned_mut(&mut self) -> Result<&mut T, ShapeError> {
        match self {
            Self::Owned(value) => Ok(value),
            Self::Borrowed(_) => Err(borrowed_document()),
        }
    }
    pub(super) fn replace_owned(&mut self, value: T) -> Option<T> {
        match self {
            Self::Owned(current) => Some(replace(current.as_mut(), value)),
            Self::Borrowed(_) => replace(self, Self::owned(value)).into_owned(),
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
    publication_fonts: PublicationFonts,
}

/// A document's publication `@font-face` declarations and the reader font
/// policy it was styled with. Every shaping pass of the document hands them to
/// the backend, so its active faces always match the text being shaped.
#[derive(Clone)]
pub(crate) struct PublicationFonts {
    faces: Arc<[FontFace]>,
    overrides: ReaderStyleOverrides,
}

impl Default for PublicationFonts {
    fn default() -> Self {
        Self::new(Arc::from(Vec::new()), ReaderStyleOverrides::default())
    }
}

impl PublicationFonts {
    pub(crate) fn new(faces: Arc<[FontFace]>, overrides: ReaderStyleOverrides) -> Self {
        Self { faces, overrides }
    }

    fn select(&self, glyph_shaper: &mut impl GlyphShaper) {
        glyph_shaper.select_publication_fonts(&self.faces, &self.overrides);
    }
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
            mathml::prepare(&mut document, &styles)
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
            publication_fonts: PublicationFonts::default(),
        })
    }

    /// Attach the publication font declarations and reader font policy that
    /// produced these styles. Documents without them shape with system fonts.
    pub fn with_publication_fonts(
        mut self,
        faces: Arc<[FontFace]>,
        overrides: ReaderStyleOverrides,
    ) -> Self {
        self.publication_fonts = PublicationFonts::new(faces, overrides);
        self
    }

    /// Run html-text after formatting boundaries are known and before shaping.
    pub fn prepare_text(self) -> PreparedDocument<'static> {
        let Self {
            document,
            styles,
            layout_tree,
            inline_content,
            note_flow,
            publication_fonts,
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
                publication_fonts: Storage::owned(publication_fonts),
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
    pub(super) publication_fonts: Storage<'a, PublicationFonts>,
}

impl PreparedInputs<'_> {
    fn shape_active_document(
        &self,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<ShapedDocument<'_>, ShapeError> {
        shape_text(self, glyph_shaper, resources).map(|shaped| ShapedDocument {
            inputs: Box::new(self.borrowed()),
            shaped,
        })
    }

    fn report_structure_memory(&self, report: &mut MemoryUsageReport, prefix: &str) {
        for (suffix, usage) in [
            (".document", self.document.memory_usage_report()),
            (".styles", self.styles.memory_usage_report()),
            (".layout_tree", self.layout_tree.memory_usage_report()),
        ] {
            report.extend_prefixed([prefix, suffix].concat(), usage);
        }
    }

    fn replace_paint_overrides(
        &mut self,
        overrides: &ReaderStyleOverrides,
    ) -> Result<PaintPalette, ShapeError> {
        let paint = self
            .paint
            .with_overrides(&self.styles, &self.layout_tree, overrides)
            .into_palette();
        Ok(self.paint.owned_mut()?.replace_palette(paint))
    }

    fn require_static_text(&self) -> Result<(), ShapeError> {
        if self.styles.has_first_line_styles() {
            return Err(first_line_shaper_required());
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
            publication_fonts: Storage::Borrowed(&self.publication_fonts),
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
    pub(super) words: WordIndex,
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
        report.add_vec_storage("glyph_patches", &self.glyph_patches);
        for patch in &self.glyph_patches {
            report.add_vec_storage("glyphs.storage", &patch.glyphs);
        }
        report.add_vec_storage("text_paint_ranges", &self.paint_ranges);
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
        report.add_vec_storage("glyphs.storage", &self.glyphs);
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
            report.add_hash_map_storage("ellipsis_glyphs.storage", &self.shared.ellipsis_glyphs);
            report.add_hash_map_storage("hyphen_glyphs.storage", &self.shared.hyphen_glyphs);
            report.add_hash_map_storage(
                "link_glyph_targets.storage",
                &self.shared.link_glyph_targets,
            );
            report.add_hash_map_storage("anchor_glyphs.storage", &self.shared.anchor_glyphs);
            report.extend_prefixed("words", self.shared.words.memory_usage_report());
        }
        report
    }
}

/// Retains the published ledger until native document replacement commits.
/// Unfinished shaping restores native resources and metrics when dropped.
struct DocumentShapingTransaction<'a, S: GlyphShaper> {
    glyph_shaper: &'a mut S,
    resources: &'a mut GlyphResourceStore,
    previous_resources: Option<GlyphResourceStore>,
}

impl<'a, S: GlyphShaper> DocumentShapingTransaction<'a, S> {
    fn begin(glyph_shaper: &'a mut S, resources: &'a mut GlyphResourceStore) -> Self {
        glyph_shaper.begin_document_shaping();
        let previous_resources = replace(
            resources,
            GlyphResourceStore::new(glyph_shaper.resource_generation()),
        );
        Self {
            glyph_shaper,
            resources,
            previous_resources: Some(previous_resources),
        }
    }

    fn commit(mut self) {
        self.glyph_shaper.commit_document_shaping();
        self.previous_resources = None;
    }
}

impl<S: GlyphShaper> Drop for DocumentShapingTransaction<'_, S> {
    fn drop(&mut self) {
        if let Some(previous_resources) = self.previous_resources.take() {
            self.glyph_shaper.rollback_document_shaping();
            *self.resources = previous_resources;
        }
    }
}

/// An append transaction owns only its prefix checkpoint, without cloning data.
struct AppendShapingTransaction<'a, S: GlyphShaper> {
    glyph_shaper: &'a mut S,
    resources: &'a mut GlyphResourceStore,
    checkpoint: Option<GlyphResourceCheckpoint>,
}

impl<'a, S: GlyphShaper> AppendShapingTransaction<'a, S> {
    fn begin(
        glyph_shaper: &'a mut S,
        resources: &'a mut GlyphResourceStore,
    ) -> Result<Self, ShapeError> {
        if glyph_shaper.resource_generation() != resources.generation() {
            return Err(resource_generation_mismatch());
        }
        glyph_shaper.begin_append_shaping()?;
        let checkpoint = resources.checkpoint();
        Ok(Self {
            glyph_shaper,
            resources,
            checkpoint: Some(checkpoint),
        })
    }

    fn commit(mut self) {
        self.glyph_shaper.commit_append_shaping();
        self.checkpoint = None;
    }
}

impl<S: GlyphShaper> Drop for AppendShapingTransaction<'_, S> {
    fn drop(&mut self) {
        if let Some(checkpoint) = self.checkpoint.take() {
            self.glyph_shaper.rollback_append_shaping();
            assert!(self.resources.rollback_to(checkpoint));
        }
    }
}

fn with_document_shaping<S: GlyphShaper, T>(
    glyph_shaper: &mut S,
    fonts: &PublicationFonts,
    work: impl FnOnce(&mut S, &mut GlyphResourceStore) -> Result<T, ShapeError>,
    resources: &mut GlyphResourceStore,
) -> Result<T, ShapeError> {
    fonts.select(glyph_shaper);
    let transaction = DocumentShapingTransaction::begin(glyph_shaper, resources);
    let value = work(&mut *transaction.glyph_shaper, &mut *transaction.resources)?;
    transaction.commit();
    Ok(value)
}

pub(super) fn with_append_shaping<S: GlyphShaper, T>(
    glyph_shaper: &mut S,
    fonts: &PublicationFonts,
    work: impl FnOnce(&mut S, &mut GlyphResourceStore) -> Result<T, ShapeError>,
    resources: &mut GlyphResourceStore,
) -> Result<T, ShapeError> {
    fonts.select(glyph_shaper);
    let transaction = AppendShapingTransaction::begin(glyph_shaper, resources)?;
    let value = work(&mut *transaction.glyph_shaper, &mut *transaction.resources)?;
    transaction.commit();
    Ok(value)
}

/// Wall time spent in the two renderer-resource phases of an explicitly timed
/// shape-and-layout operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct ShapeLayoutTimings {
    pub shape: Duration,
    pub layout: Duration,
}

/// One coordinated layout request. Metrics and timing collection are optional;
/// the coordinator handles first-line refinement and resource validity.
pub struct LayoutRequest<'a> {
    constraints: LayoutConstraints,
    metrics: Option<&'a ImageMetrics>,
    timings: Option<&'a mut LayoutTimings>,
}

impl<'a> LayoutRequest<'a> {
    #[inline]
    pub fn new(constraints: LayoutConstraints) -> Self {
        Self {
            constraints,
            metrics: None,
            timings: None,
        }
    }
    #[inline]
    pub fn with_image_metrics(mut self, metrics: &'a ImageMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }
    pub fn with_timings(mut self, timings: &'a mut LayoutTimings) -> Self {
        self.timings = Some(timings);
        self
    }
}

/// Only the canonical DOM needs rollback while replacement preparation is private.
struct DocumentPreparation<'a> {
    document: &'a mut Document,
    previous_font: Option<RootFontSize>,
    math_undo: Vec<MathMlUndo>,
}

impl DocumentPreparation<'_> {
    fn commit(mut self) {
        self.previous_font = None;
    }
}

impl Drop for DocumentPreparation<'_> {
    fn drop(&mut self) {
        if let Some(font) = self.previous_font {
            self.document.set_root_font_size(font);
            mathml::restore(self.document, take(&mut self.math_undo));
        }
    }
}

/// Build replacement preparation privately, borrowing the canonical DOM.
/// Publish derived inputs only after the complete shape/layout transaction succeeds.
fn reprepare_and_layout_candidate<'a>(
    inputs: &mut PreparedInputs<'a>,
    root_font_size: RootFontSize,
    note_flow: NoteFlow,
    overrides: &ReaderStyleOverrides,
    resolve: impl FnOnce(&Document) -> (ComputedStyles, Arc<[FontFace]>),
    constraints: LayoutConstraints,
    image_metrics: &ImageMetrics,
    glyph_shaper: &mut impl GlyphShaper,
    resources: &mut GlyphResourceStore,
) -> Result<(Box<ShapedText>, LayoutProducts), ShapeError> {
    // Exhaustive destructuring makes additions to preparation explicit here.
    let PreparedInputs {
        generation,
        document,
        styles: published_styles,
        layout_tree,
        text: published_text,
        note_flow: published_flow,
        paint: published_paint,
        publication_fonts: published_fonts,
    } = inputs;
    let document = document.owned_mut()?;
    let old_font = dom_values::root_font_size_new(document.root_font_size()).expect("validated root font size");
    let mut update = DocumentPreparation {
        document,
        previous_font: Some(old_font),
        math_undo: Vec::new(),
    };
    update.document.set_root_font_size(root_font_size);
    let (styles, faces) = resolve(update.document);
    let fonts = PublicationFonts::new(faces, overrides.clone());
    styles
        .validate_for(update.document)
        .expect("resolver styles must belong to the canonical DOM");
    update.math_undo = mathml::prepare_with_undo(update.document, &styles);
    let mut tree = LayoutTree::default();
    let mut content = InlineContent::default();
    html_format::build_layout_inputs(update.document, &styles, &mut tree, &mut content, note_flow);
    let text = prepare_logical_text(update.document, &styles, &tree, content);
    let mut paint: PaintStyles<'a> = PaintStyles::new(update.document, &styles, &tree);
    let palette = paint
        .with_overrides(&styles, &tree, overrides)
        .into_palette();
    paint.replace_palette(palette);
    let next_generation = PreparationGeneration::new();
    let candidate = PreparedInputs {
        generation: next_generation,
        document: Storage::Borrowed(update.document),
        styles: Storage::Borrowed(&styles),
        layout_tree: Storage::Borrowed(&tree),
        text: Storage::Borrowed(&text),
        note_flow,
        paint: Storage::Borrowed(&paint),
        publication_fonts: Storage::Borrowed(&fonts),
    };
    let result = shape_and_layout_candidate(
        &candidate,
        constraints,
        image_metrics,
        glyph_shaper,
        resources,
    )?;
    update.commit();
    *generation = next_generation;
    *published_styles = Storage::owned(styles);
    *layout_tree = Storage::owned(tree);
    *published_text = Storage::owned(text);
    *published_paint = Storage::owned(paint);
    *published_fonts = Storage::owned(fonts);
    *published_flow = note_flow;
    Ok(result)
}

impl<'a> PreparedDocument<'a> {
    pub fn shape_and_layout(
        self,
        constraints: LayoutConstraints,
        shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, ShapeError> {
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
        resolve: impl FnOnce(&Document) -> (ComputedStyles, Arc<[FontFace]>),
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'_>, ShapeError> {
        let (shaped, candidate) = reprepare_and_layout_candidate(
            &mut self.inputs,
            root_font_size,
            note_flow,
            overrides,
            resolve,
            constraints,
            image_metrics,
            glyph_shaper,
            resources,
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
        self.inputs
            .replace_paint_overrides(overrides)
            .expect("canonical paint owner");
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
        self.inputs
            .report_structure_memory(&mut report, "PreparedDocument");
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
            .element_for_box(box_idx, self.document())
            .map_or("", |element| element.tag())
    }

    pub fn get_id(&self, box_idx: usize) -> Option<&str> {
        storage::id_idx(
            self.inputs
                .layout_tree
                .element_for_box(box_idx, self.document())?,
        )
        .map(|id| self.document().string(id))
    }

    pub fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.inputs.layout_tree.get_box_style_indices(box_idx)
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
        shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<ShapedDocument<'a>, ShapeError> {
        // The closure takes ownership of the inputs, so the selection is
        // cloned once for this pass.
        let fonts = PublicationFonts::clone(&self.inputs.publication_fonts);
        with_document_shaping(
            shaper,
            &fonts,
            |shaper, resources| {
                let shaped = shape_text(&self.inputs, shaper, resources)?;
                Ok(ShapedDocument {
                    inputs: self.inputs,
                    shaped,
                })
            },
            resources,
        )
    }

    /// Attach a completed layout to its sole preparation owner.
    pub fn with_layout(mut self, layout: DetachedLayout) -> LaidOutDocument<'a> {
        assert_eq!(self.inputs.generation, layout.data.base_shaped.preparation);
        if let Some(paint) = layout.paint {
            self.inputs
                .paint
                .owned_mut()
                .expect("canonical paint owner")
                .replace_palette(paint);
        }
        LaidOutDocument {
            inputs: self.inputs,
            data: layout.data,
        }
    }

    pub fn shape_borrowed(
        &self,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<ShapedDocument<'_>, ShapeError> {
        with_document_shaping(
            glyph_shaper,
            &self.inputs.publication_fonts,
            |glyph_shaper, resources| self.inputs.shape_active_document(glyph_shaper, resources),
            resources,
        )
    }

    /// Shapes and lays out a document as one renderer-resource transaction.
    /// A shaping or exact-line-refinement failure leaves the previous renderer
    /// resources active.
    pub fn shape_and_layout_with_metrics_and_shaper(
        &self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'_>, ShapeError> {
        with_document_shaping(
            glyph_shaper,
            &self.inputs.publication_fonts,
            |glyph_shaper, resources| {
                self.inputs
                    .shape_active_document(glyph_shaper, resources)?
                    .layout_coordinated(
                        constraints,
                        image_metrics,
                        glyph_shaper,
                        None,
                        resources,
                        RefinementTransaction::Existing,
                    )
            },
            resources,
        )
    }

    /// Timed form of [`Self::shape_and_layout_with_metrics_and_shaper`]. Clock
    /// reads are kept out of the ordinary layout API.
    pub fn shape_and_layout_with_metrics_and_shaper_timed(
        &self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(LaidOutDocument<'_>, ShapeLayoutTimings), ShapeError> {
        with_document_shaping(
            glyph_shaper,
            &self.inputs.publication_fonts,
            |glyph_shaper, resources| {
                let shape_started = Instant::now();
                let shaped = self.inputs.shape_active_document(glyph_shaper, resources)?;
                let shape = shape_started.elapsed();
                let layout_started = Instant::now();
                let laid_out = shaped.layout_coordinated(
                    constraints,
                    image_metrics,
                    glyph_shaper,
                    None,
                    resources,
                    RefinementTransaction::Existing,
                )?;
                Ok((
                    laid_out,
                    ShapeLayoutTimings {
                        shape,
                        layout: layout_started.elapsed(),
                    },
                ))
            },
            resources,
        )
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
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'_>, ShapeError> {
        with_append_shaping(
            glyph_shaper,
            &self.inputs.publication_fonts,
            |glyph_shaper, resources| {
                let shaped = self.inputs.shape_active_document(glyph_shaper, resources)?;
                shaped.layout_coordinated(
                    constraints,
                    image_metrics,
                    glyph_shaper,
                    None,
                    resources,
                    RefinementTransaction::Existing,
                )
            },
            resources,
        )
    }
}

fn shape_text(
    inputs: &PreparedInputs,
    glyph_shaper: &mut impl GlyphShaper,
    resources: &mut GlyphResourceStore,
) -> Result<Box<ShapedText>, ShapeError> {
    profile_scope!("shape.total");

    let inline_content = inputs.text.content();
    let mut glyphs = profile!(
        "shape.allocate_glyphs",
        inline_content
            .characters()
            .iter()
            .map(|&character| GlyphId(character as u32))
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
    let (link_glyph_targets, anchor_glyphs) = profile!(
        "shape.semantic_targets",
        collect_glyph_targets(&inputs.document, &inputs.layout_tree, inline_content)
    );
    let inline_plan_generation = InlinePlanGeneration::new();
    let words = build_word_index(inputs, &font_metrics);
    Ok(Box::new(ShapedText {
        resource_generation: resources.generation(),
        preparation: inputs.generation,
        geometry_box_count: inputs.layout_tree.box_count(),
        glyphs,
        text_geometry,
        shared: SharedShapedData {
            words,
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
    glyph_shaper: &mut impl GlyphShaper,
    resources: &mut GlyphResourceStore,
) -> Result<(Box<ShapedText>, LayoutProducts), ShapeError> {
    with_document_shaping(
        glyph_shaper,
        &inputs.publication_fonts,
        |glyph_shaper, resources| {
            let shaped = shape_text(inputs, glyph_shaper, resources)?;
            let candidate = layout_with_first_lines(
                inputs,
                &shaped,
                constraints,
                image_metrics,
                glyph_shaper,
                None,
                LayoutScratch::default(),
                resources,
                RefinementTransaction::Existing,
            )?;
            Ok((shaped, candidate))
        },
        resources,
    )
}

/// A prepared document whose inline text has concrete glyph IDs and metrics.
/// Layout constraints can change without repeating parsing, styling, box-tree
/// construction, or shaping.
impl<'a> ShapedDocument<'a> {
    /// Layout through the coordinator, including width-dependent pseudo text.
    pub fn layout_with(
        self,
        request: LayoutRequest<'_>,
        shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, ShapeError> {
        let fallback;
        let metrics = match request.metrics {
            Some(metrics) => metrics,
            None => {
                fallback = ImageMetrics::from_document(self.inputs.document.as_ref());
                &fallback
            }
        };
        self.layout_coordinated(
            request.constraints,
            metrics,
            shaper,
            request.timings,
            resources,
            RefinementTransaction::Append,
        )
    }
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
        self.inputs
            .report_structure_memory(&mut report, "ShapedDocument");
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
    #[doc(hidden)]
    pub fn layout(
        self,
        constraints: LayoutConstraints,
        resources: &GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, ShapeError> {
        let image_metrics = ImageMetrics::from_document(self.inputs.document.as_ref());
        self.layout_with_metrics(constraints, &image_metrics, resources)
    }

    #[doc(hidden)]
    pub fn layout_with_timings(
        self,
        constraints: LayoutConstraints,
        resources: &GlyphResourceStore,
    ) -> Result<(LaidOutDocument<'a>, LayoutTimings), ShapeError> {
        let image_metrics = ImageMetrics::from_document(self.inputs.document.as_ref());
        self.layout_with_timings_and_metrics(constraints, &image_metrics, resources)
    }

    #[doc(hidden)]
    pub fn layout_with_metrics(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, ShapeError> {
        self.layout_impl(constraints, image_metrics, None, resources)
    }

    /// Coordinate layout and any width-dependent first-line reshaping.
    #[doc(hidden)]
    pub fn layout_with_shaper(
        self,
        constraints: LayoutConstraints,
        shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, ShapeError> {
        let metrics = ImageMetrics::from_document(self.inputs.document.as_ref());
        self.layout_with_metrics_and_shaper(constraints, &metrics, shaper, resources)
    }

    /// Compatibility entry point. Document shaping is authoritative, so width
    /// changes only invoke the backend shaper for width-dependent pseudo text.
    #[doc(hidden)]
    pub fn layout_with_metrics_and_shaper(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, ShapeError> {
        self.layout_coordinated(
            constraints,
            image_metrics,
            glyph_shaper,
            None,
            resources,
            RefinementTransaction::Append,
        )
    }

    /// Timed layout follows the same first-line coordination as ordinary layout.
    #[doc(hidden)]
    pub fn layout_with_timings_and_metrics_and_shaper(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(LaidOutDocument<'a>, LayoutTimings), ShapeError> {
        let mut timings = LayoutTimings::default();
        let document = self.layout_coordinated(
            constraints,
            image_metrics,
            shaper,
            Some(&mut timings),
            resources,
            RefinementTransaction::Append,
        )?;
        Ok((document, timings))
    }

    fn layout_coordinated(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl GlyphShaper,
        timings: Option<&mut LayoutTimings>,
        resources: &mut GlyphResourceStore,
        transaction: RefinementTransaction,
    ) -> Result<LaidOutDocument<'a>, ShapeError> {
        match transaction {
            RefinementTransaction::Existing => {
                debug_assert_eq!(self.shaped.resource_generation, resources.generation());
            }
            RefinementTransaction::Append => {
                if self.shaped.resource_generation != resources.generation()
                    || glyph_shaper.resource_generation() != resources.generation()
                {
                    return Err(resource_generation_mismatch());
                }
            }
        }
        let candidate = layout_with_first_lines(
            &self.inputs,
            &self.shaped,
            constraints,
            image_metrics,
            glyph_shaper,
            timings,
            LayoutScratch::default(),
            resources,
            transaction,
        )?;
        Ok(candidate.into_document(
            self.inputs,
            self.shaped,
            constraints,
            image_metrics,
            resources,
        ))
    }

    #[doc(hidden)]
    pub fn layout_with_timings_and_metrics(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) -> Result<(LaidOutDocument<'a>, LayoutTimings), ShapeError> {
        let mut timings = LayoutTimings::default();
        let document =
            self.layout_impl(constraints, image_metrics, Some(&mut timings), resources)?;
        Ok((document, timings))
    }

    fn layout_impl(
        self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut LayoutTimings>,
        resources: &GlyphResourceStore,
    ) -> Result<LaidOutDocument<'a>, ShapeError> {
        self.inputs.require_static_text()?;
        if self.shaped.resource_generation != resources.generation() {
            return Err(resource_generation_mismatch());
        }
        Ok(LayoutProducts::new(
            &self.inputs,
            &self.shaped,
            constraints,
            image_metrics,
            timings,
            LayoutScratch::default(),
            resources,
        )
        .into_document(
            self.inputs,
            self.shaped,
            constraints,
            image_metrics,
            resources,
        ))
    }
}

/// Mutable layout outputs and scratch reused before and after publication.
/// Preparation and base shaping are borrowed for each layout pass.
pub(crate) struct LayoutProducts {
    pub(super) geometry: BoxGeometry,
    /// Retained placement relationships follow geometry through publication.
    pub(super) placement: PlacementState,
    pub(super) refinement: Option<FirstLineRefinement>,
    pub(super) state: LayoutState,
    pub(super) scratch: LayoutScratch,
}

impl LayoutProducts {
    pub(super) fn new(
        inputs: &PreparedInputs,
        base: &ShapedText,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        timings: Option<&mut LayoutTimings>,
        layout_scratch: LayoutScratch,
        resources: &GlyphResourceStore,
    ) -> Self {
        let mut candidate = Self {
            geometry: BoxGeometry::default(),
            placement: PlacementState::default(),
            refinement: None,
            state: LayoutState::default(),
            scratch: layout_scratch,
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
        timings: Option<&mut LayoutTimings>,
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
                state: &mut self.state,
                placement: &mut self.placement,
                scratch: &mut self.scratch,
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
            data: LayoutData {
                base_shaped,
                layout: self,
                paint_colors: Vec::new(),
                last_constraints: constraints,
                last_image_metrics: image_metrics.clone(),
            },
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
        document.data.layout = self;
        document.data.last_constraints = constraints;
        document.data.last_image_metrics = image_metrics.clone();
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
        word_index: Some(&shaped.shared.words),
        ellipsis_glyphs: &shaped.shared.ellipsis_glyphs,
        hyphen_glyphs: &shaped.shared.hyphen_glyphs,
        inline_plan_generation: shaped.inline_plan_generation,
        image_metrics,
        paint: inputs.paint.view(),
        text_style_ranges: shaped.paint_ranges,
        resource_generation: shaped.resource_generation,
    }
}

/// Owned products moved together between detached and published documents.
pub(super) struct LayoutData {
    pub(super) layout: LayoutProducts,
    /// Viewport-independent shaping retained so a width change can recompute
    /// the dynamic extent of `::first-line` without accumulating old styles.
    pub(super) base_shaped: Box<ShapedText>,
    pub(super) paint_colors: Vec<PaintColorRun>,
    pub(super) last_constraints: LayoutConstraints,
    pub(super) last_image_metrics: ImageMetrics,
}

/// A fully shaped and laid-out document.
///
/// No public constructor exists: values can only be produced from a
/// [`ShapedDocument`]. Relayout mutates only layout-owned data, retaining the
/// immutable shaping inputs and reusing top-level layout buffers.
pub struct LaidOutDocument<'a> {
    pub(super) inputs: Box<PreparedInputs<'a>>,
    pub(super) data: LayoutData,
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

impl LayoutData {
    fn read<'a>(&'a self, inputs: InputRefs<'a>) -> DocumentRead<'a> {
        DocumentRead {
            inputs,
            geometry: &self.layout.geometry,
            base_shaped: &self.base_shaped,
            refinement: self.layout.refinement.as_ref(),
            layout_state: &self.layout.state,
            paint_colors: &self.paint_colors,
        }
    }
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
    pub(super) data: LayoutData,
}

/// Scoped layout products. DOM and styles remain in the parent document.
/// Querying requires that same preparation owner; replacing it invalidates the note.
pub struct NoteLayout {
    pub(crate) resource_lifetime: Option<Arc<()>>,
    owner: PreparationGeneration,
    layout_tree: LayoutTree,
    text: PreparedText,
    paint: NotePaint,
    layout: DetachedLayout,
}

impl NoteLayout {
    /// Whether this scope still belongs to the owner's canonical preparation.
    pub fn belongs_to(&self, owner: &LaidOutDocument<'_>) -> bool {
        self.owner == owner.inputs.generation
    }

    pub fn render_view<'a>(
        &'a self,
        owner: &'a LaidOutDocument<'_>,
        resources: &'a GlyphResourceStore,
    ) -> Option<RenderView<'a>> {
        if !resources.paint_resources_valid()
            || self.layout.data.base_shaped.resource_generation != resources.generation()
            || !self.belongs_to(owner)
            || self.layout.data.base_shaped.resource_generation
                != owner.data.base_shaped.resource_generation
        {
            return None;
        }
        let doc = self.layout.data.read(InputRefs {
            document: &owner.inputs.document,
            styles: &owner.inputs.styles,
            layout_tree: &self.layout_tree,
            text: &self.text,
            paint: self.paint.view(&owner.inputs.paint)?,
        });
        if doc.image_metrics_affect_layout(
            &self.layout.data.last_image_metrics,
            &owner.data.last_image_metrics,
            self.layout.data.last_constraints,
        ) {
            return None;
        }
        Some(RenderView {
            resources: Some(resources),
            scope_lifetime: self.resource_lifetime.as_ref(),
            glyph_metrics: resources.metrics(),
            doc,
        })
    }
}

impl<'a> LaidOutDocument<'a> {
    pub(crate) fn shaping_generation(&self) -> InlinePlanGeneration {
        ShapedTextView::new(&self.data.base_shaped, self.data.layout.refinement.as_ref())
            .inline_plan_generation
    }

    /// Only omit geometry work when resolved sizing proves it independent of
    /// changed natural dimensions. Intrinsic formatting contexts stay conservative.
    pub(crate) fn image_metrics_affect_layout(&self, next: &ImageMetrics) -> bool {
        self.read().image_metrics_affect_layout(
            &self.data.last_image_metrics,
            next,
            self.data.last_constraints,
        )
    }

    pub(crate) fn accept_image_metrics(&mut self, metrics: &ImageMetrics) {
        self.data.last_image_metrics = metrics.clone();
    }

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
            resource_lifetime: None,
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
                data: self.data,
            },
        }
    }

    fn read(&self) -> DocumentRead<'_> {
        self.data.read(self.inputs.refs())
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn shape_borrowed(
        &self,
        shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<ShapedDocument<'_>, ShapeError> {
        with_document_shaping(
            shaper,
            &self.inputs.publication_fonts,
            |shaper, resources| self.inputs.shape_active_document(shaper, resources),
            resources,
        )
    }

    pub fn into_detached(self) -> DetachedLayout {
        DetachedLayout {
            paint: self
                .inputs
                .paint
                .into_owned()
                .map(PaintStyles::into_palette),
            data: self.data,
        }
    }

    /// Move the retained shape out when starting an independent layout pass.
    /// Glyph storage keeps its address; prior layout buffers are released.
    pub fn into_shaped(self) -> ShapedDocument<'a> {
        ShapedDocument {
            inputs: self.inputs,
            shaped: self.data.base_shaped,
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
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        let (shaped, candidate) = shape_and_layout_candidate(
            &self.inputs,
            constraints,
            image_metrics,
            glyph_shaper,
            resources,
        )?;
        self.data.base_shaped = shaped;
        candidate.publish(self, constraints, image_metrics, resources);
        Ok(())
    }

    /// Update a palette without manufacturing another prepared-stage owner.
    pub fn repaint_with_reader_overrides(
        &mut self,
        overrides: &ReaderStyleOverrides,
        resources: &GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        self.inputs.replace_paint_overrides(overrides)?;
        self.repaint(resources);
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
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        let previous = self.inputs.replace_paint_overrides(overrides)?;
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
        resolve: impl FnOnce(&Document) -> (ComputedStyles, Arc<[FontFace]>),
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        let (shaped, candidate) = reprepare_and_layout_candidate(
            &mut self.inputs,
            root_font_size,
            note_flow,
            overrides,
            resolve,
            constraints,
            image_metrics,
            glyph_shaper,
            resources,
        )?;
        self.data.base_shaped = shaped;
        candidate.publish(self, constraints, image_metrics, resources);
        Ok(())
    }

    /// Resolve paint over retained geometry, including specialized formatting
    /// contexts. A different prepared geometry epoch must be laid out first.
    pub fn repaint_from(
        &mut self,
        prepared: &'a PreparedDocument,
        resources: &GlyphResourceStore,
    ) -> bool {
        if self.data.base_shaped.preparation != prepared.inputs.generation {
            return false;
        }
        *self.inputs = prepared.inputs.borrowed();
        self.repaint(resources);
        true
    }

    fn repaint(&mut self, resources: &GlyphResourceStore) {
        self.rebuild_paint_colors(resources);
        html_layout::repaint(
            layout_inputs(
                &self.inputs,
                ShapedTextView::new(&self.data.base_shaped, self.data.layout.refinement.as_ref()),
                &self.data.last_image_metrics,
                resources,
            ),
            &self.data.layout.geometry,
            &mut self.data.layout.state,
            self.data.last_constraints,
        );
    }

    /// Refresh specialized decoration geometry while retaining shaped text.
    /// The paint variant must come from this document's prepared publication.
    pub fn relayout_with_paint_from(
        &mut self,
        prepared: &'a PreparedDocument,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        let image_metrics = self.data.last_image_metrics.clone();
        self.relayout_from(
            prepared,
            self.data.last_constraints,
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
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        assert_eq!(
            self.data.base_shaped.preparation,
            prepared.inputs.generation
        );
        let previous_inputs = replace(&mut self.inputs, Box::new(prepared.inputs.borrowed()));
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
        let inputs = self.inputs.as_ref();
        let tree = inputs.layout_tree.as_ref();
        let styles = inputs.styles.as_ref();
        let text = inputs.text.as_ref();
        let paint = inputs.paint.as_ref();
        let line_output = &mut self.data.layout.state.line_output;
        for line in &line_output.lines {
            let Some(detail_idx) = line.details else {
                continue;
            };
            let details = &mut line_output.line_details[detail_idx.get() as usize - 1];
            if details.paint_color.is_some() {
                let root = html_text::whitespace_context_root(tree, line.owner_box_idx as usize);
                details.paint_color = text
                    .content()
                    .pseudo_styles()
                    .first_line(root)
                    .map(|indices| paint.resolve(styles, indices, root).foreground);
            }
        }

        let paint_ranges =
            ShapedTextView::new(&self.data.base_shaped, self.data.layout.refinement.as_ref())
                .paint_ranges;
        let colors = &mut self.data.paint_colors;
        colors.clear();
        for item in text.content().text_runs() {
            let glyphs = item.glyphs;
            let box_idx = item.box_idx as usize;
            let base = tree
                .get_box_style_indices(box_idx)
                .unwrap_or_else(|| styles.default_indices());
            let mut start = glyphs.start;
            while start < glyphs.end {
                let (override_style, mut end) =
                    text.first_letter_styles().segment(start, glyphs.end);
                let index = paint_ranges.partition_point(|range| range.glyphs.end <= start);
                let mut style = override_style.unwrap_or(base);
                if let Some(range) = paint_ranges.get(index) {
                    if range.glyphs.start <= start {
                        style = range.style;
                        end = end.min(range.glyphs.end);
                    } else {
                        end = end.min(range.glyphs.start);
                    }
                }
                let color = paint
                    .box_colors(box_idx)
                    .filter(|_| style == base)
                    .map(|colors| colors.foreground)
                    .unwrap_or_else(|| paint.resolve(styles, style, box_idx).foreground);
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
        self.prepare_text_paint(resources);
    }

    fn prepare_text_paint(&mut self, resources: &GlyphResourceStore) {
        // Geometry finalization leaves native fragments unprepared. Resolve
        // the palette first, then build the final colored slices once.
        prepare_text_paint(
            layout_inputs(
                &self.inputs,
                ShapedTextView::new(&self.data.base_shaped, self.data.layout.refinement.as_ref()),
                &self.data.last_image_metrics,
                resources,
            ),
            &mut self.data.layout.state,
            // Native runs retain authored colors; only palette overrides replace them.
            if self.inputs.paint.is_default() {
                &[]
            } else {
                &self.data.paint_colors
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
    /// use html_pipeline::engine::{GlyphResourceStore, LaidOutDocument, LayoutConstraints};
    /// fn resize(document: &mut LaidOutDocument, constraints: LayoutConstraints,
    ///           resources: &GlyphResourceStore) {
    ///     let view = document.render_view(resources);
    ///     document.relayout(constraints, resources).unwrap();
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
        (self.data.base_shaped.resource_generation == resources.generation()).then(|| RenderView {
            resources: Some(resources),
            scope_lifetime: None,
            doc: self.read(),
            glyph_metrics: resources.metrics(),
        })
    }

    #[cfg(test)]
    pub(super) fn text_geometry(&self) -> crate::shaping::TextGeometryView<'_> {
        self.read().text_geometry()
    }

    #[doc(hidden)]
    pub fn relayout(
        &mut self,
        constraints: LayoutConstraints,
        resources: &GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        let image_metrics = ImageMetrics::from_document(self.document());
        self.relayout_impl(constraints, image_metrics, None, resources)
    }

    #[doc(hidden)]
    pub fn relayout_with_timings(
        &mut self,
        constraints: LayoutConstraints,
        resources: &GlyphResourceStore,
    ) -> Result<LayoutTimings, ShapeError> {
        let mut timings = LayoutTimings::default();
        let image_metrics = ImageMetrics::from_document(self.document());
        self.relayout_impl(constraints, image_metrics, Some(&mut timings), resources)?;
        Ok(timings)
    }

    #[doc(hidden)]
    pub fn relayout_with_metrics(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        self.relayout_impl(constraints, image_metrics.clone(), None, resources)
    }

    /// Resize through the coordinator, including first-line refinement.
    #[doc(hidden)]
    pub fn relayout_with_shaper(
        &mut self,
        constraints: LayoutConstraints,
        shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        let metrics = ImageMetrics::from_document(self.document());
        self.relayout_with_metrics_and_shaper(constraints, &metrics, shaper, resources)
    }

    #[doc(hidden)]
    pub fn relayout_with_metrics_and_shaper(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        if self.data.base_shaped.resource_generation != resources.generation()
            || glyph_shaper.resource_generation() != resources.generation()
        {
            return Err(resource_generation_mismatch());
        }
        if !self.inputs.styles.has_first_line_styles() {
            // Ordinary resizing cannot fail during shaping. Retain geometry,
            // line output, placement and scratch storage instead of allocating
            // a private refinement candidate for every viewport change.
            return self.relayout_with_metrics(constraints, image_metrics, resources);
        }
        let candidate = layout_with_first_lines(
            &self.inputs,
            &self.data.base_shaped,
            constraints,
            image_metrics,
            glyph_shaper,
            None,
            take(&mut self.data.layout.scratch),
            resources,
            RefinementTransaction::Append,
        )?;
        candidate.publish(self, constraints, image_metrics, resources);
        Ok(())
    }

    /// Change geometry through one coordinated entry point. Ordinary resizing
    /// retains its allocation-reusing path; pseudo text is refined when needed.
    #[inline]
    pub fn relayout_with(
        &mut self,
        request: LayoutRequest<'_>,
        glyph_shaper: &mut impl GlyphShaper,
        resources: &mut GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        let fallback;
        let image_metrics = match request.metrics {
            Some(metrics) => metrics,
            None => {
                fallback = self.data.last_image_metrics.clone();
                &fallback
            }
        };
        let constraints = request.constraints;
        if request.timings.is_none() {
            return self.relayout_with_metrics_and_shaper(
                constraints,
                image_metrics,
                glyph_shaper,
                resources,
            );
        }
        if self.data.base_shaped.resource_generation != resources.generation()
            || glyph_shaper.resource_generation() != resources.generation()
        {
            return Err(resource_generation_mismatch());
        }
        if !self.inputs.styles.has_first_line_styles() {
            // Ordinary resizing cannot fail during shaping. Retain geometry,
            // line output, placement and scratch storage instead of allocating
            // a private refinement candidate for every viewport change.
            return self.relayout_impl(
                constraints,
                image_metrics.clone(),
                request.timings,
                resources,
            );
        }
        let candidate = layout_with_first_lines(
            &self.inputs,
            &self.data.base_shaped,
            constraints,
            image_metrics,
            glyph_shaper,
            request.timings,
            take(&mut self.data.layout.scratch),
            resources,
            RefinementTransaction::Append,
        )?;
        candidate.publish(self, constraints, image_metrics, resources);
        Ok(())
    }

    #[doc(hidden)]
    pub fn relayout_with_metrics_and_timings(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: &ImageMetrics,
        resources: &GlyphResourceStore,
    ) -> Result<LayoutTimings, ShapeError> {
        let mut timings = LayoutTimings::default();
        self.relayout_impl(
            constraints,
            image_metrics.clone(),
            Some(&mut timings),
            resources,
        )?;
        Ok(timings)
    }

    fn relayout_impl(
        &mut self,
        constraints: LayoutConstraints,
        image_metrics: ImageMetrics,
        timings: Option<&mut LayoutTimings>,
        resources: &GlyphResourceStore,
    ) -> Result<(), ShapeError> {
        self.inputs.require_static_text()?;
        if self.data.base_shaped.resource_generation != resources.generation() {
            return Err(resource_generation_mismatch());
        }
        self.data.layout.relayout(
            &self.inputs,
            &self.data.base_shaped,
            constraints,
            &image_metrics,
            timings,
            resources,
        );
        self.rebuild_anchor_positions();
        self.rebuild_paint_colors(resources);
        self.data.last_constraints = constraints;
        self.data.last_image_metrics = image_metrics;
        Ok(())
    }

    fn rebuild_anchor_positions(&mut self) {
        let document = self.inputs.document.as_ref();
        let tree = self.inputs.layout_tree.as_ref();
        let anchor_glyphs = &self.data.base_shaped.shared.anchor_glyphs;
        let layout = &mut self.data.layout;
        let lines = &layout.state.line_output;
        let geometry = &layout.geometry;
        let positions = &mut layout.state.semantic_indexes.anchor_positions;
        positions.clear();
        for box_idx in 0..tree.box_count() {
            let Some(id_idx) = box_id(document, tree, box_idx) else {
                continue;
            };
            let glyph = anchor_glyphs.get(&id_idx).copied();
            let y = glyph.and_then(|glyph| {
                let line_idx = lines.glyph_line_indices.get(glyph as usize)?;
                lines.lines.get(line_idx).map(|line| line.point.y)
            });
            let y = y.or_else(|| {
                once(box_idx)
                    .chain(tree.ancestors(box_idx))
                    .find_map(|idx| {
                        tree.box_at(idx)?;
                        let point = geometry.point(idx);
                        (geometry.size(idx).height > 0.0 || point.y != 0.0).then_some(point.y)
                    })
            });
            let Some(y) = y else { continue };
            let order = glyph.unwrap_or(box_idx as u32);
            positions
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
        assert_eq!(
            self.data.base_shaped.resource_generation,
            resources.generation()
        );
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
        self.data.layout.state.line_output.lines.len()
    }

    /// Report document allocations together with its borrowed glyph ledger,
    /// counting shared shaping advances once.
    pub fn memory_usage_report_with_resources(
        &self,
        resources: &GlyphResourceStore,
    ) -> MemoryUsageReport {
        assert_eq!(
            self.data.base_shaped.resource_generation,
            resources.generation()
        );
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
        self.inputs
            .report_structure_memory(&mut report, "LaidOutDocument");
        report.extend_prefixed(
            "LaidOutDocument.geometry",
            self.data.layout.geometry.memory_usage_report(),
        );
        report.add(
            "LaidOutDocument.local_placement",
            self.data.layout.placement.memory_usage_bytes(),
            self.box_count(),
        );
        report.extend_prefixed(
            "LaidOutDocument.inline_content",
            self.inputs.text.memory_usage_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument.layout_state",
            self.data.layout.state.memory_usage_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument",
            self.data.base_shaped.memory_usage_report(seen),
        );
        if let Some(refinement) = &self.data.layout.refinement {
            report.extend_prefixed(
                "LaidOutDocument.refinement",
                refinement.memory_usage_report(seen),
            );
        }
        report.add_vec_storage(
            "LaidOutDocument.paint_colors.storage",
            &self.data.paint_colors,
        );
        report.extend_prefixed(
            "LaidOutDocument.paragraph_layouts",
            self.data.layout.scratch.paragraph_layout_memory_report(),
        );
        report.extend_prefixed(
            "LaidOutDocument.inline_plans",
            self.data.layout.scratch.inline_plan_memory_report(),
        );
        report.add(
            "LaidOutDocument.rollback_image_metrics",
            self.data.last_image_metrics.memory_usage_bytes(),
            self.data.last_image_metrics.len(),
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
                    pending.extend(element.children().rev());
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
    let root = document.element_by_id(id)?.node_id();

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
            // A note is shaped with the declarations of the document it lives in.
            publication_fonts: Storage::Borrowed(&inputs.publication_fonts),
        }),
    })
}

fn note_ids<'a>(inputs: &'a PreparedInputs<'_>) -> Vec<&'a str> {
    let document = &inputs.document;
    document
        .node_ids()
        .filter_map(|node| document.element_ref(node))
        .filter(|element| element_is_note_target(*element))
        .filter_map(|element| element.id())
        .collect()
}

impl<'a> DocumentRead<'a> {
    fn image_metrics_affect_layout(
        self,
        previous: &ImageMetrics,
        next: &ImageMetrics,
        constraints: LayoutConstraints,
    ) -> bool {
        let tree = self.inputs.layout_tree;
        for (&owner, &image) in &tree.image_indices {
            let resource = &self.inputs.document.images()[image as usize];
            if previous.display_size(image, resource) == next.display_size(image, resource) {
                continue;
            }
            if constraints.image_sizing_policy() != ImageSizingPolicy::WebCompatible {
                return true;
            }
            let Some(style) = self.box_used_style(owner as usize) else {
                return true;
            };
            if !matches!(
                (style.width(), style.height()),
                (UsedPreferredSize::Px(_), UsedPreferredSize::Px(_))
            ) || style.object_fit() != ObjectFit::Fill
                || [
                    style.min_width(),
                    style.max_width(),
                    style.min_height(),
                    style.max_height(),
                ]
                .iter()
                .any(|size| {
                    matches!(
                        size,
                        UsedPreferredSize::Content
                            | UsedPreferredSize::MinContent
                            | UsedPreferredSize::MaxContent
                            | UsedPreferredSize::FitContent
                            | UsedPreferredSize::Stretch
                    )
                })
            {
                return true;
            }
            for ancestor in tree.ancestors(owner as usize) {
                if !matches!(
                    tree.get_box_layout_mode(ancestor),
                    Some(LayoutMode::Block(_) | LayoutMode::Inline(_))
                ) {
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn element_for_box(self, box_idx: usize) -> Option<ElementRef<'a>> {
        self.inputs
            .layout_tree
            .element_for_box(box_idx, self.inputs.document)
    }

    pub(super) fn render_view(self, glyph_metrics: &'a GlyphMetrics) -> RenderView<'a> {
        RenderView {
            resources: None,
            scope_lifetime: None,
            doc: self,
            glyph_metrics,
        }
    }
    pub fn document(&self) -> &'a Document {
        self.inputs.document
    }

    pub(crate) fn link_for_glyph(&self, glyph_idx: u32) -> Option<LinkTarget<'a>> {
        let target = self.base_shaped.shared.link_glyph_targets.get(&glyph_idx)?;
        Some(LinkTarget {
            href: self.inputs.document.string(target.href),
            is_note_reference: target.note_reference,
        })
    }

    pub(crate) fn target_is_note(&self, id: &str) -> bool {
        self.document()
            .element_by_id(id)
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

    pub(crate) fn glyph_source_offset(&self, glyph_idx: u32) -> Option<u32> {
        self.inputs
            .text
            .content()
            .glyph_source_offset(glyph_idx as usize)
    }

    pub(crate) fn box_dom_element_idx(&self, box_idx: usize) -> Option<u32> {
        self.inputs.layout_tree.get_box_dom_element(box_idx)
    }

    pub(crate) fn box_layout_mode(&self, box_idx: usize) -> Option<&'a LayoutMode> {
        self.inputs.layout_tree.get_box_layout_mode(box_idx)
    }

    pub fn box_style_indices(&self, box_idx: usize) -> Option<StyleIndices> {
        self.inputs.layout_tree.get_box_style_indices(box_idx)
    }

    pub fn box_used_style(&self, box_idx: usize) -> Option<html_style_model::UsedStyleView<'a>> {
        let indices = self
            .box_style_indices(box_idx)
            .unwrap_or_else(|| self.inputs.styles.default_indices());
        self.base_shaped
            .shared
            .font_metrics
            .used_style(self.inputs.styles, indices, box_idx)
    }

    pub(super) fn box_is_visible(&self, box_idx: usize) -> bool {
        self.box_used_style(box_idx)
            .is_some_and(|style| style.visibility() == Visibility::Visible)
    }

    pub(crate) fn box_text_format(&self, box_idx: usize) -> BoxTextFormat<'a> {
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
            font_family: style
                .font_family()
                .and_then(|family| self.inputs.styles.string(family)),
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
        is_block_container(self.inputs.layout_tree, box_idx)
    }

    pub(super) fn shaped(&self) -> ShapedTextView<'a> {
        ShapedTextView::new(self.base_shaped, self.refinement)
    }

    pub(super) fn text_geometry(&self) -> crate::shaping::TextGeometryView<'a> {
        self.shaped().text_geometry
    }
}
