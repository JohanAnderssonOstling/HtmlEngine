use super::*;

#[allow(unused_variables)]
pub(super) fn apply(context: &mut PropertyContext<'_, '_>, property: &Property<'_>) -> ApplyResult {
    let doc = context.doc;
    let styles = &mut *context.styles;
    let style = &mut *context.style;
    let resolved_root_font_size = context.resolved_root_font_size;
    let parent_font_size = context.parent_font_size;
    let parent_font_weight = context.parent_font_weight;
    let parent_color = context.parent_color;
    let environment = context.environment;
    match property {
        // Display
        Property::Display(d) => {
            use lightningcss::properties::display::{
                Display as LcDisplay, DisplayInside, DisplayKeyword, DisplayOutside,
            };
            let display = match d {
                LcDisplay::Keyword(kw) => match kw {
                    DisplayKeyword::None => Display::None,
                    DisplayKeyword::TableRowGroup => Display::TableRowGroup,
                    DisplayKeyword::TableHeaderGroup => Display::TableHeaderGroup,
                    DisplayKeyword::TableFooterGroup => Display::TableFooterGroup,
                    DisplayKeyword::TableRow => Display::TableRow,
                    DisplayKeyword::TableColumnGroup => Display::TableColumnGroup,
                    DisplayKeyword::TableColumn => Display::TableColumn,
                    DisplayKeyword::TableCell => Display::TableCell,
                    DisplayKeyword::TableCaption => Display::TableCaption,
                    DisplayKeyword::Contents => Display::Contents,
                    // Ruby internals need their own formatting context. Keep an
                    // earlier supported fallback declaration active until it exists.
                    DisplayKeyword::RubyBase
                    | DisplayKeyword::RubyText
                    | DisplayKeyword::RubyBaseContainer
                    | DisplayKeyword::RubyTextContainer => return ApplyResult::Invalid,
                },
                LcDisplay::Pair(pair)
                    if pair.is_list_item
                        && matches!(pair.outside, DisplayOutside::Block)
                        && matches!(pair.inside, DisplayInside::Flow) =>
                {
                    Display::ListItem
                }
                LcDisplay::Pair(pair)
                    if pair.is_list_item
                        && matches!(pair.outside, DisplayOutside::Block)
                        && matches!(pair.inside, DisplayInside::FlowRoot) =>
                {
                    Display::FlowRootListItem
                }
                LcDisplay::Pair(pair) if pair.is_list_item => return ApplyResult::Invalid,
                LcDisplay::Pair(pair) => match (&pair.outside, &pair.inside) {
                    (DisplayOutside::Block, DisplayInside::Table) => Display::Table,
                    (DisplayOutside::Inline, DisplayInside::Table) => Display::InlineTable,
                    (DisplayOutside::Block, DisplayInside::Flow) => Display::Block,
                    (DisplayOutside::Block, DisplayInside::FlowRoot) => Display::FlowRoot,
                    (DisplayOutside::Inline, DisplayInside::Flow) => Display::Inline,
                    (DisplayOutside::Inline, DisplayInside::FlowRoot) => Display::InlineBlock,
                    (DisplayOutside::Block, DisplayInside::Flex(_)) => Display::Flex,
                    (DisplayOutside::Inline, DisplayInside::Flex(_)) => Display::InlineFlex,
                    (DisplayOutside::Block, DisplayInside::Grid) => Display::Grid,
                    (DisplayOutside::Inline, DisplayInside::Grid) => Display::InlineGrid,
                    // Flow-root, ruby, run-in, and legacy box layout are not
                    // silently approximated by normal flow.
                    _ => return ApplyResult::Invalid,
                },
            };
            style.box_model.display = display;
        }
        Property::FlexDirection(value, _) => style.layout.flex_direction = flex_direction(value),
        Property::FlexWrap(value, _) => style.layout.flex_wrap = flex_wrap(value),
        Property::FlexFlow(value, _) => {
            style.layout.flex_direction = flex_direction(&value.direction);
            style.layout.flex_wrap = flex_wrap(&value.wrap);
        }
        Property::FlexGrow(value, _) => {
            let Some(value) = checked_flex_factor(*value) else {
                return ApplyResult::Invalid;
            };
            style.layout.flex_grow = value;
        }
        Property::FlexShrink(value, _) => {
            let Some(value) = checked_flex_factor(*value) else {
                return ApplyResult::Invalid;
            };
            style.layout.flex_shrink = value;
        }
        Property::FlexBasis(value, _) => {
            let Some(value) = flex_basis(value, &style.font, doc.root_font_size(), styles) else {
                return ApplyResult::Invalid;
            };
            style.layout.flex_basis = value;
        }
        Property::Flex(value, _) => {
            let Some(basis) = flex_basis(&value.basis, &style.font, doc.root_font_size(), styles)
            else {
                return ApplyResult::Invalid;
            };
            let (Some(grow), Some(shrink)) = (
                checked_flex_factor(value.grow),
                checked_flex_factor(value.shrink),
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.flex_grow = grow;
            style.layout.flex_shrink = shrink;
            style.layout.flex_basis = basis;
        }
        Property::Order(value, _) => style.layout.order = *value,
        Property::AlignContent(value, _) => {
            let Some(value) = content_alignment(value) else {
                return ApplyResult::Invalid;
            };
            style.layout.align_content = value;
        }
        Property::JustifyContent(value, _) => style.layout.justify_content = justify_content(value),
        Property::AlignItems(value, _) => {
            let Some(value) = align_items(value) else {
                return ApplyResult::Invalid;
            };
            style.layout.align_items = value;
        }
        Property::AlignSelf(value, _) => {
            let Some(value) = align_self(value) else {
                return ApplyResult::Invalid;
            };
            style.layout.align_self = value;
        }
        Property::JustifyItems(value) => {
            let Some(value) = justify_items(value) else {
                return ApplyResult::Invalid;
            };
            style.layout.justify_items = value;
        }
        Property::JustifySelf(value) => {
            let Some(value) = justify_self(value) else {
                return ApplyResult::Invalid;
            };
            style.layout.justify_self = value;
        }
        Property::PlaceContent(value) => {
            let Some(align) = content_alignment(&value.align) else {
                return ApplyResult::Invalid;
            };
            style.layout.align_content = align;
            style.layout.justify_content = justify_content(&value.justify);
        }
        Property::PlaceItems(value) => {
            let (Some(align), Some(justify)) =
                (align_items(&value.align), justify_items(&value.justify))
            else {
                return ApplyResult::Invalid;
            };
            style.layout.align_items = align;
            style.layout.justify_items = justify;
        }
        Property::PlaceSelf(value) => {
            let (Some(align), Some(justify)) =
                (align_self(&value.align), justify_self(&value.justify))
            else {
                return ApplyResult::Invalid;
            };
            style.layout.align_self = align;
            style.layout.justify_self = justify;
        }
        Property::RowGap(value) => {
            let Some(value) = gap_value(value, style.font.font_size, doc.root_font_size()) else {
                return ApplyResult::Invalid;
            };
            style.layout.row_gap = value;
        }
        Property::ColumnGap(value) => {
            let Some(value) = gap_value(value, style.font.font_size, doc.root_font_size()) else {
                return ApplyResult::Invalid;
            };
            style.layout.column_gap = value;
        }
        Property::Gap(value) => {
            let (Some(row), Some(column)) = (
                gap_value(&value.row, style.font.font_size, doc.root_font_size()),
                gap_value(&value.column, style.font.font_size, doc.root_font_size()),
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.row_gap = row;
            style.layout.column_gap = column;
        }
        Property::GridTemplateRows(value) => {
            let Some((tracks, names)) =
                grid_template_tracks(styles, value, style.font.font_size, doc.root_font_size())
            else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_template_rows = tracks;
            style.layout.grid_template_row_names = names;
        }
        Property::GridTemplateColumns(value) => {
            let Some((tracks, names)) =
                grid_template_tracks(styles, value, style.font.font_size, doc.root_font_size())
            else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_template_columns = tracks;
            style.layout.grid_template_column_names = names;
        }
        Property::GridAutoRows(value) => {
            let Some(value) = grid_auto_tracks(value, style.font.font_size, doc.root_font_size())
            else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_auto_rows = value;
        }
        Property::GridAutoColumns(value) => {
            let Some(value) = grid_auto_tracks(value, style.font.font_size, doc.root_font_size())
            else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_auto_columns = value;
        }
        Property::GridAutoFlow(value) => style.layout.grid_auto_flow = grid_auto_flow(*value),
        Property::GridTemplateAreas(value) => {
            let Some((areas, rows, columns)) = grid_template_areas(styles, value) else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_template_areas = areas;
            style.layout.grid_template_area_rows = rows;
            style.layout.grid_template_area_columns = columns;
        }
        Property::GridTemplate(value) => {
            let (
                Some((rows, row_names)),
                Some((columns, column_names)),
                Some((areas, area_rows, area_columns)),
            ) = (
                grid_template_tracks(
                    styles,
                    &value.rows,
                    style.font.font_size,
                    doc.root_font_size(),
                ),
                grid_template_tracks(
                    styles,
                    &value.columns,
                    style.font.font_size,
                    doc.root_font_size(),
                ),
                grid_template_areas(styles, &value.areas),
            )
            else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_template_rows = rows;
            style.layout.grid_template_row_names = row_names;
            style.layout.grid_template_columns = columns;
            style.layout.grid_template_column_names = column_names;
            style.layout.grid_template_areas = areas;
            style.layout.grid_template_area_rows = area_rows;
            style.layout.grid_template_area_columns = area_columns;
        }
        Property::Grid(value) => {
            let (
                Some((rows, row_names)),
                Some((columns, column_names)),
                Some((areas, area_rows, area_columns)),
                Some(auto_rows),
                Some(auto_columns),
            ) = (
                grid_template_tracks(
                    styles,
                    &value.rows,
                    style.font.font_size,
                    doc.root_font_size(),
                ),
                grid_template_tracks(
                    styles,
                    &value.columns,
                    style.font.font_size,
                    doc.root_font_size(),
                ),
                grid_template_areas(styles, &value.areas),
                grid_auto_tracks(&value.auto_rows, style.font.font_size, doc.root_font_size()),
                grid_auto_tracks(
                    &value.auto_columns,
                    style.font.font_size,
                    doc.root_font_size(),
                ),
            )
            else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_template_rows = rows;
            style.layout.grid_template_row_names = row_names;
            style.layout.grid_template_columns = columns;
            style.layout.grid_template_column_names = column_names;
            style.layout.grid_template_areas = areas;
            style.layout.grid_template_area_rows = area_rows;
            style.layout.grid_template_area_columns = area_columns;
            style.layout.grid_auto_rows = auto_rows;
            style.layout.grid_auto_columns = auto_columns;
            style.layout.grid_auto_flow = grid_auto_flow(value.auto_flow);
        }
        Property::GridRowStart(value) => {
            let Some(value) = grid_placement(styles, value) else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_row.start = value;
        }
        Property::GridRowEnd(value) => {
            let Some(value) = grid_placement(styles, value) else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_row.end = value;
        }
        Property::GridColumnStart(value) => {
            let Some(value) = grid_placement(styles, value) else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_column.start = value;
        }
        Property::GridColumnEnd(value) => {
            let Some(value) = grid_placement(styles, value) else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_column.end = value;
        }
        Property::GridRow(value) => {
            let (Some(start), Some(end)) = (
                grid_placement(styles, &value.start),
                grid_placement(styles, &value.end),
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_row = GridPlacementRange { start, end };
        }
        Property::GridColumn(value) => {
            let (Some(start), Some(end)) = (
                grid_placement(styles, &value.start),
                grid_placement(styles, &value.end),
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_column = GridPlacementRange { start, end };
        }
        Property::GridArea(value) => {
            let (Some(row_start), Some(column_start), Some(row_end), Some(column_end)) = (
                grid_placement(styles, &value.row_start),
                grid_placement(styles, &value.column_start),
                grid_placement(styles, &value.row_end),
                grid_placement(styles, &value.column_end),
            ) else {
                return ApplyResult::Invalid;
            };
            style.layout.grid_row = GridPlacementRange {
                start: row_start,
                end: row_end,
            };
            style.layout.grid_column = GridPlacementRange {
                start: column_start,
                end: column_end,
            };
        }
        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
