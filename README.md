# Skrift

Skrift is a framework agnostic HTML and CSS rendering engine built with the purpose of high quality rendering of documents inside native applications.

## Installation

```toml
[dependencies]
html = { package = "skrift", version = "0.1.0" }
```

The published package is `skrift`; its Rust library name remains `html`.
The supporting engine packages use the `skrift-` prefix.

## Document API

Use the `skrift` package, imported as `html`, for document operations
without a reader, pagination, columns, or a UI framework:

```rust
use html::engine::{DocumentOptions, Engine};

let engine = Engine::load(
    provider,
    "chapter.xhtml",
    DocumentOptions::new(600.0),
    &mut shaper,
)?;

// Ranges use logical character indices, not byte offsets.
let selected = engine.selection(10..40).unwrap().with_semantics();
let payload = selected.text(); // plain text and Markdown
```

`Engine::prepare` loads and styles a resource without a text backend. Inspect
`font_faces()`, register those fonts on the host, then call `rehydrate_glyphs`
to finish layout. Resource metadata controls character decoding and HTML/XML
syntax; the engine loads linked and inline CSS.

Semantic selection expands touched list items and table cells, and code blocks
whose boundary the selection crosses. Copy output and highlight queries share
that interpretation. Hosts own gestures, screen projection, and clipboard access.
Use `html::selection::Selection::new(view, range)` with an existing render view.

The same document supports either scrolling or pagination:

```rust
let geometry = engine.geometry().unwrap();
let hit = geometry.hit_test(document_point);
let rectangles = geometry.selection_rects(selected_range);

// A scrolling surface chooses the visible document rectangle and its placement.
engine.painter().unwrap().paint_region(source_rect, destination, &mut painter);

// Annotation anchors combine document-relative source addresses and text quotes.
let anchor = engine.targets().unwrap().capture(range, 48);
```

`PaintProjection` supports arbitrary source clips and destination placements,
including split content and repeated regions. The engine supplies CSS paint order,
overflow clipping, text, images, backgrounds, and borders. A projection can supply
visibility candidates so a page replays only its content in that same CSS order.
`Geometry` accepts `LinePlacement` values when hit testing or querying selections
in a caller-defined arrangement. Both APIs also work in document coordinates.

`selection::SelectionState` and `TableSelectionState` own logical ranges and their
updates. Input bindings, click activation, cursor appearance, and selection handles
belong to the host. `targets()` resolves elements, links, notes, and text anchors;
`resolve_link()` uses the resource provider without initiating navigation.

`engine.search().unwrap().find(query, options)` returns document character ranges
and a logical result cursor. `Search::excerpt(range, before, after)` produces
visible source previews with caller-selected context. Search scope across files,
result UI, and scrolling to a match belong to the application.

`Geometry::table_at(point)` identifies a table cell and its row/column spans.
`table_at_where()` accepts a host's projected-box test for split or repositioned
cells. Existing `RenderTable` exporters provide Markdown and HTML payloads.
`Targets::preceding_anchor()` and `anchor_at_or_before_y()` find preceding element
IDs, with predicate variants for application-selected anchor sets.

`Engine::configuration_changes(DocumentUpdate)` builds synchronized layout, root
font size and CSS media changes while preserving unspecified settings. Hosts apply
them through `apply_changes` after preserving their own position.
`Geometry::range_geometry_lines` batches clipped rectangles for labelled ranges
and caller-selected line placements; overlay colors and order remain host choices.
`Reading::cursor` seeks and steps through logical reading units without playback
or page navigation. `request_images_for_lines` and `request_images_in` replace
the image working set for a caller-chosen interval or document region.

The remaining document operations are grouped behind small borrowed views:

| Entry point | Document responsibility |
| --- | --- |
| `engine.content()` | Export disjoint ranges or caller-selected lines in source order, count visible source characters, and collect image dependencies. |
| `engine.geometry()` | Resolve words under a point, obtain clipped reading-line regions and table-cell identity, and query selection containment. Caller-placed lines work with the same rules. |
| `selection.geometry_plan()` | Merge structurally expanded ranges and separate selected cell boxes from ordinary text highlights. Painting and hit testing consume the same plan. |
| `TableCellRange::cells()` | Enumerate selected cells while respecting row/column spans and table identity. |
| `SelectionAnchor::extend_to()` | Extend a logical word selection around a fixed range or boundary. |
| `engine.structure()` | Index box/line boundaries, describe table span groups and headers, expose authored break constraints, and identify semantic sibling relationships. |
| `engine.reading()` | Obtain reading targets, text, fragments, and bounded synthesis chunks without selecting a page or controlling playback. |

