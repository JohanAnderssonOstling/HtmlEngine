use super::super::context::{ApplyResult, PropertyContext};
use super::super::values::fonts::computed_flex_factor;
use super::super::values::grid::{
    grid_auto_flow, grid_auto_tracks, grid_placement, grid_template_areas, grid_template_tracks,
};
use super::super::values::{
    align_items, align_self, content_alignment, flex_basis, flex_direction, flex_wrap, gap_value,
    justify_content, justify_items, justify_self,
};
use html_style_model::Display;
use lightningcss::properties::Property;

pub(super) fn apply(context: &mut PropertyContext<'_>, property: &Property<'_>) -> ApplyResult {
    let root_font_size = context.root_font_size;
    let resolution = context.resolution;
    let styles = &mut *context.styles;
    let style = &mut *context.style;
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

        Property::FlexGrow(value, _) => {
            style.layout.flex_grow = computed_flex_factor(*value);
        }
        Property::FlexShrink(value, _) => {
            style.layout.flex_shrink = computed_flex_factor(*value);
        }
        Property::FlexBasis(value, _) => {
            let Some(value) = flex_basis(value, &style.font, root_font_size, styles, resolution)
            else {
                return ApplyResult::Invalid;
            };
            style.layout.flex_basis = value;
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

        Property::RowGap(value) | Property::ColumnGap(value) => {
            let Some(value) = gap_value(value, style.font.font_size, root_font_size, resolution)
            else {
                return ApplyResult::Invalid;
            };
            *match property {
                Property::RowGap(_) => &mut style.layout.row_gap,
                _ => &mut style.layout.column_gap,
            } = value;
        }

        Property::GridTemplateRows(value) | Property::GridTemplateColumns(value) => {
            let Some((tracks, names)) = grid_template_tracks(
                styles,
                value,
                style.font.font_size,
                root_font_size,
                resolution,
            ) else {
                return ApplyResult::Invalid;
            };
            match property {
                Property::GridTemplateRows(_) => {
                    style.layout.grid_template_rows = tracks;
                    style.layout.grid_template_row_names = names;
                }
                _ => {
                    style.layout.grid_template_columns = tracks;
                    style.layout.grid_template_column_names = names;
                }
            }
        }
        Property::GridAutoRows(value) | Property::GridAutoColumns(value) => {
            let Some(value) =
                grid_auto_tracks(value, style.font.font_size, root_font_size, resolution)
            else {
                return ApplyResult::Invalid;
            };
            *match property {
                Property::GridAutoRows(_) => &mut style.layout.grid_auto_rows,
                _ => &mut style.layout.grid_auto_columns,
            } = value;
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

        Property::GridRowStart(value)
        | Property::GridRowEnd(value)
        | Property::GridColumnStart(value)
        | Property::GridColumnEnd(value) => {
            let Some(value) = grid_placement(styles, value) else {
                return ApplyResult::Invalid;
            };
            *match property {
                Property::GridRowStart(_) => &mut style.layout.grid_row.start,
                Property::GridRowEnd(_) => &mut style.layout.grid_row.end,
                Property::GridColumnStart(_) => &mut style.layout.grid_column.start,
                _ => &mut style.layout.grid_column.end,
            } = value;
        }

        _ => return ApplyResult::Unhandled,
    }
    ApplyResult::Applied
}
