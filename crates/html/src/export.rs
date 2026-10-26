//! Framework-neutral clipboard/file payloads. Applications own clipboard access.

pub use html_layout::{RenderExportView, RenderTable};

#[cfg(feature = "math-export")]
#[path = "export_mathml.rs"]
mod mathml;

#[cfg(feature = "math-export")]
pub use mathml::mathml_to_latex;

/// Export a selected formula resource. Ordinary images return `None`.
#[cfg(feature = "math-export")]
pub fn equation_latex(
    view: html_layout::RenderView<'_>,
    image: u32,
) -> Option<Result<String, String>> {
    view.image_mathml(image).map(mathml_to_latex)
}
