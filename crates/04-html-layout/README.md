# html-layout

`html-layout` turns styled DOM content into backend-neutral used geometry and
resolved fragments. It does not own executable painting, UI interaction,
resource loading, or publication-specific addressing syntax.

The pipeline owns the canonical document and coordinates independent stages:

- `html-format` constructs formatting topology from styled DOM;
- `html-text` prepares canonical logical text using borrowed formatting context;
- `html-shape` produces glyph registration and run measurements;
- `html-layout` consumes those measurements and constraints to calculate geometry;
- `html-pipeline` owns stage lifetimes, transactions, first-line refinement and publication.

`html-engine-model` contains the shared topology, metrics, paint inputs, policies,
and output contracts below every executor. It has no dependency on any stage or
on the pipeline. Layout consumes borrowed inputs and mutates its own output and
scratch buffers; it does not own a document lifecycle or a font backend.

Use `html_pipeline::engine` (or the public `html::layout` facade) for document
preparation, shaping, render views and relayout. `shape_and_layout` and
`layout_with_shaper` coordinate first-line styles. No-shaper `layout`/`relayout`
variants return an explicit error when first-line styles require the coordinator,
and relayout checks this before mutating published output. A static document
can continue to reuse authoritative shaping without calling the backend.

Stage ownership moves through the pipeline and published readers borrow it.
No new reference-counted ownership, locks, or interior-mutable stage containers
are introduced by these boundaries.

Inline source has one canonical representation in prepared text: normalized
characters, source offsets, wrap flags, and inline items. Shaping and layout
borrow that source. Shaping owns a separate glyph-ID buffer and geometry because
renderer glyph IDs are different values from Unicode characters. Replacing font
resources and refining `::first-line` reuse the source metadata; they never clone
the whole inline-content structure. Prepared text owns its content directly,
without another reference-counted allocation. Diagnostics count canonical source
once. First-line refinement owns only changed glyph ranges and range-local
geometry; a borrowed `GlyphSlice` and geometry view overlay those changes on the
base shape. Callers can iterate or index the logical glyph slice without
flattening it; `as_contiguous()` returns storage only when one buffer covers the
requested range.

Token and line-breaking plans belong to the layout executor's mutable cache.
Their arrays are owned vectors: a paragraph takes its plan, borrows its data
through line selection and fragment emission, then returns the plan. Shaping
carries only a value generation ID for cache invalidation. Independent layouts
own independent caches. Parallel
measurement workers transfer completed plans back to the parent after joining.
There are no `Arc` or `OnceLock` allocations in token planning. Glyph-resource
generations also use value IDs.

The paragraph cache stores compact 16-byte line-break choices, borrowing the
canonical token plan when publishing output. Line metrics, placements, and
spacing adjustments belong to the published output. During a relayout with
unchanged constraints, eligible text paragraphs borrow the previous publication
and move its line records and sparse spacing groups into the new output.
The cache retains only break choices, scalar dependencies, and a publication
locator with the paragraph flow size. The previous output is released before
finalization, and its top-level allocations are recycled. Width changes, float
exclusions, and unsupported fragment layouts use ordinary measurement.
Paragraphs without float anchors skip float-position prediction altogether.

The pipeline consumes the parsed syntax tree into the canonical DOM, moving
text buffers. Restyling borrows that DOM. Failed preparation restores moved
previous state, root font size, and formula resources. Once publication completes,
prepared and shaped cache entries hold validity keys only. The layout document
owns shaping in a `Box`; candidate layout and first-line passes borrow it.
`ShapedDocument` moves into layout and cannot be cloned.

Prepared topology is owned once, borrowed by derived stages, and never cloned
merely to attach used points or sizes. Relayout resets the parallel geometry/output stores and reuses
their capacities. The private `LayoutSession` is the pass composition root;
its storage fields are private and it assembles direct component borrows for
`BlockContext`, `InlineContext`, `TableContext`, `TaffyContext`,
`FragmentContext`, and `FinalizationContext`. None of those contexts stores or
forwards the whole session. They own their cache, checkpoint, fragment, or
finalization operations, while the session retains only recursive whole-pass
coordination. Flex and grid have distinct entry modules over the isolated
shared Taffy adapter, including isolated reusable measurement output.

Consumers enter through `RenderView` and should immediately select the narrow
capability they need: `text()`, `boxes()`, `fragments()`, `semantics()`,
`geometry()`, `addressing()`, or `export()`.
Those borrowed views allocate nothing and do not expose mutable stage storage.
`LaidOutDocument` has no `Clone` implementation. Rendered fragments borrow the
owning document, and Rust prevents mutating the owner while a view is in use.

`document.render_view().export().markdown(start..end)` converts a selected logical
glyph range to Markdown. `selection_text(start..end)` returns both plain text and
Markdown; `markdown_for_box(box_idx)` copies a selected table, code block, list
item, or image. Applications own selection interaction and clipboard access.
Selections preserve source order and omit hidden text, generated content, and
layout-added hyphens. Simple tables use GFM; complex tables retain structural HTML.
`RenderTable::markdown()` and `markdown_in(rows, columns)` provide plain-text GFM
grids for rectangular cell selections, flattening merged cells into grid slots.
`RenderTable::unstyled_html()` retains cell spans and escapes cell text without
copying author styling. The facade's optional `math-export` feature exposes
`html::export::mathml_to_latex` and `equation_latex` for formula copying.

