pub(crate) mod document {
    pub use html_dom::*;
}

#[path = "resources/mod.rs"]
pub mod resources;

#[cfg(feature = "font-data")]
pub mod fonts;

pub use resources::*;
