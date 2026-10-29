//! Framework-neutral clipboard/file payloads. Applications own clipboard access.

use crate::layout::RenderView;
use crate::resources::{DecodedImage, ImagePipeline};
use crate::selection::{Selection, TableCellRange, table_selection_markdown};
pub use html_pipeline::engine::{RenderExportView, RenderTable};
use std::ops::Range;

/// Content identities use document-local logical ranges and resource/box IDs.
pub enum ExportTarget {
    Text(Range<u32>),
    SemanticText(Range<u32>),
    Table(usize),
    TableCells(TableCellRange),
    Image(u32),
    Equation(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    PlainText,
    Markdown,
    Html,
    AuthoredHtml,
    Image,
    Latex,
}

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

#[derive(Clone, Copy)]
pub struct DocumentExport<'a> {
    view: RenderView<'a>,
    images: Option<&'a ImagePipeline>,
}

impl<'a> DocumentExport<'a> {
    pub fn new(view: RenderView<'a>) -> Self {
        Self { view, images: None }
    }

    pub fn with_images(mut self, images: &'a ImagePipeline) -> Self {
        self.images = Some(images);
        self
    }

    /// Missing content or unloaded images return `Ok(None)`. Unsupported
    /// target/format combinations and conversion failures return an error.
    pub fn request(
        self,
        target: ExportTarget,
        format: ExportFormat,
    ) -> Result<Option<ExportPayload<'a>>, String> {
        let text = match (target, format) {
            (
                target @ (ExportTarget::Text(_) | ExportTarget::SemanticText(_)),
                ExportFormat::PlainText | ExportFormat::Markdown,
            ) => {
                let semantic = matches!(target, ExportTarget::SemanticText(_));
                let range = match target {
                    ExportTarget::Text(range) | ExportTarget::SemanticText(range) => range,
                    _ => unreachable!(),
                };
                let selection = Selection::new(self.view, range);
                let selection = if semantic {
                    selection.with_semantics()
                } else {
                    selection
                };
                let payload = selection.text();
                if format == ExportFormat::PlainText {
                    payload.plain
                } else {
                    payload.markdown
                }
            }
            (ExportTarget::Table(index), ExportFormat::Markdown) => self
                .view
                .boxes()
                .table(index)
                .and_then(|table| table.markdown()),
            (ExportTarget::Table(index), ExportFormat::Html) => self
                .view
                .boxes()
                .table(index)
                .and_then(|table| table.unstyled_html()),
            (ExportTarget::Table(index), ExportFormat::AuthoredHtml) => {
                self.view.boxes().table(index).and_then(|table| {
                    (!table.authored_html().is_empty()).then(|| table.authored_html().to_owned())
                })
            }
            (ExportTarget::TableCells(range), ExportFormat::Markdown) => self
                .view
                .boxes()
                .table(range.table_box)
                .and_then(|table| table_selection_markdown(&table, range)),
            (ExportTarget::Image(index), ExportFormat::Image) => {
                return Ok(self
                    .images
                    .and_then(|images| image_payload(images, index))
                    .map(ExportPayload::Image));
            }
            (ExportTarget::Equation(index), ExportFormat::Latex) => {
                #[cfg(feature = "math-export")]
                {
                    equation_latex(self.view, index).transpose()?
                }
                #[cfg(not(feature = "math-export"))]
                {
                    let _ = index;
                    return Err("LaTeX export requires the math-export feature".into());
                }
            }
            _ => return Err("unsupported export target and format".into()),
        };
        Ok(text.map(ExportPayload::Text))
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

impl crate::engine::Engine {
    pub fn export(&self) -> Option<DocumentExport<'_>> {
        Some(DocumentExport::new(self.render_view()?).with_images(self.images()))
    }

    pub fn image_payload(&self, image: u32) -> Option<ImagePayload<'_>> {
        image_payload(self.images(), image)
    }
}

fn image_payload(images: &ImagePipeline, image: u32) -> Option<ImagePayload<'_>> {
    Some(match images.get_decoded(image)? {
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

#[cfg(feature = "math-export")]
pub use mathml::mathml_to_latex;

/// Export a selected formula resource. Ordinary images return `None`.
#[cfg(feature = "math-export")]
pub fn equation_latex(
    view: html_pipeline::engine::RenderView<'_>,
    image: u32,
) -> Option<Result<String, String>> {
    view.image_mathml(image).map(mathml_to_latex)
}
