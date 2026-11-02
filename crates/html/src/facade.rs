//! Deliberate public contracts. New implementation exports do not automatically
//! expand the application API. Each exported contract has one canonical module:
//! document addresses and parsing, resources, CSS, text backends, layout products,
//! or painting. Pipeline orchestration stays inside the engine.

pub mod document {
    pub use html_dom::{
        AttributeRef, Document, DocumentBuildError, DocumentLineage, DocumentMode, DomAncestorIter,
        DomNodeId, ElementRef, ImageResource, ImageSource, InvalidRootFontSize, MemoryUsage,
        MemoryUsageEntry, MemoryUsageReport, NodeRef, RootFontSize, SourceElementStep,
        SourcePosition, TextChildPosition, TextRef, cfi, collapse_whitespace,
        element_is_note_reference, element_is_note_target, write_escaped_html_attribute,
        write_escaped_html_text, write_escaped_xml,
    };
    pub use html_parse::{
        DocumentTextIndex, HtmlFragmentContext, HtmlParserOptions, HtmlQuirksMode,
        HtmlScriptingMode, HtmlSyntaxAttribute, HtmlSyntaxElement, HtmlSyntaxNode, HtmlSyntaxTree,
        MarkupSyntax, ParsedHtml, ParsedHtmlFragment, SourceTextPosition, StylesheetReference,
        XmlParseError, build_dom_document, decode_html_bytes, parse_document, parse_dom_document,
        parse_html_document, parse_html_document_bytes, parse_html_document_with_options,
        parse_html_fragment, parse_xml_document, plain_text_from_fragment, read_document_source,
    };

    /// Declared media type takes precedence over the resource filename.
    pub fn markup_syntax_for_resource(
        provider: &dyn crate::resources::ResourceProvider,
        uri: &str,
    ) -> MarkupSyntax {
        let media_type = provider
            .metadata(uri)
            .ok()
            .and_then(|metadata| metadata.media_type);
        MarkupSyntax::from_resource(uri, media_type.as_deref())
    }

    /// Build a parse-only text/source index for search without loading fonts,
    /// images, styles, or a reader viewport.
    pub fn text_index_for_resource(
        source: &str,
        provider: &dyn crate::resources::ResourceProvider,
        uri: &str,
    ) -> Option<DocumentTextIndex> {
        let parsed = parse_document(source, markup_syntax_for_resource(provider, uri)).ok()?;
        DocumentTextIndex::from_parsed(&parsed).ok()
    }
}

pub mod resources {
    pub use html_resources::{
        BlockingJob, BlockingJobSpawner, DecodedImage, FileSystemProvider, ImageLoadCompletion,
        ImageLoadError, ImageLoader, ImagePipeline, ImagePipelinePoll, ImageResourceContext,
        ImageService, InlineJobSpawner, ProviderImageLoader, ResourceMetadata, ResourceProvider,
        has_uri_scheme, load_image_bytes, load_image_bytes_async, media_type_essence,
        probe_dimensions, uri_path,
    };
}

/// Reader preferences applied over the publication's authored stylesheets.
pub mod style {
    pub use html_style_model::{Hyphens, ReaderStyleOverrides, TextAlign, ThemeAdaptation};
}

pub mod text {
    pub use html_pipeline::engine::{
        CapsSynthesis, FontFeatures, CaretCursor, CaretIter, CaretPositions, FontMetrics,
        FontRelativeMetrics, FontSlant, ObliqueAngle, GlyphHandle, GlyphId, GlyphMetric,
        GlyphRequest, GlyphResourceCheckpoint, GlyphResourceGeneration, GlyphResourceStore,
        GlyphResources, GlyphShaper, GlyphSlice, CursiveJoining, NativeResourceCheckpoint,
        PreparedTextRunFragment, ResourceShaper, RunFontMetrics, ShapeError, ShapedGlyph,
        ShapedLine, ShapedRun, ShapedTextRun, ShapingStyle, TextRunId, TextRunShapeRequest,
        TextStyleSpan,
    };
    pub use html_pipeline::engine::{
        TextPatternMatch, TextPatternMatcher, TextSearchOptions, find_text_matches, text_matches,
    };
    pub use html_style::{FontFace, FontFaceDescriptor, FontFaceSource, FontFaceStyle};
    pub use html_style_model::{
        ContentLanguage, FontStyle, FontSynthesisStyle, OpenTypeFeature, OpenTypeFeatureKind,
        TextDirection, is_cjk_character,
    };
}

pub mod layout {
    pub use html_pipeline::engine::{
        BoxTextFormat, CaretAffinity, ContentRole, ContentSemantics, ImageSizingPolicy,
        LineCharacterPlacement, LineId, LineTextPlacement, NoteFlow, PaginationGroup,
        PaginationLine, PaginationLines, PositionedShapedLine, RenderAddressingView,
        RenderAnchorPosition, RenderAnchorPositions, RenderAuthoritativeTextRun, RenderBoxView,
        RenderDecoration, RenderDecorationPattern, RenderDecorations, RenderEllipsisFragment,
        RenderExportView, RenderForcedBreak, RenderFragmentView, RenderGeometryView,
        RenderGlyphAdvanceRun, RenderGlyphAdvanceRuns, RenderGlyphOffsetRun, RenderGlyphOffsetRuns,
        RenderHyphenFragment, RenderImageFragment, RenderImageFragments, RenderLine,
        RenderLineTextFragment, RenderLineTextFragments, RenderLines, RenderListItemMarker,
        RenderOverflowClip, RenderPaintItem, RenderResourceLease, RenderSemanticsView, RenderTable,
        RenderTableCell, RenderTableRow, RenderTextRun, RenderTextRuns, RenderTextView, RenderView,
        SourceLocation, TextCompositionPolicy, is_navigation_section_title, merge_text_ranges,
    };
    pub use html_style_model::{ListStylePosition, StyleStringId};
}

/// Framework-neutral painting and basic, non-paginated fragment rendering.
pub mod render {
    pub use html_render_core::{
        DocumentPainter, FragmentImagePolicy, FragmentRenderError, FragmentRenderOptions,
        FragmentRenderer, LinePlacement, PaintFragment, PaintProjection, ProjectedLine,
        RegionProjection, RenderScene, RenderedFragment, paint_decoration_style,
        paint_line_ellipsis, paint_line_glyph_fragment, paint_line_glyphs, paint_line_text_runs,
        paint_projected_decoration, paint_resolved_decoration,
    };
    pub use html_style_model::TextDecorationLines;
}
