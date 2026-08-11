use super::*;

pub(super) fn parse_break_between(tokens: &TokenList<'_>) -> Option<BreakBetween> {
    match single_ident_keyword(tokens)?.to_ascii_lowercase().as_str() {
        "auto" => Some(BreakBetween::Auto),
        "avoid" | "avoid-page" | "avoid-column" => Some(BreakBetween::Avoid),
        "column" => Some(BreakBetween::Column),
        // This reader does not model facing-page parity. Preserve the forced
        // page break for side-specific values as the standards-compatible
        // fallback without claiming left/right placement.
        "always" | "page" | "left" | "right" | "recto" | "verso" => Some(BreakBetween::Page),
        _ => None,
    }
}

pub(super) fn parse_break_inside(tokens: &TokenList<'_>) -> Option<BreakInside> {
    match single_ident_keyword(tokens)?.to_ascii_lowercase().as_str() {
        "auto" => Some(BreakInside::Auto),
        "avoid" | "avoid-page" | "avoid-column" => Some(BreakInside::Avoid),
        _ => None,
    }
}

pub(super) fn single_positive_integer(tokens: &TokenList<'_>) -> Option<u8> {
    let mut values = tokens.0.iter().filter(|token| !is_ignorable_token(token));
    let value = match values.next()? {
        TokenOrValue::Token(Token::Number {
            int_value: Some(value),
            ..
        }) => *value,
        _ => return None,
    };
    (values.next().is_none() && value > 0).then(|| value.min(u8::MAX as i32) as u8)
}

pub(super) fn apply_custom_box_keyword(
    style: &mut WorkingStyle,
    name: &str,
    tokens: &TokenList<'_>,
) -> bool {
    match name.to_ascii_lowercase().as_str() {
        "float" => {
            let Some(value) = single_ident_keyword(tokens).map(str::to_ascii_lowercase) else {
                return false;
            };
            style.box_model.float =
                logical_float(&value, style.text.direction, style.box_model.float)
        }
        "clear" => {
            let Some(value) = single_ident_keyword(tokens).map(str::to_ascii_lowercase) else {
                return false;
            };
            style.box_model.clear =
                logical_clear(&value, style.text.direction, style.box_model.clear)
        }
        "table-layout" => {
            let Some(value) = crate::style::syntax::renderer_owned::table_layout(tokens) else {
                return false;
            };
            style.box_model.table_layout = value;
        }
        "border-collapse" => {
            let Some(value) = crate::style::syntax::renderer_owned::border_collapse(tokens) else {
                return false;
            };
            style.box_model.border_collapse = value;
        }
        "caption-side" => {
            let Some(value) = crate::style::syntax::renderer_owned::caption_side(tokens) else {
                return false;
            };
            style.box_model.caption_side = value;
        }
        "empty-cells" => {
            let Some(value) = crate::style::syntax::renderer_owned::empty_cells(tokens) else {
                return false;
            };
            style.box_model.empty_cells = value;
        }
        _ => return false,
    }
    true
}

pub(super) fn apply_text_box_property(
    style: &mut WorkingStyle,
    name: &str,
    tokens: &TokenList<'_>,
) -> bool {
    match name {
        "text-box-trim" => {
            let Some(trim) = crate::style::syntax::renderer_owned::text_box_trim(tokens) else {
                return false;
            };
            style.box_model.text_box_trim = trim;
        }
        "text-box-edge" => {
            let Some(edge) = crate::style::syntax::renderer_owned::text_box_edge(tokens) else {
                return false;
            };
            style.text.text_box_edge = edge;
        }
        "text-box" => {
            let Some((trim, edge)) = crate::style::syntax::renderer_owned::text_box(tokens) else {
                return false;
            };
            style.box_model.text_box_trim = trim;
            style.text.text_box_edge = edge;
        }
        _ => return false,
    }
    true
}
