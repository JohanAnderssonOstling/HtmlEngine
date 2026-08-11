//! Stage 1: stylesheet sources and compatibility preprocessing.

pub mod declarations;
pub mod imports;

pub const DEFAULT_CSS: &str = include_str!("default.css");
