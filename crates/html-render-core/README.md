# html-render-core

This crate turns HTML or XHTML fragments into laid-out documents and retained,
backend-neutral render scenes. It owns the shared painter contract and line
painting logic, but no reader navigation, EPUB loading, Floem, GPUI, or other
window-system state.

Callers provide a glyph shaper and optionally a resource provider. The default
renderer denies external resources, which makes untrusted catalog markup safe to
layout without network or filesystem access. Image handling is explicit: omit,
draw placeholders, or retain resource references for a target adapter.
