//! Document-local reading units. [`crate::pagination::Paginator::append_reading_fragments`]
//! traverses pages and preserves whole words within the requested source range.
//! Synthesis chunking and target queries use [`crate::layout::RenderAddressingView`].
//! Playback and following the active passage belong to the host.
pub use html_pipeline::engine::{ReadingFragment, ReadingTarget};

/// Where forward page traversal begins for a reading request.
#[derive(Clone, Copy, Debug)]
pub enum ReadingOrigin {
    /// The first laid-out line in the document.
    Document,
    /// A pagination cursor, including an installed page's start cursor.
    Cursor(usize),
    /// A document-space offset before a page cursor is available.
    Offset(f64),
}
