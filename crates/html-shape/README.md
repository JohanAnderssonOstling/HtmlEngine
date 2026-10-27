# html-shape

Plans contextual text runs and invokes a renderer-supplied GlyphShaper over
prepared logical text. It borrows styles, formatting topology and source text,
writing glyph IDs and measurements into separate owned shaping buffers. It
knows no viewport or document lifecycle. Shared renderer contracts and borrowed
views live in html-engine-model. Width-dependent first-line refinement is
requested by html-pipeline, never by a recursive call into layout.
