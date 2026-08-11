//! CSS-wide keyword application, including origin and layer rollback.

use super::*;

fn is_inherited_property(name: &str) -> bool {
    matches!(
        name,
        "color"
            | "direction"
            | "font"
            | "font-size"
            | "font-weight"
            | "font-style"
            | "font-family"
            | "font-variant-caps"
            | "font-variant-numeric"
            | "font-variant-ligatures"
            | "font-kerning"
            | "font-feature-settings"
            | "line-height"
            | "letter-spacing"
            | "word-spacing"
            | "tab-size"
            | "text-align"
            | "text-align-last"
            | "text-indent"
            | "text-transform"
            | "text-box-edge"
            | "white-space"
            | "word-break"
            | "overflow-wrap"
            | "word-wrap"
            | "list-style"
            | "list-style-type"
            | "list-style-position"
            | "list-style-image"
            | "border-collapse"
            | "border-spacing"
            | "caption-side"
            | "empty-cells"
            | "hyphens"
            | "quotes"
            | "widows"
            | "orphans"
    )
}

/// Handle the CSS-wide keywords `inherit`, `initial`, `unset`, and `revert`.
/// lightningcss leaves them as `Unparsed` (typed properties only hold concrete
/// values), so they would otherwise be silently dropped. Returns true when the
/// declaration was consumed.
pub(super) fn try_apply_css_wide_keyword(
    style: &mut WorkingStyle,
    property: &Property,
    parent: &ParentStyle,
    root_font_size: f32,
    phase: CascadePhase,
    revert_basis: &WorkingStyle,
    revert_layer_basis: &WorkingStyle,
) -> bool {
    let (name, value) = match property {
        Property::Unparsed(unparsed) => (unparsed.property_id.name(), &unparsed.value),
        Property::Custom(custom) if !custom.name.as_ref().starts_with("--") => {
            (custom.name.as_ref(), &custom.value)
        }
        _ => return false,
    };
    let Some(keyword) = single_ident_keyword(value).map(str::to_ascii_lowercase) else {
        return false;
    };

    apply_css_wide_keyword_in_phase(
        style,
        name,
        &keyword,
        parent,
        root_font_size,
        phase,
        Some(revert_basis),
        Some(revert_layer_basis),
    )
}

pub(super) fn apply_css_wide_keyword_in_phase(
    style: &mut WorkingStyle,
    name: &str,
    keyword: &str,
    parent: &ParentStyle,
    root_font_size: f32,
    phase: CascadePhase,
    revert_basis: Option<&WorkingStyle>,
    revert_layer_basis: Option<&WorkingStyle>,
) -> bool {
    if phase == CascadePhase::Remaining
        && matches!(name, "color" | "direction" | "font-size" | "line-height")
    {
        return matches!(
            keyword,
            "inherit" | "initial" | "unset" | "revert" | "revert-layer"
        );
    }
    if phase != CascadePhase::Remaining || name != "font" {
        return apply_css_wide_keyword_with_rollback(
            style,
            name,
            keyword,
            parent,
            root_font_size,
            revert_basis,
            revert_layer_basis,
        );
    }

    // The prerequisite pass has already selected the winning font size and
    // line height across longhands, shorthands, and CSS-wide keywords.
    // Applying the remaining components of a wide `font` shorthand must not
    // replay either earlier value.
    let final_font_size = (
        style.font.font_size,
        style.font.font_size_x_height_px,
        style.font.font_size_ch_advance_px,
        style.font.font_size_cap_height_px,
        style.font.font_size_root_ch,
        style.font.font_size_root_cap_height,
        style.font.font_size_root_line_height,
    );
    let final_line_height = (
        style.line_height_spec.clone(),
        style.text.line_height_number,
        style.text.line_height,
        style.text.line_height_x_height_px,
        style.text.line_height_normal,
    );
    let consumed = apply_css_wide_keyword_with_rollback(
        style,
        name,
        keyword,
        parent,
        root_font_size,
        revert_basis,
        revert_layer_basis,
    );
    style.font.font_size = final_font_size.0;
    style.font.font_size_x_height_px = final_font_size.1;
    style.font.font_size_ch_advance_px = final_font_size.2;
    style.font.font_size_cap_height_px = final_font_size.3;
    style.font.font_size_root_ch = final_font_size.4;
    style.font.font_size_root_cap_height = final_font_size.5;
    style.font.font_size_root_line_height = final_font_size.6;
    style.line_height_spec = final_line_height.0;
    style.text.line_height_number = final_line_height.1;
    style.text.line_height = final_line_height.2;
    style.text.line_height_x_height_px = final_line_height.3;
    style.text.line_height_normal = final_line_height.4;
    consumed
}

