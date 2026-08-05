pub(crate) mod document {
    pub use html_dom::*;
}

#[path = "resources/mod.rs"]
pub mod resources;

pub use resources::*;
