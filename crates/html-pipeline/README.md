# skrift-pipeline

Coordinates incremental parsing, styling, shaping, and layout.

This package is an implementation component of [Skrift](https://github.com/JohanAnderssonOstling/HtmlEngine). Applications
normally depend on the `skrift` package, which exposes the document API and
coordinates the supporting crates.

## Direct use

The Rust library name remains `html_pipeline`. Use a dependency alias to match the
existing imports:

```toml
[dependencies]
html-pipeline = { package = "skrift-pipeline", version = "0.1.0" }
```

`PipelineSession` owns document inputs, published products and their caches.
Its `attach` method borrows a text backend for shaping and layout operations;
query inputs, font declarations and published products on the session itself.
Applications use `html::Document` from the `skrift` facade.

Shaping validity stores the backend generation with its cache key. Backend
invalidation clears this record while keeping the last published geometry
available for source queries and failure recovery.

Media-query definitions retain their activation key in the same cache entry.
Updates reuse that key while the preparation remains valid; changed rule
activation or root typography selects style preparation before publication.

Updates return the earliest recomputed stage directly; `EarliestStage::None`
means the existing products were reused. A stage change also recomputes its
dependent stages. The application facade reports `Unchanged`, `Paint` or
`Layout`, including whether logical anchors were preserved.

## License

The Rust code is licensed under GPL-3.0-or-later; see `LICENSE`.
