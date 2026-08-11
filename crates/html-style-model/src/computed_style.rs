use html_dom::{Document, DocumentLineage, DomNodeId, MemoryUsageReport, NodeRef};
use kurbo::Vec2;
use rustc_data_structures::fx::{FxHashMap, FxHasher};
use std::hash::{Hash, Hasher};
use std::num::{NonZeroI16, NonZeroU16, NonZeroU32};
use std::sync::atomic::{AtomicU32, Ordering};

/// Implement bitwise `PartialEq`/`Eq`/`Hash` for a style sub-struct so it can be
/// deduplicated in `StyleStore`. Float fields are compared and hashed by their
/// bit pattern (exact equality), which is exactly what sharing identical computed
/// styles needs.
macro_rules! impl_style_key {
    ($t:ty { $($f:ident),* } floats { $($ff:ident),* }) => {
        impl PartialEq for $t {
            fn eq(&self, other: &Self) -> bool {
                $( self.$f == other.$f && )* $( self.$ff.to_bits() == other.$ff.to_bits() && )* true
            }
        }
        impl Eq for $t {}
        impl Hash for $t {
            fn hash<H: Hasher>(&self, state: &mut H) {
                $( self.$f.hash(state); )*
                $( state.write_u32(self.$ff.to_bits()); )*
            }
        }
    };
}

// ============================================================================
// Sub-Structs for Servo-inspired Style Splitting
// ============================================================================

/// Computed letter/word spacing.
///
/// CSS percentages remain relative to the used font size of the element where
/// the inherited value is finally used. Lengths are resolved to absolute pixels
/// during style computation, so keeping the two components separate preserves
/// CSS inheritance semantics without exposing parser-specific calc trees to
/// layout.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextSpacing {
    absolute_px: f32,
    font_size_fraction: f32,
}

impl TextSpacing {
    pub const ZERO: Self = Self { absolute_px: 0.0, font_size_fraction: 0.0 };

    pub fn new(absolute_px: f32, font_size_fraction: f32) -> Option<Self> {
        (absolute_px.is_finite() && font_size_fraction.is_finite()).then_some(Self { absolute_px, font_size_fraction })
    }

    pub fn from_px(absolute_px: f32) -> Option<Self> {
        Self::new(absolute_px, 0.0)
    }

    pub fn absolute_px(self) -> f32 {
        self.absolute_px
    }

    pub fn font_size_fraction(self) -> f32 {
        self.font_size_fraction
    }

    pub fn resolve(self, used_font_size: f32) -> f32 {
        let resolved = self.absolute_px as f64 + self.font_size_fraction as f64 * used_font_size as f64;
        resolved.clamp(-(f32::MAX as f64), f32::MAX as f64) as f32
    }

    fn is_finite(self) -> bool {
        self.absolute_px.is_finite() && self.font_size_fraction.is_finite()
    }
}

impl PartialEq for TextSpacing {
    fn eq(&self, other: &Self) -> bool {
        self.absolute_px.to_bits() == other.absolute_px.to_bits() && self.font_size_fraction.to_bits() == other.font_size_fraction.to_bits()
    }
}

impl Eq for TextSpacing {}

impl Hash for TextSpacing {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(self.absolute_px.to_bits());
        state.write_u32(self.font_size_fraction.to_bits());
    }
}

/// Computed `tab-size`, represented without parser-specific units.
///
/// The private fields and checked constructors make negative and non-finite
/// tab intervals unrepresentable at the style/layout boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TabSizeKind {
    Spaces,
    LengthPx,
}

#[derive(Clone, Copy, Debug)]
pub struct TabSize {
    kind: TabSizeKind,
    value: f32,
}

impl TabSize {
    pub const DEFAULT: Self = Self { kind: TabSizeKind::Spaces, value: 8.0 };

    pub fn spaces(value: f32) -> Option<Self> {
        (value.is_finite() && value >= 0.0).then_some(Self { kind: TabSizeKind::Spaces, value })
    }

    pub fn length_px(value: f32) -> Option<Self> {
        (value.is_finite() && value >= 0.0).then_some(Self { kind: TabSizeKind::LengthPx, value })
    }

    pub fn kind(self) -> TabSizeKind {
        self.kind
    }

    pub fn value(self) -> f32 {
        self.value
    }
}

impl PartialEq for TabSize {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.value.to_bits() == other.value.to_bits()
    }
}

impl Eq for TabSize {}

impl Hash for TabSize {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.kind.hash(state);
        state.write_u32(self.value.to_bits());
    }
}

/// Font properties (inherited)
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OpenTypeFeature {
    tag: [u8; 4],
    value: u32,
}

impl OpenTypeFeature {
    pub const fn new(tag: [u8; 4], value: u32) -> Self {
        Self { tag, value }
    }

    pub const fn tag(self) -> [u8; 4] {
        self.tag
    }

    pub const fn value(self) -> u32 {
        self.value
    }
}

#[derive(Clone, Debug)]
pub struct Font {
    pub font_size: f32,
    /// Deferred selected-face contributions for a linear `font-size: calc()`.
    /// Each value is already scaled by the inherited font size; shaping only
    /// supplies the dimensionless x-height/ZERO-advance ratios.
    pub font_size_x_height_px: f32,
    pub font_size_ch_advance_px: f32,
    pub font_size_cap_height_px: f32,
    /// Deferred root-font contributions for `rch`, `rcap`, and `rlh`. Unlike `ch`,
    /// these are resolved against the root element's selected face and used
    /// line height, not the face selected for this style.
    pub font_size_root_ch: f32,
    pub font_size_root_cap_height: f32,
    pub font_size_root_line_height: f32,
    pub font_weight: u16,
    pub font_style: FontStyle,
    pub font_family: Option<StyleStringId>,
    /// Features selected by each CSS longhand are retained separately because
    /// `font-feature-settings` overrides the higher-level variant controls for
    /// duplicate tags regardless of declaration order.
    pub font_variant_caps_features: Vec<OpenTypeFeature>,
    pub font_variant_numeric_features: Vec<OpenTypeFeature>,
    pub font_variant_ligature_features: Vec<OpenTypeFeature>,
    pub font_kerning_features: Vec<OpenTypeFeature>,
    pub font_feature_settings: Vec<OpenTypeFeature>,
}

impl Font {
    fn resolved_font_size(&self, metrics: FontRelativeRatios, root: RootRelativeLengths) -> f32 {
        (self.font_size
            + self.font_size_x_height_px * metrics.x_height
            + self.font_size_ch_advance_px * metrics.ch_advance
            + self.font_size_cap_height_px * metrics.cap_height
            + self.font_size_root_ch * root.ch_px
            + self.font_size_root_cap_height * root.cap_height_px
            + self.font_size_root_line_height * root.line_height_px)
            .max(0.0)
    }

    pub fn open_type_features(&self) -> Vec<OpenTypeFeature> {
        let mut merged = Vec::new();
        for feature in self.font_variant_caps_features.iter().chain(&self.font_variant_numeric_features).chain(&self.font_variant_ligature_features).chain(&self.font_kerning_features).chain(&self.font_feature_settings).copied() {
            if let Some(index) = merged.iter().position(|existing: &OpenTypeFeature| existing.tag == feature.tag) {
                merged[index] = feature;
            } else {
                merged.push(feature);
            }
        }
        merged
    }
}

/// Text properties (inherited)
#[derive(Clone, Debug)]
pub struct InheritedText {
    pub color: u32,
    pub visibility: Visibility,
    /// Inherited BCP 47 language tag sourced from `lang`/`xml:lang`.
    pub language: Option<StyleStringId>,
    /// Computed inline base direction. Logical box properties are resolved to
    /// physical sides in the style stage, so layout never has to interpret
    /// parser-owned logical-property values.
    pub direction: TextDirection,
    pub line_height: f32,
    /// Deferred `ex` contribution, already multiplied by the computed font
    /// size. Used-style construction multiplies this by the selected face's
    /// dimensionless x-height ratio.
    pub line_height_x_height_px: f32,
    /// Distinguishes CSS `normal` from an authored zero length/number. Both
    /// resolve to `0.0` in `line_height`, but layout must only synthesize font
    /// leading for `normal`.
    pub line_height_normal: bool,
    /// Unitless `line-height` multiplier (0.0 = none). Numbers inherit as
    /// numbers, so descendants recompute `line_height` against their own font
    /// size; lengths/percentages inherit only the resolved `line_height` px.
    pub line_height_number: f32,
    pub letter_spacing: TextSpacing,
    pub word_spacing: TextSpacing,
    pub tab_size: TabSize,
    pub text_align: TextAlign,
    /// Retains the logical keyword behind the resolved physical alignment so
    /// an inherited `start`/`end` can be re-resolved when a descendant changes
    /// `direction`.
    pub text_align_logical: LogicalTextAlign,
    pub text_align_last: TextAlign,
    pub text_align_last_logical: LogicalTextAlign,
    /// True when `text-align-last` was explicitly set to a non-`auto` value, so
    /// a later `text-align` no longer mirrors itself into `text_align_last`.
    pub text_align_last_explicit: bool,
    pub text_indent: LengthPct,
    pub text_indent_hanging: bool,
    pub text_indent_each_line: bool,
    pub white_space: WhiteSpace,
    pub hyphens: Hyphens,
    pub word_break: WordBreak,
    pub overflow_wrap: OverflowWrap,
    /// Font metrics selected for `text-box-trim`. The edge is inherited even
    /// though the trimming operation itself is a reset property.
    pub text_box_edge: TextBoxEdge,
    /// Minimum lines retained at the top and bottom of a fragmentainer.
    pub widows: u8,
    pub orphans: u8,
    pub text_transform: TextTransform,
    pub quotes: QuoteStyle,
    pub list_style_type: ListStyleType,
    pub list_style_position: ListStylePosition,
    /// Interned URL string of `list-style-image` (`None` for `none`). Resolved to
    /// an image resource when the marker box is built.
    pub list_style_image: Option<StyleStringId>,
}

/// Whether an element's generated boxes are painted. Unlike `display: none`,
/// hidden boxes still participate fully in layout, and descendants may opt
/// back into painting with `visibility: visible`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
}

/// Computed `quotes`: automatic language quotes, suppression, or an authored
/// sequence of open/close pairs encoded in the style string table.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum QuoteStyle {
    #[default]
    Auto,
    None,
    Pairs(StyleStringId),
}

/// Computed `aspect-ratio`, reduced to the positive finite ratio used by
/// layout while retaining whether replaced elements prefer their intrinsic
/// ratio (`auto`). Degenerate ratios are valid CSS but provide no usable ratio.
#[derive(Clone, Copy, Debug)]
pub struct AspectRatio {
    preferred: f32,
    auto: bool,
}

impl AspectRatio {
    pub const AUTO: Self = Self { preferred: 0.0, auto: true };

    pub fn new(auto: bool, components: Option<(f32, f32)>) -> Option<Self> {
        let preferred = match components {
            None => 0.0,
            Some((width, height)) if width.is_finite() && height.is_finite() && width >= 0.0 && height >= 0.0 => {
                let ratio = width / height;
                if ratio.is_finite() && ratio > 0.0 { ratio } else { 0.0 }
            }
            Some(_) => return None,
        };
        Some(Self { preferred, auto })
    }

    pub fn preferred(self) -> Option<f32> {
        (self.preferred > 0.0).then_some(self.preferred)
    }

    pub fn uses_intrinsic(self) -> bool {
        self.auto
    }
}

impl Default for AspectRatio {
    fn default() -> Self {
        Self::AUTO
    }
}

impl PartialEq for AspectRatio {
    fn eq(&self, other: &Self) -> bool {
        self.auto == other.auto && self.preferred.to_bits() == other.preferred.to_bits()
    }
}

impl Eq for AspectRatio {}

impl Hash for AspectRatio {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.auto.hash(state);
        state.write_u32(self.preferred.to_bits());
    }
}

