pub mod computed_style;

pub use computed_style::*;
pub mod language;
pub use language::{ContentLanguage, LanguageId, LanguageTag};

mod font_face;
pub use font_face::{FontFace, FontFaceDescriptor, FontFaceSource, FontFaceStyle};

pub mod open_type;
pub use open_type::{OpenTypeFeature, OpenTypeFeatureKind};

pub mod reader_style;
