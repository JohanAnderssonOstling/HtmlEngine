// Bundled from mathema, revision d9cce9ffeb7346181d4c3201dc0a13727269863b.
// SPDX-License-Identifier: Apache-2.0
// Modified for HtmlEngine: internal module paths and disabled upstream features.
// See UPSTREAM.txt and LICENSE-APACHE in the mathema module directory.

use crate::mathema::error::Span;

/// Structured error types for the LaTeX parser.
///
/// Each variant carries an optional [`Span`] identifying the byte range
/// in the original input where the error was detected.
#[derive(Debug, thiserror::Error)]
pub enum LatexParseError {
    #[error("unexpected token '{token}'")]
    UnexpectedToken { token: String, span: Option<Span> },

    #[error("unmatched delimiter '{delimiter}'")]
    UnmatchedDelimiter {
        delimiter: String,
        span: Option<Span>,
    },

    #[error("invalid command '\\{command}'")]
    InvalidCommand { command: String, span: Option<Span> },

    #[error("maximum nesting depth ({depth}) exceeded")]
    DepthExceeded { depth: u32, span: Option<Span> },

    #[error(
        "macro expansion exceeded size limit ({limit} bytes); possible runaway recursive macro"
    )]
    ExpansionLimit { limit: usize },
}

impl LatexParseError {
    /// Return the source span, if available.
    pub fn span(&self) -> Option<Span> {
        match self {
            Self::UnexpectedToken { span, .. }
            | Self::UnmatchedDelimiter { span, .. }
            | Self::InvalidCommand { span, .. }
            | Self::DepthExceeded { span, .. } => *span,
            Self::ExpansionLimit { .. } => None,
        }
    }
}

impl From<LatexParseError> for crate::mathema::error::Error {
    fn from(e: LatexParseError) -> Self {
        let span = e.span();
        crate::mathema::error::Error::LatexParse {
            message: e.to_string(),
            span,
        }
    }
}
