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
The packages currently use sibling checkout paths and are not published. The
GPUI shared backend also uses the COSMIC Text fork in the sibling
`Html-Prototype/vendor/cosmic-text` checkout.

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

Publication face selection uses `font-stretch`/`font-width` before style and
weight matching, and uses oblique angles when choosing between declared face
ranges. The shared COSMIC Text GPUI backend applies variable-font weight, width,
italic and slant coordinates consistently during shaping, measurement and
rasterization. Variable slant stays within the font’s supported range. For static
outline fonts, the backend synthesizes the remaining oblique angle;
synthesized angles are limited to ±89° to keep the shear finite. Width and angle
are part of the font and glyph cache identities, including fallback glyphs.
These settings are not yet applied by the separate CoreText and DirectWrite
backends. The shared backend also renders built-in `light`/`dark` font palettes
and named `@font-palette-values`, including base palette selection and color
overrides for COLR fonts supported by Swash. Palette definitions are scoped to
the active document and participate in font and glyph cache identities. Each
fallback glyph selects its palette using its resolved font family. Color layers
use unhinted outlines so Unicode subsetting keeps their authored geometry. Empty
quoted font family names are preserved when extracting and selecting publication
faces.

Explicit font-style descriptors control native slant axes and synthetic shear.
`font-synthesis-style: none` and `oblique-only` constrain synthesized styles.
When a width descriptor is automatic or omitted, publication font matching uses
the width range from the font itself.

Applications needing a complete reader can instead use HtmlViewCore's
`html-view-gpui`, which shares this backend.
