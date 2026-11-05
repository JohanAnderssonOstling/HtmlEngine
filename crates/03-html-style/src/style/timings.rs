//! Timings shared by CSS compilation and document resolution.
use std::time::Duration;

#[derive(Clone, Debug, Default)]
pub struct StyleTimings {
    /// Parsing and fragment preparation on first use in this process; zero
    /// once the shared immutable default fragment has been initialized.
    pub parse_default_css: Duration,
    /// Acquiring author fragments, including parsing and preparation on cache misses.
    pub parse_author_css: Duration,
    /// Linking prepared fragments for the current environment and cascade order.
    pub prepare_rules: Duration,
    pub resolve_styles: Duration,
    pub selector_index: Duration,
    pub resolver_setup: Duration,
    pub selector_matching: Duration,
    pub cascade: Duration,
    pub style_store: Duration,
}
