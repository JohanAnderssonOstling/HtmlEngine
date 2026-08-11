//! Internal CSS pipeline, ordered by the lifetime of style data.
//!
//! `source` normalizes stylesheet text, `syntax` validates declarations,
//! `rules` prepares effective rules, `matching` finds DOM targets, and
//! `cascade` produces renderer-owned computed styles.

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
