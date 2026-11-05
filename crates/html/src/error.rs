//! Stable document error categories with the original diagnostic source chain.

use html_dom::InvalidRootFontSize;
use html_parse::XmlParseError;
use html_dom::DocumentBuildError;
use html_pipeline::engine::LayoutConstraintError;
use crate::pipeline::{PipelineError, PipelineErrorKind};
use crate::text::ShapeError;
use std::{error, fmt, io};

#[derive(Clone, Debug)]
pub struct Error {
    cause: PipelineError,
}

/// Public document failures, independent of pipeline stages and session states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Parse,
    Style,
    Shape,
    InvalidInput,
    Resource,
    Internal,
}

impl Error {
    pub(crate) fn kind(&self) -> ErrorKind {
        let mut cause = &self.cause;
        loop {
            return match cause.kind() {
                PipelineErrorKind::Xml(_) | PipelineErrorKind::Dom(_) => ErrorKind::Parse,
                PipelineErrorKind::Prepare(_) => ErrorKind::Style,
                PipelineErrorKind::Shape(_) => ErrorKind::Shape,
                PipelineErrorKind::Constraints(_) | PipelineErrorKind::RootFontSize(_) => {
                    ErrorKind::InvalidInput
                }
                PipelineErrorKind::Resource(_) => ErrorKind::Resource,
                PipelineErrorKind::EmptySession | PipelineErrorKind::Message(_) => {
                    ErrorKind::Internal
                }
                PipelineErrorKind::Context { source, .. } => {
                    cause = source;
                    continue;
                }
            };
        }
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self {
            cause: PipelineError::message(message),
        }
    }

    pub(crate) fn with_context(self, message: impl Into<String>) -> Self {
        Self {
            cause: self.cause.with_context(message),
        }
    }
}

impl From<PipelineError> for Error {
    fn from(cause: PipelineError) -> Self {
        Self { cause }
    }
}

macro_rules! document_error_from {
    ($($cause:ty),* $(,)?) => {
        $(impl From<$cause> for Error {
            fn from(cause: $cause) -> Self {
                Self::from(PipelineError::from(cause))
            }
        })*
    };
}

document_error_from! {
    XmlParseError, DocumentBuildError, ShapeError, LayoutConstraintError,
    InvalidRootFontSize, io::Error,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.cause, formatter)
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        error::Error::source(&self.cause)
    }
}