`Structure::index_lines()` indexes only the supplied line interval, including
empty boxes between its owners. Hosts choose the interval and any surrounding
context. Table header inference is explicit through `HeaderPolicy`; repeating
headers, fitting groups, heading keeps, widow/orphan placement, and page breaks
remain consumer decisions. Structural facts carry no page height or column count.

`SearchIndex::load(provider, uri)` provides parse-only search with source addresses,
excerpts, and character statistics. It decodes transport metadata but does not load
CSS, fonts, images, or layout. Its character ranges address normalized parse text,
so consumers use the returned source addresses to resolve matches in rendered
documents. `engine.search()` instead searches visible rendered source text.
Neither API chooses book scope, CFI serialization, batching, or result navigation.

`engine.revisions()` reports document product versions local to that engine;
caches spanning engines also need the host's document identity. Successful pipeline
updates advance affected products, decoded images advance resource/paint versions,
and mutable document access conservatively invalidates layout. Reader page and
overlay revisions remain separate. `Engine::load_timed()` reports measured resource
reading and pipeline durations through `LoadTimings`, without placeholder subphase
counters.

`image_source_request()` resolves one document image into a read that can run on
the host's executor. `image_payload()` borrows decoded raster/SVG data for export;
the host chooses whether to open a viewer, save a file, or write a clipboard.

Each `Engine` owns its image pipeline. Hosts can share an `ImageService` with
`set_image_service()`, select a working set with `request_images()`, and call
`update_resources()` to incorporate decoded intrinsic sizes. `poll_resources()`
instead returns proposed pipeline changes for adapters that preserve a reading
position during relayout. Hosts own execution, repaint scheduling, prefetch policy,
and cache budgets. `resources_pending()` includes deferred layout updates.
`release_resources()` drops local decoded buffers while keeping
the document's intrinsic metrics available.

The optional `font-data` Cargo feature adds `fonts::resolve_face` and
`fonts::prepare_face`: authored font-source fallback, data URL handling and
SFNT/WOFF/WOFF2 validation/decoding. Hosts supply local-font discovery and I/O.
Native font registration, family rewriting and persistent caches remain adapter
responsibilities. See [the document API reference](crates/html/README.md).

## Features

- **Tables**
  - [x] Automatic and fixed table layouts
  - [x] Row and column spans
  - [x] Nested tables, row groups, and column groups
  - [x] Separate and collapsed borders
  - [x] Captions, cell alignment, and layered backgrounds
  - [ ] Cells spanning all remaining rows
  - [ ] Static row and column collapsing

- **Images and SVG**
  - [x] PNG, JPEG, and WebP decoding
  - [x] Optional GIF and AVIF support
  - [x] Inline and external SVG
  - [x] Intrinsic dimensions and CSS size constraints
  - [x] Aspect ratios, image fitting, and positioning
  - [x] Image placeholders and external image loading
  - [ ] Responsive image source selection

- **Mathematics**
  - [x] Inline and block presentation MathML
  - [x] Fractions, roots, scripts, accents, and matrices
  - [x] Stretchy operators
  - [x] Scalable mathematical notation
  - [x] Baseline alignment, scaling, and inherited colors
  - [ ] Arabic script math tokens

- **Bidi and RTL**
  - [x] Mixed LTR and RTL text
  - [x] Text direction and bidirectional controls
  - [x] Automatic direction, text isolation, and overrides
  - [x] Line reordering and mirrored brackets
  - [x] Direction aware tables, flexbox, grid, and lists
  - [x] Contextual text shaping
  - [ ] Vertical and sideways writing

- **Text and typography**
  - [x] Chinese, Japanese, and Korean line breaking
  - [x] Manual and automatic hyphenation for supported languages
  - [x] Justification and paragraph composition
  - [x] `::first-line` and `::first-letter` styling
  - [x] Underline, overline, and line-through decorations
  - [x] Reader font size and color overrides
  - [x] Font loading and fallback
  - [ ] Ruby annotations

- **Layout**
  - [x] Block flow, inline blocks, floats, and margin collapsing
  - [x] Flexbox wrapping, alignment, ordering, and gaps
  - [x] Grid named lines, areas, placement, and spanning
  - [x] Flexible and repeating grid tracks
  - [x] Relative and absolute positioning
  - [x] Overflow clipping and stacking order
  - [ ] Fixed and sticky positioning
  - [ ] Subgrid (shared tracks across nested grids)

