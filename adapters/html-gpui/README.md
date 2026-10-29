# html-gpui

Reusable GPUI integration for `skrift`. This crate owns native shaping,
GPUI painting, image conversion and the `HtmlFragmentElement` widget. It has no
application, pagination or reader dependency.

For a local checkout alongside `GPUI-Fork`, an application's dependencies can be:

```toml
[dependencies]
html-gpui = { path = "../HtmlEngine/adapters/html-gpui" }
gpui = { path = "../GPUI-Fork/crates/gpui", default-features = false }
```

Select your platform implementation and GPUI features in the application, using
the same GPUI checkout as the adapter. See the complete
[standalone application](../examples/fragment-app) for desktop window setup.
The packages currently use sibling checkout paths and are not published.

Embed HTML in a GPUI parent:

```rust,ignore
use gpui::{ParentElement, div, rgb};
use html_gpui::HtmlFragmentElement;

div().child(
    HtmlFragmentElement::new("description", "<p>Some <strong>HTML</strong>.</p>")
        .stylesheet("body { font-family: sans-serif; font-size: 16px; }")
        .selectable(rgb(0xc8dcff).into()),
)
```

The element fills its available width and reports its natural height. Its parent
owns scrolling. HTML and supplied CSS control typography and colors; surrounding
GPUI text styles are not inherited. Selection is optional. Fragment images are
currently omitted.

For a custom document surface, `backend::GpuiTextCache` implements the engine's
text shaping contract. `backend::GpuiCommandPainter` records engine paint output;
`backend::paint_commands` replays it into a GPUI window. The backend also exposes
publication font registration and image caching. The host chooses document
placement, scrolling or pagination.

Applications needing a complete reader can instead use HtmlViewCore's
`html-view-gpui`, which shares this backend.
