# skrift-engine-model

Defines shared formatting, glyph, geometry and page-placement data.

`page_placements` owns canonical positioned lines shared by page composition and
host painting. Each page retains neutral column geometry and its paint origin;
page reprojection updates the stored geometry together with its placements.
Native construction, column mapping and cursor queries remain outside the `html`
facade; the facade exposes the result operations used by reader integration.

This package is an implementation component of [Skrift](https://github.com/JohanAnderssonOstling/HtmlEngine). Applications
normally depend on the `skrift` package, which exposes the document API and
coordinates the supporting crates.

## Direct use

The Rust library name remains `html_engine_model`. Use a dependency alias to match the
existing imports:

```toml
[dependencies]
html-engine-model = { package = "skrift-engine-model", version = "0.1.0" }
```

## License

The Rust code is licensed under GPL-3.0-or-later; see `LICENSE`.
