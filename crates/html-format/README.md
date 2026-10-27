# html-format

Constructs formatting topology, generated inline content and MathML resources
from the styled DOM. It runs before logical-text preprocessing and geometry.
Topology and raw text buffers are owned by the caller; builders borrow them
mutably for one operation. Shared types live in html-engine-model. This crate
has no normal dependency on shaping, layout execution, or pipeline ownership.
