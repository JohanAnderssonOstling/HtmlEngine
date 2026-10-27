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
  - [x] First line and initial letter styling
  - [x] Text decorations
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
  - [ ] Shared tracks across nested grids

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
  - [x] Media and feature queries
  - [x] Solid backgrounds, borders, outlines, and text decorations
  - [x] Rounded corners and clipping
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

| Component | Version |
| --- | --- |
| GPUI fork | 0.2.2 |
| Wry binding (`lb-wry`) | 0.53.3 |
| WebKitGTK runtime | 2.54.2 |
| GTK runtime | 3.24.52 |

## Warm engine RAM

<table>
<thead>
<tr><th>GPUI + Skrift</th><th>GPUI + Wry</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><td>88 MiB</td><td>215 MiB</td><td>2.43×</td></tr>
</tbody>
</table>

## Cold first rendered frame

<table>
<thead>
<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th><th colspan="3">Total RAM (MiB)</th></tr>
<tr><th>Skrift</th><th>Wry</th><th>Speedup</th><th>Skrift</th><th>Wry</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/article.html">Article</a></th><td>53</td><td>280</td><td>5.32×</td><td>94</td><td>213</td><td>2.26×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/images.html">Images</a></th><td>44</td><td>308</td><td>7.04×</td><td>83</td><td>208</td><td>2.50×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/tables.html">Tables</a></th><td>45</td><td>284</td><td>6.25×</td><td>98</td><td>219</td><td>2.23×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/languages/rtl-arabic.html">Arabic (RTL)</a></th><td>26</td><td>274</td><td>10.61×</td><td>96</td><td>221</td><td>2.30×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/languages/cjk-japanese.html">Japanese (CJK)</a></th><td>31</td><td>294</td><td>9.58×</td><td>98</td><td>219</td><td>2.23×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/large-document.html">Very large document</a></th><td>682</td><td>593</td><td>0.87×</td><td>285</td><td>299</td><td>1.05×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/books/middlemarch/chapter-37.xhtml">Middlemarch chapter</a></th><td>95</td><td>448</td><td>4.70×</td><td>103</td><td>222</td><td>2.16×</td></tr>
</tbody>
</table>

## Warm first rendered frame

<table>
<thead>
<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th><th colspan="3">RAM before (MiB)</th><th colspan="3">RAM after (MiB)</th></tr>
<tr><th>Skrift</th><th>Wry</th><th>Speedup</th><th>Skrift</th><th>Wry</th><th>Size ratio</th><th>Skrift</th><th>Wry</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/article.html">Article</a></th><td>44</td><td>28</td><td>0.64×</td><td>88</td><td>215</td><td>2.43×</td><td>98</td><td>221</td><td>2.26×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/images.html">Images</a></th><td>38</td><td>21</td><td>0.56×</td><td>89</td><td>215</td><td>2.43×</td><td>102</td><td>223</td><td>2.19×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/tables.html">Tables</a></th><td>40</td><td>29</td><td>0.71×</td><td>88</td><td>214</td><td>2.42×</td><td>99</td><td>221</td><td>2.23×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/languages/rtl-arabic.html">Arabic (RTL)</a></th><td>24</td><td>18</td><td>0.74×</td><td>89</td><td>212</td><td>2.39×</td><td>99</td><td>217</td><td>2.20×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/languages/cjk-japanese.html">Japanese (CJK)</a></th><td>28</td><td>39</td><td>1.42×</td><td>88</td><td>214</td><td>2.42×</td><td>101</td><td>222</td><td>2.20×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/large-document.html">Very large document</a></th><td>680</td><td>382</td><td>0.56×</td><td>88</td><td>213</td><td>2.42×</td><td>286</td><td>299</td><td>1.05×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/books/middlemarch/chapter-37.xhtml">Middlemarch chapter</a></th><td>98</td><td>95</td><td>0.98×</td><td>89</td><td>211</td><td>2.38×</td><td>105</td><td>226</td><td>2.15×</td></tr>
</tbody>
</table>

## Warm whole document render

<table>
<thead>
<tr><th rowspan="2">Document</th><th colspan="3">CPU time (ms)</th><th colspan="3">RAM before (MiB)</th><th colspan="3">RAM after (MiB)</th></tr>
<tr><th>Skrift</th><th>Wry</th><th>Speedup</th><th>Skrift</th><th>Wry</th><th>Size ratio</th><th>Skrift</th><th>Wry</th><th>Size ratio</th></tr>
</thead>
<tbody>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/tables.html">Tables</a></th><td>46</td><td>84</td><td>1.83×</td><td>88</td><td>211</td><td>2.39×</td><td>100</td><td>220</td><td>2.20×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/large-document.html">Very large document</a></th><td>5401</td><td>18275</td><td>3.38×</td><td>88</td><td>208</td><td>2.37×</td><td>288</td><td>349</td><td>1.21×</td></tr>
<tr><th scope="row"><a href="https://github.com/JohanAnderssonOstling/HtmlEngine/blob/master/tools/embed-bench/fixtures/books/middlemarch/chapter-37.xhtml">Middlemarch chapter</a></th><td>125</td><td>335</td><td>2.69×</td><td>89</td><td>211</td><td>2.38×</td><td>108</td><td>231</td><td>2.14×</td></tr>
</tbody>
</table>

One sample per engine and document. CPU includes viewport snapshot readback. RAM is total process session PSS including helpers and excluding GPU VRAM. Cold shows total RAM after rendering; warm tables show RAM before and after loading. Cold starts from an empty GPUI window; warm starts after the engine renders an empty document. Layout differences remain between engines.

Raw results: [cold](tools/embed-bench/results/gpui-culled-suite-cold-first-20261027.json), [warm](tools/embed-bench/results/gpui-culled-suite-warm-first-20261027.json), [whole document](tools/embed-bench/results/gpui-culled-suite-warm-whole-20261027.json).

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
