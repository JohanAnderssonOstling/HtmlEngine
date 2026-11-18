//! Resolve box constraints, dispatch contents, and publish final geometry.
//! Shared box models and positioning rules also serve formatting contexts.

#[path = "00-layout.rs"]
mod s00_layout;
#[path = "01-sizing.rs"]
mod s01_sizing;
#[path = "02-contents.rs"]
mod s02_contents;
#[path = "03-finish.rs"]
mod s03_finish;

pub(crate) mod absolute_positioning;
pub(crate) mod constraints;
pub(crate) mod model;
pub(crate) mod replaced;
pub(crate) mod request;

pub(crate) use s01_sizing::{ResolvedBoxSizing, block_inline_alignment_offset};
pub(crate) use s03_finish::{
    relative_position_offset, translate_laid_out_content, translate_laid_out_output,
};
