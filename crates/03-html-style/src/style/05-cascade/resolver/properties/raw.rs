use super::*;
use crate::style::syntax::renderer_owned;

pub(super) fn apply_legacy_items(
    style: &mut WorkingStyle,
    property: &Property<'_>,
    parent: &ParentStyle,
) -> Option<ApplyResult> {
    use crate::style::syntax::alignment::{LegacyItems, legacy_items};

    let Property::Unparsed(value) = property else {
        return None;
    };
    if !matches!(
        value.property_id,
        PropertyId::JustifyItems | PropertyId::PlaceItems
    ) {
        return None;
    }
    let css = token_list_to_css_string(&value.value)?;
    let value = legacy_items(&value.property_id, &css)?;
    if let LegacyItems::Place(align) = value {
        let Some(align) = align_items(&align) else {
            return Some(ApplyResult::Invalid);
        };
        style.layout.align_items = align;
    }
    style.layout.justify_items = default_justify_items(parent.layout.justify_items);
    Some(ApplyResult::Applied)
}

pub(super) fn apply_custom_box_keyword(
    style: &mut WorkingStyle,
    id: RendererPropertyId,
    tokens: &TokenList<'_>,
) -> bool {
    match id {
        RendererPropertyId::Float => {
            let Some(value) = single_ident_keyword(tokens).map(str::to_ascii_lowercase) else {
                return false;
            };
            style.box_model.float =
                logical_float(&value, style.text.direction, style.box_model.float)
        }
        RendererPropertyId::Clear => {
            let Some(value) = single_ident_keyword(tokens).map(str::to_ascii_lowercase) else {
                return false;
            };
            style.box_model.clear =
                logical_clear(&value, style.text.direction, style.box_model.clear)
        }
        RendererPropertyId::TableLayout => {
            let Some(value) = renderer_owned::table_layout(tokens) else {
                return false;
            };
            style.box_model.table_layout = value;
        }
        RendererPropertyId::BorderCollapse => {
            let Some(value) = renderer_owned::border_collapse(tokens) else {
                return false;
            };
            style.box_model.border_collapse = value;
        }
        RendererPropertyId::CaptionSide => {
            let Some(value) = renderer_owned::caption_side(tokens) else {
                return false;
            };
            style.box_model.caption_side = value;
        }
        RendererPropertyId::EmptyCells => {
            let Some(value) = renderer_owned::empty_cells(tokens) else {
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
    id: RendererPropertyId,
    tokens: &TokenList<'_>,
) -> bool {
    match id {
        RendererPropertyId::TextBoxTrim => {
            let Some(trim) = renderer_owned::text_box_trim(tokens) else {
                return false;
            };
            style.box_model.text_box_trim = trim;
        }
        RendererPropertyId::TextBoxEdge => {
            let Some(edge) = renderer_owned::text_box_edge(tokens) else {
                return false;
            };
            style.text.text_box_edge = edge;
        }
        RendererPropertyId::TextBox => {
            let Some((trim, edge)) = renderer_owned::text_box(tokens) else {
                return false;
            };
            style.box_model.text_box_trim = trim;
            style.text.text_box_edge = edge;
        }
        _ => return false,
    }
    true
}
