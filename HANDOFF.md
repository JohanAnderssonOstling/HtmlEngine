# Software design and API cleanup handoff

Updated 8 November 2026. The requested cleanup and final source review are complete. The application boundary is an opaque `html::Document`; consumers submit input changes and use published services rather than manipulating pipeline state. Live reader and application consumers were migrated with the API changes.

This handoff records intended boundaries, implementation changes and checks.
The current resource/interaction cleanup is source-checked only. Earlier release
checks apply only to the revisions explicitly identified in the history below;
they do not establish type correctness of later changes. Compilation, tests and
benchmarks were not run for the current cleanup. Runtime behavior and performance
remain unmeasured.

The current contracts below and the final dated entry supersede older dated
entries. Earlier entries are a chronological record, including APIs since removed.

## Working rules

Follow [AGENTS.md](AGENTS.md) and any applicable instructions in sibling repositories.

- Never run debug-profile builds. When compilation is authorized, pass `--release` (the user's instruction; debug caches filled /home).
- Do not compile unless the user explicitly asks. This includes `cargo check`, `cargo build`, and direct `rustc` commands.
- Do not run or create tests unless explicitly asked. Requests to implement, review, fix, optimize, or validate do not authorize them.
- Do not introduce production APIs or helpers solely to support tests. Migrate existing fixtures or remove obsolete assertions when contracts change.
- Prefer Rust `use` imports over repeated qualified paths where the surrounding style allows it.
- Preserve performance. Avoid extra buffers, copies, allocations, backend calls, or repeated stage work when changing API boundaries.
- Use types to express valid states. Do not add runtime numeric validation for trusted backend values; preserve necessary source, file, and input checks and internal invariants.
- Migrate production consumers rather than adding compatibility aliases. Expose only what HtmlViewCore uses through `html`; broader native services belong to implementation crates.
- Do not modify `../GPUI-Fork` internals or prototype vendor snapshots.
- Do not delegate to subagents without explicit authorization.

The shared workspace contains substantial existing changes and untracked artifacts. Do not reset files to HEAD or attribute the entire diff to this cleanup. Snapshot current contents before editing and isolate changes against those snapshots.

## Repository boundaries

| Location | Responsibility |
| --- | --- |
| `crates/html` | Application facade, opaque document lifecycle, content and geometry services |
| `crates/html-pipeline` | Incremental stages, committed inputs, caches, resource transactions |
| `crates/html-engine-model` | Backend shaping contracts, geometry and resource identities |
| `crates/05-html-render-core` | Scene recording and replay |
| `adapters/html-gpui` | Native shaping resources, font integration, GPUI painting and fragment widget |
| `../HtmlViewCore` | Reader navigation, host placement and reader GPUI integration |
| `../Html-Prototype/apps/components` | Application font-loading callback and other host integration |
| `tools/embed-bench/gpui` | A live consumer of the adapter recording and replay API |

Package names and Rust import names can differ. For example, the facade package is described as Skrift in its README while source consumers use `html`.

## Design decisions to preserve

The cleanup follows the principles of deep modules, information hiding, fewer overlapping ways to perform an operation, and types that keep related state together.

Applications supply reader overrides through document options, and viewport, typography and note-flow changes through document configuration. Source replacement belongs to native pipeline consumers. The engine owns style preparation, intermediate representations, stage selection, caches, and published layout. There is no public `html::engine` module or `Engine` alias exposing mutable internals.

The engine owns measured geometry and logical glyph identities. Adapters own native glyphs, shaped runs, native caches, and paint handles. Text backends receive a unified shaping style whose defaults are resolved by the engine. The host does not negotiate a separate advanced-feature capability protocol; whole-run shaping supplies the advances and cluster geometry that layout actually uses.

The engine owns pagination, page content placement, selection interpretation and link hover. The reader owns navigation, host display transforms, annotation storage and activation, and persistent addresses. The facade retains the geometry, selection, search, target, scoped-layout and export operations used by HtmlViewCore. Font preparation and broader content services remain in native implementation crates. Diagnostic tools may depend directly on implementation crates. Internal visibility at the application boundary does not require deleting those implementation services.

Do not reinstate unused table-selection inspection queries. The user explicitly declined those APIs. Preserve the table-hit and selection behavior required by current consumers.

## Recent changes

### Document and pipeline operations

`Document` owns a private `PipelineSession`; its application-facing updates use `DocumentUpdate` and return `Update::{Unchanged, Paint, Layout { anchors_preserved }}`. The engine chooses the necessary stages.

Pipeline operations now return `EarliestStage` directly. Removed `PipelineUpdate`, `ReuseReport`, and `Reuse`, whose extra reporting duplicated information derived from that stage. Existing assertions that checked only the duplicate projection were removed; substantive stage, shaping, and rendering assertions were retained.

Removed `OwnedActivePipelineSession`, `into_active`, and unused forwarding queries on the borrowed active wrapper. `PipelineSession` remains a useful implementation service. Its borrowed `attach` operation provides backend-bound updates, glyph rehydration, and note layout; session queries stay on the owning session.

Shaping validity is one optional cache entry containing both its key and `GlyphResourceGeneration`. Invalidation clears that record while retaining the last published geometry for queries and failure recovery.

Media definitions and their activation key likewise occupy one optional cache entry. Updates reuse the retained key when preparation is still valid. Changed media activation or root typography selects preparation before publication. Removed the redundant final media-key recomputation and the unused public prediction method `media_environment_change_affects_style`.

Primary files: [session.rs](crates/html-pipeline/src/session.rs), [types.rs](crates/html-pipeline/src/types.rs), [configuration.rs](crates/html/src/configuration.rs), and [facade implementation](crates/html/src/lib.rs).

### Opaque caret and render storage

`CaretPositions` is a transparent public wrapper around private dense or derived storage. Consumers use its construction and query operations rather than matching storage variants. `from_advances` accepts existing positions, compacts dense input using the established algorithm, and retains already-derived input without materializing it. Advances are shared only when the compact representation needs them.

`RenderCommand` is private and `RenderScene::commands()` was removed. Scenes retain their existing shared command storage and paint through the established replay interface. `RecordingPainter` remains available as a useful diagnostic service.

Primary files: [caret_positions.rs](crates/html-engine-model/src/caret_positions.rs) and [render core](crates/05-html-render-core/src/lib.rs).

### Native resources and painting

The backend owns native cache entries and returns either borrowed handles or
retained entry references. The engine owns document-local glyph registrations
and their identity; native generation identifies the backend namespace.

| API | Contract |
| --- | --- |
| `GpuiCommandPainter::new(images)` | Record content with image support |
| `GpuiCommandPainter::commands_only()` | Record content without an image cache |
| `GpuiCommandPainter::reusing(scene)` / `finish()` | Reuse a previous command buffer and return a scene |
| `GpuiScene::paint(bounds, window, cx)` | Replay valid commands with retained native entries |
| `GpuiTextCache::reset()` | Caller-controlled cache clearing and native generation change |

Scenes retain the entries they use rather than snapshots of the caller's cache.
Each scope owns its registration ledger. Its scenes retain that ledger and a
parent validity lease, so scenes can outlive the scope wrapper but reject parent
replacement. Engine checkpoints affect registrations only; no native checkpoint
hooks remain. The GPUI one-character run cache is bounded at 4,096 entries with batch eviction.

`GlyphShaper` has five methods (two required: resource generation and run shaping). Do not reintroduce raw registry
getters, caller registration coordination or traits used only to hide stages.
Primary files: [text.rs](adapters/html-gpui/src/text.rs), [paint.rs](adapters/html-gpui/src/paint.rs), [fragment.rs](adapters/html-gpui/src/fragment.rs), `../HtmlViewCore/adapters/html-view-gpui/src/view.rs`, and [benchmark consumer](tools/embed-bench/gpui/src/skrift_content.rs).

### Font loading

The host callback now receives one borrowed typed request:

```rust
pub enum FontLoadRequest<'a> {
    Family(&'a str),
    TextFallbacks(&'a str),
}

pub type FontLoader =
    Rc<dyn Fn(&WindowTextSystem, FontLoadRequest<'_>) -> bool>;
```

For `Family`, return whether the named family is available. For `TextFallbacks`, return whether new fonts were registered. These boolean meanings are deliberately different and documented. The request replaces the old optional-family and text argument combination without allocating request strings.

The production callback in `../Html-Prototype/apps/components/src/fonts.rs` matches the two variants. Family loading, text fallback loading, native CJK handling, wasm handling, cache refresh, and invalidation behavior were retained. HtmlViewCore reexports `FontLoadRequest` alongside `configure_font_loader` so hook consumers can name the variants.

Keep the useful font preparation service, including local and decoded sources, authored fallback order, data URLs, and SFNT/WOFF/WOFF2 support. The host still owns network access, native registration, and caches. CSS font palette customization was deliberately removed from the supported shaping contract.

### Hit testing and selection

Private `PaintedHit` represents the winning text or image hit as one enum. It replaces two independently optional internal results. Both hit queries still execute once; text wins ties in paint rank. Public `ContentHit` continues to permit a link or table hit alongside the winning painted content.

`SelectionState::is_selecting()` replaced the exposed interaction snapshot. `finish_interaction()` returns a caret only for a collapsed active gesture; the host decides whether to activate a link there. `TableSelectionState::is_in_selected_table` intentionally includes cells outside the current selection rectangle when they belong to the same selected table.

Primary file for the hit change: [geometry.rs](crates/html/src/geometry.rs). Reader gesture behavior and the public combined-hit contract were preserved.

### Design alignment pass (3 November 2026)

- `crates/mathema` is the separate Apache-2.0 `skrift-mathema` package behind `math-export`.
- `html_pipeline` publishes engine contracts once, under `engine`. Root aliases are crate-internal.
- `PipelineInputs` revisions are session-assigned. `PipelineChanges` reports `resources_changed`, `stylesheets_changed` and `fonts_changed`; replaced source advances its revision automatically. `PaintSettingsRevision` and the render-only cache key were removed. Build initial inputs with `PipelineInputs::new`.
- Validation and empty-session failures are typed `PipelineErrorKind` variants. Export returns `ExportError`.
- `Document` keeps image state in one private `DocumentImages`; `ImagePipeline` is not re-exported.
- `FragmentRenderer` was removed. In-memory markup uses `Document::from_source` and `Document::scene(SceneImagePolicy)`; render-core keeps `SceneOptions` and scene recording only.
- The facade no longer duplicates `RenderView` queries. `Targets`, `Search`, `Content` and `Reading` were removed; quote resolution, reading targets, excerpts and line text live on the pipeline views. `Selection` is the one range-to-text path.
- The GPUI adapter records into `GpuiScene`, which pairs commands with their glyph storage. `PaintCommand`, `paint_commands` and `GpuiPaintResources` are private.

- Projection and highlight results use caller-owned output buffers instead of visitor callbacks: `PaintProjection` methods append to `&mut Vec<ProjectedLine>` / `&mut Vec<PaintFragment>`, and `Geometry::highlights` appends text rectangles and highlighted cell boxes for the host to project. The reader's `TextGeometry::project_box` and `projected_box_contains` follow the same pattern. The user prefers output buffers over closures and callbacks in new or changed APIs.
- Pagination is the facade `html::pagination` module (`Document::pagination()`, `RenderView::pagination()`). `Pagination` replaces `PaginationLines`; its queries return `PaginationLine` values carrying id, origin, bounds and glyph range, so pagination callers no longer need `RenderLine`.

### Typed line identities (3 November 2026)

Public line queries take and return `LineId` instead of raw storage indexes. Internal storage stays raw; views convert at the boundary. Covered: `RenderTextView` (`line`, `line_placement`, `line_text_fragments`, glyph offset/advance runs, ellipsis/hyphen, overflow clip, prepared runs, `paint_order_rank`), `RenderGeometryView` caret/hit/bounds/selection queries, `RenderFragmentView` (`images_for_line`, `decorations_for_line`, `line_paint_rank`, `paint_order_for*`, `RenderPaintItem::Line`), `RenderBoxView::{contains_line, fragmentation_distance}`, `content_line_range -> Range<LineId>`, `source_location_on_line`, `text_for_lines`, `request_images_for_lines`, facade `TextHit`/`RangeRect`/`text_intersections`, render-core `LinePlacement`, `PaintCandidates` and `PaintProjection`, and HtmlViewCore `VisibleLinePositions`. Renamed `line_index_for_glyph` to `line_for_glyph`; image/decoration `line_idx()` to `line()`; removed `RenderLine::index()` in favour of `id()`. Tests iterate identities through the existing `spatial_order()` rather than a test-only helper.

Spatial ranks and pagination cursors deliberately remain `usize`: `start_line_at`, `end_line_at`, `line_intersecting_vertical_range`, `spatial_rank`, `Pagination` cursors, and `StructureView::{first_line, before, after}` / `trigger_line`. These are positions in a traversal, not identities.

The typing exposed cursor/identity confusion, now fixed: `StructureView::authored_boundaries` resolved `trigger_line` (a pagination cursor) through `text().line()`; it now uses `pagination().at()`. Several tests likewise used spatial ranks or cursors as line IDs and were corrected.

### Typed image identities (3 November 2026)

`ImageId` lives in `html-dom` beside `ImageResource` and is re-exported as `html::document::ImageId` and `html_pipeline::engine::ImageId`. It replaces raw `u32` image indexes in `DecodedImages::get`, `ImagePipeline` (`failure`, `retry_failed`, `ensure_window`, `intrinsic_dimensions`, `ImagePipelinePoll::dimensions_changed`; its maps are keyed by `ImageId`), `RenderImageFragment::image()` (was `image_idx()`), `RenderView::{image_mathml, image_uri}`, `RenderExportView::is_equation`, `Painter::draw_resource_image`, `RecordingPainter::resource_images`, facade `ImageHit.image`, `Geometry::image_at_lines`, `ExportRequest::{Image, EquationLatex}`, `Document::image_source_request`, and HtmlViewCore `DocumentImage.image` / `TextGeometry::hit_test_image`. `ImageMetrics` and layout storage stay raw and convert with `ImageId::raw()`.

### Typed box identities (3 November 2026)

`BoxId` lives in `html-engine-model` beside `LineId` and is re-exported as `html::layout::BoxId` and `html_pipeline::engine::BoxId`. Pipeline view methods take `BoxId` (every `RenderBoxView` box query, `RenderSemanticsView`, `link_for_box`, `markdown_for_box`, `only_box`). Each converts once with `box_id.index()`; internal storage and private helpers stay raw. Accessors were renamed as they were typed: `RenderLine::owner_box()`, `RenderDecoration::owner_box()`, and `box_id()` on text runs, image fragments, tables and table cells. `parent`, `ancestors`, `nearest`, `block_container` and `marker_box` return `BoxId`. In the facade, `structure.rs` uses `BoxId` throughout (its indexes, `StructureView` queries, `SiblingPair`, `TableGroup.table_box`), as do `TableHit`/`TableCellRange::table_box`, `ExportRequest::Table`, `ReadingLineGeometry.cell`, `Geometry::{content_at_lines, table_at_where}` predicates and highlight cell output. In HtmlViewCore: `TextGeometry::project_box*`, keep constraints, and `ReadingLine.cell`. Iterating all boxes is `(0..boxes.len()).map(BoxId::new)`; no new iteration API was added. Pagination cursors stay `usize`.

Splitting `GlyphShaper` into shaping and resource-lifecycle traits was evaluated and declined by the user: facade and reader callers pass the shaper through to the pipeline, which needs both halves, so a split adds bounds for callers and only moves boilerplate for implementors. The engine, both adapter workspaces, HtmlViewCore, and Html-Prototype's `reader-ui` and `ui-components` now pass native release checks with `--all-targets`; details follow below. The older nav-fixture and profiling-example compilation errors were migrated to current contracts. Standalone benchmark tool workspaces were not rechecked in this pass; their earlier `register_runs` and GPUI dependency limitations remain unverified.

## Verification and limits

### Release type checks (3 November 2026)

After the user's explicit compilation request, all five native checks passed:

| Working directory | Command |
| --- | --- |
| HtmlEngine | `cargo check --release --workspace --all-targets` |
| HtmlViewCore | `cargo check --release --workspace --all-targets --target-dir ../HtmlEngine/target` |
| Html-Prototype | `cargo check --release --workspace --all-targets --manifest-path ../HtmlEngine/adapters/Cargo.toml --target-dir target` |
| Html-Prototype | `cargo check --release --workspace --all-targets --manifest-path ../HtmlViewCore/adapters/Cargo.toml --target-dir target` |
| Html-Prototype | `cargo check --release -p reader-ui -p ui-components --all-targets` |

Running the adapter checks from Html-Prototype applies its existing `.cargo/config.toml` font patches and resolves the previously recorded standalone cosmic-text mismatch without modifying GPUI-Fork or vendor code. Release caches were kept local and reused. Existing compiler warnings remain. Test targets were type-checked but no tests, applications or benchmarks were executed; other platforms were not checked.

Compilation-driven fixes: nav fixtures drain session effects through the public API; overlay assertions use `visible`/`reading_target`; removed obsolete source-pointer assertions while retaining selection/geometry/revision checks; the ruler's existing fixture uses `BoxId` through a direct engine dev-dependency; the profiling example allocates `GlyphId` values. Removed empty `select_note_glyph_document` remnants and their call site. No production inspection APIs or new test helpers were added.

### Earlier source review

The final review parsed 18 Rust files across the latest changes and their consumer migrations using `rustfmt --emit stdout`. Parsing succeeded without rewriting those files. The facade named-import audit returned no findings. A source search found no remaining live references to the specific removed APIs listed above.

Scoped `git diff --check` passed for the reviewed files in HtmlEngine, HtmlViewCore, and Html-Prototype. A repository-wide check also reported two whitespace issues outside that scoped set: `tools/pipeline-bench/src/main.rs:23` and `tools/pipeline-bench/src/opening_deep.rs:7`. They were left untouched.

No compilation, tests, or benchmarks were run during the final cleanup review. Syntax parsing and named-import searches do not establish Rust type correctness, runtime behavior, or measured performance. Source comparisons checked preservation of important algorithms, resource handoffs, and allocation patterns; they are not a performance guarantee.

## Remaining considerations

There is no unfinished implementation identified by the final source audit. Do not automatically begin another API reduction pass. Future verification should continue to cover the engine and sibling consumers together; the font callback and recording/replay boundary cross repository boundaries. Tests still require an explicit request.

Publication fonts are selected by the engine (3 November 2026; release type-checked). `FontFace` and its descriptor types now live in `html-style-model`; `html_style::FontFace` and `html::text::FontFace` re-export them, and `html-style` converts rules with `font_face_from_rule`. `GlyphShaper::select_publication_fonts(faces, overrides)` (default no-op) is called by `with_document_shaping` and `with_append_shaping` before every shaping pass. Each `PreparedInputs` carries its document's `PublicationFonts` (declarations plus the reader overrides it was styled with) as borrowed `Storage`. The factory attaches them through `FormattingDocument::with_publication_fonts`; restyling's `resolve` closure returns `(ComputedStyles, Arc<[FontFace]>)` and publishes the new declarations only on success; scoped notes borrow their document's set. Link targets and notes from other spine items therefore shape with their own faces without any reader coordination.

`GpuiTextCache` implements the trait method and owns a crate-private `PublicationFonts`; `SharedPublicationFonts` (`Rc<RefCell<…>>`) is gone. Active faces are stored with their keys, selection borrows the retained faces without allocating, and changes clear the font caches at the next shaping transaction. `register_publication_font(loaded)` registers a loaded face. In HtmlViewCore, `RendererHost::set_font_overrides` and the GPUI host's `FontSelectionChanged` message were removed. `prepare_font_faces(faces, overrides, provider)` only loads faces. The view registers `FontReady` faces through `RendererSession::glyph_shaper_mut()` and then issues `RefreshFonts`. `GpuiHost` lost its `fonts`, `font_overrides` and `document_fonts` `RefCell`s and its `glyph_document` `Cell`; `font_requests` (I/O de-duplication) remains. Two GPUI font tests lost assertions on removed host state; their behavioural assertions remain.

### Renderer effects as data (3 November 2026; release type-checked)

Continued the chosen functional-core / sans-I/O renderer design in HtmlViewCore.
The effects/services definitions were already present at the start of this pass;
the remaining production consumers still referenced the removed `RendererHost`.

- `RendererCore` owns `RendererEffects` and `RendererServices`; it appends repaint/restyle flags, events, clipboard payloads and publication font-load requests. `RendererSession::drain_effects(&mut out)` transfers pending effects into a caller-owned reusable buffer. Drain after construction and every operation, including viewport/frame preparation. Repaint requests coalesce.
- Worker spawners and the optional resource waker are supplied once through `RendererServices`. Asynchronous image reads, chapter preparation and font loading retain their channels and completion wakeups. Popup note width is an explicit `RendererSession::set_note_popup_width` input.
- Methods that produce clipboard/events now take `&mut self`. Copy methods queue owned payloads; platform clipboard errors belong to the shell, while renderer export failures still emit `OperationFailed`. Existing recording-host fixtures were migrated to effect assertions; obsolete host clipboard-failure assertions were removed. No tests were added or run.
- GPUI interprets effects in one `flush_effects` method, called after input operations, background completions and appearance updates, and at frame boundaries. Repaint/restyle signals notify the view synchronously. The `Repaint`, event and clipboard message round-trips, shared `Rc<GpuiHost>`, `repaint_pending`, unused timer scheduling and the note-width `Cell` are gone. `GpuiHost` is owned view state and its font-request set is a plain `HashSet`. `HtmlView::document_origin` is a plain field. Clipboard and selection-handle view methods now take their GPUI context; live Html-Prototype consumers and the popup example were migrated.
- The harness owns its event state and clipboard directly and drains after window input/frame handling. Removing `set_glyph_document` required resolving its shaping-context dependency: one book-wide glyph registry now provides stable handles for both page and cross-document note painting. Document/append transactions retain length checkpoints and truncate appended resources on rollback; explicit reset replaces the registry and generation. The raster cache was subsequently moved into separate mutable paint state; see the source-only follow-up below.
- The navigation and paint components receive mutable effect buffers instead of host callbacks. No `RendererHost`, `RecordingHost`, `TestHost`, `ActiveDocument` or `set_glyph_document` source references remain in the migrated reader tree.

The initial source migration preceded compilation authorization. The subsequent explicitly requested release checks now cover publication fonts, effects, adapters and live application consumers. See the command matrix above. Runtime behavior and measured performance remain unverified.

### Explicit mutable ownership (3 November 2026; source-only)

The interior-mutability survey led to five ownership changes after the release checks above:

- The hyphenation dictionary cache uses `Option<Box<[DictionaryEntry; 256]>>`, replacing the intermediate `OnceCell`/`Cell` representation. The layout executor allocates the table lazily and updates slots through mutable access. Word eligibility, keys, hashing, matching and break emission are unchanged.
- Layout timing collectors, counters and profiling hooks were subsequently removed (10 November 2026). Use external `perf` sampling and benchmark-level elapsed times.
- CSS `ResolutionContext` owns ordinary viewport-dependency and line-height fields. Computed-value conversion receives mutable context. Cached parsed spacing values travel separately as immutable input, so canonical font-size conversion no longer clones their `Arc`.
- Knuth–Plass tracking admission takes `FnMut`; its lazy tracking index and last-range cache use ordinary `Option` locals.
- The harness reader owns a `RasterCache` and lends it mutably to `BufferPainter`, alongside the shared `FontShaper`. Page, note and interface text reuse the same cache across frames without a `RefCell` in the shaper.

Rust syntax parsing and scoped whitespace checks cover these edits. Compilation, tests and benchmarks were not run for this follow-up; the earlier release matrix does not cover it. Shared resource synchronization, lazy immutable indexes and fonts, profiling thread-locals, and the web font-request map remain. The application's bundled-font callback bridge also retains its shared callback state.

### Comprehensive review fixes (3 November 2026; source-only)

The subsequent review identified resource-identity, failure-recovery, popup-image,
and allocation issues. The implemented fixes cross the engine, reader, GPUI
adapter, harness, and live application event consumers:

- Replacement parsing remaps inherited image metrics before shaping. Dimensions
  survive only at slots still naming the same source under the same provider
  resource revision and base URI. Changed sources use their own natural sizes;
  provider-resource replacement discards previous decoded dimensions. Metric
  snapshots and layout keys commit only after successful layout.
- Synchronous loading helpers, renderer construction, and GPUI `from_provider`
  return `Result`. Chapter-navigation acquisition reports `OperationFailed` and
  retains the active page on preparation or shaping failure. Existing fixtures
  and examples unwrap their controlled construction results; no tests were added.
- Note-display policy and viewport settings commit after successful engine
  updates. Deferred progress navigation carries the requested fraction through
  chapter preparation rather than applying it to the old document.
- Font-load completion includes its request key and success/failure. Loading or
  native-registration failure releases that key, allowing a later request to
  retry; successful registration remains deduplicated.
- `Document::request_images` accepts IDs from its catalog, including images in
  scopes. Open notes retain their image demand and popup placement. Same-document
  demand is combined with page demand; cross-document owners request and poll
  independently. Completed dimensions refresh scopes through
  `ScopedContent::refresh_resources`, preserving logical selection within the
  same canonical preparation. Popup refresh emits the existing `FootnoteOpened`
  event so the shell replaces its scene. Page image relayout waits while a
  cross-document popup owns the native backend and is accepted when it closes.
- `BookSearchResults` retains shared immutable batches. Full search snapshots
  share their payloads, and `SearchResultsAppended` sends only the new batch.
  The application retains those batches instead of cloning excerpt/CFI strings.
- GPUI append rollback uses registry lengths and a lazy undo list for recolored
  prefix runs. Starting an append no longer pins both registries and forces
  their vectors to copy. Existing retained snapshots still use copy-on-write.
- `OverlayScratch` belongs to the renderer's reusable frame scratch; page overlay
  rectangles, cell IDs and projected fragments retain their capacity across
  paint operations. Input ranges stream directly into geometry.

These changes have not been compiled or exercised. Source parsing, caller
searches, and scoped whitespace checks are the authorized verification for this
pass. The older release matrix does not cover these changes.

Retain combinations that encode legitimate states. In particular, the four combinations in decoded-resource/layout polling and in previous-document/append-checkpoint transaction state are meaningful. Optional source-position arguments also have valid combinations. Do not replace these with restrictive enums without tracing actual callers and transitions.

### Review simplification (3 November 2026; source-only)

The follow-up removes 42 physical production Rust lines, including 17 lines of
outdated popup-scene comments. `BookSearchResults` stores each batch's exclusive
end and derives its total length, retaining binary-search lookup and shared
payloads without a separate length field. Open notes rely on the engine's
canonical preparation and resource checks instead of also storing a reader
document index. Image polling handles page decode invalidation once and keeps
the cross-document popup's deferred page relayout. Cross-document image demand
uses its owner directly. Public APIs and the review fixes are retained.

Source parsing and scoped whitespace checks cover this follow-up. Compilation,
tests and benchmarks were not run.

A second pass removes another 50 physical production Rust lines. Note previews
share one fallback path, overlay ranges stream without an intermediate buffer,
and synchronous/background chapter acquisition share font-load effects and
shaping. Chapter activation builds its spine item once, without cloning the
column layout twice. Selection publication reads its document identity from
the current page, eliminating an internal parameter and lookup helper. Public
APIs remain unchanged. Both simplification passes total 92 removed lines; the
review fixes and simplifications together are now a net addition of 193 lines.
The second pass also has syntax and whitespace checks only.

### Thorough simplification fixes (3 November 2026; source-only)

The nine subsequent review findings are implemented. This pass removes 181
physical production Rust lines, excluding fixtures and examples:

- Reader queries use the engine's immutable `RenderView` directly; `DocQuery`
  and the two resulting unused crate dependencies are removed.
- Pagination transfers the frame's line-position buffer directly. The inert
  spare-buffer pool and its installation wrappers are removed.
- `ColumnLayout::configure_viewport` and `recompute_columns` replace the unused
  width-proposal handshake. Explicit document-width commands remain responsible
  for reflow; viewport composition never invokes the pipeline.
- The reader uses engine `SelectionState` directly and keeps link hover on the
  spine item, with the same reset points. Root font size is read from committed
  document configuration instead of mirrored in the renderer.
- Both pipeline input-update paths share finalization after their own commit
  or rollback, including native invalidation and scoped-resource retention.
- Publication-font filtering borrows declarations. Unchanged reader-font
  overrides compare borrowed strings before allocating owned values.
- Quiet image polls retain pending metrics. New decodes or invalidated decode
  identities rebuild the snapshot; failed/deferred updates retain it for retry.
- Search cursor movement and same-query batches reuse local ranges and borrow
  the selected result. Query/options changes, chapter activation and reshaping
  refresh ranges; pure layout preserves them. Empty queries skip scanning.

Existing fixtures were migrated with the removed interfaces; no tests were
created or run. All 24 changed Rust files pass syntax parsing. Named facade
imports, changed manifests/lockfiles, and scoped whitespace checks pass.
Compilation and benchmarks were not run; the older release matrix does not
cover these changes.

### Follow-up review fixes (3 November 2026; source-only)

The next four review findings are implemented:

- Successful updates that invalidate glyph identities re-resolve annotations
  from their source anchors. Paint-only changes and layout with preserved
  anchors retain their resolved ranges.
- Search refresh rebuilds both local ranges and reported results for
  current-document searches, clamps the result cursor, and emits fresh results
  and counts after preparation or chapter changes. Whole-book searches retain
  their publication results and only refresh the visible projection. Initial
  result installation emits one snapshot and does not navigate.
- GPUI retains display keys together with its recorded base and overlay scenes.
  Composition records only layers whose keys changed, retaining native resource
  snapshots with reused scenes. Page turns and ruler corrections update the
  same cache as ordinary frame preparation.
- `Document::font_faces`, `PreparedDocument::font_faces`, and the pipeline getter
  return shared immutable `Arc<[FontFace]>` snapshots. Font-load effects share
  those declarations and capture only the allocation-free publication-font
  eligibility decision instead of cloning reader overrides. Eligibility is
  defined by `ReaderStyleOverrides::uses_publication_fonts` and used by native
  shaping and effect loading. Requests still occur at every previous update
  point; failed load or registration keys are still released for retry.

All 11 changed Rust files pass syntax parsing. Named facade imports and scoped
whitespace checks pass. No tests were added or run; compilation and benchmarks
were not run. These checks do not establish type correctness or runtime behavior.

### Release checks after follow-up fixes (3 November 2026)

After the user's explicit compilation request, the five native release checks
listed in the earlier release matrix were repeated and all passed. This covers
the HtmlEngine and HtmlViewCore workspaces, both adapter workspaces, and the
reader's `reader-ui` and `ui-components` packages, including all their targets.
The adapter checks again used Html-Prototype's existing font patches and local
release caches. Only one Rust compilation ran at a time.

Compilation exposed and corrected a leftover `html_view_doc::` prefix before
`self.page.render_view()` in navigation-anchor updates, a missing `ImageMetrics`
import in an existing pipeline fixture, and two existing fixture calls to the
removed `SpineItem::doc()` wrapper. The fixtures now use `render_view()`.
No production inspection APIs, test helpers, or new tests were added.

Compiler warnings remain. Test targets were type-checked; no tests, applications,
or benchmarks were executed. This verifies native type correctness through
these fixes, not runtime behavior, performance, or other platform targets.

### GPUI cache ownership fixes (3 November 2026; source-only)

- Fragment layout validity is separate from its paint color. Color-only changes
  re-record the scene without applying a document update or clearing selection.
  Source, style, and final-width changes keep the existing selection reset
  behavior, including preserving selection when layout probes return to the
  same final width. Width-only updates borrow retained source and styles.
- Reader composition moves cached scenes into the next composition. Unchanged
  layers retain their commands and native resources; changed layers pass their
  previous scene to `GpuiCommandPainter::reusing`. Exclusively owned command
  buffers retain capacity, while shared snapshots remain immutable and force
  a fresh buffer. Key comparison still follows viewport preparation.
- Note selection geometry uses retained rectangle and cell buffers in
  `FrameScratch`. Buffers are cleared before each pass and restored even when
  no valid note or selection is present. `paint_note_selection` now takes
  mutable session access to reuse this owned scratch; paint order, placement,
  colors, and canonical scope checks are unchanged.

All five changed Rust files pass syntax parsing. Named facade imports and scoped
whitespace checks pass. Existing fragment fixtures were migrated to the nested
layout key; no tests or test helpers were added. Compilation, tests, and
benchmarks were not run for this pass; the previous release matrix predates it.

### Fragment failure recovery and shared styles (3 November 2026; source-only)

- A fragment's cached key, scene, and height live in one `FragmentDisplay`.
  Both measurement and final-width layout use the same state-owned failure
  cleanup: discard the display result, reset selection, and retain the document
  and shaper for retry. Success publishes all display metadata together.
- Hitboxes and painting require a display result. Mouse and keyboard callbacks
  obtain selection and document geometry through `selection_context`, which
  also requires that result. Previously registered callbacks cannot select or
  copy a retained document after a layout failure. Enabling selection no longer
  invalidates an otherwise current display.
- Stylesheet text is frozen once into `Arc<[String]>` for each measured-layout
  callback. Measurement probes, prepaint keys, and selection keys share it;
  owned stylesheet copies are made only when submitting them to the engine.
  Key equality still compares stylesheet contents, preserving cache hits for
  unchanged CSS even when the shared allocation differs.

The changed Rust file passes syntax parsing; named facade imports and scoped
whitespace checks pass. Existing fixtures were migrated to the grouped display
state and shared stylesheet type. No tests or test helpers were added.
Compilation, tests, and benchmarks were not run for this pass; the previous
release matrix predates it.

### API surface cleanup (3 November 2026; source-only)

- The reader root exports an explicit list of session, configuration, effect,
  command, event, search, address, and paint-cache contracts. Navigation,
  selection, pagination, viewport, and overlay machinery remains available in
  its component crates and is no longer automatically re-exported. Internal
  renderer imports are private. The root still exports `Color` for palette
  construction, including when it is also used by internal note scratch.
- Scalar session setters, search activation, and match stepping use `apply`,
  sharing command dispatch and navigation revision bookkeeping. Result-bearing
  navigation and borrowed search queries retain direct methods to preserve
  their results and avoid temporary command-string allocations. The exact
  `restore_position_from_cfi` session alias is removed; `set_cfi_position` is
  the canonical public restore operation. Lower-level anchor restoration
  methods remain distinct and unchanged.
- `BoxTextFormat<'a>::font_family` is `Option<&'a str>`, resolved through the
  document's style storage without allocation. The generic style-string lookup
  and `StyleStringId` facade export are removed. Font-format inspection remains
  available. The embed benchmark and existing application fixture read the
  returned family directly.

All nine changed Rust files pass syntax parsing. Named facade and reader-root
import audits and scoped whitespace checks pass. Existing fixture imports and
format queries were migrated; no tests or test helpers were added. Compilation,
tests, and benchmarks were not run for this pass; the previous release matrix
predates these API changes.

### Session API reduction (3 November 2026; source-only)

`RendererSession` now declares 86 public methods and associated functions,
down from 101. Removed 14 convenience entry points for position, document
stepping, scalar settings and relative adjustments, search activation/toggling,
and search match stepping. Hosts use the existing `RendererCommand` variants
through `apply`; no new commands or forwarding types were introduced. Relative
settings derive the next value at the host. The duplicate `root_font_size`
getter is removed; read `render_view().root_font_size()` instead.

The GPUI view, standalone reader harness, and existing fixtures use the canonical
operations. Navigation revision bookkeeping and effects remain in the existing
command path. Result-bearing navigation, borrowed search input, reader projection,
narration navigation semantics, and separate resource/frame preparation operations
retain their direct APIs. The GPUI view's UI-facing helpers still flush effects.

All four changed Rust files pass syntax parsing. The reader-root import audit
covers 33 consumer files without unresolved named root imports. Source searches
found no remaining calls to the removed session methods; scoped whitespace checks
pass. No tests or test helpers were added. Compilation, tests, and benchmarks were
not run; the earlier release matrix predates this pass.

### Reader operation consolidation (3 November 2026; source-only)

`RendererSession` now declares 79 public methods and associated functions,
down from 86 in the preceding pass (101 before both reductions).

- Table copying uses `copy_table_at(position, TableExportFormat)` through selection,
  core, session, and GPUI. The application uses the same engine enum instead of
  its duplicate `TableCopyFormat`. Markdown, unstyled HTML, authored HTML, point
  projection, handled results, and clipboard effects retain their prior paths.
  Copying the selected cells remains separate.
- `RendererCommand::NavigatePage(i8)` and `NavigateLine(i8)` dispatch one step by
  sign. Zero does nothing, including leaving manual navigation revision unchanged.
  The session's four next/previous wrappers and wheel-direction helper are removed.
  GPUI and harness hosts translate wheel direction into commands. GPUI page turns
  retain synchronous composition, movement reporting, ruler updates and effect
  flushing. Narration navigation remains distinct from manual commands.
- Reading queries share the private visible-position calculation. Full-document
  fragments append the bounded prefix, record the starting index, then append
  onward fragments into one output buffer. The prefix walk stops at the visible
  glyph instead of building and discarding a suffix. Word-aware page boundaries,
  pending-document readiness and the pre-viewport source-range fallback remain.

All ten changed Rust files pass read-only `rustfmt` syntax parsing. Reader-root
and engine-facade named-import audits pass; source argument checks cover all four
migrated table-copy calls, and command dispatch covers all 36 variants. Searches
found no stale removed APIs, and scoped whitespace checks pass. Existing fixtures
were migrated and an obsolete assertion on the removed wheel helper was deleted;
no tests or test helpers were added. Compilation, tests and benchmarks were not
run. The earlier release matrix predates this pass. An optional tree-sitter
traversal audit crashed; the syntax checks above were completed with `rustfmt`,
and argument/dispatch checks use a separate source scanner.

### Reader API boundary fixes (3 November 2026; source-only)

`RendererSession` now declares 73 public methods and associated functions,
down from 79. Removed the borrowed search setter, three overlapping content
boolean queries and the three public origin/conversion methods. Added one
combined `content_at` operation. The existing `SetSearchQuery(String)` command
moves its query into search state without cloning it. Legacy query navigation
and modern `SetSearch` behavior remain distinct.

- `DocumentGlyph` and `DocumentImage` are private root imports. Their component
  crate types remain available for implementation consumers. The root exports
  the new `ContentHit` and `PointerDownOutcome` result contracts.
- `ContentHit` reports table, image and equation action availability together.
  Images under text remain copyable; equation actions require the equation to
  win paint order. A raw image fallback runs only when text wins. Table lookup
  is shared with the existing content query. The GPUI context menu uses one
  snapshot instead of separate table/image/equation queries.
- `pointer_down` returns both handled state and the retained image flag. GPUI
  uses the flag for tap policy. The session resolves the image once and passes
  it to the private image-opening operation, without running the broader content
  query on pointer presses. Clipboard precedence and failed-open fallback to
  text selection are retained.
- All document interaction methods accept painted positions relative to the
  document's top-left. The session performs scale conversion exactly once;
  returned selection/reading geometry already uses painted coordinates. GPUI
  only subtracts its window origin. Notes keep their own coordinates, and
  engine views keep intrinsic coordinates. The unused origin/pivot field is
  removed; painting and interaction use the document's top-left origin.
- Selection and table copying return `bool` through core and session. Removed
  always-successful `Result` wrappers, private `copy_result`, unreachable command
  error handling and `RendererOperation::CopySelection`. Real image and equation
  export errors still produce operation-failure events. Existing fixtures use
  the revised pointer result, copy boolean and owned query signature.

All eight changed Rust files pass read-only `rustfmt` syntax parsing. Named
reader-root imports (33 consumer files) and engine-facade imports pass their
source audits. The coordinate audit covers all 16 point-taking session methods;
command dispatch still covers all 36 variants. Source searches found no stale
removed session APIs or operation variant. Scoped whitespace checks pass. No
tests or test helpers were added. Compilation, tests and benchmarks were not run;
the earlier release matrix predates this pass. These source checks do not verify
runtime scaling, hit precedence or clipboard behavior.

### HtmlEngine facade boundary fixes (4 November 2026; source-only)

This pass addresses the HTML engine API, independently of the earlier reader
session reductions. `Document` and required layout/projection services remain.

- `RenderBoxView::dom_node` returns a branded canonical `DomNodeId` instead of
  exposing a raw DOM index. Table-source mapping and the existing WPT fixture use
  that typed identity. `RenderTextView::glyph_slice` is crate-private; supported
  character, glyph, line-placement and source-addressing services remain.
- The facade owns `ParsedHtml` and `QuirksMode`. Parsing exposes metadata,
  encoding and diagnostics with one checked, consuming `into_dom()` conversion.
  Raw syntax trees and cloned/panicking DOM builder variants are available only
  through the parser implementation crate. The live TOC consumer uses the checked
  facade conversion. Existing fixtures that deliberately construct implementation
  text indexes/factories import that implementation parser directly.
- The facade owns `Error` and the copyable `ErrorKind` categories `Parse`, `Style`,
  `Shape`, `InvalidInput`, `Resource`, `Internal`. Display and standard error
  sources preserve the original pipeline diagnostics and nested context. The
  private payload retains the existing shared pipeline error allocation; no
  success-path allocation is added. Empty-session and stage variants are no
  longer the public document classification. `From` conversions support engine
  and backend integration without exposing the pipeline kind through `kind()`.
- The engine no longer exports a drag threshold. `SelectionState` and scoped
  selection accept the host's drag distance in hit coordinates. HtmlViewCore owns
  the existing four-pixel reader policy, including application dictionary click
  detection; the standalone GPUI fragment adapter owns its own four-pixel policy.
  Logical selection transitions, semantic promotion and scoped selection remain
  useful engine services. Existing behavior retains the previous threshold.
- `RenderExportView::text` takes `TextExportFormat`; its former plain/paragraph/
  Markdown/both helpers are private. Both representations still share one DOM
  conversion. The facade `Selection` service retains its lazy text methods.
  `RenderTable::export(TableExportRequest)` consolidates whole-table formats and
  rectangular Markdown export, retaining owned snapshots and existing serializers.
  `DocumentExport::request` and semantic table selection share that operation.
  Borrowed authored markup, excerpts, box export, multi-range and line export are
  retained. The format/request contracts are exported from `html::export`.
- Removed `DocumentUpdate::overrides()`. Font preference selection remains inside
  engine shaping; callers no longer inspect proposed font preferences as a step
  before applying an update.

Existing fixtures and reader/adapter/application callers were migrated. No tests
or test-only helpers were added. Validation is source-only: read-only Rust syntax
parsing, named facade and reader-root import audits, argument/visibility audits,
serializer-body comparisons and scoped whitespace checks. Compilation, tests and
benchmarks were not run; the earlier release matrix predates these changes.
Public methods on re-exported layout types remain application API and must be
reviewed as such; the useful projection/render contracts were deliberately retained.

### Comprehensive HtmlEngine API cleanup (4 November 2026; source-only)

Review scope includes document loading/configuration/revisions, parsing and DOM
contracts, resource and font ports, selection/scoped content, search/reading and
source targets, intrinsic/projected geometry, structural pagination queries,
exports, painting/scenes, and public methods on re-exported layout products.
The review checks ownership, overlapping operations, implementation exposure,
identity versus traversal position, and separation of host policy.

Changes applied:

- `html::export::TextExport` is the shared named `plain`/`markdown` result from
  `RenderExportView::text` and `Selection::text`. It replaces the positional tuple
  and the separate `SelectionText` record. Single-format requests stay lazy; the
  paired serializer retains its shared conversion. Existing assertions and
  destructuring callers were migrated without adding tests or test helpers.
- `start_spatial_rank_at` and `end_spatial_rank_at` explicitly name their spatial
  ranks. The former preserves its past-end length sentinel; the latter preserves
  `None`. `line_intersecting_vertical_range` returns `LineId` directly, removing
  a conversion in decoration painting. `RenderTextView::paint_order()` returns
  line identities, consistent with `spatial_order()`. Pagination cursors remain
  separate from both stable identities and spatial ranks.
- `RenderLinePlacement` retains the native placement cursor privately and exposes
  only `at(character)`. The former facade `LineTextPlacement` also exposed a
  constructor accepting internal content, glyph slices, metric storage and line
  buffers. That constructor remains available to engine implementation crates;
  the borrowed facade cursor preserves lookup hints without allocating or copying.
- `RenderBoxView::break_before` is private. Public forced/avoid break queries cover
  its semantics without returning the internal CSS enum.
- `RenderFragmentView::paint_order_for` is one iterator API, replacing its Vec and
  iterator variants. It retains candidate sorting/deduplication, invalid-candidate
  filtering, line-owned decorations and independent candidate-slice lifetimes.
  Removed raw `decorations_by_indices` traversal; selection goes through the same
  CSS-ordered iterator. Its alternate selection state and manual index iterator
  are removed. Existing candidate fixtures retain invalid/duplicate coverage.
- `html::render` now exports the required `Painter`, `CornerRadii` and continuous
  `RegionProjection` contracts. `html::layout` names `LayoutConstraintError`,
  already returned by public scene validation. GPUI, reader painting and the
  harness use facade imports. Four unused production render-crate dependencies
  were removed; the paginator's recording-painter dependency is development-only.
  Existing lockfile dependency edges were updated without dependency resolution.
- `TextPatternError` retains matcher-construction diagnostics behind a private
  payload; the public matcher no longer returns an automaton implementation error.
- `is_navigation_section_title` and its existing fixture moved to the application's
  book-content layer. EPUB and PDF retain the same narration-filter behavior.
  Semantic reading-unit discovery remains in the engine.

Retained deliberately: standalone view constructors and document conveniences
serve different ownership contexts; owned table snapshots and borrowed authored
HTML serve different lifetime/copy needs. Geometry, source addressing, scoped
content, image working-set requests, backend transactions and independent decoded
versus layout poll flags remain available. Canonical DOM construction/traversal is
still a broad standalone API. Public methods on re-exported types still count as
application contracts; named exports alone do not seal their upstream methods.
Block-decoration candidates remain layout-local integer handles passed back to
paint-order selection, with validity and traversal expectations documented.
No new layer was added merely to lower the method count.

Validation: read-only Rust syntax parsing, facade import/reference audits,
TOML/lockfile parsing, serializer/policy body comparisons and scoped whitespace
checks. Compilation, tests and benchmarks were not run. These checks do not
establish type correctness or runtime behavior; earlier release checks predate
this cleanup. Before snapshots and the scoped diff are recorded in
`/home/johan/.cache/htmlengine-api-cleanup-20261104-031545`.

### Re-exported type boundaries (4 November 2026; source-only)

This pass revisits the broad canonical-DOM re-export retained in the preceding
review. A re-export includes a type's inherent methods, so selecting type names
alone does not isolate implementation APIs.

- `html::document::DomDocument` is now a facade-owned read-only source service,
  with 17 operations for metadata/images, branded node traversal and source
  addresses. Checked parser conversion moves the existing canonical DOM into it;
  there is no `Deref`, raw accessor, public native conversion or buffer clone.
  Image lookup takes `ImageId`; containment rejects foreign document identities.
  Native construction, mutation, interning, raw lookup, diagnostics and redundant
  document-level attribute getters stay in the DOM implementation crate.
- Read-only element/text/attribute references remain useful public contracts.
  Interned element IDs/attribute slots, raw image slots and computed CSS selector
  state require `html_dom::storage::ElementStorage`, which is absent from the
  facade. Internal formatting/style/pipeline users explicitly import that trait;
  their helper bodies retain the same direct cached-slot access. Removed three
  redundant raw `node_idx` methods; internal table mapping uses branded node IDs.
  Public `ElementRef::image()` returns the source catalog's typed `ImageId`.
- Removed facade exports for DOM builder diagnostics, document-lineage comparison
  and the concrete ancestor iterator. Parse failures still use the facade Error
  and retain their standard diagnostic source chain. Branded node identities,
  borrowed attributes and source-position records remain public.
- Layout/paint partitioning and comparison moved from `ReaderStyleOverrides`
  inherent methods to the implementation-only `ReaderStylePartition` trait.
  Method bodies, bitwise float comparisons and equality behavior are unchanged.
  Pipeline and profiling tools import the trait explicitly; application callers
  continue submitting the complete preference record. Publication-font policy
  remains a supported backend query.
- Named `CaretPositionsBuilder` in the text backend contract. Its existing
  streaming callback was already public through `build_compact`; the builder
  has private storage and only a `push` operation. Native compact/dense fallback,
  geometry, resources and append rollback are unchanged.
- TOC anchor resolution now uses `nodes` and `element_source_path`, preserving
  duplicate-ID collection order and source-step conventions. Existing parser,
  loader and reading-position fixtures were migrated without adding tests.

Intentionally retained: layout/render products, branded IDs, owned snapshots,
portable resource ports, shaping transactions and backend result construction.
These are public engine contracts despite residing in implementation packages;
wrapping every record would duplicate the same useful API. Future inherent
methods on re-exported types still require application-API review.

Using the preceding source-counting rule (all features; explicit functions and
inherent methods on named exports), the API is 646 -> 591: 147 -> 164 declarations
in the facade and 499 -> 427 through re-exported types. The facade gains its
17-method source owner while excluding 72 implementation methods. The 46 public
backend/provider trait methods remain, for 692 -> 637 total counted callables;
Document itself remains at 24. These counts exclude generated/standard trait
methods and indirectly reachable types not named by the facade.

Validation is source-only: Rust syntax parsing, facade/reader import audits,
explicit extension-trait caller audits, unchanged helper-body comparisons,
source API counts and scoped diff/whitespace review. Compilation, tests and
benchmarks were not run. Before snapshots and the task diff are under
`/home/johan/.cache/htmlengine-reexport-boundaries-20261104-033610`.

### Implementation modules, without visibility traits (4 November 2026; source-only)

Public methods on a pipeline type are application API when `html` re-exports
that type or returns it from a public operation. Public functions/types that
remain inside implementation modules do not widen the facade. Do not add traits
solely to hide stage internals; use ordinary visibility and module boundaries.

Removed `TextPaintQueries`, `BoxInspection`, `ReaderStylePartition` and
`ElementStorage`. Their existing implementations are normal functions in:

- `html_pipeline::engine::text_paint` for native paint and shaping products;
- `html_pipeline::engine::box_queries` for storage/style/topology inspection;
- `html_style_model::reader_style` (also available through pipeline) for preference
  partitioning and comparisons;
- `html_dom::storage` for cached element slots and selector state.

These modules are absent from the `html` facade. Bodies live directly in their
implementation modules, with explicit view/preference/element arguments, rather
than forwarding through duplicate public methods. The previous visibility-only
trait approach described in the earlier re-export review is superseded here.
Real behavioral contracts (painter, shaping, resources and projections) remain
traits. `ReaderStyleOverrides` equality still uses the same bitwise comparisons.

`RenderTextView` retains 16 public application methods (34 before this cleanup),
and `RenderBoxView` retains five (32 before): point, size, fragmentation distance,
block classification and owned table snapshots. Source/search/word queries,
`Structure`, addressing, semantics, geometry, selection and export remain.
Box line containment is a private function; removed unused expanded-attribute
and empty-box helpers. Necessary cross-crate
implementation functions remain public in their owning implementation modules.

Migrated engine services, native painters, tools and existing inspection fixtures.
HtmlViewCore production still depends only on the facade. No tests, test-only
helpers, wrappers, buffers or new dependency edges were added in this correction.

The named facade count remains 519 functions/inherent methods (164 facade-owned,
355 re-exported) plus 46 port trait methods, or 565 counted callables. `Document`
remains at 24. Counts are source inventories, excluding standard/generated
methods and unnamed indirectly reachable types, and are not the reason to move
an operation across a boundary.

Validation is source-only: Rust parsing, source import/caller audits, helper-body
and call-argument comparisons, API counts, and scoped diff/whitespace review.
Compilation, tests and benchmarks were not run. Before snapshots and task
artifacts are under
`/home/johan/.cache/htmlengine-implementation-modules-20261104-040800`.

### Review of 303 reader-unused facade entries (4 November 2026; source-only)

The direct-call inventory found 216 of 519 explicit functions/inherent methods
used by HtmlViewCore production, including its GPUI adapter and harness. That
leaves 303 candidates, not 303 safe deletions: backend construction, parsing,
standalone query services and portable result inspection remain engine contracts.
Every candidate has a disposition, role rationale and separately labelled lexical
consumer hints in `review-303.json` under the task audit directory below.

Removed 42 entries from the application facade:

- Moved 34 method bodies directly to ordinary implementation functions: text
  paint metrics/layers and bidi fragment placement; decoration color/layer/clip
  inspection and canonical CSS scene traversal; inherited equation image color;
  paint-resource validation and retention; HTML list normalization and selector
  name-matching rules. `text_paint`, `fragment_paint`, `box_queries` and DOM
  `storage` are implementation modules, not facade re-exports or visibility traits.
- Removed redundant line `point_x`/`point_y` methods; callers use `point().x/y`.
  Removed the unused image-fragment paint-order getter and its copied scalar.
  Canonical scene ordering still comes from the retained paint program.
- Removed facade names for `RenderPaintItem`, `RenderDecorationPattern`,
  `RenderOverflowClip` and `RenderResourceLease`; native painters still use them
  through pipeline. No remaining public facade methods return these internal
  products. Excluding the clip's four methods and lease's validity method accounts
  for five of the 42 removed callable entries.

Retained 261 reader-unused entries: 66 source/parsing contracts, 69 standalone
services, 58 backend contracts, 48 snapshot/collection/semantic operations,
15 value/identity operations and five resource operations. Retention is based on
an operation's supported role and its consumers/ports; a lexical name match is
not claimed as proof of a typed call. Shaping transactions and construction,
source navigation/TOC extraction, search/reading, scoped content, owned table
snapshots/export, standalone geometry and resource ports remain available.

Migrated engine services, native painters, diagnostic tools and existing fixtures.
No new tests, test-only helpers, traits, forwarding wrappers or dependencies.
Paint-resource ownership, clipping, placement and serialization are preserved.
The module export audit also repaired missing pipeline re-exports for
`box_queries`/`text_paint` while adding the `fragment_paint` implementation module.

The named facade count is 519 -> 477 (164 facade-owned, 313 re-exported), plus
46 public port trait methods, or 523 counted callables. `Document` remains at 24.
All 216 directly reader-used entries remain. These are source counts, not a
compiler-derived reachability inventory; standard/generated methods and unnamed
indirect types are excluded by the same historical counting rule.

Validation: source syntax/import/module export checks, migration and moved-body
comparisons, all-candidate classification, source API counts and scoped diff/
whitespace review. Compilation, tests and benchmarks were not run. Before
snapshots and task artifacts:
`/home/johan/.cache/htmlengine-unused-facade-review-20261104-043300`.

### HtmlViewCore-only facade (4 November 2026; source-only)

The user's explicit rule supersedes the conservative retention policy in the
preceding review: **only expose what HtmlViewCore actually uses**. Potential
standalone uses, other applications and native backend convenience are not
justifications for widening `html`.

Removed the remaining 261 explicit reader-unused functions/inherent methods
from the facade: 477 -> 216. Every retained entry belongs to the reviewed
HtmlViewCore production call set, including its adapter and standalone harness.
The behavioral injection contracts remain; their named facade count is 42
trait methods. Generated and standard trait methods are excluded from these
source counts.

Removed unused named type exports and HtmlViewCore forwarding exports too.
Private facade aliases were replaced with direct native imports. The unused
`DomDocument`/`ParsedHtml` facade wrapper modules were deleted. Native parsing,
TOC extraction, source search, font preparation, shaping, scene construction and
snapshot inspection continue through their implementation owners.

Public inherent methods on shared types were narrowed as well as the named
exports. Required cross-crate operations moved directly into native module
functions, preserving their bodies; local operations became crate-private.
No visibility-only traits or compatibility wrappers were added.

The GPUI fragment widget owns a native `PipelineSession` for incremental source
and stylesheet updates. Its existing selection and painting behavior uses
native queries and the retained placement-based geometry API. Font-source
preparation moved into `adapters/html-fonts`, and the existing no-resource
provider moved into the native resource crate. CPU source extraction depends
on the parser directly.

Existing engine integration fixtures were registered in crate scope so private
operations do not remain public merely to support their assertions. Existing
consumer fixtures were migrated; no tests or fixture helpers were created.
No compilation, tests or benchmarks were run. Validation is source-only: Rust
parsing, caller/import/export audits and comparison of the 100 moved native
function bodies modulo receiver and call spelling. A source graph of public
signatures, fields, enum payloads and iterator items found no additional public
inherent methods through returned types. The final caller pass found no stale
calls, and HtmlViewCore production sources retain no implementation-crate imports.

Audit directory:
`/home/johan/.cache/htmlengine-reader-only-api-20261104-045350`.

### Coarser reader operations (4 November 2026; source-only)

The follow-up traced production callers through reader helpers. The earlier
216-entry direct-call inventory included caret wrappers without production
callers and a reader paint-order vector read only by a fixture. Removed those
wrappers and the unused preparation/storage, and migrated existing assertions
to native queries or the retained visible-line contract.

The facade now has 190 explicit public functions and inherent methods, down
from 216. Its 42 behavioral trait methods remain. Compiled-pattern construction
is part of the new matching input contract. No visibility-only traits or public
forwarding wrappers were added; 15 required native method bodies moved directly
to implementation functions. The unused paint-rank getter was deleted.

- `TextPatternMatcher::match_lines` owns visible-source extraction, all-pattern
  matching, word-boundary context and mapping matches to source-backed glyph
  ranges and source endpoint addresses. The reader serializes those addresses
  into publication CFIs without repeating engine source lookups. Matching results
  retain the extraction buffers and yield borrowed text, so only accepted
  annotations copy text and no intermediate result vectors are introduced. Recurring-note
  storage, applicability and appearance remain application-owned. PDF matching
  reuses the same compiled native matcher; phrases are not recompiled per page.
- Bounded `StructureView::blocks` records join bounds, constraints, roles and
  line-flow facts once. Sibling facts include geometry and trigger cursors.
  `has_unbroken_border` hides decoration inspection. Column layout, keeps and
  rhythm policy stay in HtmlViewCore.
- `RenderAddressingView::destination` returns stable line/cursor geometry or
  a position fallback for glyph, element and element-source targets. CFI
  parsing, publication selection, navigation history and viewport policy stay
  in the reader.
- Combined content hits retain underlying images as well as the foreground
  winner, eliminating the reader's second image query. Link-only hover stays
  separate and avoids table traversal.
- `Document::request_images` accepts selected lines and owned scopes. Image
  discovery and deduplication stay in the engine; the reader chooses the
  prefetch range and no longer caches a note's image identity list.

Existing fixtures were migrated; no new tests or fixture helpers were created.
No compilation, tests or benchmarks were run. Verification is limited to source
parsing and caller, import/export, moved-body and returned-type audits.

Audit directory:
`/home/johan/.cache/htmlengine-coarse-api-20261104-123052`.

### Engine-owned selection interaction and painting (4 November 2026; source-only)

The facade now has 174 explicit public functions and inherent methods (190
before this pass); its 42 backend trait methods are unchanged. The selection
category is down from 20 methods to six: `SelectionController::{interact,
selection, hit_test, export, paint}` and `Selection::range`. The existing
`Selection::{plain_text, markdown}` remain in the export category.

A controller owns one view's text, table and touch-anchor state. Inputs carry
pointer phases, click count, selection intent and the host's drag threshold.
The engine owns word snapping, cell-span handling, touch-handle extension,
click-versus-drag transitions and selection drawing. Hosts consume changed,
finished, click and pointer-capture outcomes; publication CFIs, clipboard,
scrolling, image actions and link activation stay with the host.

`SelectionLayout` accepts lazy line placements and the existing geometric
`PaintProjection`; no new trait or visibility bridge was added. The reader's
page projection now lives in html-view-doc and serves both content painting and
selection. Engine scratch buffers retain selection rectangles and projected
cells across frames. Idle pointer moves skip selection hit testing, and passing
placements introduces no extra collection. Copy remains explicit and lazy.
Search, reading and annotation overlays remain reader-owned and paint before
selection.

HtmlViewCore no longer owns separate table state or touch anchors and no longer
draws selection rectangles/borders itself. Notes use `ScopedContent::{interact,
paint_selection}` with the same controller. The native continuous GPUI fragment
also uses the controller for gestures and retains its focus, native overlay and
paragraph-copy policies. Existing low-level selection fixtures were relocated
from the reader to their engine owner, existing callers were migrated, and no
new tests or test-only production helpers were added.

Validation is source-only: Rust parsing, changed caller/constructor/import and
manifest/lock checks, API inventory and comparison of retained low-level state
bodies and the moved projection. No compilation, tests or benchmarks were run.

Audit directory: `/home/johan/.cache/htmlengine-selection-20261104-133445`.

### Remaining surface consolidation (4 November 2026; source-only)

Implemented the five follow-up reductions. The facade now has 166 explicit
public functions and inherent methods, down from 174. The 42 backend trait
methods and Document's 18 methods are unchanged.

- Removed obsolete HtmlViewCore word/caret wrappers and made the corresponding
  engine word, caret, cell-hit, endpoint and raw-highlight operations private.
  Opaque input/result types no longer expose the removed word-selection bridge.
- `SelectionController::geometry` returns text-selection bounds and handle
  endpoints from one displayed-line traversal. The reader applies its existing
  outer scale conversion; generic range bounds remain available for annotations.
- `render::paint_overlays` accepts styled ranges and reusable scratch storage.
  The engine owns intrinsic clipping, highlight, underline, squiggle and
  strikethrough drawing. The reader supplies ranges, colors and ordering;
  `AnnotationStyle` reexports the engine's canonical `OverlayStyle`. Search,
  reading and annotation policy remain in the reader.
- `Pagination::groups` provides complete-group forward/backward iteration, and
  `locate(PaginationTarget)` returns cursor, line and group facts together.
  Forward/backward page building, line navigation and image-window traversal use
  those operations. Page fitting, semantic keeps, repeated headers, oversized
  groups and vertical rhythm remain reader policy. Native query bodies moved
  directly to ordinary functions; only the two structural queries needed across
  crates remain public. The unused group-end getter was deleted.
- `addressing().reading_page(interval, page_end)` returns fragments and their
  continuation position together. The engine owns word-boundary adjustment and
  interval clipping; the reader still chooses pages and playback policy. A
  final clamped boundary skips unnecessary lookahead. The former two low-level
  operations are private native helpers with their bodies retained.

Seven moved native bodies and seven geometry/style bodies were compared against
before snapshots, modulo receiver/call spelling and formatting. Caller,
constructor, import/dependency, lock identity and source syntax checks pass.
Old multiline selection fixture calls missed by the earlier migration were also
updated. Existing style fixtures moved with their implementation; no tests or
production helpers solely for tests were created. Compilation, tests and
benchmarks were not run, per the user's instructions. Runtime behavior and
performance remain unverified.

Audit directory: `/home/johan/.cache/htmlengine-surface-20261104-143022`.

The source audit encountered a full /home volume. The older downloaded WPT corpus
under `~/.cache/htmlengine-design-review/wpt` was compressed and retained as
`wpt-cache-20261104.tar.gz` beside its original location, with restoration details
in `archived-wpt-cache.json`. Project sources and audit before snapshots were
retained; no network storage or builds were used.

### Internal table export (4 November 2026; source-only)

Removed the unused public `ExportRequest::Table` and `TableCellsMarkdown`
variants. The selection controller now performs table conversion directly,
preserving selected-table precedence and merged-cell ranges. `TableCellRange`
and `DocumentExport::new` are crate-private; document and scope export still
provide image and equation payloads. The media match handles both configurations
of the optional `math-export` feature without a shared unreachable text branch.

The facade now has 165 explicit public functions and inherent methods, down
from 166; backend trait methods remain 42 and Document retains 18 methods.
Existing fixtures were migrated to the internal conversion operations. No
tests or test-only production helpers were added. Validation is source-only;
compilation, tests and benchmarks were not run.

Audit directory: `/home/johan/.cache/htmlengine-export-20261104-150709`.

### Engine page composition (4 November 2026; source-only)

Moved the former HtmlViewCore paginator into `html::pagination`. `Paginator`
exposes `new(view, PageGeometry, PaginationMode)` and one `compose` operation.
`PageRequest::{StartingAt, StartingAtOffset, EndingAt}` expresses the anchor;
there is no separate direction flag. Cursor requests retain group identity even
when source-space positions overlap. The returned page preserves its start/end
cursor anchors, placements, geometric continuation, headers and paint dependencies.

The engine now owns forward/backward fitting, CSS forced/avoided breaks,
widows/orphans, table fragmentation/repeated headers, book keeps and optional
vertical rhythm. HtmlViewCore retains viewport and column preferences, scaling,
page-turn behavior, navigation and rendering integration. Its `ColumnLayout`
supplies four neutral geometry fields; shared horizontal placement lives in
`PageGeometry`. Frame preparation and narration use the same coarse request API.

The reader's `html-view-paginate` package and dependency edges were removed.
`structure` and its six methods are crate-private; content role/constraint types
and the vertical-rhythm border check no longer escape through the facade.
Native `page_placements` owns the unchanged result buffers and identity index.
Construction/cursor operations are ordinary native functions used by production
composition, outside the facade. Reader-needed result accessors remain public;
unused storage getters were removed. No visibility-only traits were introduced.

The facade inventory is 167 explicit public functions and inherent methods,
versus 165 before this move. Eight existing methods left the facade; three
composition/geometry methods and seven existing reader placement accessors
entered it with their new engine owner. This count includes moved reader APIs,
not only composition entrypoints. Backend trait methods remain 42 and Document
retains 18 methods. Low-level navigation/prefetch queries remain public where
HtmlViewCore still calls them.

Source comparison preserves 32 algorithm/storage bodies and absorbed anchor
workflows in total, modulo names, call syntax and formatting. Existing pagination and placement fixtures
moved to their owners or were migrated; no tests or test-only production helpers
were created. Syntax, import, call-arity, record-constructor and TOML/lock identity
checks pass. Consumer lockfiles changed only for local dependency edges and the
removed reader package; no package versions/checksums were changed. The active
query benchmark imports its role type from the native engine. No compilation,
tests or benchmarks were run; runtime behavior remains unverified.

Audit directory: `/home/johan/.cache/htmlengine-pagination-20261104-151611`.

### Engine-owned composed-page projection (4 November 2026; source-only)

Completed the composed-page boundary. `PagePositions` now retains `PageGeometry`
and its paint origin with placements, offsets, repeated headers and decoration
candidates. HtmlViewCore frame state retains the engine result intact and uses
one `install_page` operation for either traversal anchor. It no longer unpacks
and reconstructs projection state, calculates column translations or resolves
backward paint coordinates. `PagePositions::reproject_columns` owns width-only
translation and updates retained geometry; `clear` preserves reusable buffers.

`PageView::new(render_view, &page)` provides the existing real `PaintProjection`
implementation and page-aware hit tests, pointer interaction, selection geometry,
hits/copy/painting, styled overlays and reading bands. Projection formulas moved
directly from the reader, including offset-break cuts, clipped repeated headers
and nearest-owning-flow box fragments. Inputs and results are page-local and
unscaled. The reader retains view origin/scale, viewport/cache decisions,
navigation, palettes, copy policy and publication address formatting.

Removed the reader `html-view-doc` crate and its dependency edges in all three
consumer lockfiles. Removed the redundant reader selection-interaction bridge;
callers send `SelectionInput` straight through the engine page view. Reading
bands come from the engine; the reader scales and orders them. Selection/overlay
painting skips absent pages in the engine. Native continuous/GPUI projections
continue to use the existing lower-level geometry and projection contracts;
no visibility-only traits were introduced. Native horizontal mapping is an
ordinary function, and reader-only column-coordinate methods were removed.

The facade has 180 explicit public functions/inherent methods, versus 167 before
this move: twelve page-view methods and two page-result lifecycle methods entered
the facade with their engine owner; the column-coordinate getter left it.
Backend trait methods remain 42 and Document retains 18 methods. This is an
ownership/consolidation change; the inventory includes operations previously
public in reader crates. Projection internals remain private.

Source comparison preserves 35 projection, horizontal-mapping and composition
bodies modulo names, field access, optional candidates and returned page metadata.
Existing query/projection fixtures moved with their owner; other fixtures were
migrated. No tests or test-only production helpers were created. Syntax, import,
call-arity, record-constructor, TOML and lock identity checks pass. No dependency
versions/checksums changed. Compilation, tests and benchmarks were not run;
runtime behavior and performance remain unverified.

Audit directory: `/home/johan/.cache/htmlengine-page-boundary-20261104-155250`.

### Engine-owned narration page traversal (4 November 2026; source-only)

Moved `RendererSession::append_reading_fragments` into the engine's existing
`Paginator`. Its coarse operation accepts a glyph interval, `ReadingOrigin`
(document beginning, cursor or fallback offset) and the caller's fragment buffer.
The engine now composes successive pages, determines their source ends, keeps
words whole and advances continuation. The no-viewport path still extracts the
requested source range directly. Loop bounds, end/progress guards and allocation
behavior are preserved.

HtmlViewCore retains document readiness, publication identity, playback and the
active-fragment index. It makes one engine request for the current-page suffix
and, for the full-document operation, one preceding prefix request. Installed
page cursors and pre-install fallback offsets retain their previous semantics.
The low-level `reading_page` implementation is now an ordinary native query;
its inherent facade method and `html::reading::ReadingPage` reexport were removed.
Existing fixtures use the native query. No traits or dependencies were added.

The facade remains at 180 explicit public functions/inherent methods: one coarse
paginator operation replaces one addressing primitive. Backend traits remain at
42 methods and Document at 18. HtmlViewCore runtime libraries now contain 5,621
production code lines, down 49; GPUI and harness counts are unchanged.
Source comparison and syntax/import/call-arity
checks are recorded in the local audit. No tests were added or run; compilation
and benchmarks were not run. Runtime behavior remains unverified.

Audit directory: `/home/johan/.cache/htmlengine-narration-20261104-164132`.

### Engine-owned adjacent page anchors (5 November 2026; source-only)

`Paginator::next_page_anchor` and `previous_page_anchor` now resolve adjacent
page starts/ends. They take the optional installed engine page and viewport
fallback offset; forward turns also take the retained forward continuation
(`None` after forward exhaustion). They return a `PageAnchor` containing the
resolved cursor and document offset, or `None` at the corresponding document
boundary. Forward cursor results already identify the owning group's start,
removing the reader's second group lookup. Cursor addition is checked.

HtmlViewCore retains chapter switching, pending-read cancellation, repaint and
anchor-dirty notifications. It applies forward anchors through one viewport
setter; existing source/glyph navigation uses that setter too. Backward turns
retain the previous raw end cursor and group-bottom offset. Missing pages,
missing groups, empty pages and overlapping offsets keep their existing fallback
behavior. A backward page's `reached_end` still means the beginning; the engine
uses the reader's retained forward continuation when resolving forward turns.
No new traits, dependencies or tests were introduced.

The facade adds two coarse methods, reaching 182 explicit public functions and
inherent methods. Backend traits remain at 42 methods and Document at 18.
HtmlViewCore runtime libraries now contain 5,587 production code lines, down 34.
Source-only comparison, syntax, named-import and call-arity checks are recorded
in the local audit. Compilation, tests and benchmarks were not run.

Audit directory: `/tmp/htmlengine-page-turn-20261105-162552`. The prior narration audit was moved to
temporary storage with its original cache path preserved by a symlink because
the workspace filesystem was full; no project files or unrelated caches were
deleted.

### Engine-owned current document position (5 November 2026; source-only)

`pagination::document_position` resolves the current pagination cursor, source
line's first glyph and document fraction from a `PageAnchor` and optional
preferred reading glyph. The returned `DocumentPosition` borrows the render view;
its `source` accessor resolves the owned address on demand. Glyph-only and
cursor-only callers allocate no source path. The geometric page glyph remains
independent of the preferred glyph used for source addressing and progress.

Moved HtmlViewCore's empty-document start address, missing-address behavior,
image-only glyph filtering, offset fallback and clamped text/geometric progress
calculations into this operation. The reader retains settled-page priority over
pending composition anchors, publication-specific preferred glyphs, document
identity, CFI formatting, whole-publication progress, deduplication and event
ordering. Position emission consumes one engine query result; line navigation
uses its cursor. Existing glyph/source reader entry points remain compatible.

Removed the now-unused reader `find_start_line` helper. The low-level
`source_location_on_line` body is an ordinary native query rather than an
inherent facade method. Existing fixtures call that native query; no tests or
test-only production helpers were created. No dependencies or traits were added.

The facade has 183 explicit public functions/inherent methods: the coarse query
and lazy source accessor replace one source-address primitive. Backend traits
remain at 42 methods and Document at 18. HtmlViewCore runtime libraries now
contain 5,540 production code lines, down 47. Source-only comparison, syntax,
named import and arity checks are recorded in the audit. Compilation, tests and
benchmarks were not run; runtime behavior remains unverified.

Audit directory: `/tmp/htmlengine-document-position-20261105-163720`.

### Remaining engine/reader boundary work completed (5 November 2026; verified)

Completed the remaining boundary moves and their cleanup:

- `Document::request_images` accepts an optional composed page, lookahead count
  and owned scopes. The engine selects groups/lines and discovers/deduplicates
  resources. The reader retains timing, lookahead and note ownership.
- `PageView` owns visible text export/matching, narration's visible source start,
  glyph visibility and reading-band ordering. Reading bands reuse one engine
  vector, scaled in place by the reader. Unready matching retains no source batch.
- `DocumentTarget` resolves cursor, next/previous line, offset, document-end and
  fractional destinations. The reader retains publication navigation and events.
- `PageRequest::EndingAtOffset` and `Paginator::compose` own backward end
  resolution, cursor clamping and first-page normalization from document zero.
  The reader applies the engine result without another composition fallback.
- `DocumentPosition::reflow_anchor` captures one source address and glyph
  fallback, preserving leading authored content at document start. The pending
  request encodes traversal; no separate direction flag is supplied.
- Removed `html-view-select` and its dependency edges. Selection, geometry,
  export and painting call the engine page view directly; copy/clipboard policy
  remains in the reader.
- Removed separate `BackwardComposition` storage and its unused transient
  offsets. Viewport state retains one engine request plus pending/displayed
  offsets. Installed page cursors come directly from `PagePositions`, with one
  installation marker instead of duplicate start/end fields.

Removed superseded facade exposure: raw `Geometry`, four low-level controller
methods, free overlay painting, mutable placement methods, the pagination
accessor/traversal types, and primitives for text-line export, pattern-line
matching, decoration candidates and box-flow distance. Native implementations
use ordinary functions; no visibility-only traits were introduced. The real
continuous GPUI selection contract remains supported.

The facade has **155** explicit public functions/inherent methods, down **28**
from 183. Document retains 18 methods and backend traits retain 42. HtmlViewCore
runtime libraries contain **5,368 production code lines**, down **172** from
5,540; physical production lines are 6,331. Counts exclude harness (757 code
lines) and GPUI adapter (1,733, including two required event-emitter impls).

Actual compilation exposed stale imports and fixture calls from prior source-only
refactors. Migrated callers to native APIs rather than restoring facade wrappers.
Fixed the foreign inherent `PageGeometry` impl as a private ordinary function.
Restored functional GPUI event-emission impls, direct font-decoder imports and
native theme calls. The GPUI glyph cache retains its own handle/measurements
instead of reading opaque engine fields; its value size and allocation pattern
are unchanged. No GPUI-Fork or vendor sources were edited.

Validation (authorized by the request to complete the verification item):

- HtmlViewCore workspace library tests: **189 passed, one existing ignored**.
- Engine/pipeline/model library tests with math export: **147 + 157 + 36 passed**.
- HtmlViewCore workspace `cargo check --all-targets`: passed, including harness.
- Reader GPUI and native GPUI adapter test targets: compilation checks passed
  in their respective workspaces. GPUI window tests and benchmarks were not run.
- Source syntax/body, import, test-case inventory and dependency-identity audits
  pass. No test cases or test-only production helpers were added. One existing
  object-fit fixture now supplies explicit natural image metrics rather than
  mistaking HTML dimension hints for natural size; its assertions are unchanged.

Total executed: **529 tests passed, one ignored**. Existing warnings remain.
Adapter checks used temporary configuration reproducing the application's
existing harfrust/cosmic-text/swash patches. No dependency versions/checksums
changed. Cargo reconciled stale local dependency edges and removed the unused
root `data-url` entry; consumer locks dropped `html-view-select`.

Audit snapshots, diffs, logs, profiles and patch configuration are retained at
`/home/johan/.cache/htmlengine-remaining-boundaries-20261105-164956` (the original
`/tmp/htmlengine-remaining-boundaries-20261105-164956` path is preserved). The dedicated local build target is
`/home/johan/.cache/html-remaining-boundaries-build-20261105`. Storage failures
were resolved using task-owned local caches; network mounts were not used.

### Consistent pagination results (5 November 2026; source-only)

`PagePositions::reached_end` now always describes the document end, including
pages composed backward. The beginning remains represented by `start_line == 0`
and is interpreted by `previous_page_anchor` inside the engine. Backward results
resolve their following group and geometric continuation directly; a host no
longer supplies retained forward continuation to `next_page_anchor`.

Each result carries its resolved `PageRequest`, retaining cursor identity or the
original forward offset as appropriate. A backward request reaching the first
page returns the forward request used by engine normalization. `PageRequest`
lives alongside `PagePositions` in the engine model, retaining the same facade
import path. No new wrapper, visibility trait or dependency was added.

HtmlViewCore installs the result through one `ViewportState::install_page` call.
Removed `ViewportContinuation`, its storage/accessors and the separate forward,
backward and first-page installation branches. Pending composition anchors,
settled page geometry, chapter transitions and buffer/scratch reuse remain.
Viewport defaults now derive from the engine request default.

The facade remains at **155** explicit public functions/inherent methods;
backend traits remain at 42 and Document at 18. HtmlViewCore runtime libraries
now contain **5,301 production code lines**, down **67** from 5,368. Harness and
GPUI adapter counts are unchanged.

Validation: changed Rust sources parse through rustfmt without writing formatted
output. Removed-symbol and result-constructor audits pass. Existing fixtures
were migrated without adding test cases (180 existing `#[test]` cases in changed
Rust files before and after). **Compilation, tests and benchmarks were not run**,
as required by the current project instructions. Source checks do not establish
compilation or runtime correctness.

Snapshots, source diff, syntax/inventory reports and line/API inventories:
`/home/johan/.cache/htmlengine-pagination-result-20261105-205114`.

### Request-derived page cache identity (5 November 2026; source-only)

`FrameGeometryCacheKey` now derives page identity directly from `PageRequest`:
the standard enum discriminant plus its cursor or bit-exact document offset.
This distinguishes all four request variants and preserves exact offset hashing
without adding another request enum or changing the engine API. Cursor requests
no longer carry a second geometric offset in their cache identity. Layout-only
compatibility ignores the request; width-only reprojection compares it directly.

Removed `PaintDirection`, backward end-offset storage, four direction/end-anchor
getters, and the private cache-key forwarding helper. Viewport composition keeps
the pending request and displayed start offset. Cache reuse republishes the
installed page's resolved origin, so repeated cursor navigation cannot leave
position reporting tied to a pending fallback offset. Existing chapter and
viewport fixtures now inspect engine requests directly.

HtmlViewCore runtime libraries contain **5,263 production code lines**, down
**38** from 5,301. Harness and adapter counts are unchanged. The engine facade
remains at 155 explicit public functions/inherent methods; no engine sources,
dependencies or vendor files changed.

Validation: all five changed Rust files parse through rustfmt without writing
formatted output. Removed-symbol, cache-field and caller/import audits pass.
The 150 existing test cases in changed files retain their names and count; no
new cases were added. **Compilation, tests and benchmarks were not run** under
the current instructions. Source checks do not establish compilation or runtime
correctness.

Snapshots, source diff, syntax/test-inventory reports and production line counts:
`/home/johan/.cache/htmlengine-request-cache-20261105-210836`.

### Flattened viewport state (5 November 2026; source-only)

Merged `ViewportComposition` into `ViewportState`. The retained state owns its
engine request and viewport origin directly; request, cursor and origin queries
no longer pass through `composition()`. Navigation, page preparation, reflow and
existing fixtures use the direct getters. Removed the wrapper type, forwarding
getter and unnecessary local snapshots.

The fields retain their separate meanings: backward positioning changes only
the request until an engine page supplies the resolved origin. Forward
positioning and page installation update both fields together. Defaults,
`Clone`/`Copy`, buffer reuse and the engine API are unchanged.

HtmlViewCore runtime libraries contain **5,252 production code lines**, down
**11** from 5,263. Harness and adapter counts are unchanged. No dependencies,
engine sources, visibility traits or new test cases were added.

Validation: all eight changed Rust files parse through rustfmt without writing
formatted output. No wrapper-type or `composition()` callers remain in the
reader or application source. The 157 existing test cases in changed files
retain their names and count. **Compilation, tests and benchmarks were not run**
under the current instructions; source checks do not establish runtime or
compilation correctness.

Snapshots, source diff, syntax/test-inventory reports and production line counts:
`/home/johan/.cache/htmlengine-flat-viewport-20261105-211634`.

### Private reader loading policy (5 November 2026; source-only)

Removed the `html-view-load` crate and the reader's public `load` re-export.
The existing preparation helper, six-field `DocumentLoadSettings` snapshot,
validated engine-options conversion and note-display-to-flow policy now live in
`skrift-column-paginator/src/load.rs`, behind a private module and crate-visible
items. Initial, synchronous and background preparation keep their existing
calls and error conversion. Pending-navigation settings equality and stale-work
rejection are unchanged.

Deleted the three unused loading wrappers and the now-unused `book` convenience
constructor. Existing loading fixtures call `Document::load`/`load_timed`
directly, retaining their validated width/font size, smart standalone images,
book composition and excluded note flow. The nine existing loader cases moved
to `src/load/tests.rs`; the profiling case remains ignored. No test cases or
test-only production helpers were added.

Removed workspace/package dependency edges and reconciled the three consumer
locks. The reader gained the existing parser package as a dev dependency for the
moved fixtures. Lock identities confirm that only `html-view-load` disappeared;
all other package versions, sources and checksums are unchanged. No engine,
GPUI-Fork or vendor sources changed. The workspace now has six members including
the harness.

HtmlViewCore runtime libraries contain **5,205 production code lines**, down
**47** from 5,252. Harness and adapter counts are unchanged. The engine facade
is unchanged.

Validation: changed/new Rust sources parse through rustfmt; manifests and
workspace membership resolve in source audits. Removed loader APIs and crate
edges have no remaining source consumers. The 147 existing test cases across
changed/moved files retain their names and count, including the ignored case;
all 30 moved-loader and 561 navigation assertion token sequences are preserved.
All three lock identity/dependency audits pass. **Compilation, tests and
benchmarks were not run** under the current instructions. These checks establish
source consistency, not runtime or compilation correctness.

Snapshots, source diff, syntax/manifest, test-inventory, assertion and lock reports:
`/home/johan/.cache/htmlengine-private-load-20261105-212122`.

### Direct navigation queries (5 November 2026; source-only)

Removed the `NavigationDocuments` and `NavigationLocation` getter wrappers,
their construction methods and unused collection/raw-anchor accessors.
`NavigationState` now supplies the three production queries directly:
`document_uris`, `current_document` and `anchor_update_pending`. Callers use the
returned URI slice for indexing, iteration and length. This removes two public
types and seven public query methods across the navigation crate.

`SpineAnchor`, `NavigationState` fields/initialization and the validated mutation
bodies are unchanged. The sole test-only raw-anchor read now uses the existing
`anchor_glyph_here` query, checking that the anchor belongs to the current
document. No test-only production getter or new test case was introduced.

HtmlViewCore runtime libraries contain **5,161 production code lines**, down
**44** from 5,205. Harness and adapter counts are unchanged. No engine sources,
dependencies, lockfiles, traits or new allocations changed.

Validation: all thirteen changed Rust files parse through rustfmt without
writing formatted output. Removed-symbol/caller audits find no remaining
wrapper consumers. The 156 existing test cases in changed files retain their
names and count. Source comparisons confirm unchanged state, initialization,
`SpineAnchor` and mutation bodies; all consumer edits are getter substitutions.
**Compilation, tests and benchmarks were not run** under the current
instructions. Source checks do not establish compilation or runtime correctness.

Snapshots, source diff, syntax, test-inventory, caller and contract reports:
`/home/johan/.cache/htmlengine-direct-navigation-20261105-215232`.

### Reader cleanup items 1–12 (6 November 2026; source-only)

Completed the twelve authorized follow-ups from the comprehensive reader review:

1. Navigation and reflow restoration share `NavigationState::document_position`
   through the production `reflow_anchor` query. Displayed-page cursor, pending
   cursor, viewport origin and document-qualified glyph precedence are retained.
2. Synchronous and background image reads share event/error conversion. A failed
   synchronous read now emits the same operation failure as the worker path;
   its `false` result still permits the existing selection fallback.
3. Removed the three unused default-CFI publication-search wrappers and the
   unused `publication_text_lengths` helper. Production consumers use the retained
   cancellable, streaming `search_publication_with_cfi` operation.
4. Removed the unused session search-active getter, search cursor getter,
   annotation count getter and default-CFI annotation-resolution wrapper.
5. Removed unused `ColumnLayout::for_column_count`, `inline_metrics` and
   `InlineMetrics`.
6. Removed `RendererCore::text_geometry`; callers use the existing
   `SpineItem::geometry` page view directly.
7. Whole-publication result installation no longer restarts a synchronous
   query twice. The two production entry points explicitly own query reset.
8. Note pointer methods share internal dispatch while preserving note-local
   coordinates, drag thresholds and handled-versus-finished return semantics.
9. Reflow anchors are still captured before updating the document. Their CFI
   strings are formatted only after paint/unchanged early returns.
10. CFI writers append into their output buffers, range construction avoids
    segment/join vectors and strings, range component splitting borrows slices,
    and numeric/path parsing avoids temporary collections. Existing escaping,
    delimiter handling and recovery rules are preserved.
11. Startup timing logs use the existing `BOKHEIM_PROFILE_RESIZE` and
    `GPUI_KOBO_PROFILE` controls.
12. Removed the reader fragment convenience methods and `ReadingFragment`
    re-export. Existing fixtures use `reading_items` or the existing engine
    paginator operation directly. The engine fragment composition API remains;
    no replacement reader facade or test-only production helper was added.

HtmlViewCore runtime libraries contain **5,018 production code lines**, down
**143** from 5,161, counting formatted code without tests, comments or blank
lines. Harness and adapter counts are unchanged. No engine Rust sources,
application sources, dependencies or lockfiles changed in this task.

Validation: all twelve changed Rust files parse through rustfmt and tree-sitter.
Caller searches across active reader/application Rust sources find no remaining
uses of the removed reader APIs; unrelated PDF methods and frozen vendor sources
are excluded from that conclusion. The 173 existing test case names/counts are
preserved. Source token comparisons confirm that the existing CFI test function
bodies are unchanged. Exact source comparisons confirm unchanged font-load
emission, publication CFI mapping and `pointer_leave` behavior.
**Compilation, tests and benchmarks were not run** under the current
instructions. Source parsing and caller audits do not establish type correctness,
runtime behavior or measured performance.

Items 13–15 remain for discussion before implementation:

- **13: font-load effects.** Document updates still emit font-load requests on
  paint and unchanged updates. Define the correct identity/change criteria for
  suppressing redundant requests without missing provider/font-policy changes.
- **14: CFI parsing consistency.** Delimiter/assertion scanners still have
  differing recovery rules. Consolidation needs an explicit policy for escaped
  delimiters and malformed input; the allocation work above leaves those rules
  intact.
- **15: pointer leave.** The session cleanup operation has no active host caller.
  Review the host gesture lifecycle before connecting it or removing it, since
  it clears pending pointer state and link hover.

Snapshots, scoped diff, syntax, caller, source-contract, inventory and line reports:
`/home/johan/.cache/htmlengine-reader-cleanup-1-12-20261105-220755`.

### Link hover underlining (6 November 2026; source-only)

Links now receive a hover underline through the engine's existing text-overlay
painter. `html::pagination::LinkHover` owns the hovered source-element identity
and text range; its three production operations update from page-local pointer
data, report active hover and paint the underline. DOM identity distinguishes
separate links that share a destination. Source lookup includes inline
descendants; visible fragments use the existing page/column/overflow projection.
Linked images retain hover/cursor behavior; an image-only link has no text to
underline. Underline color follows the reader foreground override or the link's
resolved text color. This is a link interaction overlay, not general CSS
`:hover` selector matching.

The reader stores engine hover state instead of a boolean and passes it to the
page painter. Hover transitions invalidate the interaction overlay, including
moves between two links, while retaining the base display layer. Reader palette
changes refresh the underline color; geometry/document changes clear stale
hover. Removed the old cursor activation/deactivation helpers.

The GPUI adapter now clears hover when leaving the document hitbox or the
window, including window exits without a final mouse-move event.
`RendererSession::pointer_leave` now clears hover only: it preserves the saved
press position and active selection until release. The adapter continues routing
movement and release outside the document for captured gestures. This completes
the hover-leave integration discussed in review item 15. Actual focus-loss or
lost-release gesture cancellation remains a separate lifecycle question; no
cancellation API or policy was introduced here. Review items 13–14 remain
unchanged.

Validation: all eleven changed Rust files parse through rustfmt and tree-sitter.
Source comparisons confirm unchanged pointer-down/up function bodies and all
thirteen existing test function bodies in the changed files. Caller searches
confirm production use of all three engine hover operations and the leave hooks,
and no remaining cursor-activation helper calls. No tests or test-only production
helpers were added. **Compilation, tests and benchmarks were not run** under the
current instructions; rendering and Rust type correctness remain unverified.

Snapshots, scoped diff, syntax, caller, inventory and source-contract reports:
`/home/johan/.cache/htmlengine-link-hover-20261105-232631`.

### Font-load effect deduplication: review item 13 (6 November 2026; source-only)

Document updates now emit a font-load effect only when the committed font-face
list or the publication-font policy changes. The reader captures the two inputs
before applying an update and compares them with the successful result. Shared
face snapshots use an Arc identity check; rebuilt snapshots with equal contents
also suppress emission. Unchanged updates, paint changes, resize/reflow and
`RefreshFonts` therefore do not issue another request unless the actual font
inputs changed. Both disabling and re-enabling publication fonts still emit the
policy transition, and media-dependent declaration changes are detected by value.

Initial construction and prepared-document loading retain their existing
unconditional requests, covering chapter navigation and cross-document notes.
The resource provider is fixed for a renderer's lifetime; a new renderer retains
its initial request. No persistent cache, state field, public API, dependency or
test-only helper was introduced. The host's existing per-face loading/registration
and failure handling code is unchanged. Incidental redraws no longer trigger
retries of failed font loads; later loading or font-input transitions still pass
through the existing host request tracking.

Only `apply_document_update_impl` changed in Rust source. Static syntax parsing
through rustfmt and tree-sitter passes. Source comparisons confirm unchanged
initial/chapter request producers, host handling, effect types, struct definitions,
existing test inventory and the update control flow after font-effect emission.
The pipeline's existing rollback on failed updates preserves the comparison's
committed-input boundary. **Compilation, tests and benchmarks were not run** under
the current instructions; source checks do not establish type or runtime
correctness. Review item 14 remains open.

Snapshots, scoped diff, syntax and source-contract reports:
`/home/johan/.cache/htmlengine-font-effects-20261105-235903`.

### Shared CFI syntax: review item 14 (6 November 2026; source-only)

The CFI crate now has one private lexical scanner and parsed point/range model
in `syntax.rs`. Source-position parsing, glyph point/range resolution, package
mapping and document-index queries use this model. Delimiters are structural
only outside assertions; escaped assertion characters remain data. The same
reserved-character predicate drives assertion writing and escape validation.

The supported address syntax consistently rejects unmatched/nested unescaped
brackets, unescaped parentheses, dangling or invalid escapes, zero steps,
leading zeros, integer overflow, trailing numeric junk, missing path components
and extra unescaped indirections. Offsets can be zero and retain UTF-16 units.
Both range endpoints are parsed even when the caller requests only its start.
ID/text assertions and their parameters remain advisory: they are not compared
against document content. Unsupported spatial/temporal offsets now fail instead
of being silently truncated to numeric prefixes. Document-only publication
anchors and the existing empty/slash document-start forms are retained.

Range generation compares structured source elements, including their IDs,
then writes common and relative paths directly into the final string. It no
longer splits serialized paths at slashes inside IDs and no longer allocates the
two temporary serialized address strings. Publication source-position parsing
and document-index lookup no longer convert through a temporary synthetic CFI.
Unknown EPUB documents still receive no fabricated address, and invalid package
bases cannot be serialized.

Syntax validation remains separate from DOM address resolution. Syntactically
numeric paths that no longer identify content can still reach the existing
annotation quote-recovery path; the `/999` recovery fixture remains applicable.
The engine resolver and annotation recovery code were not changed.

All sixteen existing public function signatures are unchanged; the syntax types
and helpers are private to the CFI crate. No dependency, trait, stored cache,
engine Rust source or application source was introduced or changed. Existing
CFI test names and expectation literals are preserved; one existing comma-split
fixture calls the shared scanner in place of the removed helper. No new tests or
test-only production helpers were added.

Validation: all three changed/new Rust files parse through rustfmt and
tree-sitter. Source audits confirm signature preservation, fixture preservation,
shared lexical scanning, both-endpoint parsing, checked numeric conversion and
structured range generation. **Compilation, tests and benchmarks were not run**
under the current instructions. These are source checks, not type, runtime or
performance verification.

Syntax reference: [EPUB CFI 1.1](https://w3c.github.io/epub-specs/epub33/epubcfi/#sec-epubcfi-syntax).
The reader implements structural/text addresses and retains its documented
recovery behavior; this change does not claim full CFI feature support.

Review items 13–15 are now addressed. Lost-release/focus-loss gesture cancellation
remains the separate follow-up identified during hover integration.

Snapshots, scoped diff, syntax, public-signature, fixture and source-contract reports:
`/home/johan/.cache/htmlengine-cfi-syntax-20261106-000900`.

## Shaping geometry no longer owns foreground color (2026-11-06)

Theme changes already followed the paint-only path. Removed the redundant color
state rather than introducing a new theme invalidation path:

- `ShapingStyle` contains geometry/font properties only; `TextRunShapeRequest`
  no longer carries per-character paint colors. Migrated engine callers,
  existing fixtures and the reader's two native profiling examples.
- Shaping-span scans now determine spacing placement without allocating color
  vectors. The engine's resolved paint-color runs remain the source for authored,
  generated, first-line and theme colors.
- GPUI run metadata contains fonts and glyph ordering only. Paint fragments own
  resolved colors, and replay reads them directly. The painter uses its existing
  text override before line/fragment colors and defaults unspecified ink to black.
- Removed native recoloring, byte-color conversion and recoloring rollback state.
  Retained scenes still share immutable run geometry; removed the now-redundant
  inner glyph `Arc` and use owned metadata arrays. Document/append transactions
  remain in place; `GlyphShaper` still has 14 methods.
- Adapted existing geometry, retention and paint fixtures. Removed the two unit
  cases whose request-color field/helper no longer exist; added no test cases or
  test-only production helpers.

Validation: tree-sitter and rustfmt parsed all 13 changed Rust files. A source
caller audit checked all 31 shaping-constructor calls, including macro bodies.
Removed color accessors/mutators have no remaining Rust references in the engine,
reader core or reader apps. **Compilation and tests were not run** under the
repository instructions; syntax/caller checks do not verify types or runtime behavior.

Task snapshots, scoped diff and source reports: `/home/johan/.cache/htmlengine-shaping-color-20261106-004519`.

## Published paint inputs retain native resources (2026-11-06)

Established the ownership boundary before attempting to remove the shaping
transaction protocol. Published GPUI geometry no longer requires the host to
supply a snapshot of the currently active shaper.

- Added `NativePaintResources`, an opaque shared snapshot of immutable,
  `Send + Sync` native data. `GlyphShaper::retain_paint_resources` lets the engine
  capture it after a successful document/append pass; failed passes preserve
  the published ledger and snapshot. Compact glyph/run IDs remain unchanged.
- The metric ledger, detached note output and retained render scene keep the
  appropriate snapshot. Immediate and recorded paint bind resources from their
  input through `Painter::bind_text_resources`; a backend rejects incompatible
  native payloads before drawing.
- GPUI glyph/run stores now use `Arc` copy-on-write storage for the owned boundary.
  The font-loading callback keeps its existing `Rc`; no backend or callback
  receives new thread bounds. Adapters with borrowed native resources, including
  `ResourceShaper`, can keep the existing generation-based contract by returning
  `None`; their borrowing/non-Clone support is unchanged.
- GPUI commands retain native layouts or runs directly. Removed the separate
  replay registry and its unreachable run-fragment fallback. A painter can bind
  another input without changing the meaning of already recorded commands.
- Removed HtmlViewCore's document/note snapshots, synchronization method,
  generation comparisons and generation-forwarding APIs. GPUI painter
  constructors now take only the image cache when needed. The snapshot type and
  shaper snapshot accessor are private to the GPUI adapter. Migrated the
  continuous fragment renderer and live embedding benchmark consumer.
- Backend generation changes still invalidate reusable shaping. They do not
  invalidate published GPUI paint backed by an owned snapshot. Stale scene and
  note ownership checks remain. Scope reclamation refreshes the retained prefix
  after truncating active native storage and the metric ledger.

Document/append rollback and checkpoint hooks remain: they still protect mutable
active registries and reclaim metric registrations. This pass establishes owned
publication; it does not claim to eliminate that protocol. `GlyphShaper` now has
15 methods, including the new snapshot handoff. No traits were introduced to
hide pipeline stages.

Validation: tree-sitter and rustfmt parsed all 17 changed Rust files. Source
caller checks cover all 10 GPUI painter constructions. Existing test names/counts
are unchanged; fixtures were migrated to the new ownership contract, including
prefix-value preservation rather than copy-on-write allocation identity.
**Compilation, tests and benchmarks were not run.** Syntax and caller checks do
not establish type correctness, runtime behavior or performance.

Before snapshots, scoped diff and source reports: `/home/johan/.cache/htmlengine-owned-paint-resources-20261106-024601`.

## Native paint data belongs to caller cache entries (2026-11-06)

Replaced pass-wide backend snapshots with references returned by each shaping
result. The caller providing the backend owns native glyph/run data, cache
storage and eviction policy. The engine retains shared entry references and
assigns compact document-local paint IDs.

- `NativePaintResource` shares an existing `Arc<T>` without wrapping or retaining
  a cache registry. `ShapedGlyph::retained` and `shaped_run_retained` accept the
  caller's `Arc` directly (or an already erased entry reference). Painters can
  recover that original typed `Arc` through `downcast` without copying native
  data. Native payloads need not implement `Clone`. Retained payloads keep the
  existing `Send + Sync` boundary; borrowed callbacks retain their previous
  lifetime/thread support.
- `NativePaintResources` is now the engine's compact table of registered entry
  references. One run occupies one entry, regardless of character count. Run
  lookup remains constant-time; scalar lookup uses the sorted glyph intervals.
  The table contains no backend cache keys or registry ownership. Entries and
  their distinct native payload types share one reference-counted table, so
  binding a table needs one `Arc` clone. Compatibility checks inspect distinct types once
  per binding, rather than scanning document entries or allocating per command.
  Hot lookups allow cross-crate inlining; geometry and native arrays stay shared.
- Retained glyph handles and run IDs are assigned by the engine. Backend cache
  indices do not escape into retained document paint commands. Single-character
  interning uses native-entry identity plus measurements, so independent cache
  entries cannot alias through reused backend handles.
- GPUI returns the existing `Arc<GpuiTextRun>` / `Arc<LineLayout>` cache entries
  directly. There is no extra `Arc<GpuiGlyph>` wrapper allocation. Painting
  moves the recovered typed reference into commands, with one reference-count
  increment per painted fragment/glyph. Zero-ink glyphs share one empty marker
  per cache. Scalar cache hits use a compact record index for constant-time
  lookup; the result handoff does not search the glyph intervals.
  Removed `GpuiTextResources`, the pass snapshot accessor/hook and publication
  snapshot collection. Backend registries remain private cache implementation
  details; clearing/replacing them does not release entries retained by layouts.
- Metric rollback truncates entry references together with registrations. An
  unchanged cache prefix keeps entry identity; partial truncation changes only
  range metadata. A no-op entry-table truncation avoids copying shared storage.
  Existing note and scene ownership checks remain.
- Borrowed-handle constructors remain supported. One document must consistently
  use retained entries or borrowed handles; mixed ownership is rejected before
  registration to prevent overlapping paint-ID namespaces.

`ShapedGlyph` now implements `Clone` rather than `Copy` because it may hold an
entry reference; migrated the existing fixed-result fixture. The backend trait
is back to **14 methods** after removing the whole-cache snapshot hook. Active
cache transactions and metric reclamation remain separate follow-up concerns.
No traits were introduced to hide pipeline stages, and no native cache policy
was added to the engine.

Validation: tree-sitter and rustfmt parsed all seven changed Rust files; source
reference checks found no remaining whole-cache snapshot type/hook consumers.
All 87 existing test names/counts in the affected files are unchanged; no test
cases or test-only production helpers were added. **Compilation, tests and
benchmarks were not run.** Source checks do not establish type correctness,
runtime behavior or performance. The performance review removed avoidable
wrapper allocations, duplicate paint-time reference increments and new binary
searches on shaping cache hits. Ownership still adds entry references and
registration metadata; no measured claim of unchanged performance is made.

Before snapshots, scoped diff and source reports: `/home/johan/.cache/htmlengine-native-entry-references-20261106-031557`.

## Direct GPUI native results without duplicate registries (2026-11-06)

Removed the GPUI run-ID round trip and the parallel native glyph/run registries.
`shape_line_geometry` returns `(ShapedTextRun, GpuiTextRun)` directly. Synthetic
caps remaps that returned native data and geometry directly; only the final
retained shaping result allocates its `Arc<GpuiTextRun>`. The engine assigns the
published run ID during registration; the adapter's geometry uses an unassigned
placeholder rather than an index into a native registry.

- Removed `GpuiGlyph`, `GlyphStorage`, `GlyphRecord`, `GlyphStore`, `RunStore`
  and `shaped_text_run_from_line`. Normal runs no longer append to either a
  native-run vector or a second glyph-record vector. There are no backend run
  lookups or extra native-reference clones during the result handoff.
- The scalar cache owns its native entry reference and measurements directly.
  A hit uses one map lookup and clones that shared reference. Zero-ink results
  still share one marker per backend. Font changes can clear the cache without
  releasing native data retained by documents or scenes.
- Generation checks, document/append transactions and scope reclamation remain.
  Logical glyph/run append counters preserve checkpoint comparisons without
  retaining native runs. Scalar cache entries retain their logical start, so
  append rollback and note reclamation remove suffix cache entries. Document
  rollback moves back the previous scalar cache, generation and counters.
  Engine entry-reference and metric reclamation are unchanged.
- `TextShapeCacheStats::backend_glyph_records` now reports actual scalar cache
  entries; native runs allocate no backend registry records. `registered_glyphs`
  remains the active generation's logical registration count. The backend trait
  remains at 14 methods; this change adds no trait or public API.

This removes **114 physical production Rust lines** across the two changed
adapter files, excluding existing test modules. Normal native shaping loops and
shared geometry arrays are unchanged. The source review confirms removal of two
run registry writes, their retained references and lookup, plus the intermediate
native `Arc` allocation for synthetic caps. Runtime performance was not measured.

Validation: tree-sitter and rustfmt parsed both changed Rust files, removed-symbol
searches found no live callers, and scoped whitespace checks passed. Migrated the
existing geometry/ownership fixtures to direct native results and retained engine
references; all 19 existing test names/counts are preserved. No test cases or
test-only production helpers were added. **Compilation, tests and benchmarks
were not run.** Source checks do not establish type correctness, runtime behavior
or unchanged performance.

Before snapshots, scoped diff and source reports: `/home/johan/.cache/htmlengine-direct-native-runs-20261106-035925`.

## Infallible native shaping and caller-owned cache lifetime (2026-11-06)

Native character shaping, run shaping and font metrics now return `ShapedGlyph`,
`Option<ShapedRun>` and `FontRelativeMetrics` directly. Run conversion can still
choose the scalar fallback with `None`. GPUI's internal line/caps conversion
returns `Option` directly. Native shaping no longer exposes a recoverable error
channel for failures the implementations never produced.

`GlyphShaper` has **9 methods, down from 14**. Removed document/append begin,
commit and rollback callbacks. Added one non-transactional `finish_shaping`
notification so GPUI releases native scratch once at batch end on success or an
engine/layout error, preserving scratch reuse between runs. The private engine
publication guards invoke it; no per-run scratch clearing was added. No traits
were introduced to hide pipeline stages.

- Engine metric/entry registration still reports capacity and ownership errors.
  First-line layout convergence and other stage errors remain reported at engine
  boundaries. `ShapeError` is no longer re-exported from `html::text`; document
  errors still preserve the original engine diagnostic chain. Its capacity
  constructor is now crate-private, and unsupported-append errors are removed.
- Private engine guards preserve the committed ledger for a failed same-document
  candidate, or discard partial appended registrations on layout failure. They
  never snapshot, reset, commit or roll back caller cache contents. Optional
  checkpoint reclamation still protects note batches and borrowed native handle
  prefixes; explicit reset/generation invalidation remains supported.
- A failed initial/new document request has no published document. For source,
  base URI or markup-syntax replacement failures, the session discards the old
  publication, resource ledger, scene leases and note batch, while retaining the
  requested inputs for retry. Failed same-document configuration changes retain
  their committed inputs. Worker preparation applies the same replacement rule.
- `ResourceShaper` callbacks and `GlyphWriter::insert` are infallible. Native
  resources may still borrow platform state and need not implement `Clone` or
  `Send + Sync`. The basic adapter retains values until explicit reset or note
  checkpoint reclamation. Exhausting its 32-bit native handle space now triggers
  an invariant panic; engine metric-capacity exhaustion remains a reported error.
- GPUI drops the previous-document/append state and moves font-cache refresh to
  publication-font selection. The harness keeps its book-wide handle namespace
  without transaction checkpoints. Native caches can now survive document work
  and failures; font changes, explicit reset and optional note reclamation govern
  their retention. This changes cache lifetime, and its memory/performance effect
  has not been measured. Native geometry loops, shared arrays, compact entry
  storage, constant-time run lookup and the native reference handoff are unchanged.

Migrated live adapters, profiling/example callers and existing fixtures in all
three workspaces, including the shared `test-support/stage-fixtures.rs`. Removed
10 obsolete existing cases that depended on injected native shaping errors or
the deleted transaction protocol. Remaining fixture behaviors were migrated;
no new test cases or test-only production helpers were added. The affected Rust
files contain 731 existing cases after this cleanup; none were run.

The change removes **257 physical production-source Rust lines**, excluding test
files and `cfg(test)`/testing-only items. Source validation parsed all 38 changed
Rust files with tree-sitter and rustfmt, audited 32 live backend implementations
and 46 native declarations, checked caller chains and removed symbols, and passed
task-scoped whitespace checks. **Compilation, tests and benchmarks were not run.**
These checks do not establish type correctness, runtime behavior or unchanged
performance.

Before snapshots, scoped diff and source reports: `/home/johan/.cache/htmlengine-infallible-shaping-20261106-055023`.

## Engine-owned note reclamation (2026-11-07)

Note batches now track the engine ledger prefix, its appended end and the
owner's shaping generation independently of native checkpoint support. On the
next update, note layout or rehydration, a closed batch releases its engine
metrics and retained entry references even when the backend supplies no native
checkpoint or refuses native reclamation. Owner or ledger-boundary changes
abandon reclamation so resources used by the published layout survive.

One shared lifetime token per batch replaces the vector of weak tokens. Every
note and its retained render-resource leases keep that token alive. Collection
checks one weak token; it does not scan all previous notes.

Borrowed adapters keep optional native checkpoint cleanup. Foreign native
allocations disable only native truncation, including before another note joins
the batch; they no longer prevent engine cleanup. `ResourceShaper` keeps its
borrowed-value storage and existing prefix-preserving reclamation hooks.
`GlyphShaper` remains at **9 methods**; GPUI implements **7**, relying on the
defaults for the two optional native checkpoint methods.

Removed GPUI's checkpoint methods, logical glyph/run counts and cached scalar
append offsets. Its scalar cache survives note closure independently of engine
registrations. Removed the synthetic `TextShapeCacheStats::registered_glyphs`
field and migrated the live reader-load profiling caller; `backend_glyph_records`
continues to report actual cached scalar entries. Native run/scalar shaping is
identical apart from removed checkpoint bookkeeping. The GPUI README now describes
retained entries and engine binding instead of the removed backend snapshot API.

Net change: **27 fewer production Rust lines**, excluding existing test modules.
Migrated the two existing GPUI ownership fixtures; all **74 existing cases** in
the changed Rust files retain their names and count. No cases or test-only
production helpers were added. Tree-sitter and rustfmt parsed all four changed
Rust files, live caller searches found no removed statistics/checkpoint uses,
and scoped whitespace checks passed across both affected workspaces.
**Compilation, tests and benchmarks were not run.** Source checks do not establish
type correctness, runtime behavior or unchanged performance.

Before snapshots, scoped diff and source reports: `/home/johan/.cache/htmlengine-note-cleanup-20261107-142101`.

## Caller-owned cache reset (2026-11-07)

Removed `reset` from `GlyphShaper`: the trait now has **8 methods**, of which
only `resource_generation` and `shape_glyph` are required. The engine does not
clear caller caches. Backends report invalidated borrowed handles by changing
their resource generation, and existing engine generation checks are unchanged.

`GpuiTextCache::reset` and `ResourceShaper::reset` are public inherent methods
with the same bodies as their former trait implementations. The live native
shape profiler continues to call GPUI's concrete method. Deleted the other
**30 backend reset implementations**, including unused harness/profile methods
and fixture boilerplate. Three existing invalidation fixtures now create fresh
concrete shapers to exercise generation mismatch; no reset helper, new trait,
test case or test-only production code was added.

Source audit covered 1,378 live Rust files and all 32 backend implementations.
Every backend method other than the removed reset is byte-for-byte unchanged;
the two retained concrete reset bodies are also unchanged. Updated the adapter
README and native-resource generation contract to document caller ownership.
Net change: **13 fewer production Rust lines**, excluding fixtures and test-only
items. The 28 affected Rust files retain all **730 existing test names/counts**.

Validation: tree-sitter and rustfmt parsed every changed Rust file; method and
caller audits found no remaining reset in the shaping trait or its live
implementations, and scoped whitespace checks passed across all three
workspaces. **Compilation, tests and benchmarks were not run.** These checks do
not establish type correctness, runtime behavior or measured performance.

Before snapshots, scoped diff and source reports: `/home/johan/.cache/htmlengine-caller-reset-20261107-151424`.

## Unassigned retained run keys (2026-11-08)

Retained GPUI shaping no longer constructs placeholder `TextRunId(0)` values.
Normal shaping, synthetic caps and the existing geometry conversion fixture use
`shaped_text_run_with_carets`, which returns advances, cluster boundaries, carets
and ascent with no paint key. Native entries remain caller-owned references.
`GlyphResourceStore::shape_run` assigns the real document-local key after the
nonempty retained run is registered in its native entry table.

The private native geometry key is now `Option<TextRunId>`. Paint run views
require `Some(key)`; unregistered geometry cannot be interpreted as native run
zero. Caret/ascent accessors, spacing and grapheme adjustments continue to work
before registration. The profiler can still consume `shaped_run_into_geometry`
without registering resources. Borrowed backends keep
`shaped_text_run_with_backend_run`, preserving the supplied key as `Some(key)`.
`GlyphShaper` remains at **8 methods**, with 2 required.

The new constructor moves existing arrays and caret storage without allocating.
Native shaping, metrics, spacing and geometry loops are unchanged except for
removing placeholder constructor arguments. Run-key metadata is now optional;
its binary layout and runtime performance have not been measured. This is a
contract/state cleanup, with **21 additional production Rust lines** for the
unkeyed constructor, documentation and validity checks.

Validation: surveyed 1,378 live Rust files, checked all new-constructor/key-access
callers and borrowed-key construction, and compared shaping/geometry functions
against task-start snapshots. Tree-sitter and rustfmt parsed all 3 changed Rust
files; all 26 existing test names/counts are preserved. No test cases or test-only
production helpers were added. Scoped whitespace checks passed.
**Compilation, tests and benchmarks were not run.** Source checks do not establish
type correctness, runtime behavior or unchanged performance.

Before snapshots, scoped diff and source reports: `/home/johan/.cache/htmlengine-retained-run-keys-20261108-175342`.

## Shared scalar/run paint ownership (2026-11-08)

`ShapedGlyph` now stores one crate-private `NativePaintSource`: either a borrowed
handle or a retained native entry. Run results use that same enum instead of a
separate `RunPaintSource`. The retained scalar constructor no longer fills an
unused `GlyphHandle(0)`; public constructor signatures are unchanged.

Removed the three private scalar source accessors (`resource`, `into_resource`,
`handle`) and folded the one-call `register_shaped` helper into `shape_glyph`.
Scalar interning still uses the same borrowed handle/retained pointer identity,
character and metric bits. A cache miss checks ownership before registering
metrics, moves the native source into its paint mapping/table, then records the
interned ID. Borrowed native handles and retained entry identity stay intact.

No native resource clones, allocations, geometry passes or backend callbacks
were added. Other shaping-contract functions are unchanged apart from the shared
enum name. `GlyphShaper` remains at 8 methods, with 2 required. No traits or public
API types were added. Net change: **22 fewer production Rust lines**, excluding
test-only items.

Migrated the existing borrowed-storage fixture away from the removed accessor,
including its stale unwrap on an infallible shaping result. All 9 existing test
names/counts in the affected files are preserved; no test cases or test-only
production helpers were added. Tree-sitter and rustfmt parsed both changed Rust
files. Public signatures, registration order, resource moves and removed-symbol
searches were checked against task-start snapshots; scoped whitespace checks
passed. **Compilation, tests and benchmarks were not run.** These source checks
do not establish type correctness, runtime behavior or measured performance.

Before snapshots, scoped diff and source reports: `/home/johan/.cache/htmlengine-scalar-source-20261108-191429`.

## Resource and interaction review fixes (2026-11-08; source-only)

- Engine ledgers now carry distinct identities. Shaped products and scopes
  validate those identities independently of the shared backend generation.
- Append checkpoints track discarded prefixes, reject wrong-ledger and stale
  checkpoints after regrowth, preserve surviving outer checkpoints, and return
  immediately for a valid no-op rollback. No registration adds branch metadata.
- Removed the two borrowed-native checkpoint hooks and their token type.
  Native callbacks own their storage until explicit reset. `GlyphShaper` is
  reduced from eight methods to six, with two required.
- Notes own independent registration ledgers. Removed parent scope batching,
  retired-note accumulation and the reader's cross-document-popup image-relayout
  workaround. Scene leases retain scope ledgers and track parent invalidation.
- Bounded the GPUI scalar cache at 4,096 entries. Old entries are evicted in
  batches using the existing miss counter; cache hits retain one lookup.
  Referenced native entries survive eviction without copying glyph data.
- Engine memory reports include retained native table metadata and checkpoint
  history, deduplicating shared tables. Native payload sizes remain opaque and
  belong to adapter accounting.
- Continuous fragments delegate selection painting and plain-text clipboard
  export to the engine, record overlays only when interaction or colors change,
  and reuse paint scratch and command buffers.
- Selection controllers now own per-view link hover for pages, notes and
  fragments. Movement updates selection and hover together. Pointer leave and
  window exit clear the underline without ending a captured drag. Text color
  overrides also apply to hover ink; nonselectable fragments can still hover.
- Updated current ownership documentation and renamed profiling batch timing
  from `batch_commit_ms` to `batch_finalize_ms`, matching scratch finalization.

No tests or test-only production helpers were added. Existing fixtures were
migrated only where the contract changed. Verification is limited to Rust source
parsing, caller/signature and removed-symbol audits, fixture inventory and scoped
whitespace checks. **Compilation, tests and benchmarks were not run.** No measured
performance claim is made. The changes retain lazy placement traversal, reused
paint buffers and caller-owned native data.

Before snapshots and audit reports:
`/home/johan/.cache/htmlengine-review-cleanup-20261108-193452`.

## Hover and interrupted-gesture fixes (2026-11-08; source-only)

Link/content hit testing now replays cloneable placement iterators rather than
collecting a temporary vector. Paint-rank precedence, intrinsic clipping and
text-over-image rules are retained. Idle pointer movement in scopes and native
fragments uses the existing vertical spatial index, which includes tall and
overflowing image carriers. Active gestures retain the full nearest-text path.

Note movement now receives the host's current left-button state and finishes a
missed release before any selection update. GPUI routes moves outside the panel
so re-entry can recover a release missed outside the window; explicit leave
continues to clear hover without ending a held drag. Fragment recovery also
finishes before extending selection. The native harness forwards its button state.

Fragment interaction is always initialized, independently of optional focus.
Only selectable fragments allocate a focus handle. Disabling selection clears
text/table state and releases focus while preserving hovered links. Existing
fixtures use the focus-independent interaction state; no cases or test-only
helpers were added.

Source parsing, caller/signature checks, fixture inventory and scoped whitespace
checks were performed. **Compilation, tests and benchmarks were not run.** Source
checks do not establish type correctness, runtime behavior or measured performance.
Before snapshots and audit reports:
`/home/johan/.cache/htmlengine-interaction-fixes-20261108-201349`.

## Run-only shaping and remaining review fixes (2026-11-08; source-only)

- Removed `GlyphRequest`, the scalar backend method/factory and the engine's
  character-shaping entry point. All requests use required `shape_run`, including
  synthetic hyphens, ellipses, mirrored characters and source-only soft hyphens.
  Existing adapters, consumers, profiling tools and fixtures were migrated.
- Run results contain their paint entries. Basic adapters measure each distinct
  character once per run; the engine registers each once. Native results retain
  shared geometry and caller-owned paint data. GPUI caches complete one-character
  runs with full feature, direction, caps and joining keys, retaining its bounded
  eviction policy. Repeated isolated results reuse engine registrations.
- Mandatory line breaks split contextual text runs instead of forcing the whole
  span through individual-character shaping. Synthetic characters receive the
  same resolved font/style features as authored text.
- Link fragments are percent-decoded. Reader navigation uses the resolved resource
  identity, including the document's effective `<base href>`.
- Publication font registration accepts changed loaded content for an unchanged
  declaration. Native registration releases the adapter's redundant byte reference
  after decoding width capabilities; metadata and native registration remain.
- Escape clears fragment selection and preserves hover. Scope relayout clears stale
  hover while preserving selection. Idle off-panel note movement skips hit testing
  and repaint churn, retaining captured-drag and missed-release recovery behavior.
- Command painters allocate no empty native paint-resource table before binding.

No tests or test-only production helpers were added. Existing fixtures were
migrated for the new contract. Verification used Rust source parsing, signature
and removed-symbol audits, fixture inventory and scoped whitespace checks.
**Compilation, tests and benchmarks were not run.** These checks do not establish
type correctness, runtime behavior or measured performance.

Before snapshots and audit reports:
`/home/johan/.cache/htmlengine-run-cleanup-20261108-212516`.

## Feature-aware metrics and link edge cases (2026-11-08; source-only)

Font-relative metric requests now carry resolved OpenType features. Their cache
keys borrow all five CSS feature groups, so feature differences cannot reuse
incompatible measurements and cache hits allocate no feature vectors. GPUI uses
its prepared font cache for measurement; nondefault space features use a shaped
probe while default spacing retains the direct native advance path.

Link resolution removes any existing fragment from the base URI before resolving
the new href, preserving query strings. Native runs containing at most one scalar
skip grapheme-boundary allocation; multi-character CRLF and combining sequences
retain the existing boundary processing.

Source parsing, changed-source audits and scoped whitespace checks were used.
No tests or test-only helpers were added. **Compilation, tests and benchmarks were
not run.** Before snapshots and audit reports: `/home/johan/.cache/htmlengine-metric-link-fixes-20261108-214808`.

## Small-caps fallback fixes (2026-11-08; source-only)

Native small-caps capability probes now apply the same publication `unicode-range`
filtering as display-text shaping. Filtering happens after the capability-cache
lookup; cache hits keep their existing path. Active publication or loaded-font
changes already invalidate capability results.

Caps results preserve their full-size primary-font strut and include the shaped
run's ascent/descent, so taller fallback faces and independent CJK fonts contribute
to the returned geometry and paint metrics. This uses existing run aggregates;
it adds no glyph traversal or shaping call.

Source parsing, changed-source audits and scoped whitespace checks were used.
No tests or test-only helpers were added. **Compilation, tests and benchmarks were
not run.** Before snapshots and audit reports: `/home/johan/.cache/htmlengine-caps-fallback-fixes-20261108-222206`.

## Main-page missed-release recovery (2026-11-08; source-only)

`RendererSession::pointer_move` now receives the current primary-button state.
When a press is pending but the button is released, it clears the pending press
and finishes the engine gesture before hit testing the move. Finishing preserves
the existing selection and publishes its normal completion event without link
activation. GPUI clears its capture and tap state on that unpressed move, so a
later mouse-up cannot activate the old gesture or emit a document tap.

Held drags still continue outside the hitbox; hover continues after recovery and
pointer leave retains its existing behavior. The harness and the two existing
fixture call sites were migrated to the new signature. No new methods or tests
were added.

Source parsing, caller/signature audits and scoped whitespace checks were used.
**Compilation, tests and benchmarks were not run.** Before snapshots and audit
reports: `/home/johan/.cache/htmlengine-page-release-fix-20261108-224626`.

## Font-level caps capabilities and shared uppercase mapping (2026-11-08; source-only)

Native caps support now comes from the supplying face's feature metadata. GPUI
exposes a substitution-feature query implemented by its Cosmic, CoreText and
DirectWrite backends; the inert backend reports no support. Cosmic reads Swash
feature metadata and the other backends inspect borrowed GSUB tables. Unchanged
glyphs are no longer treated as evidence that a font lacks a feature.

The adapter selects the face using CSS Unicode subsets and native fallback.
Face selections and per-font/tag capabilities have bounded caches, invalidated
with publication/fallback font changes. Missing primary glyphs require at most
one uncached fallback-selection layout per cached character; capability checking
itself never shapes text.

Text preparation and synthetic caps now share the scalar uppercase fallback and
contextual Greek uppercase mapping from `html-text`. Greek mapping is computed
lazily only when synthesis is needed. Removed marks and expanded forms retain
logical scalar slots. Cached one-character runs distinguish Greek casing, so a
result from another language cannot mask the tailored form.

The direct `html-text` dependency uses the already locked package and versions.
The GPUI repository's required README review marker was already present and was
preserved. No tests or test-only helpers were added. **Compilation, tests and
benchmarks were not run.** Source parsing, contract/caller and dependency-edge
audits, existing-fixture inventory and scoped whitespace checks were used. The
local tree-sitter grammar still rejects one unchanged preexisting trait signature
in GPUI's `platform.rs`; rustfmt parses that file and all changed Rust files.
Before snapshots and audit reports: `/home/johan/.cache/htmlengine-caps-capabilities-20261108-235153`.

## Fragment external links and component theme (2026-11-09; source-only)

`HtmlFragmentElement` now opens external HTTP(S), mail and telephone links through
GPUI's system URL handler. Link presses are independent of text selection; a
matching hovered destination is required on release. Movement beyond the drag
threshold, pointer leave, source/layout replacement and missed-release recovery
cancel pending activation. Selection still receives the final release position
outside the hitbox. Internal fragment and relative-document navigation remain
host concerns.

`SelectionController::hovered_link` resolves its retained hovered text range on
demand, so the adapter does not repeat engine hit testing or allocate URLs during
hover. The adapter retains one owned destination only during a link press.

The prototype component's fragment stylesheet overrides the default blue,
permanently underlined link rule. Links inherit the component's foreground and
use the existing theme-colored hover underline. The generic HTML default remains
browser-compatible; paint-time text overrides still do not recolor CSS fills.

Rustfmt/source parsing and scoped whitespace checks passed. Existing tests were
unchanged. **Compilation, tests and benchmarks were not run.** Before snapshots
and audit reports: `/home/johan/.cache/htmlengine-fragment-links-20261109-002619`.

## Local audit artifacts

Exact before snapshots, source diffs, and audit reports for recent changes are under `/home/johan/.cache/`. These are local working artifacts and may not exist on another machine.

- `htmlengine-typed-font-load-request`
- `htmlengine-opaque-gpui-resources`
- `htmlengine-exclusive-painted-hit`
- `htmlengine-private-scene-commands`
- `htmlengine-paired-media-validity`
- `htmlengine-canonical-pipeline-outcome`
- `htmlengine-paired-shaping-validity`
- `htmlengine-pipeline-session-overlap`
- `htmlengine-opaque-caret-storage`
- `htmlengine-typed-line-ids` (`before/` snapshots, `paths.json`, `diff.patch`)
- `htmlengine-typed-image-ids` (`before/` snapshots, `paths.json`, `diff.patch`)
- `htmlengine-typed-box-ids` (`before/` snapshots, `paths.json`, `diff.patch`)
- `htmlengine-owned-publication-fonts` (`before/` snapshots, `paths.json`, `diff.patch`)
- `htmlengine-engine-font-selection` (`before/` snapshots, `paths.json`, `diff.patch`)
- `htmlengine-renderer-effects` (`before/` snapshots across all three repositories, `paths.json`, `changed.json`, `diff.patch`, source audit report)
- `htmlengine-release-checks` (`before/` snapshots for compilation-driven fixes, `paths.json`, `diff.patch`, release check logs and `checks.json`)
- `htmlengine-hyphenation-once-cell` (`before/` snapshots, `paths.json`, `diff.patch`, `source-audit.txt`)
- `htmlengine-explicit-mutation` (`before/` snapshots across engine and harness, `paths.json`, `changed.json`, `diff.patch`, `source-audit.txt`)
- `htmlengine-review-fixes` (`before/` snapshots across all three repositories, `paths.json`, `changed.json`, `diff.patch`, `source-audit.txt`)
- `htmlengine-review-simplification` (`before/` snapshots, `paths.json`, `changed.json`, `diff.patch`, `source-audit.txt`)
- `htmlengine-review-simplification-2` (`before/` snapshots, `paths.json`, `changed.json`, `diff.patch`, `source-audit.txt`)
- `htmlengine-simplification-fixes` (`before/` snapshots, `paths.json`, `changed.json`, source diffs, `metrics.json`, `source-audit.txt`)
- `htmlengine-review-round-2-fixes` (`before/` snapshots, `paths.json`, source diffs, `syntax.json`, `source-audit.txt`)
- `htmlengine-release-followup-20261103-202600` (`before/` snapshots, `paths.json`, compilation logs, `checks.json`, source diffs, `source-audit.txt`)
- `htmlengine-gpui-cache-fixes-20261103-205043` (`before/` snapshots, `paths.json`, source diffs, `syntax.json`, `source-audit.txt`)
- `htmlengine-fragment-recovery-20261103-215352` (`before/` snapshots, `paths.json`, source diffs, `syntax.json`, `source-audit.txt`)
- `htmlengine-api-surface-20261103-222320` (`before/` snapshots, `paths.json`, source diffs, `syntax.json`, import audits, `source-audit.txt`)

- `htmlengine-session-api-20261103-224445` (`before/` snapshots, source diffs, `syntax.json`, `metrics.json`, reader import audit, `source-audit.txt`)

- `htmlengine-reader-consolidation-20261103-225842` (`before/` snapshots, source diffs, `syntax.json`, `metrics.json`, import audits, `source-audit.json`, `source-audit.txt`)

- `htmlengine-reader-api-boundary-20261103-232542` (`before/` snapshots, source diffs, `syntax.json`, `metrics.json`, import audits, coordinate/source audit)

- `htmlengine-facade-boundary-20261103-235402` (`before/` snapshots, `owned-paths.json`, source diffs, syntax/import/source audits)

Each contains `paths.json` and the corresponding snapshots, diffs, or report. The existing facade import audit is `/home/johan/.cache/htmlengine-owned-updates/audit-imports.py`. It checks named imports and is not a compiler. Some older migration scripts mutate source when executed; inspect them before use rather than rerunning them as verification tools.

For current usage and contracts, consult [the facade README](crates/html/README.md), [the pipeline README](crates/html-pipeline/README.md), and [the GPUI adapter README](adapters/html-gpui/README.md).
