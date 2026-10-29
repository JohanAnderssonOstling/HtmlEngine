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

## Document sessions and content operations

These APIs work without a publication, page, column, or window system:

| API | Responsibility |
| --- | --- |
| `Engine::scope(id, constraints, shaper)` | Independently lay out an element and retain its logical selection. `ScopedContent` provides geometry, a paintable scene, and export. |
| `Geometry::content_at(point)` | Identify visible text, links, images/equations and table cells. `content_at_lines` accepts host-selected line placements and projected cell containment. |
| `AnnotationIndex` | Resolve source/quote anchors, query overlapping ranges or points, and preserve source order with opaque host IDs. |
| `Engine::configuration_changes(DocumentUpdate)` | Build consistent layout, root font size and media updates for the host to apply. |
| `Geometry::range_geometry[_lines]` | Batch clipped rectangles for labelled logical ranges, including overlapping ranges and caller placements. |
| `Reading::cursor(glyph)` | Seek and step through logical document reading units without playback or navigation policy. |
| `Engine::request_images_for_lines` / `request_images_in` | Discover and request images for caller-selected document lines or regions. |
| `DocumentSearch` | Own a document's query, Unicode matching options, results and logical result cursor. |
| `Engine::export().request(target, format)` | Produce plain text, Markdown, table HTML, decoded image data or LaTeX payloads. |

Scopes share DOM, styles and native resources with their owning engine. The text
backend must support append transactions (`ResourceShaper` supplies this lifecycle
for basic adapters). Creating a scope preserves existing resources. Scope queries
return `None` after the owner preparation or native resource generation is replaced.

Refresh search and re-resolve annotation indexes against the current render view
after document edits. `DocumentSearch::refresh` preserves its result cursor where
possible. Annotation source addresses are checked against quoted text and can
recover using surrounding context. An empty result does not initiate navigation.

Export returns `Ok(None)` for absent content or unloaded media, and `Err` for an
unsupported conversion or a conversion failure. `Html` exports table structure;
`AuthoredHtml` retains the original table markup. LaTeX needs `math-export`.

Hosts own publication scope and CFI decoding, screen projection, scrolling,
pagination, popup placement, visual styles, clipboard access and persistence.

`DocumentUpdate` preserves unspecified values and the current media type. An explicit
`height(None)` clears the height constraint. Changes are built separately from
`Engine::apply_changes` so a host can preserve its reading position.

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