fn apply_css_wide_keyword_with_rollback(
    style: &mut WorkingStyle,
    name: &str,
    keyword: &str,
    parent: &ParentStyle,
    root_font_size: f32,
    revert_basis: Option<&WorkingStyle>,
    revert_layer_basis: Option<&WorkingStyle>,
) -> bool {
    let initial;
    let reverted;
    let src: &ParentStyle = match keyword {
        "inherit" => parent,
        "initial" => {
            initial = ParentStyle::initial(root_font_size);
            &initial
        }
        "unset" => {
            if is_inherited_property(name) {
                parent
            } else {
                initial = ParentStyle::initial(root_font_size);
                &initial
            }
        }
        "revert" => {
            let Some(basis) = revert_basis else {
                return true;
            };
            reverted = ParentStyle::from_working(basis);
            &reverted
        }
        "revert-layer" => {
            let Some(basis) = revert_layer_basis else {
                return true;
            };
            reverted = ParentStyle::from_working(basis);
            &reverted
        }
        _ => return false,
    };

    match name {
        "color" => style.text.color = src.text.color,
        "direction" => style.text.direction = src.text.direction,
        "font" => {
            style.font = src.font.clone();
            style.text.line_height = src.text.line_height;
            style.text.line_height_x_height_px = src.text.line_height_x_height_px;
            style.text.line_height_number = src.text.line_height_number;
            style.text.line_height_normal = src.text.line_height_normal;
            style.line_height_spec = None;
        }
        "font-size" => {
            style.font.font_size = src.font.font_size;
            style.font.font_size_x_height_px = src.font.font_size_x_height_px;
            style.font.font_size_ch_advance_px = src.font.font_size_ch_advance_px;
            style.font.font_size_cap_height_px = src.font.font_size_cap_height_px;
            style.font.font_size_root_ch = src.font.font_size_root_ch;
            style.font.font_size_root_cap_height = src.font.font_size_root_cap_height;
            style.font.font_size_root_line_height = src.font.font_size_root_line_height;
        }
        "font-weight" => style.font.font_weight = src.font.font_weight,
        "font-style" => style.font.font_style = src.font.font_style,
        "font-family" => style.font.font_family = src.font.font_family,
        "font-variant-caps" => {
            style.font.font_variant_caps_features = src.font.font_variant_caps_features.clone()
        }
        "font-variant-numeric" => {
            style.font.font_variant_numeric_features =
                src.font.font_variant_numeric_features.clone()
        }
        "font-variant-ligatures" => {
            style.font.font_variant_ligature_features =
                src.font.font_variant_ligature_features.clone()
        }
        "font-kerning" => style.font.font_kerning_features = src.font.font_kerning_features.clone(),
        "font-feature-settings" => {
            style.font.font_feature_settings = src.font.font_feature_settings.clone()
        }
        "line-height" => {
            style.text.line_height = src.text.line_height;
            style.text.line_height_x_height_px = src.text.line_height_x_height_px;
            style.text.line_height_number = src.text.line_height_number;
            style.text.line_height_normal = src.text.line_height_normal;
            style.line_height_spec = None;
        }
        "letter-spacing" => style.text.letter_spacing = src.text.letter_spacing,
        "word-spacing" => style.text.word_spacing = src.text.word_spacing,
        "quotes" => style.text.quotes = src.text.quotes,
        "tab-size" => style.text.tab_size = src.text.tab_size,
        "text-align" => {
            style.text.text_align = src.text.text_align;
            style.text.text_align_logical = src.text.text_align_logical;
            if !style.text.text_align_last_explicit {
                style.text.text_align_last = style.text.text_align;
                style.text.text_align_last_logical = style.text.text_align_logical;
            }
        }
        "text-align-last" => {
            style.text.text_align_last = src.text.text_align_last;
            style.text.text_align_last_logical = src.text.text_align_last_logical;
            style.text.text_align_last_explicit = src.text.text_align_last_explicit;
        }
        "text-indent" => {
            style.text.text_indent = src.text.text_indent;
            style.text.text_indent_hanging = src.text.text_indent_hanging;
            style.text.text_indent_each_line = src.text.text_indent_each_line;
        }
        "text-transform" => style.text.text_transform = src.text.text_transform,
        "white-space" => style.text.white_space = src.text.white_space,
        "word-break" => style.text.word_break = src.text.word_break,
        "overflow-wrap" | "word-wrap" => style.text.overflow_wrap = src.text.overflow_wrap,
        "overflow" => {
            style.box_model.overflow_x = src.box_model.overflow_x;
            style.box_model.overflow_y = src.box_model.overflow_y;
        }
        "overflow-x" => style.box_model.overflow_x = src.box_model.overflow_x,
        "overflow-y" => style.box_model.overflow_y = src.box_model.overflow_y,
        "text-overflow" => style.box_model.text_overflow = src.box_model.text_overflow,
        "text-box" => {
            style.box_model.text_box_trim = src.box_model.text_box_trim;
            style.text.text_box_edge = src.text.text_box_edge;
        }
        "text-box-trim" => style.box_model.text_box_trim = src.box_model.text_box_trim,
        "text-box-edge" => style.text.text_box_edge = src.text.text_box_edge,
        "box-sizing" => style.box_model.box_sizing = src.box_model.box_sizing,
        "list-style" => {
            style.text.list_style_type = src.text.list_style_type;
            style.text.list_style_position = src.text.list_style_position;
            style.text.list_style_image = src.text.list_style_image;
        }
        "list-style-type" => style.text.list_style_type = src.text.list_style_type,
        "list-style-position" => style.text.list_style_position = src.text.list_style_position,
        "list-style-image" => style.text.list_style_image = src.text.list_style_image,
        "text-decoration" => style.background.text_decoration = src.background.text_decoration,
        "text-decoration-line" => {
            style.background.text_decoration.lines = src.background.text_decoration.lines
        }
        "text-decoration-color" => {
            style.background.text_decoration.color = src.background.text_decoration.color
        }
        "text-decoration-style" => {
            style.background.text_decoration.style = src.background.text_decoration.style
        }
        "text-decoration-thickness" => {
            style.background.text_decoration.thickness = src.background.text_decoration.thickness
        }
        "outline" => style.background.outline = src.background.outline,
        "outline-width" => {
            style
                .background
                .outline
                .set_width(src.background.outline.width());
        }
        "outline-style" => style.background.outline.style = src.background.outline.style,
        "outline-color" => style.background.outline.color = src.background.outline.color,
        "background" => {
            style.background.background_color = src.background.background_color;
            style.background.background_color_current_color =
                src.background.background_color_current_color;
            style.background.background_image_present = src.background.background_image_present;
        }
        "background-color" => {
            style.background.background_color = src.background.background_color;
            style.background.background_color_current_color =
                src.background.background_color_current_color;
        }
        "background-image" => {
            style.background.background_image_present = src.background.background_image_present;
        }
        "vertical-align" => style.box_model.vertical_align = src.box_model.vertical_align,
        "contain" => style.box_model.size_containment = src.box_model.size_containment,
        "display" => style.box_model.display = src.box_model.display,
        "position" => style.layout.position = src.layout.position,
        "break-before" | "page-break-before" => style.layout.break_before = src.layout.break_before,
        "break-after" | "page-break-after" => style.layout.break_after = src.layout.break_after,
        "break-inside" | "page-break-inside" => style.layout.break_inside = src.layout.break_inside,
        "widows" => style.text.widows = src.text.widows,
        "orphans" => style.text.orphans = src.text.orphans,
        "z-index" => style.layout.z_index = src.layout.z_index,
        "top" => style.layout.inset_top = src.layout.inset_top,
        "right" => style.layout.inset_right = src.layout.inset_right,
        "bottom" => style.layout.inset_bottom = src.layout.inset_bottom,
        "left" => style.layout.inset_left = src.layout.inset_left,
        "inset" => {
            style.layout.inset_top = src.layout.inset_top;
            style.layout.inset_right = src.layout.inset_right;
            style.layout.inset_bottom = src.layout.inset_bottom;
            style.layout.inset_left = src.layout.inset_left;
        }
        "flex-direction" => style.layout.flex_direction = src.layout.flex_direction,
        "flex-wrap" => style.layout.flex_wrap = src.layout.flex_wrap,
        "flex-flow" => {
            style.layout.flex_direction = src.layout.flex_direction;
            style.layout.flex_wrap = src.layout.flex_wrap;
        }
        "flex-grow" => style.layout.flex_grow = src.layout.flex_grow,
        "flex-shrink" => style.layout.flex_shrink = src.layout.flex_shrink,
        "flex-basis" => style.layout.flex_basis = src.layout.flex_basis,
        "flex" => {
            style.layout.flex_grow = src.layout.flex_grow;
            style.layout.flex_shrink = src.layout.flex_shrink;
            style.layout.flex_basis = src.layout.flex_basis;
        }
        "order" => style.layout.order = src.layout.order,
        "align-content" => style.layout.align_content = src.layout.align_content,
        "justify-content" => style.layout.justify_content = src.layout.justify_content,
        "place-content" => {
            style.layout.align_content = src.layout.align_content;
            style.layout.justify_content = src.layout.justify_content;
        }
        "align-items" => style.layout.align_items = src.layout.align_items,
        "justify-items" => style.layout.justify_items = src.layout.justify_items,
        "place-items" => {
            style.layout.align_items = src.layout.align_items;
            style.layout.justify_items = src.layout.justify_items;
        }
        "align-self" => style.layout.align_self = src.layout.align_self,
        "justify-self" => style.layout.justify_self = src.layout.justify_self,
        "place-self" => {
            style.layout.align_self = src.layout.align_self;
            style.layout.justify_self = src.layout.justify_self;
        }
        "row-gap" | "grid-row-gap" => style.layout.row_gap = src.layout.row_gap,
        "column-gap" | "grid-column-gap" => style.layout.column_gap = src.layout.column_gap,
        "gap" => {
            style.layout.row_gap = src.layout.row_gap;
            style.layout.column_gap = src.layout.column_gap;
        }
        "grid-template-rows" => {
            style.layout.grid_template_rows = src.layout.grid_template_rows.clone();
            style.layout.grid_template_row_names = src.layout.grid_template_row_names.clone();
        }
        "grid-template-columns" => {
            style.layout.grid_template_columns = src.layout.grid_template_columns.clone();
            style.layout.grid_template_column_names = src.layout.grid_template_column_names.clone();
        }
        "grid-template-areas" => {
            style.layout.grid_template_areas = src.layout.grid_template_areas.clone();
            style.layout.grid_template_area_rows = src.layout.grid_template_area_rows;
            style.layout.grid_template_area_columns = src.layout.grid_template_area_columns;
        }
        "grid-template" => {
            style.layout.grid_template_rows = src.layout.grid_template_rows.clone();
            style.layout.grid_template_columns = src.layout.grid_template_columns.clone();
            style.layout.grid_template_row_names = src.layout.grid_template_row_names.clone();
            style.layout.grid_template_column_names = src.layout.grid_template_column_names.clone();
            style.layout.grid_template_areas = src.layout.grid_template_areas.clone();
            style.layout.grid_template_area_rows = src.layout.grid_template_area_rows;
            style.layout.grid_template_area_columns = src.layout.grid_template_area_columns;
        }
        "grid-auto-rows" => style.layout.grid_auto_rows = src.layout.grid_auto_rows.clone(),
        "grid-auto-columns" => {
            style.layout.grid_auto_columns = src.layout.grid_auto_columns.clone()
        }
        "grid-auto-flow" => style.layout.grid_auto_flow = src.layout.grid_auto_flow,
        "grid-row-start" => style.layout.grid_row.start = src.layout.grid_row.start,
        "grid-row-end" => style.layout.grid_row.end = src.layout.grid_row.end,
        "grid-row" => style.layout.grid_row = src.layout.grid_row,
        "grid-column-start" => style.layout.grid_column.start = src.layout.grid_column.start,
        "grid-column-end" => style.layout.grid_column.end = src.layout.grid_column.end,
        "grid-column" => style.layout.grid_column = src.layout.grid_column,
        "grid-area" => {
            style.layout.grid_row = src.layout.grid_row;
            style.layout.grid_column = src.layout.grid_column;
        }
        "grid" => {
            style.layout.grid_template_rows = src.layout.grid_template_rows.clone();
            style.layout.grid_template_columns = src.layout.grid_template_columns.clone();
            style.layout.grid_template_row_names = src.layout.grid_template_row_names.clone();
            style.layout.grid_template_column_names = src.layout.grid_template_column_names.clone();
            style.layout.grid_template_areas = src.layout.grid_template_areas.clone();
            style.layout.grid_template_area_rows = src.layout.grid_template_area_rows;
            style.layout.grid_template_area_columns = src.layout.grid_template_area_columns;
            style.layout.grid_auto_rows = src.layout.grid_auto_rows.clone();
            style.layout.grid_auto_columns = src.layout.grid_auto_columns.clone();
            style.layout.grid_auto_flow = src.layout.grid_auto_flow;
        }
        "float" => style.box_model.float = src.box_model.float,
        "clear" => style.box_model.clear = src.box_model.clear,
        "table-layout" => style.box_model.table_layout = src.box_model.table_layout,
        "border-collapse" => style.box_model.border_collapse = src.box_model.border_collapse,
        "border-spacing" => {
            style.box_model.border_spacing_horizontal = src.box_model.border_spacing_horizontal;
            style.box_model.border_spacing_vertical = src.box_model.border_spacing_vertical;
        }
        "caption-side" => style.box_model.caption_side = src.box_model.caption_side,
        "empty-cells" => style.box_model.empty_cells = src.box_model.empty_cells,
        "width" | "inline-size" => style.box_model.width = src.box_model.width,
        "height" | "block-size" => style.box_model.height = src.box_model.height,
        "aspect-ratio" => style.box_model.aspect_ratio = src.box_model.aspect_ratio,
        "min-width" | "min-inline-size" => style.box_model.min_width = src.box_model.min_width,
        "min-height" | "min-block-size" => style.box_model.min_height = src.box_model.min_height,
        "max-width" | "max-inline-size" => style.box_model.max_width = src.box_model.max_width,
        "max-height" | "max-block-size" => style.box_model.max_height = src.box_model.max_height,
        "margin-top" | "margin-block-start" => {
            style.box_model.margin_top = src.box_model.margin_top;
            style.layout.margin_top_auto = src.layout.margin_top_auto;
        }
        "margin-bottom" | "margin-block-end" => {
            style.box_model.margin_bottom = src.box_model.margin_bottom;
            style.layout.margin_bottom_auto = src.layout.margin_bottom_auto;
        }
        "margin-left" => {
            style.box_model.margin_left = src.box_model.margin_left;
            style.layout.margin_left_auto = src.layout.margin_left_auto;
        }
        "margin-right" => {
            style.box_model.margin_right = src.box_model.margin_right;
            style.layout.margin_right_auto = src.layout.margin_right_auto;
        }
        "margin-inline-start" => copy_margin_side(style, src, inline_sides(style.text.direction).0),
        "margin-inline-end" => copy_margin_side(style, src, inline_sides(style.text.direction).1),
        "margin-block" => {
            style.box_model.margin_top = src.box_model.margin_top;
            style.box_model.margin_bottom = src.box_model.margin_bottom;
            style.layout.margin_top_auto = src.layout.margin_top_auto;
            style.layout.margin_bottom_auto = src.layout.margin_bottom_auto;
        }
        "margin-inline" => {
            let (start, end) = inline_sides(style.text.direction);
            copy_margin_side(style, src, start);
            copy_margin_side(style, src, end);
        }
        "margin" => {
            style.box_model.margin_top = src.box_model.margin_top;
            style.box_model.margin_bottom = src.box_model.margin_bottom;
            style.box_model.margin_left = src.box_model.margin_left;
            style.box_model.margin_right = src.box_model.margin_right;
            style.layout.margin_top_auto = src.layout.margin_top_auto;
            style.layout.margin_right_auto = src.layout.margin_right_auto;
            style.layout.margin_bottom_auto = src.layout.margin_bottom_auto;
            style.layout.margin_left_auto = src.layout.margin_left_auto;
        }
        "padding-top" => style.box_model.padding_top = src.box_model.padding_top,
        "padding-bottom" => style.box_model.padding_bottom = src.box_model.padding_bottom,
        "padding-left" => style.box_model.padding_left = src.box_model.padding_left,
        "padding-right" => style.box_model.padding_right = src.box_model.padding_right,
        "padding-block-start" => style.box_model.padding_top = src.box_model.padding_top,
        "padding-block-end" => style.box_model.padding_bottom = src.box_model.padding_bottom,
        "padding-inline-start" => {
            copy_padding_side(style, src, inline_sides(style.text.direction).0)
        }
        "padding-inline-end" => copy_padding_side(style, src, inline_sides(style.text.direction).1),
        "padding-block" => {
            style.box_model.padding_top = src.box_model.padding_top;
            style.box_model.padding_bottom = src.box_model.padding_bottom;
        }
        "padding-inline" => {
            let (start, end) = inline_sides(style.text.direction);
            copy_padding_side(style, src, start);
            copy_padding_side(style, src, end);
        }
        "padding" => {
            style.box_model.padding_top = src.box_model.padding_top;
            style.box_model.padding_bottom = src.box_model.padding_bottom;
            style.box_model.padding_left = src.box_model.padding_left;
            style.box_model.padding_right = src.box_model.padding_right;
        }
        "border"
        | "border-width"
        | "border-style"
        | "border-color"
        | "border-top"
        | "border-right"
        | "border-bottom"
        | "border-left"
        | "border-top-width"
        | "border-right-width"
        | "border-bottom-width"
        | "border-left-width"
        | "border-top-style"
        | "border-right-style"
        | "border-bottom-style"
        | "border-left-style"
        | "border-top-color"
        | "border-right-color"
        | "border-bottom-color"
        | "border-left-color" => {
            // `inherit` copies the parent's computed border color.  Do not
            // carry the parent's internal `currentColor` dependency into the
            // child, where it would incorrectly resolve against the child's
            // own `color`.  Initial/unset values still need that dependency.
            apply_border_keyword(style, name, src, keyword != "inherit");
        }
        "border-radius" => style.radii = src.radii,
        "border-top-left-radius" => style.radii.top_left = src.radii.top_left,
        "border-top-right-radius" => style.radii.top_right = src.radii.top_right,
        "border-bottom-right-radius" => style.radii.bottom_right = src.radii.bottom_right,
        "border-bottom-left-radius" => style.radii.bottom_left = src.radii.bottom_left,
        "border-start-start-radius" => copy_corner_radius(
            style,
            src,
            if style.text.direction == TextDirection::Ltr {
                0
            } else {
                1
            },
        ),
        "border-start-end-radius" => copy_corner_radius(
            style,
            src,
            if style.text.direction == TextDirection::Ltr {
                1
            } else {
                0
            },
        ),
        "border-end-start-radius" => copy_corner_radius(
            style,
            src,
            if style.text.direction == TextDirection::Ltr {
                3
            } else {
                2
            },
        ),
        "border-end-end-radius" => copy_corner_radius(
            style,
            src,
            if style.text.direction == TextDirection::Ltr {
                2
            } else {
                3
            },
        ),
        "border-block"
        | "border-block-width"
        | "border-block-style"
        | "border-block-color"
        | "border-block-start"
        | "border-block-start-width"
        | "border-block-start-style"
        | "border-block-start-color"
        | "border-block-end"
        | "border-block-end-width"
        | "border-block-end-style"
        | "border-block-end-color"
        | "border-inline"
        | "border-inline-width"
        | "border-inline-style"
        | "border-inline-color"
        | "border-inline-start"
        | "border-inline-start-width"
        | "border-inline-start-style"
        | "border-inline-start-color"
        | "border-inline-end"
        | "border-inline-end-width"
        | "border-inline-end-style"
        | "border-inline-end-color" => {
            apply_logical_border_keyword(style, name, src, keyword != "inherit")
        }
        _ => return false,
    }
    true
}

