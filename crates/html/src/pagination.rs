//! Compose pages from published content, or query pagination order for navigation.

mod composition;
pub use composition::{PaginationMode, PaginationScratch, Paginator};
pub use html_engine_model::{
    LineScreen, PageGeometry, PagePositions, PageRequest, VisibleLinePositions,
};
mod projection;
mod table_fragments;
mod position;
pub use position::{DocumentPosition, ReflowAnchor, document_position};
mod view;
pub(crate) use html_pipeline::engine::PaginationTarget;
pub use view::{PageReadingLine, PageView};

pub use html_engine_model::PageAnchor;