`semantics()` classifies headings, lists, code, tables, figures, notes, and book
structures into `ContentRole` values. It also provides ancestry and owned-text
queries. HTML, EPUB attributes, and ARIA token interpretation stay in the engine;
pagination preferences, such as keeping short code blocks together, stay in the
reader.

Publication TOCs come from the host. Parsing does not generate outlines or
heading anchors. Narration can use the publication's embedded TOC to select
sections; reading queries do not infer exclusions from heading titles.

`text().word_range_at`, `find_matches`, and `locate_quote` operate on logical
character ranges. Search preserves source offsets through case and diacritic
normalization, returns complete graphemes, and excludes hidden and generated
content. Quote location accepts exact text plus optional adjoining context.
Whole-publication search scope and annotation ownership remain application
policy. `html::text::find_text_matches` applies the same matching rules to a
parse-only text index without requiring layout.

`geometry()` supplies intrinsic caret positions, line hit testing, and selection
rectangles in document coordinates. Callers may pass retained
`PositionedShapedLine` fragments for native caret geometry; an empty slice uses
the engine's retained character advances and cluster boundaries. Applications
project these coordinates into their screen, scroll, and column layouts.

`addressing().source_location_on_line(line, glyph)` resolves images, text, and
owners of generated content against the current document's canonical DOM.
`source_element_y` resolves element-only locations through that same DOM. There
is no separate source index to prepare, retain, or invalidate across reflow and
backend rehydration. A `SourceLocation` contains document-relative steps and a
UTF-16 offset, leaving spine identifiers and EPUB CFI encoding to the application.

Absolute publication updates the owning coordinate arrays in place, without
retaining a second local-coordinate snapshot.
Layout fragments retain geometry-dependent CSS resolution and semantic
pattern/layer information. They do not tessellate dashed, dotted, or double
decorations: `html-render-core` owns that expansion, color conversion, paint
sequencing, clipping, and painter commands for both retained and interactive
paths. Intrinsic image dimensions enter as a backend-neutral metrics table;
loading and decoding policy remain upstream. `html-view-core` owns EPUB CFI
syntax and resolves it through generic source positions from the addressing
view.

Production code forbids `unsafe_code`. Stage dependencies must remain acyclic,
with document lifecycle and coordination in `html-pipeline`.
Text tests use deterministic metrics and require no windowing framework.

Horizontal bidirectional text is resolved per paragraph before shaping and
reordered per wrapped line. CSS `direction`/`unicode-bidi`, HTML `dir` (including
`auto`), `bdi`, and `bdo` participate without changing logical source positions.
Flex, grid, tables, inline edges, indentation, overflow markers, and list markers
use the corresponding horizontal direction.

Font adapters receive logical text in runs of one resolved direction through
`TextStyleSpan::direction()`. Native shaping must honor that direction and retain
caret stops in logical character order (decreasing X for RTL runs). Layout owns
visual placement and reuses the native resources on width-only relayout. Native
backends remain responsible for contextual glyph shaping and font fallback.
Scalar painters use `paint_glyph_at` to obtain mirrored punctuation and suppress
directional controls; `glyph_at` and reading/addressing views retain logical text.
Text fragments expose their physical offset and `is_rtl()` while retaining
source-token paint order for interleaving with inline boxes.

Horizontal Chinese, Japanese, and Korean paragraphs use Unicode line-break
opportunities with CSS `word-break` and `line-break` tailoring. Opening and
closing punctuation, small kana, iteration marks, mixed Latin words/numbers,
and Korean `keep-all` participate in both wrapping and intrinsic sizing.
`line-break: anywhere` still respects grapheme and backend shaping clusters.
Collapsed source newlines between unspaced CJK characters are removed; ordinary
spaces, Korean word separation, and preserved newlines retain their semantics.
Joining uses the current inline flow and stops at atomic boxes. Explicit script
subtags override the language's usual writing system. Bidi controls keep their
logical source order while remaining transparent to whitespace normalization.
Wrapping between inline boxes uses their nearest common ancestor's `white-space`;
discardable spaces retain their own style's wrap behavior.

Preparation detects candidate runs with an integer reduction before detailed
Unicode classification. Latin paragraphs retain their existing wrap metadata,
and CJK opportunities reuse the existing one-byte wrap store and zero-width
inline tokens. Width-only relayout does not repeat Unicode segmentation.
Font adapters receive an inherited `ContentLanguage` through
`TextStyleSpan::language()` and `FontMetricsRequest::language()` for localized
glyph and font-metric selection. Valid BCP 47 tags use the external
`language-tags` parser and expose structured language, script, region, variants,
extensions, and private-use fields through `ContentLanguage::tag()`. Computed
styles intern each tag once and inherit a typed `LanguageId`; empty declarations
reset inheritance and malformed declarations retain their original spelling.
Namespaced `xml:lang` takes precedence over `lang`. Content language does not set
text direction. Untagged documents allocate no language-table storage. Font coverage and
fallback remain backend responsibilities. Ruby, vertical writing, and advanced
CJK punctuation spacing/justification are separate features.

The Unicode line-break baseline comes from `unicode-linebreak` 0.1.5 (Unicode
15), with CSS tailoring and supplementary Han range handling in this crate.

Independent table-cell measurement can use worker-owned layout arenas without
changing deterministic publication order. Embedders opt in per layout through
`LayoutConstraints::with_parallel_workers`; small workloads remain sequential
to avoid dispatch overhead.