- **Lists and generated content**
  - [x] Decimal, alphabetic, Roman numeral, and bullet markers
  - [x] Inside and outside markers
  - [x] Start values, reversed numbering, and item values
  - [x] Generated text before and after elements
  - [x] Quotes, attribute values, and nested counters
  - [ ] Custom counter styles and string markers
  - [ ] Additional standard list numbering styles

- **CSS and painting**
  - [x] Linked and embedded stylesheets and imports
  - [x] Cascade layers and custom properties
  - [x] Media queries and `@supports` for implemented features
  - [x] Solid backgrounds, borders, outlines, and text decorations
  - [x] Rounded corners and overflow clipping
  - [ ] Background images, gradients, and border images
  - [ ] Shadows, opacity, transforms, and filters

## CSS support limits

CSS syntax and typed values are parsed by Lightning CSS. Existing raw-token
conversion is retained for renderer properties such as floats, tables, and
generated content. The engine does not add its own CSS grammar or rewrite
unsupported syntax into substitute values.

Parser acceptance alone does not imply rendering support. `@supports` checks
the implemented property and value, and reports unsupported features as false.
An implemented property can still have unsupported values.
Selector syntax checks follow the native parser; newer pseudo-element chains
that require parser extensions are outside their scope.

| Area | Unsupported features |
| --- | --- |
| Painting | CSS background images, gradients, border images, box and text shadows, opacity, transforms, masks, `clip-path`, and filters including backdrop filters |
| Motion | CSS animations, transitions, and `@keyframes` |
| Layout | Fixed and sticky positioning, subgrid, grid lanes, anchor positioning, `calc-size()`, vertical and sideways writing, and containment beyond `contain: size` |
| Typography | Ruby annotations, advanced baseline alignment modes, custom text overflow strings, fading overflow, custom counter styles, and string list markers |
| Generated content | `content: url(...)`, CSS `reversed()` counters, and alternative text forms such as `content: 'text' / 'alternative'` |
| Colors | `color-layers()`, `contrast-color()`, and relative color values the backend cannot resolve |
| At-rules | `@container` queries, `@page` styling, `@property` registration, `@position-try`, `@custom-media` aliases, and `@counter-style` definitions |
| Parser extensions | Local support for `random()`, `random-item()`, `inherit()`, sibling-counting functions, dimensional-product math, unbounded `clamp()` bounds, `hairline`, extended `flex-basis` keywords, newer whitespace combinations, and percentage text spacing has been removed; accepted syntax follows Lightning CSS |

Document images such as `<img>` and SVG are supported. Ordinary grid tracks,
`text-overflow: ellipsis`, generated text and counters, and HTML reversed lists
are also supported within the feature checklist above. The
[renderer capability checks](crates/03-html-style/src/style/02-syntax/capabilities.rs)
define the implemented CSS subset.

The style crate's [program coordinator](crates/03-html-style/src/style/program.rs)
compiles stylesheets into reusable rule and selector data, parses each document's
inline declarations, and invokes document resolution. Compilation has no DOM
dependency; scope matching and inheritance run during resolution. Both sides
consume [compiled data](crates/03-html-style/src/style/compiled.rs), and lower
modules do not depend on the coordinator.

Built-in, cached, and uncached stylesheets use the same prepared fragment
representation. Caches reuse fragments; one linker evaluates media conditions,
layers, scopes, and source order for the current environment. Media queries retain
Lightning CSS’s native owned nodes and are evaluated against the renderer’s
environment. Resource expansion retains parsed rules through imports and font
URL rebasing, so compilation does not parse serialized CSS again. Source caches
keep unresolved font URLs; each resource context resolves its own URLs.

Logical text preparation records pseudo-style ownership per inline formatting
root. Layout, first-line shaping refinement, and repainting reuse those bindings.
Inline-item context ownership and horizontal edge rules are shared across stages.

A shared [property catalogue](crates/03-html-style/src/style/property_catalogue.rs)
defines typed property identities, aliases, inheritance, dependency phases, and
computed field ownership. Native shorthands are expanded through Lightning CSS;
conversion validates a complete declaration before committing its winning
longhands. Within each element's cascade, variable substitution and native
shorthand extraction run once per selected declaration, with results reused
across dependency phases and fallback retries. CSS-wide keywords use the same
computed-value copying rules.
HTML presentational hints enter the same declaration cascade below author CSS,
so ordinary winner selection also handles their overrides and rollback.
Parent snapshots, initial values, and declaration results share one style
representation. Loading a parent resets declaration-local state, so inheritance
uses computed values rather than reapplying the parent's authored expressions.
Property converters receive resolved metrics directly; the cascade coordinator
resolves their DOM-dependent inputs.