/// Box model properties (reset)
#[derive(Clone, Debug)]
pub struct BoxModel {
    // Index of the separately interned flex/grid reset group. Keeping this on
    // the deduplicated box style, rather than on every `StyleIndices` handle,
    // means ordinary DOM nodes do not grow just because flex/grid is enabled.
    layout_idx: u32,
    pub display: Display,
    pub box_sizing: BoxSizing,
    pub overflow_x: OverflowMode,
    pub overflow_y: OverflowMode,
    pub text_overflow: TextOverflow,
    pub text_box_trim: TextBoxTrim,
    pub size_containment: bool,
    pub vertical_align: VerticalAlignValue,
    pub float: Float,
    pub clear: Clear,
    pub table_layout: TableLayoutMode,
    pub border_collapse: BorderCollapseMode,
    pub border_spacing_horizontal: f32,
    pub border_spacing_vertical: f32,
    pub empty_cells: EmptyCellsMode,
    pub caption_side: CaptionSide,
    pub width: PreferredSize,
    pub height: PreferredSize,
    pub min_width: PreferredSize,
    pub min_height: PreferredSize,
    pub max_width: PreferredSize,
    pub max_height: PreferredSize,
    pub aspect_ratio: AspectRatio,
    pub object_fit: ObjectFit,
    pub object_position: ObjectPosition,
    pub margin_top: LengthPct,
    pub margin_bottom: LengthPct,
    pub margin_left: LengthPct,
    pub margin_right: LengthPct,
    pub padding_top: LengthPct,
    pub padding_bottom: LengthPct,
    pub padding_left: LengthPct,
    pub padding_right: LengthPct,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ObjectFit {
    #[default]
    Fill,
    Contain,
    Cover,
    CoverScaleDown,
    None,
    ScaleDown,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ObjectPositionOrigin {
    Start,
    End,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ObjectPositionAxis {
    pub origin: ObjectPositionOrigin,
    pub offset: LengthPct,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ObjectPosition {
    pub x: ObjectPositionAxis,
    pub y: ObjectPositionAxis,
}

impl Default for ObjectPosition {
    fn default() -> Self {
        let center = ObjectPositionAxis { origin: ObjectPositionOrigin::Start, offset: LengthPct::Pct(0.5) };
        Self { x: center, y: center }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextBoxTrim {
    #[default]
    None,
    Start,
    End,
    Both,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextBoxOverEdge {
    #[default]
    Text,
    Cap,
    Ex,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextBoxUnderEdge {
    #[default]
    Text,
    Alphabetic,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextBoxEdge {
    pub over: TextBoxOverEdge,
    pub under: TextBoxUnderEdge,
}

/// Reset properties consumed by flexbox and grid layout. This is interned in a
/// separate style table so documents that only use normal flow share one small
/// default entry rather than carrying grid track vectors in every box style.
#[derive(Clone, Debug)]
pub struct LayoutStyle {
    pub margin_top_auto: bool,
    pub margin_right_auto: bool,
    pub margin_bottom_auto: bool,
    pub margin_left_auto: bool,
    pub position: PositionMode,
    pub z_index: Option<i32>,
    pub inset_top: Option<LengthPct>,
    pub inset_right: Option<LengthPct>,
    pub inset_bottom: Option<LengthPct>,
    pub inset_left: Option<LengthPct>,
    pub break_before: BreakBetween,
    pub break_after: BreakBetween,
    pub break_inside: BreakInside,
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: PreferredSize,
    pub order: i32,
    pub align_content: ContentAlignment,
    pub justify_content: ContentAlignment,
    pub align_items: ItemAlignment,
    pub align_self: ItemAlignment,
    pub justify_items: ItemAlignment,
    pub justify_self: ItemAlignment,
    pub row_gap: LengthPct,
    pub column_gap: LengthPct,
    pub grid_template_rows: Vec<GridTemplateTrack>,
    pub grid_template_columns: Vec<GridTemplateTrack>,
    pub grid_template_row_names: Vec<Vec<StyleStringId>>,
    pub grid_template_column_names: Vec<Vec<StyleStringId>>,
    pub grid_template_areas: Vec<GridTemplateArea>,
    pub grid_template_area_rows: u16,
    pub grid_template_area_columns: u16,
    pub grid_auto_rows: Vec<GridTrackSize>,
    pub grid_auto_columns: Vec<GridTrackSize>,
    pub grid_auto_flow: GridAutoFlow,
    pub grid_row: GridPlacementRange,
    pub grid_column: GridPlacementRange,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum BreakBetween {
    #[default]
    Auto,
    Avoid,
    Column,
    Page,
}

impl BreakBetween {
    pub fn is_forced(self) -> bool {
        matches!(self, Self::Column | Self::Page)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum BreakInside {
    #[default]
    Auto,
    Avoid,
}

/// Border properties (reset)
#[derive(Clone, Debug)]
pub struct Border {
    radii_idx: u32,
    /// Bit 0..3 preserve `currentColor` for top, right, bottom, and left.
    /// The resolved RGBA is cached in the color fields for painting, while
    /// this specified/computed distinction remains available to `inherit`.
    pub current_color_sides: u8,
    pub border_top_width: FontRelativeLength,
    pub border_right_width: FontRelativeLength,
    pub border_bottom_width: FontRelativeLength,
    pub border_left_width: FontRelativeLength,
    pub border_top_color: u32,
    pub border_right_color: u32,
    pub border_bottom_color: u32,
    pub border_left_color: u32,
    pub border_top_style: BorderStyle,
    pub border_right_style: BorderStyle,
    pub border_bottom_style: BorderStyle,
    pub border_left_style: BorderStyle,
}

/// One elliptical CSS corner radius in computed-value form.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CornerRadius {
    pub x: LengthPct,
    pub y: LengthPct,
}

/// Computed `border-radius`, kept parser-independent and resolved only once the
/// border box dimensions are known.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct BorderRadii {
    pub top_left: CornerRadius,
    pub top_right: CornerRadius,
    pub bottom_right: CornerRadius,
    pub bottom_left: CornerRadius,
}

/// Used corner radii after percentages and CSS overlap scaling are resolved.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UsedBorderRadii {
    pub top_left: (f32, f32),
    pub top_right: (f32, f32),
    pub bottom_right: (f32, f32),
    pub bottom_left: (f32, f32),
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct UsedCornerRadius {
    pub x: UsedLengthPct,
    pub y: UsedLengthPct,
}

/// Border radii after font-relative resolution but before percentages can be
/// resolved against the final border box.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct UsedBorderRadiiSpec {
    pub top_left: UsedCornerRadius,
    pub top_right: UsedCornerRadius,
    pub bottom_right: UsedCornerRadius,
    pub bottom_left: UsedCornerRadius,
}

impl BorderRadii {
    fn validate(&self) -> Result<(), ComputedStyleValueError> {
        for (name, radius) in [("top_left", self.top_left), ("top_right", self.top_right), ("bottom_right", self.bottom_right), ("bottom_left", self.bottom_left)] {
            radius.x.validate("BorderRadii", name, false)?;
            radius.y.validate("BorderRadii", name, false)?;
        }
        Ok(())
    }

    fn resolve_font_relative(self, metrics: FontRelativeRatios) -> UsedBorderRadiiSpec {
        let resolve = |radius: CornerRadius| UsedCornerRadius { x: radius.x.resolve_font_relative(metrics), y: radius.y.resolve_font_relative(metrics) };
        UsedBorderRadiiSpec { top_left: resolve(self.top_left), top_right: resolve(self.top_right), bottom_right: resolve(self.bottom_right), bottom_left: resolve(self.bottom_left) }
    }
}

fn ratio(available: f64, requested: f32) -> f32 {
    if requested > 0.0 { (available.max(0.0) as f32 / requested).min(1.0) } else { 1.0 }
}

impl UsedBorderRadii {
    pub fn is_zero(self) -> bool {
        [self.top_left, self.top_right, self.bottom_right, self.bottom_left].into_iter().all(|(x, y)| x == 0.0 && y == 0.0)
    }

    pub fn map(self, mut f: impl FnMut(f32, f32) -> (f32, f32)) -> Self {
        Self { top_left: f(self.top_left.0, self.top_left.1), top_right: f(self.top_right.0, self.top_right.1), bottom_right: f(self.bottom_right.0, self.bottom_right.1), bottom_left: f(self.bottom_left.0, self.bottom_left.1) }
    }
}

impl UsedBorderRadiiSpec {
    pub fn resolve(self, width: f64, height: f64) -> UsedBorderRadii {
        let resolve = |radius: UsedCornerRadius| (radius.x.resolve(width).max(0.0) as f32, radius.y.resolve(height).max(0.0) as f32);
        let mut used = UsedBorderRadii { top_left: resolve(self.top_left), top_right: resolve(self.top_right), bottom_right: resolve(self.bottom_right), bottom_left: resolve(self.bottom_left) };
        let ratios = [ratio(width, used.top_left.0 + used.top_right.0), ratio(width, used.bottom_left.0 + used.bottom_right.0), ratio(height, used.top_left.1 + used.bottom_left.1), ratio(height, used.top_right.1 + used.bottom_right.1)];
        let scale = ratios.into_iter().fold(1.0_f32, f32::min);
        if scale < 1.0 {
            for radius in [&mut used.top_left, &mut used.top_right, &mut used.bottom_right, &mut used.bottom_left] {
                radius.0 *= scale;
                radius.1 *= scale;
            }
        }
        used
    }
}

/// Lines established by `text-decoration-line`. The representation is closed:
/// downstream stages cannot manufacture parser-only or unsupported flags.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextDecorationLines(u8);

impl TextDecorationLines {
    const UNDERLINE: u8 = 1;
    const OVERLINE: u8 = 2;
    const LINE_THROUGH: u8 = 4;

    pub fn new(underline: bool, overline: bool, line_through: bool) -> Self {
        Self(((underline as u8) * Self::UNDERLINE) | ((overline as u8) * Self::OVERLINE) | ((line_through as u8) * Self::LINE_THROUGH))
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn underline(self) -> bool {
        self.0 & Self::UNDERLINE != 0
    }

    pub fn overline(self) -> bool {
        self.0 & Self::OVERLINE != 0
    }

    pub fn line_through(self) -> bool {
        self.0 & Self::LINE_THROUGH != 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextDecorationStyle {
    #[default]
    Solid,
    Double,
    Dotted,
    Dashed,
}

/// A computed decoration color. Keeping `currentColor` symbolic makes color
/// declaration order irrelevant and prevents stale copied colors.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum DecorationColor {
    #[default]
    CurrentColor,
    Rgba(u32),
}

impl DecorationColor {
    pub fn resolve(self, current_color: u32) -> u32 {
        match self {
            Self::CurrentColor => current_color,
            Self::Rgba(color) => color,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TextDecorationThickness {
    #[default]
    Auto,
    FromFont,
    Length(LengthPct),
}

impl TextDecorationThickness {
    pub fn length(value: LengthPct) -> Option<Self> {
        value.validate("TextDecoration", "thickness", false).ok().map(|()| Self::Length(value))
    }

    fn resolve_font_relative(self, metrics: FontRelativeRatios) -> UsedTextDecorationThickness {
        match self {
            Self::Auto => UsedTextDecorationThickness::Auto,
            Self::FromFont => UsedTextDecorationThickness::FromFont,
            Self::Length(value) => UsedTextDecorationThickness::Length(value.resolve_font_relative(metrics)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum UsedTextDecorationThickness {
    #[default]
    Auto,
    FromFont,
    Length(UsedLengthPct),
}

impl UsedTextDecorationThickness {
    pub fn resolve(self, font_size: f32) -> f32 {
        match self {
            Self::Auto | Self::FromFont => (font_size / 16.0).max(1.0),
            Self::Length(value) => value.resolve(font_size as f64).max(0.0) as f32,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextDecoration {
    pub lines: TextDecorationLines,
    pub style: TextDecorationStyle,
    pub color: DecorationColor,
    pub thickness: TextDecorationThickness,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct UsedTextDecoration {
    pub lines: TextDecorationLines,
    pub style: TextDecorationStyle,
    pub color: DecorationColor,
    pub thickness: UsedTextDecorationThickness,
}

impl TextDecoration {
    fn resolve_font_relative(self, metrics: FontRelativeRatios) -> UsedTextDecoration {
        UsedTextDecoration { lines: self.lines, style: self.style, color: self.color, thickness: self.thickness.resolve_font_relative(metrics) }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Outline {
    width: FontRelativeLength,
    offset: LengthPct,
    offset_inset: bool,
    pub style: BorderStyle,
    pub color: DecorationColor,
}

impl Outline {
    pub fn width(self) -> FontRelativeLength {
        self.width
    }

    pub fn set_width(&mut self, width: FontRelativeLength) {
        self.width = width;
    }

    pub fn offset(self) -> LengthPct {
        self.offset
    }

    pub fn set_offset(&mut self, offset: LengthPct) {
        self.offset = offset;
        self.offset_inset = false;
    }

    pub fn offset_is_inset(self) -> bool {
        self.offset_inset
    }

    pub fn set_offset_inset(&mut self) {
        self.offset = LengthPct::Px(0.0);
        self.offset_inset = true;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UsedOutline {
    width: f32,
    offset: f32,
    pub style: BorderStyle,
    pub color: DecorationColor,
}

impl UsedOutline {
    pub fn width(self) -> f32 {
        self.width
    }

    pub fn offset(self) -> f32 {
        self.offset
    }
}

impl Default for Outline {
    fn default() -> Self {
        Self { width: FontRelativeLength(3.0), offset: LengthPct::Px(0.0), offset_inset: false, style: BorderStyle::None, color: DecorationColor::CurrentColor }
    }
}

impl_style_key!(Outline { width, offset, offset_inset, style, color } floats {});

/// Background and non-layout-affecting paint properties (reset).
#[derive(Clone, Debug, Default)]
pub struct Background {
    pub background_color: u32,
    /// Preserve `currentColor` symbolically through inheritance. The RGBA
    /// field caches the declaring element's value, while used-style accessors
    /// resolve this dependency against the element that owns the style.
    pub background_color_current_color: bool,
    /// Whether the winning computed background contains a non-`none` image
    /// layer, even when this renderer cannot paint that layer yet.
    pub background_image_present: bool,
    pub text_decoration: TextDecoration,
    pub outline: Outline,
}

impl_style_key!(Font {
    font_weight, font_style, font_family, font_variant_caps_features, font_variant_numeric_features,
    font_variant_ligature_features, font_kerning_features, font_feature_settings
} floats { font_size, font_size_x_height_px, font_size_ch_advance_px, font_size_cap_height_px, font_size_root_ch, font_size_root_cap_height, font_size_root_line_height });
impl_style_key!(InheritedText {
    color, visibility, language, direction, letter_spacing, word_spacing, tab_size, text_align, text_align_logical, text_align_last, text_align_last_logical, text_align_last_explicit, text_indent, text_indent_hanging, text_indent_each_line, white_space, hyphens, word_break, overflow_wrap, widows, orphans, text_transform,
    quotes, list_style_type, list_style_position, list_style_image, line_height_normal, text_box_edge
} floats { line_height, line_height_x_height_px, line_height_number });
impl_style_key!(BoxModel {
    layout_idx, display, box_sizing, overflow_x, overflow_y, text_overflow, text_box_trim, size_containment, vertical_align, float, clear, table_layout, border_collapse, empty_cells, caption_side,
    width, height, min_width, min_height, max_width, max_height, aspect_ratio, object_fit, object_position,
    margin_top, margin_bottom, margin_left, margin_right,
    padding_top, padding_bottom, padding_left, padding_right
} floats {
    border_spacing_horizontal, border_spacing_vertical
});
impl_style_key!(LayoutStyle {
    margin_top_auto, margin_right_auto, margin_bottom_auto, margin_left_auto,
    position, z_index, inset_top, inset_right, inset_bottom, inset_left, break_before, break_after, break_inside,
    flex_direction, flex_wrap, flex_basis, order,
    align_content, justify_content, align_items, align_self, justify_items, justify_self,
    row_gap, column_gap, grid_template_rows, grid_template_columns,
    grid_template_row_names, grid_template_column_names, grid_template_areas,
    grid_template_area_rows, grid_template_area_columns,
    grid_auto_rows, grid_auto_columns, grid_auto_flow, grid_row, grid_column
} floats { flex_grow, flex_shrink });
impl_style_key!(Border {
    radii_idx, current_color_sides,
    border_top_color, border_right_color, border_bottom_color, border_left_color,
    border_top_style, border_right_style, border_bottom_style, border_left_style,
    border_top_width, border_right_width, border_bottom_width, border_left_width
} floats { });
impl_style_key!(Background { background_color, background_color_current_color, background_image_present, text_decoration, outline } floats { });

/// Opaque handle to one entry in each split style table.
///
/// Handles can only be produced by a [`ComputedStylesBuilder`] and carry the
/// identity of that builder's store. Their component indices cannot be forged
/// or rewritten by downstream stages:
///
/// ```compile_fail
/// let _ = html_style_model::StyleIndices {
///     font_idx: 0,
///     text_idx: 0,
///     box_idx: 0,
///     border_idx: 0,
///     bg_idx: 0,
/// };
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StyleIndices {
    store_id: NonZeroU32,
    font_idx: u32,
    text_idx: u32,
    box_idx: u32,
    border_idx: u32,
    bg_idx: u32,
}

/// An opaque index into the string table owned by one computed-style result.
///
/// CSS-created strings deliberately do not use the parsed document's string
/// interner. This keeps styling from mutating parsing output and lets either
/// representation evolve independently.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StyleStringId {
    store_id: NonZeroU32,
    index: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CounterStyle {
    #[default]
    Decimal,
    DecimalLeadingZero,
    LowerRoman,
    UpperRoman,
    LowerGreek,
    LowerAlpha,
    UpperAlpha,
    Armenian,
    Georgian,
    Disc,
    Circle,
    Square,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CounterDirective {
    pub name: StyleStringId,
    pub value: i32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CounterDirectives {
    pub resets: Vec<CounterDirective>,
    pub increments: Vec<CounterDirective>,
}

impl CounterDirectives {
    pub fn is_empty(&self) -> bool {
        self.resets.is_empty() && self.increments.is_empty()
    }
}

/// One value in the computed `content` list of a generated pseudo-element.
///
/// Text and attribute names live in the computed-style string table so the
/// style result remains independent from the parsed document's interner.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GeneratedContentItem {
    Text(StyleStringId),
    Attribute(StyleStringId),
    Counter { name: StyleStringId, style: CounterStyle },
    Counters { name: StyleStringId, separator: StyleStringId, style: CounterStyle },
    OpenQuote,
    CloseQuote,
    NoOpenQuote,
    NoCloseQuote,
}

/// A computed `content` list. Its presence creates the pseudo-element even
/// when every item produces an empty string; `normal` and `none` are stored as
/// absence instead.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedContent {
    pub items: Vec<GeneratedContentItem>,
}

#[derive(Clone, Debug)]
struct GeneratedPseudoStyle {
    node: u32,
    style: StyleIndices,
    content: GeneratedContent,
    counters: CounterDirectives,
}

/// Immutable computed-style side table keyed by DOM node ID.
///
/// This is deliberately separate from [`html_dom::Document`]: parsing owns the
/// DOM, while style resolution produces this value. Downstream stages can
/// retain or replace either representation independently.
pub struct ComputedStyles {
    document_lineage: DocumentLineage,
    node_styles: Vec<Option<StyleIndices>>,
    first_line_styles: Vec<(u32, StyleIndices)>,
    first_letter_styles: Vec<(u32, StyleIndices)>,
    before_first_letter_styles: Vec<(u32, StyleIndices)>,
    before_styles: Vec<GeneratedPseudoStyle>,
    after_styles: Vec<GeneratedPseudoStyle>,
    counter_directives: Vec<(u32, CounterDirectives)>,
    anonymous_box_styles: Vec<u32>,
    store: StyleStore,
    strings: Vec<String>,
}

/// Reader preferences applied after the publication cascade without adding a
/// stylesheet to the publication. `None` preserves the publisher-computed
/// value.
#[derive(Clone, Debug, Default)]
pub struct ReaderStyleOverrides {
    pub font_family: Option<String>,
    pub text_align: Option<TextAlign>,
    pub line_height: Option<f32>,
    pub minimum_font_size: Option<f32>,
    pub foreground: Option<u32>,
    pub background: Option<u32>,
}

impl PartialEq for ReaderStyleOverrides {
    fn eq(&self, other: &Self) -> bool {
        self.font_family == other.font_family
            && self.text_align == other.text_align
            && self.line_height.map(f32::to_bits) == other.line_height.map(f32::to_bits)
            && self.minimum_font_size.map(f32::to_bits) == other.minimum_font_size.map(f32::to_bits)
            && self.foreground == other.foreground
            && self.background == other.background
    }
}

impl Eq for ReaderStyleOverrides {}

/// Construction-only form used by the style stage. Finishing consumes the
/// deduplication maps, so they are not retained by layout or rendering.
pub struct ComputedStylesBuilder {
    document_lineage: DocumentLineage,
    node_styles: Vec<Option<StyleIndices>>,
    first_line_styles: Vec<(u32, StyleIndices)>,
    first_letter_styles: Vec<(u32, StyleIndices)>,
    before_first_letter_styles: Vec<(u32, StyleIndices)>,
    before_styles: Vec<GeneratedPseudoStyle>,
    after_styles: Vec<GeneratedPseudoStyle>,
    counter_directives: Vec<(u32, CounterDirectives)>,
    element_nodes: Vec<bool>,
    store: StyleStore,
    strings: Vec<String>,
    string_dedup: FxHashMap<String, StyleStringId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputedStylesBuildError {
    NodeOutOfBounds { node: usize },
    TextNodeCannotHaveStyle { node: usize },
    MissingElementStyle { node: usize },
    InvalidStyleIndex { node: usize },
    InvalidStyleValue(ComputedStyleValueError),
}

impl std::fmt::Display for ComputedStylesBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NodeOutOfBounds { node } => write!(formatter, "node {node} is outside the source document"),
            Self::TextNodeCannotHaveStyle { node } => write!(formatter, "text node {node} cannot have a computed element style"),
            Self::MissingElementStyle { node } => write!(formatter, "element node {node} has no computed style"),
            Self::InvalidStyleIndex { node } => write!(formatter, "node {node} contains an index from another style store"),
            Self::InvalidStyleValue(error) => write!(formatter, "computed style value is invalid: {error}"),
        }
    }
}

impl std::error::Error for ComputedStylesBuildError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputedStyleValueError {
    NonFiniteValue { property: &'static str, field: &'static str, value: u32 },
    OutOfRangeValue { property: &'static str, field: &'static str, value: u32, min: Option<u32>, max: Option<u32> },
    MustBePositive { property: &'static str, field: &'static str, value: u32 },
    InvalidFontWeight { value: u16 },
    InvalidStructure { property: &'static str, field: &'static str },
    ForeignStyleStringId { property: &'static str, field: &'static str, expected_store: u32, actual_store: u32 },
}

impl std::fmt::Display for ComputedStyleValueError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFiniteValue { property, field, value } => write!(formatter, "non-finite value in {property}.{field}: {}", f32::from_bits(*value)),
            Self::OutOfRangeValue { property, field, value, min, max } => match (min, max) {
                (Some(min), Some(max)) => write!(formatter, "value in {property}.{field}={}: expected within [{}, {}]", f32::from_bits(*value), f32::from_bits(*min), f32::from_bits(*max)),
                (Some(min), None) => write!(formatter, "value in {property}.{field}={}: expected >= {}", f32::from_bits(*value), f32::from_bits(*min)),
                (None, Some(max)) => write!(formatter, "value in {property}.{field}={}: expected <= {}", f32::from_bits(*value), f32::from_bits(*max)),
                (None, None) => write!(formatter, "value in {property}.{field}={} is invalid", f32::from_bits(*value)),
            },
            Self::MustBePositive { property, field, value } => {
                write!(formatter, "value in {property}.{field}={} must be > 0.0", f32::from_bits(*value))
            }
            Self::InvalidFontWeight { value } => write!(formatter, "font weight {value} is outside supported range"),
            Self::InvalidStructure { property, field } => write!(formatter, "invalid structural value in {property}.{field}"),
            Self::ForeignStyleStringId { property, field, expected_store, actual_store } => {
                write!(formatter, "string handle in {property}.{field} belongs to a different style store (expected {expected_store}, got {actual_store})")
            }
        }
    }
}

impl std::error::Error for ComputedStyleValueError {}

fn must_be_finite(property: &'static str, field: &'static str, value: f32) -> Result<(), ComputedStyleValueError> {
    if value.is_finite() { Ok(()) } else { Err(ComputedStyleValueError::NonFiniteValue { property, field, value: value.to_bits() }) }
}

fn must_be_non_negative(property: &'static str, field: &'static str, value: f32) -> Result<(), ComputedStyleValueError> {
    must_be_finite(property, field, value)?;
    if value >= 0.0 { Ok(()) } else { Err(ComputedStyleValueError::OutOfRangeValue { property, field, value: value.to_bits(), min: Some(0.0f32.to_bits()), max: None }) }
}

fn validate_style_string_id(property: &'static str, field: &'static str, string_id: StyleStringId, store_id: NonZeroU32) -> Result<(), ComputedStyleValueError> {
    let actual_store = string_id.store_id.get();
    if actual_store == store_id.get() { Ok(()) } else { Err(ComputedStyleValueError::ForeignStyleStringId { property, field, expected_store: store_id.get(), actual_store }) }
}

impl Font {
    pub fn validate(&self, store_id: NonZeroU32) -> Result<(), ComputedStyleValueError> {
        must_be_non_negative("Font", "font_size", self.font_size)?;
        must_be_finite("Font", "font_size_x_height_px", self.font_size_x_height_px)?;
        must_be_finite("Font", "font_size_ch_advance_px", self.font_size_ch_advance_px)?;
        must_be_finite("Font", "font_size_cap_height_px", self.font_size_cap_height_px)?;
        must_be_finite("Font", "font_size_root_ch", self.font_size_root_ch)?;
        must_be_finite("Font", "font_size_root_cap_height", self.font_size_root_cap_height)?;
        must_be_finite("Font", "font_size_root_line_height", self.font_size_root_line_height)?;
        if self.font_weight == 0 || self.font_weight > 1000 {
            return Err(ComputedStyleValueError::InvalidFontWeight { value: self.font_weight });
        }
        if let Some(font_family) = self.font_family {
            validate_style_string_id("Font", "font_family", font_family, store_id)?;
        }
        Ok(())
    }
}

impl InheritedText {
    pub fn validate(&self, store_id: NonZeroU32) -> Result<(), ComputedStyleValueError> {
        // `line_height_normal` distinguishes the CSS keyword from an authored
        // zero; the numeric value itself is always finite and non-negative.
        must_be_finite("InheritedText", "line_height", self.line_height)?;
        must_be_non_negative("InheritedText", "line_height_x_height_px", self.line_height_x_height_px)?;
        if self.line_height < 0.0 {
            return Err(ComputedStyleValueError::OutOfRangeValue { property: "InheritedText", field: "line_height", value: self.line_height.to_bits(), min: Some(0.0f32.to_bits()), max: None });
        }
        must_be_non_negative("InheritedText", "line_height_number", self.line_height_number)?;
        debug_assert!(self.letter_spacing.is_finite());
        debug_assert!(self.word_spacing.is_finite());
        debug_assert!(self.tab_size.value().is_finite() && self.tab_size.value() >= 0.0);
        self.text_indent.validate("InheritedText", "text_indent", true)?;
        if let Some(list_style_image) = self.list_style_image {
            validate_style_string_id("InheritedText", "list_style_image", list_style_image, store_id)?;
        }
        if let Some(language) = self.language {
            validate_style_string_id("InheritedText", "language", language, store_id)?;
        }
        if let QuoteStyle::Pairs(pairs) = self.quotes {
            validate_style_string_id("InheritedText", "quotes", pairs, store_id)?;
        }
        Ok(())
    }
}

impl BoxModel {
    /// Applies the cross-axis computed-value rules from CSS Overflow. This is
    /// called once after the cascade so specified `visible`/`clip` values are
    /// not lost while declarations for the other axis are still arriving.
    pub fn normalize_overflow_axes(&mut self) {
        let specified_x = self.overflow_x;
        let specified_y = self.overflow_y;
        self.overflow_x = specified_x.computed_against(specified_y);
        self.overflow_y = specified_y.computed_against(specified_x);
    }

    pub fn validate(&self, _store_id: NonZeroU32) -> Result<(), ComputedStyleValueError> {
        self.width.validate("BoxModel", "width", false)?;
        self.height.validate("BoxModel", "height", false)?;
        self.min_width.validate("BoxModel", "min_width", false)?;
        self.min_height.validate("BoxModel", "min_height", false)?;
        self.max_width.validate("BoxModel", "max_width", false)?;
        self.max_height.validate("BoxModel", "max_height", false)?;
        self.object_position.x.offset.validate("BoxModel", "object_position.x", true)?;
        self.object_position.y.offset.validate("BoxModel", "object_position.y", true)?;
        must_be_non_negative("BoxModel", "border_spacing_horizontal", self.border_spacing_horizontal)?;
        must_be_non_negative("BoxModel", "border_spacing_vertical", self.border_spacing_vertical)?;
        self.margin_top.validate("BoxModel", "margin_top", true)?;
        self.margin_bottom.validate("BoxModel", "margin_bottom", true)?;
        self.margin_left.validate("BoxModel", "margin_left", true)?;
        self.margin_right.validate("BoxModel", "margin_right", true)?;
        self.padding_top.validate("BoxModel", "padding_top", false)?;
        self.padding_bottom.validate("BoxModel", "padding_bottom", false)?;
        self.padding_left.validate("BoxModel", "padding_left", false)?;
        self.padding_right.validate("BoxModel", "padding_right", false)?;
        match self.vertical_align {
            VerticalAlignValue::Length(value) => must_be_finite("BoxModel", "vertical_align.length", value)?,
            VerticalAlignValue::Percent(value) => must_be_finite("BoxModel", "vertical_align.percent", value)?,
            VerticalAlignValue::Calc { absolute_px, line_height_fraction, x_height_px } => {
                must_be_finite("BoxModel", "vertical_align.calc.absolute_px", absolute_px)?;
                must_be_finite("BoxModel", "vertical_align.calc.line_height_fraction", line_height_fraction)?;
                must_be_finite("BoxModel", "vertical_align.calc.x_height_px", x_height_px)?;
            }
            _ => {}
        }
        Ok(())
    }
}

impl LayoutStyle {
    pub fn validate(&self, store_id: NonZeroU32) -> Result<(), ComputedStyleValueError> {
        for (field, inset) in [("inset_top", self.inset_top), ("inset_right", self.inset_right), ("inset_bottom", self.inset_bottom), ("inset_left", self.inset_left)] {
            if let Some(inset) = inset {
                inset.validate("LayoutStyle", field, true)?;
            }
        }
        must_be_non_negative("LayoutStyle", "flex_grow", self.flex_grow)?;
        must_be_non_negative("LayoutStyle", "flex_shrink", self.flex_shrink)?;
        self.flex_basis.validate("LayoutStyle", "flex_basis", false)?;
        self.row_gap.validate("LayoutStyle", "row_gap", false)?;
        self.column_gap.validate("LayoutStyle", "column_gap", false)?;
        for track in self.grid_template_rows.iter().chain(&self.grid_template_columns) {
            validate_grid_template_track(track, store_id)?;
        }
        for track in self.grid_auto_rows.iter().chain(&self.grid_auto_columns) {
            validate_grid_track_size(track)?;
        }
        for names in self.grid_template_row_names.iter().chain(&self.grid_template_column_names) {
            for &name in names {
                validate_style_string_id("LayoutStyle", "grid_line_name", name, store_id)?;
            }
        }
        if !self.grid_template_row_names.is_empty() && self.grid_template_row_names.len() != self.grid_template_rows.len() + 1 {
            return Err(ComputedStyleValueError::InvalidStructure { property: "LayoutStyle", field: "grid_template_row_names" });
        }
        if !self.grid_template_column_names.is_empty() && self.grid_template_column_names.len() != self.grid_template_columns.len() + 1 {
            return Err(ComputedStyleValueError::InvalidStructure { property: "LayoutStyle", field: "grid_template_column_names" });
        }
        for area in &self.grid_template_areas {
            validate_style_string_id("LayoutStyle", "grid_template_area", area.name, store_id)?;
            if area.row_start == 0 || area.column_start == 0 || area.row_end <= area.row_start || area.column_end <= area.column_start {
                return Err(ComputedStyleValueError::InvalidStructure { property: "LayoutStyle", field: "grid_template_area_bounds" });
            }
        }
        for (index, area) in self.grid_template_areas.iter().enumerate() {
            if self.grid_template_areas[..index].iter().any(|earlier| earlier.name == area.name) {
                return Err(ComputedStyleValueError::InvalidStructure { property: "LayoutStyle", field: "grid_template_area_names" });
            }
        }
        for placement in [self.grid_row.start, self.grid_row.end, self.grid_column.start, self.grid_column.end] {
            match placement {
                GridPlacement::NamedLine { name, .. } | GridPlacement::NamedSpan { name, .. } => {
                    validate_style_string_id("LayoutStyle", "grid_placement", name, store_id)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn validate_grid_template_track(track: &GridTemplateTrack, store_id: NonZeroU32) -> Result<(), ComputedStyleValueError> {
    match track {
        GridTemplateTrack::Single(size) => validate_grid_track_size(size),
        GridTemplateTrack::Repeat { tracks, line_names, .. } => {
            if tracks.is_empty() || (!line_names.is_empty() && line_names.len() != tracks.len() + 1) {
                return Err(ComputedStyleValueError::InvalidStructure { property: "LayoutStyle", field: "grid_repeat" });
            }
            for size in tracks {
                validate_grid_track_size(size)?;
            }
            for names in line_names {
                for &name in names {
                    validate_style_string_id("LayoutStyle", "grid_repeat_line_name", name, store_id)?;
                }
            }
            Ok(())
        }
    }
}

fn validate_grid_track_size(size: &GridTrackSize) -> Result<(), ComputedStyleValueError> {
    match size {
        GridTrackSize::Auto => Ok(()),
        GridTrackSize::Breadth(value) => validate_grid_track_breadth(value),
        GridTrackSize::MinMax { min, max } => {
            validate_grid_track_breadth(min)?;
            validate_grid_track_breadth(max)
        }
        GridTrackSize::FitContent(value) => value.validate("LayoutStyle", "grid_fit_content", false),
    }
}

fn validate_grid_track_breadth(value: &GridTrackBreadth) -> Result<(), ComputedStyleValueError> {
    match value {
        GridTrackBreadth::Length(value) => value.validate("LayoutStyle", "grid_track_length", false),
        GridTrackBreadth::Flex(value) => must_be_non_negative("LayoutStyle", "grid_track_flex", *value),
        _ => Ok(()),
    }
}

impl Border {
    pub fn validate(&self, _store_id: NonZeroU32) -> Result<(), ComputedStyleValueError> {
        self.border_top_width.validate("Border", "border_top_width")?;
        self.border_right_width.validate("Border", "border_right_width")?;
        self.border_bottom_width.validate("Border", "border_bottom_width")?;
        self.border_left_width.validate("Border", "border_left_width")?;
        Ok(())
    }
}

impl Background {
    pub fn validate(&self, _store_id: NonZeroU32) -> Result<(), ComputedStyleValueError> {
        self.outline.width.validate("Outline", "width")?;
        self.outline.offset.validate("Outline", "offset", true)?;
        if let TextDecorationThickness::Length(value) = self.text_decoration.thickness {
            value.validate("TextDecoration", "thickness", false)?;
        }
        Ok(())
    }
}

impl LengthPct {
    fn validate(&self, property: &'static str, field: &'static str, allow_negative: bool) -> Result<(), ComputedStyleValueError> {
        match self {
            LengthPct::Px(px) | LengthPct::Ex(px) | LengthPct::Ch(px) | LengthPct::Cap(px) => {
                if allow_negative {
                    must_be_finite(property, field, *px)
                } else {
                    must_be_non_negative(property, field, *px)
                }
            }
            LengthPct::Pct(pct) => {
                if allow_negative {
                    must_be_finite(property, field, *pct)
                } else {
                    must_be_non_negative(property, field, *pct)
                }
            }
            LengthPct::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, .. } => {
                must_be_finite(property, field, *absolute_px)?;
                must_be_finite(property, field, *percentage)?;
                must_be_finite(property, field, *x_height_px)?;
                must_be_finite(property, field, *ch_advance_px)?;
                must_be_finite(property, field, *cap_height_px)
            }
        }
    }
}

impl PreferredSize {
    fn validate(&self, property: &'static str, field: &'static str, allow_negative: bool) -> Result<(), ComputedStyleValueError> {
        match self {
            Self::Auto | Self::MinContent | Self::MaxContent | Self::FitContent | Self::Stretch => Ok(()),
            Self::Px(px) | Self::Ex(px) | Self::Ch(px) | Self::Cap(px) => {
                if allow_negative {
                    must_be_finite(property, field, *px)
                } else {
                    must_be_non_negative(property, field, *px)
                }
            }
            Self::Percent(pct) => {
                if allow_negative {
                    must_be_finite(property, field, *pct)
                } else {
                    must_be_non_negative(property, field, *pct)
                }
            }
            Self::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, .. } => {
                must_be_finite(property, field, *absolute_px)?;
                must_be_finite(property, field, *percentage)?;
                must_be_finite(property, field, *x_height_px)?;
                must_be_finite(property, field, *ch_advance_px)?;
                must_be_finite(property, field, *cap_height_px)
            }
            Self::Comparison(_) => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputedStylesValidationError {
    WrongDocument,
    WrongNodeCount { expected: usize, actual: usize },
    MissingElementStyle { node: usize },
    UnexpectedTextStyle { node: usize },
    InvalidStyleIndex { node: usize },
}

impl std::fmt::Display for ComputedStylesValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongDocument => formatter.write_str("computed styles belong to a different document"),
            Self::WrongNodeCount { expected, actual } => write!(formatter, "computed-style node count is {actual}, expected {expected}"),
            Self::MissingElementStyle { node } => write!(formatter, "element node {node} has no computed style"),
            Self::UnexpectedTextStyle { node } => write!(formatter, "text node {node} unexpectedly has a computed style"),
            Self::InvalidStyleIndex { node } => write!(formatter, "node {node} contains an out-of-bounds style index"),
        }
    }
}

impl std::error::Error for ComputedStylesValidationError {}

impl ComputedStylesBuilder {
    pub fn new(document: &Document) -> Self {
        let mut store = StyleStore::new(next_style_store_id());
        store.ensure_defaults();
        let element_nodes = document.nodes().map(|(_, node)| matches!(node, NodeRef::Element(_))).collect();
        Self {
            document_lineage: document.lineage().clone(),
            node_styles: vec![None; document.node_count()],
            first_line_styles: Vec::new(),
            first_letter_styles: Vec::new(),
            before_first_letter_styles: Vec::new(),
            before_styles: Vec::new(),
            after_styles: Vec::new(),
            counter_directives: Vec::new(),
            element_nodes,
            store,
            strings: Vec::new(),
            string_dedup: FxHashMap::default(),
        }
    }

    pub fn intern_string(&mut self, value: &str) -> StyleStringId {
        if let Some(&id) = self.string_dedup.get(value) {
            return id;
        }
        let id = StyleStringId { store_id: self.store.store_id, index: self.strings.len() as u32 };
        let owned = value.to_owned();
        self.string_dedup.insert(owned.clone(), id);
        self.strings.push(owned);
        id
    }

    pub fn intern_size_comparison(&mut self, kind: SizeComparison, values: [ComputedSizeComponent; 3], count: u8) -> Option<PreferredSize> {
        if !(1..=3).contains(&count) {
            return None;
        }
        let expression = ComputedSizeExpression { kind, values, count };
        let index = intern_style(&mut self.store.size_expressions, &mut self.store.size_expression_dedup, expression);
        Some(PreferredSize::Comparison(SizeExpressionId { store_id: self.store.store_id, index }))
    }

    pub fn string(&self, id: StyleStringId) -> Option<&str> {
        (id.store_id == self.store.store_id).then(|| self.strings.get(id.index as usize)).flatten().map(String::as_str)
    }

    pub fn push(&mut self, font: Font, text: InheritedText, box_model: BoxModel, border: Border, background: Background) -> Result<StyleIndices, ComputedStyleValueError> {
        self.push_with_layout(font, text, box_model, border, background, LayoutStyle::default())
    }

    pub fn push_with_layout(&mut self, font: Font, text: InheritedText, box_model: BoxModel, border: Border, background: Background, layout: LayoutStyle) -> Result<StyleIndices, ComputedStyleValueError> {
        self.push_with_layout_and_radii(font, text, box_model, border, background, layout, BorderRadii::default())
    }

    pub fn push_with_layout_and_radii(&mut self, font: Font, text: InheritedText, box_model: BoxModel, border: Border, background: Background, layout: LayoutStyle, radii: BorderRadii) -> Result<StyleIndices, ComputedStyleValueError> {
        font.validate(self.store.store_id)?;
        text.validate(self.store.store_id)?;
        box_model.validate(self.store.store_id)?;
        border.validate(self.store.store_id)?;
        background.validate(self.store.store_id)?;
        layout.validate(self.store.store_id)?;
        radii.validate()?;

        Ok(self.store.push(font, text, box_model, border, background, layout, radii))
    }

    pub fn default_indices(&self) -> StyleIndices {
        self.store.default_indices()
    }

    pub fn set_node_style(&mut self, node_idx: DomNodeId, style: StyleIndices) -> Result<(), ComputedStylesBuildError> {
        let node = node_idx.index();
        let Some(is_element) = self.element_nodes.get(node).copied() else {
            return Err(ComputedStylesBuildError::NodeOutOfBounds { node });
        };
        if !is_element {
            return Err(ComputedStylesBuildError::TextNodeCannotHaveStyle { node });
        }
        let slot = &mut self.node_styles[node];
        *slot = Some(style);
        Ok(())
    }

    pub fn set_first_line_style(&mut self, node_idx: DomNodeId, style: StyleIndices) -> Result<(), ComputedStylesBuildError> {
        self.set_pseudo_style(node_idx, style, true)
    }

    pub fn set_first_letter_style(&mut self, node_idx: DomNodeId, style: StyleIndices) -> Result<(), ComputedStylesBuildError> {
        self.set_pseudo_style(node_idx, style, false)
    }

    pub fn set_before_first_letter_style(&mut self, node_idx: DomNodeId, style: StyleIndices) -> Result<(), ComputedStylesBuildError> {
        let node = node_idx.index();
        let Some(is_element) = self.element_nodes.get(node).copied() else {
            return Err(ComputedStylesBuildError::NodeOutOfBounds { node });
        };
        if !is_element {
            return Err(ComputedStylesBuildError::TextNodeCannotHaveStyle { node });
        }
        if !self.store.contains(style) {
            return Err(ComputedStylesBuildError::InvalidStyleIndex { node });
        }
        if let Some((_, existing)) = self.before_first_letter_styles.iter_mut().find(|(candidate, _)| *candidate == node as u32) {
            *existing = style;
        } else {
            self.before_first_letter_styles.push((node as u32, style));
        }
        Ok(())
    }

    pub fn set_before_style(&mut self, node_idx: DomNodeId, style: StyleIndices, content: GeneratedContent, counters: CounterDirectives) -> Result<(), ComputedStylesBuildError> {
        self.set_generated_pseudo_style(node_idx, style, content, counters, true)
    }

    pub fn set_after_style(&mut self, node_idx: DomNodeId, style: StyleIndices, content: GeneratedContent, counters: CounterDirectives) -> Result<(), ComputedStylesBuildError> {
        self.set_generated_pseudo_style(node_idx, style, content, counters, false)
    }

    fn set_generated_pseudo_style(&mut self, node_idx: DomNodeId, style: StyleIndices, content: GeneratedContent, counters: CounterDirectives, before: bool) -> Result<(), ComputedStylesBuildError> {
        let node = node_idx.index();
        let Some(is_element) = self.element_nodes.get(node).copied() else {
            return Err(ComputedStylesBuildError::NodeOutOfBounds { node });
        };
        if !is_element {
            return Err(ComputedStylesBuildError::TextNodeCannotHaveStyle { node });
        }
        if !self.store.contains(style) {
            return Err(ComputedStylesBuildError::InvalidStyleIndex { node });
        }
        let target = if before { &mut self.before_styles } else { &mut self.after_styles };
        if let Some(existing) = target.iter_mut().find(|candidate| candidate.node == node as u32) {
            existing.style = style;
            existing.content = content;
            existing.counters = counters;
        } else {
            target.push(GeneratedPseudoStyle { node: node as u32, style, content, counters });
        }
        Ok(())
    }

    pub fn set_counter_directives(&mut self, node_idx: DomNodeId, counters: CounterDirectives) -> Result<(), ComputedStylesBuildError> {
        let node = node_idx.index();
        let Some(is_element) = self.element_nodes.get(node).copied() else {
            return Err(ComputedStylesBuildError::NodeOutOfBounds { node });
        };
        if !is_element {
            return Err(ComputedStylesBuildError::TextNodeCannotHaveStyle { node });
        }
        if let Some((_, existing)) = self.counter_directives.iter_mut().find(|(candidate, _)| *candidate == node as u32) {
            *existing = counters;
        } else {
            self.counter_directives.push((node as u32, counters));
        }
        Ok(())
    }

    fn set_pseudo_style(&mut self, node_idx: DomNodeId, style: StyleIndices, first_line: bool) -> Result<(), ComputedStylesBuildError> {
        let node = node_idx.index();
        let Some(is_element) = self.element_nodes.get(node).copied() else {
            return Err(ComputedStylesBuildError::NodeOutOfBounds { node });
        };
        if !is_element {
            return Err(ComputedStylesBuildError::TextNodeCannotHaveStyle { node });
        }
        if !self.store.contains(style) {
            return Err(ComputedStylesBuildError::InvalidStyleIndex { node });
        }
        let target = if first_line { &mut self.first_line_styles } else { &mut self.first_letter_styles };
        if let Some((_, existing)) = target.iter_mut().find(|(candidate, _)| *candidate == node as u32) {
            *existing = style;
        } else {
            target.push((node as u32, style));
        }
        Ok(())
    }

    pub fn style_for_node(&self, node_idx: DomNodeId) -> Option<StyleIndices> {
        self.node_styles.get(node_idx.index()).copied().flatten()
    }

    pub fn view(&self, indices: StyleIndices) -> Option<StyleView<'_>> {
        self.store.view(indices)
    }

    pub fn font_style(&self, indices: StyleIndices) -> Option<&Font> {
        self.store.font_style(indices)
    }

    pub fn text_style(&self, indices: StyleIndices) -> Option<&InheritedText> {
        self.store.text_style(indices)
    }

    pub fn box_model_style(&self, indices: StyleIndices) -> Option<&BoxModel> {
        self.store.box_model_style(indices)
    }

    pub fn border_style(&self, indices: StyleIndices) -> Option<&Border> {
        self.store.border_style(indices)
    }

    pub fn border_radii_style(&self, indices: StyleIndices) -> Option<&BorderRadii> {
        self.store.border_radii_style(indices)
    }

    pub fn background_style(&self, indices: StyleIndices) -> Option<&Background> {
        self.store.background_style(indices)
    }

    pub fn layout_style(&self, indices: StyleIndices) -> Option<&LayoutStyle> {
        self.store.layout_style(indices)
    }

    pub fn finish(mut self) -> Result<ComputedStyles, ComputedStylesBuildError> {
        let anonymous_box_styles = self.store.build_anonymous_box_styles();
        self.store.validate_values().map_err(ComputedStylesBuildError::InvalidStyleValue)?;
        for (node, (&is_element, style)) in self.element_nodes.iter().zip(self.node_styles.iter().copied()).enumerate() {
            if is_element && style.is_none() {
                return Err(ComputedStylesBuildError::MissingElementStyle { node });
            }
            if let Some(indices) = style
                && !self.store.contains(indices)
            {
                return Err(ComputedStylesBuildError::InvalidStyleIndex { node });
            }
        }
        self.store.discard_deduplication_maps();
        Ok(ComputedStyles {
            document_lineage: self.document_lineage,
            node_styles: self.node_styles,
            first_line_styles: self.first_line_styles,
            first_letter_styles: self.first_letter_styles,
            before_first_letter_styles: self.before_first_letter_styles,
            before_styles: self.before_styles,
            after_styles: self.after_styles,
            counter_directives: self.counter_directives,
            anonymous_box_styles,
            store: self.store,
            strings: self.strings,
        })
    }
}

fn reintern_style_string(source: &ComputedStyles, builder: &mut ComputedStylesBuilder, id: StyleStringId) -> StyleStringId {
    builder.intern_string(source.string(id).expect("validated style string belongs to the source store"))
}

fn reintern_generated_content(source: &ComputedStyles, builder: &mut ComputedStylesBuilder, content: &GeneratedContent) -> GeneratedContent {
    GeneratedContent {
        items: content
            .items
            .iter()
            .map(|item| match *item {
                GeneratedContentItem::Text(id) => GeneratedContentItem::Text(reintern_style_string(source, builder, id)),
                GeneratedContentItem::Attribute(id) => GeneratedContentItem::Attribute(reintern_style_string(source, builder, id)),
                GeneratedContentItem::Counter { name, style } => GeneratedContentItem::Counter { name: reintern_style_string(source, builder, name), style },
                GeneratedContentItem::Counters { name, separator, style } => GeneratedContentItem::Counters { name: reintern_style_string(source, builder, name), separator: reintern_style_string(source, builder, separator), style },
                other => other,
            })
            .collect(),
    }
}

fn reintern_counter_directives(source: &ComputedStyles, builder: &mut ComputedStylesBuilder, counters: &CounterDirectives) -> CounterDirectives {
    let reintern = |directive: &CounterDirective, builder: &mut ComputedStylesBuilder| CounterDirective { name: reintern_style_string(source, builder, directive.name), value: directive.value };
    CounterDirectives { resets: counters.resets.iter().map(|directive| reintern(directive, builder)).collect(), increments: counters.increments.iter().map(|directive| reintern(directive, builder)).collect() }
}

fn reintern_line_names(source: &ComputedStyles, builder: &mut ComputedStylesBuilder, line_names: &mut [Vec<StyleStringId>]) {
    for names in line_names {
        for name in names {
            *name = reintern_style_string(source, builder, *name);
        }
    }
}

fn reintern_placement(source: &ComputedStyles, builder: &mut ComputedStylesBuilder, placement: &mut GridPlacement) {
    match placement {
        GridPlacement::NamedLine { name, .. } | GridPlacement::NamedSpan { name, .. } => *name = reintern_style_string(source, builder, *name),
        GridPlacement::Auto | GridPlacement::Line(_) | GridPlacement::Span(_) => {}
    }
}

fn reintern_layout_strings(source: &ComputedStyles, builder: &mut ComputedStylesBuilder, layout: &mut LayoutStyle) {
    reintern_line_names(source, builder, &mut layout.grid_template_row_names);
    reintern_line_names(source, builder, &mut layout.grid_template_column_names);
    for track in layout.grid_template_rows.iter_mut().chain(layout.grid_template_columns.iter_mut()) {
        if let GridTemplateTrack::Repeat { line_names, .. } = track {
            reintern_line_names(source, builder, line_names);
        }
    }
    for area in &mut layout.grid_template_areas {
        area.name = reintern_style_string(source, builder, area.name);
    }
    reintern_placement(source, builder, &mut layout.grid_row.start);
    reintern_placement(source, builder, &mut layout.grid_row.end);
    reintern_placement(source, builder, &mut layout.grid_column.start);
    reintern_placement(source, builder, &mut layout.grid_column.end);
}

fn reintern_preferred_size(source: &ComputedStyles, builder: &mut ComputedStylesBuilder, size: &mut PreferredSize) {
    let PreferredSize::Comparison(id) = *size else { return };
    let Some(expression) = source.store.size_expressions.get(id.index as usize).copied() else {
        *size = PreferredSize::Auto;
        return;
    };
    *size = builder.intern_size_comparison(expression.kind, expression.values, expression.count).unwrap_or(PreferredSize::Auto);
}

fn reintern_with_reader_overrides(source: &ComputedStyles, builder: &mut ComputedStylesBuilder, indices: StyleIndices, overrides: &ReaderStyleOverrides, override_background: bool) -> Result<StyleIndices, ComputedStylesBuildError> {
    let mut font = source.font_style(indices).expect("validated font style").clone();
    font.font_family = match overrides.font_family.as_deref() {
        Some(family) => Some(builder.intern_string(family)),
        None => font.font_family.and_then(|id| source.string(id)).map(|value| builder.intern_string(value)),
    };
    if let Some(minimum) = overrides.minimum_font_size.filter(|value| value.is_finite() && *value > 0.0) {
        font.font_size = font.font_size.max(minimum);
    }
    let mut text = source.text_style(indices).expect("validated text style").clone();
    if let Some(alignment) = overrides.text_align {
        text.text_align = alignment;
        if !text.text_align_last_explicit {
            text.text_align_last = alignment;
        }
    }
    if let Some(multiplier) = overrides.line_height.filter(|value| value.is_finite() && *value > 0.0) {
        text.line_height_number = multiplier;
        text.line_height = font.font_size * multiplier;
        text.line_height_x_height_px = 0.0;
        text.line_height_normal = false;
    }
    text.list_style_image = text.list_style_image.map(|id| reintern_style_string(source, builder, id));
    text.language = text.language.map(|id| reintern_style_string(source, builder, id));
    if let QuoteStyle::Pairs(pairs) = text.quotes {
        text.quotes = QuoteStyle::Pairs(reintern_style_string(source, builder, pairs));
    }
    if let Some(color) = overrides.foreground {
        text.color = color;
    }
    let mut background = source.background_style(indices).expect("validated background style").clone();
    if override_background && let Some(color) = overrides.background {
        background.background_color = color;
        background.background_color_current_color = false;
    }
    let mut layout = source.layout_style(indices).expect("validated layout style").clone();
    reintern_layout_strings(source, builder, &mut layout);
    reintern_preferred_size(source, builder, &mut layout.flex_basis);
    let mut box_model = source.box_model_style(indices).expect("validated box style").clone();
    for size in [&mut box_model.width, &mut box_model.height, &mut box_model.min_width, &mut box_model.min_height, &mut box_model.max_width, &mut box_model.max_height] {
        reintern_preferred_size(source, builder, size);
    }
    builder
        .push_with_layout_and_radii(font, text, box_model, source.border_style(indices).expect("validated border style").clone(), background, layout, *source.border_radii_style(indices).expect("validated radius style"))
        .map_err(ComputedStylesBuildError::InvalidStyleValue)
}

impl ComputedStyles {
    pub fn with_reader_overrides(&self, document: &Document, overrides: &ReaderStyleOverrides) -> Result<Self, ComputedStylesBuildError> {
        let mut builder = ComputedStylesBuilder::new(document);
        for (node, node_ref) in document.nodes() {
            if !matches!(node_ref, NodeRef::Element(_)) {
                continue;
            }
            let indices = self.style_for_node(node).ok_or(ComputedStylesBuildError::MissingElementStyle { node: node.index() })?;
            let override_background = document.element_ref(node).is_some_and(|element| matches!(element.tag(), "html" | "body"));
            let style = reintern_with_reader_overrides(self, &mut builder, indices, overrides, override_background)?;
            builder.set_node_style(node, style)?;
            if let Some(counters) = self.counter_directives_for_node(node) {
                let counters = reintern_counter_directives(self, &mut builder, counters);
                builder.set_counter_directives(node, counters)?;
            }
        }
        for &(raw, indices) in &self.first_line_styles {
            let node = document.node_id_from_raw(raw).ok_or(ComputedStylesBuildError::NodeOutOfBounds { node: raw as usize })?;
            let style = reintern_with_reader_overrides(self, &mut builder, indices, overrides, false)?;
            builder.set_first_line_style(node, style)?;
        }
        for &(raw, indices) in &self.first_letter_styles {
            let node = document.node_id_from_raw(raw).ok_or(ComputedStylesBuildError::NodeOutOfBounds { node: raw as usize })?;
            let style = reintern_with_reader_overrides(self, &mut builder, indices, overrides, false)?;
            builder.set_first_letter_style(node, style)?;
        }
        for &(raw, indices) in &self.before_first_letter_styles {
            let node = document.node_id_from_raw(raw).ok_or(ComputedStylesBuildError::NodeOutOfBounds { node: raw as usize })?;
            let style = reintern_with_reader_overrides(self, &mut builder, indices, overrides, false)?;
            builder.set_before_first_letter_style(node, style)?;
        }
        for pseudo in &self.before_styles {
            let node = document.node_id_from_raw(pseudo.node).ok_or(ComputedStylesBuildError::NodeOutOfBounds { node: pseudo.node as usize })?;
            let style = reintern_with_reader_overrides(self, &mut builder, pseudo.style, overrides, false)?;
            let content = reintern_generated_content(self, &mut builder, &pseudo.content);
            let counters = reintern_counter_directives(self, &mut builder, &pseudo.counters);
            builder.set_before_style(node, style, content, counters)?;
        }
        for pseudo in &self.after_styles {
            let node = document.node_id_from_raw(pseudo.node).ok_or(ComputedStylesBuildError::NodeOutOfBounds { node: pseudo.node as usize })?;
            let style = reintern_with_reader_overrides(self, &mut builder, pseudo.style, overrides, false)?;
            let content = reintern_generated_content(self, &mut builder, &pseudo.content);
            let counters = reintern_counter_directives(self, &mut builder, &pseudo.counters);
            builder.set_after_style(node, style, content, counters)?;
        }
        builder.finish()
    }

    pub fn node_count(&self) -> usize {
        self.node_styles.len()
    }

    pub fn style_for_node(&self, node_idx: DomNodeId) -> Option<StyleIndices> {
        self.node_styles.get(node_idx.index()).copied().flatten()
    }

    pub fn first_line_style_for_node(&self, node_idx: DomNodeId) -> Option<StyleIndices> {
        self.first_line_styles.iter().find_map(|(node, style)| (*node == node_idx.raw()).then_some(*style))
    }

    pub fn first_letter_style_for_node(&self, node_idx: DomNodeId) -> Option<StyleIndices> {
        self.first_letter_styles.iter().find_map(|(node, style)| (*node == node_idx.raw()).then_some(*style))
    }

    pub fn before_first_letter_style_for_node(&self, node_idx: DomNodeId) -> Option<StyleIndices> {
        self.before_first_letter_styles.iter().find_map(|(node, style)| (*node == node_idx.raw()).then_some(*style))
    }

    pub fn before_style_for_node(&self, node_idx: DomNodeId) -> Option<(StyleIndices, &GeneratedContent, &CounterDirectives)> {
        self.before_styles.iter().find(|pseudo| pseudo.node == node_idx.raw()).map(|pseudo| (pseudo.style, &pseudo.content, &pseudo.counters))
    }

    pub fn after_style_for_node(&self, node_idx: DomNodeId) -> Option<(StyleIndices, &GeneratedContent, &CounterDirectives)> {
        self.after_styles.iter().find(|pseudo| pseudo.node == node_idx.raw()).map(|pseudo| (pseudo.style, &pseudo.content, &pseudo.counters))
    }

    pub fn counter_directives_for_node(&self, node_idx: DomNodeId) -> Option<&CounterDirectives> {
        self.counter_directives.iter().find_map(|(node, counters)| (*node == node_idx.raw()).then_some(counters))
    }

    pub fn string(&self, id: StyleStringId) -> Option<&str> {
        (id.store_id == self.store.store_id).then(|| self.strings.get(id.index as usize)).flatten().map(String::as_str)
    }

    pub fn default_indices(&self) -> StyleIndices {
        self.store.default_indices()
    }

    /// Preserve inherited parts for an anonymous box while resetting its
    /// non-inherited parts. A handle from another style result is rejected.
    pub fn anonymous_box_indices(&self, parent: StyleIndices) -> Option<StyleIndices> {
        if !self.store.contains(parent) {
            return None;
        }
        let defaults = self.store.default_indices();
        let box_idx = *self.anonymous_box_styles.get(parent.box_idx as usize)?;
        Some(StyleIndices { store_id: self.store.store_id, font_idx: parent.font_idx, text_idx: parent.text_idx, box_idx, border_idx: defaults.border_idx, bg_idx: defaults.bg_idx })
    }

    pub fn validate_for(&self, document: &Document) -> Result<(), ComputedStylesValidationError> {
        if !self.document_lineage.matches(document.lineage()) {
            return Err(ComputedStylesValidationError::WrongDocument);
        }
        if self.node_styles.len() != document.node_count() {
            return Err(ComputedStylesValidationError::WrongNodeCount { expected: document.node_count(), actual: self.node_styles.len() });
        }
        for (node_id, style) in document.node_ids().zip(self.node_styles.iter().copied()) {
            let node = node_id.index();
            match (document.node_ref(node_id), style) {
                (Some(NodeRef::Element(_)), None) => return Err(ComputedStylesValidationError::MissingElementStyle { node }),
                (Some(NodeRef::Text(_)), Some(_)) => return Err(ComputedStylesValidationError::UnexpectedTextStyle { node }),
                (_, Some(indices)) if !self.store.contains(indices) => return Err(ComputedStylesValidationError::InvalidStyleIndex { node }),
                _ => {}
            }
        }
        for &(raw, indices) in self.first_line_styles.iter().chain(&self.first_letter_styles).chain(&self.before_first_letter_styles) {
            let Some(node_id) = document.node_id_from_raw(raw) else {
                return Err(ComputedStylesValidationError::InvalidStyleIndex { node: raw as usize });
            };
            if !matches!(document.node_ref(node_id), Some(NodeRef::Element(_))) {
                return Err(ComputedStylesValidationError::UnexpectedTextStyle { node: node_id.index() });
            }
            if !self.store.contains(indices) {
                return Err(ComputedStylesValidationError::InvalidStyleIndex { node: node_id.index() });
            }
        }
        for pseudo in self.before_styles.iter().chain(&self.after_styles) {
            let Some(node_id) = document.node_id_from_raw(pseudo.node) else {
                return Err(ComputedStylesValidationError::InvalidStyleIndex { node: pseudo.node as usize });
            };
            if !matches!(document.node_ref(node_id), Some(NodeRef::Element(_))) {
                return Err(ComputedStylesValidationError::UnexpectedTextStyle { node: node_id.index() });
            }
            if !self.store.contains(pseudo.style) {
                return Err(ComputedStylesValidationError::InvalidStyleIndex { node: node_id.index() });
            }
        }
        Ok(())
    }

    pub fn view(&self, indices: StyleIndices) -> Option<StyleView<'_>> {
        self.store.view(indices)
    }

    /// Construct the only style view accepted by layout for font-relative
    /// lengths. The ratio is supplied by shaping after selecting a font face.
    pub fn used_view(&self, indices: StyleIndices, x_height_ratio: f32, ch_advance_ratio: f32, cap_height_ratio: f32) -> Option<UsedStyleView<'_>> {
        self.used_view_with_root(indices, x_height_ratio, ch_advance_ratio, cap_height_ratio, 0.0, 0.0, 0.0)
    }

    pub fn used_view_with_root(&self, indices: StyleIndices, x_height_ratio: f32, ch_advance_ratio: f32, cap_height_ratio: f32, root_ch_px: f32, root_cap_height_px: f32, root_line_height_px: f32) -> Option<UsedStyleView<'_>> {
        let font_relative = FontRelativeRatios::new(x_height_ratio, ch_advance_ratio, cap_height_ratio)?;
        let root_relative = RootRelativeLengths::new(root_ch_px, root_cap_height_px, root_line_height_px)?;
        self.view(indices).map(|computed| UsedStyleView { computed, font_relative, root_relative })
    }

    pub fn font_style(&self, indices: StyleIndices) -> Option<&Font> {
        self.store.font_style(indices)
    }

    pub fn text_style(&self, indices: StyleIndices) -> Option<&InheritedText> {
        self.store.text_style(indices)
    }

    pub fn box_model_style(&self, indices: StyleIndices) -> Option<&BoxModel> {
        self.store.box_model_style(indices)
    }

    pub fn border_style(&self, indices: StyleIndices) -> Option<&Border> {
        self.store.border_style(indices)
    }

    pub fn border_radii_style(&self, indices: StyleIndices) -> Option<&BorderRadii> {
        self.store.border_radii_style(indices)
    }

    pub fn background_style(&self, indices: StyleIndices) -> Option<&Background> {
        self.store.background_style(indices)
    }

    pub fn layout_style(&self, indices: StyleIndices) -> Option<&LayoutStyle> {
        self.store.layout_style(indices)
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<Option<StyleIndices>>("ComputedStyles.node_styles.storage", self.node_styles.capacity(), self.node_styles.len());
        report.add_slice_storage::<(u32, StyleIndices)>("ComputedStyles.first_line_styles.storage", self.first_line_styles.capacity(), self.first_line_styles.len());
        report.add_slice_storage::<(u32, StyleIndices)>("ComputedStyles.first_letter_styles.storage", self.first_letter_styles.capacity(), self.first_letter_styles.len());
        report.add_slice_storage::<(u32, StyleIndices)>("ComputedStyles.before_first_letter_styles.storage", self.before_first_letter_styles.capacity(), self.before_first_letter_styles.len());
        report.add_slice_storage::<GeneratedPseudoStyle>("ComputedStyles.before_styles.storage", self.before_styles.capacity(), self.before_styles.len());
        report.add_slice_storage::<GeneratedPseudoStyle>("ComputedStyles.after_styles.storage", self.after_styles.capacity(), self.after_styles.len());
        report.add_slice_storage::<(u32, CounterDirectives)>("ComputedStyles.counter_directives.storage", self.counter_directives.capacity(), self.counter_directives.len());
        for (_, counters) in &self.counter_directives {
            report.add_slice_storage::<CounterDirective>("ComputedStyles.counter_directives.resets", counters.resets.capacity(), counters.resets.len());
            report.add_slice_storage::<CounterDirective>("ComputedStyles.counter_directives.increments", counters.increments.capacity(), counters.increments.len());
        }
        report.extend_prefixed("ComputedStyles.store", self.store.memory_usage_report());
        report.add_slice_storage::<String>("ComputedStyles.strings.storage", self.strings.capacity(), self.strings.len());
        for value in &self.strings {
            report.add("ComputedStyles.strings.contents", value.capacity(), value.len());
        }
        report
    }
}

/// Computed-style storage (Servo-inspired split styles). Produced by style
/// resolution and consumed read-only by box construction, shaping, and layout.
/// `StyleIndices` from the `ComputedStyles` node side table index these vecs.
struct StyleStore {
    store_id: NonZeroU32,
    font_styles: Vec<Font>,
    text_styles: Vec<InheritedText>,
    box_styles: Vec<BoxModel>,
    border_styles: Vec<Border>,
    border_radii_styles: Vec<BorderRadii>,
    bg_styles: Vec<Background>,
    layout_styles: Vec<LayoutStyle>,
    size_expressions: Vec<ComputedSizeExpression>,
    // Dedup maps: identical computed sub-styles share one index, so runs of
    // similarly-styled elements (and inherited groups shared down subtrees)
    // collapse to a single stored entry.
    font_dedup: StyleDedup,
    text_dedup: StyleDedup,
    box_dedup: StyleDedup,
    border_dedup: StyleDedup,
    border_radii_dedup: StyleDedup,
    bg_dedup: StyleDedup,
    layout_dedup: StyleDedup,
    size_expression_dedup: StyleDedup,
}

#[derive(Default)]
struct StyleDedup {
    indices: FxHashMap<u64, u32>,
    collisions: FxHashMap<u64, Vec<u32>>,
}

static NEXT_STYLE_STORE_ID: AtomicU32 = AtomicU32::new(1);

fn next_style_store_id() -> NonZeroU32 {
    let id = NEXT_STYLE_STORE_ID.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1)).expect("computed-style store identity space exhausted");
    NonZeroU32::new(id).expect("style store IDs start at one")
}

/// Append `value` unless an identical one already exists, returning its index.
/// The index stores hashes rather than cloned style values; equality is checked
/// against the canonical value vector, with a side table for true hash
/// collisions.
fn intern_style<T: Eq + Hash>(values: &mut Vec<T>, dedup: &mut StyleDedup, value: T) -> u32 {
    let mut hasher = FxHasher::default();
    value.hash(&mut hasher);
    let hash = hasher.finish();
    if let Some(&idx) = dedup.indices.get(&hash) {
        if values[idx as usize] == value {
            return idx;
        }
        if let Some(indices) = dedup.collisions.get(&hash)
            && let Some(&idx) = indices.iter().find(|&&idx| values[idx as usize] == value)
        {
            return idx;
        }
        let idx = u32::try_from(values.len()).expect("computed style count fits in u32");
        values.push(value);
        dedup.collisions.entry(hash).or_default().push(idx);
        return idx;
    }
    let idx = u32::try_from(values.len()).expect("computed style count fits in u32");
    values.push(value);
    dedup.indices.insert(hash, idx);
    idx
}

impl StyleStore {
    fn new(store_id: NonZeroU32) -> Self {
        Self {
            store_id,
            font_styles: Vec::new(),
            text_styles: Vec::new(),
            box_styles: Vec::new(),
            border_styles: Vec::new(),
            border_radii_styles: Vec::new(),
            bg_styles: Vec::new(),
            layout_styles: Vec::new(),
            size_expressions: Vec::new(),
            font_dedup: StyleDedup::default(),
            text_dedup: StyleDedup::default(),
            box_dedup: StyleDedup::default(),
            border_dedup: StyleDedup::default(),
            border_radii_dedup: StyleDedup::default(),
            bg_dedup: StyleDedup::default(),
            layout_dedup: StyleDedup::default(),
            size_expression_dedup: StyleDedup::default(),
        }
    }

    fn contains(&self, indices: StyleIndices) -> bool {
        let base_indices_are_valid = indices.store_id == self.store_id
            && (indices.font_idx as usize) < self.font_styles.len()
            && (indices.text_idx as usize) < self.text_styles.len()
            && (indices.box_idx as usize) < self.box_styles.len()
            && (indices.border_idx as usize) < self.border_styles.len()
            && (indices.bg_idx as usize) < self.bg_styles.len();
        base_indices_are_valid && (self.box_styles[indices.box_idx as usize].layout_idx as usize) < self.layout_styles.len() && (self.border_styles[indices.border_idx as usize].radii_idx as usize) < self.border_radii_styles.len()
    }

    #[inline]
    fn owns(&self, indices: StyleIndices) -> bool {
        indices.store_id == self.store_id
    }

    fn default_indices(&self) -> StyleIndices {
        debug_assert!(!self.font_styles.is_empty());
        StyleIndices { store_id: self.store_id, font_idx: 0, text_idx: 0, box_idx: 0, border_idx: 0, bg_idx: 0 }
    }

    /// Ensure default styles exist at index 0 for fallback. Interning here so the
    /// dedup maps know index 0, letting later default styles share it.
    pub fn ensure_defaults(&mut self) {
        if self.font_styles.is_empty() {
            intern_style(&mut self.font_styles, &mut self.font_dedup, Font::default());
        }
        if self.text_styles.is_empty() {
            intern_style(&mut self.text_styles, &mut self.text_dedup, InheritedText::default());
        }
        if self.box_styles.is_empty() {
            intern_style(&mut self.box_styles, &mut self.box_dedup, BoxModel::default());
        }
        if self.border_styles.is_empty() {
            intern_style(&mut self.border_styles, &mut self.border_dedup, Border::default());
        }
        if self.border_radii_styles.is_empty() {
            intern_style(&mut self.border_radii_styles, &mut self.border_radii_dedup, BorderRadii::default());
        }
        if self.bg_styles.is_empty() {
            intern_style(&mut self.bg_styles, &mut self.bg_dedup, Background::default());
        }
        if self.layout_styles.is_empty() {
            intern_style(&mut self.layout_styles, &mut self.layout_dedup, LayoutStyle::default());
        }
    }

    /// Intern one computed style block, sharing each sub-style's index with an
    /// identical existing one. Returns the indices that address it.
    pub fn push(&mut self, font: Font, text: InheritedText, mut box_model: BoxModel, mut border: Border, background: Background, layout: LayoutStyle, radii: BorderRadii) -> StyleIndices {
        box_model.layout_idx = intern_style(&mut self.layout_styles, &mut self.layout_dedup, layout);
        border.radii_idx = intern_style(&mut self.border_radii_styles, &mut self.border_radii_dedup, radii);
        StyleIndices {
            store_id: self.store_id,
            font_idx: intern_style(&mut self.font_styles, &mut self.font_dedup, font),
            text_idx: intern_style(&mut self.text_styles, &mut self.text_dedup, text),
            box_idx: intern_style(&mut self.box_styles, &mut self.box_dedup, box_model),
            border_idx: intern_style(&mut self.border_styles, &mut self.border_dedup, border),
            bg_idx: intern_style(&mut self.bg_styles, &mut self.bg_dedup, background),
        }
    }

    fn build_anonymous_box_styles(&mut self) -> Vec<u32> {
        // Snapshot the authored styles because interning the anonymous
        // variants can append to `box_styles`. The returned table remains
        // indexed by every pre-existing parent box style.
        let parent_styles = self.box_styles.clone();
        let default_layout = self.default_indices().box_idx;
        let default_layout_idx = self.box_styles[default_layout as usize].layout_idx;
        let mut anonymous_box_styles: Vec<u32> = parent_styles
            .into_iter()
            .map(|parent| {
                let mut anonymous = BoxModel::default();
                anonymous.layout_idx = default_layout_idx;
                anonymous.border_collapse = parent.border_collapse;
                anonymous.border_spacing_horizontal = parent.border_spacing_horizontal;
                anonymous.border_spacing_vertical = parent.border_spacing_vertical;
                anonymous.caption_side = parent.caption_side;
                anonymous.empty_cells = parent.empty_cells;
                intern_style(&mut self.box_styles, &mut self.box_dedup, anonymous)
            })
            .collect();
        // Every box style appended above is itself already an anonymous
        // reset style, so nesting another anonymous box can reuse it.
        let authored_count = anonymous_box_styles.len();
        anonymous_box_styles.extend((authored_count..self.box_styles.len()).map(|index| index as u32));
        anonymous_box_styles
    }

    fn discard_deduplication_maps(&mut self) {
        self.font_dedup = StyleDedup::default();
        self.text_dedup = StyleDedup::default();
        self.box_dedup = StyleDedup::default();
        self.border_dedup = StyleDedup::default();
        self.border_radii_dedup = StyleDedup::default();
        self.bg_dedup = StyleDedup::default();
        self.layout_dedup = StyleDedup::default();
        self.size_expression_dedup = StyleDedup::default();
    }

    fn validate_values(&self) -> Result<(), ComputedStyleValueError> {
        for font in &self.font_styles {
            font.validate(self.store_id)?;
        }
        for text in &self.text_styles {
            text.validate(self.store_id)?;
        }
        for box_model in &self.box_styles {
            box_model.validate(self.store_id)?;
            for size in [box_model.width, box_model.height, box_model.min_width, box_model.min_height, box_model.max_width, box_model.max_height] {
                if let PreferredSize::Comparison(id) = size
                    && (id.store_id != self.store_id || id.index as usize >= self.size_expressions.len())
                {
                    return Err(ComputedStyleValueError::InvalidStructure { property: "BoxModel", field: "size_expression" });
                }
            }
        }
        for border in &self.border_styles {
            border.validate(self.store_id)?;
            if border.radii_idx as usize >= self.border_radii_styles.len() {
                return Err(ComputedStyleValueError::InvalidStructure { property: "Border", field: "radii_idx" });
            }
        }
        for radii in &self.border_radii_styles {
            radii.validate()?;
        }
        for background in &self.bg_styles {
            background.validate(self.store_id)?;
        }
        for layout in &self.layout_styles {
            layout.validate(self.store_id)?;
            if let PreferredSize::Comparison(id) = layout.flex_basis
                && (id.store_id != self.store_id || id.index as usize >= self.size_expressions.len())
            {
                return Err(ComputedStyleValueError::InvalidStructure { property: "LayoutStyle", field: "size_expression" });
            }
        }
        Ok(())
    }

    /// Resolve a flat `StyleView` from a branded style handle.
    pub fn view(&self, indices: StyleIndices) -> Option<StyleView<'_>> {
        self.owns(indices).then(|| {
            let box_model = &self.box_styles[indices.box_idx as usize];
            StyleView {
                font: &self.font_styles[indices.font_idx as usize],
                text: &self.text_styles[indices.text_idx as usize],
                box_model,
                border: &self.border_styles[indices.border_idx as usize],
                radii: &self.border_radii_styles[self.border_styles[indices.border_idx as usize].radii_idx as usize],
                background: &self.bg_styles[indices.bg_idx as usize],
                layout: &self.layout_styles[box_model.layout_idx as usize],
                size_expressions: &self.size_expressions,
            }
        })
    }

    pub fn font_style(&self, indices: StyleIndices) -> Option<&Font> {
        self.owns(indices).then(|| &self.font_styles[indices.font_idx as usize])
    }

    pub fn text_style(&self, indices: StyleIndices) -> Option<&InheritedText> {
        self.owns(indices).then(|| &self.text_styles[indices.text_idx as usize])
    }

    pub fn box_model_style(&self, indices: StyleIndices) -> Option<&BoxModel> {
        self.owns(indices).then(|| &self.box_styles[indices.box_idx as usize])
    }

    pub fn border_style(&self, indices: StyleIndices) -> Option<&Border> {
        self.owns(indices).then(|| &self.border_styles[indices.border_idx as usize])
    }

    pub fn border_radii_style(&self, indices: StyleIndices) -> Option<&BorderRadii> {
        self.owns(indices).then(|| {
            let radii_idx = self.border_styles[indices.border_idx as usize].radii_idx;
            &self.border_radii_styles[radii_idx as usize]
        })
    }

    pub fn background_style(&self, indices: StyleIndices) -> Option<&Background> {
        self.owns(indices).then(|| &self.bg_styles[indices.bg_idx as usize])
    }

    pub fn layout_style(&self, indices: StyleIndices) -> Option<&LayoutStyle> {
        self.owns(indices).then(|| {
            let layout_idx = self.box_styles[indices.box_idx as usize].layout_idx;
            &self.layout_styles[layout_idx as usize]
        })
    }

    pub fn memory_usage_report(&self) -> MemoryUsageReport {
        let mut report = MemoryUsageReport::new();
        report.add_slice_storage::<Font>("StyleStore.font_styles.storage", self.font_styles.capacity(), self.font_styles.len());
        report.add_slice_storage::<InheritedText>("StyleStore.text_styles.storage", self.text_styles.capacity(), self.text_styles.len());
        report.add_slice_storage::<BoxModel>("StyleStore.box_styles.storage", self.box_styles.capacity(), self.box_styles.len());
        report.add_slice_storage::<Border>("StyleStore.border_styles.storage", self.border_styles.capacity(), self.border_styles.len());
        report.add_slice_storage::<BorderRadii>("StyleStore.border_radii_styles.storage", self.border_radii_styles.capacity(), self.border_radii_styles.len());
        report.add_slice_storage::<Background>("StyleStore.bg_styles.storage", self.bg_styles.capacity(), self.bg_styles.len());
        report.add_slice_storage::<LayoutStyle>("StyleStore.layout_styles.storage", self.layout_styles.capacity(), self.layout_styles.len());
        report.add_slice_storage::<ComputedSizeExpression>("StyleStore.size_expressions.storage", self.size_expressions.capacity(), self.size_expressions.len());
        for (label, dedup) in [
            ("font", &self.font_dedup),
            ("text", &self.text_dedup),
            ("box", &self.box_dedup),
            ("border", &self.border_dedup),
            ("border_radii", &self.border_radii_dedup),
            ("background", &self.bg_dedup),
            ("layout", &self.layout_dedup),
            ("size_expression", &self.size_expression_dedup),
        ] {
            report.add_slice_storage::<(u64, u32)>(format!("StyleStore.{label}_dedup.storage"), dedup.indices.capacity(), dedup.indices.len());
            report.add_slice_storage::<(u64, Vec<u32>)>(format!("StyleStore.{label}_dedup_collisions.storage"), dedup.collisions.capacity(), dedup.collisions.len());
            for indices in dedup.collisions.values() {
                report.add_slice_storage::<u32>(format!("StyleStore.{label}_dedup_collisions.indices"), indices.capacity(), indices.len());
            }
        }
        report
    }
}

/// Style view - returned by get_style(), provides flat access to all fields
#[derive(Clone, Copy, Debug)]
pub struct StyleView<'a> {
    pub font: &'a Font,
    pub text: &'a InheritedText,
    pub box_model: &'a BoxModel,
    pub border: &'a Border,
    radii: &'a BorderRadii,
    pub background: &'a Background,
    pub layout: &'a LayoutStyle,
    size_expressions: &'a [ComputedSizeExpression],
}

/// A computed style paired with metrics for the font face selected during
/// shaping. Layout APIs use this type so unresolved `ex` values cannot cross
/// the stage boundary accidentally.
#[derive(Clone, Copy, Debug)]
pub struct UsedStyleView<'a> {
    computed: StyleView<'a>,
    font_relative: FontRelativeRatios,
    root_relative: RootRelativeLengths,
}

#[derive(Clone, Copy, Debug, Default)]
struct RootRelativeLengths {
    ch_px: f32,
    cap_height_px: f32,
    line_height_px: f32,
}

impl RootRelativeLengths {
    fn new(ch_px: f32, cap_height_px: f32, line_height_px: f32) -> Option<Self> {
        (ch_px.is_finite() && ch_px >= 0.0 && cap_height_px.is_finite() && cap_height_px >= 0.0 && line_height_px.is_finite() && line_height_px >= 0.0).then_some(Self { ch_px, cap_height_px, line_height_px })
    }
}

#[derive(Clone, Copy, Debug)]
struct FontRelativeRatios {
    x_height: f32,
    ch_advance: f32,
    cap_height: f32,
}

impl FontRelativeRatios {
    fn new(x_height: f32, ch_advance: f32, cap_height: f32) -> Option<Self> {
        (x_height.is_finite() && x_height > 0.0 && ch_advance.is_finite() && ch_advance >= 0.0 && cap_height.is_finite() && cap_height > 0.0).then_some(Self { x_height, ch_advance, cap_height })
    }
}

impl<'a> StyleView<'a> {
    // Font accessors
    pub fn font_size(&self) -> f32 {
        self.font.font_size
    }
    pub fn resolved_font_size(&self, x_height_ratio: f32, ch_advance_ratio: f32, cap_height_ratio: f32) -> Option<f32> {
        FontRelativeRatios::new(x_height_ratio, ch_advance_ratio, cap_height_ratio).map(|metrics| self.font.resolved_font_size(metrics, RootRelativeLengths::default()))
    }
    pub fn resolved_font_size_with_root(&self, x_height_ratio: f32, ch_advance_ratio: f32, cap_height_ratio: f32, root_ch_px: f32, root_cap_height_px: f32, root_line_height_px: f32) -> Option<f32> {
        Some(self.font.resolved_font_size(FontRelativeRatios::new(x_height_ratio, ch_advance_ratio, cap_height_ratio)?, RootRelativeLengths::new(root_ch_px, root_cap_height_px, root_line_height_px)?))
    }
    pub fn font_weight(&self) -> u16 {
        self.font.font_weight
    }
    pub fn font_style(&self) -> FontStyle {
        self.font.font_style
    }
    pub fn font_family(&self) -> Option<StyleStringId> {
        self.font.font_family
    }
    pub fn font_variant_small_caps(&self) -> bool {
        self.font.font_variant_caps_features.iter().any(|feature| feature.tag == *b"smcp" && feature.value != 0)
    }
    pub fn open_type_features(&self) -> Vec<OpenTypeFeature> {
        self.font.open_type_features()
    }

    // Text accessors
    pub fn color(&self) -> u32 {
        self.text.color
    }
    pub fn visibility(&self) -> Visibility {
        self.text.visibility
    }
    pub fn language(&self) -> Option<StyleStringId> {
        self.text.language
    }
    pub fn direction(&self) -> TextDirection {
        self.text.direction
    }
    pub fn quotes(&self) -> QuoteStyle {
        self.text.quotes
    }
    pub fn line_height(&self) -> f32 {
        self.text.line_height
    }
    pub fn resolved_line_height(&self, x_height_ratio: f32) -> Option<f32> {
        (x_height_ratio.is_finite() && x_height_ratio > 0.0).then_some(self.text.line_height + self.text.line_height_x_height_px * x_height_ratio)
    }
    pub fn line_height_is_normal(&self) -> bool {
        self.text.line_height_normal
    }
    pub fn letter_spacing(&self) -> f32 {
        self.text.letter_spacing.resolve(self.font.font_size)
    }
    pub fn word_spacing(&self) -> f32 {
        self.text.word_spacing.resolve(self.font.font_size)
    }
    pub fn tab_size(&self) -> TabSize {
        self.text.tab_size
    }
    pub fn text_align(&self) -> TextAlign {
        self.text.text_align
    }
    pub fn text_align_last(&self) -> TextAlign {
        self.text.text_align_last
    }
    pub fn text_indent(&self) -> LengthPct {
        self.text.text_indent
    }
    pub fn text_indent_hanging(&self) -> bool {
        self.text.text_indent_hanging
    }
    pub fn text_indent_each_line(&self) -> bool {
        self.text.text_indent_each_line
    }
    pub fn white_space(&self) -> WhiteSpace {
        self.text.white_space
    }
    pub fn hyphens(&self) -> Hyphens {
        self.text.hyphens
    }
    pub fn word_break(&self) -> WordBreak {
        self.text.word_break
    }
    pub fn overflow_wrap(&self) -> OverflowWrap {
        self.text.overflow_wrap
    }
    pub fn text_box_edge(&self) -> TextBoxEdge {
        self.text.text_box_edge
    }
    pub fn widows(&self) -> u8 {
        self.text.widows
    }
    pub fn orphans(&self) -> u8 {
        self.text.orphans
    }
    pub fn text_transform(&self) -> TextTransform {
        self.text.text_transform
    }
    pub fn list_style_type(&self) -> ListStyleType {
        self.text.list_style_type
    }
    pub fn list_style_position(&self) -> ListStylePosition {
        self.text.list_style_position
    }
    pub fn list_style_image(&self) -> Option<StyleStringId> {
        self.text.list_style_image
    }

    // Box model accessors
    pub fn display(&self) -> Display {
        self.box_model.display
    }
    pub fn box_sizing(&self) -> BoxSizing {
        self.box_model.box_sizing
    }
    pub fn overflow_x(&self) -> OverflowMode {
        self.box_model.overflow_x
    }
    pub fn overflow_y(&self) -> OverflowMode {
        self.box_model.overflow_y
    }
    pub fn text_overflow(&self) -> TextOverflow {
        self.box_model.text_overflow
    }
    pub fn text_box_trim(&self) -> TextBoxTrim {
        self.box_model.text_box_trim
    }
    pub fn size_containment(&self) -> bool {
        self.box_model.size_containment
    }
    pub fn vertical_align(&self) -> VerticalAlignValue {
        self.box_model.vertical_align
    }
    pub fn float(&self) -> Float {
        self.box_model.float
    }
    pub fn clear(&self) -> Clear {
        self.box_model.clear
    }
    pub fn table_layout(&self) -> TableLayoutMode {
        self.box_model.table_layout
    }
    pub fn border_collapse(&self) -> BorderCollapseMode {
        self.box_model.border_collapse
    }
    pub fn border_spacing_horizontal(&self) -> f32 {
        self.box_model.border_spacing_horizontal
    }
    pub fn border_spacing_vertical(&self) -> f32 {
        self.box_model.border_spacing_vertical
    }
    pub fn empty_cells(&self) -> EmptyCellsMode {
        self.box_model.empty_cells
    }
    pub fn caption_side(&self) -> CaptionSide {
        self.box_model.caption_side
    }
    pub fn width(&self) -> PreferredSize {
        self.box_model.width
    }
    pub fn height(&self) -> PreferredSize {
        self.box_model.height
    }
    pub fn min_width(&self) -> PreferredSize {
        self.box_model.min_width
    }
    pub fn min_height(&self) -> PreferredSize {
        self.box_model.min_height
    }
    pub fn max_width(&self) -> PreferredSize {
        self.box_model.max_width
    }
    pub fn max_height(&self) -> PreferredSize {
        self.box_model.max_height
    }
    pub fn aspect_ratio(&self) -> AspectRatio {
        self.box_model.aspect_ratio
    }
    pub fn object_fit(&self) -> ObjectFit {
        self.box_model.object_fit
    }
    pub fn object_position(&self) -> ObjectPosition {
        self.box_model.object_position
    }
    pub fn margin_top(&self) -> LengthPct {
        self.box_model.margin_top
    }
    pub fn margin_bottom(&self) -> LengthPct {
        self.box_model.margin_bottom
    }
    pub fn margin_left(&self) -> LengthPct {
        self.box_model.margin_left
    }
    pub fn margin_right(&self) -> LengthPct {
        self.box_model.margin_right
    }
    pub fn padding_top(&self) -> LengthPct {
        self.box_model.padding_top
    }
    pub fn padding_bottom(&self) -> LengthPct {
        self.box_model.padding_bottom
    }
    pub fn padding_left(&self) -> LengthPct {
        self.box_model.padding_left
    }
    pub fn padding_right(&self) -> LengthPct {
        self.box_model.padding_right
    }

    // Border accessors
    pub fn border_top_width(&self) -> FontRelativeLength {
        self.border.border_top_width
    }
    pub fn border_right_width(&self) -> FontRelativeLength {
        self.border.border_right_width
    }
    pub fn border_bottom_width(&self) -> FontRelativeLength {
        self.border.border_bottom_width
    }
    pub fn border_left_width(&self) -> FontRelativeLength {
        self.border.border_left_width
    }
    pub fn border_top_color(&self) -> u32 {
        self.border.border_top_color
    }
    pub fn border_right_color(&self) -> u32 {
        self.border.border_right_color
    }
    pub fn border_bottom_color(&self) -> u32 {
        self.border.border_bottom_color
    }
    pub fn border_left_color(&self) -> u32 {
        self.border.border_left_color
    }
    pub fn border_top_style(&self) -> BorderStyle {
        self.border.border_top_style
    }
    pub fn border_right_style(&self) -> BorderStyle {
        self.border.border_right_style
    }
    pub fn border_bottom_style(&self) -> BorderStyle {
        self.border.border_bottom_style
    }
    pub fn border_left_style(&self) -> BorderStyle {
        self.border.border_left_style
    }
    pub fn border_radii(&self) -> BorderRadii {
        *self.radii
    }

    // Background accessors
    pub fn background_color(&self) -> u32 {
        if self.background.background_color_current_color { self.text.color } else { self.background.background_color }
    }
    pub fn background_image_present(&self) -> bool {
        self.background.background_image_present
    }
    pub fn text_decoration(&self) -> TextDecoration {
        self.background.text_decoration
    }
    pub fn text_decoration_color(&self) -> u32 {
        self.background.text_decoration.color.resolve(self.text.color)
    }
    pub fn outline(&self) -> Outline {
        self.background.outline
    }
    pub fn outline_color(&self) -> u32 {
        self.background.outline.color.resolve(self.text.color)
    }

    pub fn should_parent_sibling_collapse(&self) -> bool {
        self.padding_top().is_zero() && self.padding_bottom().is_zero() && self.padding_left().is_zero() && self.padding_right().is_zero()
    }

    /// Whether converting this computed style to a layout-safe used style
    /// requires metrics from the selected font face.
    pub fn requires_font_metrics(&self) -> bool {
        let length = |value: LengthPct| match value {
            LengthPct::Ex(value) | LengthPct::Ch(value) | LengthPct::Cap(value) => value != 0.0,
            LengthPct::Calc { x_height_px, ch_advance_px, cap_height_px, .. } => x_height_px != 0.0 || ch_advance_px != 0.0 || cap_height_px != 0.0,
            _ => false,
        };
        let preferred = |value: PreferredSize| match value {
            PreferredSize::Ex(value) | PreferredSize::Ch(value) | PreferredSize::Cap(value) => value != 0.0,
            PreferredSize::Calc { x_height_px, ch_advance_px, cap_height_px, .. } => x_height_px != 0.0 || ch_advance_px != 0.0 || cap_height_px != 0.0,
            PreferredSize::Comparison(id) => self
                .size_expressions
                .get(id.index as usize)
                .is_some_and(|expression| expression.values[..usize::from(expression.count)].iter().any(|value| value.x_height_px != 0.0 || value.ch_advance_px != 0.0 || value.cap_height_px != 0.0)),
            _ => false,
        };
        let breadth = |value: GridTrackBreadth| matches!(value, GridTrackBreadth::Length(LengthPct::Ex(value) | LengthPct::Ch(value) | LengthPct::Cap(value)) if value != 0.0);
        let track = |value: GridTrackSize| match value {
            GridTrackSize::Breadth(value) => breadth(value),
            GridTrackSize::MinMax { min, max } => breadth(min) || breadth(max),
            GridTrackSize::FitContent(value) => length(value),
            GridTrackSize::Auto => false,
        };
        let template = |value: &GridTemplateTrack| match value {
            GridTemplateTrack::Single(value) => track(*value),
            GridTemplateTrack::Repeat { tracks, .. } => tracks.iter().copied().any(track),
        };
        let radii = [self.radii.top_left, self.radii.top_right, self.radii.bottom_right, self.radii.bottom_left].into_iter().any(|radius| length(radius.x) || length(radius.y));
        let decoration = matches!(self.background.text_decoration.thickness, TextDecorationThickness::Length(value) if length(value));

        !matches!(self.text.text_box_edge.over, TextBoxOverEdge::Text)
            || self.font.font_size_x_height_px != 0.0
            || self.font.font_size_ch_advance_px != 0.0
            || self.font.font_size_cap_height_px != 0.0
            || self.font.font_size_root_ch != 0.0
            || self.font.font_size_root_cap_height != 0.0
            || self.font.font_size_root_line_height != 0.0
            || self.text.line_height_x_height_px != 0.0
            || matches!(self.box_model.vertical_align, VerticalAlignValue::Calc { x_height_px, .. } if x_height_px != 0.0)
            || [
                self.text.text_indent,
                self.box_model.margin_top,
                self.box_model.margin_right,
                self.box_model.margin_bottom,
                self.box_model.margin_left,
                self.box_model.padding_top,
                self.box_model.padding_right,
                self.box_model.padding_bottom,
                self.box_model.padding_left,
                self.layout.row_gap,
                self.layout.column_gap,
            ]
            .into_iter()
            .any(length)
            || [self.box_model.width, self.box_model.height, self.box_model.min_width, self.box_model.min_height, self.box_model.max_width, self.box_model.max_height, self.layout.flex_basis].into_iter().any(preferred)
            || [self.layout.inset_top, self.layout.inset_right, self.layout.inset_bottom, self.layout.inset_left].into_iter().flatten().any(length)
            || [self.border.border_top_width, self.border.border_right_width, self.border.border_bottom_width, self.border.border_left_width].into_iter().any(FontRelativeLength::requires_font_metrics)
            || radii
            || decoration
            || self.background.outline.width.requires_font_metrics()
            || length(self.background.outline.offset)
            || self.layout.grid_template_rows.iter().chain(&self.layout.grid_template_columns).any(template)
            || self.layout.grid_auto_rows.iter().chain(&self.layout.grid_auto_columns).copied().any(track)
    }
}

fn used_border_width(style: BorderStyle, width: f32) -> f32 {
    if matches!(style, BorderStyle::None | BorderStyle::Hidden) { 0.0 } else { width }
}

impl<'a> UsedStyleView<'a> {
    pub fn font_size(&self) -> f32 {
        self.computed.font.resolved_font_size(self.font_relative, self.root_relative)
    }
    pub fn font_weight(&self) -> u16 {
        self.computed.font_weight()
    }
    pub fn font_style(&self) -> FontStyle {
        self.computed.font_style()
    }
    pub fn font_family(&self) -> Option<StyleStringId> {
        self.computed.font_family()
    }
    pub fn font_variant_small_caps(&self) -> bool {
        self.computed.font_variant_small_caps()
    }
    pub fn color(&self) -> u32 {
        self.computed.color()
    }
    pub fn visibility(&self) -> Visibility {
        self.computed.visibility()
    }
    pub fn language(&self) -> Option<StyleStringId> {
        self.computed.language()
    }
    pub fn direction(&self) -> TextDirection {
        self.computed.text.direction
    }
    pub fn line_height(&self) -> f32 {
        self.computed.text.line_height + self.computed.text.line_height_x_height_px * self.font_relative.x_height
    }
    pub fn line_height_is_normal(&self) -> bool {
        self.computed.line_height_is_normal()
    }
    pub fn letter_spacing(&self) -> f32 {
        self.computed.letter_spacing()
    }
    pub fn word_spacing(&self) -> f32 {
        self.computed.word_spacing()
    }
    pub fn tab_size(&self) -> TabSize {
        self.computed.tab_size()
    }
    pub fn text_align(&self) -> TextAlign {
        self.computed.text_align()
    }
    pub fn text_align_last(&self) -> TextAlign {
        self.computed.text_align_last()
    }
    pub fn text_indent_hanging(&self) -> bool {
        self.computed.text_indent_hanging()
    }
    pub fn text_indent_each_line(&self) -> bool {
        self.computed.text_indent_each_line()
    }
    pub fn white_space(&self) -> WhiteSpace {
        self.computed.white_space()
    }
    pub fn hyphens(&self) -> Hyphens {
        self.computed.hyphens()
    }
    pub fn word_break(&self) -> WordBreak {
        self.computed.word_break()
    }
    pub fn overflow_wrap(&self) -> OverflowWrap {
        self.computed.overflow_wrap()
    }
    pub fn text_box_edge(&self) -> TextBoxEdge {
        self.computed.text_box_edge()
    }
    pub fn widows(&self) -> u8 {
        self.computed.widows()
    }
    pub fn orphans(&self) -> u8 {
        self.computed.orphans()
    }
    pub fn text_transform(&self) -> TextTransform {
        self.computed.text_transform()
    }
    pub fn list_style_type(&self) -> ListStyleType {
        self.computed.list_style_type()
    }
    pub fn list_style_position(&self) -> ListStylePosition {
        self.computed.list_style_position()
    }
    pub fn list_style_image(&self) -> Option<StyleStringId> {
        self.computed.list_style_image()
    }
    pub fn display(&self) -> Display {
        self.computed.display()
    }
    pub fn box_sizing(&self) -> BoxSizing {
        self.computed.box_sizing()
    }
    pub fn overflow_x(&self) -> OverflowMode {
        self.computed.overflow_x()
    }
    pub fn overflow_y(&self) -> OverflowMode {
        self.computed.overflow_y()
    }
    pub fn text_overflow(&self) -> TextOverflow {
        self.computed.text_overflow()
    }
    pub fn text_box_trim(&self) -> TextBoxTrim {
        self.computed.text_box_trim()
    }
    pub fn size_containment(&self) -> bool {
        self.computed.size_containment()
    }
    pub fn vertical_align(&self) -> VerticalAlignValue {
        match self.computed.vertical_align() {
            VerticalAlignValue::Calc { absolute_px, line_height_fraction, x_height_px } => VerticalAlignValue::Calc { absolute_px: absolute_px + x_height_px * self.font_relative.x_height, line_height_fraction, x_height_px: 0.0 },
            value => value,
        }
    }
    pub fn float(&self) -> Float {
        self.computed.float()
    }
    pub fn clear(&self) -> Clear {
        self.computed.clear()
    }
    pub fn table_layout(&self) -> TableLayoutMode {
        self.computed.table_layout()
    }
    pub fn border_collapse(&self) -> BorderCollapseMode {
        self.computed.border_collapse()
    }
    pub fn border_spacing_horizontal(&self) -> f32 {
        self.computed.border_spacing_horizontal()
    }
    pub fn border_spacing_vertical(&self) -> f32 {
        self.computed.border_spacing_vertical()
    }
    pub fn empty_cells(&self) -> EmptyCellsMode {
        self.computed.empty_cells()
    }
    pub fn caption_side(&self) -> CaptionSide {
        self.computed.caption_side()
    }
    pub fn position(&self) -> PositionMode {
        self.computed.layout.position
    }
    pub fn break_before(&self) -> BreakBetween {
        self.computed.layout.break_before
    }
    pub fn break_after(&self) -> BreakBetween {
        self.computed.layout.break_after
    }
    pub fn break_inside(&self) -> BreakInside {
        self.computed.layout.break_inside
    }
    pub fn z_index(&self) -> Option<i32> {
        self.computed.layout.z_index
    }
    pub fn margin_top_auto(&self) -> bool {
        self.computed.layout.margin_top_auto
    }
    pub fn margin_right_auto(&self) -> bool {
        self.computed.layout.margin_right_auto
    }
    pub fn margin_bottom_auto(&self) -> bool {
        self.computed.layout.margin_bottom_auto
    }
    pub fn margin_left_auto(&self) -> bool {
        self.computed.layout.margin_left_auto
    }
    pub fn aspect_ratio(&self) -> AspectRatio {
        self.computed.aspect_ratio()
    }
    pub fn border_top_color(&self) -> u32 {
        self.computed.border_top_color()
    }
    pub fn border_right_color(&self) -> u32 {
        self.computed.border_right_color()
    }
    pub fn border_bottom_color(&self) -> u32 {
        self.computed.border_bottom_color()
    }
    pub fn border_left_color(&self) -> u32 {
        self.computed.border_left_color()
    }
    pub fn border_top_style(&self) -> BorderStyle {
        self.computed.border_top_style()
    }
    pub fn border_right_style(&self) -> BorderStyle {
        self.computed.border_right_style()
    }
    pub fn border_bottom_style(&self) -> BorderStyle {
        self.computed.border_bottom_style()
    }
    pub fn border_left_style(&self) -> BorderStyle {
        self.computed.border_left_style()
    }
    pub fn background_color(&self) -> u32 {
        self.computed.background_color()
    }
    pub fn background_image_present(&self) -> bool {
        self.computed.background_image_present()
    }
    pub fn text_decoration_color(&self) -> u32 {
        self.computed.text_decoration_color()
    }
    pub fn outline(&self) -> UsedOutline {
        let outline = self.computed.outline();
        UsedOutline {
            width: outline.width.resolve(self.font_relative),
            offset: if outline.offset_inset { -outline.width.resolve(self.font_relative) } else { outline.offset.resolve_font_relative(self.font_relative).resolve(0.0) as f32 },
            style: outline.style,
            color: outline.color,
        }
    }
    pub fn outline_color(&self) -> u32 {
        self.computed.outline_color()
    }
    pub fn x_height_ratio(&self) -> f32 {
        self.font_relative.x_height
    }
    pub fn width(&self) -> UsedPreferredSize {
        self.computed.box_model.width.resolve_font_relative(self.font_relative, self.computed.size_expressions)
    }
    pub fn height(&self) -> UsedPreferredSize {
        self.computed.box_model.height.resolve_font_relative(self.font_relative, self.computed.size_expressions)
    }
    pub fn min_width(&self) -> UsedPreferredSize {
        self.computed.box_model.min_width.resolve_font_relative(self.font_relative, self.computed.size_expressions)
    }
    pub fn min_height(&self) -> UsedPreferredSize {
        self.computed.box_model.min_height.resolve_font_relative(self.font_relative, self.computed.size_expressions)
    }
    pub fn max_width(&self) -> UsedPreferredSize {
        self.computed.box_model.max_width.resolve_font_relative(self.font_relative, self.computed.size_expressions)
    }
    pub fn max_height(&self) -> UsedPreferredSize {
        self.computed.box_model.max_height.resolve_font_relative(self.font_relative, self.computed.size_expressions)
    }
    pub fn object_fit(&self) -> ObjectFit {
        self.computed.box_model.object_fit
    }
    pub fn object_position(&self) -> UsedObjectPosition {
        UsedObjectPosition {
            x: UsedObjectPositionAxis { origin: self.computed.box_model.object_position.x.origin, offset: self.computed.box_model.object_position.x.offset.resolve_font_relative(self.font_relative) },
            y: UsedObjectPositionAxis { origin: self.computed.box_model.object_position.y.origin, offset: self.computed.box_model.object_position.y.offset.resolve_font_relative(self.font_relative) },
        }
    }
    pub fn text_indent(&self) -> UsedLengthPct {
        self.computed.text.text_indent.resolve_font_relative(self.font_relative)
    }
    pub fn margin_top(&self) -> UsedLengthPct {
        self.computed.box_model.margin_top.resolve_font_relative(self.font_relative)
    }
    pub fn margin_right(&self) -> UsedLengthPct {
        self.computed.box_model.margin_right.resolve_font_relative(self.font_relative)
    }
    pub fn margin_bottom(&self) -> UsedLengthPct {
        self.computed.box_model.margin_bottom.resolve_font_relative(self.font_relative)
    }
    pub fn margin_left(&self) -> UsedLengthPct {
        self.computed.box_model.margin_left.resolve_font_relative(self.font_relative)
    }
    pub fn padding_top(&self) -> UsedLengthPct {
        self.computed.box_model.padding_top.resolve_font_relative(self.font_relative)
    }
    pub fn padding_right(&self) -> UsedLengthPct {
        self.computed.box_model.padding_right.resolve_font_relative(self.font_relative)
    }
    pub fn padding_bottom(&self) -> UsedLengthPct {
        self.computed.box_model.padding_bottom.resolve_font_relative(self.font_relative)
    }
    pub fn padding_left(&self) -> UsedLengthPct {
        self.computed.box_model.padding_left.resolve_font_relative(self.font_relative)
    }
    pub fn border_top_width(&self) -> f32 {
        used_border_width(self.computed.border.border_top_style, self.computed.border.border_top_width.resolve(self.font_relative))
    }
    pub fn border_right_width(&self) -> f32 {
        used_border_width(self.computed.border.border_right_style, self.computed.border.border_right_width.resolve(self.font_relative))
    }
    pub fn border_bottom_width(&self) -> f32 {
        used_border_width(self.computed.border.border_bottom_style, self.computed.border.border_bottom_width.resolve(self.font_relative))
    }
    pub fn border_left_width(&self) -> f32 {
        used_border_width(self.computed.border.border_left_style, self.computed.border.border_left_width.resolve(self.font_relative))
    }
    pub fn border_radii(&self) -> UsedBorderRadiiSpec {
        (*self.computed.radii).resolve_font_relative(self.font_relative)
    }
    pub fn text_decoration(&self) -> UsedTextDecoration {
        self.computed.background.text_decoration.resolve_font_relative(self.font_relative)
    }
    pub fn grid_template_rows(&self) -> impl ExactSizeIterator<Item = UsedGridTemplateTrack<'a>> + 'a {
        let tracks: &'a [GridTemplateTrack] = &self.computed.layout.grid_template_rows;
        let metrics = self.font_relative;
        tracks.iter().map(move |track| track.resolve_font_relative(metrics))
    }
    pub fn grid_template_columns(&self) -> impl ExactSizeIterator<Item = UsedGridTemplateTrack<'a>> + 'a {
        let tracks: &'a [GridTemplateTrack] = &self.computed.layout.grid_template_columns;
        let metrics = self.font_relative;
        tracks.iter().map(move |track| track.resolve_font_relative(metrics))
    }
    pub fn grid_auto_rows(&self) -> impl ExactSizeIterator<Item = UsedGridTrackSize> + 'a {
        let tracks: &'a [GridTrackSize] = &self.computed.layout.grid_auto_rows;
        let metrics = self.font_relative;
        tracks.iter().copied().map(move |track| track.resolve_font_relative(metrics))
    }
    pub fn grid_auto_columns(&self) -> impl ExactSizeIterator<Item = UsedGridTrackSize> + 'a {
        let tracks: &'a [GridTrackSize] = &self.computed.layout.grid_auto_columns;
        let metrics = self.font_relative;
        tracks.iter().copied().map(move |track| track.resolve_font_relative(metrics))
    }
    pub fn flex_basis(&self) -> UsedPreferredSize {
        self.computed.layout.flex_basis.resolve_font_relative(self.font_relative, self.computed.size_expressions)
    }
    pub fn row_gap(&self) -> UsedLengthPct {
        self.computed.layout.row_gap.resolve_font_relative(self.font_relative)
    }
    pub fn column_gap(&self) -> UsedLengthPct {
        self.computed.layout.column_gap.resolve_font_relative(self.font_relative)
    }
    pub fn inset_top(&self) -> Option<UsedLengthPct> {
        self.computed.layout.inset_top.map(|value| value.resolve_font_relative(self.font_relative))
    }
    pub fn inset_right(&self) -> Option<UsedLengthPct> {
        self.computed.layout.inset_right.map(|value| value.resolve_font_relative(self.font_relative))
    }
    pub fn inset_bottom(&self) -> Option<UsedLengthPct> {
        self.computed.layout.inset_bottom.map(|value| value.resolve_font_relative(self.font_relative))
    }
    pub fn inset_left(&self) -> Option<UsedLengthPct> {
        self.computed.layout.inset_left.map(|value| value.resolve_font_relative(self.font_relative))
    }

    pub fn get_horizontal_margin_padding(&self, cb_width: f64) -> f64 {
        self.get_horizontal_margin(cb_width) + self.get_horizontal_padding(cb_width)
    }
    pub fn get_horizontal_margin(&self, cb_width: f64) -> f64 {
        self.margin_left().resolve(cb_width) + self.margin_right().resolve(cb_width)
    }
    pub fn get_vertical_margin_padding(&self, cb_width: f64) -> f64 {
        self.margin_top().resolve(cb_width) + self.margin_bottom().resolve(cb_width) + self.padding_top().resolve(cb_width) + self.padding_bottom().resolve(cb_width)
    }
    pub fn get_horizontal_padding(&self, cb_width: f64) -> f64 {
        self.padding_left().resolve(cb_width).max(0.0) + self.padding_right().resolve(cb_width).max(0.0)
    }
    pub fn get_vertical_padding(&self, cb_width: f64) -> f64 {
        self.padding_top().resolve(cb_width).max(0.0) + self.padding_bottom().resolve(cb_width).max(0.0)
    }
    pub fn get_top_left_padding(&self, cb_width: f64) -> Vec2 {
        Vec2::new(self.padding_left().resolve(cb_width), self.padding_top().resolve(cb_width))
    }
    pub fn should_parent_sibling_collapse(&self) -> bool {
        self.padding_top().is_zero() && self.padding_bottom().is_zero() && self.padding_left().is_zero() && self.padding_right().is_zero()
    }
}

// Default implementations for sub-structs
impl Default for Font {
    fn default() -> Self {
        Self {
            font_size: 16.0,
            font_size_x_height_px: 0.0,
            font_size_ch_advance_px: 0.0,
            font_size_cap_height_px: 0.0,
            font_size_root_ch: 0.0,
            font_size_root_cap_height: 0.0,
            font_size_root_line_height: 0.0,
            font_weight: 400,
            font_style: FontStyle::Normal,
            font_family: None,
            font_variant_caps_features: Vec::new(),
            font_variant_numeric_features: Vec::new(),
            font_variant_ligature_features: Vec::new(),
            font_kerning_features: Vec::new(),
            font_feature_settings: Vec::new(),
        }
    }
}

impl Default for InheritedText {
    fn default() -> Self {
        Self {
            color: 0x000000FF,
            visibility: Visibility::Visible,
            language: None,
            direction: TextDirection::Ltr,
            line_height: 0.0,
            line_height_x_height_px: 0.0,
            line_height_normal: true,
            line_height_number: 0.0,
            letter_spacing: TextSpacing::ZERO,
            word_spacing: TextSpacing::ZERO,
            tab_size: TabSize::DEFAULT,
            text_align: TextAlign::Left,
            text_align_logical: LogicalTextAlign::Start,
            text_align_last: TextAlign::Left,
            text_align_last_logical: LogicalTextAlign::Start,
            text_align_last_explicit: false,
            text_indent: LengthPct::Px(0.0),
            text_indent_hanging: false,
            text_indent_each_line: false,
            white_space: WhiteSpace::Normal,
            hyphens: Hyphens::Manual,
            word_break: WordBreak::Normal,
            overflow_wrap: OverflowWrap::Normal,
            text_box_edge: TextBoxEdge::default(),
            widows: 2,
            orphans: 2,
            text_transform: TextTransform::None,
            quotes: QuoteStyle::Auto,
            list_style_type: ListStyleType::Disc,
            list_style_position: ListStylePosition::Outside,
            list_style_image: None,
        }
    }
}

impl Default for BoxModel {
    fn default() -> Self {
        Self {
            layout_idx: 0,
            display: Display::Block,
            box_sizing: BoxSizing::ContentBox,
            overflow_x: OverflowMode::Visible,
            overflow_y: OverflowMode::Visible,
            text_overflow: TextOverflow::Clip,
            text_box_trim: TextBoxTrim::None,
            size_containment: false,
            vertical_align: VerticalAlignValue::Baseline,
            float: Float::None,
            clear: Clear::None,
            table_layout: TableLayoutMode::Auto,
            border_collapse: BorderCollapseMode::Separate,
            border_spacing_horizontal: 0.0,
            border_spacing_vertical: 0.0,
            empty_cells: EmptyCellsMode::Show,
            caption_side: CaptionSide::Top,
            width: PreferredSize::Auto,
            height: PreferredSize::Auto,
            min_width: PreferredSize::Auto,
            min_height: PreferredSize::Auto,
            max_width: PreferredSize::Auto,
            max_height: PreferredSize::Auto,
            aspect_ratio: AspectRatio::AUTO,
            object_fit: ObjectFit::Fill,
            object_position: ObjectPosition::default(),
            margin_top: LengthPct::Px(0.0),
            margin_bottom: LengthPct::Px(0.0),
            margin_left: LengthPct::Px(0.0),
            margin_right: LengthPct::Px(0.0),
            padding_top: LengthPct::Px(0.0),
            padding_bottom: LengthPct::Px(0.0),
            padding_left: LengthPct::Px(0.0),
            padding_right: LengthPct::Px(0.0),
        }
    }
}

impl Default for LayoutStyle {
    fn default() -> Self {
        Self {
            margin_top_auto: false,
            margin_right_auto: false,
            margin_bottom_auto: false,
            margin_left_auto: false,
            position: PositionMode::Static,
            z_index: None,
            inset_top: None,
            inset_right: None,
            inset_bottom: None,
            inset_left: None,
            break_before: BreakBetween::Auto,
            break_after: BreakBetween::Auto,
            break_inside: BreakInside::Auto,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: PreferredSize::Auto,
            order: 0,
            align_content: ContentAlignment::Normal,
            justify_content: ContentAlignment::Normal,
            align_items: ItemAlignment::Normal,
            align_self: ItemAlignment::Auto,
            justify_items: ItemAlignment::Normal,
            justify_self: ItemAlignment::Auto,
            row_gap: LengthPct::Px(0.0),
            column_gap: LengthPct::Px(0.0),
            grid_template_rows: Vec::new(),
            grid_template_columns: Vec::new(),
            grid_template_row_names: Vec::new(),
            grid_template_column_names: Vec::new(),
            grid_template_areas: Vec::new(),
            grid_template_area_rows: 0,
            grid_template_area_columns: 0,
            grid_auto_rows: Vec::new(),
            grid_auto_columns: Vec::new(),
            grid_auto_flow: GridAutoFlow::Row,
            grid_row: GridPlacementRange::default(),
            grid_column: GridPlacementRange::default(),
        }
    }
}

impl Default for Border {
    fn default() -> Self {
        Self {
            radii_idx: 0,
            current_color_sides: 0,
            border_top_width: FontRelativeLength::ZERO,
            border_right_width: FontRelativeLength::ZERO,
            border_bottom_width: FontRelativeLength::ZERO,
            border_left_width: FontRelativeLength::ZERO,
            border_top_color: 0x000000FF,
            border_right_color: 0x000000FF,
            border_bottom_color: 0x000000FF,
            border_left_color: 0x000000FF,
            border_top_style: BorderStyle::None,
            border_right_style: BorderStyle::None,
            border_bottom_style: BorderStyle::None,
            border_left_style: BorderStyle::None,
        }
    }
}

// Default constants for reset groups (used by anonymous boxes)
pub const DEFAULT_BOX_MODEL: BoxModel = BoxModel {
    layout_idx: 0,
    display: Display::Block,
    box_sizing: BoxSizing::ContentBox,
    overflow_x: OverflowMode::Visible,
    overflow_y: OverflowMode::Visible,
    text_overflow: TextOverflow::Clip,
    text_box_trim: TextBoxTrim::None,
    size_containment: false,
    vertical_align: VerticalAlignValue::Baseline,
    float: Float::None,
    clear: Clear::None,
    table_layout: TableLayoutMode::Auto,
    border_collapse: BorderCollapseMode::Separate,
    border_spacing_horizontal: 0.0,
    border_spacing_vertical: 0.0,
    empty_cells: EmptyCellsMode::Show,
    caption_side: CaptionSide::Top,
    width: PreferredSize::Auto,
    height: PreferredSize::Auto,
    min_width: PreferredSize::Auto,
    min_height: PreferredSize::Auto,
    max_width: PreferredSize::Auto,
    max_height: PreferredSize::Auto,
    aspect_ratio: AspectRatio::AUTO,
    object_fit: ObjectFit::Fill,
    object_position: ObjectPosition { x: ObjectPositionAxis { origin: ObjectPositionOrigin::Start, offset: LengthPct::Pct(0.5) }, y: ObjectPositionAxis { origin: ObjectPositionOrigin::Start, offset: LengthPct::Pct(0.5) } },
    margin_top: LengthPct::Px(0.0),
    margin_bottom: LengthPct::Px(0.0),
    margin_left: LengthPct::Px(0.0),
    margin_right: LengthPct::Px(0.0),
    padding_top: LengthPct::Px(0.0),
    padding_bottom: LengthPct::Px(0.0),
    padding_left: LengthPct::Px(0.0),
    padding_right: LengthPct::Px(0.0),
};

pub const DEFAULT_BORDER: Border = Border {
    radii_idx: 0,
    current_color_sides: 0,
    border_top_width: FontRelativeLength::ZERO,
    border_right_width: FontRelativeLength::ZERO,
    border_bottom_width: FontRelativeLength::ZERO,
    border_left_width: FontRelativeLength::ZERO,
    border_top_color: 0x000000FF,
    border_right_color: 0x000000FF,
    border_bottom_color: 0x000000FF,
    border_left_color: 0x000000FF,
    border_top_style: BorderStyle::None,
    border_right_style: BorderStyle::None,
    border_bottom_style: BorderStyle::None,
    border_left_style: BorderStyle::None,
};

pub const DEFAULT_BACKGROUND: Background = Background {
    background_color: 0x00000000,
    background_color_current_color: false,
    background_image_present: false,
    text_decoration: TextDecoration { lines: TextDecorationLines(0), style: TextDecorationStyle::Solid, color: DecorationColor::CurrentColor, thickness: TextDecorationThickness::Auto },
    outline: Outline { width: FontRelativeLength(3.0), offset: LengthPct::Px(0.0), offset_inset: false, style: BorderStyle::None, color: DecorationColor::CurrentColor },
};

/// A box-model length (margin, padding, or text-indent) that may be a fixed
/// pixel value or a percentage of the containing block's inline-size.
///
/// CSS resolves percentage margins/padding/text-indent against the containing
/// block *width* — even for the top/bottom edges (CSS 2.1 §8.3, §8.4) — which is
/// not known during style resolution. So these keep this form after the cascade
/// and are resolved to used values at the shaping boundary via
/// [`LengthPct::resolve_font_relative`]. Pixel
/// values (including resolved `em`/`rem`, which are font-relative, not
/// container-relative) are stored directly as `Px`.
#[derive(Clone, Copy, Debug)]
pub enum LengthPct {
    Px(f32),
    /// Fraction of the containing block inline-size (`0.05` == `5%`).
    Pct(f32),
    /// CSS `ex` count multiplied by the computed font size. Shaping resolves
    /// this once it knows the selected face's x-height.
    Ex(f32),
    /// CSS `ch` count multiplied by the computed font size. Shaping resolves
    /// this once it knows the selected face's ZERO glyph advance.
    Ch(f32),
    /// CSS `cap` count multiplied by the computed font size. Shaping resolves
    /// this once it knows the selected face's cap height.
    Cap(f32),
    /// A linear `<length-percentage>` such as `calc(50% - 3px)`.
    /// The percentage remains unresolved until layout supplies its basis.
    Calc {
        absolute_px: f32,
        percentage: f32,
        x_height_px: f32,
        ch_advance_px: f32,
        cap_height_px: f32,
        percentage_dependent: bool,
    },
}

impl LengthPct {
    /// True when this contributes no length regardless of containing-block width.
    /// Used by margin-collapsing edge checks, which only care about zero-ness.
    pub fn is_zero(self) -> bool {
        match self {
            LengthPct::Px(px) => px == 0.0,
            LengthPct::Pct(fraction) => fraction == 0.0,
            LengthPct::Ex(em_px) => em_px == 0.0,
            LengthPct::Ch(em_px) => em_px == 0.0,
            LengthPct::Cap(em_px) => em_px == 0.0,
            LengthPct::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, .. } => absolute_px == 0.0 && percentage == 0.0 && x_height_px == 0.0 && ch_advance_px == 0.0 && cap_height_px == 0.0,
        }
    }

    fn resolve_font_relative(self, metrics: FontRelativeRatios) -> UsedLengthPct {
        match self {
            Self::Px(px) => UsedLengthPct::Px(px),
            Self::Pct(fraction) => UsedLengthPct::Pct(fraction),
            Self::Ex(em_px) => UsedLengthPct::Px(em_px * metrics.x_height),
            Self::Ch(em_px) => UsedLengthPct::Px(em_px * metrics.ch_advance),
            Self::Cap(em_px) => UsedLengthPct::Px(em_px * metrics.cap_height),
            Self::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent } => {
                UsedLengthPct::Calc { absolute_px: absolute_px + x_height_px * metrics.x_height + ch_advance_px * metrics.ch_advance + cap_height_px * metrics.cap_height, percentage, percentage_dependent }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsedObjectPositionAxis {
    pub origin: ObjectPositionOrigin,
    pub offset: UsedLengthPct,
}

impl UsedObjectPositionAxis {
    pub fn resolve(self, free_space: f64) -> f64 {
        let offset = self.offset.resolve(free_space);
        match self.origin {
            ObjectPositionOrigin::Start => offset,
            ObjectPositionOrigin::End => free_space - offset,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsedObjectPosition {
    pub x: UsedObjectPositionAxis,
    pub y: UsedObjectPositionAxis,
}

/// A length-percentage accepted by layout. Font-relative units cannot be
/// represented after the shaping boundary.
#[derive(Clone, Copy, Debug)]
pub enum UsedLengthPct {
    Px(f32),
    Pct(f32),
    Calc { absolute_px: f32, percentage: f32, percentage_dependent: bool },
}

impl Default for UsedLengthPct {
    fn default() -> Self {
        Self::Px(0.0)
    }
}

impl UsedLengthPct {
    pub fn resolve(self, cb_width: f64) -> f64 {
        match self {
            Self::Px(px) => px as f64,
            Self::Pct(fraction) => cb_width * fraction as f64,
            Self::Calc { absolute_px, percentage, .. } => absolute_px as f64 + cb_width * percentage as f64,
        }
    }

    pub fn is_zero(self) -> bool {
        match self {
            Self::Px(px) => px == 0.0,
            Self::Pct(fraction) => fraction == 0.0,
            Self::Calc { absolute_px, percentage, .. } => absolute_px == 0.0 && percentage == 0.0,
        }
    }

    pub fn has_percentage(self) -> bool {
        match self {
            Self::Px(_) => false,
            Self::Pct(_) => true,
            Self::Calc { percentage_dependent, .. } => percentage_dependent,
        }
    }
}

impl PartialEq for UsedLengthPct {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Px(a), Self::Px(b)) | (Self::Pct(a), Self::Pct(b)) => a.to_bits() == b.to_bits(),
            (Self::Calc { absolute_px: a_px, percentage: a_pct, percentage_dependent: a_dep }, Self::Calc { absolute_px: b_px, percentage: b_pct, percentage_dependent: b_dep }) => {
                a_px.to_bits() == b_px.to_bits() && a_pct.to_bits() == b_pct.to_bits() && a_dep == b_dep
            }
            _ => false,
        }
    }
}

impl Eq for UsedLengthPct {}

impl Hash for UsedLengthPct {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Px(value) | Self::Pct(value) => state.write_u32(value.to_bits()),
            Self::Calc { absolute_px, percentage, percentage_dependent } => {
                state.write_u32(absolute_px.to_bits());
                state.write_u32(percentage.to_bits());
                percentage_dependent.hash(state);
            }
        }
    }
}

impl Default for LengthPct {
    fn default() -> Self {
        LengthPct::Px(0.0)
    }
}

impl PartialEq for LengthPct {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (LengthPct::Px(a), LengthPct::Px(b)) => a.to_bits() == b.to_bits(),
            (LengthPct::Pct(a), LengthPct::Pct(b)) => a.to_bits() == b.to_bits(),
            (LengthPct::Ex(a), LengthPct::Ex(b)) => a.to_bits() == b.to_bits(),
            (LengthPct::Ch(a), LengthPct::Ch(b)) => a.to_bits() == b.to_bits(),
            (LengthPct::Cap(a), LengthPct::Cap(b)) => a.to_bits() == b.to_bits(),
            (
                LengthPct::Calc { absolute_px: a_px, percentage: a_pct, x_height_px: a_ex, ch_advance_px: a_ch, cap_height_px: a_cap, percentage_dependent: a_dep },
                LengthPct::Calc { absolute_px: b_px, percentage: b_pct, x_height_px: b_ex, ch_advance_px: b_ch, cap_height_px: b_cap, percentage_dependent: b_dep },
            ) => a_px.to_bits() == b_px.to_bits() && a_pct.to_bits() == b_pct.to_bits() && a_ex.to_bits() == b_ex.to_bits() && a_ch.to_bits() == b_ch.to_bits() && a_cap.to_bits() == b_cap.to_bits() && a_dep == b_dep,
            _ => false,
        }
    }
}
impl Eq for LengthPct {}
impl Hash for LengthPct {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            LengthPct::Px(v) | LengthPct::Pct(v) | LengthPct::Ex(v) | LengthPct::Ch(v) | LengthPct::Cap(v) => state.write_u32(v.to_bits()),
            LengthPct::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent } => {
                state.write_u32(absolute_px.to_bits());
                state.write_u32(percentage.to_bits());
                state.write_u32(x_height_px.to_bits());
                state.write_u32(ch_advance_px.to_bits());
                state.write_u32(cap_height_px.to_bits());
                percentage_dependent.hash(state);
            }
        }
    }
}

/// Compact non-negative font-relative length. The sign bit is the unit tag
/// (`+` = px, `-` = ex-scaled em pixels), so this stays one `f32` wide.
#[repr(transparent)]
#[derive(Clone, Copy, Default)]
pub struct FontRelativeLength(f32);

impl FontRelativeLength {
    pub const ZERO: Self = Self(0.0);

    pub fn px(value: f32) -> Option<Self> {
        (value.is_finite() && value >= 0.0).then_some(Self(value.abs()))
    }

    /// `value` is the authored ex count multiplied by computed font size.
    pub fn ex(value: f32) -> Option<Self> {
        (value.is_finite() && value >= 0.0).then_some(if value == 0.0 { Self::ZERO } else { Self(-value) })
    }

    fn resolve(self, metrics: FontRelativeRatios) -> f32 {
        if self.0.is_sign_negative() { -self.0 * metrics.x_height } else { self.0 }
    }

    fn requires_font_metrics(self) -> bool {
        self.0.is_sign_negative()
    }

    fn validate(self, property: &'static str, field: &'static str) -> Result<(), ComputedStyleValueError> {
        must_be_finite(property, field, self.0)
    }

    #[cfg(test)]
    fn invalid_for_test(value: f32) -> Self {
        Self(value)
    }
}

impl PartialEq for FontRelativeLength {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for FontRelativeLength {}

impl Hash for FontRelativeLength {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(self.0.to_bits());
    }
}

impl std::fmt::Debug for FontRelativeLength {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_sign_negative() { formatter.debug_tuple("Ex").field(&-self.0).finish() } else { formatter.debug_tuple("Px").field(&self.0).finish() }
    }
}

pub fn resolve_used_preferred_size(preferred_size: UsedPreferredSize, auto: f64, parent_size: f64) -> f64 {
    match preferred_size {
        UsedPreferredSize::Auto | UsedPreferredSize::MinContent | UsedPreferredSize::MaxContent | UsedPreferredSize::FitContent => auto,
        UsedPreferredSize::Stretch => parent_size,
        UsedPreferredSize::Px(px) => px as f64,
        UsedPreferredSize::Percent(pct) => parent_size * pct as f64,
        UsedPreferredSize::Calc { absolute_px, percentage, .. } => absolute_px as f64 + parent_size * percentage as f64,
        UsedPreferredSize::Comparison { kind, values, count } => {
            let resolve = |value: UsedSizeComponent| value.absolute_px as f64 + parent_size * value.percentage as f64;
            let values = &values[..usize::from(count)];
            match kind {
                SizeComparison::Min => values.iter().copied().map(resolve).reduce(f64::min).unwrap_or(auto),
                SizeComparison::Max => values.iter().copied().map(resolve).reduce(f64::max).unwrap_or(auto),
                SizeComparison::Clamp if values.len() == 3 => resolve(values[1]).clamp(resolve(values[0]), resolve(values[2])),
                SizeComparison::Clamp => auto,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    #[default]
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LogicalTextAlign {
    Physical,
    #[default]
    Start,
    End,
}

/// Computed value of CSS `direction` for horizontal writing mode.
///
/// Vertical writing modes are intentionally not represented: the pinned CSS
/// parser does not expose `writing-mode`, and the layout/text pipeline does not
/// implement vertical line construction. This keeps that unsupported state
/// from crossing the style boundary accidentally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextDirection {
    #[default]
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WhiteSpace {
    #[default]
    Normal,
    Pre,
    NoWrap,
    PreWrap,
    PreLine,
    BreakSpaces,
    PreserveBreaksNoWrap,
    BreakSpacesNoWrap,
}

impl WhiteSpace {
    pub fn allows_wrap(self) -> bool {
        !matches!(self, Self::Pre | Self::NoWrap | Self::PreserveBreaksNoWrap | Self::BreakSpacesNoWrap)
    }

    pub fn collapses_spaces(self) -> bool {
        matches!(self, Self::Normal | Self::NoWrap | Self::PreLine | Self::PreserveBreaksNoWrap)
    }

    pub fn preserves_newlines(self) -> bool {
        !matches!(self, Self::Normal | Self::NoWrap)
    }

    pub fn preserves_spaces(self) -> bool {
        matches!(self, Self::Pre | Self::PreWrap | Self::BreakSpaces | Self::BreakSpacesNoWrap)
    }
}

/// Computed `word-break`. The legacy `break-word` value behaves like
/// `overflow-wrap: break-word` and is mapped onto that property instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WordBreak {
    #[default]
    Normal,
    /// Allow a break between any two characters.
    BreakAll,
    /// Forbid breaks inside CJK "words". Treated like `Normal` here, since the
    /// default breaker already only breaks at spaces.
    KeepAll,
}

/// Computed value of CSS `hyphens`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Hyphens {
    None,
    #[default]
    Manual,
    Auto,
}

/// Computed `overflow-wrap` (and its legacy alias `word-wrap`). Both
/// non-normal values allow an emergency mid-word break when a word cannot fit
/// on a line by itself; the min-content sizing distinction between them is not
/// modeled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OverflowWrap {
    #[default]
    Normal,
    BreakWord,
    Anywhere,
}

/// Computed overflow behavior for one physical axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OverflowMode {
    #[default]
    Visible,
    Hidden,
    Clip,
    Scroll,
    Auto,
}

impl OverflowMode {
    /// Whether inline overflow is visually confined to the box in a static
    /// rendering. Scrollable modes also confine their viewport even though this
    /// renderer does not expose interactive scrolling controls.
    pub fn clips(self) -> bool {
        !matches!(self, Self::Visible)
    }

    fn computed_against(self, other: Self) -> Self {
        if matches!(other, Self::Visible | Self::Clip) {
            return self;
        }
        match self {
            Self::Visible => Self::Auto,
            _ => self,
        }
    }
}

/// Marker policy at the inline edge of clipped overflowing text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextOverflow {
    #[default]
    Clip,
    Ellipsis,
}

/// Computed `box-sizing`: whether `width`/`height` (and their min/max) specify
/// the content box or the border box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BoxSizing {
    #[default]
    ContentBox,
    BorderBox,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BorderStyle {
    #[default]
    None,
    Hidden,
    Solid,
    Dashed,
    Dotted,
    Groove,
    Ridge,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

/// Computed `list-style-type` for the marker systems the renderer formats
/// exactly. Other typed CSS values are rejected before this boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ListStyleType {
    None,
    #[default]
    Disc,
    Circle,
    Square,
    Decimal,
    DecimalLeadingZero,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

impl ListStyleType {
    /// True for the bullet glyph types whose marker text is fixed (no counter).
    pub fn is_bullet(self) -> bool {
        matches!(self, ListStyleType::Disc | ListStyleType::Circle | ListStyleType::Square)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ListStylePosition {
    #[default]
    Outside,
    Inside,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Display {
    #[default]
    Block,
    FlowRoot,
    Inline,
    InlineBlock,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    Contents,
    None,
    ListItem,
    FlowRootListItem,
    // Table display types
    Table,
    InlineTable,
    TableRowGroup,
    TableHeaderGroup,
    TableFooterGroup,
    TableRow,
    TableColumnGroup,
    TableColumn,
    TableCell,
    TableCaption,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

/// Renderer-owned alignment vocabulary. Parser-specific overflow-safety flags
/// are intentionally absent because they do not change static document layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ContentAlignment {
    #[default]
    Normal,
    Start,
    End,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ItemAlignment {
    #[default]
    Auto,
    Normal,
    Start,
    End,
    SelfStart,
    SelfEnd,
    FlexStart,
    FlexEnd,
    Left,
    Right,
    Center,
    Stretch,
    Baseline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GridAutoFlow {
    #[default]
    Row,
    Column,
    RowDense,
    ColumnDense,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GridRepeatCount {
    Count(NonZeroU16),
    AutoFill,
    AutoFit,
    #[default]
    Once,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum GridTrackBreadth {
    Length(LengthPct),
    Flex(f32),
    MinContent,
    MaxContent,
    #[default]
    Auto,
}

/// Grid breadth after font-relative lengths have been resolved by shaping.
/// Layout deliberately cannot observe the computed `Ex` variant.
#[derive(Clone, Copy, Debug, Default)]
pub enum UsedGridTrackBreadth {
    Length(UsedLengthPct),
    Flex(f32),
    MinContent,
    MaxContent,
    #[default]
    Auto,
}

impl UsedGridTrackBreadth {
    pub fn flex(self) -> Option<f32> {
        match self {
            Self::Flex(value) => Some(value),
            _ => None,
        }
    }
}

impl PartialEq for UsedGridTrackBreadth {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Length(a), Self::Length(b)) => a == b,
            (Self::Flex(a), Self::Flex(b)) => a.to_bits() == b.to_bits(),
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}
impl Eq for UsedGridTrackBreadth {}
impl Hash for UsedGridTrackBreadth {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Length(value) => value.hash(state),
            Self::Flex(value) => state.write_u32(value.to_bits()),
            _ => {}
        }
    }
}

impl PartialEq for GridTrackBreadth {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Length(a), Self::Length(b)) => a == b,
            (Self::Flex(a), Self::Flex(b)) => a.to_bits() == b.to_bits(),
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}
impl Eq for GridTrackBreadth {}
impl Hash for GridTrackBreadth {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Length(value) => value.hash(state),
            Self::Flex(value) => state.write_u32(value.to_bits()),
            _ => {}
        }
    }
}

impl GridTrackBreadth {
    fn resolve_font_relative(self, metrics: FontRelativeRatios) -> UsedGridTrackBreadth {
        match self {
            Self::Length(value) => UsedGridTrackBreadth::Length(value.resolve_font_relative(metrics)),
            Self::Flex(value) => UsedGridTrackBreadth::Flex(value),
            Self::MinContent => UsedGridTrackBreadth::MinContent,
            Self::MaxContent => UsedGridTrackBreadth::MaxContent,
            Self::Auto => UsedGridTrackBreadth::Auto,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GridTrackSize {
    #[default]
    Auto,
    Breadth(GridTrackBreadth),
    MinMax {
        min: GridTrackBreadth,
        max: GridTrackBreadth,
    },
    FitContent(LengthPct),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UsedGridTrackSize {
    #[default]
    Auto,
    Breadth(UsedGridTrackBreadth),
    MinMax {
        min: UsedGridTrackBreadth,
        max: UsedGridTrackBreadth,
    },
    FitContent(UsedLengthPct),
}

impl GridTrackSize {
    fn resolve_font_relative(self, metrics: FontRelativeRatios) -> UsedGridTrackSize {
        match self {
            Self::Auto => UsedGridTrackSize::Auto,
            Self::Breadth(value) => UsedGridTrackSize::Breadth(value.resolve_font_relative(metrics)),
            Self::MinMax { min, max } => UsedGridTrackSize::MinMax { min: min.resolve_font_relative(metrics), max: max.resolve_font_relative(metrics) },
            Self::FitContent(value) => UsedGridTrackSize::FitContent(value.resolve_font_relative(metrics)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GridTemplateTrack {
    Single(GridTrackSize),
    Repeat { count: GridRepeatCount, tracks: Vec<GridTrackSize>, line_names: Vec<Vec<StyleStringId>> },
}

#[derive(Clone, Debug)]
pub enum UsedGridTemplateTrack<'a> {
    Single(UsedGridTrackSize),
    Repeat { count: GridRepeatCount, tracks: UsedGridTrackList<'a>, line_names: &'a [Vec<StyleStringId>] },
}

#[derive(Clone, Copy, Debug)]
pub struct UsedGridTrackList<'a> {
    source: &'a [GridTrackSize],
    font_relative: FontRelativeRatios,
}

impl PartialEq for UsedGridTrackList<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}
impl Eq for UsedGridTrackList<'_> {}

impl PartialEq for UsedGridTemplateTrack<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Single(left), Self::Single(right)) => left == right,
            (Self::Repeat { count: left_count, tracks: left_tracks, line_names: left_names }, Self::Repeat { count: right_count, tracks: right_tracks, line_names: right_names }) => {
                left_count == right_count && left_tracks == right_tracks && left_names == right_names
            }
            _ => false,
        }
    }
}
impl Eq for UsedGridTemplateTrack<'_> {}

impl<'a> UsedGridTrackList<'a> {
    pub fn iter(self) -> impl ExactSizeIterator<Item = UsedGridTrackSize> + 'a {
        let metrics = self.font_relative;
        self.source.iter().copied().map(move |track| track.resolve_font_relative(metrics))
    }
}

impl GridTemplateTrack {
    fn resolve_font_relative(&self, metrics: FontRelativeRatios) -> UsedGridTemplateTrack<'_> {
        match self {
            Self::Single(size) => UsedGridTemplateTrack::Single(size.resolve_font_relative(metrics)),
            Self::Repeat { count, tracks, line_names } => UsedGridTemplateTrack::Repeat { count: *count, tracks: UsedGridTrackList { source: tracks, font_relative: metrics }, line_names },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GridTemplateArea {
    pub name: StyleStringId,
    pub row_start: u16,
    pub row_end: u16,
    pub column_start: u16,
    pub column_end: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GridPlacement {
    #[default]
    Auto,
    Line(NonZeroI16),
    NamedLine {
        name: StyleStringId,
        index: i16,
    },
    Span(NonZeroU16),
    NamedSpan {
        name: StyleStringId,
        count: NonZeroU16,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GridPlacementRange {
    pub start: GridPlacement,
    pub end: GridPlacement,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Float {
    #[default]
    None,
    Left,
    Right,
}

/// Positioning modes whose layout semantics are implemented by this renderer.
/// Unsupported fixed and sticky modes never cross the style boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PositionMode {
    #[default]
    Static,
    Relative,
    Absolute,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Clear {
    #[default]
    None,
    Left,
    Right,
    Both,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BorderCollapseMode {
    Collapse,
    #[default]
    Separate,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TableLayoutMode {
    #[default]
    Auto,
    Fixed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EmptyCellsMode {
    Hide,
    #[default]
    Show,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CaptionSide {
    Bottom,
    #[default]
    Top,
}

/// Computed `vertical-align` values exposed by Lightning CSS.
#[derive(Clone, Copy, Debug, Default)]
pub enum VerticalAlignValue {
    #[default]
    Baseline,
    Sub,
    Super,
    Top,
    TextTop,
    Middle,
    Bottom,
    TextBottom,
    Length(f32),
    Percent(f32),
    Calc {
        absolute_px: f32,
        line_height_fraction: f32,
        x_height_px: f32,
    },
}

impl PartialEq for VerticalAlignValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (VerticalAlignValue::Length(a), VerticalAlignValue::Length(b)) => a.to_bits() == b.to_bits(),
            (VerticalAlignValue::Percent(a), VerticalAlignValue::Percent(b)) => a.to_bits() == b.to_bits(),
            (VerticalAlignValue::Calc { absolute_px: a_px, line_height_fraction: a_fraction, x_height_px: a_x_height }, VerticalAlignValue::Calc { absolute_px: b_px, line_height_fraction: b_fraction, x_height_px: b_x_height }) => {
                a_px.to_bits() == b_px.to_bits() && a_fraction.to_bits() == b_fraction.to_bits() && a_x_height.to_bits() == b_x_height.to_bits()
            }
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}
impl Eq for VerticalAlignValue {}
impl Hash for VerticalAlignValue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            VerticalAlignValue::Length(v) | VerticalAlignValue::Percent(v) => state.write_u32(v.to_bits()),
            VerticalAlignValue::Calc { absolute_px, line_height_fraction, x_height_px } => {
                state.write_u32(absolute_px.to_bits());
                state.write_u32(line_height_fraction.to_bits());
                state.write_u32(x_height_px.to_bits());
            }
            _ => {}
        }
    }
}

impl VerticalAlignValue {
    pub fn is_initial(self) -> bool {
        self == Self::Baseline
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum PreferredSize {
    #[default]
    Auto,
    MinContent,
    MaxContent,
    FitContent,
    Stretch,
    Px(f32),
    Percent(f32),
    /// CSS `ex` count multiplied by the computed font size.
    Ex(f32),
    /// CSS `ch` count multiplied by the computed font size.
    Ch(f32),
    /// CSS `cap` count multiplied by the computed font size.
    Cap(f32),
    /// A linear size such as `calc(100% - 20px)`, resolved by layout.
    Calc {
        absolute_px: f32,
        percentage: f32,
        x_height_px: f32,
        ch_advance_px: f32,
        cap_height_px: f32,
        percentage_dependent: bool,
    },
    /// A CSS comparison function whose linear branches must be selected once
    /// the containing-block size is known.
    Comparison(SizeExpressionId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SizeExpressionId {
    store_id: NonZeroU32,
    index: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SizeComparison {
    #[default]
    Min,
    Max,
    Clamp,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ComputedSizeComponent {
    pub absolute_px: f32,
    pub percentage: f32,
    pub x_height_px: f32,
    pub ch_advance_px: f32,
    pub cap_height_px: f32,
    pub percentage_dependent: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ComputedSizeExpression {
    kind: SizeComparison,
    values: [ComputedSizeComponent; 3],
    count: u8,
}

impl ComputedSizeComponent {
    fn resolve_font_relative(self, metrics: FontRelativeRatios) -> UsedSizeComponent {
        UsedSizeComponent {
            absolute_px: self.absolute_px + self.x_height_px * metrics.x_height + self.ch_advance_px * metrics.ch_advance + self.cap_height_px * metrics.cap_height,
            percentage: self.percentage,
            percentage_dependent: self.percentage_dependent,
        }
    }
}

impl PartialEq for ComputedSizeComponent {
    fn eq(&self, other: &Self) -> bool {
        self.absolute_px.to_bits() == other.absolute_px.to_bits()
            && self.percentage.to_bits() == other.percentage.to_bits()
            && self.x_height_px.to_bits() == other.x_height_px.to_bits()
            && self.ch_advance_px.to_bits() == other.ch_advance_px.to_bits()
            && self.cap_height_px.to_bits() == other.cap_height_px.to_bits()
            && self.percentage_dependent == other.percentage_dependent
    }
}
impl Eq for ComputedSizeComponent {}
impl Hash for ComputedSizeComponent {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(self.absolute_px.to_bits());
        state.write_u32(self.percentage.to_bits());
        state.write_u32(self.x_height_px.to_bits());
        state.write_u32(self.ch_advance_px.to_bits());
        state.write_u32(self.cap_height_px.to_bits());
        self.percentage_dependent.hash(state);
    }
}

impl PreferredSize {
    fn resolve_font_relative(self, metrics: FontRelativeRatios, expressions: &[ComputedSizeExpression]) -> UsedPreferredSize {
        match self {
            Self::Auto => UsedPreferredSize::Auto,
            Self::MinContent => UsedPreferredSize::MinContent,
            Self::MaxContent => UsedPreferredSize::MaxContent,
            Self::FitContent => UsedPreferredSize::FitContent,
            Self::Stretch => UsedPreferredSize::Stretch,
            Self::Px(px) => UsedPreferredSize::Px(px),
            Self::Percent(percent) => UsedPreferredSize::Percent(percent),
            Self::Ex(em_px) => UsedPreferredSize::Px(em_px * metrics.x_height),
            Self::Ch(em_px) => UsedPreferredSize::Px(em_px * metrics.ch_advance),
            Self::Cap(em_px) => UsedPreferredSize::Px(em_px * metrics.cap_height),
            Self::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent } => {
                UsedPreferredSize::Calc { absolute_px: absolute_px + x_height_px * metrics.x_height + ch_advance_px * metrics.ch_advance + cap_height_px * metrics.cap_height, percentage, percentage_dependent }
            }
            Self::Comparison(id) => expressions.get(id.index as usize).map_or(UsedPreferredSize::Auto, |expression| UsedPreferredSize::Comparison {
                kind: expression.kind,
                values: expression.values.map(|value| value.resolve_font_relative(metrics)),
                count: expression.count,
            }),
        }
    }
}

/// Preferred size accepted by layout. Unlike [`PreferredSize`], it has no
/// unresolved font-relative variant.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum UsedPreferredSize {
    #[default]
    Auto,
    MinContent,
    MaxContent,
    FitContent,
    Stretch,
    Px(f32),
    Percent(f32),
    Calc {
        absolute_px: f32,
        percentage: f32,
        percentage_dependent: bool,
    },
    Comparison {
        kind: SizeComparison,
        values: [UsedSizeComponent; 3],
        count: u8,
    },
}

impl UsedPreferredSize {
    pub fn percentage_dependent(self) -> bool {
        match self {
            Self::Percent(_) => true,
            Self::Calc { percentage_dependent, .. } => percentage_dependent,
            Self::Comparison { values, count, .. } => values[..usize::from(count)].iter().any(|value| value.percentage_dependent),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UsedSizeComponent {
    pub absolute_px: f32,
    pub percentage: f32,
    pub percentage_dependent: bool,
}

impl PartialEq for PreferredSize {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (PreferredSize::Px(a), PreferredSize::Px(b)) => a.to_bits() == b.to_bits(),
            (PreferredSize::Percent(a), PreferredSize::Percent(b)) => a.to_bits() == b.to_bits(),
            (PreferredSize::Ex(a), PreferredSize::Ex(b)) => a.to_bits() == b.to_bits(),
            (PreferredSize::Ch(a), PreferredSize::Ch(b)) => a.to_bits() == b.to_bits(),
            (PreferredSize::Cap(a), PreferredSize::Cap(b)) => a.to_bits() == b.to_bits(),
            (
                PreferredSize::Calc { absolute_px: a_px, percentage: a_pct, x_height_px: a_ex, ch_advance_px: a_ch, cap_height_px: a_cap, percentage_dependent: a_dep },
                PreferredSize::Calc { absolute_px: b_px, percentage: b_pct, x_height_px: b_ex, ch_advance_px: b_ch, cap_height_px: b_cap, percentage_dependent: b_dep },
            ) => a_px.to_bits() == b_px.to_bits() && a_pct.to_bits() == b_pct.to_bits() && a_ex.to_bits() == b_ex.to_bits() && a_ch.to_bits() == b_ch.to_bits() && a_cap.to_bits() == b_cap.to_bits() && a_dep == b_dep,
            (PreferredSize::Comparison(a), PreferredSize::Comparison(b)) => a == b,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}
impl Eq for PreferredSize {}
impl Hash for PreferredSize {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            PreferredSize::Px(v) | PreferredSize::Percent(v) | PreferredSize::Ex(v) | PreferredSize::Ch(v) | PreferredSize::Cap(v) => state.write_u32(v.to_bits()),
            PreferredSize::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent } => {
                state.write_u32(absolute_px.to_bits());
                state.write_u32(percentage.to_bits());
                state.write_u32(x_height_px.to_bits());
                state.write_u32(ch_advance_px.to_bits());
                state.write_u32(cap_height_px.to_bits());
                percentage_dependent.hash(state);
            }
            PreferredSize::Comparison(id) => id.hash(state),
            PreferredSize::Auto | PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent | PreferredSize::Stretch => {}
        }
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::{ComputedStyleValueError, ComputedStylesBuilder, Font, InheritedText, StyleDedup, StyleIndices, StyleStringId, VerticalAlignValue, intern_style};
    use html_dom::Document;
    use std::hash::{Hash, Hasher};
    use std::mem::size_of;

    #[derive(Eq, PartialEq)]
    struct CollidingStyle(u32);

    impl Hash for CollidingStyle {
        fn hash<H: Hasher>(&self, state: &mut H) {
            0u8.hash(state);
        }
    }

    #[test]
    fn style_interner_resolves_hash_collisions_by_value() {
        let mut values = Vec::new();
        let mut dedup = StyleDedup::default();
        assert_eq!(intern_style(&mut values, &mut dedup, CollidingStyle(1)), 0);
        assert_eq!(intern_style(&mut values, &mut dedup, CollidingStyle(2)), 1);
        assert_eq!(intern_style(&mut values, &mut dedup, CollidingStyle(1)), 0);
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn style_handles_are_branded_by_their_store() {
        let document = Document::new();
        let mut first = ComputedStylesBuilder::new(&document);
        let second = ComputedStylesBuilder::new(&document);
        let foreign = first.default_indices();
        let foreign_string = first.intern_string("first store");

        assert!(!second.store.contains(foreign));
        assert!(second.view(foreign).is_none());
        assert!(second.font_style(foreign).is_none());
        assert!(second.string(foreign_string).is_none());
    }

    #[test]
    fn store_brand_uses_the_option_niche_without_growing_node_entries() {
        assert_eq!(size_of::<Option<StyleIndices>>(), size_of::<StyleIndices>());
        assert_eq!(size_of::<StyleIndices>(), 24, "adding layout modes must not add per-node style-handle storage");
        assert_eq!(size_of::<Option<StyleStringId>>(), size_of::<StyleStringId>());
        assert!(size_of::<VerticalAlignValue>() <= 12, "vertical alignment is stored per distinct box style and must remain compact");
        assert_eq!(size_of::<super::OverflowMode>(), 1);
        assert_eq!(size_of::<super::TextOverflow>(), 1);
        assert!(size_of::<super::Border>() <= 40, "optional corner radii must not inflate every distinct border style");
        assert_eq!(size_of::<super::FontRelativeLength>(), size_of::<f32>(), "font-relative border widths must remain compact");
    }

    #[test]
    fn font_relative_computed_values_cross_to_closed_used_value_types() {
        let metrics = super::FontRelativeRatios::new(0.8, 0.6, 0.7).unwrap();
        assert_eq!(super::LengthPct::Ex(40.0).resolve_font_relative(metrics), super::UsedLengthPct::Px(32.0));
        assert_eq!(super::LengthPct::Ch(40.0).resolve_font_relative(metrics), super::UsedLengthPct::Px(24.0));
        assert_eq!(super::PreferredSize::Ex(60.0).resolve_font_relative(metrics, &[]), super::UsedPreferredSize::Px(48.0));
        assert_eq!(super::PreferredSize::Ch(60.0).resolve_font_relative(metrics, &[]), super::UsedPreferredSize::Px(36.0));
        assert_eq!(super::FontRelativeLength::ex(20.0).unwrap().resolve(metrics), 16.0);
    }

    #[test]
    fn square_borders_share_one_default_radius_entry() {
        let document = Document::new();
        let mut builder = ComputedStylesBuilder::new(&document);
        for color in 0..32 {
            let mut border = super::Border::default();
            border.border_top_color = color;
            builder.push(Font::default(), InheritedText::default(), Default::default(), border, Default::default()).unwrap();
        }
        assert_eq!(builder.store.border_radii_styles.len(), 1);
    }

    #[test]
    fn computed_style_push_rejects_non_finite_values() {
        let document = Document::new();
        let mut builder = ComputedStylesBuilder::new(&document);

        let mut font = Font::default();
        font.font_size = f32::NAN;
        assert!(matches!(builder.push(font, InheritedText::default(), Default::default(), Default::default(), Default::default()), Err(ComputedStyleValueError::NonFiniteValue { property: "Font", field: "font_size", .. })));

        assert!(super::TextSpacing::new(f32::INFINITY, 0.0).is_none());
        assert!(super::TextSpacing::new(0.0, f32::NAN).is_none());

        let mut box_model = super::BoxModel::default();
        box_model.width = super::PreferredSize::Percent(f32::NAN);
        assert!(matches!(builder.push(Font::default(), InheritedText::default(), box_model, Default::default(), Default::default()), Err(ComputedStyleValueError::NonFiniteValue { property: "BoxModel", field: "width", .. })));

        let mut padding = super::BoxModel::default();
        padding.padding_left = super::LengthPct::Px(f32::INFINITY);
        assert!(matches!(builder.push(Font::default(), InheritedText::default(), padding, Default::default(), Default::default()), Err(ComputedStyleValueError::NonFiniteValue { property: "BoxModel", field: "padding_left", .. })));

        let mut vertical_align = super::BoxModel::default();
        vertical_align.vertical_align = VerticalAlignValue::Length(f32::NAN);
        assert!(matches!(
            builder.push(Font::default(), InheritedText::default(), vertical_align, Default::default(), Default::default()),
            Err(ComputedStyleValueError::NonFiniteValue { property: "BoxModel", field: "vertical_align.length", .. })
        ));

        let mut vertical_align_calc = super::BoxModel::default();
        vertical_align_calc.vertical_align = VerticalAlignValue::Calc { absolute_px: 1.0, line_height_fraction: f32::INFINITY, x_height_px: 0.0 };
        assert!(matches!(
            builder.push(Font::default(), InheritedText::default(), vertical_align_calc, Default::default(), Default::default()),
            Err(ComputedStyleValueError::NonFiniteValue { property: "BoxModel", field: "vertical_align.calc.line_height_fraction", .. })
        ));

        let mut border = super::Border::default();
        border.border_top_width = super::FontRelativeLength::invalid_for_test(f32::NAN);
        assert!(matches!(builder.push(Font::default(), InheritedText::default(), Default::default(), border, Default::default()), Err(ComputedStyleValueError::NonFiniteValue { property: "Border", field: "border_top_width", .. })));

        let mut background = super::Background::default();
        assert!(super::FontRelativeLength::px(f32::NAN).is_none());
        assert!(super::FontRelativeLength::px(-1.0).is_none());

        background.text_decoration.thickness = super::TextDecorationThickness::Length(super::LengthPct::Px(f32::NAN));
        assert!(matches!(builder.push(Font::default(), InheritedText::default(), Default::default(), Default::default(), background), Err(ComputedStyleValueError::NonFiniteValue { property: "TextDecoration", field: "thickness", .. })));
    }

    #[test]
    fn decoration_lines_expose_only_supported_combinations() {
        let lines = super::TextDecorationLines::new(true, true, true);
        assert!(lines.underline());
        assert!(lines.overline());
        assert!(lines.line_through());
        assert!(!lines.is_empty());
        assert!(super::TextDecorationLines::default().is_empty());
        assert!(super::TextDecorationThickness::length(super::LengthPct::Px(-1.0)).is_none());
    }

    #[test]
    fn computed_style_push_rejects_invalid_grid_area_geometry() {
        let document = Document::new();
        let mut builder = ComputedStylesBuilder::new(&document);
        let name = builder.intern_string("broken");
        let mut layout = super::LayoutStyle::default();
        layout.grid_template_areas.push(super::GridTemplateArea { name, row_start: 0, row_end: 1, column_start: 1, column_end: 2 });

        assert!(matches!(
            builder.push_with_layout(Font::default(), InheritedText::default(), Default::default(), Default::default(), Default::default(), layout),
            Err(ComputedStyleValueError::InvalidStructure { property: "LayoutStyle", field: "grid_template_area_bounds" })
        ));
    }

    #[test]
    fn computed_style_push_accepts_zero_font_size_and_rejects_negative_sizes() {
        let document = Document::new();
        let mut builder = ComputedStylesBuilder::new(&document);

        let mut font = Font::default();
        font.font_size = 0.0;
        assert!(builder.push(font, InheritedText::default(), Default::default(), Default::default(), Default::default()).is_ok());

        let mut font = Font::default();
        font.font_size = -1.0;
        assert!(matches!(builder.push(font, InheritedText::default(), Default::default(), Default::default(), Default::default()), Err(ComputedStyleValueError::OutOfRangeValue { property: "Font", field: "font_size", .. })));

        let mut font = Font::default();
        font.font_weight = 1001;
        assert!(matches!(builder.push(font, InheritedText::default(), Default::default(), Default::default(), Default::default()), Err(ComputedStyleValueError::InvalidFontWeight { value: 1001 })));

        let mut text = InheritedText::default();
        text.line_height = -1.0;
        assert!(matches!(builder.push(Font::default(), text, Default::default(), Default::default(), Default::default()), Err(ComputedStyleValueError::OutOfRangeValue { property: "InheritedText", field: "line_height", .. })));

        assert!(builder.push(Font::default(), InheritedText::default(), Default::default(), Default::default(), Default::default()).is_ok(), "zero line-height sentinel remains valid");
    }

    #[test]
    fn finish_revalidates_stored_values_as_defense_in_depth() {
        let document = Document::new();
        let mut builder = ComputedStylesBuilder::new(&document);
        builder.store.font_styles[0].font_size = f32::NAN;

        assert!(matches!(builder.finish(), Err(super::ComputedStylesBuildError::InvalidStyleValue(ComputedStyleValueError::NonFiniteValue { property: "Font", field: "font_size", .. }))));
    }

    #[test]
    fn computed_style_push_accepts_negative_margins_and_rejects_negative_border_width() {
        let document = Document::new();
        let mut builder = ComputedStylesBuilder::new(&document);

        let mut box_model = super::BoxModel::default();
        box_model.margin_left = crate::LengthPct::Px(-4.0);
        assert!(builder.push(Font::default(), InheritedText::default(), box_model, Default::default(), Default::default()).is_ok());

        assert!(super::FontRelativeLength::px(-0.5).is_none(), "negative border widths cannot be represented");
    }

    #[test]
    fn computed_style_push_rejects_foreign_style_string_ids() {
        let document = Document::new();
        let mut first = ComputedStylesBuilder::new(&document);
        let foreign = first.intern_string("foreign");
        let mut builder = ComputedStylesBuilder::new(&document);

        let mut font = Font::default();
        font.font_family = Some(foreign);
        assert!(matches!(builder.push(font, InheritedText::default(), Default::default(), Default::default(), Default::default()), Err(ComputedStyleValueError::ForeignStyleStringId { .. })));
    }

    #[test]
    fn border_radius_resolution_applies_css_overlap_scaling() {
        let radii = super::BorderRadii {
            top_left: super::CornerRadius { x: crate::LengthPct::Px(80.0), y: crate::LengthPct::Px(30.0) },
            top_right: super::CornerRadius { x: crate::LengthPct::Px(80.0), y: crate::LengthPct::Px(30.0) },
            bottom_right: super::CornerRadius::default(),
            bottom_left: super::CornerRadius::default(),
        };
        let used = radii.resolve_font_relative(super::FontRelativeRatios::new(0.5, 0.5, 0.8).unwrap()).resolve(100.0, 100.0);
        assert_eq!(used.top_left, (50.0, 18.75));
        assert_eq!(used.top_right, (50.0, 18.75));
    }

    #[test]
    fn computed_style_push_rejects_invalid_corner_radius() {
        let document = Document::new();
        let mut builder = ComputedStylesBuilder::new(&document);
        let mut radii = super::BorderRadii::default();
        radii.top_left.x = crate::LengthPct::Px(-1.0);
        assert!(matches!(
            builder.push_with_layout_and_radii(Font::default(), InheritedText::default(), Default::default(), Default::default(), Default::default(), Default::default(), radii),
            Err(ComputedStyleValueError::OutOfRangeValue { property: "BorderRadii", field: "top_left", .. })
        ));
    }
}
