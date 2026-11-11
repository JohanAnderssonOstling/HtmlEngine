# Pipeline architecture review (excluding style and layout)

10 November 2026. Focus: simplicity without sacrificing performance.

Scope: parse, the pipeline session and stage lifecycle, shaping orchestration,
paint, the facade and the GPUI adapter. Style and layout are covered by
[style-architecture-review.md](style-architecture-review.md) and
[layout-architecture-review.md](layout-architecture-review.md).

Method: source reading only. Nothing was compiled, run or measured, so every
performance effect below is inferred from the code. Coverage is partial; see
[Coverage](#coverage) for what was and was not read.

## Summary

The stage code itself is in good shape: a single CSS-order paint traversal,
in-place relayout, and borrowed `RenderView`s. The complexity is concentrated in
the orchestration around it, and most of it comes from three places:

- a multi-entry cache design guarding a single entry,
- an ownership ladder that production barely uses,
- a recorded scene that every consumer immediately re-records.

## Findings, by payoff

### 1. Stage selection uses cache keys for a cache that holds one entry

- **Where:** `crates/html-pipeline/src/session.rs:509-721`,
  `crates/html-pipeline/src/types.rs:255-363`.
- **What:** The session holds one document and its committed `inputs`. Change
  detection still runs through four nested keys
  (`Parsed`/`Prepared`/`Shaped`/`LayoutCacheKey`), five input revisions, a
  `PreviousInputs` bool set, and `layout_key_reusing`.
- **Why it is redundant:** `validate_cache_against_requested`
  (`session.rs:957`) asserts the keys equal what the committed inputs derive.
  `PipelineChanges` already says which fields changed.
- **Simpler shape:** compute `EarliestStage` directly from the diff, plus three
  validity facts: prepared or published, the shaped backend generation, and the
  media match key. The keys, `PreviousInputs`, and the source, font and
  image-metrics revisions go away. The resource and stylesheet revisions remain
  only for cache clearing.
- **Performance:**
  - `prepared_cache_key()` clones every user stylesheet into an `Arc<[String]>`
    (`types.rs:307`); `rehydrate_glyphs` builds it twice
    (`session.rs:870/886`, `889`).
  - A source replacement compares the full source three times
    (`crates/html/src/configuration.rs:150`, `types.rs:218`, `session.rs:467`).
- **Also:** `update` and `apply_changes` are two entry points into the same
  `update_inner`; a full load can be "changes against empty".

### 2. The typestate ladder and `Storage<Owned|Borrowed>` do the same job as `InputRefs`

- **Where:** `crates/html-pipeline/src/stages/lifecycle.rs:45-88, 230-293,
  1400-1441`.
- **What:** There are four document types plus `DetachedLayout`, and every
  `PreparedInputs` field is `Storage`. Ownership is then checked at runtime
  (`borrowed_document()`, `expect("canonical paint owner")`).
- **Unused surface:** roughly 20 shape/layout/relayout variants exist.
  `layout`, `relayout`, `relayout_from`, `relayout_with_paint_from`
  and `..._timed` have no callers. `repaint_from` is called only from tests;
  `with_reader_paint_overrides` has one production caller (`session.rs:661`).
- **Simpler shape:** one always-owned `Prepared`, with compute functions taking
  the `Copy` ref struct that readers already use:
  `shape(refs, …) -> ShapedText` and
  `layout(refs, &shaped, …) -> LayoutProducts`. Candidates become plain locals.
  A palette change becomes a `&PaintPalette` parameter committed on success,
  instead of travelling through `DetachedLayout.paint`. `NoteLayout` already
  works this way.
- **Performance:** neutral; the same buffers move.

### 3. The unused surface in #2 exists for tests of lower crates

- **Where:** `html-format`, `html-shape` and `04-html-layout` dev-depend on
  `html-pipeline` with the `testing` feature, and `include!` shared fixtures.
- **Effect:** this keeps the `#[doc(hidden)] pub` matrix and
  `DocumentFactory`'s seven entry points alive.
- **Fix:** route `test-support/stage-fixtures.rs` through one session-backed
  helper. This is a fixture migration, and it is the precondition for deleting
  #2's variants.

### 4. Every `RenderScene` consumer re-records it

- **Where:** `adapters/html-gpui/src/fragment.rs:302-310`;
  `../HtmlViewCore/adapters/html-view-gpui/src/view.rs:1297-1300`.
- **What:** both build a `RenderScene`, replay it into `GpuiCommandPainter`,
  and keep only `content_height()`.
- **Cost of the intermediate:** recording copies the image hash, the full SVG
  bytes and the URI per draw
  (`crates/05-html-render-core/src/lib.rs:693-733`).
- **Simpler shape:** paint `DocumentPainter` with `RegionProjection` straight
  into the adapter painter, as the reader's page path already does, and expose
  content height as a view query. `RenderScene`, `RenderCommand` and
  `SceneRecorder` can then go; `RecordingPainter` stays for diagnostics. The
  resource lease `RenderScene` carries is already duplicated by `GpuiScene`
  retaining its own entries.
- **Performance:** one fewer command list and no byte copies per fragment
  relayout.

### 5. Four stylesheet caches, and the outermost one costs time

- **Where:** `session.rs:1036-1109`,
  `crates/html-pipeline/src/parser/html.rs:256-291, 580-589`.
- **What:** `RevisionCachedResourceProvider` clones the byte `Vec` on every hit
  and on insert. `metadata()` is not cached. Text is re-decoded on each
  restyle.
- **Overlap:** `BookStylesheetCache` (which no production code installs),
  `StylesheetCache` and the factory-local program cache sit behind a three-way
  branch.
- **Simpler shape:** one session map `(uri, encoding) -> Arc<str>` cleared on
  revision change, and one `Arc<Mutex<StyleProgramCache>>` that is private
  unless the host shares one.
- **Performance:** a font-size or media restyle stops copying and decoding
  every sheet.

### 6. `DocumentFactory` mirrors `PipelineInputs` through setters

- **Where:** `session.rs:360-374`, `parser/html.rs:75-134` and `313-383`,
  `crates/02-html-parse/src/parser.rs:6`, `crates/02-html-parse/src/dom.rs:11`.
- **What:** four input fields are copied into the factory on every update. Its
  entry points are duplicated on `ResourceDocumentFactory`.
- **Parse types:** `ParsedHtml` and `ParsedDomDocument` are now the same thing
  (`build_dom` is a clone), which forces the three-arm match at
  `session.rs:326-353`.
- **Simpler shape:** one parse result type and one
  `prepare(dom, encoding, &inputs, resources, &mut caches)` function.

### 7. A free-function layer parallel to methods

- **Where:** `crates/html-pipeline/src/stages_output/mod.rs:47-49, 104-163`,
  `stages_output/fragments.rs:262-325`.
- **What:** about 230 free `pub fn`s across the query and model crates sit
  beside methods on the same types, for example
  `geometry_queries::render_geometry_view_line_vertical_bounds(geometry_queries::render_view_geometry(view), line)`.
  They leave empty `impl` blocks and nine module aliases
  (`fragments as fragment_paint`, and so on).
- **Fix:** make them methods and control visibility through the `engine`
  re-export. No runtime effect; it roughly halves the vocabulary.

### 8. HTML is parsed into two trees

- **Where:** `crates/02-html-parse/src/html.rs:79-94` versus `xml.rs:88`.
- **What:** html5ever builds a `scraper::Html`, which is then walked into the
  engine DOM. The XML path streams directly into `DocumentBuilder`.
- **Option:** a `TreeSink` over `DocumentBuilder` would be single-pass and drop
  `scraper` and `ego-tree`. It needs reparenting support in the builder, so it
  is the largest item here and only worth doing if parse shows up in profiles.

## Small performance items

- **Colour runs on resize:** `rebuild_paint_colors` runs on every relayout
  (`lifecycle.rs:1347`, `2048`). The runs depend only on the palette and
  first-line ranges, so a width-only relayout rebuilds an identical vector.
  Only `prepare_text_paint` needs to rerun.
- **Environment lookups per paint:** `paint_commands` calls `std::env::var_os`
  twice per scene paint (`adapters/html-gpui/src/paint.rs:657`); cache it in a
  `OnceLock`.
- **Repeated line lookups:** the scalar text path re-fetches the line, its
  placement and `paint_resources_valid` per fragment
  (`crates/05-html-render-core/src/lib.rs:402-413`); pass the `RenderLine`
  down.

## Suggested order

1. **#4 and #5** are self-contained and should each be a direct speedup.
2. **#1** removes the most session code.
3. **#3, then #2.**
4. **#6 and #7** are mechanical.
5. **#8** only with profile evidence.

## Coverage

### Read (non-test code)

- `html-pipeline`: `session.rs`, `types.rs`, `stages.rs`, `lib.rs`,
  `stages/lifecycle.rs` up to line 2100, `parser/html.rs` lines 26-595,
  `stages_output/mod.rs`, `stages_output/fragments.rs` up to line 330.
- `05-html-render-core`: `document.rs`, `lib.rs` up to the test module.
- `html` facade: `lib.rs`, `facade.rs`, `configuration.rs`, `revisions.rs`,
  `engine_load.rs`.
- `02-html-parse`: the entry points in `html.rs`, `parser.rs`, `dom.rs`, and
  the start of `xml.rs`.
- `adapters/html-gpui`: the layout path of `fragment.rs` and the command
  definitions and replay entry of `paint.rs`.

### Outlined only (signatures, not bodies)

- `html-pipeline`: `stages/paint.rs`, `stages_output/text.rs`, `boxes.rs`,
  `geometry.rs`; the tail of `lifecycle.rs` (memory reports, note scoping,
  `DocumentRead` accessors).
- `html-engine-model`: the `GlyphShaper` trait and `glyph_resources.rs`.
- `html/src/engine_resources.rs`.
- The rest of `adapters/html-gpui/src/paint.rs` and `fragment.rs`.

### Not reviewed

- **DOM:** `html-dom` (compact storage, `document/dom.rs`, CFI, columns).
- **Text preparation:** `html-text` (normalization, line breaks, first-letter,
  pseudo styles).
- **Box construction:** `html-format` (`box_builder/*`, `list_marker.rs`,
  `mathml.rs`).
- **Shaping execution:** `html-shape` (`execution.rs`, `spans.rs`).
- **Engine model:** most of `html-engine-model` (`shaping_contracts.rs`,
  `layout_state.rs`, `text_fragments.rs`, `glyph_metrics.rs`,
  `caret_positions.rs`, `bidi.rs`, `layout_tree.rs`, `paint_program.rs`).
- **Resources:** `html-resources` (`resources/images.rs`, the image service and
  loader).
- **Pipeline queries:** `stages_output/addressing.rs`, `search.rs`,
  `reading.rs`, `markdown.rs`, `pagination.rs`, `semantics.rs`, `source.rs`,
  `words.rs`, and `stages/semantics.rs`.
- **Facade services:** `geometry.rs`, `selection/*`, `pagination/*`
  (including `composition.rs`), `structure.rs`, `search.rs`, `scoped.rs`,
  `export*.rs`, `overlays.rs`, `targets.rs`.
- **Parse internals:** the bodies of `xml.rs`, `encoding.rs`, `text.rs`,
  `metadata.rs`.
- **GPUI adapter:** `text.rs` (native shaping and caches), `fonts.rs`,
  `fragment_selection.rs`, `math_svg.rs`, `raster_theme.rs`, `svg_theme.rs`,
  and the `html-fonts` and `gpui-image-eviction` crates.
- **Other:** `mathema`, `html-style-model`, `tools/*`, `test-support`, and all
  test modules.
