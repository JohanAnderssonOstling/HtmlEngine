# HTML Engine

Framework-agnostic HTML parsing, styling, layout, resources, and rendering.

Consumers should depend only on the `html-engine` package and import its
library as `html`. The other workspace crates are implementation details.

```toml
[dependencies]
html = { package = "html-engine", git = "https://github.com/JohanAnderssonOstling/HtmlEngine" }
```
