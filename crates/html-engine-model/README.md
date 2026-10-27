# html-engine-model

Shared stage contracts: formatting topology, glyph/font measurements, borrowed
text geometry, sizing policies, paint inputs and published geometry buffers.
The model has no normal dependency on formatting, shaping, layout executors, or
the pipeline. Renderer backend contracts are shared here so independent stages
and their tests use the same glyph/resource identities. Palette views borrow
pipeline-owned values. The optional profiling implementation is the existing
diagnostic recorder, shared by all stages rather than duplicated per crate.
