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
    let Some(value) = single_ident_keyword(tokens).map(str::to_ascii_lowercase) else {
        return false;
    };
    match name.to_ascii_lowercase().as_str() {
        "float" => {
            style.box_model.float =
                logical_float(&value, style.text.direction, style.box_model.float)
        }
        "clear" => {
            style.box_model.clear =
                logical_clear(&value, style.text.direction, style.box_model.clear)
        }
        "table-layout" => {
            style.box_model.table_layout = match value.as_str() {
                "auto" => TableLayoutMode::Auto,
                "fixed" => TableLayoutMode::Fixed,
                _ => return false,
            }
        }
        "border-collapse" => {
            style.box_model.border_collapse = match value.as_str() {
                "collapse" => BorderCollapseMode::Collapse,
                "separate" => BorderCollapseMode::Separate,
                _ => return false,
            }
        }
        "caption-side" => {
            style.box_model.caption_side = match value.as_str() {
                "top" => CaptionSide::Top,
                "bottom" => CaptionSide::Bottom,
                _ => return false,
            }
        }
        "empty-cells" => {
            style.box_model.empty_cells = match value.as_str() {
                "show" => EmptyCellsMode::Show,
                "hide" => EmptyCellsMode::Hide,
                _ => return false,
            }
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
    let words = significant_tokens(tokens)
        .into_iter()
        .map(|token| token_ident(token).map(str::to_ascii_lowercase))
        .collect::<Option<Vec<_>>>();
    let Some(words) = words else { return false };
    match name {
        "text-box-trim" => {
            let [word] = words.as_slice() else {
                return false;
            };
            let Some(trim) = parse_text_box_trim(word) else {
                return false;
            };
            style.box_model.text_box_trim = trim;
        }
        "text-box-edge" => {
            let Some(edge) = parse_text_box_edge(&words) else {
                return false;
            };
            style.text.text_box_edge = edge;
        }
        "text-box" => {
            if words.as_slice() == ["normal"] {
                style.box_model.text_box_trim = TextBoxTrim::None;
                style.text.text_box_edge = TextBoxEdge::default();
                return true;
            }
            let mut trim = None;
            let mut edge_words = Vec::with_capacity(2);
            for word in &words {
                if let Some(value) = parse_text_box_trim(word) {
                    if trim.replace(value).is_some() {
                        return false;
                    }
                } else {
                    edge_words.push(word.clone());
                }
            }
            if edge_words.len() > 2 {
                return false;
            }
            let edge = if edge_words.is_empty() {
                TextBoxEdge::default()
            } else if let Some(edge) = parse_text_box_edge(&edge_words) {
                edge
            } else {
                return false;
            };
            style.box_model.text_box_trim = trim.unwrap_or(TextBoxTrim::Both);
            style.text.text_box_edge = edge;
        }
        _ => return false,
    }
    true
}

pub(super) fn parse_text_box_trim(word: &str) -> Option<TextBoxTrim> {
    match word {
        "none" => Some(TextBoxTrim::None),
        "trim-start" => Some(TextBoxTrim::Start),
        "trim-end" => Some(TextBoxTrim::End),
        "trim-both" => Some(TextBoxTrim::Both),
        _ => None,
    }
}

pub(super) fn parse_text_box_edge(words: &[String]) -> Option<TextBoxEdge> {
    if words == ["auto"] {
        return Some(TextBoxEdge::default());
    }
    let over = match words.first()?.as_str() {
        "text" | "ideographic" | "ideographic-ink" => TextBoxOverEdge::Text,
        "cap" => TextBoxOverEdge::Cap,
        "ex" => TextBoxOverEdge::Ex,
        _ => return None,
    };
    let under = match words.get(1).map(String::as_str).unwrap_or("text") {
        "text" | "ideographic" | "ideographic-ink" => TextBoxUnderEdge::Text,
        "alphabetic" => TextBoxUnderEdge::Alphabetic,
        _ => return None,
    };
    Some(TextBoxEdge { over, under })
}
