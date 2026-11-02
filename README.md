# Skrift

Skrift is a framework agnostic HTML and CSS rendering engine built with the purpose of high quality rendering of documents inside native applications.

## Features

- **Tables**
  - [x] Automatic and fixed table layouts
  - [x] Row and column spans
  - [x] Nested tables, row groups, and column groups
  - [x] Separate and collapsed borders
  - [x] Captions, cell alignment, and layered backgrounds
  - [ ] Cells spanning all remaining rows
  - [ ] Static row and column collapsing

- **Images and SVG**
  - [x] PNG, JPEG, and WebP decoding
  - [x] Optional GIF and AVIF support
  - [x] Inline and external SVG
  - [x] Intrinsic dimensions and CSS size constraints
  - [x] Aspect ratios, image fitting, and positioning
  - [x] Image placeholders and external image loading
  - [ ] Responsive image source selection

- **Mathematics**
  - [x] Inline and block presentation MathML
  - [x] Fractions, roots, scripts, accents, and matrices
  - [x] Stretchy operators
  - [x] Scalable mathematical notation
  - [x] Baseline alignment, scaling, and inherited colors
  - [ ] Arabic script math tokens

- **Bidi and RTL**
  - [x] Mixed LTR and RTL text
  - [x] Text direction and bidirectional controls
  - [x] Automatic direction, text isolation, and overrides
  - [x] Line reordering and mirrored brackets
  - [x] Direction aware tables, flexbox, grid, and lists
  - [x] Contextual text shaping
  - [ ] Vertical and sideways writing

- **Text and typography**
  - [x] Chinese, Japanese, and Korean line breaking
  - [x] Manual and automatic hyphenation for supported languages
  - [x] Justification and paragraph composition
  - [x] `::first-line` and `::first-letter` styling
  - [x] Underline, overline, and line-through decorations
  - [x] Reader font size and color overrides
  - [x] Font loading and fallback
  - [ ] Ruby annotations

- **Layout**
  - [x] Block flow, inline blocks, floats, and margin collapsing
  - [x] Flexbox wrapping, alignment, ordering, and gaps
  - [x] Grid named lines, areas, placement, and spanning
  - [x] Flexible and repeating grid tracks
  - [x] Relative and absolute positioning
  - [x] Overflow clipping and stacking order
  - [ ] Fixed and sticky positioning
  - [ ] Subgrid (shared tracks across nested grids)

- **Lists and generated content**
  - [x] Decimal, alphabetic, Roman numeral, and bullet markers
  - [x] Inside and outside markers
  - [x] Start values, reversed numbering, and item values
  - [x] Generated text before and after elements
  - [x] Quotes, attribute values, and nested counters
  - [ ] Custom counter styles and string markers
  - [ ] Additional standard list numbering styles

- **CSS and painting**
  - [x] Linked and embedded stylesheets and imports
  - [x] Cascade layers and custom properties
  - [x] Media queries and `@supports` for implemented features
  - [x] Solid backgrounds, borders, outlines, and text decorations
  - [x] Rounded corners and overflow clipping
  - [ ] Background images, gradients, and border images
  - [ ] Shadows, opacity, transforms, and filters

## Additional features

- Knuth Plass paragraph line breaking
- Configurable hyphenation quality safeguards
- Punctuation-aware justification
- Automatic microtracking for justified text
- Proportional reader font scaling, allowing scaling of absolute sized text
- Independent footnote and endnote layout
- Text-to-speech highlighting support
- Structured table copy and paste
- Copy selected content as Markdown

## Benchmark environment

| Component | Version / configuration |
| --- | --- |
| CPU | AMD Ryzen 7 6800HS Creator Edition |
| GPUI | 0.2.2 |
| Wry binding (`lb-wry`) | 0.53.3 |
| WebKitGTK runtime | 2.54.2 |
| GTK runtime | 3.24.52 |
| Host/helper CPU set | 8, 10, 12, 14: four physical cores, one SMT thread per core |
| CPU frequency limits | 3.0 GHz minimum and maximum, including SMT siblings |
| First viewport | 800 × 600 pixels, scale 1, X11 |
| Samples | 5 fresh processes per engine and case; 160 measurements |

## Warm engine RAM

Median after empty-page warmup for the Article case, before loading its content.

<table>
<thead>
<tr><th>GPUI + Skrift</th><th>GPUI + WebKitGTK</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><td>73.2 MiB</td><td>210.1 MiB</td><td>2.87×</td></tr>
</tbody>
</table>

## Cold first viewport

<table>
<thead>
<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th><th colspan="3">Total RAM (MiB)</th></tr>
<tr><th>Skrift</th><th>WebKitGTK</th><th>CPU ratio</th><th>Skrift</th><th>WebKitGTK</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/article.html">Article</a></th><td>108.1</td><td>465.6</td><td>4.31×</td><td>74.1</td><td>216.3</td><td>2.92×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/images.html">Images</a></th><td>92.3</td><td>432.7</td><td>4.69×</td><td>77.9</td><td>220.4</td><td>2.83×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/tables.html">Tables</a></th><td>94.2</td><td>508.4</td><td>5.39×</td><td>78.0</td><td>219.3</td><td>2.81×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/languages/rtl-arabic.html">Arabic (RTL)</a></th><td>93.5</td><td>407.7</td><td>4.36×</td><td>75.6</td><td>218.6</td><td>2.89×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/languages/cjk-japanese.html">Japanese (CJK)</a></th><td>129.0</td><td>496.8</td><td>3.85×</td><td>78.0</td><td>220.7</td><td>2.83×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/large-document.html">Very large document</a></th><td>2353.4</td><td>1030.8</td><td>0.44×</td><td>235.5</td><td>295.2</td><td>1.25×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/results/middlemarch-hyphenation-performance-20261101-180925/inputs/hyphens-auto.xhtml">Middlemarch chapter</a></th><td>108.2</td><td>527.7</td><td>4.88×</td><td>80.0</td><td>234.5</td><td>2.93×</td></tr>
</tbody>
</table>

