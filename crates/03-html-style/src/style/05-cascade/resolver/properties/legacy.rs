use super::*;

/// Lightning CSS preserves the legacy Grid gap aliases as raw custom
/// declarations. Resolve them with the canonical Box Alignment grammar while
/// keeping their independent place in cascade order.
pub(super) fn try_apply_legacy_grid_gap_alias(
    doc: &Document,
    style: &mut WorkingStyle,
    property: &Property<'_>,
    parent: &ParentStyle,
    resolution: &ResolutionContext,
) -> bool {
    #[derive(Clone, Copy)]
    enum Axis {
        Row,
        Column,
        Both,
    }

    let (name, tokens) = match property {
        Property::Custom(custom) => (custom.name.as_ref(), &custom.value),
        Property::Unparsed(unparsed) => (unparsed.property_id.name(), &unparsed.value),
        _ => return false,
    };
    let axis = match name {
        "grid-row-gap" => Axis::Row,
        "grid-column-gap" => Axis::Column,
        "grid-gap" => Axis::Both,
        _ => return false,
    };

    if let Some(keyword) = single_ident_keyword(tokens).map(str::to_ascii_lowercase) {
        let values = match keyword.as_str() {
            "inherit" => Some((parent.layout.row_gap, parent.layout.column_gap)),
            "initial" | "unset" => Some((LengthPct::Px(0.0), LengthPct::Px(0.0))),
            "revert" | "revert-layer" => return true,
            _ => None,
        };
        if let Some((row, column)) = values {
            match axis {
                Axis::Row => style.layout.row_gap = row,
                Axis::Column => style.layout.column_gap = column,
                Axis::Both => {
                    style.layout.row_gap = row;
                    style.layout.column_gap = column;
                }
            }
            return true;
        }
    }

    let Some(css) = token_list_to_css_string(tokens) else {
        return true;
    };
    let canonical = match axis {
        Axis::Row => "row-gap",
        Axis::Column => "column-gap",
        Axis::Both => "gap",
    };
    let Ok(property) =
        Property::parse_string(PropertyId::from(canonical), &css, ParserOptions::default())
    else {
        return true;
    };
    match (axis, property) {
        (Axis::Row, Property::RowGap(value)) => {
            if let Some(value) = gap_value(
                &value,
                style.font.font_size,
                doc.root_font_size(),
                resolution,
            ) {
                style.layout.row_gap = value;
            }
        }
        (Axis::Column, Property::ColumnGap(value)) => {
            if let Some(value) = gap_value(
                &value,
                style.font.font_size,
                doc.root_font_size(),
                resolution,
            ) {
                style.layout.column_gap = value;
            }
        }
        (Axis::Both, Property::Gap(value)) => {
            if let (Some(row), Some(column)) = (
                gap_value(
                    &value.row,
                    style.font.font_size,
                    doc.root_font_size(),
                    resolution,
                ),
                gap_value(
                    &value.column,
                    style.font.font_size,
                    doc.root_font_size(),
                    resolution,
                ),
            ) {
                style.layout.row_gap = row;
                style.layout.column_gap = column;
            }
        }
        _ => {}
    }
    true
}
