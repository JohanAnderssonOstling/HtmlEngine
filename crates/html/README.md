# Skrift

Skrift renders HTML and CSS through host-provided resource, shaping and painting
backends. The package is named `skrift`; its Rust library name is `html`.

```toml
[dependencies]
html = { package = "skrift", version = "0.1.0" }
```

The public `html` facade contains the operations used by HtmlViewCore. Native
adapters, source extraction and diagnostic tools use the implementation crates
for operations outside that boundary.

| Module | Reader contracts |
| --- | --- |
| `html` | `Document`, loading options, preparation, load timings and errors. |
| `configuration` | Committed preferences and batched viewport, typography and note-flow updates. |
| `document` | Typed image identities, root font sizes and source addresses. |
| `resources` | Providers, metadata, image loaders, job spawning and decoded-image views. |
| `style` | Reader overrides and theme preferences. |
| `text` | Font declarations, glyph requests, shaping callbacks and search options. |
| `layout` | Borrowed text, line, box, fragment and addressing products. |
| `pagination` | Page composition, painting/hit-test views, column geometry and navigation queries. |
| `reading` | Reading fragments and targets. |
| `selection` | Per-view interaction controller, selection painting, hit queries and lazy export. |
| `geometry` | Queries over caller-supplied line placements. |
| `search` | Source search, rendered-document search state and compiled visible-text matching. |
| `scoped` | Independently laid-out content sharing its owner's resources. |
| `render` | Painting callbacks, projections, document painting and recorded scenes. |
| `export` | Image and equation requests and portable output payloads. |
| `revisions`, `targets` | Published product versions, source targets and link resolution. |

## Documents and placement

Load a document with `Document::load` or `load_timed`. Initial `DocumentOptions`
carry reader preferences. `Document::prepare` exposes font
declarations before a platform backend is borrowed; `PreparedDocument::finish`
then shapes and lays out the prepared document.

`Document::apply` accepts `DocumentUpdate` and returns `Update::{Unchanged,
Paint, Layout { anchors_preserved }}`. The engine chooses invalidation and stage
reuse. Source replacement and standalone pipeline orchestration are native
implementation operations.

```rust,ignore
use html::configuration::DocumentUpdate;

let update = document.apply(
    DocumentUpdate::default().width(480.0).height(Some(800.0)),
    shaper,
)?;
```

Borrow published content with `Document::render_view`. Its text, pagination,
addressing and geometry products retain stable identities within one layout.
The engine composes pages from host-supplied column geometry and preferences.
Navigation, publication identity and persistent address formats belong to the reader.

`PageView::new(document.render_view(), &page)` borrows a complete engine page.
Its hit queries and `interact` consume page-local coordinates; the engine owns
column choice and document-to-page projection, including fragmented boxes and
repeated header regions. The host removes its view origin and outer display
scale before passing a pointer position.

`PageView` implements the existing `PaintProjection` contract. Pass it directly
to `DocumentPainter::paint` to use the same mapping for content and interaction.
`paint_overlays`, `paint_selection`, selection geometry/hits/copy, range bounds
and `reading_lines` use that engine-owned mapping. Hosts supply style ranges,
colors and policy. Native continuous-fragment adapters can still use `Geometry`
with their own placements.

`TextPatternMatcher` compiles phrases once. `match_lines` searches visible source
text in caller-selected lines. Its result retains the extraction buffers; `iter`
yields pattern identities, glyph ranges, source endpoint addresses and borrowed
matched text. Only text accepted by the application needs copying. The engine
excludes hidden/generated content, preserves Unicode mapping and word-boundary
context, and rejects matches across unpainted gaps.
Readers format source addresses and choose annotation scope and appearance.

`Paginator::new(view, geometry, mode)` creates a lightweight composition view.
`compose(request, positions_buffer, scratch)` accepts `PageRequest::StartingAt`
for a pagination cursor, `StartingAtOffset` for initial/restored document geometry,
or `EndingAt` for a final cursor. The request determines traversal; there is no
separate direction flag. Retain `PaginationScratch` and return the preceding
page's `positions.into_buffer()` to preserve existing buffer reuse.

The returned `PagePositions` contains stable line identities and placements,
start/end cursor anchors, column geometry, paint origin, geometric continuation,
repeated table headers and paint dependencies. Its `request` reproduces the
resolved page, including first-page normalization, and `reached_end` always means
the document end, regardless of traversal direction. Use `next_page_anchor` and
`previous_page_anchor` for page turns; the engine resolves boundaries and preserves
group identity when source-space y coordinates overlap. Forward continuation lives
on the page, so the host need not retain or supply a separate continuation.

The engine owns fitting, CSS forced/avoided breaks, widows/orphans, table
fragmentation and header repetition. `PaginationMode` selects CSS-only web rules,
book keeps, or book keeps with vertical rhythm. The host chooses those preferences,
column geometry, outer display scale, viewport state and page-turn behavior.
Structural records, event assembly and the unbroken-border check are internal.
Retain the returned page intact. `PagePositions::reproject_columns` updates both
placements and retained geometry when the host identifies a width-only change;
`clear` discards the page while retaining its line buffers.

