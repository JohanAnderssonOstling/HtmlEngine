//! Framework-neutral clipboard/file payloads. Applications own clipboard access.

#[cfg(feature = "math-export")]
use html_pipeline::engine::text_paint;
use html_resources::resources::images as image_resources;
use crate::document::ImageId;
use crate::layout::RenderView;
use html_resources::DecodedImage;
use crate::resources::DecodedImages;
use std::error::Error;
use std::fmt;

/// Each request carries only formats supported by that content type.
/// Resource IDs are local to the current document. Export text and tables
/// through [`crate::selection::SelectionController`].
pub enum ExportRequest {
    Image(ImageId),
    EquationLatex(ImageId),
}

pub use html_pipeline::engine::TableExportFormat;
pub(crate) use html_pipeline::engine::{TableExportRequest, TextExport, TextExportFormat};

/// Portable output. This API never accesses a clipboard or chooses a file path.
pub enum ExportPayload<'a> {
    Text(String),
    Image(ImagePayload<'a>),
}

impl ExportPayload<'_> {
    pub fn into_text(self) -> Option<String> {
        match self {
            Self::Text(text) => Some(text),
            Self::Image(_) => None,
        }
    }
}

/// A requested conversion which could not produce its format.
#[derive(Debug)]
pub enum ExportError {
    /// Equation conversion requires the `math-export` feature.
    MathExportUnavailable,
    /// The retained source could not be converted to the requested format.
    Conversion(Box<dyn Error + Send + Sync>),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MathExportUnavailable => {
                f.write_str("LaTeX export requires the math-export feature")
            }
            Self::Conversion(error) => write!(f, "export conversion failed: {error}"),
        }
    }
}

impl Error for ExportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::MathExportUnavailable => None,
            Self::Conversion(error) => Some(error.as_ref()),
        }
    }
}

#[derive(Clone, Copy)]
pub struct DocumentExport<'a> {
    #[cfg(feature = "math-export")]
    view: RenderView<'a>,
    images: Option<DecodedImages<'a>>,
}

impl<'a> DocumentExport<'a> {
    pub(crate) fn new(_view: RenderView<'a>) -> Self {
        Self {
            #[cfg(feature = "math-export")]
            view: _view,
            images: None,
        }
    }

    pub(crate) fn with_images(mut self, images: DecodedImages<'a>) -> Self {
        self.images = Some(images);
        self
    }

    /// Missing content or unloaded images return `Ok(None)`. Formula conversion
    /// failures and unavailable math-export support return an error.
    pub fn request(self, request: ExportRequest) -> Result<Option<ExportPayload<'a>>, ExportError> {
        match request {
            ExportRequest::Image(index) => Ok(self
                .images
                .and_then(|images| image_payload(images, index))
                .map(ExportPayload::Image)),
            ExportRequest::EquationLatex(index) => {
                #[cfg(feature = "math-export")]
                {
                    let text = text_paint::render_view_image_mathml(&self.view, index)
                        .map(mathml::mathml_to_latex)
                        .transpose()?;
                    Ok(text.map(ExportPayload::Text))
                }
                #[cfg(not(feature = "math-export"))]
                {
                    let _ = index;
                    Err(ExportError::MathExportUnavailable)
                }
            }
        }
    }
}

/// Borrowed portable media data. Clipboard and file destinations are host policy.
pub enum ImagePayload<'a> {
    Raster {
        width: usize,
        height: usize,
        rgba: &'a [u8],
    },
    Svg {
        bytes: &'a [u8],
    },
}

pub struct SourceImage {
    pub uri: String,
    pub bytes: Vec<u8>,
}

impl crate::Document {
    pub fn export(&self) -> Option<DocumentExport<'_>> {
        Some(DocumentExport::new(self.render_view()?).with_images(self.images()))
    }
}

fn image_payload<'a>(images: DecodedImages<'a>, image: ImageId) -> Option<ImagePayload<'a>> {
    Some(match image_resources::decoded_images_get(images, image)? {
        DecodedImage::Raster { image, .. } => ImagePayload::Raster {
            width: image.width as usize,
            height: image.height as usize,
            rgba: image.data.data(),
        },
        DecodedImage::Svg { bytes, .. } => ImagePayload::Svg { bytes },
    })
}

#[cfg(feature = "math-export")]
#[path = "export_mathml.rs"]
mod mathml;
