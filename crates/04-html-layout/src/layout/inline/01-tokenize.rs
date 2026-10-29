//! Build canonical tokens. Composition owns source-order float placement.
use super::InlineFormattingInput;
use super::tokens::{InlineTokenKind, InlineTokens, build_inline_tokens};
use crate::layout::LayoutEngine;

pub(super) fn prepare_inline_tokens(
    engine: &mut LayoutEngine<'_, '_>,
    input: &InlineFormattingInput,
) -> (InlineTokens<'static>, bool) {
    profile_scope!("layout.prepare_tokens");
    let max_width = (input.area.width - input.paragraph.indent.amount.max(0.0)).max(0.0);
    let tokens = build_inline_tokens(
        engine,
        input.run_range.clone(),
        input.container_box_idx,
        max_width,
        input.area.percentage_height_basis,
    );
    let has_float_exclusions = tokens
        .iter()
        .any(|token| matches!(token.kind(), InlineTokenKind::FloatAnchor { .. }))
        || engine
            .floats
            .current_float_context()
            .is_some_and(|context| !context.is_empty());
    (tokens, has_float_exclusions)
}
