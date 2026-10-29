# HTML engine integrations

This separate workspace owns optional host integrations. The core workspace does
not depend on GPUI or on an application repository.

- `html-gpui`: continuous `HtmlFragmentElement`, native text shaping, painting and
  image conversion, reused by the reader adapter and engine benchmarks.
- `html-fonts`: publication font I/O, private family preparation and persistent caching,
  using the engine’s optional `font-data` API for decoding and authored fallback.
- `gpui-image-eviction`: shared GPUI image/atlas retention for HTML, PDF and app UI.

Selection uses engine document ranges, Unicode word boundaries, caret geometry,
clipping and plain-text export. GPUI owns focus, gestures and clipboard access.

From the HtmlEngine checkout, run:

```sh
cargo test --manifest-path adapters/Cargo.toml --workspace
```

The sibling `GPUI-Fork` checkout supplies GPUI. Applications select their own
platform features; the libraries disable GPUI's defaults and do not enable a
window-system backend. Applications may enable `gpui-image-eviction/kobo`.

For continuous HTML in any GPUI application, depend directly on
[`html-gpui`](html-gpui/README.md). It has no dependency on Bokheim or
HtmlViewCore. Use `html-view-gpui` from HtmlViewCore only when you also want
reader navigation and pagination.

The [standalone consumer](examples/fragment-app/Cargo.toml) has its own Cargo
workspace and demonstrates a selectable HTML element inside an application-owned
scroll container:

```sh
cargo run --manifest-path adapters/examples/fragment-app/Cargo.toml
```

Font caches default to `<system-cache>/html-engine/fonts/v1` on native hosts and
`html-engine-fonts-v1` in browser CacheStorage. Native applications can select a
private directory through `html_gpui::backend::configure_cache_directory` before
loading fonts. Private publication font families are implementation details;
font pickers can filter them using `is_private_font_family`.