## Additional features

- Knuth Plass paragraph line breaking
- Configurable hyphenation quality safeguards
- Punctuation-aware justification
- Automatic microtracking for justified text
- Proportional reader font scaling, allowing scaling of absolute sized text
- Independent footnote and endnote layout
- Text-to-speech highlighting support
- Structured table copy and paste
- Copy selected content as Markdown

## Benchmark environment

| Component | Version |
| --- | --- |
| GPUI | 0.2.2 |
| Wry binding (`lb-wry`) | 0.53.3 |
| WebKitGTK runtime | 2.54.2 |
| GTK runtime | 3.24.52 |

## Warm engine RAM

<table>
<thead>
<tr><th>GPUI + Skrift</th><th>GPUI + Wry</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><td>88 MiB</td><td>215 MiB</td><td>2.43×</td></tr>
</tbody>
</table>

## Cold first rendered frame

<table>
<thead>
<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th><th colspan="3">Total RAM (MiB)</th></tr>
<tr><th>Skrift</th><th>Wry</th><th>Speedup</th><th>Skrift</th><th>Wry</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/article.html">Article</a></th><td>53</td><td>280</td><td>5.32×</td><td>94</td><td>213</td><td>2.26×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/images.html">Images</a></th><td>44</td><td>308</td><td>7.04×</td><td>83</td><td>208</td><td>2.50×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/tables.html">Tables</a></th><td>45</td><td>284</td><td>6.25×</td><td>98</td><td>219</td><td>2.23×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/languages/rtl-arabic.html">Arabic (RTL)</a></th><td>26</td><td>274</td><td>10.61×</td><td>96</td><td>221</td><td>2.30×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/languages/cjk-japanese.html">Japanese (CJK)</a></th><td>31</td><td>294</td><td>9.58×</td><td>98</td><td>219</td><td>2.23×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/large-document.html">Very large document</a></th><td>682</td><td>593</td><td>0.87×</td><td>285</td><td>299</td><td>1.05×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/books/middlemarch/chapter-37.xhtml">Middlemarch chapter</a></th><td>95</td><td>448</td><td>4.70×</td><td>103</td><td>222</td><td>2.16×</td></tr>
</tbody>
</table>

## Warm first rendered frame

<table>
<thead>
<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th><th colspan="3">RAM before (MiB)</th><th colspan="3">RAM after (MiB)</th></tr>
<tr><th>Skrift</th><th>Wry</th><th>Speedup</th><th>Skrift</th><th>Wry</th><th>Size ratio</th><th>Skrift</th><th>Wry</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/article.html">Article</a></th><td>44</td><td>28</td><td>0.64×</td><td>88</td><td>215</td><td>2.43×</td><td>98</td><td>221</td><td>2.26×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/images.html">Images</a></th><td>38</td><td>21</td><td>0.56×</td><td>89</td><td>215</td><td>2.43×</td><td>102</td><td>223</td><td>2.19×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/tables.html">Tables</a></th><td>40</td><td>29</td><td>0.71×</td><td>88</td><td>214</td><td>2.42×</td><td>99</td><td>221</td><td>2.23×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/languages/rtl-arabic.html">Arabic (RTL)</a></th><td>24</td><td>18</td><td>0.74×</td><td>89</td><td>212</td><td>2.39×</td><td>99</td><td>217</td><td>2.20×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/languages/cjk-japanese.html">Japanese (CJK)</a></th><td>28</td><td>39</td><td>1.42×</td><td>88</td><td>214</td><td>2.42×</td><td>101</td><td>222</td><td>2.20×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/large-document.html">Very large document</a></th><td>680</td><td>382</td><td>0.56×</td><td>88</td><td>213</td><td>2.42×</td><td>286</td><td>299</td><td>1.05×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/books/middlemarch/chapter-37.xhtml">Middlemarch chapter</a></th><td>98</td><td>95</td><td>0.98×</td><td>89</td><td>211</td><td>2.38×</td><td>105</td><td>226</td><td>2.15×</td></tr>
</tbody>
</table>

## Warm whole document render