`addressing().destination(DocumentTarget)` resolves glyphs, element IDs and
element source addresses to stable line/cursor geometry or a position fallback.
The reader decides whether and how to reveal the result.

Each view owns a `SelectionController`. Supply pointer down/move/up inputs,
click counts, selection intent and drag distance through `interact`. The engine
owns text/table gestures, word snapping, touch anchors and handle extension.
Outcomes report repaint changes, completed selections and collapsed clicks;
hosts choose link activation, scrolling and clipboard effects.

For composed pages, use `PageView::selection_geometry`, `selection_hit`,
`export_selection` and `paint_selection` with the view's `SelectionController`.
`interact` routes pointer data through the same page, and text serialization
remains explicit and lazy. Selection geometry returns bounds and handle
endpoints together. Native fragment adapters use `SelectionLayout` with their
own projection; normal reader pages need no caller-built layout or projection.
Low-level transitions, table cell ranges and conversion stay internal. Obtain
image/equation export through `Document::export`.

`render::paint_overlays` accepts styled glyph ranges, caller line placements and
a reusable `OverlayPaintScratch`. The engine draws highlights, underlines,
squiggles and strikethroughs with intrinsic clipping. Hosts retain annotation
ownership, search focus, colors and ordering; `OverlayStyle` is the shared style
vocabulary.

`Paginator::compose` accepts a `PageRequest` whose anchor determines traversal.
`EndingAtOffset` resolves restored end offsets inside the engine; backward
composition clamps cursors and normalizes the first page to forward composition
from document zero, preserving authored leading content. `next_page_anchor` and
`previous_page_anchor` return adjacent `PageAnchor` values or a document boundary.
The engine retains exact group identity even when offsets overlap. Native
pagination traversal and decoration candidate queries remain implementation APIs.

`pagination::document_position(render_view, origin, preferred_glyph)` resolves
the current cursor, page glyph and fraction through this document together.
Its `DocumentPosition::source` resolves the source address on demand, keeping
glyph-only queries free of source-path allocations. Empty documents have an
intentional start address; image-only lines retain their own source identity.
Hosts choose settled versus pending anchors and retain publication identity,
CFI formatting, whole-publication progress and notification policy.
`DocumentPosition::reflow_anchor` captures one source address and glyph fallback
for a document update, preserving the true document start for forward requests.

`PageView::text` and `match_text` select visible source content inside the engine.
`reading_start`, `contains_glyph` and source-ordered `reading_lines` provide page
facts for narration and following. Reading bands are returned in one vector;
hosts may scale that vector in place without another allocation.

`Paginator::append_reading_fragments(interval, origin, fragments)` traverses
pages inside the engine, preserving whole words and clipping the requested
glyph interval. `ReadingOrigin` selects the document beginning, a page cursor
or an offset before a page is installed. Without a configured viewport, the
engine reads the interval directly. Hosts retain playback and publication
navigation; synthesis chunking and target queries remain available independently.

`Document::scope` lays out an element independently. `ScopedContent` provides a
render view, recorded scene, `interact` and `paint_selection`. Scopes use the
same controller and keep their own selection. They share their owner's
resources and reject replaced or invalidated products. Resource polling reports
decoded changes separately from pending layout changes. `request_images` accepts
an optional composed page, a page-sized lookahead count and owned scopes. The
engine selects lines and discovers/deduplicates their images; hosts choose
prefetch timing, lookahead and scope ownership.

## Backend ownership

`GlyphShaper` receives resolved properties through borrowed requests and returns
native paint handles and measured geometry. Basic backends return
`ShapedGlyph::new` with `FontMetrics`. Native adapters own font selection,
OpenType conversion, run construction and caches; their implementation helpers
are outside the facade.

`Painter`, `PaintProjection` and `DocumentPainter` keep host placement separate
from CSS interpretation and paint order. Recorded scenes retain resource
validity and refuse to replay invalidated commands. Native resources and their
append/replacement transactions retain rollback behavior without copying their
buffers.

## Implementation crates

Parsing and DOM traversal live in `html-parse` and `html-dom`. Native shaping
operations live in `html-engine-model`; style operations live in
`html-style-model`. Pipeline query functions live under
`html_pipeline::engine`, including `text_paint`, `fragment_paint`, `box_queries`,
`geometry_queries`, `search_queries`, `semantic_queries`, `address_queries`,
`reading_queries`, `source_queries` and `export_queries`. Scene construction and continuous region
painting belong to `html-render-core`. Font-source preparation belongs to the
existing `html-fonts` adapter.

These implementation operations are ordinary functions or crate-private
methods. They are not reexported by `html`, and no traits are introduced just to
hide pipeline stages. The continuous GPUI fragment widget owns a native pipeline
session and retains incremental source and stylesheet updates there.
