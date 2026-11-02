// Bundled from mathema, revision d9cce9ffeb7346181d4c3201dc0a13727269863b.
// SPDX-License-Identifier: Apache-2.0
// Modified for HtmlEngine: internal module paths and disabled upstream features.
// See UPSTREAM.txt and LICENSE-APACHE in the mathema module directory.

//! Bundled mathema conversion library; see UPSTREAM.txt for provenance.

#![allow(dead_code, unused_imports)]

#![deny(missing_docs)]

pub mod ast;
pub mod convert;
#[path = "render/css_colors.rs"]
pub(crate) mod css_colors;
pub mod error;
pub(crate) mod latex;
#[doc(hidden)]
pub mod reader;
#[cfg(any())]
pub mod render;
#[doc(hidden)]
pub mod unicode;
#[cfg(any())]
pub mod wasm;
#[doc(hidden)]
pub mod writer;

// Re-export the public API
pub use ast::arena::AstArena;
pub use ast::compact::{CompactExpr, CompactNode, NodeId, NodeSpan, StrId};
pub use ast::{MathExpr, MathNode};
pub use convert::{
    ConvertResult, convert, convert_with, from_latex, from_latex_with, from_mathml,
    from_mathml_with, from_mtef, from_mtef_with, from_omml, from_omml_with, from_unicode_math,
    from_unicode_math_with,
};
pub use error::{
    Config, Diagnostic, DiagnosticKind, Error, Format, ParseResult, Result, Strictness,
};
#[cfg(any())]
pub use render::raster::{ImageFormat, RasterOptions, render_to_image};
#[cfg(any())]
pub use render::{RenderOptions, render_to_svg};
#[cfg(any())]
pub use render::{
    render_to_svg_standalone, render_to_svg_standalone_compact,
    render_to_svg_standalone_compact_arena,
};