## Warm first viewport

<table>
<thead>
<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th><th colspan="3">RAM before (MiB)</th><th colspan="3">RAM after (MiB)</th></tr>
<tr><th>Skrift</th><th>WebKitGTK</th><th>CPU ratio</th><th>Skrift</th><th>WebKitGTK</th><th>Size ratio</th><th>Skrift</th><th>WebKitGTK</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/article.html">Article</a></th><td>56.2</td><td>54.8</td><td>0.98×</td><td>73.2</td><td>210.1</td><td>2.87×</td><td>75.6</td><td>216.6</td><td>2.87×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/images.html">Images</a></th><td>57.4</td><td>51.5</td><td>0.90×</td><td>73.7</td><td>211.5</td><td>2.87×</td><td>78.2</td><td>219.9</td><td>2.81×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/tables.html">Tables</a></th><td>55.6</td><td>63.2</td><td>1.14×</td><td>75.0</td><td>214.7</td><td>2.86×</td><td>78.3</td><td>223.0</td><td>2.85×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/languages/rtl-arabic.html">Arabic (RTL)</a></th><td>53.5</td><td>37.0</td><td>0.69×</td><td>73.8</td><td>210.3</td><td>2.85×</td><td>76.0</td><td>216.0</td><td>2.84×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/languages/cjk-japanese.html">Japanese (CJK)</a></th><td>75.8</td><td>42.2</td><td>0.56×</td><td>73.3</td><td>213.7</td><td>2.92×</td><td>78.4</td><td>221.9</td><td>2.83×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/large-document.html">Very large document</a></th><td>2267.9</td><td>587.1</td><td>0.26×</td><td>73.4</td><td>217.7</td><td>2.96×</td><td>234.3</td><td>303.4</td><td>1.29×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/results/middlemarch-hyphenation-performance-20261101-180925/inputs/hyphens-auto.xhtml">Middlemarch chapter</a></th><td>71.2</td><td>124.4</td><td>1.75×</td><td>73.5</td><td>212.2</td><td>2.89×</td><td>80.5</td><td>229.0</td><td>2.84×</td></tr>
</tbody>
</table>

## Warm whole document render

<table>
<thead>
<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th><th colspan="3">RAM before (MiB)</th><th colspan="3">RAM after (MiB)</th></tr>
<tr><th>Skrift</th><th>WebKitGTK</th><th>CPU ratio</th><th>Skrift</th><th>WebKitGTK</th><th>Size ratio</th><th>Skrift</th><th>WebKitGTK</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><th scope="row"><a href="tools/embed-bench/fixtures/tables.html">Tables</a></th><td>59.6</td><td>65.4</td><td>1.10×</td><td>73.9</td><td>219.0</td><td>2.96×</td><td>79.0</td><td>228.4</td><td>2.89×</td></tr>
<tr><th scope="row"><a href="tools/embed-bench/results/middlemarch-hyphenation-performance-20261101-180925/inputs/hyphens-auto.xhtml">Middlemarch chapter</a></th><td>105.6</td><td>147.4</td><td>1.40×</td><td>73.4</td><td>226.2</td><td>3.08×</td><td>91.5</td><td>243.9</td><td>2.66×</td></tr>
</tbody>
</table>

Raw results: [summary](tools/embed-bench/results/fixed-frequency-suite-20261101-204746/summary.json), [measurement plan](tools/embed-bench/results/fixed-frequency-suite-20261101-204746/plan.json) and [validation](tools/embed-bench/results/fixed-frequency-suite-20261101-204746/validation.json). Individual samples, CPU checkpoints and ranges are retained in the results directory.

## Executable size

| Application | Stripped executable |
| --- | ---: |
| GPUI + Skrift | 42 MiB |
| GPUI + Wry | 27 MiB |

## Platforms

- Linux
- Windows
- macOS
- Android
- Browser WebAssembly

## Non goals

- JavaScript execution and scripted DOM updates.
- Interactive elements such as editable text fields, buttons, menus, and form
  controls, including validation and submission.
- Browser APIs such as web storage, service workers, and geolocation.
- Audio and video playback.
- Canvas and WebGL applications.
- A complete web browser with tabs, navigation history, and a user interface.

Host applications can provide navigation and input handling around the rendered
document.

Optional GPUI rendering and font/image support live in the separate
[`adapters` workspace](adapters/README.md). Core engine crates remain independent
of GPUI and application repositories.

Saved WPT dependency limitations are tracked in the
[dependency registry](testdata/wpt/dependency-limitations/README.txt), with exact
assertion keys, parser evidence, and separate classifications for confirmed
limitations, candidates requiring investigation, and engine implementation gaps.