pub(super) fn apply_all_from_basis(
    style: &mut WorkingStyle,
    basis: &WorkingStyle,
    phase: CascadePhase,
) {
    match phase {
        CascadePhase::Prerequisites => {
            style.font.font_size = basis.font.font_size;
            style.font.font_size_x_height_px = basis.font.font_size_x_height_px;
            style.font.font_size_ch_advance_px = basis.font.font_size_ch_advance_px;
            style.font.font_size_cap_height_px = basis.font.font_size_cap_height_px;
            style.font.font_size_root_ch = basis.font.font_size_root_ch;
            style.font.font_size_root_cap_height = basis.font.font_size_root_cap_height;
            style.font.font_size_root_line_height = basis.font.font_size_root_line_height;
            style.text.color = basis.text.color;
            style.text.line_height = basis.text.line_height;
            style.text.line_height_x_height_px = basis.text.line_height_x_height_px;
            style.text.line_height_number = basis.text.line_height_number;
            style.text.line_height_normal = basis.text.line_height_normal;
            style.line_height_spec = basis.line_height_spec.clone();
        }
        CascadePhase::Remaining => {
            // `all` excludes direction and unicode-bidi. Preserve direction,
            // plus the document language carried beside CSS text properties.
            // Preserve values finalized in the prerequisite pass so a later
            // declaration in the same layer remains the winner.
            let direction = style.text.direction;
            let language = style.text.language;
            let color = style.text.color;
            let line_height = style.text.line_height;
            let line_height_x_height_px = style.text.line_height_x_height_px;
            let line_height_number = style.text.line_height_number;
            let line_height_normal = style.text.line_height_normal;
            let line_height_spec = style.line_height_spec.clone();
            let font_size = style.font.font_size;
            let x_height = style.font.font_size_x_height_px;
            let ch_advance = style.font.font_size_ch_advance_px;
            let cap_height = style.font.font_size_cap_height_px;
            let root_ch = style.font.font_size_root_ch;
            let root_cap_height = style.font.font_size_root_cap_height;
            let root_line_height = style.font.font_size_root_line_height;
            *style = basis.clone();
            style.text.direction = direction;
            style.text.language = language;
            style.text.color = color;
            style.text.line_height = line_height;
            style.text.line_height_x_height_px = line_height_x_height_px;
            style.text.line_height_number = line_height_number;
            style.text.line_height_normal = line_height_normal;
            style.line_height_spec = line_height_spec;
            style.font.font_size = font_size;
            style.font.font_size_x_height_px = x_height;
            style.font.font_size_ch_advance_px = ch_advance;
            style.font.font_size_cap_height_px = cap_height;
            style.font.font_size_root_ch = root_ch;
            style.font.font_size_root_cap_height = root_cap_height;
            style.font.font_size_root_line_height = root_line_height;
        }
    }
}

