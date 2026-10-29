# skrift-format

Builds formatting boxes from styled documents.

This package is an implementation component of [Skrift](https://github.com/JohanAnderssonOstling/HtmlEngine). Applications
normally depend on the `skrift` package, which exposes the document API and
coordinates the supporting crates.

## Direct use

The Rust library name remains `html_format`. Use a dependency alias to match the
existing imports:

```toml
[dependencies]
html-format = { package = "skrift-format", version = "0.1.0" }
```

## License

The Rust code is licensed under GPL-3.0-or-later; see `LICENSE`.

The bundled STIX Two Math font is licensed under OFL-1.1; see
`assets/math/OFL.txt`. The font and its license ship with this package.
