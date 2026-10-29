//! Supported property identities, dependency phases, and computed field ownership.
//! The catalogue describes renderer semantics; Lightning CSS owns CSS parsing.
macro_rules! property_catalogue {
    ($define:ident) => {
        $define! {
            All => "all", false, Remaining, [], shorthand;
            FontSize => "font-size", true, Prerequisites, [
                font.font_size,
                font.font_size_x_height_px,
                font.font_size_ch_advance_px,
                font.font_size_cap_height_px,
                font.font_size_root_ch,
                font.font_size_root_cap_height,
                font.font_size_root_line_height,
            ], plain;
            FontWeight => "font-weight", true, Remaining, [font.font_weight], plain;
            FontStyle => "font-style", true, Remaining, [font.font_style], plain;
            FontFamily => "font-family", true, Remaining, [font.font_family], plain;
            Font => "font", true, Remaining, [], shorthand;
            LineHeight => "line-height", true, Prerequisites, [
                text.line_height,
                text.line_height_x_height_px,
                text.line_height_number,
                text.line_height_normal,
                line_height_spec,
            ], plain;
            LetterSpacing => "letter-spacing", true, Remaining, [text.letter_spacing], plain;
            WordSpacing => "word-spacing", true, Remaining, [text.word_spacing], plain;
            TabSize => "tab-size", true, Remaining, [text.tab_size], plain;
            TextAlign => "text-align", true, Remaining, [text.text_align, text.text_align_logical], text_align;
            TextAlignLast => "text-align-last", true, Remaining, [
                text.text_align_last,
                text.text_align_last_logical,
                text.text_align_last_explicit,
            ], plain;
            TextIndent => "text-indent", true, Remaining, [text.text_indent, text.text_indent_hanging, text.text_indent_each_line], plain;
            TextOverflow => "text-overflow", false, Remaining, [box_model.text_overflow], plain;
            TextTransform => "text-transform", true, Remaining, [text.text_transform], plain;
            Direction => "direction", true, Prerequisites, [text.direction], direction;
            UnicodeBidi => "unicode-bidi", false, Remaining, [box_model.unicode_bidi], plain;
            FontVariantCaps => "font-variant-caps", true, Remaining, [font.font_variant_caps_features], plain;
            TextDecorationLine => "text-decoration-line", false, Remaining, [background.text_decoration.lines], plain;
            TextDecoration => "text-decoration", false, Remaining, [], shorthand;
            TextDecorationColor => "text-decoration-color", false, Remaining, [background.text_decoration.color], plain;
            TextDecorationStyle => "text-decoration-style", false, Remaining, [background.text_decoration.style], plain;
            TextDecorationThickness => "text-decoration-thickness", false, Remaining, [background.text_decoration.thickness], plain;
            VerticalAlign => "vertical-align", false, Remaining, [box_model.vertical_align], plain;
            WhiteSpace => "white-space", true, Remaining, [text.white_space], plain;
            ListStyleType => "list-style-type", true, Remaining, [text.list_style_type], plain;
            ListStylePosition => "list-style-position", true, Remaining, [text.list_style_position], plain;
            ListStyleImage => "list-style-image", true, Remaining, [text.list_style_image], plain;
            ListStyle => "list-style", true, Remaining, [], shorthand;
            Color => "color", true, Prerequisites, [text.color], plain;
            BackgroundColor => "background-color", false, Remaining, [
                background.background_color,
                background.background_color_current_color,
            ], plain;
            Background => "background", false, Remaining, [], shorthand;
            Width => "width", false, Remaining, [box_model.width], plain;
            Height => "height", false, Remaining, [box_model.height], plain;
            AspectRatio => "aspect-ratio", false, Remaining, [box_model.aspect_ratio], plain;
            Visibility => "visibility", true, Remaining, [text.visibility], plain;
            MinWidth => "min-width", false, Remaining, [box_model.min_width], plain;
            MinHeight => "min-height", false, Remaining, [box_model.min_height], plain;
            MaxWidth => "max-width", false, Remaining, [box_model.max_width], plain;
            MaxHeight => "max-height", false, Remaining, [box_model.max_height], plain;
            InlineSize => "inline-size", false, Remaining, [box_model.width], plain;
            BlockSize => "block-size", false, Remaining, [box_model.height], plain;
            MinInlineSize => "min-inline-size", false, Remaining, [box_model.min_width], plain;
            MinBlockSize => "min-block-size", false, Remaining, [box_model.min_height], plain;
            MaxInlineSize => "max-inline-size", false, Remaining, [box_model.max_width], plain;
            MaxBlockSize => "max-block-size", false, Remaining, [box_model.max_height], plain;
            MarginTop => "margin-top", false, Remaining, [box_model.margin_top, layout.margin_top_auto], plain;
            MarginBottom => "margin-bottom", false, Remaining, [box_model.margin_bottom, layout.margin_bottom_auto], plain;
            MarginLeft => "margin-left", false, Remaining, [box_model.margin_left, layout.margin_left_auto], plain;
            MarginRight => "margin-right", false, Remaining, [box_model.margin_right, layout.margin_right_auto], plain;
            Margin => "margin", false, Remaining, [], shorthand;
            MarginBlock => "margin-block", false, Remaining, [], shorthand;
            MarginBlockStart => "margin-block-start", false, Remaining, [box_model.margin_top, layout.margin_top_auto], plain;
            MarginBlockEnd => "margin-block-end", false, Remaining, [box_model.margin_bottom, layout.margin_bottom_auto], plain;
            MarginInline => "margin-inline", false, Remaining, [], shorthand;
            MarginInlineStart => "margin-inline-start", false, Remaining, [], logical(MarginLeft, MarginRight);
            MarginInlineEnd => "margin-inline-end", false, Remaining, [], logical(MarginRight, MarginLeft);
            PaddingTop => "padding-top", false, Remaining, [box_model.padding_top], plain;
            PaddingBottom => "padding-bottom", false, Remaining, [box_model.padding_bottom], plain;
            PaddingLeft => "padding-left", false, Remaining, [box_model.padding_left], plain;
            PaddingRight => "padding-right", false, Remaining, [box_model.padding_right], plain;
            Padding => "padding", false, Remaining, [], shorthand;
            PaddingBlock => "padding-block", false, Remaining, [], shorthand;
            PaddingBlockStart => "padding-block-start", false, Remaining, [box_model.padding_top], plain;
            PaddingBlockEnd => "padding-block-end", false, Remaining, [box_model.padding_bottom], plain;
            PaddingInline => "padding-inline", false, Remaining, [], shorthand;
            PaddingInlineStart => "padding-inline-start", false, Remaining, [], logical(PaddingLeft, PaddingRight);
            PaddingInlineEnd => "padding-inline-end", false, Remaining, [], logical(PaddingRight, PaddingLeft);
            BorderWidth => "border-width", false, Remaining, [], shorthand;
            BorderTopWidth => "border-top-width", false, Remaining, [border.border_top_width], plain;
            BorderRightWidth => "border-right-width", false, Remaining, [border.border_right_width], plain;
            BorderBottomWidth => "border-bottom-width", false, Remaining, [border.border_bottom_width], plain;
            BorderLeftWidth => "border-left-width", false, Remaining, [border.border_left_width], plain;
            BorderBlockStartWidth => "border-block-start-width", false, Remaining, [border.border_top_width], plain;
            BorderBlockEndWidth => "border-block-end-width", false, Remaining, [border.border_bottom_width], plain;
            BorderInlineStartWidth => "border-inline-start-width", false, Remaining, [], logical(BorderLeftWidth, BorderRightWidth);
            BorderInlineEndWidth => "border-inline-end-width", false, Remaining, [], logical(BorderRightWidth, BorderLeftWidth);
            BorderBlockWidth => "border-block-width", false, Remaining, [], shorthand;
            BorderInlineWidth => "border-inline-width", false, Remaining, [], shorthand;
            BorderColor => "border-color", false, Remaining, [], shorthand;
            BorderTopColor => "border-top-color", false, Remaining, [border.border_top_color], border_color(0);
            BorderRightColor => "border-right-color", false, Remaining, [border.border_right_color], border_color(1);
            BorderBottomColor => "border-bottom-color", false, Remaining, [border.border_bottom_color], border_color(2);
            BorderLeftColor => "border-left-color", false, Remaining, [border.border_left_color], border_color(3);
            BorderBlockStartColor => "border-block-start-color", false, Remaining, [border.border_top_color], border_color(0);
            BorderBlockEndColor => "border-block-end-color", false, Remaining, [border.border_bottom_color], border_color(2);
            BorderInlineStartColor => "border-inline-start-color", false, Remaining, [], logical(BorderLeftColor, BorderRightColor);
            BorderInlineEndColor => "border-inline-end-color", false, Remaining, [], logical(BorderRightColor, BorderLeftColor);
            BorderBlockColor => "border-block-color", false, Remaining, [], shorthand;
            BorderInlineColor => "border-inline-color", false, Remaining, [], shorthand;
            BorderStyle => "border-style", false, Remaining, [], shorthand;
            BorderTopStyle => "border-top-style", false, Remaining, [border.border_top_style], plain;
            BorderRightStyle => "border-right-style", false, Remaining, [border.border_right_style], plain;
            BorderBottomStyle => "border-bottom-style", false, Remaining, [border.border_bottom_style], plain;
            BorderLeftStyle => "border-left-style", false, Remaining, [border.border_left_style], plain;
            BorderBlockStartStyle => "border-block-start-style", false, Remaining, [border.border_top_style], plain;
            BorderBlockEndStyle => "border-block-end-style", false, Remaining, [border.border_bottom_style], plain;
            BorderInlineStartStyle => "border-inline-start-style", false, Remaining, [], logical(BorderLeftStyle, BorderRightStyle);
            BorderInlineEndStyle => "border-inline-end-style", false, Remaining, [], logical(BorderRightStyle, BorderLeftStyle);
            BorderBlockStyle => "border-block-style", false, Remaining, [], shorthand;
            BorderInlineStyle => "border-inline-style", false, Remaining, [], shorthand;
            Border => "border", false, Remaining, [], shorthand;
            BorderTop => "border-top", false, Remaining, [], shorthand;
            BorderBottom => "border-bottom", false, Remaining, [], shorthand;
            BorderLeft => "border-left", false, Remaining, [], shorthand;
            BorderRight => "border-right", false, Remaining, [], shorthand;
            BorderBlock => "border-block", false, Remaining, [], shorthand;
            BorderBlockStart => "border-block-start", false, Remaining, [], shorthand;
            BorderBlockEnd => "border-block-end", false, Remaining, [], shorthand;
            BorderInline => "border-inline", false, Remaining, [], shorthand;
            BorderInlineStart => "border-inline-start", false, Remaining, [], shorthand;
            BorderInlineEnd => "border-inline-end", false, Remaining, [], shorthand;
            BorderRadius => "border-radius", false, Remaining, [], shorthand;
            BorderTopLeftRadius => "border-top-left-radius", false, Remaining, [radii.top_left], plain;
            BorderTopRightRadius => "border-top-right-radius", false, Remaining, [radii.top_right], plain;
            BorderBottomRightRadius => "border-bottom-right-radius", false, Remaining, [radii.bottom_right], plain;
            BorderBottomLeftRadius => "border-bottom-left-radius", false, Remaining, [radii.bottom_left], plain;
            BorderStartStartRadius => "border-start-start-radius", false, Remaining, [], logical(BorderTopLeftRadius, BorderTopRightRadius);
            BorderStartEndRadius => "border-start-end-radius", false, Remaining, [], logical(BorderTopRightRadius, BorderTopLeftRadius);
            BorderEndStartRadius => "border-end-start-radius", false, Remaining, [
            ], logical(BorderBottomLeftRadius, BorderBottomRightRadius);
            BorderEndEndRadius => "border-end-end-radius", false, Remaining, [], logical(BorderBottomRightRadius, BorderBottomLeftRadius);
            BorderSpacing => "border-spacing", true, Remaining, [
                box_model.border_spacing_horizontal,
                box_model.border_spacing_vertical,
            ], plain;
            Outline => "outline", false, Remaining, [], shorthand;
            OutlineWidth => "outline-width", false, Remaining, [], outline_width;
            OutlineStyle => "outline-style", false, Remaining, [background.outline.style], plain;
            OutlineColor => "outline-color", false, Remaining, [background.outline.color], plain;
            Hyphens => "hyphens", true, Remaining, [text.hyphens], plain;
            WordBreak => "word-break", true, Remaining, [text.word_break], plain;
            LineBreak => "line-break", true, Remaining, [text.line_break], plain;
            OverflowWrap => "overflow-wrap", true, Remaining, [text.overflow_wrap], plain;
            Overflow => "overflow", false, Remaining, [], shorthand;
            OverflowX => "overflow-x", false, Remaining, [box_model.overflow_x], plain;
            OverflowY => "overflow-y", false, Remaining, [box_model.overflow_y], plain;
            BoxSizing => "box-sizing", false, Remaining, [box_model.box_sizing], plain;
            Display => "display", false, Remaining, [box_model.display], plain;
            Position => "position", false, Remaining, [layout.position], plain;
            ZIndex => "z-index", false, Remaining, [layout.z_index], plain;
            Top => "top", false, Remaining, [layout.inset_top], plain;
            Right => "right", false, Remaining, [layout.inset_right], plain;
            Bottom => "bottom", false, Remaining, [layout.inset_bottom], plain;
            Left => "left", false, Remaining, [layout.inset_left], plain;
            Inset => "inset", false, Remaining, [], shorthand;
            FlexDirection => "flex-direction", false, Remaining, [layout.flex_direction], plain;
            FlexWrap => "flex-wrap", false, Remaining, [layout.flex_wrap], plain;
            FlexFlow => "flex-flow", false, Remaining, [], shorthand;
            FlexGrow => "flex-grow", false, Remaining, [layout.flex_grow], plain;
            FlexShrink => "flex-shrink", false, Remaining, [layout.flex_shrink], plain;
            FlexBasis => "flex-basis", false, Remaining, [layout.flex_basis], plain;
            Flex => "flex", false, Remaining, [], shorthand;
            Order => "order", false, Remaining, [layout.order], plain;
            AlignContent => "align-content", false, Remaining, [layout.align_content], plain;
            JustifyContent => "justify-content", false, Remaining, [layout.justify_content], plain;
            AlignItems => "align-items", false, Remaining, [layout.align_items], plain;
            AlignSelf => "align-self", false, Remaining, [layout.align_self], plain;
            JustifyItems => "justify-items", false, Remaining, [layout.justify_items], plain;
            JustifySelf => "justify-self", false, Remaining, [layout.justify_self], plain;
            PlaceContent => "place-content", false, Remaining, [], shorthand;
            PlaceItems => "place-items", false, Remaining, [], shorthand;
            PlaceSelf => "place-self", false, Remaining, [], shorthand;
            RowGap => "row-gap", false, Remaining, [layout.row_gap], plain;
            ColumnGap => "column-gap", false, Remaining, [layout.column_gap], plain;
            Gap => "gap", false, Remaining, [], shorthand;
            GridTemplateRows => "grid-template-rows", false, Remaining, [layout.grid_template_rows, layout.grid_template_row_names], plain;
            GridTemplateColumns => "grid-template-columns", false, Remaining, [
                layout.grid_template_columns,
                layout.grid_template_column_names,
            ], plain;
            GridTemplateAreas => "grid-template-areas", false, Remaining, [
                layout.grid_template_areas,
                layout.grid_template_area_rows,
                layout.grid_template_area_columns,
            ], plain;
            GridAutoRows => "grid-auto-rows", false, Remaining, [layout.grid_auto_rows], plain;
            GridAutoColumns => "grid-auto-columns", false, Remaining, [layout.grid_auto_columns], plain;
            GridAutoFlow => "grid-auto-flow", false, Remaining, [layout.grid_auto_flow], plain;
            GridTemplate => "grid-template", false, Remaining, [], shorthand;
            Grid => "grid", false, Remaining, [], shorthand;
            GridRowStart => "grid-row-start", false, Remaining, [layout.grid_row.start], plain;
            GridRowEnd => "grid-row-end", false, Remaining, [layout.grid_row.end], plain;
            GridColumnStart => "grid-column-start", false, Remaining, [layout.grid_column.start], plain;
            GridColumnEnd => "grid-column-end", false, Remaining, [layout.grid_column.end], plain;
            GridRow => "grid-row", false, Remaining, [], shorthand;
            GridColumn => "grid-column", false, Remaining, [], shorthand;
            GridArea => "grid-area", false, Remaining, [], shorthand;
            Float => "float", false, Remaining, [box_model.float], plain;
            Clear => "clear", false, Remaining, [box_model.clear], plain;
            ObjectFit => "object-fit", false, Remaining, [box_model.object_fit], plain;
            ObjectPosition => "object-position", false, Remaining, [box_model.object_position], plain;
            OutlineOffset => "outline-offset", false, Remaining, [], outline_offset;
            TableLayout => "table-layout", false, Remaining, [box_model.table_layout], plain;
            BorderCollapse => "border-collapse", true, Remaining, [box_model.border_collapse], plain;
            CaptionSide => "caption-side", true, Remaining, [box_model.caption_side], plain;
            EmptyCells => "empty-cells", true, Remaining, [box_model.empty_cells], plain;
            FontKerning => "font-kerning", true, Remaining, [font.font_kerning_features], plain;
            FontFeatureSettings => "font-feature-settings", true, Remaining, [font.font_feature_settings], plain;
            FontVariantNumeric => "font-variant-numeric", true, Remaining, [font.font_variant_numeric_features], plain;
            FontVariantLigatures => "font-variant-ligatures", true, Remaining, [font.font_variant_ligature_features], plain;
            TextBox => "text-box", false, Remaining, [], shorthand;
            TextBoxTrim => "text-box-trim", false, Remaining, [box_model.text_box_trim], plain;
            TextBoxEdge => "text-box-edge", true, Remaining, [text.text_box_edge], plain;
            Contain => "contain", false, Remaining, [box_model.size_containment], plain;
            BreakBefore => "break-before", false, Remaining, [layout.break_before], plain;
            BreakAfter => "break-after", false, Remaining, [layout.break_after], plain;
            BreakInside => "break-inside", false, Remaining, [layout.break_inside], plain;
            Widows => "widows", true, Remaining, [text.widows], plain;
            Orphans => "orphans", true, Remaining, [text.orphans], plain;
            Content => "content", false, Remaining, [generated_content], plain;
            CounterReset => "counter-reset", false, Remaining, [counters.resets], plain;
            CounterIncrement => "counter-increment", false, Remaining, [counters.increments], plain;
            Quotes => "quotes", true, Remaining, [text.quotes], plain;
            BackgroundImage => "background-image", false, Remaining, [background.background_image_present], plain;
        }
    };
}
pub(crate) use property_catalogue;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CascadePhase {
    Prerequisites,
    Remaining,
}

