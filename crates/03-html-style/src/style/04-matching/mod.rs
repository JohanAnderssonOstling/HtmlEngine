//! Stage 4: selector indexing and DOM matching.

mod compiled;
pub(crate) mod filter;
mod names;
pub(crate) use compiled::BoundSelectors;
pub mod dom;
pub(crate) mod scope;
pub mod selectors;
mod traversal;

pub(crate) use traversal::StyleTraversal;
