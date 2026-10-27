//! Grammar and numeric lowering for DOM sibling-counting functions.

use super::numeric_functions;
use cssparser::{Parser, ParserInput};
use std::borrow::Cow;

#[derive(Clone, Copy)]
pub(crate) enum TreeFunction {
    Index,
    Count,
}

pub(crate) fn lower<'a>(
    source: &'a str,
    mut value: impl FnMut(TreeFunction) -> u32,
) -> Result<Cow<'a, str>, ()> {
    if !source.contains('\\')
        && !source
            .as_bytes()
            .windows(8)
            .any(|part| part.eq_ignore_ascii_case(b"sibling-"))
    {
        return Ok(Cow::Borrowed(source));
    }
    numeric_functions::replace(source, |name, arguments| {
        let function = if name.eq_ignore_ascii_case("sibling-index") {
            TreeFunction::Index
        } else if name.eq_ignore_ascii_case("sibling-count") {
            TreeFunction::Count
        } else {
            return Ok(None);
        };
        let mut input = ParserInput::new(arguments);
        if !Parser::new(&mut input).is_exhausted() {
            return Err(());
        }
        // Preserve the calculation's numeric type, including unitless zero
        // rules and computed range clamping in the containing property.
        Ok(Some(format!("calc({})", value(function))))
    })
}