macro_rules! define_property_keys {
    ($($key:ident => $name:literal, $inherited:literal, $phase:ident, [$($field:ident $(.$member:ident)*),* $(,)?], $copy:ident $(($($arg:tt)*))?;)+) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        #[repr(u16)]
        pub(crate) enum PropertyKey { $($key,)+ }
        impl PropertyKey {
            pub(crate) const ALL: &'static [Self] = &[$(Self::$key,)+];
            pub(crate) fn from_name(name: &str) -> Option<Self> {
                Some(cssparser::match_ignore_ascii_case! { name,
                    $($name => Self::$key,)+
                    "word-wrap" => Self::OverflowWrap,
                    "page-break-before" => Self::BreakBefore,
                    "page-break-after" => Self::BreakAfter,
                    "page-break-inside" => Self::BreakInside,
                    "grid-gap" => Self::Gap,
                    "grid-row-gap" => Self::RowGap,
                    "grid-column-gap" => Self::ColumnGap,

                    _ => return None,
                })
            }
            pub(crate) fn inherited(self) -> bool { match self { $(Self::$key => $inherited,)+ } }
            pub(crate) fn phase(self) -> CascadePhase { match self { $(Self::$key => CascadePhase::$phase,)+ } }
            pub(crate) fn is_longhand(self) -> bool { match self { $(Self::$key => property_kind!($copy),)+ } }
            pub(crate) fn affected_by_all(self) -> bool { !matches!(self, Self::Direction | Self::UnicodeBidi) }
        }
    };
}
macro_rules! property_kind {
    (shorthand) => {
        false
    };
    ($other:ident) => {
        true
    };
}
property_catalogue!(define_property_keys);
