# skrift-style

Parses CSS, matches selectors, and resolves styles.

This package is an implementation component of [Skrift](https://github.com/JohanAnderssonOstling/HtmlEngine). Applications
normally depend on the `skrift` package, which exposes the document API and
coordinates the supporting crates.

## Direct use

The Rust library name remains `html_style`. Use a dependency alias to match the
existing imports:

```toml
[dependencies]
html-style = { package = "skrift-style", version = "0.1.0" }
```

## License

The Rust code is licensed under GPL-3.0-or-later; see `LICENSE`.