<table>
<thead>
<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th><th colspan="3">RAM before (MiB)</th><th colspan="3">RAM after (MiB)</th></tr>
<tr><th>Skrift</th><th>Wry</th><th>Speedup</th><th>Skrift</th><th>Wry</th><th>Size ratio</th><th>Skrift</th><th>Wry</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/tables.html">Tables</a></th><td>46</td><td>84</td><td>1.83×</td><td>88</td><td>211</td><td>2.39×</td><td>100</td><td>220</td><td>2.20×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/large-document.html">Very large document</a></th><td>5401</td><td>18275</td><td>3.38×</td><td>88</td><td>208</td><td>2.37×</td><td>288</td><td>349</td><td>1.21×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/books/middlemarch/chapter-37.xhtml">Middlemarch chapter</a></th><td>125</td><td>335</td><td>2.69×</td><td>89</td><td>211</td><td>2.38×</td><td>108</td><td>231</td><td>2.14×</td></tr>
</tbody>
</table>

One sample per engine and document. CPU includes viewport snapshot readback. RAM is total process session PSS including helpers and excluding GPU VRAM. Cold shows total RAM after rendering; warm tables show RAM before and after loading. Cold starts from an empty GPUI window; warm starts after the engine renders an empty document. Layout differences remain between engines.

Raw results: [cold](tools/embed-bench/results/gpui-culled-suite-cold-first-20261027.json), [warm](tools/embed-bench/results/gpui-culled-suite-warm-first-20261027.json), [whole document](tools/embed-bench/results/gpui-culled-suite-warm-whole-20261027.json).

## Executable size

| Application | Stripped executable |
| --- | ---: |
| GPUI + Skrift | 42 MiB |
| GPUI + Wry | 27 MiB |

## Platforms

- Linux
- Windows
- macOS
- Android
- Browser WebAssembly

## Non goals

- JavaScript execution and scripted DOM updates.
- Interactive elements such as editable text fields, buttons, menus, and form
  controls, including validation and submission.
- Browser APIs such as web storage, service workers, and geolocation.
- Audio and video playback.
- Canvas and WebGL applications.
- A complete web browser with tabs, navigation history, and a user interface.

Host applications can provide navigation and input handling around the rendered
document.

Optional GPUI rendering and font/image support live in the separate
[`adapters` workspace](adapters/README.md). Core engine crates remain independent
of GPUI and application repositories.

## Publishing

The engine is released as 13 Cargo packages with one shared version in
`[workspace.package]`. The public application package is `skrift`. Supporting
packages have the `skrift-` prefix, while library names and dependency aliases
preserve existing Rust imports and source directories. GUI adapters, development
tools, and `html-wpt-test-support` remain unpublished.

Each published package includes its README, license text, source, and required
assets. `skrift-format` also includes the STIX Two Math font and its OFL-1.1
license. `skrift` includes mathema's Apache-2.0 source license and provenance.

Inspect the complete release without compiling or uploading:

```bash
cargo package --workspace --exclude html-wpt-test-support --no-verify --allow-dirty
```

`--allow-dirty` is useful while reviewing changes. Actual releases should use a
reviewed, committed checkout. `--no-verify` checks packaging without building;
normal publishing also verifies that the packaged source builds.

Use a crates.io account with a verified email address and authenticate locally
with `cargo login`. With a Cargo version supporting workspace publishing, release
all engine packages together:

```bash
cargo publish --workspace --exclude html-wpt-test-support
```

For individual publishing, use `cargo publish -p PACKAGE` in the following order:

| Package | Rust library name |
| --- | --- |
| `skrift-dom` | `html_dom` |
| `skrift-parse` | `html_parse` |
| `skrift-style-model` | `html_style_model` |
| `skrift-style` | `html_style` |
| `skrift-text` | `html_text` |
| `skrift-engine-model` | `html_engine_model` |
| `skrift-format` | `html_format` |
| `skrift-shape` | `html_shape` |
| `skrift-layout` | `html_layout` |
| `skrift-resources` | `html_resources` |
| `skrift-pipeline` | `html_pipeline` |
| `skrift-render-core` | `html_render_core` |
| `skrift` | `html` |

Normal and build dependencies retain both their local path and a registry version.
Crates.io consumers resolve the registry version; every internal runtime dependency
must be published before its dependents. Workspace-wide publishing handles that
ordering. Keep the shared package version and internal dependency requirements in
sync when preparing future releases.

Path-only dev-dependencies are deliberately local to the checkout. Cargo omits them
from published manifests, avoiding first-release cycles through the pipeline and
keeping WPT support unpublished. Integration tests, benchmark examples, and external
WPT fixtures run from this repository and are excluded from release archives.
