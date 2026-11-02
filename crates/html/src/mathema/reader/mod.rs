// Bundled from mathema, revision d9cce9ffeb7346181d4c3201dc0a13727269863b.
// SPDX-License-Identifier: Apache-2.0
// Modified for HtmlEngine: internal module paths and disabled upstream features.
// See UPSTREAM.txt and LICENSE-APACHE in the mathema module directory.

pub mod latex;
pub mod mathml;
pub mod mtef;
pub mod omml;
pub mod unicode_math;

use crate::mathema::error::{CompactParseResult, Config, ParseResult, Result};

/// Trait for parsing a math format into the shared AST.
pub trait Reader {
    fn parse(input: &str, config: &Config) -> Result<ParseResult>;

    /// Parse directly into a compact arena representation.
    ///
    /// The default implementation bridges via `parse()` + `from_math_expr`.
    /// Readers that build into an arena natively should override this.
    fn parse_compact(input: &str, config: &Config) -> Result<CompactParseResult> {
        let result = Self::parse(input, config)?;
        Ok(CompactParseResult::from_parse_result(&result))
    }
}
