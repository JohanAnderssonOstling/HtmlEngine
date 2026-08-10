pub mod box_syntax;
pub mod capabilities;
pub(crate) mod contain;
pub mod declarations;
pub mod imports;
pub mod media;
pub mod prepared;
pub mod resolver;
pub mod selectors;
pub mod selectors_dom;
pub mod tab_size;
pub mod text_spacing;
pub mod white_space;

pub const DEFAULT_CSS: &str = include_str!("default.css");
