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

- [x] Knuth Plass paragraph line breaking
- [x] Configurable hyphenation quality safeguards
- [x] Punctuation-aware justification
- [x] Automatic microtracking for justified text
- [x] Proportional reader font scaling, allowing scaling of absolute sized text
- [x] Independent footnote and endnote layout
- [x] Text-to-speech highlighting support
- [x] Structured table copy and paste
- [x] Copy selected content as Markdown

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
