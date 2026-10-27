# html-text

CSS logical-text preparation after styling and formatting-structure construction,
before font shaping and geometry layout.

`PreparedText::new` consumes an `InlineContent` buffer and borrows the DOM,
computed styles, and a `TextStructure` implementation. It collapses or preserves
whitespace, selects initial-letter style runs, applies text transforms, and
records Unicode/CSS break opportunities. UTF-16 source offsets follow characters
through whitespace removal and case expansions. Font shaping still checks
native cluster boundaries when consuming the recorded break opportunities.

`TextStructure` exposes only formatting ancestry, style handles, originating DOM
elements and inline/flex/grid context boundaries. The crate has no dependency on
layout geometry, a layout executor, renderer font resources, or pipeline state.
Pure pseudo-style selection helpers are reused by first-line composition; the
layout-dependent refinement loop is owned by html-pipeline.

`PreparedText` owns its characters, source offsets, inline items, wrap flags and
sparse initial-letter style runs. It exposes borrowed read-only views, has no
`Clone` implementation, and uses no reference counting, locks or interior
mutability. Shaping stores glyph IDs separately and borrows the canonical source.
The optional `testing` feature exposes legacy normalization fixtures and enables
raw-source cloning for downstream tests, without making prepared text cloneable.

`PreparedText::with_observer` lets the owner attach scoped diagnostics. The
observer returns a guard dropped at the end of each step, so profiling remains
with the calling pipeline without a global recorder in this crate.
