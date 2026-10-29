//! CSS compilation and document resolution with shared data contracts.
//!
//! `program` coordinates the stages. Rules compile native CSS without DOM
//! access; matching and cascade resolve one document from compiled data and
//! already-parsed inline declarations. Lower layers never import `program`.

#[path = "05-cascade/mod.rs"]
pub mod cascade;
#[path = "04-matching/mod.rs"]
pub mod matching;
#[path = "03-rules/mod.rs"]
pub mod rules;
#[path = "01-source/mod.rs"]
pub mod source;
#[path = "02-syntax/mod.rs"]
pub mod syntax;

pub use source::DEFAULT_CSS;
pub(crate) mod compiled;
pub(crate) mod document;
pub(crate) mod environment;
pub(crate) mod program;
pub(crate) mod support;
pub(crate) mod timings;

pub(crate) mod property_catalogue;