fn apply_border_keyword(
    style: &mut WorkingStyle,
    name: &str,
    src: &ParentStyle,
    preserve_current_color_dependency: bool,
) {
    let side_shorthand = matches!(
        name,
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left"
    );
    let widths = side_shorthand || name == "border-width" || name.ends_with("-width");
    let styles = side_shorthand || name == "border-style" || name.ends_with("-style");
    let colors = side_shorthand || name == "border-color" || name.ends_with("-color");
    let sides: &[usize] = match name {
        n if n.starts_with("border-top") => &[0],
        n if n.starts_with("border-right") => &[1],
        n if n.starts_with("border-bottom") => &[2],
        n if n.starts_with("border-left") => &[3],
        _ => &[0, 1, 2, 3],
    };
    for &side in sides {
        if widths {
            let value = [
                src.border.border_top_width,
                src.border.border_right_width,
                src.border.border_bottom_width,
                src.border.border_left_width,
            ][side];
            *[
                &mut style.border.border_top_width,
                &mut style.border.border_right_width,
                &mut style.border.border_bottom_width,
                &mut style.border.border_left_width,
            ][side] = value;
        }
        if styles {
            let value = [
                src.border.border_top_style,
                src.border.border_right_style,
                src.border.border_bottom_style,
                src.border.border_left_style,
            ][side];
            *[
                &mut style.border.border_top_style,
                &mut style.border.border_right_style,
                &mut style.border.border_bottom_style,
                &mut style.border.border_left_style,
            ][side] = value;
        }
        if colors {
            let value = [
                src.border.border_top_color,
                src.border.border_right_color,
                src.border.border_bottom_color,
                src.border.border_left_color,
            ][side];
            set_border_color(style, side, value);
            let inherited_dependency = if preserve_current_color_dependency {
                src.border.current_color_sides & (1 << side)
            } else {
                0
            };
            style.border.current_color_sides =
                (style.border.current_color_sides & !(1 << side)) | inherited_dependency;
        }
    }
}

