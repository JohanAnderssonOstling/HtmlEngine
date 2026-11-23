//! Write layout fragments and retain placement transforms until publication.
//! Layout and paint share this storage interface; formatting algorithms and
//! decoration generation remain in their owning modules.

mod fragment_writer;
mod placement;

pub(crate) use fragment_writer::{DecorationMetadata, FragmentWriter, OutputCursor, OutputRanges};
pub use placement::PlacementState;
pub(crate) use placement::{BoxPlacement, PlacementId};
