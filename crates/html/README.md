# Skrift

Skrift is a framework-agnostic HTML and CSS rendering engine for native applications.
It provides HTML/XML parsing, CSS styling, text shaping, layout, painting,
selection, search, and document export. Applications supply resource providers,
font shaping, and painting backends.

## Installation

The package is named `skrift`; its Rust library name is `html`.

```toml
[dependencies]
html = { package = "skrift", version = "0.1.0" }
```

Use `html::engine::Engine` for the document API. The [project documentation](https://github.com/JohanAnderssonOstling/HtmlEngine)
includes integration examples and the supported HTML/CSS feature set.

Import supporting contracts from their owning module:

| Module | Contracts |
| --- | --- |
| `html::document` | DOM/source addresses, parsing and `RootFontSize`. |
| `html::resources` | Providers, resource metadata and image loading. |
| `html::style` | Reader style overrides and their option enums. |
| `html::text` | Font declarations, language/shaping values, glyph backends and text matching. |
| `html::layout` | Read-only geometry, text and pagination products. |
| `html::reading` | Reading targets, fragments and cursors. |
| `html::render` | Document painting, scenes, projections, text decorations and fragment rendering. |

Text backends receive resolved font and text properties through `ShapingStyle`.
Glyph requests borrow that record; run requests add text and paint colors. Request construction belongs to the engine implementation,
with no per-property builders in the application facade.
`ShapingStyle::font_features` borrows a resolved `FontFeatures` record: standard
controls are named booleans, with a borrowed extension for font-specific settings.
The engine resolves defaults, CSS overrides and tracking before calling the backend.
Backends apply supported controls and return the actual advances and cluster
geometry used by layout. No capability registration is required; whole-run
shaping remains optional, with per-character measurements as the fallback.

`GlyphShaper::shape_run` receives only a `TextRunShapeRequest` and returns a
`ShapedRun`. The adapter owns native font selection, feature conversion and
caching. `ShapedRun::painted` supplies geometry, measured font extents and the
first of consecutive native paint handles; `ShapedRun::geometry` retains
individual-glyph painting. The engine registers both results and assigns all layout glyph IDs.
Application code using a supplied adapter manages no font keys or native IDs.
Line breaking and placement belong to the engine; the backend has one optional
run callback. `RunFontMetrics` represents uniform or per-character extents from
the actual selected faces, including mixed-font clusters. Compact metric records
retain these shared arrays and a native paint-handle base, keeping engine IDs
independent without per-character handle mappings for painted runs.
Run geometry exposes read-only arrays; spacing and grapheme adjustments belong
to the engine. `FontSlant::Oblique` carries its angle, so ordinary and
italic styles carry no unrelated angle value. Adapters are responsible for
finite numeric values, matching geometry arrays and representable paint-handle
ranges. Run and glyph-metric constructors retain this data without validation
scans. Caret storage chooses dense or compact representation from its memory
cost, using the supplied positions directly.

Applications supply stylesheets and typed overrides. The engine owns style
compilation, cascade resolution, caching, layout constraints and stage selection.
Internal stage types and cache controls are available only in implementation
crates, which profiling tools may depend on directly.

## Document sessions and content operations

These APIs work without a publication, page, column, or window system:

| API | Responsibility |
| --- | --- |
| `Engine::scope(id, width, shaper)` | Independently lay out an element and retain its logical selection. `ScopedContent` provides geometry, a paintable scene, and export. |
| `Geometry::content_at(point)` | Identify visible text, links, images/equations and table cells. `content_at_lines` accepts host-selected line placements and projected cell containment. |
| `AnnotationIndex` | Resolve source/quote anchors, query overlapping ranges or points, and preserve source order with opaque host IDs. |
| `Engine::apply(DocumentUpdate, shaper)` | Apply a batch of input changes, keeping viewport and media inputs consistent and reusing valid products. |
| `Geometry::range_geometry[_lines]` | Batch clipped rectangles for labelled logical ranges, including overlapping ranges and caller placements. |
| `Reading::cursor(glyph)` | Seek and step through logical document reading units without playback or navigation policy. |
| `Engine::request_images_for_lines` / `request_images_in` | Discover and request images for caller-selected document lines or regions. |
| `DocumentSearch` | Own a document's query, Unicode matching options, results and logical result cursor. |
| `Engine::export().request(target, format)` | Produce plain text, Markdown, table HTML, decoded image data or LaTeX payloads. |

`DocumentOptions::new(width)` checks dimensions before resource loading, and
`with_root_font_size(size)` checks typography while preserving whole-pixel
rounding. Initial options carry user stylesheets and reader overrides; authored
inline and linked stylesheets are discovered through the resource provider.

Use `DocumentUpdate` to report new inputs in one batch:

```rust,ignore
use html::configuration::{DocumentUpdate, Update};

let update = engine.apply(
    DocumentUpdate::default()
        .width(480.0)
        .height(Some(800.0))
        .user_styles(vec!["p { line-height: 1.5 }".to_owned()]),
    shaper,
)?;
match update {
    Update::Unchanged => {}
    Update::Paint => { /* repaint existing geometry */ }
    Update::Layout { anchors_preserved } => { /* refresh geometry and restore position */ }
}
```

Omitted fields retain their values. `height(None)` clears a height constraint.
`Engine::configuration()` borrows the committed input values without exposing
cache keys, computed styles or pipeline revisions. Callers can preserve their
reading position before `apply`; invalid values are rejected before the session
changes, and pipeline failures restore the previous committed inputs.

After `Engine::prepare`, load the declared font faces on the host, then finish
with `Engine::apply(DocumentUpdate::default(), shaper)`. The same update operation
handles subsequent changes and notices replaced backend resource generations.

The engine chooses the required work. A width change also updates CSS media
inputs and restyles when media-query matches change. Repeated values reuse the
existing products. Color-only overrides can repaint without changing geometry.
Source edits advance their internal revision automatically. Notify the engine
with `fonts_changed()`, `stylesheets_changed()` or `resources_changed()` when
external data changes at existing identities; callers do not manage counters.

`poll_resources()` reports decoded work and whether intrinsic dimensions need
layout. The engine retains those dimensions until a successful `apply`, so they
can join a viewport or style batch. `update_resources(shaper)` combines polling
and applying for hosts that do not need to preserve reading position separately.
Fixed sizing can accept natural dimensions without a layout pass. Resource
notifications refresh images at unchanged URIs and reject older completions.

Published geometry remains readable after backend invalidation, but
`Engine::painter` is unavailable until shaping succeeds. Retained `RenderScene`
values keep scope resources leased and detect replacement of their paint
products: `is_valid` reports readiness, and `paint` returns false without
submitting stale commands. Successful updates invalidate earlier scenes;
failed updates preserve scenes whose native resources were restored.

`Engine::scope` returns `Result<Option<_>, Error>`:
`Ok(None)` means the target is absent or no laid-out owner is available. Backend
and shaping failures return a typed cause through `Error::kind()` and the
standard error source chain. Image failures use a bounded cache; inspect
`Engine::images().failure(id)` and call `Engine::retry_failed_image(id)` before
requesting that image again.

Scopes share DOM, styles and native resources with their owning engine. The text
backend must support append transactions (`ResourceShaper` supplies this lifecycle
for basic adapters). Creating a scope preserves existing resources. Scope queries
return `None` after the owner preparation or native resource generation is replaced.
Backends with resource checkpoints reclaim closed scope batches before the next
scope or update. `ResourceShaper` and the GPUI adapter support these checkpoints;
live scopes and retained painter snapshots keep their resources valid.
Ordinary resizing preserves scope reclamation tracking. A scope whose intrinsic
image sizing became stale returns `None` until it is recreated. Scoped scenes
prefer the owner's decoded images and preserve inherited equation colors.

Optional parallel table measurement retains one worker-pool configuration.
Changing the worker limit releases the previous pool after in-flight work
finishes; failure to create workers uses sequential measurement.

Painting adapters depend directly on `skrift-render-core`. Its backend contract
contains `Painter`, resolved corner radii, text-run batches and command recording;
these are not reexported as application configuration APIs.

Painters must implement clipping. Advertising native text support requires a
range-aware fragment implementation or a prepared-batch override; drawing an
entire run is not a substitute for drawing one of its slices.

Refresh search and re-resolve annotation indexes against the current render view
after document edits. `DocumentSearch::refresh` preserves its result cursor where
possible. Annotation source addresses are checked against quoted text and can
recover using surrounding context. An empty result does not initiate navigation.

Export returns `Ok(None)` for absent content or unloaded media, and `Err` for an
unsupported conversion or a conversion failure. `Html` exports table structure;
`AuthoredHtml` retains the original table markup. LaTeX needs `math-export`.

Hosts own publication scope and CFI decoding, screen projection, scrolling,
pagination, popup placement, visual styles, clipboard access and persistence.

Batch range geometry returns placement order followed by input range order; labels
are opaque and retain duplicates. Consumers choose colors and overlay priority.
A reading cursor stops at exhaustion and never wraps or moves to another document.
Image convenience requests replace the current working set, including empty sets;
the caller still chooses its visible region and prefetch extent.

With `font-data`, `fonts::resolve_face` tries authored sources using a host
`FontSourceLoader`, handles data URLs, and validates/decodes SFNT, WOFF and WOFF2.
`fonts::prepare_face` also falls back when the host rejects native preparation.
The engine does not fetch HTTP resources, cache files, rewrite family names or
register fonts with a platform.

## Features

- `font-data`: optional font format validation, decoding and authored source fallback with host-provided I/O.
- `math-export`: convert retained MathML to LaTeX on demand, preferring authored TeX annotations.
- `profiling`: enable engine profiling scopes.
- `testing`: expose engine diagnostics for conformance adapters.

## License

The engine code is licensed under GPL-3.0-or-later; see `LICENSE`.
The bundled mathema conversion code is Apache-2.0 licensed; its license and
provenance are in `src/mathema/LICENSE-APACHE` and `src/mathema/UPSTREAM.txt`.