fn apply_logical_border_keyword(
    style: &mut WorkingStyle,
    name: &str,
    src: &ParentStyle,
    preserve_current_color_dependency: bool,
) {
    let side_shorthand = matches!(
        name,
        "border-block"
            | "border-block-start"
            | "border-block-end"
            | "border-inline"
            | "border-inline-start"
            | "border-inline-end"
    );
    let widths = side_shorthand || name.ends_with("-width");
    let styles = side_shorthand || name.ends_with("-style");
    let colors = side_shorthand || name.ends_with("-color");
    let inline = inline_sides(style.text.direction);
    let sides: &[usize] = if name.starts_with("border-block-start") {
        &[0]
    } else if name.starts_with("border-block-end") {
        &[2]
    } else if name.starts_with("border-block") {
        &[0, 2]
    } else if name.starts_with("border-inline-start") {
        std::slice::from_ref(&inline.0)
    } else if name.starts_with("border-inline-end") {
        std::slice::from_ref(&inline.1)
    } else {
        // Inline shorthand updates both sides. The order is immaterial because
        // CSS-wide keywords copy one already-computed value per physical side.
        &[1, 3]
    };

    for &side in sides {
        if widths {
            let value = [
                src.border.border_top_width,
                src.border.border_right_width,
                src.border.border_bottom_width,
                src.border.border_left_width,
            ][side];
            set_border_width(style, side, value);
        }
        if styles {
            let value = [
                src.border.border_top_style,
                src.border.border_right_style,
                src.border.border_bottom_style,
                src.border.border_left_style,
            ][side];
            set_border_style(style, side, value);
        }
        if colors {
            let value = [
                src.border.border_top_color,
                src.border.border_right_color,
                src.border.border_bottom_color,
                src.border.border_left_color,
            ][side];
            set_border_color(style, side, value);
            let inherited_dependency = if preserve_current_color_dependency {
                src.border.current_color_sides & (1 << side)
            } else {
                0
            };
            style.border.current_color_sides =
                (style.border.current_color_sides & !(1 << side)) | inherited_dependency;
        }
    }
}
