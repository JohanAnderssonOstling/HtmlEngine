# html-layout

`html-layout` turns styled DOM content into backend-neutral used geometry and
resolved fragments. It does not own executable painting, UI interaction,
resource loading, or publication-specific addressing syntax.

The pipeline has three typed stages:

- preparation finalizes immutable box topology and inline source;
- shaping owns glyph registration and cluster geometry;
- layout owns reusable box geometry, lines, fragments, semantic indexes,
  algorithm caches, and pass-local scratch state.

Prepared topology is shared by `Arc` and is never cloned merely to attach used
points or sizes. Relayout resets the parallel geometry/output stores and reuses
their capacities. The private `LayoutSession` is the pass composition root;
its storage fields are private and it assembles direct component borrows for
`BlockContext`, `InlineContext`, `TableContext`, `TaffyContext`,
`FragmentContext`, and `FinalizationContext`. None of those contexts stores or
forwards the whole session. They own their cache, checkpoint, fragment, or
finalization operations, while the session retains only recursive whole-pass
coordination. Flex and grid have distinct entry modules over the isolated
shared Taffy adapter, including isolated reusable measurement output.

Consumers enter through `RenderView` and should immediately select the narrow
capability they need: `text()`, `boxes()`, `fragments()`, or `addressing()`.
Those borrowed views allocate nothing and do not expose mutable stage storage.
Layout fragments retain geometry-dependent CSS resolution and semantic
pattern/layer information. They do not tessellate dashed, dotted, or double
decorations: `html-render-core` owns that expansion, color conversion, paint
sequencing, clipping, and painter commands for both retained and interactive
paths. Intrinsic image dimensions enter as a backend-neutral metrics table;
loading and decoding policy remain upstream. `html-view-core` owns EPUB CFI
syntax and resolves it through generic source positions from the addressing
view.

Production code denies `unsafe_code` and the crate denies `unreachable_pub`.
Run `bash HtmlRenderer/scripts/check-html-boundaries.sh` from the workspace root
after boundary changes.
Text tests use deterministic metrics and require no windowing framework.
