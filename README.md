# HTML Engine

Framework-agnostic HTML parsing, styling, layout, resources, and rendering.

Runtime-stage crate directories are numbered in pipeline order:

1. `01-html-source`
2. `02-html-parse`
3. `03-html-style`
4. `04-html-layout` (preparation, shaping, and layout)
5. `05-html-render-core` (paint and scene construction)

The prefixes organize the workspace directories only; Cargo package and Rust
crate names remain stable.

Consumers should depend only on the `html-engine` package and import its
library as `html`. The other workspace crates are implementation details.

```toml
[dependencies]
html = { package = "html-engine", git = "https://github.com/JohanAnderssonOstling/HtmlEngine" }
```

`Engine::rehydrate_glyphs` updates the engine's active layout in place and
returns `Result<(), Error>`. After it succeeds, borrow the document through
`Engine::document()` instead of storing a second owned layout. Integrations
using the earlier returned-document API must update their reader ownership
alongside this engine change.
