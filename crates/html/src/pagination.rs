//! Compose pages from published content, or query pagination order for navigation.

mod composition;
pub use composition::{PaginationMode, PaginationScratch, Paginator};
pub use html_engine_model::{
    LineScreen, PageGeometry, PagePositions, PageRequest, RepeatedTableHeader, VisibleLinePositions,
};
mod position;
pub use position::{DocumentPosition, ReflowAnchor, document_position};
mod view;
pub use view::{PageReadingLine, PageView};
pub(crate) use html_pipeline::engine::PaginationTarget;

/// A resolved page boundary in document coordinates. A cursor preserves group
/// identity when multiple groups share an offset; an offset alone is available
/// before a page is installed.
#[derive(Clone, Copy, Debug)]
pub struct PageAnchor {
    pub cursor: Option<usize>,
    pub offset_y: f64,
}
