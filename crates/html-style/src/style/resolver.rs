use crate::style::declarations::normalize as normalize_declarations;
use crate::style::prepared::{CascadeOrigin, EffectiveRuleId, PreparedRuleSet, RulePriority};
use crate::style::selectors::SelectorIndex;
use crate::style::selectors_dom::{PseudoTarget, selector_matches_dom_pseudo_in_scope};
use crate::style::tab_size::{CASCADE_MARKER as TAB_SIZE_CASCADE_MARKER, ParsedTabSize, parse as parse_tab_size};
use crate::style::text_spacing::{LETTER_SPACING_MARKER, ParsedSpacing, WORD_SPACING_MARKER, parse as parse_text_spacing};
use crate::style::white_space::{CASCADE_MARKER as WHITE_SPACE_CASCADE_MARKER, from_marker_tokens};
use html_dom::{Document, DomNodeId};
use html_style_model::{
    AspectRatio as ComputedAspectRatio, Background, Border, BorderCollapseMode, BorderRadii, BorderStyle, BoxModel, BoxSizing, BreakBetween, BreakInside, CaptionSide, Clear, ComputedSizeComponent, ComputedStyleValueError, ComputedStyles,
    ComputedStylesBuilder, ContentAlignment, CornerRadius, CounterDirective, CounterDirectives, CounterStyle, DecorationColor, Display, EmptyCellsMode, FlexDirection, FlexWrap, Float, Font, FontRelativeLength, FontStyle, GeneratedContent,
    GeneratedContentItem, GridAutoFlow, GridPlacement, GridPlacementRange, GridRepeatCount, GridTemplateArea, GridTemplateTrack, GridTrackBreadth, GridTrackSize, Hyphens, InheritedText, ItemAlignment, LayoutStyle, LengthPct,
    LogicalTextAlign, OpenTypeFeature, OverflowMode, OverflowWrap, PositionMode, PreferredSize, QuoteStyle, SizeComparison, StyleIndices, StyleStringId, TabSize, TextAlign, TextDecorationLines, TextDecorationStyle, TextDecorationThickness,
    TextDirection, TextOverflow, TextSpacing, TextTransform, VerticalAlignValue, WhiteSpace, WordBreak,
};
use lightningcss::printer::{Printer, PrinterOptions};
use lightningcss::properties::border::{BorderSideWidth, LineStyle};
use lightningcss::properties::custom::{Token, TokenList, TokenOrValue, Variable};
use lightningcss::properties::font::LineHeight as CssLineHeight;
use lightningcss::properties::font::{AbsoluteFontWeight, Font as CssFont, FontFamily, FontSize, FontWeight, VerticalAlign, VerticalAlignKeyword};
use lightningcss::properties::overflow::{OverflowKeyword, TextOverflow as CssTextOverflow};
use lightningcss::properties::size::{MaxSize, Size};
use lightningcss::properties::text::Spacing;
use lightningcss::properties::{CSSWideKeyword, Property, PropertyId};
use lightningcss::stylesheet::{ParserOptions, StyleAttribute};
use lightningcss::traits::{ParseWithOptions, ToCss};
use lightningcss::values::calc::{Calc, MathFunction};
use lightningcss::values::color::{CssColor, SystemColor};
use lightningcss::values::length::{Length, LengthPercentage, LengthPercentageOrAuto, LengthValue};
use lightningcss::values::size::Size2D;
use lightningcss::visitor::{Visit, Visitor};
use rustc_data_structures::fx::FxHashMap;
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[path = "html_presentational_hints.rs"]
mod html_presentational_hints;

// ============================================================================
// Style Resolver - applies CSS styles to Document boxes
// ============================================================================
/// Mutable accumulator the cascade writes into: the same split sub-structs the
/// `StyleStore` ultimately stores, bundled so resolution can mutate one value.
/// Inherited groups (`font`, `text`) are cloned from the parent; reset groups
/// start from their defaults.
#[derive(Clone)]
struct WorkingStyle {
    font: Font,
    text: InheritedText,
    box_model: BoxModel,
    border: Border,
    radii: BorderRadii,
    background: Background,
    layout: LayoutStyle,
    line_height_spec: Option<CssLineHeight>,
    generated_content: Option<GeneratedContent>,
    counters: CounterDirectives,
}

fn initial_box_model() -> BoxModel {
    let mut box_model = BoxModel::default();
    box_model.display = Display::Inline;
    box_model
}

fn initial_border() -> Border {
    let mut border = Border::default();
    let medium = FontRelativeLength::px(3.0).expect("CSS medium border width is finite");
    border.border_top_width = medium;
    border.border_right_width = medium;
    border.border_bottom_width = medium;
    border.border_left_width = medium;
    border.current_color_sides = 0b1111;
    border
}

impl Default for WorkingStyle {
    fn default() -> Self {
        Self {
            font: Font::default(),
            text: InheritedText::default(),
            box_model: initial_box_model(),
            border: initial_border(),
            radii: BorderRadii::default(),
            background: Background::default(),
            layout: LayoutStyle::default(),
            line_height_spec: None,
            generated_content: None,
            counters: CounterDirectives::default(),
        }
    }
}

impl WorkingStyle {
    /// Move the resolved sub-styles into the document's deduped style store.
    fn intern(mut self, styles: &mut ComputedStylesBuilder) -> Result<StyleIndices, ComputedStyleValueError> {
        // Absolute positioning and floating both blockify the principal box
        // at the computed-style boundary (CSS 2.1 section 9.7). The box
        // builder still recognizes a floated descendant from its `float`
        // value and leaves an anchor when it occurs inside inline content.
        if self.layout.position == PositionMode::Absolute || matches!(self.box_model.float, Float::Left | Float::Right) {
            self.box_model.display = match self.box_model.display {
                Display::Inline | Display::InlineBlock => Display::Block,
                Display::InlineTable => Display::Table,
                Display::InlineFlex => Display::Flex,
                Display::InlineGrid => Display::Grid,
                Display::TableRowGroup | Display::TableHeaderGroup | Display::TableFooterGroup | Display::TableRow | Display::TableColumnGroup | Display::TableColumn | Display::TableCell | Display::TableCaption => Display::Block,
                display => display,
            };
        }
        self.box_model.normalize_overflow_axes();
        styles.push_with_layout_and_radii(self.font, self.text, self.box_model, self.border, self.background, self.layout, self.radii)
    }
}

/// Matched rule with specificity for cascade sorting
#[derive(Clone, Copy)]
struct MatchedRule {
    specificity: u32,
    id: EffectiveRuleId,
    scope_proximity: u32,
}

fn layer_baseline<'a>(baselines: &'a [(RulePriority, WorkingStyle)], priority: RulePriority) -> Option<&'a WorkingStyle> {
    baselines.iter().find_map(|(candidate, style)| candidate.same_origin_and_layer(priority).then_some(style))
}

fn origin_baseline<'a>(baselines: &'a [(CascadeOrigin, WorkingStyle)], origin: CascadeOrigin) -> Option<&'a WorkingStyle> {
    baselines.iter().find_map(|(candidate, style)| (*candidate == origin).then_some(style))
}

fn declarations_use_rollback_keyword(declarations: &[Property<'_>], expected: CSSWideKeyword, keyword: &str) -> bool {
    declarations.iter().any(|property| match property {
        Property::All(value) => *value == expected,
        Property::Unparsed(unparsed) => single_ident_keyword(&unparsed.value).is_some_and(|value| value.eq_ignore_ascii_case(keyword)),
        Property::Custom(custom) => single_ident_keyword(&custom.value).is_some_and(|value| value.eq_ignore_ascii_case(keyword)),
        _ => false,
    })
}

fn declarations_use_revert(declarations: &[Property<'_>]) -> bool {
    declarations_use_rollback_keyword(declarations, CSSWideKeyword::Revert, "revert")
}

fn declarations_use_revert_layer(declarations: &[Property<'_>]) -> bool {
    declarations_use_rollback_keyword(declarations, CSSWideKeyword::RevertLayer, "revert-layer")
}

fn rollback_layers(matched_rules: &[MatchedRule], prepared: &PreparedRuleSet<'_, '_>) -> Vec<RulePriority> {
    let mut layers = Vec::new();
    for matched in matched_rules {
        let rule = prepared.get(matched.id);
        if (declarations_use_revert_layer(&rule.style_rule().declarations.declarations) || declarations_use_revert_layer(&rule.style_rule().declarations.important_declarations))
            && !layers.iter().any(|existing: &RulePriority| existing.same_origin_and_layer(rule.priority()))
        {
            layers.push(rule.priority());
        }
    }
    layers
}

fn layer_needs_rollback(layers: &[RulePriority], priority: RulePriority) -> bool {
    layers.iter().any(|candidate| candidate.same_origin_and_layer(priority))
}

fn rollback_origins(matched_rules: &[MatchedRule], prepared: &PreparedRuleSet<'_, '_>) -> Vec<CascadeOrigin> {
    let mut origins = Vec::new();
    for matched in matched_rules {
        let rule = prepared.get(matched.id);
        if (declarations_use_revert(&rule.style_rule().declarations.declarations) || declarations_use_revert(&rule.style_rule().declarations.important_declarations)) && !origins.contains(&rule.priority().origin()) {
            origins.push(rule.priority().origin());
        }
    }
    origins
}

fn origin_needs_rollback(origins: &[CascadeOrigin], origin: CascadeOrigin) -> bool {
    origins.contains(&origin)
}

/// The parent element's interned computed style, used to resolve the CSS-wide
/// keywords `inherit` and `unset` (and `initial` via [`ParentStyle::initial`]).
struct ParentStyle {
    font: Font,
    text: InheritedText,
    box_model: BoxModel,
    border: Border,
    radii: BorderRadii,
    background: Background,
    layout: LayoutStyle,
}

impl ParentStyle {
    fn from_working(style: &WorkingStyle) -> Self {
        Self { font: style.font.clone(), text: style.text.clone(), box_model: style.box_model.clone(), border: style.border.clone(), radii: style.radii, background: style.background.clone(), layout: style.layout.clone() }
    }

    fn from_indices(styles: &ComputedStylesBuilder, indices: StyleIndices) -> Self {
        Self {
            font: styles.font_style(indices).expect("builder-issued style handle").clone(),
            text: styles.text_style(indices).expect("builder-issued style handle").clone(),
            box_model: styles.box_model_style(indices).expect("builder-issued style handle").clone(),
            border: styles.border_style(indices).expect("builder-issued style handle").clone(),
            radii: *styles.border_radii_style(indices).expect("builder-issued style handle"),
            background: styles.background_style(indices).expect("builder-issued style handle").clone(),
            layout: styles.layout_style(indices).expect("builder-issued style handle").clone(),
        }
    }

    fn for_node(doc: &Document, styles: &ComputedStylesBuilder, node_idx: DomNodeId) -> Self {
        if let Some(parent_idx) = doc.get_dom_parent(node_idx)
            && let Some(indices) = styles.style_for_node(parent_idx)
        {
            return Self {
                font: styles.font_style(indices).expect("builder-issued style handle").clone(),
                text: styles.text_style(indices).expect("builder-issued style handle").clone(),
                box_model: styles.box_model_style(indices).expect("builder-issued style handle").clone(),
                border: styles.border_style(indices).expect("builder-issued style handle").clone(),
                radii: *styles.border_radii_style(indices).expect("builder-issued style handle"),
                background: styles.background_style(indices).expect("builder-issued style handle").clone(),
                layout: styles.layout_style(indices).expect("builder-issued style handle").clone(),
            };
        }
        Self::initial(doc.root_font_size())
    }

    /// The initial value of every property (`font-size: medium` resolves to
    /// the document's root font size).
    fn initial(root_font_size: f32) -> Self {
        let mut font = Font::default();
        font.font_size = root_font_size;
        Self { font, text: InheritedText::default(), box_model: initial_box_model(), border: initial_border(), radii: BorderRadii::default(), background: Background::default(), layout: LayoutStyle::default() }
    }

    fn to_working_style(&self) -> WorkingStyle {
        WorkingStyle {
            font: self.font.clone(),
            text: self.text.clone(),
            box_model: self.box_model.clone(),
            border: self.border.clone(),
            radii: self.radii,
            background: self.background.clone(),
            layout: self.layout.clone(),
            line_height_spec: None,
            generated_content: None,
            counters: CounterDirectives::default(),
        }
    }

    fn unset_working_style(&self, root_font_size: f32) -> WorkingStyle {
        let mut style = Self::initial(root_font_size).to_working_style();
        style.font = self.font.clone();
        style.text = self.text.clone();
        inherit_box_model_properties(&mut style.box_model, &self.box_model);
        style
    }
}

/// Resolve styles for all DOM elements
/// This works with the DOM tree before boxes are created
#[derive(Clone, Debug, Default)]
pub(crate) struct ResolveStyleTimings {
    pub selector_index: Duration,
    pub resolver_setup: Duration,
    pub selector_matching: Duration,
    pub cascade: Duration,
    pub style_store: Duration,
}

struct StyleResolverContext<'a, 'sheet, 'css> {
    doc: &'a Document,
    prepared: &'a PreparedRuleSet<'sheet, 'css>,
    index: &'a SelectorIndex,
    styles: &'a mut ComputedStylesBuilder,
    candidate_scratch: &'a mut Vec<EffectiveRuleId>,
    candidate_seen: &'a mut rustc_data_structures::fx::FxHashSet<EffectiveRuleId>,
    timings: &'a mut ResolveStyleTimings,
}

thread_local! {
    /// Length conversion is a synchronous part of one style-resolution pass.
    /// Keeping its immutable media environment thread-local avoids threading
    /// duplicate viewport arguments through every typed property converter,
    /// while remaining isolated when documents are styled in parallel.
    static RESOLUTION_MEDIA_ENVIRONMENT: Cell<crate::MediaEnvironment> = Cell::new(crate::MediaEnvironment::default());
    static RESOLUTION_USES_VIEWPORT_UNITS: Cell<bool> = const { Cell::new(false) };
    /// The computed line height against which `lh` resolves for the property
    /// currently being converted. `None` means no finite basis is available.
    static RESOLUTION_LINE_HEIGHT: Cell<Option<f32>> = const { Cell::new(None) };
    /// The root element's computed line height for `rlh` conversion.
    static RESOLUTION_ROOT_LINE_HEIGHT: Cell<Option<f32>> = const { Cell::new(None) };
}

pub(crate) fn resolve_styles_for_dom_timed<'sheet, 'css>(doc: &Document, prepared: &PreparedRuleSet<'sheet, 'css>) -> (ComputedStyles, ResolveStyleTimings) {
    RESOLUTION_MEDIA_ENVIRONMENT.set(prepared.environment());
    RESOLUTION_USES_VIEWPORT_UNITS.set(false);
    RESOLUTION_ROOT_LINE_HEIGHT.set(None);
    // Build selector index for fast candidate lookup
    let started = Instant::now();
    let index = SelectorIndex::from_prepared(prepared);
    let mut timings = ResolveStyleTimings { selector_index: started.elapsed(), ..ResolveStyleTimings::default() };

    // Process each DOM element
    let started = Instant::now();
    let mut custom_maps: Vec<FxHashMap<String, TokenList<'css>>> = vec![FxHashMap::default(); doc.node_count()];
    let mut computed_styles = ComputedStylesBuilder::new(doc);
    let mut candidate_scratch = Vec::new();
    let mut candidate_seen = rustc_data_structures::fx::FxHashSet::default();
    let mut ancestor_filters = vec![AncestorFilter::default(); doc.node_count()];
    timings.resolver_setup = started.elapsed();
    {
        let mut resolver = StyleResolverContext { doc, prepared, index: &index, styles: &mut computed_styles, candidate_scratch: &mut candidate_scratch, candidate_seen: &mut candidate_seen, timings: &mut timings };
        for node_idx in doc.node_ids() {
            // Skip text nodes
            if doc.is_text_node(node_idx) {
                continue;
            }

            let ancestor_filter = if let Some(parent_idx) = doc.get_dom_parent(node_idx) {
                let mut filter = ancestor_filters[parent_idx.index()];
                extend_ancestor_filter_dom(doc, parent_idx, &mut filter);
                filter
            } else {
                AncestorFilter::default()
            };
            ancestor_filters[node_idx.index()] = ancestor_filter;
            let parent_custom = doc.get_dom_parent(node_idx).and_then(|parent_idx| custom_maps.get(parent_idx.index())).cloned();

            let (mut style, custom_map) = resolver.compute_style_for_dom_element(node_idx, parent_custom, &ancestor_filter);

            let started = Instant::now();
            let counters = std::mem::take(&mut style.counters);
            let style_indices = style.intern(resolver.styles).expect("resolver built style values should pass style validation");
            resolver.styles.set_node_style(node_idx, style_indices).expect("resolver only assigns styles to nodes from its source document");
            if !counters.is_empty() {
                resolver.styles.set_counter_directives(node_idx, counters).expect("counter directives belong to their originating element");
            }
            let first_line = resolver.compute_pseudo_style_for_dom_element(node_idx, style_indices, PseudoTarget::FirstLine, &custom_map).map(|style| style.intern(resolver.styles).expect("pseudo style values should pass validation"));
            if let Some(style) = first_line {
                resolver.styles.set_first_line_style(node_idx, style).expect("pseudo style belongs to its originating element");
            }
            let mut before_first_letter_parent = None;
            if let Some(mut style) = resolver.compute_pseudo_style_for_dom_element(node_idx, style_indices, PseudoTarget::Before, &custom_map)
                && let Some(content) = style.generated_content.take()
            {
                let counters = std::mem::take(&mut style.counters);
                let style = style.intern(resolver.styles).expect("pseudo style values should pass validation");
                resolver.styles.set_before_style(node_idx, style, content, counters).expect("pseudo style belongs to its originating element");
                before_first_letter_parent = if let Some(first_line) = first_line {
                    resolver
                        .compute_pseudo_style_for_dom_element(node_idx, first_line, PseudoTarget::Before, &custom_map)
                        .and_then(|mut style| style.generated_content.take().map(|_| style.intern(resolver.styles).expect("pseudo style values should pass validation")))
                } else {
                    Some(style)
                };
            }
            let first_letter_parent = first_line.unwrap_or(style_indices);
            let first_letter =
                resolver.compute_pseudo_style_for_dom_element(node_idx, first_letter_parent, PseudoTarget::FirstLetter, &custom_map).map(|style| style.intern(resolver.styles).expect("pseudo style values should pass validation"));
            if let Some(style) = first_letter {
                resolver.styles.set_first_letter_style(node_idx, style).expect("pseudo style belongs to its originating element");
            }
            if let Some(parent) = before_first_letter_parent
                && let Some(style) = resolver.compute_pseudo_style_for_dom_element(node_idx, parent, PseudoTarget::FirstLetter, &custom_map).map(|style| style.intern(resolver.styles).expect("pseudo style values should pass validation"))
            {
                resolver.styles.set_before_first_letter_style(node_idx, style).expect("pseudo style belongs to its originating element");
            }
            if let Some(mut style) = resolver.compute_pseudo_style_for_dom_element(node_idx, style_indices, PseudoTarget::After, &custom_map)
                && let Some(content) = style.generated_content.take()
            {
                let counters = std::mem::take(&mut style.counters);
                let style = style.intern(resolver.styles).expect("pseudo style values should pass validation");
                resolver.styles.set_after_style(node_idx, style, content, counters).expect("pseudo style belongs to its originating element");
            }
            if let Some(slot) = custom_maps.get_mut(node_idx.index()) {
                *slot = custom_map;
            }
            resolver.timings.style_store += started.elapsed();
        }
    }
    let started = Instant::now();
    let styles = computed_styles.finish().expect("resolver must assign a valid computed style to every element");
    timings.style_store += started.elapsed();
    if RESOLUTION_USES_VIEWPORT_UNITS.get() {
        prepared.media_queries().mark_viewport_unit_dependency();
    }
    (styles, timings)
}

impl<'a, 'sheet, 'css> StyleResolverContext<'a, 'sheet, 'css> {
    fn compute_pseudo_style_for_dom_element(&mut self, node_idx: DomNodeId, inherited_from: StyleIndices, pseudo: PseudoTarget, inherited_custom_properties: &FxHashMap<String, TokenList<'css>>) -> Option<WorkingStyle> {
        let doc = self.doc;
        let prepared = self.prepared;
        let mut matched_rules = Vec::new();
        for id in self.candidate_scratch.iter().copied() {
            let style_rule = prepared.get(id).style_rule();
            let Some(scope_match) = prepared.scope_match(prepared.get(id).scope(), doc, node_idx) else { continue };
            // Parcel selectors represents the pseudo-to-originating-element hop as
            // a synthetic combinator. The ancestor bloom filter treats that as a
            // real tree hop, so it cannot safely prefilter pseudo selectors.
            let specificity = style_rule.selectors.0.iter().filter(|selector| selector_matches_dom_pseudo_in_scope(selector, doc, node_idx, pseudo, scope_match.root)).map(crate::style::selectors_dom::selector_specificity).max();
            if let Some(specificity) = specificity {
                matched_rules.push(MatchedRule { specificity, id, scope_proximity: scope_match.proximity });
            }
        }
        if matched_rules.is_empty() {
            return None;
        }

        let mut important_rules = matched_rules.clone();
        matched_rules.sort_by(|a, b| prepared.get(a.id).priority().compare_normal(a.specificity, a.scope_proximity, prepared.get(b.id).priority(), b.specificity, b.scope_proximity));
        important_rules.sort_by(|a, b| prepared.get(a.id).priority().compare_important(a.specificity, a.scope_proximity, prepared.get(b.id).priority(), b.specificity, b.scope_proximity));

        let inherited = ParentStyle::from_indices(self.styles, inherited_from);
        let mut style = WorkingStyle {
            font: inherited.font.clone(),
            text: inherited.text.clone(),
            box_model: initial_box_model(),
            border: initial_border(),
            radii: BorderRadii::default(),
            background: Background::default(),
            layout: LayoutStyle::default(),
            line_height_spec: None,
            generated_content: None,
            counters: CounterDirectives::default(),
        };
        inherit_box_model_properties(&mut style.box_model, &inherited.box_model);
        // Generated pseudo-elements use the CSS initial value of `display`,
        // which is `inline`.  Ordinary DOM elements commonly become blocks
        // through the UA sheet, but a pseudo has no element-name UA rule to
        // override this initial value.
        style.box_model.display = Display::Inline;
        let parent_font_size = style.font.font_size;
        let mut custom_properties = inherited_custom_properties.clone();
        let rollback_layers = rollback_layers(&matched_rules, prepared);
        let rollback_origins = rollback_origins(&matched_rules, prepared);
        let unused_custom_rollback_basis = FxHashMap::default();
        let mut normal_custom_origin_baselines = Vec::new();
        let mut normal_custom_baselines = Vec::new();
        for matched in &matched_rules {
            let rule = prepared.get(matched.id);
            let priority = rule.priority();
            if origin_needs_rollback(&rollback_origins, priority.origin()) && !normal_custom_origin_baselines.iter().any(|(previous, _): &(CascadeOrigin, FxHashMap<String, TokenList<'css>>)| *previous == priority.origin()) {
                normal_custom_origin_baselines.push((priority.origin(), custom_properties.clone()));
            }
            if layer_needs_rollback(&rollback_layers, priority) && !normal_custom_baselines.iter().any(|(previous, _): &(RulePriority, FxHashMap<String, TokenList<'css>>)| previous.same_origin_and_layer(priority)) {
                normal_custom_baselines.push((priority, custom_properties.clone()));
            }
            let origin_basis = normal_custom_origin_baselines.iter().find_map(|(candidate, basis)| (*candidate == priority.origin()).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            let layer_basis = normal_custom_baselines.iter().find_map(|(candidate, basis)| candidate.same_origin_and_layer(priority).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            collect_custom_properties(&mut custom_properties, inherited_custom_properties, &rule.style_rule().declarations.declarations, origin_basis, layer_basis);
        }
        for origin in [crate::style::prepared::CascadeOrigin::Author, crate::style::prepared::CascadeOrigin::UserAgent] {
            for matched in important_rules.iter().filter(|matched| prepared.get(matched.id).priority().origin() == origin) {
                let rule = prepared.get(matched.id);
                let origin_basis = normal_custom_origin_baselines.iter().find_map(|(origin, basis)| (*origin == rule.priority().origin()).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
                let layer_basis = normal_custom_baselines.iter().find_map(|(priority, basis)| priority.same_origin_and_layer(rule.priority()).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
                collect_custom_properties(&mut custom_properties, inherited_custom_properties, &rule.style_rule().declarations.important_declarations, origin_basis, layer_basis);
            }
        }
        resolve_custom_properties(&mut custom_properties, inherited_custom_properties);

        let unused_rollback_basis = WorkingStyle::default();
        for phase in [CascadePhase::Prerequisites, CascadePhase::Remaining] {
            let mut normal_origin_baselines: Vec<(CascadeOrigin, WorkingStyle)> = Vec::new();
            let mut normal_layer_baselines: Vec<(RulePriority, WorkingStyle)> = Vec::new();
            for matched in &matched_rules {
                let rule = prepared.get(matched.id);
                let priority = rule.priority();
                if origin_needs_rollback(&rollback_origins, priority.origin()) && origin_baseline(&normal_origin_baselines, priority.origin()).is_none() {
                    normal_origin_baselines.push((priority.origin(), style.clone()));
                }
                if layer_needs_rollback(&rollback_layers, priority) && layer_baseline(&normal_layer_baselines, priority).is_none() {
                    normal_layer_baselines.push((priority, style.clone()));
                }
                let origin_basis = origin_baseline(&normal_origin_baselines, priority.origin()).unwrap_or(&unused_rollback_basis);
                let layer_basis = layer_baseline(&normal_layer_baselines, priority).unwrap_or(&unused_rollback_basis);
                self.apply_declarations(&mut style, &rule.style_rule().declarations.declarations, parent_font_size, Some(&custom_properties), phase, &inherited, origin_basis, layer_basis);
            }
            for matched in important_rules.iter().filter(|matched| prepared.get(matched.id).priority().origin() == crate::style::prepared::CascadeOrigin::Author) {
                let rule = prepared.get(matched.id);
                let origin_basis = origin_baseline(&normal_origin_baselines, rule.priority().origin()).unwrap_or(&unused_rollback_basis);
                let layer_basis = layer_baseline(&normal_layer_baselines, rule.priority()).unwrap_or(&unused_rollback_basis);
                self.apply_declarations(&mut style, &rule.style_rule().declarations.important_declarations, parent_font_size, Some(&custom_properties), phase, &inherited, origin_basis, layer_basis);
            }
            for matched in important_rules.iter().filter(|matched| prepared.get(matched.id).priority().origin() == crate::style::prepared::CascadeOrigin::UserAgent) {
                let rule = prepared.get(matched.id);
                let origin_basis = origin_baseline(&normal_origin_baselines, rule.priority().origin()).unwrap_or(&unused_rollback_basis);
                let layer_basis = layer_baseline(&normal_layer_baselines, rule.priority()).unwrap_or(&unused_rollback_basis);
                self.apply_declarations(&mut style, &rule.style_rule().declarations.important_declarations, parent_font_size, Some(&custom_properties), phase, &inherited, origin_basis, layer_basis);
            }
        }
        Some(style)
    }
}

/// The cascade runs in dependency-ordered passes. `direction` is finalized
/// before logical properties are mapped, and `font-size`/`color` are finalized
/// before dependent lengths and `currentColor` are resolved.
#[derive(Clone, Copy, PartialEq)]
enum CascadePhase {
    Prerequisites,
    Remaining,
}

/// Apply CSS declarations to a computed style
impl<'a, 'sheet, 'css> StyleResolverContext<'a, 'sheet, 'css> {
    fn apply_declarations<'declaration>(
        &mut self, style: &mut WorkingStyle, declarations: &[Property<'declaration>], parent_font_size: f32, custom_properties: Option<&FxHashMap<String, TokenList<'declaration>>>, phase: CascadePhase, parent: &ParentStyle,
        revert_basis: &WorkingStyle, revert_layer_basis: &WorkingStyle,
    ) {
        let doc = self.doc;
        let styles = &mut *self.styles;
        let mut var_map: HashMap<&str, TokenList<'declaration>> = HashMap::new();
        if let Some(map) = custom_properties {
            for (name, value) in map {
                var_map.insert(name.as_str(), value.clone());
            }
        }

        for property in declarations {
            match property {
                Property::Custom(custom) => {
                    let raw_name = custom.name.as_ref();
                    if raw_name.starts_with("--") {
                        // Custom properties were already cascaded, inherited,
                        // and resolved for CSS-wide keywords into `var_map`.
                        // Re-inserting this raw declaration would undo such
                        // resolution (for example, restoring literal `initial`).
                        continue;
                    }
                }
                Property::Unparsed(unparsed) if unparsed.property_id.name().starts_with("--") => {
                    continue;
                }
                _ => {}
            }

            if let Property::Unparsed(unparsed) = property
                && token_list_contains_var(&unparsed.value)
            {
                let mut substitutable = unparsed.clone();
                mark_var_substitution_boundaries(&mut substitutable.value);
                let resolved = substitutable.substitute_variables(&var_map).ok().or_else(|| resolve_single_var_property(unparsed, &var_map));
                if let Some(resolved) = resolved {
                    if let Property::Unparsed(value) = &resolved
                        && let Some(keyword) = single_ident_keyword(&value.value)
                        && apply_css_wide_keyword_in_phase(style, value.property_id.name(), keyword, parent, doc.root_font_size(), phase, None, None)
                    {
                        continue;
                    }
                    // A supported typed property only remains `Unparsed` when
                    // the substituted component values do not match its
                    // grammar. The declaration is invalid at computed-value
                    // time even though LightningCSS preserves it for output.
                    let invalid_after_substitution = matches!(&resolved, Property::Unparsed(_));
                    set_line_height_resolution_bases(doc, &*styles, &resolved, style, parent);
                    if invalid_after_substitution || property_has_faulty_numeric_value(doc, style, &resolved, parent_font_size, self.prepared.environment()) {
                        // A declaration containing var() has already won the
                        // cascade. If substitution produces an invalid computed
                        // value, CSS requires `unset` rather than exposing the
                        // declaration that lost earlier in the cascade.
                        let _ = apply_css_wide_keyword_in_phase(style, unparsed.property_id.name(), "unset", parent, doc.root_font_size(), phase, None, None);
                    } else {
                        apply_property_in_phase(doc, styles, style, &resolved, parent_font_size, self.prepared.environment(), phase, parent, revert_basis, revert_layer_basis);
                    }
                } else {
                    // Missing variables without a usable fallback are likewise
                    // invalid at computed-value time.
                    let _ = apply_css_wide_keyword_in_phase(style, unparsed.property_id.name(), "unset", parent, doc.root_font_size(), phase, None, None);
                }
                continue;
            }

            if let Property::Custom(custom) = property
                && !custom.name.as_ref().starts_with("--")
                && token_list_contains_var(&custom.value)
            {
                let mut resolved = custom.clone();
                mark_var_substitution_boundaries(&mut resolved.value);
                resolved.value.substitute_variables(&var_map);
                if token_list_contains_var(&resolved.value) {
                    let _ = apply_css_wide_keyword_in_phase(style, custom.name.as_ref(), "unset", parent, doc.root_font_size(), phase, None, None);
                } else {
                    apply_property_in_phase(doc, styles, style, &Property::Custom(resolved), parent_font_size, self.prepared.environment(), phase, parent, revert_basis, revert_layer_basis);
                }
                continue;
            }

            apply_property_in_phase(doc, styles, style, property, parent_font_size, self.prepared.environment(), phase, parent, revert_basis, revert_layer_basis);
        }
    }
}

fn apply_property_in_phase<'a>(
    doc: &Document, styles: &mut ComputedStylesBuilder, style: &mut WorkingStyle, property: &Property<'a>, parent_font_size: f32, environment: crate::MediaEnvironment, phase: CascadePhase, parent: &ParentStyle, revert_basis: &WorkingStyle,
    revert_layer_basis: &WorkingStyle,
) {
    set_line_height_resolution_bases(doc, styles, property, style, parent);
    // Preserve whether the winning background establishes an image layer even
    // while image painting is unsupported. Canvas background propagation
    // depends on the computed image being `none`, not merely on whether a
    // drawable image fragment was produced.
    if super::capabilities::property_uses_background_image(property) {
        if matches!(phase, CascadePhase::Remaining) {
            style.background.background_image_present = true;
        }
        // A background shorthand's color remains independently paintable
        // underneath an image that this renderer cannot draw. Keep rejecting
        // standalone background-image declarations after recording their
        // computed presence, but allow the shorthand's supported color through.
        if !matches!(property, Property::Background(_)) {
            return;
        }
    }
    // Parsing a gradient does not imply that this renderer can paint it. Skip
    // the entire declaration so an earlier usable fallback remains active.
    if super::capabilities::property_uses_gradient(property) {
        return;
    }
    // Unsupported paint styles are rejected as whole declarations so their
    // shorthand color/width/line components cannot leak into computed style.
    if super::capabilities::property_uses_unsupported_text_decoration_style(property) || super::capabilities::property_uses_unsupported_outline_style(property) {
        return;
    }
    if try_apply_legacy_grid_gap_alias(doc, style, property, parent) {
        return;
    }
    // CSS-wide keywords copy already-computed values, so applying them in both
    // phases is idempotent and keeps cascade order within each phase.
    if let Property::All(keyword) = property {
        match keyword {
            CSSWideKeyword::Initial => {
                let basis = ParentStyle::initial(doc.root_font_size()).to_working_style();
                apply_all_from_basis(style, &basis, phase);
            }
            CSSWideKeyword::Inherit => {
                let basis = parent.to_working_style();
                apply_all_from_basis(style, &basis, phase);
            }
            CSSWideKeyword::Unset => {
                let basis = parent.unset_working_style(doc.root_font_size());
                apply_all_from_basis(style, &basis, phase);
            }
            CSSWideKeyword::Revert => apply_all_from_basis(style, revert_basis, phase),
            CSSWideKeyword::RevertLayer => apply_all_from_basis(style, revert_layer_basis, phase),
        }
        return;
    }
    if try_apply_css_wide_keyword(style, property, parent, doc.root_font_size(), phase, revert_basis, revert_layer_basis) {
        return;
    }
    match phase {
        CascadePhase::Prerequisites => match property {
            Property::Direction(direction) => {
                use lightningcss::properties::text::Direction;
                style.text.direction = match direction {
                    Direction::Ltr => TextDirection::Ltr,
                    Direction::Rtl => TextDirection::Rtl,
                };
                resolve_logical_text_alignments(&mut style.text);
            }
            Property::FontSize(_) | Property::LineHeight(_) | Property::Color(_) => apply_property(doc, styles, style, property, parent_font_size, parent.font.font_weight, parent.text.color, environment),
            Property::Font(font) => {
                let Some(values) = checked_font_shorthand(font, parent_font_size, parent.font.font_weight, doc.root_font_size(), environment) else { return };
                style.font.font_size = values.font_size;
                style.font.font_size_x_height_px = 0.0;
                style.font.font_size_ch_advance_px = 0.0;
                style.font.font_size_cap_height_px = 0.0;
                style.font.font_size_root_ch = 0.0;
                style.font.font_size_root_cap_height = 0.0;
                style.font.font_size_root_line_height = 0.0;
                style.line_height_spec = Some(font.line_height.clone());
                style.text.line_height_number = values.line_height_number;
                style.text.line_height = values.line_height;
                style.text.line_height_x_height_px = values.line_height_x_height_px;
                style.text.line_height_normal = values.line_height_normal;
            }
            _ => {}
        },
        CascadePhase::Remaining => match property {
            // Already final from the first pass.
            Property::Direction(_) | Property::FontSize(_) | Property::LineHeight(_) | Property::Color(_) => {}
            // Apply the shorthand but keep the font size resolved by the first
            // pass, re-binding the shorthand's line-height to it.
            Property::Font(_) => {
                let final_font_size = (
                    style.font.font_size,
                    style.font.font_size_x_height_px,
                    style.font.font_size_ch_advance_px,
                    style.font.font_size_cap_height_px,
                    style.font.font_size_root_ch,
                    style.font.font_size_root_cap_height,
                    style.font.font_size_root_line_height,
                );
                let final_line_height = (style.line_height_spec.clone(), style.text.line_height_number, style.text.line_height, style.text.line_height_x_height_px, style.text.line_height_normal);
                apply_property(doc, styles, style, property, parent_font_size, parent.font.font_weight, parent.text.color, environment);
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
                if let Some(spec) = style.line_height_spec.as_ref() {
                    if let Some((line_height, x_height_px)) = checked_line_height_components(spec, final_font_size.0, doc.root_font_size()) {
                        style.text.line_height = line_height;
                        style.text.line_height_x_height_px = x_height_px;
                    }
                }
            }
            _ => apply_property(doc, styles, style, property, parent_font_size, parent.font.font_weight, parent.text.color, environment),
        },
    }
}

fn apply_property<'a>(doc: &Document, styles: &mut ComputedStylesBuilder, style: &mut WorkingStyle, property: &Property<'a>, parent_font_size: f32, parent_font_weight: u16, parent_color: u32, environment: crate::MediaEnvironment) {
    let resolved_root_font_size = root_font_size_for_resolution(doc, styles);
    let resolved_document = ResolutionDocument { document: doc, root_font_size: resolved_root_font_size };
    let doc = &resolved_document;
    let raw_property = match property {
        Property::Custom(custom) => Some((custom.name.as_ref(), &custom.value)),
        Property::Unparsed(unparsed) => Some((unparsed.property_id.name(), &unparsed.value)),
        _ => None,
    };
    if let Some((name, tokens)) = raw_property {
        let normalized_name = name.to_ascii_lowercase();
        match normalized_name.as_str() {
            "font-feature-settings" => {
                if let Some(value) = token_list_to_css_string(tokens).as_deref().and_then(parse_font_feature_settings) {
                    style.font.font_feature_settings = value;
                }
                return;
            }
            "font-kerning" => {
                if let Some(value) = token_list_to_css_string(tokens).as_deref().and_then(parse_font_kerning) {
                    style.font.font_kerning_features = value;
                }
                return;
            }
            "font-variant-ligatures" => {
                if let Some(value) = token_list_to_css_string(tokens).as_deref().and_then(parse_font_variant_ligatures) {
                    style.font.font_variant_ligature_features = value;
                }
                return;
            }
            "font-variant-numeric" => {
                if let Some(value) = token_list_to_css_string(tokens).as_deref().and_then(parse_font_variant_numeric) {
                    style.font.font_variant_numeric_features = value;
                }
                return;
            }
            "break-before" | "page-break-before" => {
                if let Some(value) = parse_break_between(tokens) {
                    style.layout.break_before = value;
                }
                return;
            }
            "break-after" | "page-break-after" => {
                if let Some(value) = parse_break_between(tokens) {
                    style.layout.break_after = value;
                }
                return;
            }
            "break-inside" | "page-break-inside" => {
                if let Some(value) = parse_break_inside(tokens) {
                    style.layout.break_inside = value;
                }
                return;
            }
            "widows" => {
                if let Some(value) = single_positive_integer(tokens) {
                    style.text.widows = value;
                }
                return;
            }
            "orphans" => {
                if let Some(value) = single_positive_integer(tokens) {
                    style.text.orphans = value;
                }
                return;
            }
            _ => {}
        }
        if apply_custom_box_keyword(style, name, tokens) {
            return;
        }
        if name.eq_ignore_ascii_case("content")
            && let Some(content) = parse_generated_content(styles, tokens)
        {
            style.generated_content = content;
            return;
        }
        if name.eq_ignore_ascii_case("counter-reset")
            && let Some(resets) = parse_counter_directives(styles, tokens, 0)
        {
            style.counters.resets = resets;
            return;
        }
        if name.eq_ignore_ascii_case("counter-increment")
            && let Some(increments) = parse_counter_directives(styles, tokens, 1)
        {
            style.counters.increments = increments;
            return;
        }
        if name.eq_ignore_ascii_case("quotes")
            && let Some(quotes) = parse_quotes(styles, tokens)
        {
            style.text.quotes = quotes;
            return;
        }
    }
    match property {
        // Font properties
        Property::FontSize(size) => {
            let Some((font_size, x_height_px, ch_advance_px, cap_height_px, root_ch, root_cap_height, root_line_height)) = checked_font_size_components(size, &style.font, parent_font_size, resolved_root_font_size, environment) else {
                return;
            };
            style.font.font_size = font_size;
            style.font.font_size_x_height_px = x_height_px;
            style.font.font_size_ch_advance_px = ch_advance_px;
            style.font.font_size_cap_height_px = cap_height_px;
            style.font.font_size_root_ch = root_ch;
            style.font.font_size_root_cap_height = root_cap_height;
            style.font.font_size_root_line_height = root_line_height;
            if let Some(spec) = style.line_height_spec.as_ref() {
                if let Some((line_height, x_height_px)) = checked_line_height_components(spec, style.font.font_size, doc.root_font_size()) {
                    style.text.line_height = line_height;
                    style.text.line_height_x_height_px = x_height_px;
                }
            } else if style.text.line_height_number != 0.0 {
                // An inherited unitless line-height re-resolves against this
                // element's own font size.
                style.text.line_height = style.font.font_size * style.text.line_height_number;
                style.text.line_height_x_height_px = 0.0;
            }
        }
        Property::FontWeight(weight) => {
            let Some(weight) = checked_font_weight(weight, parent_font_weight) else { return };
            style.font.font_weight = weight;
        }
        Property::FontStyle(fs) => {
            use lightningcss::properties::font::FontStyle as LcFontStyle;
            style.font.font_style = match fs {
                LcFontStyle::Normal => FontStyle::Normal,
                LcFontStyle::Italic => FontStyle::Italic,
                LcFontStyle::Oblique(_) => FontStyle::Oblique,
            };
        }
        Property::FontFamily(families) => {
            style.font.font_family = Some(intern_font_family(styles, families));
        }
        Property::Font(font) => {
            let Some(values) = checked_font_shorthand(font, parent_font_size, parent_font_weight, doc.root_font_size(), environment) else { return };
            style.font.font_size = values.font_size;
            style.font.font_size_x_height_px = 0.0;
            style.font.font_size_ch_advance_px = 0.0;
            style.font.font_size_cap_height_px = 0.0;
            style.font.font_size_root_ch = 0.0;
            style.font.font_size_root_cap_height = 0.0;
            style.font.font_size_root_line_height = 0.0;
            style.font.font_weight = values.font_weight;
            use lightningcss::properties::font::FontStyle as LcFontStyle;
            style.font.font_style = match font.style {
                LcFontStyle::Normal => FontStyle::Normal,
                LcFontStyle::Italic => FontStyle::Italic,
                LcFontStyle::Oblique(_) => FontStyle::Oblique,
            };
            style.line_height_spec = Some(font.line_height.clone());
            style.text.line_height_number = values.line_height_number;
            style.text.line_height = values.line_height;
            style.text.line_height_x_height_px = values.line_height_x_height_px;
            style.text.line_height_normal = values.line_height_normal;
            style.font.font_family = Some(intern_font_family(styles, &font.family));
            style.font.font_variant_caps_features = font_variant_caps_features(&font.variant_caps);
            style.font.font_variant_numeric_features.clear();
            style.font.font_variant_ligature_features.clear();
            style.font.font_kerning_features.clear();
            style.font.font_feature_settings.clear();
        }

        // Text properties
        Property::LineHeight(lh) => {
            let Some((line_height, x_height_px)) = checked_line_height_components(lh, style.font.font_size, doc.root_font_size()) else { return };
            style.line_height_spec = Some(lh.clone());
            style.text.line_height_number = line_height_number(lh);
            style.text.line_height = line_height;
            style.text.line_height_x_height_px = x_height_px;
            style.text.line_height_normal = line_height_is_normal(lh);
        }
        Property::LetterSpacing(spacing) => {
            let Some(value) = spacing_to_text_spacing(spacing, style.font.font_size, doc.root_font_size()) else { return };
            style.text.letter_spacing = value;
        }
        Property::WordSpacing(spacing) => {
            let Some(value) = spacing_to_text_spacing(spacing, style.font.font_size, doc.root_font_size()) else { return };
            style.text.word_spacing = value;
        }
        Property::TextAlign(ta) => {
            use lightningcss::properties::text::TextAlign as LcTextAlign;
            style.text.text_align_logical = match ta {
                LcTextAlign::Start => LogicalTextAlign::Start,
                LcTextAlign::End => LogicalTextAlign::End,
                LcTextAlign::MatchParent => style.text.text_align_logical,
                _ => LogicalTextAlign::Physical,
            };
            style.text.text_align = match ta {
                LcTextAlign::Left => TextAlign::Left,
                LcTextAlign::Right => TextAlign::Right,
                LcTextAlign::Center => TextAlign::Center,
                LcTextAlign::Justify => TextAlign::Justify,
                LcTextAlign::Start => text_start_alignment(style.text.direction),
                LcTextAlign::End => text_end_alignment(style.text.direction),
                LcTextAlign::JustifyAll => TextAlign::Justify,
                // For horizontal LTR, `match-parent` computes to the parent's
                // value — which is what this element inherited.
                LcTextAlign::MatchParent => style.text.text_align,
            };
            // `text-align-last: auto` (the default) follows `text-align`.
            if !style.text.text_align_last_explicit {
                style.text.text_align_last = style.text.text_align;
                style.text.text_align_last_logical = style.text.text_align_logical;
            }
        }
        Property::TextAlignLast(ta, _) => {
            use lightningcss::properties::text::TextAlignLast as LcTextAlignLast;
            let explicit = match ta {
                LcTextAlignLast::Auto => None,
                LcTextAlignLast::Left => Some(TextAlign::Left),
                LcTextAlignLast::Right => Some(TextAlign::Right),
                LcTextAlignLast::Center => Some(TextAlign::Center),
                LcTextAlignLast::Justify => Some(TextAlign::Justify),
                LcTextAlignLast::Start => Some(text_start_alignment(style.text.direction)),
                LcTextAlignLast::End => Some(text_end_alignment(style.text.direction)),
                _ => None,
            };
            style.text.text_align_last_explicit = explicit.is_some();
            style.text.text_align_last = explicit.unwrap_or(style.text.text_align);
            style.text.text_align_last_logical = match ta {
                LcTextAlignLast::Start => LogicalTextAlign::Start,
                LcTextAlignLast::End => LogicalTextAlign::End,
                LcTextAlignLast::Auto => style.text.text_align_logical,
                _ => LogicalTextAlign::Physical,
            };
        }
        Property::TextIndent(ti) => {
            let Some(value) = length_percentage_to_lengthpct(&ti.value, style.font.font_size, doc.root_font_size()) else { return };
            style.text.text_indent = value;
            style.text.text_indent_hanging = ti.hanging;
            style.text.text_indent_each_line = ti.each_line;
        }
        Property::TextTransform(tt) => {
            use lightningcss::properties::text::TextTransformCase;
            if !tt.other.is_empty() {
                return;
            }
            style.text.text_transform = match tt.case {
                TextTransformCase::Uppercase => TextTransform::Uppercase,
                TextTransformCase::Lowercase => TextTransform::Lowercase,
                TextTransformCase::Capitalize => TextTransform::Capitalize,
                _ => TextTransform::None,
            };
        }
        Property::FontVariantCaps(caps) => {
            style.font.font_variant_caps_features = font_variant_caps_features(caps);
        }
        Property::ListStyleType(lst) => {
            let Some(value) = map_list_style_type(lst) else { return };
            style.text.list_style_type = value;
        }
        Property::ListStylePosition(pos) => {
            style.text.list_style_position = map_list_style_position(pos);
        }
        Property::ListStyleImage(image) => {
            style.text.list_style_image = list_style_image_to_interned(styles, image);
        }
        Property::ListStyle(ls) => {
            let Some(list_style_type) = map_list_style_type(&ls.list_style_type) else { return };
            style.text.list_style_type = list_style_type;
            style.text.list_style_position = map_list_style_position(&ls.position);
            style.text.list_style_image = list_style_image_to_interned(styles, &ls.image);
        }
        Property::TextDecorationLine(line, _) => {
            style.background.text_decoration.lines = text_decoration_lines(line);
        }
        Property::TextDecoration(td, _) => {
            let Some(decoration_style) = text_decoration_style(&td.style) else { return };
            let Some(thickness) = text_decoration_thickness(&td.thickness, style.font.font_size, doc.root_font_size()) else {
                return;
            };
            style.background.text_decoration.lines = text_decoration_lines(&td.line);
            style.background.text_decoration.style = decoration_style;
            style.background.text_decoration.color = decoration_color(&td.color, style.text.color);
            style.background.text_decoration.thickness = thickness;
        }
        Property::TextDecorationColor(color, _) => {
            style.background.text_decoration.color = decoration_color(color, style.text.color);
        }
        Property::TextDecorationStyle(decoration_style, _) => {
            let Some(decoration_style) = text_decoration_style(decoration_style) else { return };
            style.background.text_decoration.style = decoration_style;
        }
        Property::TextDecorationThickness(thickness) => {
            let Some(thickness) = text_decoration_thickness(thickness, style.font.font_size, doc.root_font_size()) else { return };
            style.background.text_decoration.thickness = thickness;
        }
        Property::Outline(outline) => {
            let Some(outline_style) = outline_style(&outline.style) else { return };
            let Some(width) = border_width(&outline.width, style.font.font_size, doc.root_font_size()) else { return };
            style.background.outline.set_width(width);
            style.background.outline.style = outline_style;
            style.background.outline.color = decoration_color(&outline.color, style.text.color);
        }
        Property::OutlineWidth(width) => {
            let Some(width) = border_width(width, style.font.font_size, doc.root_font_size()) else { return };
            style.background.outline.set_width(width);
        }
        Property::OutlineStyle(value) => {
            let Some(value) = outline_style(value) else { return };
            style.background.outline.style = value;
        }
        Property::OutlineColor(color) => {
            style.background.outline.color = decoration_color(color, style.text.color);
        }
        Property::VerticalAlign(va) => {
            let vertical_align = match va {
                VerticalAlign::Keyword(keyword) => match keyword {
                    VerticalAlignKeyword::Baseline => VerticalAlignValue::Baseline,
                    VerticalAlignKeyword::Sub => VerticalAlignValue::Sub,
                    VerticalAlignKeyword::Super => VerticalAlignValue::Super,
                    VerticalAlignKeyword::Top => VerticalAlignValue::Top,
                    VerticalAlignKeyword::TextTop => VerticalAlignValue::TextTop,
                    VerticalAlignKeyword::Middle => VerticalAlignValue::Middle,
                    VerticalAlignKeyword::Bottom => VerticalAlignValue::Bottom,
                    VerticalAlignKeyword::TextBottom => VerticalAlignValue::TextBottom,
                },
                VerticalAlign::Length(lp) => match vertical_align_value(lp, style.font.font_size, doc.root_font_size()) {
                    Some(value) => value,
                    None => return,
                },
            };
            style.box_model.vertical_align = vertical_align;
        }
        Property::Overflow(overflow) => {
            style.box_model.overflow_x = overflow_mode(overflow.x);
            style.box_model.overflow_y = overflow_mode(overflow.y);
        }
        Property::OverflowX(overflow) => {
            style.box_model.overflow_x = overflow_mode(*overflow);
        }
        Property::OverflowY(overflow) => {
            style.box_model.overflow_y = overflow_mode(*overflow);
        }
        Property::TextOverflow(value, _) => {
            style.box_model.text_overflow = match value {
                CssTextOverflow::Clip => TextOverflow::Clip,
                CssTextOverflow::Ellipsis => TextOverflow::Ellipsis,
            };
        }
        Property::WhiteSpace(ws) => {
            use lightningcss::properties::text::WhiteSpace as LcWhiteSpace;
            style.text.white_space = match ws {
                LcWhiteSpace::Normal => WhiteSpace::Normal,
                LcWhiteSpace::Pre => WhiteSpace::Pre,
                LcWhiteSpace::NoWrap => WhiteSpace::NoWrap,
                LcWhiteSpace::PreWrap => WhiteSpace::PreWrap,
                LcWhiteSpace::PreLine => WhiteSpace::PreLine,
                LcWhiteSpace::BreakSpaces => WhiteSpace::BreakSpaces,
            };
        }

        // Color
        Property::Color(c) => {
            // On the `color` property itself, currentColor computes as the
            // inherited color. It must not observe an earlier declaration in
            // the same cascade phase.
            style.text.color = css_color_to_u32(c, parent_color);
        }
        Property::BackgroundColor(c) => {
            style.background.background_color = css_color_to_u32(c, style.text.color);
            style.background.background_color_current_color = matches!(c, CssColor::CurrentColor);
        }
        Property::Background(bg) => {
            style.background.background_image_present = super::capabilities::property_uses_background_image(property);
            // The color may only appear in the shorthand's last layer; earlier
            // layers carry the transparent default.
            if let Some(layer) = bg.last() {
                style.background.background_color = css_color_to_u32(&layer.color, style.text.color);
                style.background.background_color_current_color = matches!(layer.color, CssColor::CurrentColor);
            }
        }
        Property::BackgroundImage(_) => {
            style.background.background_image_present = false;
        }

        // Size
        Property::Width(size) => {
            let Some(value) = size_to_preferred(size, &style.font, resolved_root_font_size, styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.width = value;
        }
        Property::Height(size) => {
            let Some(value) = size_to_preferred(size, &style.font, resolved_root_font_size, styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.height = value;
        }
        Property::AspectRatio(value) => {
            let components = value.ratio.as_ref().map(|ratio| (ratio.0, ratio.1));
            let Some(value) = ComputedAspectRatio::new(value.auto, components) else { return };
            style.box_model.aspect_ratio = value;
        }
        Property::BorderSpacing(spacing) => {
            let Some(horizontal) = length_to_px_from_length(&spacing.0, style.font.font_size, doc.root_font_size()) else { return };
            let Some(vertical) = length_to_px_from_length(&spacing.1, style.font.font_size, doc.root_font_size()) else { return };
            if !horizontal.is_finite() || !vertical.is_finite() || horizontal < 0.0 || vertical < 0.0 {
                return;
            }
            style.box_model.border_spacing_horizontal = horizontal;
            style.box_model.border_spacing_vertical = vertical;
        }
        Property::MinWidth(size) => {
            let Some(value) = size_to_preferred(size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.min_width = value;
        }
        Property::MinHeight(size) => {
            let Some(value) = size_to_preferred(size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.min_height = value;
        }
        Property::MaxWidth(max_size) => {
            let Some(value) = max_size_to_preferred(max_size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.max_width = value;
        }
        Property::MaxHeight(max_size) => {
            let Some(value) = max_size_to_preferred(max_size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.max_height = value;
        }
        Property::InlineSize(size) => {
            let Some(value) = size_to_preferred(size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.width = value;
        }
        Property::BlockSize(size) => {
            let Some(value) = size_to_preferred(size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.height = value;
        }
        Property::MinInlineSize(size) => {
            let Some(value) = size_to_preferred(size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.min_width = value;
        }
        Property::MinBlockSize(size) => {
            let Some(value) = size_to_preferred(size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.min_height = value;
        }
        Property::MaxInlineSize(size) => {
            let Some(value) = max_size_to_preferred(size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.max_width = value;
        }
        Property::MaxBlockSize(size) => {
            let Some(value) = max_size_to_preferred(size, &style.font, doc.root_font_size(), styles).and_then(non_negative_preferred_size) else { return };
            style.box_model.max_height = value;
        }

        // Absolute boxes cross the typed style boundary and are removed from
        // normal flow by layout. Fixed and sticky still remain unsupported.
        Property::Position(position) => {
            use lightningcss::properties::position::Position;
            style.layout.position = match position {
                Position::Static => PositionMode::Static,
                Position::Relative => PositionMode::Relative,
                Position::Absolute => PositionMode::Absolute,
                Position::Fixed | Position::Sticky(_) => return,
            };
        }
        Property::ZIndex(value) => {
            use lightningcss::properties::position::ZIndex;
            style.layout.z_index = match value {
                ZIndex::Auto => None,
                ZIndex::Integer(value) => Some(*value),
            };
        }
        Property::Top(value) => {
            let Some(value) = inset_value(value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.inset_top = value;
        }
        Property::Right(value) => {
            let Some(value) = inset_value(value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.inset_right = value;
        }
        Property::Bottom(value) => {
            let Some(value) = inset_value(value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.inset_bottom = value;
        }
        Property::Left(value) => {
            let Some(value) = inset_value(value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.inset_left = value;
        }
        Property::Inset(value) => {
            let (Some(top), Some(right), Some(bottom), Some(left)) = (
                inset_value(&value.top, style.font.font_size, doc.root_font_size()),
                inset_value(&value.right, style.font.font_size, doc.root_font_size()),
                inset_value(&value.bottom, style.font.font_size, doc.root_font_size()),
                inset_value(&value.left, style.font.font_size, doc.root_font_size()),
            ) else {
                return;
            };
            style.layout.inset_top = top;
            style.layout.inset_right = right;
            style.layout.inset_bottom = bottom;
            style.layout.inset_left = left;
        }

        // Margin
        Property::MarginTop(m) => {
            let Some(value) = margin_value(m, style.font.font_size, doc.root_font_size()) else { return };
            (style.box_model.margin_top, style.layout.margin_top_auto) = value;
        }
        Property::MarginBottom(m) => {
            let Some(value) = margin_value(m, style.font.font_size, doc.root_font_size()) else { return };
            (style.box_model.margin_bottom, style.layout.margin_bottom_auto) = value;
        }
        Property::MarginLeft(m) => {
            let Some(value) = margin_value(m, style.font.font_size, doc.root_font_size()) else { return };
            (style.box_model.margin_left, style.layout.margin_left_auto) = value;
        }
        Property::MarginRight(m) => {
            let Some(value) = margin_value(m, style.font.font_size, doc.root_font_size()) else { return };
            (style.box_model.margin_right, style.layout.margin_right_auto) = value;
        }
        Property::Margin(m) => {
            let (Some(top), Some(bottom), Some(left), Some(right)) = (
                margin_value(&m.top, style.font.font_size, doc.root_font_size()),
                margin_value(&m.bottom, style.font.font_size, doc.root_font_size()),
                margin_value(&m.left, style.font.font_size, doc.root_font_size()),
                margin_value(&m.right, style.font.font_size, doc.root_font_size()),
            ) else {
                return;
            };
            (style.box_model.margin_top, style.layout.margin_top_auto) = top;
            (style.box_model.margin_bottom, style.layout.margin_bottom_auto) = bottom;
            (style.box_model.margin_left, style.layout.margin_left_auto) = left;
            (style.box_model.margin_right, style.layout.margin_right_auto) = right;
        }
        Property::MarginBlock(m) => {
            let (Some(start), Some(end)) = (margin_value(&m.block_start, style.font.font_size, doc.root_font_size()), margin_value(&m.block_end, style.font.font_size, doc.root_font_size())) else { return };
            set_margin_side(style, 0, start);
            set_margin_side(style, 2, end);
        }
        Property::MarginBlockStart(m) => {
            let Some(value) = margin_value(m, style.font.font_size, doc.root_font_size()) else { return };
            set_margin_side(style, 0, value);
        }
        Property::MarginBlockEnd(m) => {
            let Some(value) = margin_value(m, style.font.font_size, doc.root_font_size()) else { return };
            set_margin_side(style, 2, value);
        }
        Property::MarginInline(m) => {
            let (start, end) = inline_sides(style.text.direction);
            let (Some(start_value), Some(end_value)) = (margin_value(&m.inline_start, style.font.font_size, doc.root_font_size()), margin_value(&m.inline_end, style.font.font_size, doc.root_font_size())) else { return };
            set_margin_side(style, start, start_value);
            set_margin_side(style, end, end_value);
        }
        Property::MarginInlineStart(m) => {
            let (start, _) = inline_sides(style.text.direction);
            let Some(value) = margin_value(m, style.font.font_size, doc.root_font_size()) else { return };
            set_margin_side(style, start, value);
        }
        Property::MarginInlineEnd(m) => {
            let (_, end) = inline_sides(style.text.direction);
            let Some(value) = margin_value(m, style.font.font_size, doc.root_font_size()) else { return };
            set_margin_side(style, end, value);
        }

        // Padding - uses LengthPercentageOrAuto in lightningcss
        Property::PaddingTop(p) => {
            let Some(value) = length_or_auto_to_lengthpct(p, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct) else { return };
            style.box_model.padding_top = value;
        }
        Property::PaddingBottom(p) => {
            let Some(value) = length_or_auto_to_lengthpct(p, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct) else { return };
            style.box_model.padding_bottom = value;
        }
        Property::PaddingLeft(p) => {
            let Some(value) = length_or_auto_to_lengthpct(p, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct) else { return };
            style.box_model.padding_left = value;
        }
        Property::PaddingRight(p) => {
            let Some(value) = length_or_auto_to_lengthpct(p, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct) else { return };
            style.box_model.padding_right = value;
        }
        Property::Padding(p) => {
            let (Some(top), Some(right), Some(bottom), Some(left)) = (
                length_or_auto_to_lengthpct(&p.top, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(&p.right, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(&p.bottom, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(&p.left, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct),
            ) else {
                return;
            };
            style.box_model.padding_top = top;
            style.box_model.padding_right = right;
            style.box_model.padding_bottom = bottom;
            style.box_model.padding_left = left;
        }
        Property::PaddingBlock(p) => {
            let (Some(start), Some(end)) = (
                length_or_auto_to_lengthpct(&p.block_start, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(&p.block_end, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct),
            ) else {
                return;
            };
            set_padding_side(style, 0, start);
            set_padding_side(style, 2, end);
        }
        Property::PaddingBlockStart(p) => {
            let Some(value) = length_or_auto_to_lengthpct(p, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct) else { return };
            set_padding_side(style, 0, value);
        }
        Property::PaddingBlockEnd(p) => {
            let Some(value) = length_or_auto_to_lengthpct(p, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct) else { return };
            set_padding_side(style, 2, value);
        }
        Property::PaddingInline(p) => {
            let (Some(start_value), Some(end_value)) = (
                length_or_auto_to_lengthpct(&p.inline_start, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct),
                length_or_auto_to_lengthpct(&p.inline_end, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct),
            ) else {
                return;
            };
            let (start, end) = inline_sides(style.text.direction);
            set_padding_side(style, start, start_value);
            set_padding_side(style, end, end_value);
        }
        Property::PaddingInlineStart(p) => {
            let Some(value) = length_or_auto_to_lengthpct(p, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct) else { return };
            let (start, _) = inline_sides(style.text.direction);
            set_padding_side(style, start, value);
        }
        Property::PaddingInlineEnd(p) => {
            let Some(value) = length_or_auto_to_lengthpct(p, style.font.font_size, doc.root_font_size()).and_then(non_negative_length_pct) else { return };
            let (_, end) = inline_sides(style.text.direction);
            set_padding_side(style, end, value);
        }

        // Border (per-side)
        Property::BorderWidth(bw) => {
            let (Some(top), Some(right), Some(bottom), Some(left)) = (
                checked_border_width(&bw.top, style.font.font_size, doc.root_font_size()),
                checked_border_width(&bw.right, style.font.font_size, doc.root_font_size()),
                checked_border_width(&bw.bottom, style.font.font_size, doc.root_font_size()),
                checked_border_width(&bw.left, style.font.font_size, doc.root_font_size()),
            ) else {
                return;
            };
            style.border.border_top_width = top;
            style.border.border_right_width = right;
            style.border.border_bottom_width = bottom;
            style.border.border_left_width = left;
        }
        Property::BorderTopWidth(w) => {
            let Some(width) = checked_border_width(w, style.font.font_size, doc.root_font_size()) else { return };
            style.border.border_top_width = width;
        }
        Property::BorderRightWidth(w) => {
            let Some(width) = checked_border_width(w, style.font.font_size, doc.root_font_size()) else { return };
            style.border.border_right_width = width;
        }
        Property::BorderBottomWidth(w) => {
            let Some(width) = checked_border_width(w, style.font.font_size, doc.root_font_size()) else { return };
            style.border.border_bottom_width = width;
        }
        Property::BorderLeftWidth(w) => {
            let Some(width) = checked_border_width(w, style.font.font_size, doc.root_font_size()) else { return };
            style.border.border_left_width = width;
        }
        Property::BorderColor(bc) => {
            set_border_css_color(style, 0, &bc.top);
            set_border_css_color(style, 1, &bc.right);
            set_border_css_color(style, 2, &bc.bottom);
            set_border_css_color(style, 3, &bc.left);
        }
        Property::BorderTopColor(c) => {
            set_border_css_color(style, 0, c);
        }
        Property::BorderRightColor(c) => {
            set_border_css_color(style, 1, c);
        }
        Property::BorderBottomColor(c) => {
            set_border_css_color(style, 2, c);
        }
        Property::BorderLeftColor(c) => {
            set_border_css_color(style, 3, c);
        }
        Property::BorderStyle(bs) => {
            style.border.border_top_style = line_style_to_border_style(&bs.top);
            style.border.border_right_style = line_style_to_border_style(&bs.right);
            style.border.border_bottom_style = line_style_to_border_style(&bs.bottom);
            style.border.border_left_style = line_style_to_border_style(&bs.left);
        }
        Property::BorderTopStyle(s) => {
            style.border.border_top_style = line_style_to_border_style(s);
        }
        Property::BorderRightStyle(s) => {
            style.border.border_right_style = line_style_to_border_style(s);
        }
        Property::BorderBottomStyle(s) => {
            style.border.border_bottom_style = line_style_to_border_style(s);
        }
        Property::BorderLeftStyle(s) => {
            style.border.border_left_style = line_style_to_border_style(s);
        }
        Property::Border(b) => {
            let Some(width) = checked_border_width(&b.width, style.font.font_size, doc.root_font_size()) else { return };
            let border_style = line_style_to_border_style(&b.style);
            style.border.border_top_width = width;
            style.border.border_right_width = width;
            style.border.border_bottom_width = width;
            style.border.border_left_width = width;
            set_border_css_color(style, 0, &b.color);
            set_border_css_color(style, 1, &b.color);
            set_border_css_color(style, 2, &b.color);
            set_border_css_color(style, 3, &b.color);
            style.border.border_top_style = border_style;
            style.border.border_right_style = border_style;
            style.border.border_bottom_style = border_style;
            style.border.border_left_style = border_style;
        }
        Property::BorderTop(b) => {
            let Some(width) = checked_border_width(&b.width, style.font.font_size, doc.root_font_size()) else { return };
            style.border.border_top_width = width;
            set_border_css_color(style, 0, &b.color);
            style.border.border_top_style = line_style_to_border_style(&b.style);
        }
        Property::BorderRight(b) => {
            let Some(width) = checked_border_width(&b.width, style.font.font_size, doc.root_font_size()) else { return };
            style.border.border_right_width = width;
            set_border_css_color(style, 1, &b.color);
            style.border.border_right_style = line_style_to_border_style(&b.style);
        }
        Property::BorderBottom(b) => {
            let Some(width) = checked_border_width(&b.width, style.font.font_size, doc.root_font_size()) else { return };
            style.border.border_bottom_width = width;
            set_border_css_color(style, 2, &b.color);
            style.border.border_bottom_style = line_style_to_border_style(&b.style);
        }
        Property::BorderLeft(b) => {
            let Some(width) = checked_border_width(&b.width, style.font.font_size, doc.root_font_size()) else { return };
            style.border.border_left_width = width;
            set_border_css_color(style, 3, &b.color);
            style.border.border_left_style = line_style_to_border_style(&b.style);
        }
        Property::BorderRadius(value, _) => {
            let (Some(top_left), Some(top_right), Some(bottom_right), Some(bottom_left)) = (
                corner_radius(&value.top_left, style.font.font_size, doc.root_font_size()),
                corner_radius(&value.top_right, style.font.font_size, doc.root_font_size()),
                corner_radius(&value.bottom_right, style.font.font_size, doc.root_font_size()),
                corner_radius(&value.bottom_left, style.font.font_size, doc.root_font_size()),
            ) else {
                return;
            };
            style.radii.top_left = top_left;
            style.radii.top_right = top_right;
            style.radii.bottom_right = bottom_right;
            style.radii.bottom_left = bottom_left;
        }
        Property::BorderTopLeftRadius(value, _) => {
            let Some(value) = corner_radius(value, style.font.font_size, doc.root_font_size()) else { return };
            style.radii.top_left = value;
        }
        Property::BorderTopRightRadius(value, _) => {
            let Some(value) = corner_radius(value, style.font.font_size, doc.root_font_size()) else { return };
            style.radii.top_right = value;
        }
        Property::BorderBottomRightRadius(value, _) => {
            let Some(value) = corner_radius(value, style.font.font_size, doc.root_font_size()) else { return };
            style.radii.bottom_right = value;
        }
        Property::BorderBottomLeftRadius(value, _) => {
            let Some(value) = corner_radius(value, style.font.font_size, doc.root_font_size()) else { return };
            style.radii.bottom_left = value;
        }
        Property::BorderStartStartRadius(value) => {
            let Some(value) = corner_radius(value, style.font.font_size, doc.root_font_size()) else { return };
            set_corner_radius(style, if style.text.direction == TextDirection::Ltr { 0 } else { 1 }, value);
        }
        Property::BorderStartEndRadius(value) => {
            let Some(value) = corner_radius(value, style.font.font_size, doc.root_font_size()) else { return };
            set_corner_radius(style, if style.text.direction == TextDirection::Ltr { 1 } else { 0 }, value);
        }
        Property::BorderEndStartRadius(value) => {
            let Some(value) = corner_radius(value, style.font.font_size, doc.root_font_size()) else { return };
            set_corner_radius(style, if style.text.direction == TextDirection::Ltr { 3 } else { 2 }, value);
        }
        Property::BorderEndEndRadius(value) => {
            let Some(value) = corner_radius(value, style.font.font_size, doc.root_font_size()) else { return };
            set_corner_radius(style, if style.text.direction == TextDirection::Ltr { 2 } else { 3 }, value);
        }

        // Logical borders are resolved here, while `direction` is known, and
        // only renderer-owned physical sides cross into layout.
        Property::BorderBlockStartWidth(value) => {
            let Some(value) = checked_border_width(value, style.font.font_size, doc.root_font_size()) else { return };
            set_border_width(style, 0, value);
        }
        Property::BorderBlockEndWidth(value) => {
            let Some(value) = checked_border_width(value, style.font.font_size, doc.root_font_size()) else { return };
            set_border_width(style, 2, value);
        }
        Property::BorderInlineStartWidth(value) => {
            let Some(value) = checked_border_width(value, style.font.font_size, doc.root_font_size()) else { return };
            set_border_width(style, inline_sides(style.text.direction).0, value);
        }
        Property::BorderInlineEndWidth(value) => {
            let Some(value) = checked_border_width(value, style.font.font_size, doc.root_font_size()) else { return };
            set_border_width(style, inline_sides(style.text.direction).1, value);
        }
        Property::BorderBlockWidth(value) => {
            let (Some(start), Some(end)) = (checked_border_width(&value.start, style.font.font_size, doc.root_font_size()), checked_border_width(&value.end, style.font.font_size, doc.root_font_size())) else { return };
            set_border_width(style, 0, start);
            set_border_width(style, 2, end);
        }
        Property::BorderInlineWidth(value) => {
            let (Some(start), Some(end)) = (checked_border_width(&value.start, style.font.font_size, doc.root_font_size()), checked_border_width(&value.end, style.font.font_size, doc.root_font_size())) else { return };
            let (start_side, end_side) = inline_sides(style.text.direction);
            set_border_width(style, start_side, start);
            set_border_width(style, end_side, end);
        }
        Property::BorderBlockStartStyle(value) => set_border_style(style, 0, line_style_to_border_style(value)),
        Property::BorderBlockEndStyle(value) => set_border_style(style, 2, line_style_to_border_style(value)),
        Property::BorderInlineStartStyle(value) => set_border_style(style, inline_sides(style.text.direction).0, line_style_to_border_style(value)),
        Property::BorderInlineEndStyle(value) => set_border_style(style, inline_sides(style.text.direction).1, line_style_to_border_style(value)),
        Property::BorderBlockStyle(value) => {
            set_border_style(style, 0, line_style_to_border_style(&value.start));
            set_border_style(style, 2, line_style_to_border_style(&value.end));
        }
        Property::BorderInlineStyle(value) => {
            let (start, end) = inline_sides(style.text.direction);
            set_border_style(style, start, line_style_to_border_style(&value.start));
            set_border_style(style, end, line_style_to_border_style(&value.end));
        }
        Property::BorderBlockStartColor(value) => set_border_css_color(style, 0, value),
        Property::BorderBlockEndColor(value) => set_border_css_color(style, 2, value),
        Property::BorderInlineStartColor(value) => set_border_css_color(style, inline_sides(style.text.direction).0, value),
        Property::BorderInlineEndColor(value) => set_border_css_color(style, inline_sides(style.text.direction).1, value),
        Property::BorderBlockColor(value) => {
            set_border_css_color(style, 0, &value.start);
            set_border_css_color(style, 2, &value.end);
        }
        Property::BorderInlineColor(value) => {
            let (start, end) = inline_sides(style.text.direction);
            set_border_css_color(style, start, &value.start);
            set_border_css_color(style, end, &value.end);
        }
        Property::BorderBlockStart(value) => apply_logical_border(style, 0, value, doc.root_font_size()),
        Property::BorderBlockEnd(value) => apply_logical_border(style, 2, value, doc.root_font_size()),
        Property::BorderInlineStart(value) => apply_logical_border(style, inline_sides(style.text.direction).0, value, doc.root_font_size()),
        Property::BorderInlineEnd(value) => apply_logical_border(style, inline_sides(style.text.direction).1, value, doc.root_font_size()),
        Property::BorderBlock(value) => {
            if checked_border_width(&value.width, style.font.font_size, doc.root_font_size()).is_none() {
                return;
            }
            apply_logical_border(style, 0, value, doc.root_font_size());
            apply_logical_border(style, 2, value, doc.root_font_size());
        }
        Property::BorderInline(value) => {
            if checked_border_width(&value.width, style.font.font_size, doc.root_font_size()).is_none() {
                return;
            }
            let (start, end) = inline_sides(style.text.direction);
            apply_logical_border(style, start, value, doc.root_font_size());
            apply_logical_border(style, end, value, doc.root_font_size());
        }

        // Display
        Property::Display(d) => {
            use lightningcss::properties::display::{Display as LcDisplay, DisplayInside, DisplayKeyword, DisplayOutside};
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
                    DisplayKeyword::RubyBase | DisplayKeyword::RubyText | DisplayKeyword::RubyBaseContainer | DisplayKeyword::RubyTextContainer => return,
                },
                LcDisplay::Pair(pair) if pair.is_list_item && matches!(pair.outside, DisplayOutside::Block) && matches!(pair.inside, DisplayInside::Flow) => Display::ListItem,
                LcDisplay::Pair(pair) if pair.is_list_item && matches!(pair.outside, DisplayOutside::Block) && matches!(pair.inside, DisplayInside::FlowRoot) => Display::FlowRootListItem,
                LcDisplay::Pair(pair) if pair.is_list_item => return,
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
                    _ => return,
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
            let Some(value) = checked_flex_factor(*value) else { return };
            style.layout.flex_grow = value;
        }
        Property::FlexShrink(value, _) => {
            let Some(value) = checked_flex_factor(*value) else { return };
            style.layout.flex_shrink = value;
        }
        Property::FlexBasis(value, _) => {
            let Some(value) = flex_basis(value, &style.font, doc.root_font_size(), styles) else { return };
            style.layout.flex_basis = value;
        }
        Property::Flex(value, _) => {
            let Some(basis) = flex_basis(&value.basis, &style.font, doc.root_font_size(), styles) else { return };
            let (Some(grow), Some(shrink)) = (checked_flex_factor(value.grow), checked_flex_factor(value.shrink)) else { return };
            style.layout.flex_grow = grow;
            style.layout.flex_shrink = shrink;
            style.layout.flex_basis = basis;
        }
        Property::Order(value, _) => style.layout.order = *value,
        Property::AlignContent(value, _) => {
            let Some(value) = content_alignment(value) else { return };
            style.layout.align_content = value;
        }
        Property::JustifyContent(value, _) => style.layout.justify_content = justify_content(value),
        Property::AlignItems(value, _) => {
            let Some(value) = align_items(value) else { return };
            style.layout.align_items = value;
        }
        Property::AlignSelf(value, _) => {
            let Some(value) = align_self(value) else { return };
            style.layout.align_self = value;
        }
        Property::JustifyItems(value) => {
            let Some(value) = justify_items(value) else { return };
            style.layout.justify_items = value;
        }
        Property::JustifySelf(value) => {
            let Some(value) = justify_self(value) else { return };
            style.layout.justify_self = value;
        }
        Property::PlaceContent(value) => {
            let Some(align) = content_alignment(&value.align) else { return };
            style.layout.align_content = align;
            style.layout.justify_content = justify_content(&value.justify);
        }
        Property::PlaceItems(value) => {
            let (Some(align), Some(justify)) = (align_items(&value.align), justify_items(&value.justify)) else { return };
            style.layout.align_items = align;
            style.layout.justify_items = justify;
        }
        Property::PlaceSelf(value) => {
            let (Some(align), Some(justify)) = (align_self(&value.align), justify_self(&value.justify)) else { return };
            style.layout.align_self = align;
            style.layout.justify_self = justify;
        }
        Property::RowGap(value) => {
            let Some(value) = gap_value(value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.row_gap = value;
        }
        Property::ColumnGap(value) => {
            let Some(value) = gap_value(value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.column_gap = value;
        }
        Property::Gap(value) => {
            let (Some(row), Some(column)) = (gap_value(&value.row, style.font.font_size, doc.root_font_size()), gap_value(&value.column, style.font.font_size, doc.root_font_size())) else {
                return;
            };
            style.layout.row_gap = row;
            style.layout.column_gap = column;
        }
        Property::GridTemplateRows(value) => {
            let Some((tracks, names)) = grid_template_tracks(styles, value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.grid_template_rows = tracks;
            style.layout.grid_template_row_names = names;
        }
        Property::GridTemplateColumns(value) => {
            let Some((tracks, names)) = grid_template_tracks(styles, value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.grid_template_columns = tracks;
            style.layout.grid_template_column_names = names;
        }
        Property::GridAutoRows(value) => {
            let Some(value) = grid_auto_tracks(value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.grid_auto_rows = value;
        }
        Property::GridAutoColumns(value) => {
            let Some(value) = grid_auto_tracks(value, style.font.font_size, doc.root_font_size()) else { return };
            style.layout.grid_auto_columns = value;
        }
        Property::GridAutoFlow(value) => style.layout.grid_auto_flow = grid_auto_flow(*value),
        Property::GridTemplateAreas(value) => {
            let Some(value) = grid_template_areas(styles, value) else { return };
            style.layout.grid_template_areas = value;
        }
        Property::GridTemplate(value) => {
            let (Some((rows, row_names)), Some((columns, column_names)), Some(areas)) =
                (grid_template_tracks(styles, &value.rows, style.font.font_size, doc.root_font_size()), grid_template_tracks(styles, &value.columns, style.font.font_size, doc.root_font_size()), grid_template_areas(styles, &value.areas))
            else {
                return;
            };
            style.layout.grid_template_rows = rows;
            style.layout.grid_template_row_names = row_names;
            style.layout.grid_template_columns = columns;
            style.layout.grid_template_column_names = column_names;
            style.layout.grid_template_areas = areas;
        }
        Property::Grid(value) => {
            let (Some((rows, row_names)), Some((columns, column_names)), Some(areas), Some(auto_rows), Some(auto_columns)) = (
                grid_template_tracks(styles, &value.rows, style.font.font_size, doc.root_font_size()),
                grid_template_tracks(styles, &value.columns, style.font.font_size, doc.root_font_size()),
                grid_template_areas(styles, &value.areas),
                grid_auto_tracks(&value.auto_rows, style.font.font_size, doc.root_font_size()),
                grid_auto_tracks(&value.auto_columns, style.font.font_size, doc.root_font_size()),
            ) else {
                return;
            };
            style.layout.grid_template_rows = rows;
            style.layout.grid_template_row_names = row_names;
            style.layout.grid_template_columns = columns;
            style.layout.grid_template_column_names = column_names;
            style.layout.grid_template_areas = areas;
            style.layout.grid_auto_rows = auto_rows;
            style.layout.grid_auto_columns = auto_columns;
            style.layout.grid_auto_flow = grid_auto_flow(value.auto_flow);
        }
        Property::GridRowStart(value) => {
            let Some(value) = grid_placement(styles, value) else { return };
            style.layout.grid_row.start = value;
        }
        Property::GridRowEnd(value) => {
            let Some(value) = grid_placement(styles, value) else { return };
            style.layout.grid_row.end = value;
        }
        Property::GridColumnStart(value) => {
            let Some(value) = grid_placement(styles, value) else { return };
            style.layout.grid_column.start = value;
        }
        Property::GridColumnEnd(value) => {
            let Some(value) = grid_placement(styles, value) else { return };
            style.layout.grid_column.end = value;
        }
        Property::GridRow(value) => {
            let (Some(start), Some(end)) = (grid_placement(styles, &value.start), grid_placement(styles, &value.end)) else { return };
            style.layout.grid_row = GridPlacementRange { start, end };
        }
        Property::GridColumn(value) => {
            let (Some(start), Some(end)) = (grid_placement(styles, &value.start), grid_placement(styles, &value.end)) else { return };
            style.layout.grid_column = GridPlacementRange { start, end };
        }
        Property::GridArea(value) => {
            let (Some(row_start), Some(column_start), Some(row_end), Some(column_end)) =
                (grid_placement(styles, &value.row_start), grid_placement(styles, &value.column_start), grid_placement(styles, &value.row_end), grid_placement(styles, &value.column_end))
            else {
                return;
            };
            style.layout.grid_row = GridPlacementRange { start: row_start, end: row_end };
            style.layout.grid_column = GridPlacementRange { start: column_start, end: column_end };
        }
        Property::Custom(custom) => match custom.name.as_ref().to_ascii_lowercase().as_str() {
            "float" => {
                if let Some(value) = single_ident_keyword(&custom.value).map(str::to_ascii_lowercase) {
                    style.box_model.float = logical_float(value.as_str(), style.text.direction, style.box_model.float);
                }
            }
            "clear" => {
                if let Some(value) = single_ident_keyword(&custom.value).map(str::to_ascii_lowercase) {
                    style.box_model.clear = logical_clear(value.as_str(), style.text.direction, style.box_model.clear);
                }
            }
            _ => {}
        },
        Property::Hyphens(value, _) => {
            use lightningcss::properties::text::Hyphens as CssHyphens;
            style.text.hyphens = match value {
                CssHyphens::None => Hyphens::None,
                CssHyphens::Manual => Hyphens::Manual,
                CssHyphens::Auto => Hyphens::Auto,
            };
        }
        Property::WordBreak(wb) => {
            use lightningcss::properties::text::WordBreak as LcWordBreak;
            style.text.word_break = match wb {
                LcWordBreak::Normal => WordBreak::Normal,
                LcWordBreak::BreakAll => WordBreak::BreakAll,
                LcWordBreak::KeepAll => WordBreak::KeepAll,
                // Legacy value: behaves as `overflow-wrap: break-word`.
                LcWordBreak::BreakWord => {
                    if style.text.overflow_wrap == OverflowWrap::Normal {
                        style.text.overflow_wrap = OverflowWrap::BreakWord;
                    }
                    WordBreak::Normal
                }
            };
        }
        Property::OverflowWrap(ow) | Property::WordWrap(ow) => {
            use lightningcss::properties::text::OverflowWrap as LcOverflowWrap;
            style.text.overflow_wrap = match ow {
                LcOverflowWrap::Normal => OverflowWrap::Normal,
                LcOverflowWrap::BreakWord => OverflowWrap::BreakWord,
                LcOverflowWrap::Anywhere => OverflowWrap::Anywhere,
            };
        }
        Property::BoxSizing(bs, _) => {
            use lightningcss::properties::size::BoxSizing as LcBoxSizing;
            style.box_model.box_sizing = match bs {
                LcBoxSizing::ContentBox => BoxSizing::ContentBox,
                LcBoxSizing::BorderBox => BoxSizing::BorderBox,
            };
        }

        _ => {
            if let Property::Unparsed(unparsed) = property {
                match unparsed.property_id.name() {
                    "page-break-before" | "page-break-after" | "page-break-inside" | "break-before" | "break-after" | "break-inside" => {
                        // Not supported yet.
                        return;
                    }
                    "content" => {
                        if let Some(content) = parse_generated_content(styles, &unparsed.value) {
                            style.generated_content = content;
                            return;
                        }
                    }
                    "float" => {
                        if let Some(value) = single_ident_keyword(&unparsed.value).map(str::to_ascii_lowercase) {
                            style.box_model.float = logical_float(value.as_str(), style.text.direction, style.box_model.float);
                            return;
                        }
                    }
                    "clear" => {
                        if let Some(value) = single_ident_keyword(&unparsed.value).map(str::to_ascii_lowercase) {
                            style.box_model.clear = logical_clear(value.as_str(), style.text.direction, style.box_model.clear);
                            return;
                        }
                    }
                    "flex-basis" => {
                        if let Some(value) = single_ident_keyword(&unparsed.value).map(str::to_ascii_lowercase) {
                            style.layout.flex_basis = match value.as_str() {
                                "min-content" => PreferredSize::MinContent,
                                "max-content" => PreferredSize::MaxContent,
                                "fit-content" => PreferredSize::FitContent,
                                _ => style.layout.flex_basis,
                            };
                            return;
                        }
                    }
                    _ => {}
                }
            }
            let prop_name = property.property_id().name().to_string();
            if matches!(prop_name.as_str(), "float" | "clear" | "css-float" | "css-clear")
                && let Ok(css) = property.to_css_string(false, PrinterOptions::default())
                && let Some(value) = css.split(':').nth(1).map(|v| v.trim().to_ascii_lowercase())
            {
                let value = value.as_str();
                if prop_name == "float" {
                    style.box_model.float = logical_float(value, style.text.direction, style.box_model.float);
                    return;
                }
                style.box_model.clear = logical_clear(value, style.text.direction, style.box_model.clear);
                return;
            }
            log_unhandled_property(property);
        }
    }
}

fn parse_break_between(tokens: &TokenList<'_>) -> Option<BreakBetween> {
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

fn parse_break_inside(tokens: &TokenList<'_>) -> Option<BreakInside> {
    match single_ident_keyword(tokens)?.to_ascii_lowercase().as_str() {
        "auto" => Some(BreakInside::Auto),
        "avoid" | "avoid-page" | "avoid-column" => Some(BreakInside::Avoid),
        _ => None,
    }
}

fn single_positive_integer(tokens: &TokenList<'_>) -> Option<u8> {
    let mut values = tokens.0.iter().filter(|token| !is_ignorable_token(token));
    let value = match values.next()? {
        TokenOrValue::Token(Token::Number { int_value: Some(value), .. }) => *value,
        _ => return None,
    };
    (values.next().is_none() && value > 0).then(|| value.min(u8::MAX as i32) as u8)
}

fn apply_custom_box_keyword(style: &mut WorkingStyle, name: &str, tokens: &TokenList<'_>) -> bool {
    let Some(value) = single_ident_keyword(tokens).map(str::to_ascii_lowercase) else {
        return false;
    };
    match name.to_ascii_lowercase().as_str() {
        "float" => style.box_model.float = logical_float(&value, style.text.direction, style.box_model.float),
        "clear" => style.box_model.clear = logical_clear(&value, style.text.direction, style.box_model.clear),
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

/// Parse the CSS 2.1 generated-content forms that do not require counter or
/// replaced-image state. The outer `Option` distinguishes an invalid/unsupported
/// declaration from a valid suppressing value (`normal` or `none`).
fn parse_generated_content(styles: &mut ComputedStylesBuilder, tokens: &TokenList<'_>) -> Option<Option<GeneratedContent>> {
    let significant: Vec<&TokenOrValue<'_>> = tokens.0.iter().filter(|token| !matches!(token, TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_)))).collect();
    if significant.len() == 1
        && let TokenOrValue::Token(Token::Ident(keyword)) = significant[0]
    {
        return match keyword.as_ref().to_ascii_lowercase().as_str() {
            "normal" | "none" | "initial" | "unset" => Some(None),
            // `content` is not inherited; on a generated pseudo-element its
            // originating element normally computes to `normal`.
            "inherit" => Some(None),
            "revert" | "revert-layer" => None,
            _ => parse_generated_content_items(styles, &significant).map(|items| Some(GeneratedContent { items })),
        };
    }
    parse_generated_content_items(styles, &significant).map(|items| Some(GeneratedContent { items }))
}

fn parse_generated_content_items(styles: &mut ComputedStylesBuilder, tokens: &[&TokenOrValue<'_>]) -> Option<Vec<GeneratedContentItem>> {
    if tokens.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(tokens.len());
    for token in tokens {
        let item = match token {
            TokenOrValue::Token(Token::String(value)) => GeneratedContentItem::Text(styles.intern_string(value.as_ref())),
            TokenOrValue::Token(Token::Ident(value)) => match value.as_ref().to_ascii_lowercase().as_str() {
                "open-quote" => GeneratedContentItem::OpenQuote,
                "close-quote" => GeneratedContentItem::CloseQuote,
                "no-open-quote" => GeneratedContentItem::NoOpenQuote,
                "no-close-quote" => GeneratedContentItem::NoCloseQuote,
                _ => return None,
            },
            TokenOrValue::Function(function) if function.name.as_ref().eq_ignore_ascii_case("attr") => {
                let name = function.arguments.0.iter().find_map(|argument| match argument {
                    TokenOrValue::Token(Token::Ident(name)) => Some(name.as_ref()),
                    _ => None,
                })?;
                GeneratedContentItem::Attribute(styles.intern_string(name))
            }
            TokenOrValue::Function(function) if function.name.as_ref().eq_ignore_ascii_case("counter") => {
                let arguments = significant_tokens(&function.arguments);
                let name = token_ident(*arguments.first()?)?;
                let style = match arguments.as_slice() {
                    [_] => CounterStyle::Decimal,
                    [_, TokenOrValue::Token(Token::Comma), style] => parse_counter_style(token_ident(style)?)?,
                    _ => return None,
                };
                GeneratedContentItem::Counter { name: styles.intern_string(name), style }
            }
            TokenOrValue::Function(function) if function.name.as_ref().eq_ignore_ascii_case("counters") => {
                let arguments = significant_tokens(&function.arguments);
                let (name, separator, counter_style) = match arguments.as_slice() {
                    [name, TokenOrValue::Token(Token::Comma), TokenOrValue::Token(Token::String(separator))] => (token_ident(name)?, separator.as_ref(), CounterStyle::Decimal),
                    [name, TokenOrValue::Token(Token::Comma), TokenOrValue::Token(Token::String(separator)), TokenOrValue::Token(Token::Comma), style] => (token_ident(name)?, separator.as_ref(), parse_counter_style(token_ident(style)?)?),
                    _ => return None,
                };
                GeneratedContentItem::Counters { name: styles.intern_string(name), separator: styles.intern_string(separator), style: counter_style }
            }
            // Generated replaced images require their own layout path and are
            // deliberately not misrepresented as text here.
            _ => return None,
        };
        items.push(item);
    }
    Some(items)
}

fn significant_tokens<'a, 'i>(tokens: &'a TokenList<'i>) -> Vec<&'a TokenOrValue<'i>> {
    tokens.0.iter().filter(|token| !matches!(token, TokenOrValue::Token(Token::WhiteSpace(_) | Token::Comment(_)))).collect()
}

fn token_ident<'a, 'i>(token: &'a TokenOrValue<'i>) -> Option<&'a str> {
    match token {
        TokenOrValue::Token(Token::Ident(value)) => Some(value.as_ref()),
        _ => None,
    }
}

fn parse_counter_style(value: &str) -> Option<CounterStyle> {
    match value.to_ascii_lowercase().as_str() {
        "decimal" => Some(CounterStyle::Decimal),
        "decimal-leading-zero" => Some(CounterStyle::DecimalLeadingZero),
        "lower-roman" => Some(CounterStyle::LowerRoman),
        "upper-roman" => Some(CounterStyle::UpperRoman),
        "lower-greek" => Some(CounterStyle::LowerGreek),
        "lower-alpha" | "lower-latin" => Some(CounterStyle::LowerAlpha),
        "upper-alpha" | "upper-latin" => Some(CounterStyle::UpperAlpha),
        "armenian" => Some(CounterStyle::Armenian),
        "georgian" => Some(CounterStyle::Georgian),
        "disc" => Some(CounterStyle::Disc),
        "circle" => Some(CounterStyle::Circle),
        "square" => Some(CounterStyle::Square),
        _ => None,
    }
}

fn parse_counter_directives(styles: &mut ComputedStylesBuilder, tokens: &TokenList<'_>, default_value: i32) -> Option<Vec<CounterDirective>> {
    let tokens = significant_tokens(tokens);
    if tokens.len() == 1 && token_ident(tokens[0]).is_some_and(|ident| ident.eq_ignore_ascii_case("none")) {
        return Some(Vec::new());
    }
    let mut directives = Vec::new();
    let mut cursor = 0;
    while cursor < tokens.len() {
        let name = token_ident(tokens[cursor])?;
        if name.eq_ignore_ascii_case("none") || name.eq_ignore_ascii_case("inherit") || name.eq_ignore_ascii_case("initial") || name.eq_ignore_ascii_case("unset") {
            return None;
        }
        cursor += 1;
        let value = match tokens.get(cursor) {
            Some(TokenOrValue::Token(Token::Number { int_value: Some(value), .. })) => {
                cursor += 1;
                value.to_owned()
            }
            _ => default_value,
        };
        directives.push(CounterDirective { name: styles.intern_string(name), value });
    }
    (!directives.is_empty()).then_some(directives)
}

fn parse_quotes(styles: &mut ComputedStylesBuilder, tokens: &TokenList<'_>) -> Option<QuoteStyle> {
    let tokens = significant_tokens(tokens);
    if tokens.len() == 1 {
        return match token_ident(tokens[0])?.to_ascii_lowercase().as_str() {
            "auto" => Some(QuoteStyle::Auto),
            "none" => Some(QuoteStyle::None),
            _ => None,
        };
    }
    if tokens.is_empty() || tokens.len() % 2 != 0 {
        return None;
    }
    let mut encoded = String::new();
    for (index, token) in tokens.iter().enumerate() {
        let TokenOrValue::Token(Token::String(value)) = token else { return None };
        if index > 0 {
            encoded.push('\0');
        }
        encoded.push_str(value.as_ref());
    }
    Some(QuoteStyle::Pairs(styles.intern_string(&encoded)))
}

/// Lightning CSS preserves the legacy Grid gap aliases as raw custom
/// declarations. Resolve them with the canonical Box Alignment grammar while
/// keeping their independent place in cascade order.
fn try_apply_legacy_grid_gap_alias(doc: &Document, style: &mut WorkingStyle, property: &Property<'_>, parent: &ParentStyle) -> bool {
    let (name, tokens) = match property {
        Property::Custom(custom) => (custom.name.as_ref(), &custom.value),
        Property::Unparsed(unparsed) => (unparsed.property_id.name(), &unparsed.value),
        _ => return false,
    };
    let row_axis = match name {
        "grid-row-gap" => true,
        "grid-column-gap" => false,
        _ => return false,
    };

    if let Some(keyword) = single_ident_keyword(tokens).map(str::to_ascii_lowercase) {
        let inherited = if row_axis { parent.layout.row_gap } else { parent.layout.column_gap };
        let value = match keyword.as_str() {
            "inherit" => Some(inherited),
            "initial" | "unset" => Some(LengthPct::Px(0.0)),
            "revert" | "revert-layer" => None,
            _ => None,
        };
        if let Some(value) = value {
            if row_axis {
                style.layout.row_gap = value;
            } else {
                style.layout.column_gap = value;
            }
            return true;
        }
        if matches!(keyword.as_str(), "revert" | "revert-layer") {
            return true;
        }
    }

    let Some(css) = token_list_to_css_string(tokens) else { return true };
    let canonical = if row_axis { "row-gap" } else { "column-gap" };
    let Ok(property) = Property::parse_string(PropertyId::from(canonical), &css, ParserOptions::default()) else { return true };
    let parsed = match property {
        Property::RowGap(value) | Property::ColumnGap(value) => gap_value(&value, style.font.font_size, doc.root_font_size()),
        _ => None,
    };
    if let Some(value) = parsed {
        if row_axis {
            style.layout.row_gap = value;
        } else {
            style.layout.column_gap = value;
        }
    }
    true
}

/// Whether a property inherits by default (decides what `unset` means).
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
fn try_apply_css_wide_keyword(style: &mut WorkingStyle, property: &Property, parent: &ParentStyle, root_font_size: f32, phase: CascadePhase, revert_basis: &WorkingStyle, revert_layer_basis: &WorkingStyle) -> bool {
    let (name, value) = match property {
        Property::Unparsed(unparsed) => (unparsed.property_id.name(), &unparsed.value),
        Property::Custom(custom) if !custom.name.as_ref().starts_with("--") => (custom.name.as_ref(), &custom.value),
        _ => return false,
    };
    let Some(keyword) = single_ident_keyword(value).map(str::to_ascii_lowercase) else {
        return false;
    };

    apply_css_wide_keyword_in_phase(style, name, &keyword, parent, root_font_size, phase, Some(revert_basis), Some(revert_layer_basis))
}

fn apply_css_wide_keyword_in_phase(
    style: &mut WorkingStyle, name: &str, keyword: &str, parent: &ParentStyle, root_font_size: f32, phase: CascadePhase, revert_basis: Option<&WorkingStyle>, revert_layer_basis: Option<&WorkingStyle>,
) -> bool {
    if phase == CascadePhase::Remaining && matches!(name, "color" | "direction" | "font-size" | "line-height") {
        return matches!(keyword, "inherit" | "initial" | "unset" | "revert" | "revert-layer");
    }
    if phase != CascadePhase::Remaining || name != "font" {
        return apply_css_wide_keyword_with_rollback(style, name, keyword, parent, root_font_size, revert_basis, revert_layer_basis);
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
    let final_line_height = (style.line_height_spec.clone(), style.text.line_height_number, style.text.line_height, style.text.line_height_x_height_px, style.text.line_height_normal);
    let consumed = apply_css_wide_keyword_with_rollback(style, name, keyword, parent, root_font_size, revert_basis, revert_layer_basis);
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

fn apply_css_wide_keyword_with_rollback(style: &mut WorkingStyle, name: &str, keyword: &str, parent: &ParentStyle, root_font_size: f32, revert_basis: Option<&WorkingStyle>, revert_layer_basis: Option<&WorkingStyle>) -> bool {
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
            let Some(basis) = revert_basis else { return true };
            reverted = ParentStyle::from_working(basis);
            &reverted
        }
        "revert-layer" => {
            let Some(basis) = revert_layer_basis else { return true };
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
        "font-variant-caps" => style.font.font_variant_caps_features = src.font.font_variant_caps_features.clone(),
        "font-variant-numeric" => style.font.font_variant_numeric_features = src.font.font_variant_numeric_features.clone(),
        "font-variant-ligatures" => style.font.font_variant_ligature_features = src.font.font_variant_ligature_features.clone(),
        "font-kerning" => style.font.font_kerning_features = src.font.font_kerning_features.clone(),
        "font-feature-settings" => style.font.font_feature_settings = src.font.font_feature_settings.clone(),
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
        "text-decoration-line" => style.background.text_decoration.lines = src.background.text_decoration.lines,
        "text-decoration-color" => style.background.text_decoration.color = src.background.text_decoration.color,
        "text-decoration-style" => style.background.text_decoration.style = src.background.text_decoration.style,
        "text-decoration-thickness" => style.background.text_decoration.thickness = src.background.text_decoration.thickness,
        "outline" => style.background.outline = src.background.outline,
        "outline-width" => {
            style.background.outline.set_width(src.background.outline.width());
        }
        "outline-style" => style.background.outline.style = src.background.outline.style,
        "outline-color" => style.background.outline.color = src.background.outline.color,
        "background" => {
            style.background.background_color = src.background.background_color;
            style.background.background_color_current_color = src.background.background_color_current_color;
            style.background.background_image_present = src.background.background_image_present;
        }
        "background-color" => {
            style.background.background_color = src.background.background_color;
            style.background.background_color_current_color = src.background.background_color_current_color;
        }
        "background-image" => {
            style.background.background_image_present = src.background.background_image_present;
        }
        "vertical-align" => style.box_model.vertical_align = src.box_model.vertical_align,
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
        "grid-template-areas" => style.layout.grid_template_areas = src.layout.grid_template_areas.clone(),
        "grid-template" => {
            style.layout.grid_template_rows = src.layout.grid_template_rows.clone();
            style.layout.grid_template_columns = src.layout.grid_template_columns.clone();
            style.layout.grid_template_row_names = src.layout.grid_template_row_names.clone();
            style.layout.grid_template_column_names = src.layout.grid_template_column_names.clone();
            style.layout.grid_template_areas = src.layout.grid_template_areas.clone();
        }
        "grid-auto-rows" => style.layout.grid_auto_rows = src.layout.grid_auto_rows.clone(),
        "grid-auto-columns" => style.layout.grid_auto_columns = src.layout.grid_auto_columns.clone(),
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
            style.layout.grid_auto_rows = src.layout.grid_auto_rows.clone();
            style.layout.grid_auto_columns = src.layout.grid_auto_columns.clone();
            style.layout.grid_auto_flow = src.layout.grid_auto_flow;
        }
        "float" => style.box_model.float = src.box_model.float,
        "clear" => style.box_model.clear = src.box_model.clear,
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
        "padding-inline-start" => copy_padding_side(style, src, inline_sides(style.text.direction).0),
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
        "border-start-start-radius" => copy_corner_radius(style, src, if style.text.direction == TextDirection::Ltr { 0 } else { 1 }),
        "border-start-end-radius" => copy_corner_radius(style, src, if style.text.direction == TextDirection::Ltr { 1 } else { 0 }),
        "border-end-start-radius" => copy_corner_radius(style, src, if style.text.direction == TextDirection::Ltr { 3 } else { 2 }),
        "border-end-end-radius" => copy_corner_radius(style, src, if style.text.direction == TextDirection::Ltr { 2 } else { 3 }),
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
        | "border-inline-end-color" => apply_logical_border_keyword(style, name, src, keyword != "inherit"),
        _ => return false,
    }
    true
}

fn apply_all_from_basis(style: &mut WorkingStyle, basis: &WorkingStyle, phase: CascadePhase) {
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

fn apply_border_keyword(style: &mut WorkingStyle, name: &str, src: &ParentStyle, preserve_current_color_dependency: bool) {
    let side_shorthand = matches!(name, "border" | "border-top" | "border-right" | "border-bottom" | "border-left");
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
            let value = [src.border.border_top_width, src.border.border_right_width, src.border.border_bottom_width, src.border.border_left_width][side];
            *[&mut style.border.border_top_width, &mut style.border.border_right_width, &mut style.border.border_bottom_width, &mut style.border.border_left_width][side] = value;
        }
        if styles {
            let value = [src.border.border_top_style, src.border.border_right_style, src.border.border_bottom_style, src.border.border_left_style][side];
            *[&mut style.border.border_top_style, &mut style.border.border_right_style, &mut style.border.border_bottom_style, &mut style.border.border_left_style][side] = value;
        }
        if colors {
            let value = [src.border.border_top_color, src.border.border_right_color, src.border.border_bottom_color, src.border.border_left_color][side];
            set_border_color(style, side, value);
            let inherited_dependency = if preserve_current_color_dependency { src.border.current_color_sides & (1 << side) } else { 0 };
            style.border.current_color_sides = (style.border.current_color_sides & !(1 << side)) | inherited_dependency;
        }
    }
}

fn apply_logical_border_keyword(style: &mut WorkingStyle, name: &str, src: &ParentStyle, preserve_current_color_dependency: bool) {
    let side_shorthand = matches!(name, "border-block" | "border-block-start" | "border-block-end" | "border-inline" | "border-inline-start" | "border-inline-end");
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
            let value = [src.border.border_top_width, src.border.border_right_width, src.border.border_bottom_width, src.border.border_left_width][side];
            set_border_width(style, side, value);
        }
        if styles {
            let value = [src.border.border_top_style, src.border.border_right_style, src.border.border_bottom_style, src.border.border_left_style][side];
            set_border_style(style, side, value);
        }
        if colors {
            let value = [src.border.border_top_color, src.border.border_right_color, src.border.border_bottom_color, src.border.border_left_color][side];
            set_border_color(style, side, value);
            let inherited_dependency = if preserve_current_color_dependency { src.border.current_color_sides & (1 << side) } else { 0 };
            style.border.current_color_sides = (style.border.current_color_sides & !(1 << side)) | inherited_dependency;
        }
    }
}

fn parse_line_height(lh: &lightningcss::properties::font::LineHeight, font_size: f32, root_font_size: f32) -> Option<f32> {
    use lightningcss::properties::font::LineHeight;
    match lh {
        LineHeight::Normal => Some(0.0),
        LineHeight::Number(n) => Some(font_size * n),
        LineHeight::Length(lp) => length_percentage_to_px(lp, font_size, root_font_size),
    }
}

fn checked_line_height(lh: &lightningcss::properties::font::LineHeight, font_size: f32, root_font_size: f32) -> Option<f32> {
    checked_line_height_components(lh, font_size, root_font_size).map(|components| components.0)
}

fn checked_line_height_components(lh: &lightningcss::properties::font::LineHeight, font_size: f32, root_font_size: f32) -> Option<(f32, f32)> {
    use lightningcss::properties::font::LineHeight;
    let components = match lh {
        LineHeight::Length(LengthPercentage::Dimension(LengthValue::Ex(value))) => (0.0, value * font_size),
        LineHeight::Length(LengthPercentage::Calc(calc)) => {
            let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) = calc_box_length_percentage_components(calc, font_size, root_font_size)?;
            if ch_advance_px != 0.0 || cap_height_px != 0.0 {
                return None;
            }
            (absolute_px + percentage * font_size, x_height_px)
        }
        _ => (parse_line_height(lh, font_size, root_font_size)?, 0.0),
    };
    (components.0.is_finite() && components.0 >= 0.0 && components.1.is_finite() && components.1 >= 0.0).then_some(components)
}

fn overflow_mode(value: OverflowKeyword) -> OverflowMode {
    match value {
        OverflowKeyword::Visible => OverflowMode::Visible,
        OverflowKeyword::Hidden => OverflowMode::Hidden,
        OverflowKeyword::Clip => OverflowMode::Clip,
        OverflowKeyword::Scroll => OverflowMode::Scroll,
        OverflowKeyword::Auto => OverflowMode::Auto,
    }
}

fn flex_direction(value: &lightningcss::properties::flex::FlexDirection) -> FlexDirection {
    use lightningcss::properties::flex::FlexDirection as Lc;
    match value {
        Lc::Row => FlexDirection::Row,
        Lc::RowReverse => FlexDirection::RowReverse,
        Lc::Column => FlexDirection::Column,
        Lc::ColumnReverse => FlexDirection::ColumnReverse,
    }
}

fn flex_wrap(value: &lightningcss::properties::flex::FlexWrap) -> FlexWrap {
    use lightningcss::properties::flex::FlexWrap as Lc;
    match value {
        Lc::NoWrap => FlexWrap::NoWrap,
        Lc::Wrap => FlexWrap::Wrap,
        Lc::WrapReverse => FlexWrap::WrapReverse,
    }
}

fn flex_basis(value: &LengthPercentageOrAuto, font: &Font, root_font_size: f32, styles: &mut ComputedStylesBuilder) -> Option<PreferredSize> {
    let value = match value {
        LengthPercentageOrAuto::Auto => Some(PreferredSize::Auto),
        LengthPercentageOrAuto::LengthPercentage(value) => length_percentage_to_preferred(value, font, root_font_size, styles),
    }?;
    non_negative_preferred_size(value)
}

fn non_negative_preferred_size(value: PreferredSize) -> Option<PreferredSize> {
    match value {
        PreferredSize::Auto | PreferredSize::MinContent | PreferredSize::MaxContent | PreferredSize::FitContent | PreferredSize::Stretch => Some(value),
        PreferredSize::Px(value) if value >= 0.0 => Some(PreferredSize::Px(value)),
        PreferredSize::Percent(value) if value >= 0.0 => Some(PreferredSize::Percent(value)),
        PreferredSize::Ex(value) if value >= 0.0 => Some(PreferredSize::Ex(value)),
        PreferredSize::Ch(value) if value >= 0.0 => Some(PreferredSize::Ch(value)),
        PreferredSize::Cap(value) if value >= 0.0 => Some(PreferredSize::Cap(value)),
        PreferredSize::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent }
            if absolute_px.is_finite() && percentage.is_finite() && x_height_px.is_finite() && ch_advance_px.is_finite() && cap_height_px.is_finite() =>
        {
            Some(PreferredSize::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent })
        }
        PreferredSize::Comparison(_) => Some(value),
        PreferredSize::Px(_) | PreferredSize::Percent(_) | PreferredSize::Ex(_) | PreferredSize::Ch(_) | PreferredSize::Cap(_) | PreferredSize::Calc { .. } => None,
    }
}

fn length_percentage_to_preferred(value: &LengthPercentage, font: &Font, root_font_size: f32, styles: &mut ComputedStylesBuilder) -> Option<PreferredSize> {
    match value {
        LengthPercentage::Dimension(value) => preferred_from_length(value, font, root_font_size),
        LengthPercentage::Percentage(value) => Some(PreferredSize::Percent(value.0)),
        LengthPercentage::Calc(value) => comparison_preferred_size(value, font.font_size, root_font_size, styles).or_else(|| {
            let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) = calc_box_length_percentage_components(value, font.font_size, root_font_size)?;
            Some(PreferredSize::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent: calc_depends_on_percentage(value) })
        }),
    }
}

fn comparison_preferred_size(calc: &Calc<LengthPercentage>, font_size: f32, root_font_size: f32, styles: &mut ComputedStylesBuilder) -> Option<PreferredSize> {
    let Calc::Function(function) = calc else { return None };
    match function.as_ref() {
        MathFunction::Min(values) => comparison_from_operands(SizeComparison::Min, values.iter(), values.len(), font_size, root_font_size, styles),
        MathFunction::Max(values) => comparison_from_operands(SizeComparison::Max, values.iter(), values.len(), font_size, root_font_size, styles),
        MathFunction::Clamp(min, value, max) => comparison_from_operands(SizeComparison::Clamp, [min, value, max], 3, font_size, root_font_size, styles),
        MathFunction::Calc(value) => return comparison_preferred_size(value, font_size, root_font_size, styles),
        _ => None,
    }
}

fn comparison_from_operands<'a>(kind: SizeComparison, operands: impl IntoIterator<Item = &'a Calc<LengthPercentage>>, count: usize, font_size: f32, root_font_size: f32, styles: &mut ComputedStylesBuilder) -> Option<PreferredSize> {
    if count == 0 || count > 3 {
        return None;
    }
    let mut values = [ComputedSizeComponent::default(); 3];
    for (slot, operand) in values.iter_mut().zip(operands) {
        let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) = calc_box_length_percentage_components(operand, font_size, root_font_size)?;
        *slot = ComputedSizeComponent { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent: calc_depends_on_percentage(operand) };
    }
    styles.intern_size_comparison(kind, values, u8::try_from(count).ok()?)
}

fn computed_length_pct(value: &LengthPercentage, font_size: f32, root_font_size: f32) -> Option<LengthPct> {
    match value {
        LengthPercentage::Dimension(value) => length_pct_from_length(value, font_size, root_font_size),
        LengthPercentage::Percentage(value) => Some(LengthPct::Pct(value.0)),
        LengthPercentage::Calc(value) => {
            let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) = calc_box_length_percentage_components(value, font_size, root_font_size)?;
            Some(LengthPct::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent: calc_depends_on_percentage(value) })
        }
    }
}

fn non_negative_length_pct(value: LengthPct) -> Option<LengthPct> {
    match value {
        LengthPct::Px(value) if value >= 0.0 => Some(LengthPct::Px(value)),
        LengthPct::Pct(value) if value >= 0.0 => Some(LengthPct::Pct(value)),
        LengthPct::Ex(value) if value >= 0.0 => Some(LengthPct::Ex(value)),
        LengthPct::Ch(value) if value >= 0.0 => Some(LengthPct::Ch(value)),
        LengthPct::Cap(value) if value >= 0.0 => Some(LengthPct::Cap(value)),
        LengthPct::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent }
            if absolute_px.is_finite() && percentage.is_finite() && x_height_px.is_finite() && ch_advance_px.is_finite() && cap_height_px.is_finite() =>
        {
            Some(LengthPct::Calc { absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px, percentage_dependent })
        }
        LengthPct::Px(_) | LengthPct::Pct(_) | LengthPct::Ex(_) | LengthPct::Ch(_) | LengthPct::Cap(_) | LengthPct::Calc { .. } => None,
    }
}

fn corner_radius(value: &Size2D<LengthPercentage>, font_size: f32, root_font_size: f32) -> Option<CornerRadius> {
    Some(CornerRadius { x: length_percentage_to_lengthpct(&value.0, font_size, root_font_size).and_then(non_negative_length_pct)?, y: length_percentage_to_lengthpct(&value.1, font_size, root_font_size).and_then(non_negative_length_pct)? })
}

fn set_corner_radius(style: &mut WorkingStyle, corner: usize, value: CornerRadius) {
    match corner {
        0 => style.radii.top_left = value,
        1 => style.radii.top_right = value,
        2 => style.radii.bottom_right = value,
        3 => style.radii.bottom_left = value,
        _ => unreachable!("physical corner index"),
    }
}

fn copy_corner_radius(style: &mut WorkingStyle, source: &ParentStyle, corner: usize) {
    let value = match corner {
        0 => source.radii.top_left,
        1 => source.radii.top_right,
        2 => source.radii.bottom_right,
        3 => source.radii.bottom_left,
        _ => unreachable!("physical corner index"),
    };
    set_corner_radius(style, corner, value);
}

/// Physical side indices use CSS clockwise order: top, right, bottom, left.
fn inline_sides(direction: TextDirection) -> (usize, usize) {
    match direction {
        TextDirection::Ltr => (3, 1),
        TextDirection::Rtl => (1, 3),
    }
}

fn text_start_alignment(direction: TextDirection) -> TextAlign {
    match direction {
        TextDirection::Ltr => TextAlign::Left,
        TextDirection::Rtl => TextAlign::Right,
    }
}

fn text_end_alignment(direction: TextDirection) -> TextAlign {
    match direction {
        TextDirection::Ltr => TextAlign::Right,
        TextDirection::Rtl => TextAlign::Left,
    }
}

fn resolve_logical_text_alignments(text: &mut InheritedText) {
    text.text_align = match text.text_align_logical {
        LogicalTextAlign::Physical => text.text_align,
        LogicalTextAlign::Start => text_start_alignment(text.direction),
        LogicalTextAlign::End => text_end_alignment(text.direction),
    };
    text.text_align_last = match text.text_align_last_logical {
        LogicalTextAlign::Physical => text.text_align_last,
        LogicalTextAlign::Start => text_start_alignment(text.direction),
        LogicalTextAlign::End => text_end_alignment(text.direction),
    };
}

fn logical_float(value: &str, direction: TextDirection, fallback: Float) -> Float {
    match value {
        "left" => Float::Left,
        "right" => Float::Right,
        "inline-start" => match direction {
            TextDirection::Ltr => Float::Left,
            TextDirection::Rtl => Float::Right,
        },
        "inline-end" => match direction {
            TextDirection::Ltr => Float::Right,
            TextDirection::Rtl => Float::Left,
        },
        "none" => Float::None,
        _ => fallback,
    }
}

fn logical_clear(value: &str, direction: TextDirection, fallback: Clear) -> Clear {
    match value {
        "left" => Clear::Left,
        "right" => Clear::Right,
        "inline-start" => match direction {
            TextDirection::Ltr => Clear::Left,
            TextDirection::Rtl => Clear::Right,
        },
        "inline-end" => match direction {
            TextDirection::Ltr => Clear::Right,
            TextDirection::Rtl => Clear::Left,
        },
        "both" => Clear::Both,
        "none" => Clear::None,
        _ => fallback,
    }
}

fn set_margin_side(style: &mut WorkingStyle, side: usize, value: (LengthPct, bool)) {
    match side {
        0 => (style.box_model.margin_top, style.layout.margin_top_auto) = value,
        1 => (style.box_model.margin_right, style.layout.margin_right_auto) = value,
        2 => (style.box_model.margin_bottom, style.layout.margin_bottom_auto) = value,
        3 => (style.box_model.margin_left, style.layout.margin_left_auto) = value,
        _ => unreachable!("physical side index"),
    }
}

fn copy_margin_side(style: &mut WorkingStyle, source: &ParentStyle, side: usize) {
    let value = match side {
        0 => (source.box_model.margin_top, source.layout.margin_top_auto),
        1 => (source.box_model.margin_right, source.layout.margin_right_auto),
        2 => (source.box_model.margin_bottom, source.layout.margin_bottom_auto),
        3 => (source.box_model.margin_left, source.layout.margin_left_auto),
        _ => unreachable!("physical side index"),
    };
    set_margin_side(style, side, value);
}

fn set_padding_side(style: &mut WorkingStyle, side: usize, value: LengthPct) {
    match side {
        0 => style.box_model.padding_top = value,
        1 => style.box_model.padding_right = value,
        2 => style.box_model.padding_bottom = value,
        3 => style.box_model.padding_left = value,
        _ => unreachable!("physical side index"),
    }
}

fn copy_padding_side(style: &mut WorkingStyle, source: &ParentStyle, side: usize) {
    let value = match side {
        0 => source.box_model.padding_top,
        1 => source.box_model.padding_right,
        2 => source.box_model.padding_bottom,
        3 => source.box_model.padding_left,
        _ => unreachable!("physical side index"),
    };
    set_padding_side(style, side, value);
}

fn set_border_width(style: &mut WorkingStyle, side: usize, value: FontRelativeLength) {
    match side {
        0 => style.border.border_top_width = value,
        1 => style.border.border_right_width = value,
        2 => style.border.border_bottom_width = value,
        3 => style.border.border_left_width = value,
        _ => unreachable!("physical side index"),
    }
}

fn set_border_style(style: &mut WorkingStyle, side: usize, value: BorderStyle) {
    match side {
        0 => style.border.border_top_style = value,
        1 => style.border.border_right_style = value,
        2 => style.border.border_bottom_style = value,
        3 => style.border.border_left_style = value,
        _ => unreachable!("physical side index"),
    }
}

fn set_border_color(style: &mut WorkingStyle, side: usize, value: u32) {
    match side {
        0 => style.border.border_top_color = value,
        1 => style.border.border_right_color = value,
        2 => style.border.border_bottom_color = value,
        3 => style.border.border_left_color = value,
        _ => unreachable!("physical side index"),
    }
}

fn set_border_css_color(style: &mut WorkingStyle, side: usize, value: &CssColor) {
    set_border_color(style, side, css_color_to_u32(value, style.text.color));
    let bit = 1 << side;
    if matches!(value, CssColor::CurrentColor) {
        style.border.current_color_sides |= bit;
    } else {
        style.border.current_color_sides &= !bit;
    }
}

fn checked_border_width(value: &BorderSideWidth, font_size: f32, root_font_size: f32) -> Option<FontRelativeLength> {
    border_width(value, font_size, root_font_size)
}

fn apply_logical_border<const P: u8>(style: &mut WorkingStyle, side: usize, value: &lightningcss::properties::border::GenericBorder<LineStyle, P>, root_font_size: f32) {
    let Some(width) = checked_border_width(&value.width, style.font.font_size, root_font_size) else { return };
    set_border_width(style, side, width);
    set_border_style(style, side, line_style_to_border_style(&value.style));
    set_border_css_color(style, side, &value.color);
}

fn gap_value(value: &lightningcss::properties::align::GapValue, font_size: f32, root_font_size: f32) -> Option<LengthPct> {
    use lightningcss::properties::align::GapValue;
    let value = match value {
        GapValue::Normal => Some(LengthPct::Px(0.0)),
        GapValue::LengthPercentage(value) => computed_length_pct(value, font_size, root_font_size),
    }?;

    // Lightning CSS currently represents negative gaps as typed values even
    // though CSS Box Alignment makes them invalid. Do not duplicate its CSS
    // parser here, but preserve the last valid cascaded value rather than
    // allowing an invalid typed value into the computed-style store.
    non_negative_length_pct(value)
}

fn content_alignment(value: &lightningcss::properties::align::AlignContent) -> Option<ContentAlignment> {
    use lightningcss::properties::align::{AlignContent, BaselinePosition, ContentDistribution, ContentPosition};
    Some(match value {
        AlignContent::Normal => ContentAlignment::Normal,
        AlignContent::BaselinePosition(BaselinePosition::First | BaselinePosition::Last) => return None,
        AlignContent::ContentDistribution(value) => match value {
            ContentDistribution::SpaceBetween => ContentAlignment::SpaceBetween,
            ContentDistribution::SpaceAround => ContentAlignment::SpaceAround,
            ContentDistribution::SpaceEvenly => ContentAlignment::SpaceEvenly,
            ContentDistribution::Stretch => ContentAlignment::Stretch,
        },
        AlignContent::ContentPosition { value, .. } => match value {
            ContentPosition::Center => ContentAlignment::Center,
            ContentPosition::Start => ContentAlignment::Start,
            ContentPosition::End => ContentAlignment::End,
            ContentPosition::FlexStart => ContentAlignment::FlexStart,
            ContentPosition::FlexEnd => ContentAlignment::FlexEnd,
        },
    })
}

fn justify_content(value: &lightningcss::properties::align::JustifyContent) -> ContentAlignment {
    use lightningcss::properties::align::{ContentDistribution, ContentPosition, JustifyContent};
    match value {
        JustifyContent::Normal => ContentAlignment::Normal,
        JustifyContent::ContentDistribution(value) => match value {
            ContentDistribution::SpaceBetween => ContentAlignment::SpaceBetween,
            ContentDistribution::SpaceAround => ContentAlignment::SpaceAround,
            ContentDistribution::SpaceEvenly => ContentAlignment::SpaceEvenly,
            ContentDistribution::Stretch => ContentAlignment::Stretch,
        },
        JustifyContent::ContentPosition { value, .. } => match value {
            ContentPosition::Center => ContentAlignment::Center,
            ContentPosition::Start => ContentAlignment::Start,
            ContentPosition::End => ContentAlignment::End,
            ContentPosition::FlexStart => ContentAlignment::FlexStart,
            ContentPosition::FlexEnd => ContentAlignment::FlexEnd,
        },
        JustifyContent::Left { .. } => ContentAlignment::Start,
        JustifyContent::Right { .. } => ContentAlignment::End,
    }
}

fn self_position(value: &lightningcss::properties::align::SelfPosition) -> ItemAlignment {
    use lightningcss::properties::align::SelfPosition;
    match value {
        SelfPosition::Center => ItemAlignment::Center,
        SelfPosition::Start => ItemAlignment::Start,
        SelfPosition::End => ItemAlignment::End,
        SelfPosition::SelfStart => ItemAlignment::SelfStart,
        SelfPosition::SelfEnd => ItemAlignment::SelfEnd,
        SelfPosition::FlexStart => ItemAlignment::FlexStart,
        SelfPosition::FlexEnd => ItemAlignment::FlexEnd,
    }
}

fn align_items(value: &lightningcss::properties::align::AlignItems) -> Option<ItemAlignment> {
    use lightningcss::properties::align::{AlignItems, BaselinePosition};
    Some(match value {
        AlignItems::Normal => ItemAlignment::Normal,
        AlignItems::Stretch => ItemAlignment::Stretch,
        AlignItems::BaselinePosition(BaselinePosition::First) => ItemAlignment::Baseline,
        AlignItems::BaselinePosition(BaselinePosition::Last) => return None,
        AlignItems::SelfPosition { value, .. } => self_position(value),
    })
}

fn align_self(value: &lightningcss::properties::align::AlignSelf) -> Option<ItemAlignment> {
    use lightningcss::properties::align::{AlignSelf, BaselinePosition};
    Some(match value {
        AlignSelf::Auto => ItemAlignment::Auto,
        AlignSelf::Normal => ItemAlignment::Normal,
        AlignSelf::Stretch => ItemAlignment::Stretch,
        AlignSelf::BaselinePosition(BaselinePosition::First) => ItemAlignment::Baseline,
        AlignSelf::BaselinePosition(BaselinePosition::Last) => return None,
        AlignSelf::SelfPosition { value, .. } => self_position(value),
    })
}

fn justify_items(value: &lightningcss::properties::align::JustifyItems) -> Option<ItemAlignment> {
    use lightningcss::properties::align::{BaselinePosition, JustifyItems, LegacyJustify};
    Some(match value {
        JustifyItems::Normal => ItemAlignment::Normal,
        JustifyItems::Stretch => ItemAlignment::Stretch,
        JustifyItems::BaselinePosition(BaselinePosition::First) => ItemAlignment::Baseline,
        JustifyItems::BaselinePosition(BaselinePosition::Last) => return None,
        JustifyItems::SelfPosition { value, .. } => self_position(value),
        JustifyItems::Left { .. } => ItemAlignment::Start,
        JustifyItems::Right { .. } => ItemAlignment::End,
        JustifyItems::Legacy(LegacyJustify::Left) => ItemAlignment::Start,
        JustifyItems::Legacy(LegacyJustify::Right) => ItemAlignment::End,
        JustifyItems::Legacy(LegacyJustify::Center) => ItemAlignment::Center,
    })
}

fn justify_self(value: &lightningcss::properties::align::JustifySelf) -> Option<ItemAlignment> {
    use lightningcss::properties::align::{BaselinePosition, JustifySelf};
    Some(match value {
        JustifySelf::Auto => ItemAlignment::Auto,
        JustifySelf::Normal => ItemAlignment::Normal,
        JustifySelf::Stretch => ItemAlignment::Stretch,
        JustifySelf::BaselinePosition(BaselinePosition::First) => ItemAlignment::Baseline,
        JustifySelf::BaselinePosition(BaselinePosition::Last) => return None,
        JustifySelf::SelfPosition { value, .. } => self_position(value),
        JustifySelf::Left { .. } => ItemAlignment::Start,
        JustifySelf::Right { .. } => ItemAlignment::End,
    })
}

fn grid_auto_flow(value: lightningcss::properties::grid::GridAutoFlow) -> GridAutoFlow {
    use lightningcss::properties::grid::GridAutoFlow as Lc;
    match (value.contains(Lc::Column), value.contains(Lc::Dense)) {
        (false, false) => GridAutoFlow::Row,
        (true, false) => GridAutoFlow::Column,
        (false, true) => GridAutoFlow::RowDense,
        (true, true) => GridAutoFlow::ColumnDense,
    }
}

fn grid_template_tracks(styles: &mut ComputedStylesBuilder, value: &lightningcss::properties::grid::TrackSizing<'_>, font_size: f32, root_font_size: f32) -> Option<(Vec<GridTemplateTrack>, Vec<Vec<StyleStringId>>)> {
    use lightningcss::properties::grid::{RepeatCount, TrackListItem, TrackSizing};
    let TrackSizing::TrackList(list) = value else {
        return Some((Vec::new(), Vec::new()));
    };
    if list.items.is_empty() {
        return None;
    }
    let names = list.line_names.iter().map(|set| set.iter().map(|name| styles.intern_string(name.0.as_ref())).collect()).collect();
    let mut tracks = Vec::with_capacity(list.items.len());
    for item in &list.items {
        tracks.push(match item {
            TrackListItem::TrackSize(size) => GridTemplateTrack::Single(grid_track_size(size, font_size, root_font_size)?),
            TrackListItem::TrackRepeat(repeat) => {
                let count = match repeat.count {
                    RepeatCount::Number(value) => GridRepeatCount::Count(std::num::NonZeroU16::new(u16::try_from(value).ok()?)?),
                    RepeatCount::AutoFill => GridRepeatCount::AutoFill,
                    RepeatCount::AutoFit => GridRepeatCount::AutoFit,
                };
                let tracks = repeat.track_sizes.iter().map(|size| grid_track_size(size, font_size, root_font_size)).collect::<Option<Vec<_>>>()?;
                if tracks.is_empty() {
                    return None;
                }
                let line_names = repeat.line_names.iter().map(|set| set.iter().map(|name| styles.intern_string(name.0.as_ref())).collect()).collect();
                GridTemplateTrack::Repeat { count, tracks, line_names }
            }
        });
    }
    let auto_repeat_count = tracks.iter().filter(|track| matches!(track, GridTemplateTrack::Repeat { count: GridRepeatCount::AutoFill | GridRepeatCount::AutoFit, .. })).count();
    if auto_repeat_count > 1 || (auto_repeat_count == 1 && !tracks.iter().all(grid_auto_repeat_compatible)) {
        return None;
    }
    Some((tracks, names))
}

fn grid_auto_repeat_compatible(track: &GridTemplateTrack) -> bool {
    match track {
        GridTemplateTrack::Single(size) => grid_fixed_track_size(size),
        GridTemplateTrack::Repeat { tracks, .. } => tracks.iter().all(grid_fixed_track_size),
    }
}

fn grid_fixed_track_size(size: &GridTrackSize) -> bool {
    match size {
        GridTrackSize::Breadth(value) => grid_fixed_breadth(value),
        GridTrackSize::MinMax { min, max } => grid_fixed_breadth(min) || (grid_inflexible_breadth(min) && grid_fixed_breadth(max)),
        GridTrackSize::Auto | GridTrackSize::FitContent(_) => false,
    }
}

fn grid_fixed_breadth(value: &GridTrackBreadth) -> bool {
    matches!(value, GridTrackBreadth::Length(_))
}

fn grid_inflexible_breadth(value: &GridTrackBreadth) -> bool {
    !matches!(value, GridTrackBreadth::Flex(_))
}

fn grid_auto_tracks(value: &lightningcss::properties::grid::TrackSizeList, font_size: f32, root_font_size: f32) -> Option<Vec<GridTrackSize>> {
    value.0.iter().map(|size| grid_track_size(size, font_size, root_font_size)).collect()
}

fn grid_track_size(value: &lightningcss::properties::grid::TrackSize, font_size: f32, root_font_size: f32) -> Option<GridTrackSize> {
    use lightningcss::properties::grid::TrackSize;
    Some(match value {
        TrackSize::TrackBreadth(value) => GridTrackSize::Breadth(grid_track_breadth(value, font_size, root_font_size)?),
        TrackSize::MinMax { min, max } => GridTrackSize::MinMax { min: grid_track_breadth(min, font_size, root_font_size)?, max: grid_track_breadth(max, font_size, root_font_size)? },
        TrackSize::FitContent(value) => GridTrackSize::FitContent(non_negative_length_pct(computed_length_pct(value, font_size, root_font_size)?)?),
    })
}

fn grid_track_breadth(value: &lightningcss::properties::grid::TrackBreadth, font_size: f32, root_font_size: f32) -> Option<GridTrackBreadth> {
    use lightningcss::properties::grid::TrackBreadth;
    Some(match value {
        TrackBreadth::Length(value) => GridTrackBreadth::Length(non_negative_length_pct(computed_length_pct(value, font_size, root_font_size)?)?),
        TrackBreadth::Flex(value) if value.is_finite() && *value >= 0.0 => GridTrackBreadth::Flex(*value),
        TrackBreadth::Flex(_) => return None,
        TrackBreadth::MinContent => GridTrackBreadth::MinContent,
        TrackBreadth::MaxContent => GridTrackBreadth::MaxContent,
        TrackBreadth::Auto => GridTrackBreadth::Auto,
    })
}

fn grid_placement(styles: &mut ComputedStylesBuilder, value: &lightningcss::properties::grid::GridLine<'_>) -> Option<GridPlacement> {
    use lightningcss::properties::grid::GridLine;
    Some(match value {
        GridLine::Auto => GridPlacement::Auto,
        GridLine::Area { name } => GridPlacement::NamedLine { name: styles.intern_string(name.0.as_ref()), index: 0 },
        GridLine::Line { index, name: None } => GridPlacement::Line(std::num::NonZeroI16::new(i16::try_from(*index).ok()?)?),
        GridLine::Line { index, name: Some(name) } => GridPlacement::NamedLine { name: styles.intern_string(name.0.as_ref()), index: i16::try_from(*index).ok()? },
        GridLine::Span { index, name: None } => GridPlacement::Span(std::num::NonZeroU16::new(u16::try_from(*index).ok()?)?),
        GridLine::Span { index, name: Some(name) } => GridPlacement::NamedSpan { name: styles.intern_string(name.0.as_ref()), count: std::num::NonZeroU16::new(u16::try_from(*index).ok()?)? },
    })
}

fn grid_template_areas(styles: &mut ComputedStylesBuilder, value: &lightningcss::properties::grid::GridTemplateAreas) -> Option<Vec<GridTemplateArea>> {
    use lightningcss::properties::grid::GridTemplateAreas;
    let GridTemplateAreas::Areas { columns, areas } = value else {
        return Some(Vec::new());
    };
    let columns = usize::try_from(*columns).ok()?;
    if columns == 0 || areas.len() % columns != 0 {
        return None;
    }
    let rows = areas.len() / columns;
    let mut found: Vec<(String, usize, usize, usize, usize)> = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let Some(name) = areas[row * columns + column].as_ref() else { continue };
            if let Some((_, row_start, row_end, column_start, column_end)) = found.iter_mut().find(|entry| entry.0 == *name) {
                *row_start = (*row_start).min(row);
                *row_end = (*row_end).max(row + 1);
                *column_start = (*column_start).min(column);
                *column_end = (*column_end).max(column + 1);
            } else {
                found.push((name.clone(), row, row + 1, column, column + 1));
            }
        }
    }
    let mut output = Vec::with_capacity(found.len());
    for (name, row_start, row_end, column_start, column_end) in found {
        for row in row_start..row_end {
            for column in column_start..column_end {
                if areas[row * columns + column].as_deref() != Some(name.as_str()) {
                    return None;
                }
            }
        }
        output.push(GridTemplateArea {
            name: styles.intern_string(&name),
            // Renderer grid coordinates are CSS grid-line numbers (one based),
            // matching Taffy's named-area resolver rather than the zero-based
            // row/column offsets used while validating the source matrix.
            row_start: u16::try_from(row_start + 1).ok()?,
            row_end: u16::try_from(row_end + 1).ok()?,
            column_start: u16::try_from(column_start + 1).ok()?,
            column_end: u16::try_from(column_end + 1).ok()?,
        });
    }
    Some(output)
}

/// The unitless multiplier of a `line-height` value (0.0 for lengths,
/// percentages, and `normal`, which inherit as resolved px / the sentinel).
fn line_height_number(lh: &lightningcss::properties::font::LineHeight) -> f32 {
    use lightningcss::properties::font::LineHeight;
    match lh {
        LineHeight::Number(n) => *n,
        _ => 0.0,
    }
}

fn line_height_is_normal(lh: &lightningcss::properties::font::LineHeight) -> bool {
    matches!(lh, lightningcss::properties::font::LineHeight::Normal)
}

fn text_decoration_lines(line: &lightningcss::properties::text::TextDecorationLine) -> TextDecorationLines {
    use lightningcss::properties::text::TextDecorationLine as CssLine;
    TextDecorationLines::new(line.contains(CssLine::Underline), line.contains(CssLine::Overline), line.contains(CssLine::LineThrough))
}

fn text_decoration_style(style: &lightningcss::properties::text::TextDecorationStyle) -> Option<TextDecorationStyle> {
    use lightningcss::properties::text::TextDecorationStyle as CssStyle;
    match style {
        CssStyle::Solid => Some(TextDecorationStyle::Solid),
        CssStyle::Double => Some(TextDecorationStyle::Double),
        CssStyle::Dotted => Some(TextDecorationStyle::Dotted),
        CssStyle::Dashed => Some(TextDecorationStyle::Dashed),
        CssStyle::Wavy => None,
    }
}

fn text_decoration_thickness(value: &lightningcss::properties::text::TextDecorationThickness, font_size: f32, root_font_size: f32) -> Option<TextDecorationThickness> {
    use lightningcss::properties::text::TextDecorationThickness as CssThickness;
    match value {
        CssThickness::Auto => Some(TextDecorationThickness::Auto),
        CssThickness::FromFont => Some(TextDecorationThickness::FromFont),
        CssThickness::LengthPercentage(value) => TextDecorationThickness::length(computed_length_pct(value, font_size, root_font_size)?),
    }
}

fn decoration_color(color: &CssColor, current_color: u32) -> DecorationColor {
    if matches!(color, CssColor::CurrentColor) { DecorationColor::CurrentColor } else { DecorationColor::Rgba(css_color_to_u32(color, current_color)) }
}

fn outline_style(style: &lightningcss::properties::outline::OutlineStyle) -> Option<BorderStyle> {
    use lightningcss::properties::outline::OutlineStyle;
    match style {
        OutlineStyle::Auto => Some(BorderStyle::Solid),
        OutlineStyle::LineStyle(LineStyle::None | LineStyle::Hidden) => Some(BorderStyle::None),
        OutlineStyle::LineStyle(LineStyle::Solid) => Some(BorderStyle::Solid),
        OutlineStyle::LineStyle(LineStyle::Dashed) => Some(BorderStyle::Dashed),
        OutlineStyle::LineStyle(LineStyle::Dotted) => Some(BorderStyle::Dotted),
        OutlineStyle::LineStyle(LineStyle::Double | LineStyle::Groove | LineStyle::Ridge | LineStyle::Inset | LineStyle::Outset) => None,
    }
}

// ============================================================================
// Conversion helpers
// ============================================================================
fn feature(tag: &[u8; 4], value: u32) -> OpenTypeFeature {
    OpenTypeFeature::new(*tag, value)
}

fn font_variant_caps_features(value: &lightningcss::properties::font::FontVariantCaps) -> Vec<OpenTypeFeature> {
    use lightningcss::properties::font::FontVariantCaps;
    match value {
        FontVariantCaps::Normal => Vec::new(),
        FontVariantCaps::SmallCaps => vec![feature(b"smcp", 1)],
        FontVariantCaps::AllSmallCaps => vec![feature(b"smcp", 1), feature(b"c2sc", 1)],
        FontVariantCaps::PetiteCaps => vec![feature(b"pcap", 1)],
        FontVariantCaps::AllPetiteCaps => vec![feature(b"pcap", 1), feature(b"c2pc", 1)],
        FontVariantCaps::Unicase => vec![feature(b"unic", 1)],
        FontVariantCaps::TitlingCaps => vec![feature(b"titl", 1)],
    }
}

pub(crate) fn parse_font_kerning(css: &str) -> Option<Vec<OpenTypeFeature>> {
    match css.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(Vec::new()),
        "normal" => Some(vec![feature(b"kern", 1)]),
        "none" => Some(vec![feature(b"kern", 0)]),
        _ => None,
    }
}

fn parse_font_variant_numeric(css: &str) -> Option<Vec<OpenTypeFeature>> {
    let css = css.trim().to_ascii_lowercase();
    if css == "normal" {
        return Some(Vec::new());
    }
    let mut features = Vec::new();
    let mut figure_style = false;
    let mut spacing_style = false;
    let mut fraction_style = false;
    for keyword in css.split_ascii_whitespace() {
        let (tag, category) = match keyword {
            "lining-nums" => (b"lnum", Some(&mut figure_style)),
            "oldstyle-nums" => (b"onum", Some(&mut figure_style)),
            "proportional-nums" => (b"pnum", Some(&mut spacing_style)),
            "tabular-nums" => (b"tnum", Some(&mut spacing_style)),
            "diagonal-fractions" => (b"frac", Some(&mut fraction_style)),
            "stacked-fractions" => (b"afrc", Some(&mut fraction_style)),
            "ordinal" => (b"ordn", None),
            "slashed-zero" => (b"zero", None),
            _ => return None,
        };
        if let Some(category) = category {
            if *category {
                return None;
            }
            *category = true;
        }
        if features.iter().any(|existing: &OpenTypeFeature| existing.tag() == *tag) {
            return None;
        }
        features.push(feature(tag, 1));
    }
    (!features.is_empty()).then_some(features)
}

fn parse_font_variant_ligatures(css: &str) -> Option<Vec<OpenTypeFeature>> {
    let css = css.trim().to_ascii_lowercase();
    if css == "normal" {
        return Some(Vec::new());
    }
    if css == "none" {
        return Some([b"liga", b"clig", b"dlig", b"hlig", b"calt"].into_iter().map(|tag| feature(tag, 0)).collect());
    }
    let mut features = Vec::new();
    for keyword in css.split_ascii_whitespace() {
        let (tags, value): (&[[u8; 4]], u32) = match keyword {
            "common-ligatures" => (&[*b"liga", *b"clig"], 1),
            "no-common-ligatures" => (&[*b"liga", *b"clig"], 0),
            "discretionary-ligatures" => (&[*b"dlig"], 1),
            "no-discretionary-ligatures" => (&[*b"dlig"], 0),
            "historical-ligatures" => (&[*b"hlig"], 1),
            "no-historical-ligatures" => (&[*b"hlig"], 0),
            "contextual" => (&[*b"calt"], 1),
            "no-contextual" => (&[*b"calt"], 0),
            _ => return None,
        };
        for tag in tags {
            if features.iter().any(|existing: &OpenTypeFeature| existing.tag() == *tag) {
                return None;
            }
            features.push(OpenTypeFeature::new(*tag, value));
        }
    }
    (!features.is_empty()).then_some(features)
}

fn parse_font_feature_settings(css: &str) -> Option<Vec<OpenTypeFeature>> {
    let css = css.trim();
    if css.eq_ignore_ascii_case("normal") {
        return Some(Vec::new());
    }
    let mut features = Vec::new();
    for item in css.split(',') {
        let item = item.trim();
        let quote = item.as_bytes().first().copied()?;
        if quote != b'\'' && quote != b'"' {
            return None;
        }
        let end = item.as_bytes()[1..].iter().position(|byte| *byte == quote)? + 1;
        let tag_text = &item[1..end];
        if tag_text.len() != 4 || !tag_text.is_ascii() {
            return None;
        }
        let mut tag = [0; 4];
        tag.copy_from_slice(tag_text.as_bytes());
        let remainder = item[end + 1..].trim();
        let value = if remainder.is_empty() || remainder.eq_ignore_ascii_case("on") {
            1
        } else if remainder.eq_ignore_ascii_case("off") {
            0
        } else {
            remainder.parse::<u32>().ok()?
        };
        let parsed = OpenTypeFeature::new(tag, value);
        if let Some(index) = features.iter().position(|existing: &OpenTypeFeature| existing.tag() == tag) {
            features[index] = parsed;
        } else {
            features.push(parsed);
        }
    }
    (!features.is_empty()).then_some(features)
}

fn collect_custom_properties<'a>(
    custom_properties: &mut FxHashMap<String, TokenList<'a>>, parent_custom: &FxHashMap<String, TokenList<'a>>, declarations: &[Property<'a>], revert_basis: &FxHashMap<String, TokenList<'a>>,
    revert_layer_basis: &FxHashMap<String, TokenList<'a>>,
) {
    for property in declarations {
        let (name, value) = match property {
            Property::Custom(custom) => {
                let raw_name = custom.name.as_ref();
                if !raw_name.starts_with("--") {
                    continue;
                }
                (raw_name.to_string(), &custom.value)
            }
            Property::Unparsed(unparsed) if unparsed.property_id.name().starts_with("--") => (unparsed.property_id.name().to_string(), &unparsed.value),
            _ => continue,
        };
        if let Some(keyword) = single_ident_keyword(value)
            && apply_custom_keyword(custom_properties, parent_custom, revert_basis, revert_layer_basis, &name, keyword)
        {
            continue;
        }
        custom_properties.insert(name, value.clone());
    }
}

fn single_ident_keyword<'a>(tokens: &'a TokenList<'a>) -> Option<&'a str> {
    let mut iter = tokens.0.iter().filter(|token| !is_ignorable_token(token));
    let first = iter.next()?;
    if iter.next().is_some() {
        return None;
    }
    match first {
        TokenOrValue::Token(Token::Ident(ident)) => Some(ident.as_ref()),
        _ => None,
    }
}

fn is_ignorable_token(token: &TokenOrValue) -> bool {
    matches!(token, TokenOrValue::Token(Token::WhiteSpace(_)) | TokenOrValue::Token(Token::Comment(_)))
}

fn apply_custom_keyword<'a>(
    custom_properties: &mut FxHashMap<String, TokenList<'a>>, parent_custom: &FxHashMap<String, TokenList<'a>>, revert_basis: &FxHashMap<String, TokenList<'a>>, revert_layer_basis: &FxHashMap<String, TokenList<'a>>, name: &str,
    keyword: &str,
) -> bool {
    let keyword = keyword.to_ascii_lowercase();
    match keyword.as_str() {
        "inherit" | "unset" => {
            if let Some(value) = parent_custom.get(name) {
                custom_properties.insert(name.to_string(), value.clone());
            } else {
                custom_properties.remove(name);
            }
        }
        "initial" => {
            custom_properties.remove(name);
        }
        "revert" => {
            if let Some(value) = revert_basis.get(name) {
                custom_properties.insert(name.to_string(), value.clone());
            } else {
                custom_properties.remove(name);
            }
        }
        "revert-layer" => {
            if let Some(value) = revert_layer_basis.get(name) {
                custom_properties.insert(name.to_string(), value.clone());
            } else {
                custom_properties.remove(name);
            }
        }
        _ => return false,
    }
    true
}

struct CustomPropertyDependencyCollector {
    names: Vec<String>,
}

impl<'i> Visitor<'i> for CustomPropertyDependencyCollector {
    type Error = Infallible;

    fn visit_types(&self) -> lightningcss::visitor::VisitTypes {
        lightningcss::visit_types!(VARIABLES)
    }

    fn visit_variable(&mut self, variable: &mut Variable<'i>) -> Result<(), Self::Error> {
        let name = variable.name.ident.as_ref().to_string();
        if !self.names.contains(&name) {
            self.names.push(name);
        }
        variable.visit_children(self)
    }
}

fn custom_property_dependencies(tokens: &TokenList<'_>) -> Vec<String> {
    let mut tokens = tokens.clone();
    let mut collector = CustomPropertyDependencyCollector { names: Vec::new() };
    let result = tokens.visit(&mut collector);
    match result {
        Ok(()) => collector.names,
        Err(error) => match error {},
    }
}

struct CustomPropertyCycleDetector<'a> {
    dependencies: &'a FxHashMap<String, Vec<String>>,
    next_index: usize,
    indices: FxHashMap<String, usize>,
    lowlinks: FxHashMap<String, usize>,
    stack: Vec<String>,
    on_stack: HashSet<String>,
    cyclic: HashSet<String>,
}

impl CustomPropertyCycleDetector<'_> {
    fn visit(&mut self, name: &str) {
        let index = self.next_index;
        self.next_index += 1;
        self.indices.insert(name.to_string(), index);
        self.lowlinks.insert(name.to_string(), index);
        self.stack.push(name.to_string());
        self.on_stack.insert(name.to_string());

        for dependency in self.dependencies.get(name).cloned().unwrap_or_default() {
            if !self.indices.contains_key(&dependency) {
                self.visit(&dependency);
                let dependency_lowlink = self.lowlinks[&dependency];
                self.lowlinks.entry(name.to_string()).and_modify(|lowlink| *lowlink = (*lowlink).min(dependency_lowlink));
            } else if self.on_stack.contains(&dependency) {
                let dependency_index = self.indices[&dependency];
                self.lowlinks.entry(name.to_string()).and_modify(|lowlink| *lowlink = (*lowlink).min(dependency_index));
            }
        }

        if self.lowlinks[name] != self.indices[name] {
            return;
        }
        let mut component = Vec::new();
        while let Some(member) = self.stack.pop() {
            self.on_stack.remove(&member);
            let is_root = member == name;
            component.push(member);
            if is_root {
                break;
            }
        }
        let self_referential = component.len() == 1 && self.dependencies.get(name).is_some_and(|dependencies| dependencies.iter().any(|dependency| dependency == name));
        if component.len() > 1 || self_referential {
            self.cyclic.extend(component);
        }
    }
}

fn cyclic_custom_properties<'a>(specified: &FxHashMap<String, TokenList<'a>>) -> (FxHashMap<String, Vec<String>>, HashSet<String>) {
    let dependencies = specified
        .iter()
        .map(|(name, value)| {
            let local_dependencies = custom_property_dependencies(value).into_iter().filter(|dependency| specified.contains_key(dependency)).collect();
            (name.clone(), local_dependencies)
        })
        .collect::<FxHashMap<_, _>>();
    let mut detector = CustomPropertyCycleDetector { dependencies: &dependencies, next_index: 0, indices: FxHashMap::default(), lowlinks: FxHashMap::default(), stack: Vec::new(), on_stack: HashSet::new(), cyclic: HashSet::new() };
    for name in specified.keys() {
        if !detector.indices.contains_key(name) {
            detector.visit(name);
        }
    }
    let cyclic = detector.cyclic;
    (dependencies, cyclic)
}

fn resolve_custom_property<'a>(
    name: &str, specified: &FxHashMap<String, TokenList<'a>>, parent_custom: &FxHashMap<String, TokenList<'a>>, dependencies: &FxHashMap<String, Vec<String>>, cyclic: &HashSet<String>, resolving: &mut HashSet<String>,
    resolved: &mut FxHashMap<String, TokenList<'a>>,
) -> bool {
    if resolved.contains_key(name) {
        return true;
    }
    if cyclic.contains(name) || !resolving.insert(name.to_string()) {
        return false;
    }
    let Some(mut value) = specified.get(name).cloned() else {
        resolving.remove(name);
        return false;
    };

    if let Some(names) = dependencies.get(name) {
        for dependency in names {
            let _ = resolve_custom_property(dependency, specified, parent_custom, dependencies, cyclic, resolving, resolved);
        }
    }

    let variables = resolved.iter().map(|(name, value)| (name.as_str(), value.clone())).collect::<HashMap<_, _>>();
    mark_var_substitution_boundaries(&mut value);
    value.substitute_variables(&variables);
    resolving.remove(name);
    if token_list_contains_var(&value) {
        return false;
    }
    if let Some(keyword) = single_ident_keyword(&value).map(str::to_ascii_lowercase) {
        match keyword.as_str() {
            "initial" => return false,
            "inherit" | "unset" => {
                let Some(inherited) = parent_custom.get(name) else { return false };
                value = inherited.clone();
            }
            _ => {}
        }
    }
    resolved.insert(name.to_string(), value);
    true
}

fn resolve_custom_properties<'a>(custom_properties: &mut FxHashMap<String, TokenList<'a>>, parent_custom: &FxHashMap<String, TokenList<'a>>) {
    let specified = custom_properties.clone();
    let (dependencies, cyclic) = cyclic_custom_properties(&specified);
    let mut resolving = HashSet::new();
    let mut resolved = FxHashMap::default();
    for name in specified.keys() {
        let _ = resolve_custom_property(name, &specified, parent_custom, &dependencies, &cyclic, &mut resolving, &mut resolved);
    }
    *custom_properties = resolved;
}

/// Keep the component-value boundaries that surrounded each `var()` when
/// LightningCSS substitutes its token list. Its inliner splices lists directly,
/// and its printer otherwise turns adjacent identifiers such as `orange` and
/// `red` into the single, valid color `orangered`. Empty comments disappear
/// when the result is parsed, while preventing tokens on either side of the
/// substitution from being re-tokenized as one token.
fn mark_var_substitution_boundaries(tokens: &mut TokenList<'_>) {
    for token in &mut tokens.0 {
        match token {
            TokenOrValue::Function(function) => mark_var_substitution_boundaries(&mut function.arguments),
            TokenOrValue::Var(variable) => {
                if let Some(fallback) = &mut variable.fallback {
                    mark_var_substitution_boundaries(fallback);
                }
            }
            TokenOrValue::Env(environment) => {
                if let Some(fallback) = &mut environment.fallback {
                    mark_var_substitution_boundaries(fallback);
                }
            }
            _ => {}
        }
    }

    let mut marked = Vec::with_capacity(tokens.0.len());
    for token in tokens.0.drain(..) {
        if matches!(token, TokenOrValue::Var(_)) {
            marked.push(TokenOrValue::Token(Token::Comment("".into())));
            marked.push(token);
            marked.push(TokenOrValue::Token(Token::Comment("".into())));
        } else {
            marked.push(token);
        }
    }
    tokens.0 = marked;
}

fn resolve_single_var_property<'a>(unparsed: &lightningcss::properties::custom::UnparsedProperty<'a>, var_map: &HashMap<&str, TokenList<'a>>) -> Option<Property<'a>> {
    let [TokenOrValue::Var(var)] = unparsed.value.0.as_slice() else {
        return None;
    };
    let replacement = var_map.get(var.name.ident.as_ref())?;
    let css = token_list_to_css_string(replacement)?;
    let leaked: &'a str = Box::leak(css.into_boxed_str());
    Property::parse_string(unparsed.property_id.clone(), leaked, ParserOptions::default()).ok()
}

fn token_list_to_css_string(tokens: &TokenList<'_>) -> Option<String> {
    let mut out = String::new();
    write_token_list(tokens, &mut out)?;
    Some(out)
}

fn write_token_list(tokens: &TokenList<'_>, out: &mut String) -> Option<()> {
    for token in &tokens.0 {
        match token {
            TokenOrValue::Color(color) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                color.to_css(&mut printer).ok()?;
            }
            TokenOrValue::Function(function) => {
                out.push_str(function.name.as_ref());
                out.push('(');
                write_token_list(&function.arguments, out)?;
                out.push(')');
            }
            TokenOrValue::Length(length) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                length.to_css(&mut printer).ok()?;
            }
            TokenOrValue::DashedIdent(ident) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                ident.to_css(&mut printer).ok()?;
            }
            TokenOrValue::AnimationName(name) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                name.to_css(&mut printer).ok()?;
            }
            TokenOrValue::Token(token) => {
                let mut printer = Printer::new(out, PrinterOptions::default());
                token.to_css(&mut printer).ok()?;
            }
            _ => return None,
        }
    }
    Some(())
}

fn canonical_text_spacing_tokens<'a>(spacing: TextSpacing) -> Option<TokenList<'a>> {
    let absolute_px = spacing.absolute_px();
    let percentage = spacing.font_size_fraction() * 100.0;
    let css = if percentage == 0.0 {
        format!("{absolute_px}px")
    } else if absolute_px == 0.0 {
        format!("{percentage}%")
    } else if percentage.is_sign_negative() {
        format!("calc({absolute_px}px - {}%)", percentage.abs())
    } else {
        format!("calc({absolute_px}px + {percentage}%)")
    };
    let leaked: &'a str = Box::leak(css.into_boxed_str());
    TokenList::parse_string_with_options(leaked, ParserOptions::default()).ok()
}

fn canonical_tab_size_tokens<'a>(tab_size: TabSize) -> Option<TokenList<'a>> {
    let css = match tab_size.kind() {
        html_style_model::TabSizeKind::Spaces => tab_size.value().to_string(),
        html_style_model::TabSizeKind::LengthPx => format!("{}px", tab_size.value()),
    };
    let leaked: &'a str = Box::leak(css.into_boxed_str());
    TokenList::parse_string_with_options(leaked, ParserOptions::default()).ok()
}

fn token_list_contains_var(tokens: &TokenList<'_>) -> bool {
    tokens.0.iter().any(|token| match token {
        TokenOrValue::Var(_) => true,
        TokenOrValue::Function(function) => token_list_contains_var(&function.arguments),
        _ => false,
    })
}

fn map_list_style_position(pos: &lightningcss::properties::list::ListStylePosition) -> html_style_model::ListStylePosition {
    use html_style_model::ListStylePosition as Out;
    use lightningcss::properties::list::ListStylePosition as Lc;
    match pos {
        Lc::Inside => Out::Inside,
        Lc::Outside => Out::Outside,
    }
}

/// Map Lightning CSS `list-style-type` onto the renderer's exact supported
/// set. Values requiring custom counter-style or marker-string rendering are
/// kept out of computed state instead of being silently approximated.
fn map_list_style_type(lst: &lightningcss::properties::list::ListStyleType) -> Option<html_style_model::ListStyleType> {
    use html_style_model::ListStyleType as Out;
    use lightningcss::properties::list::{CounterStyle, ListStyleType as Lc, PredefinedCounterStyle as P};
    Some(match lst {
        Lc::None => Out::None,
        Lc::String(_) => return None,
        Lc::CounterStyle(CounterStyle::Predefined(p)) => match p {
            P::Disc => Out::Disc,
            P::Circle => Out::Circle,
            P::Square => Out::Square,
            P::Decimal => Out::Decimal,
            P::DecimalLeadingZero => Out::DecimalLeadingZero,
            P::LowerAlpha | P::LowerLatin => Out::LowerAlpha,
            P::UpperAlpha | P::UpperLatin => Out::UpperAlpha,
            P::LowerRoman => Out::LowerRoman,
            P::UpperRoman => Out::UpperRoman,
            _ => return None,
        },
        Lc::CounterStyle(CounterStyle::Name(_) | CounterStyle::Symbols { .. }) => return None,
    })
}

/// Intern the URL of a `list-style-image`. Gradients / `image-set()` / `none`
/// yield `None`. The interned string index (a `u16`) is widened into the
/// style's `u32` slot; it is resolved to an image resource when the marker box
/// is built.
fn list_style_image_to_interned(styles: &mut ComputedStylesBuilder, image: &lightningcss::values::image::Image) -> Option<StyleStringId> {
    use lightningcss::values::image::Image;
    match image {
        Image::Url(url) => {
            let raw = url.url.as_ref().trim();
            if raw.is_empty() { None } else { Some(styles.intern_string(raw)) }
        }
        _ => None,
    }
}

fn viewport_length_to_px(length: &LengthValue, environment: crate::MediaEnvironment) -> Option<f32> {
    if matches!(length, LengthValue::Vw(_) | LengthValue::Vh(_) | LengthValue::Vmin(_) | LengthValue::Vmax(_)) {
        RESOLUTION_USES_VIEWPORT_UNITS.set(true);
    }
    let width = environment.viewport_width() as f32;
    let value = match length {
        LengthValue::Vw(value) => value * width / 100.0,
        LengthValue::Vh(value) => value * environment.viewport_height()? as f32 / 100.0,
        LengthValue::Vmin(value) => value * width.min(environment.viewport_height()? as f32) / 100.0,
        LengthValue::Vmax(value) => value * width.max(environment.viewport_height()? as f32) / 100.0,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

fn font_size_to_px(size: &FontSize, parent_font_size: f32, root_font_size: f32, environment: crate::MediaEnvironment) -> Option<f32> {
    match size {
        FontSize::Length(LengthPercentage::Dimension(length)) if viewport_length_to_px(length, environment).is_some() => viewport_length_to_px(length, environment),
        FontSize::Length(lp) => length_percentage_to_px(lp, parent_font_size, root_font_size),
        FontSize::Absolute(abs) => Some({
            use lightningcss::properties::font::AbsoluteFontSize;
            match abs {
                AbsoluteFontSize::XXSmall => 9.0,
                AbsoluteFontSize::XSmall => 10.0,
                AbsoluteFontSize::Small => 13.0,
                AbsoluteFontSize::Medium => 16.0,
                AbsoluteFontSize::Large => 18.0,
                AbsoluteFontSize::XLarge => 24.0,
                AbsoluteFontSize::XXLarge => 32.0,
                AbsoluteFontSize::XXXLarge => 48.0,
            }
        }),
        FontSize::Relative(rel) => Some({
            use lightningcss::properties::font::RelativeFontSize;
            match rel {
                RelativeFontSize::Smaller => parent_font_size * 0.833,
                RelativeFontSize::Larger => parent_font_size * 1.2,
            }
        }),
    }
}

fn checked_font_size(size: &FontSize, parent_font_size: f32, root_font_size: f32, environment: crate::MediaEnvironment) -> Option<f32> {
    let value = font_size_to_px(size, parent_font_size, root_font_size, environment)?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn checked_font_size_components(size: &FontSize, inherited_font: &Font, parent_font_size: f32, root_font_size: f32, environment: crate::MediaEnvironment) -> Option<(f32, f32, f32, f32, f32, f32, f32)> {
    if let FontSize::Length(LengthPercentage::Dimension(length)) = size {
        return match length {
            LengthValue::Cap(value) if value.is_finite() && *value >= 0.0 => Some((0.0, 0.0, 0.0, *value * inherited_font.font_size, 0.0, 0.0, 0.0)),
            LengthValue::Rch(value) if value.is_finite() && *value >= 0.0 => Some((0.0, 0.0, 0.0, 0.0, *value, 0.0, 0.0)),
            LengthValue::Rcap(value) if value.is_finite() && *value >= 0.0 => Some((0.0, 0.0, 0.0, 0.0, 0.0, *value, 0.0)),
            LengthValue::Rlh(value) if value.is_finite() && *value >= 0.0 => Some((0.0, 0.0, 0.0, 0.0, 0.0, 0.0, *value)),
            _ => checked_font_size(size, parent_font_size, root_font_size, environment).map(|value| (value, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)),
        };
    }
    if let FontSize::Length(LengthPercentage::Calc(calc)) = size {
        let (absolute_px, percentage, x_height_px, ch_advance_px, cap_height_px) = calc_box_length_percentage_components(calc, inherited_font.font_size, root_font_size)?;
        let absolute_px = absolute_px + percentage * parent_font_size;
        return (absolute_px.is_finite() && absolute_px >= 0.0 && x_height_px.is_finite() && ch_advance_px.is_finite() && cap_height_px.is_finite()).then_some((absolute_px, x_height_px, ch_advance_px, cap_height_px, 0.0, 0.0, 0.0));
    }
    checked_font_size(size, parent_font_size, root_font_size, environment).map(|value| (value, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
}

fn checked_font_weight(weight: &FontWeight, parent_weight: u16) -> Option<u16> {
    let value = match weight {
        FontWeight::Absolute(abs) => match abs {
            AbsoluteFontWeight::Normal => 400,
            AbsoluteFontWeight::Bold => 700,
            AbsoluteFontWeight::Weight(weight) if weight.is_finite() && *weight >= 1.0 && *weight <= 1000.0 => *weight as u16,
            AbsoluteFontWeight::Weight(_) => return None,
        },
        FontWeight::Bolder => match parent_weight {
            0..=349 => 400,
            350..=549 => 700,
            _ => 900,
        },
        FontWeight::Lighter => match parent_weight {
            0..=549 => 100,
            550..=749 => 400,
            _ => 700,
        },
    };
    Some(value)
}

struct CheckedFontShorthand {
    font_size: f32,
    font_weight: u16,
    line_height: f32,
    line_height_x_height_px: f32,
    line_height_number: f32,
    line_height_normal: bool,
}

fn checked_font_shorthand(font: &CssFont<'_>, parent_font_size: f32, parent_font_weight: u16, root_font_size: f32, environment: crate::MediaEnvironment) -> Option<CheckedFontShorthand> {
    let font_size = checked_font_size(&font.size, parent_font_size, root_font_size, environment)?;
    let font_weight = checked_font_weight(&font.weight, parent_font_weight)?;
    let (line_height, line_height_x_height_px) = checked_line_height_components(&font.line_height, font_size, root_font_size)?;
    let line_height_number = line_height_number(&font.line_height);
    let line_height_normal = line_height_is_normal(&font.line_height);
    Some(CheckedFontShorthand { font_size, font_weight, line_height, line_height_x_height_px, line_height_number, line_height_normal })
}

fn computed_line_height(font: &Font, text: &InheritedText) -> Option<f32> {
    // Keep `lh` aligned with the renderer's established used-value behavior
    // for `normal` (the same 1.2 multiplier used by shaping and inline layout).
    let value = if text.line_height_normal { font.font_size * 1.2 } else { text.line_height };
    value.is_finite().then_some(value)
}

/// CSS Values makes `lh` in font-size and line-height refer to the parent's
/// computed line height, avoiding a dependency cycle. Other properties use
/// the element's own (already prerequisite-resolved) computed line height.
fn set_line_height_resolution_bases(doc: &Document, styles: &ComputedStylesBuilder, property: &Property<'_>, style: &WorkingStyle, parent: &ParentStyle) {
    let (font, text) = if matches!(property, Property::FontSize(_) | Property::LineHeight(_) | Property::Font(_)) { (&parent.font, &parent.text) } else { (&style.font, &style.text) };
    RESOLUTION_LINE_HEIGHT.set(computed_line_height(font, text));

    let root_line_height =
        doc.dom_root().and_then(|root| styles.style_for_node(root)).and_then(|indices| Some((styles.font_style(indices)?, styles.text_style(indices)?))).and_then(|(font, text)| computed_line_height(font, text)).or_else(|| {
            // While resolving the root itself, font-size and line-height must
            // use their initial basis to avoid a cycle. Other root properties
            // can use the prerequisite-resolved root line height.
            if matches!(property, Property::FontSize(_) | Property::LineHeight(_) | Property::Font(_)) {
                let initial = ParentStyle::initial(doc.root_font_size());
                computed_line_height(&initial.font, &initial.text)
            } else {
                computed_line_height(&style.font, &style.text)
            }
        });
    RESOLUTION_ROOT_LINE_HEIGHT.set(root_line_height);
}

fn property_has_faulty_numeric_value(doc: &Document, style: &WorkingStyle, property: &Property<'_>, parent_font_size: f32, environment: crate::MediaEnvironment) -> bool {
    let root_font_size = doc.root_font_size();
    let font_size = style.font.font_size;
    match property {
        Property::FontSize(value) => checked_font_size(value, parent_font_size, root_font_size, environment).is_none(),
        Property::FontWeight(value) => checked_font_weight(value, style.font.font_weight).is_none(),
        Property::Font(value) => checked_font_shorthand(value, parent_font_size, style.font.font_weight, root_font_size, environment).is_none(),
        Property::LineHeight(value) => checked_line_height(value, font_size, root_font_size).is_none(),
        Property::BorderWidth(value) => [&value.top, &value.right, &value.bottom, &value.left].into_iter().any(|value| checked_border_width(value, font_size, root_font_size).is_none()),
        Property::BorderTopWidth(value) | Property::BorderRightWidth(value) | Property::BorderBottomWidth(value) | Property::BorderLeftWidth(value) => checked_border_width(value, font_size, root_font_size).is_none(),
        Property::Border(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderTop(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderRight(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderBottom(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderLeft(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderBlockStartWidth(value) | Property::BorderBlockEndWidth(value) | Property::BorderInlineStartWidth(value) | Property::BorderInlineEndWidth(value) => checked_border_width(value, font_size, root_font_size).is_none(),
        Property::BorderBlockWidth(value) => checked_border_width(&value.start, font_size, root_font_size).is_none() || checked_border_width(&value.end, font_size, root_font_size).is_none(),
        Property::BorderInlineWidth(value) => checked_border_width(&value.start, font_size, root_font_size).is_none() || checked_border_width(&value.end, font_size, root_font_size).is_none(),
        Property::BorderBlockStart(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderBlockEnd(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderInlineStart(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderInlineEnd(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderBlock(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::BorderInline(value) => checked_border_width(&value.width, font_size, root_font_size).is_none(),
        Property::FlexGrow(value, _) | Property::FlexShrink(value, _) => checked_flex_factor(*value).is_none(),
        Property::Flex(value, _) => checked_flex_factor(value.grow).is_none() || checked_flex_factor(value.shrink).is_none(),
        _ => false,
    }
}

// CSS math may produce positive infinity for a non-negative numeric property.
// Keep the computed-style/layout boundary finite while preserving the fact
// that such a factor dominates ordinary authored values. The cap also avoids
// overflow when the layout backend sums several large flex factors.
const MAX_COMPUTED_FLEX_FACTOR: f32 = 1_000_000.0;

fn checked_flex_factor(value: f32) -> Option<f32> {
    if value.is_nan() || value < 0.0 {
        None
    } else if value.is_infinite() {
        value.is_sign_positive().then_some(MAX_COMPUTED_FLEX_FACTOR)
    } else {
        Some(value.min(MAX_COMPUTED_FLEX_FACTOR))
    }
}

fn length_to_px(length: &LengthValue, parent_font_size: f32, root_font_size: f32) -> Option<f32> {
    Some(match length {
        LengthValue::Px(px) => *px,
        LengthValue::Em(em) => em * parent_font_size,
        LengthValue::Rem(rem) => rem * root_font_size,
        LengthValue::Lh(lh) => lh * RESOLUTION_LINE_HEIGHT.get()?,
        LengthValue::Rlh(rlh) => rlh * RESOLUTION_ROOT_LINE_HEIGHT.get()?,
        LengthValue::Pt(pt) => pt * (96.0 / 72.0),
        LengthValue::In(inches) => inches * 96.0,
        LengthValue::Cm(cm) => cm * (96.0 / 2.54),
        LengthValue::Mm(mm) => mm * (96.0 / 25.4),
        LengthValue::Q(q) => q * (96.0 / 101.6),
        LengthValue::Pc(pc) => pc * 16.0,
        LengthValue::Vw(_) | LengthValue::Vh(_) | LengthValue::Vmin(_) | LengthValue::Vmax(_) => {
            return RESOLUTION_MEDIA_ENVIRONMENT.with(|environment| viewport_length_to_px(length, environment.get()));
        }
        // These require selected-face metrics that are deliberately absent
        // during computed-style construction. Callers either preserve `ex`
        // symbolically or reject the declaration; they never guess a value.
        LengthValue::Ex(_) | LengthValue::Ch(_) | LengthValue::Cap(_) => return None,
        _ => return None,
    })
}

fn length_pct_from_length(length: &LengthValue, font_size: f32, root_font_size: f32) -> Option<LengthPct> {
    match length {
        LengthValue::Ex(ex) => Some(LengthPct::Ex(ex * font_size)),
        LengthValue::Ch(ch) => Some(LengthPct::Ch(ch * font_size)),
        LengthValue::Cap(cap) => Some(LengthPct::Cap(cap * font_size)),
        _ => Some(LengthPct::Px(length_to_px(length, font_size, root_font_size)?)),
    }
}

fn preferred_from_length(length: &LengthValue, font: &Font, root_font_size: f32) -> Option<PreferredSize> {
    match length {
        LengthValue::Ex(ex) => Some(PreferredSize::Ex(ex * font.font_size)),
        LengthValue::Ch(ch) => Some(PreferredSize::Ch(ch * font.font_size)),
        LengthValue::Cap(cap) => Some(PreferredSize::Cap(cap * font.font_size)),
        LengthValue::Em(em) if font.font_size_x_height_px != 0.0 || font.font_size_ch_advance_px != 0.0 || font.font_size_cap_height_px != 0.0 => Some(PreferredSize::Calc {
            absolute_px: em * font.font_size,
            percentage: 0.0,
            x_height_px: em * font.font_size_x_height_px,
            ch_advance_px: em * font.font_size_ch_advance_px,
            cap_height_px: em * font.font_size_cap_height_px,
            percentage_dependent: false,
        }),
        _ => Some(PreferredSize::Px(length_to_px(length, font.font_size, root_font_size)?)),
    }
}

fn border_length_from_value(length: &LengthValue, font_size: f32, root_font_size: f32) -> Option<FontRelativeLength> {
    match length {
        LengthValue::Ex(ex) => FontRelativeLength::ex(ex * font_size),
        _ => FontRelativeLength::px(length_to_px(length, font_size, root_font_size)?),
    }
}

fn length_percentage_to_px(lp: &LengthPercentage, parent_font_size: f32, root_font_size: f32) -> Option<f32> {
    match lp {
        LengthPercentage::Dimension(len) => length_to_px(len, parent_font_size, root_font_size),
        LengthPercentage::Percentage(p) => Some(parent_font_size * p.0),
        LengthPercentage::Calc(value) => {
            let (px, fraction) = calc_length_percentage_components(value, parent_font_size, root_font_size)?;
            Some(px + parent_font_size * fraction)
        }
    }
}

fn parsed_text_spacing_to_computed(parsed: &ParsedSpacing, font_size: f32, root_font_size: f32) -> Option<TextSpacing> {
    match parsed {
        ParsedSpacing::Normal => Some(TextSpacing::ZERO),
        ParsedSpacing::Value(value) => length_percentage_to_text_spacing(value, font_size, root_font_size),
    }
}

fn parsed_tab_size_to_computed(parsed: &ParsedTabSize, font_size: f32, root_font_size: f32) -> Option<TabSize> {
    match parsed {
        ParsedTabSize::Spaces(value) => TabSize::spaces(*value),
        ParsedTabSize::Length(value) => {
            if let LengthPercentage::Calc(calc) = value
                && let Some(number) = calc_tab_number(calc)
            {
                return TabSize::spaces(number.max(0.0));
            }
            let resolved = length_percentage_to_text_spacing(value, font_size, root_font_size)?;
            (resolved.font_size_fraction() == 0.0).then(|| TabSize::length_px(resolved.absolute_px().max(0.0))).flatten()
        }
    }
}

fn calc_tab_number(calc: &Calc<LengthPercentage>) -> Option<f32> {
    match calc {
        Calc::Value(value) => match value.as_ref() {
            LengthPercentage::Calc(nested) => calc_tab_number(nested),
            _ => None,
        },
        Calc::Number(value) => Some(*value),
        Calc::Sum(left, right) => Some(calc_tab_number(left)? + calc_tab_number(right)?),
        Calc::Product(factor, value) => Some(*factor * calc_tab_number(value)?),
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => calc_tab_number(value),
            MathFunction::Min(values) => values.iter().map(calc_tab_number).reduce(|left, right| Some(left?.min(right?)))?,
            MathFunction::Max(values) => values.iter().map(calc_tab_number).reduce(|left, right| Some(left?.max(right?)))?,
            MathFunction::Clamp(min, value, max) => Some(calc_tab_number(value)?.clamp(calc_tab_number(min)?, calc_tab_number(max)?)),
            MathFunction::Abs(value) => Some(calc_tab_number(value)?.abs()),
            MathFunction::Sign(value) => Some(calc_tab_number(value)?.signum()),
            _ => None,
        },
    }
}

fn length_percentage_to_text_spacing(value: &LengthPercentage, font_size: f32, root_font_size: f32) -> Option<TextSpacing> {
    use lightningcss::values::percentage::DimensionPercentage;
    match value {
        DimensionPercentage::Dimension(length) => TextSpacing::from_px(length_to_px(length, font_size, root_font_size)?),
        DimensionPercentage::Percentage(percentage) => TextSpacing::new(0.0, percentage.0),
        DimensionPercentage::Calc(calc) => {
            let (absolute_px, font_size_fraction) = calc_length_percentage_components(calc, font_size, root_font_size)?;
            TextSpacing::new(absolute_px, font_size_fraction)
        }
    }
}

fn vertical_align_value(value: &LengthPercentage, font_size: f32, root_font_size: f32) -> Option<VerticalAlignValue> {
    match value {
        LengthPercentage::Dimension(LengthValue::Ex(value)) => Some(VerticalAlignValue::Calc { absolute_px: 0.0, line_height_fraction: 0.0, x_height_px: value * font_size }),
        LengthPercentage::Dimension(length) => Some(VerticalAlignValue::Length(length_to_px(length, font_size, root_font_size)?)),
        LengthPercentage::Percentage(percentage) => Some(VerticalAlignValue::Percent(percentage.0)),
        LengthPercentage::Calc(calc) => {
            let (absolute_px, line_height_fraction, x_height_px, ch_advance_px, cap_height_px) = calc_box_length_percentage_components(calc, font_size, root_font_size)?;
            if ch_advance_px != 0.0 || cap_height_px != 0.0 {
                return None;
            }
            Some(VerticalAlignValue::Calc { absolute_px, line_height_fraction, x_height_px })
        }
    }
}

/// Reduce Lightning CSS's typed linear calc tree while retaining the two
/// selected-face metric terms that cannot become pixels until shaping.
fn calc_box_length_percentage_components(calc: &Calc<LengthPercentage>, font_size: f32, root_font_size: f32) -> Option<(f32, f32, f32, f32, f32)> {
    match calc {
        Calc::Value(value) => match value.as_ref() {
            LengthPercentage::Dimension(LengthValue::Ex(value)) => Some((0.0, 0.0, value * font_size, 0.0, 0.0)),
            LengthPercentage::Dimension(LengthValue::Ch(value)) => Some((0.0, 0.0, 0.0, value * font_size, 0.0)),
            LengthPercentage::Dimension(LengthValue::Cap(value)) => Some((0.0, 0.0, 0.0, 0.0, value * font_size)),
            LengthPercentage::Dimension(value) => Some((length_to_px(value, font_size, root_font_size)?, 0.0, 0.0, 0.0, 0.0)),
            LengthPercentage::Percentage(value) => Some((0.0, value.0, 0.0, 0.0, 0.0)),
            LengthPercentage::Calc(value) => calc_box_length_percentage_components(value, font_size, root_font_size),
        },
        Calc::Number(value) => (*value == 0.0).then_some((0.0, 0.0, 0.0, 0.0, 0.0)),
        Calc::Sum(left, right) => {
            let left = calc_box_length_percentage_components(left, font_size, root_font_size)?;
            let right = calc_box_length_percentage_components(right, font_size, root_font_size)?;
            Some((left.0 + right.0, left.1 + right.1, left.2 + right.2, left.3 + right.3, left.4 + right.4))
        }
        Calc::Product(factor, value) => {
            let value = calc_box_length_percentage_components(value, font_size, root_font_size)?;
            Some((*factor * value.0, *factor * value.1, *factor * value.2, *factor * value.3, *factor * value.4))
        }
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => calc_box_length_percentage_components(value, font_size, root_font_size),
            _ => None,
        },
    }
}

fn calc_length_percentage_components(calc: &Calc<LengthPercentage>, font_size: f32, root_font_size: f32) -> Option<(f32, f32)> {
    match calc {
        Calc::Value(value) => {
            let spacing = length_percentage_to_text_spacing(value, font_size, root_font_size)?;
            Some((spacing.absolute_px(), spacing.font_size_fraction()))
        }
        Calc::Number(value) => (*value == 0.0).then_some((0.0, 0.0)),
        Calc::Sum(left, right) => {
            let left = calc_length_percentage_components(left, font_size, root_font_size)?;
            let right = calc_length_percentage_components(right, font_size, root_font_size)?;
            Some((left.0 + right.0, left.1 + right.1))
        }
        Calc::Product(factor, value) => {
            let value = calc_length_percentage_components(value, font_size, root_font_size)?;
            Some((*factor * value.0, *factor * value.1))
        }
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => calc_length_percentage_components(value, font_size, root_font_size),
            // Non-linear math functions cannot retain an unresolved percentage
            // as a two-component computed value without keeping the parser AST.
            _ => None,
        },
    }
}

fn calc_depends_on_percentage(calc: &Calc<LengthPercentage>) -> bool {
    match calc {
        Calc::Value(value) => match value.as_ref() {
            LengthPercentage::Percentage(_) => true,
            LengthPercentage::Calc(inner) => calc_depends_on_percentage(inner),
            LengthPercentage::Dimension(_) => false,
        },
        Calc::Number(_) => false,
        Calc::Sum(left, right) => calc_depends_on_percentage(left) || calc_depends_on_percentage(right),
        Calc::Product(_, value) => calc_depends_on_percentage(value),
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => calc_depends_on_percentage(value),
            _ => false,
        },
    }
}

/// Convert a margin/padding value into a `LengthPct`. Lengths (including `em`/`rem`)
/// resolve to pixels now; percentages are kept as a fraction to be resolved against
/// the containing-block width at layout time. `auto` margins are treated as `0`
/// (auto-margin centering is not implemented), matching prior behavior.
fn length_or_auto_to_lengthpct(value: &LengthPercentageOrAuto, font_size: f32, root_font_size: f32) -> Option<LengthPct> {
    match value {
        LengthPercentageOrAuto::Auto => Some(LengthPct::Px(0.0)),
        LengthPercentageOrAuto::LengthPercentage(lp) => length_percentage_to_lengthpct(lp, font_size, root_font_size),
    }
}

fn margin_value(value: &LengthPercentageOrAuto, font_size: f32, root_font_size: f32) -> Option<(LengthPct, bool)> {
    Some((length_or_auto_to_lengthpct(value, font_size, root_font_size)?, matches!(value, LengthPercentageOrAuto::Auto)))
}

fn inset_value(value: &LengthPercentageOrAuto, font_size: f32, root_font_size: f32) -> Option<Option<LengthPct>> {
    match value {
        LengthPercentageOrAuto::Auto => Some(None),
        LengthPercentageOrAuto::LengthPercentage(value) => Some(Some(length_percentage_to_lengthpct(value, font_size, root_font_size)?)),
    }
}

/// Convert a `<length-percentage>` (e.g. `text-indent`) into a `LengthPct`,
/// deferring percentage resolution to layout (against the containing-block width).
fn length_percentage_to_lengthpct(lp: &LengthPercentage, font_size: f32, root_font_size: f32) -> Option<LengthPct> {
    match lp {
        LengthPercentage::Dimension(len) => length_pct_from_length(len, font_size, root_font_size),
        LengthPercentage::Percentage(p) => Some(LengthPct::Pct(p.0)),
        LengthPercentage::Calc(value) => computed_length_pct(&LengthPercentage::Calc(value.clone()), font_size, root_font_size),
    }
}

fn size_to_preferred(size: &Size, font: &Font, root_font_size: f32, styles: &mut ComputedStylesBuilder) -> Option<PreferredSize> {
    Some(match size {
        Size::Auto => PreferredSize::Auto,
        Size::MinContent(_) => PreferredSize::MinContent,
        Size::MaxContent(_) => PreferredSize::MaxContent,
        Size::FitContent(_) => PreferredSize::FitContent,
        Size::Stretch(_) => PreferredSize::Stretch,
        Size::LengthPercentage(lp) => match lp {
            LengthPercentage::Dimension(len) => preferred_from_length(len, font, root_font_size)?,
            LengthPercentage::Percentage(p) => PreferredSize::Percent(p.0),
            LengthPercentage::Calc(value) => length_percentage_to_preferred(&LengthPercentage::Calc(value.clone()), font, root_font_size, styles)?,
        },
        _ => PreferredSize::Auto,
    })
}

fn max_size_to_preferred(size: &MaxSize, font: &Font, root_font_size: f32, styles: &mut ComputedStylesBuilder) -> Option<PreferredSize> {
    Some(match size {
        MaxSize::None => PreferredSize::Auto,
        MaxSize::MinContent(_) => PreferredSize::MinContent,
        MaxSize::MaxContent(_) => PreferredSize::MaxContent,
        MaxSize::FitContent(_) => PreferredSize::FitContent,
        MaxSize::Stretch(_) => PreferredSize::Stretch,
        MaxSize::LengthPercentage(lp) => match lp {
            LengthPercentage::Dimension(len) => preferred_from_length(len, font, root_font_size)?,
            LengthPercentage::Percentage(p) => PreferredSize::Percent(p.0),
            LengthPercentage::Calc(value) => length_percentage_to_preferred(&LengthPercentage::Calc(value.clone()), font, root_font_size, styles)?,
        },
        _ => PreferredSize::Auto,
    })
}

fn spacing_to_text_spacing(spacing: &Spacing, font_size: f32, root_font_size: f32) -> Option<TextSpacing> {
    match spacing {
        Spacing::Normal => Some(TextSpacing::ZERO),
        Spacing::Length(len) => match len {
            Length::Value(v) => length_to_px(v, font_size, root_font_size).and_then(TextSpacing::from_px),
            Length::Calc(calc) => calc_length_to_px(calc, font_size, root_font_size).and_then(TextSpacing::from_px),
        },
    }
}

fn calc_length_to_px(calc: &Calc<Length>, font_size: f32, root_font_size: f32) -> Option<f32> {
    match calc {
        Calc::Value(length) => match length.as_ref() {
            Length::Value(value) => length_to_px(value, font_size, root_font_size),
            Length::Calc(nested) => calc_length_to_px(nested, font_size, root_font_size),
        },
        Calc::Number(value) => (*value == 0.0).then_some(0.0),
        Calc::Sum(left, right) => Some(calc_length_to_px(left, font_size, root_font_size)? + calc_length_to_px(right, font_size, root_font_size)?),
        Calc::Product(factor, value) => Some(*factor * calc_length_to_px(value, font_size, root_font_size)?),
        Calc::Function(function) => match function.as_ref() {
            MathFunction::Calc(value) => calc_length_to_px(value, font_size, root_font_size),
            MathFunction::Min(values) => values.iter().map(|value| calc_length_to_px(value, font_size, root_font_size)).reduce(|left, right| Some(left?.min(right?)))?,
            MathFunction::Max(values) => values.iter().map(|value| calc_length_to_px(value, font_size, root_font_size)).reduce(|left, right| Some(left?.max(right?)))?,
            MathFunction::Clamp(min, value, max) => Some(calc_length_to_px(value, font_size, root_font_size)?.clamp(calc_length_to_px(min, font_size, root_font_size)?, calc_length_to_px(max, font_size, root_font_size)?)),
            MathFunction::Abs(value) => Some(calc_length_to_px(value, font_size, root_font_size)?.abs()),
            MathFunction::Hypot(values) => {
                let squared = values.iter().try_fold(0.0, |sum, value| {
                    let value = calc_length_to_px(value, font_size, root_font_size)?;
                    Some(sum + value * value)
                })?;
                Some(squared.sqrt())
            }
            _ => None,
        },
    }
}

fn length_to_px_from_length(length: &lightningcss::values::length::Length, font_size: f32, root_font_size: f32) -> Option<f32> {
    match length {
        lightningcss::values::length::Length::Value(v) => length_to_px(v, font_size, root_font_size),
        lightningcss::values::length::Length::Calc(value) => calc_length_to_px(value, font_size, root_font_size),
    }
}

fn border_width(width: &BorderSideWidth, font_size: f32, root_font_size: f32) -> Option<FontRelativeLength> {
    use lightningcss::values::length::Length;
    match width {
        BorderSideWidth::Thin => FontRelativeLength::px(1.0),
        BorderSideWidth::Medium => FontRelativeLength::px(3.0),
        BorderSideWidth::Thick => FontRelativeLength::px(5.0),
        BorderSideWidth::Length(len) => match len {
            Length::Value(v) => border_length_from_value(v, font_size, root_font_size),
            Length::Calc(_) => FontRelativeLength::px(3.0), // Default to medium for calc
        },
    }
}

fn line_style_to_border_style(style: &LineStyle) -> BorderStyle {
    match style {
        LineStyle::None => BorderStyle::None,
        LineStyle::Hidden => BorderStyle::Hidden,
        LineStyle::Solid => BorderStyle::Solid,
        LineStyle::Dashed => BorderStyle::Dashed,
        LineStyle::Dotted => BorderStyle::Dotted,
        LineStyle::Groove => BorderStyle::Groove,
        LineStyle::Ridge => BorderStyle::Ridge,
        // The 3D styles (and `double`) paint as a plain line rather than
        // disappearing; only `none`/`hidden` suppress the border.
        LineStyle::Double | LineStyle::Inset | LineStyle::Outset => BorderStyle::Solid,
    }
}

/// `current_color` is the element's resolved `color`, substituted for
/// `currentColor` (e.g. the default border color). `color` itself is cascaded
/// in an earlier pass, so it is already final when reset properties resolve.
fn css_color_to_u32(color: &CssColor, current_color: u32) -> u32 {
    match color {
        CssColor::RGBA(rgba) => rgba_to_u32(rgba),
        CssColor::CurrentColor => current_color,
        CssColor::System(system) => system_color_to_u32(system),
        // This renderer has a fixed light color scheme. Conversion is paid
        // only for authored non-sRGB colors; the common RGBA path above stays
        // a direct integer pack.
        CssColor::LightDark(light, _) => css_color_to_u32(light, current_color),
        other => match other.to_rgb() {
            Ok(CssColor::RGBA(rgba)) => rgba_to_u32(&rgba),
            Ok(CssColor::LightDark(light, _)) => css_color_to_u32(&light, current_color),
            _ => 0x000000FF,
        },
    }
}

fn rgba_to_u32(rgba: &lightningcss::values::color::RGBA) -> u32 {
    ((rgba.red as u32) << 24) | ((rgba.green as u32) << 16) | ((rgba.blue as u32) << 8) | (rgba.alpha as u32)
}

/// Resolve CSS system colors to fixed light-theme values (the renderer has no
/// OS theme to consult). Unlisted ones are text colors and fall back to black.
fn system_color_to_u32(color: &SystemColor) -> u32 {
    use SystemColor as S;
    match color {
        S::Mark => 0xFFFF00FF,
        S::Canvas | S::Field | S::Window | S::InfoBackground | S::Menu | S::ButtonFace | S::Scrollbar | S::AppWorkspace | S::ActiveCaption | S::Background | S::ButtonHighlight | S::ButtonShadow | S::InactiveCaption | S::ThreeDFace => {
            0xFFFFFFFF
        }
        S::LinkText => 0x0000EEFF,
        S::VisitedText => 0x551A8BFF,
        S::ActiveText => 0xEE0000FF,
        S::GrayText | S::InactiveCaptionText => 0x808080FF,
        S::Highlight | S::SelectedItem | S::AccentColor => 0x0078D7FF,
        S::HighlightText | S::SelectedItemText | S::AccentColorText => 0xFFFFFFFF,
        _ => 0x000000FF,
    }
}

fn intern_font_family(styles: &mut ComputedStylesBuilder, families: &[FontFamily]) -> StyleStringId {
    let mut out = String::new();
    for (i, family) in families.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = family.to_css(&mut Printer::new(&mut out, PrinterOptions::default()));
    }
    styles.intern_string(&out)
}

fn log_unhandled_property(property: &Property) {
    static UNHANDLED_PROPERTIES: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    let set = UNHANDLED_PROPERTIES.get_or_init(|| Mutex::new(HashSet::new()));
    let mut hasher = DefaultHasher::new();
    std::mem::discriminant(property).hash(&mut hasher);
    let id = hasher.finish();
    let mut guard = set.lock().unwrap();
    if guard.insert(id) {
        println!("Unhandled CSS property: {:?}", property);
    }
}

/// Compute style for a single DOM element (used in new pipeline)
impl<'a, 'sheet, 'css> StyleResolverContext<'a, 'sheet, 'css> {
    fn compute_style_for_dom_element(&mut self, node_idx: DomNodeId, parent_custom: Option<FxHashMap<String, TokenList<'css>>>, ancestor_filter: &AncestorFilter) -> (WorkingStyle, FxHashMap<String, TokenList<'css>>) {
        let doc = self.doc;
        let prepared = self.prepared;
        let index = self.index;
        let matching_started = Instant::now();
        let mut matched_rules: Vec<MatchedRule> = Vec::new();

        // Get element info for candidate lookup
        let tag = doc.get_dom_tag(node_idx).unwrap_or("");
        let id = doc.get_dom_id(node_idx);

        // Get candidate rule indices
        index.collect_candidates(tag, id, doc.get_dom_classes(node_idx), self.candidate_scratch, self.candidate_seen);

        // Check each candidate. A rule cascades with the most specific of its
        // matching selectors, not the first one that happens to match.
        for id in self.candidate_scratch.iter().copied() {
            let style_rule = prepared.get(id).style_rule();
            let Some(scope_match) = prepared.scope_match(prepared.get(id).scope(), doc, node_idx) else { continue };
            let specificity = style_rule
                .selectors
                .0
                .iter()
                .filter(|selector| selector_might_match_dom(selector, ancestor_filter, doc, node_idx) && crate::style::selectors_dom::selector_matches_dom_node_in_scope(selector, doc, node_idx, scope_match.root))
                .map(crate::style::selectors_dom::selector_specificity)
                .max();
            if let Some(specificity) = specificity {
                matched_rules.push(MatchedRule { specificity, id, scope_proximity: scope_match.proximity });
            }
        }

        let mut important_rules = matched_rules.clone();
        matched_rules.sort_by(|a, b| prepared.get(a.id).priority().compare_normal(a.specificity, a.scope_proximity, prepared.get(b.id).priority(), b.specificity, b.scope_proximity));
        important_rules.sort_by(|a, b| prepared.get(a.id).priority().compare_important(a.specificity, a.scope_proximity, prepared.get(b.id).priority(), b.specificity, b.scope_proximity));
        self.timings.selector_matching += matching_started.elapsed();

        // Build computed style from matched rules (same as box-based version)
        let cascade_started = Instant::now();
        let mut style = get_inherited_style_dom(doc, self.styles, node_idx);
        let parent_font_size = style.font.font_size;
        let mut custom_properties = parent_custom.clone().unwrap_or_default();
        let parent_custom = parent_custom.unwrap_or_default();
        let rollback_layers = rollback_layers(&matched_rules, prepared);
        let inline_style_attr = inline_style_attribute(doc, node_idx);
        let inline_style = parse_inline_style_attribute(doc, node_idx);
        let mut rollback_origins = rollback_origins(&matched_rules, prepared);
        if inline_style.as_ref().is_some_and(|style| declarations_use_revert(&style.declarations.declarations) || declarations_use_revert(&style.declarations.important_declarations)) && !rollback_origins.contains(&CascadeOrigin::Author) {
            rollback_origins.push(CascadeOrigin::Author);
        }
        let unused_custom_rollback_basis = FxHashMap::default();
        let mut ancestor_indices: Vec<_> = doc.dom_ancestors(node_idx).collect();
        ancestor_indices.reverse();
        for ancestor_idx in ancestor_indices {
            if let Some(style_attr) = inline_style_attribute(doc, ancestor_idx) {
                let lower_origin_basis = style_attr.to_ascii_lowercase().contains("revert").then(|| custom_properties.clone());
                let layer_basis = style_attr.to_ascii_lowercase().contains("revert-layer").then(|| custom_properties.clone());
                collect_inline_style_custom_properties(
                    &mut custom_properties,
                    &parent_custom,
                    lower_origin_basis.as_ref().unwrap_or(&unused_custom_rollback_basis),
                    layer_basis.as_ref().unwrap_or(&unused_custom_rollback_basis),
                    &style_attr,
                );
            }
        }

        // Collect custom properties
        let mut normal_custom_origin_baselines = Vec::new();
        let mut normal_custom_baselines = Vec::new();
        for matched in &matched_rules {
            let rule = prepared.get(matched.id);
            let priority = rule.priority();
            if origin_needs_rollback(&rollback_origins, priority.origin()) && !normal_custom_origin_baselines.iter().any(|(previous, _): &(CascadeOrigin, FxHashMap<String, TokenList<'css>>)| *previous == priority.origin()) {
                normal_custom_origin_baselines.push((priority.origin(), custom_properties.clone()));
            }
            if layer_needs_rollback(&rollback_layers, priority) && !normal_custom_baselines.iter().any(|(previous, _): &(RulePriority, FxHashMap<String, TokenList<'css>>)| previous.same_origin_and_layer(priority)) {
                normal_custom_baselines.push((priority, custom_properties.clone()));
            }
            let origin_basis = normal_custom_origin_baselines.iter().find_map(|(origin, basis)| (*origin == priority.origin()).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            let layer_basis = normal_custom_baselines.iter().find_map(|(candidate, basis)| candidate.same_origin_and_layer(priority).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            collect_custom_properties(&mut custom_properties, &parent_custom, &rule.style_rule().declarations.declarations, origin_basis, layer_basis);
        }
        if rollback_origins.contains(&CascadeOrigin::Author) && !normal_custom_origin_baselines.iter().any(|(origin, _)| *origin == CascadeOrigin::Author) {
            normal_custom_origin_baselines.push((CascadeOrigin::Author, custom_properties.clone()));
        }
        if let Some(inline_style) = &inline_style {
            let origin_basis = normal_custom_origin_baselines.iter().find_map(|(origin, basis)| (*origin == CascadeOrigin::Author).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            let layer_basis = declarations_use_revert_layer(&inline_style.declarations.declarations).then(|| custom_properties.clone());
            collect_custom_properties(&mut custom_properties, &parent_custom, &inline_style.declarations.declarations, origin_basis, layer_basis.as_ref().unwrap_or(&unused_custom_rollback_basis));
        }
        if let Some(inline_style_attr) = &inline_style_attr {
            let origin_basis = normal_custom_origin_baselines.iter().find_map(|(origin, basis)| (*origin == CascadeOrigin::Author).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            let layer_basis = inline_style_attr.to_ascii_lowercase().contains("revert-layer").then(|| custom_properties.clone());
            collect_inline_style_custom_properties(&mut custom_properties, &parent_custom, origin_basis, layer_basis.as_ref().unwrap_or(&unused_custom_rollback_basis), inline_style_attr);
        }
        for matched in important_rules.iter().filter(|matched| prepared.get(matched.id).priority().origin() == crate::style::prepared::CascadeOrigin::Author) {
            let rule = prepared.get(matched.id);
            let origin_basis = normal_custom_origin_baselines.iter().find_map(|(origin, basis)| (*origin == rule.priority().origin()).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            let layer_basis = normal_custom_baselines.iter().find_map(|(priority, basis)| priority.same_origin_and_layer(rule.priority()).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            collect_custom_properties(&mut custom_properties, &parent_custom, &rule.style_rule().declarations.important_declarations, origin_basis, layer_basis);
        }
        if let Some(inline_style) = &inline_style {
            let origin_basis = normal_custom_origin_baselines.iter().find_map(|(origin, basis)| (*origin == CascadeOrigin::Author).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            let layer_basis = declarations_use_revert_layer(&inline_style.declarations.important_declarations).then(|| custom_properties.clone());
            collect_custom_properties(&mut custom_properties, &parent_custom, &inline_style.declarations.important_declarations, origin_basis, layer_basis.as_ref().unwrap_or(&unused_custom_rollback_basis));
        }
        for matched in important_rules.iter().filter(|matched| prepared.get(matched.id).priority().origin() == crate::style::prepared::CascadeOrigin::UserAgent) {
            let rule = prepared.get(matched.id);
            let origin_basis = normal_custom_origin_baselines.iter().find_map(|(origin, basis)| (*origin == rule.priority().origin()).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            let layer_basis = normal_custom_baselines.iter().find_map(|(priority, basis)| priority.same_origin_and_layer(rule.priority()).then_some(basis)).unwrap_or(&unused_custom_rollback_basis);
            collect_custom_properties(&mut custom_properties, &parent_custom, &rule.style_rule().declarations.important_declarations, origin_basis, layer_basis);
        }
        resolve_custom_properties(&mut custom_properties, &parent_custom);

        // Run the full cascade (normal rules, inline normal, important rules,
        // inline important) in dependency order: direction/font-size/color first,
        // then everything else. This retains the existing two scans while making
        // logical-side mapping independent of declaration order.
        let parent_style = ParentStyle::for_node(doc, self.styles, node_idx);
        let unused_rollback_basis = WorkingStyle::default();
        for phase in [CascadePhase::Prerequisites, CascadePhase::Remaining] {
            let mut presentational_hints_applied = false;
            let mut normal_origin_baselines: Vec<(CascadeOrigin, WorkingStyle)> = Vec::new();
            let mut normal_layer_baselines: Vec<(RulePriority, WorkingStyle)> = Vec::new();
            for matched in &matched_rules {
                let rule = prepared.get(matched.id);
                let priority = rule.priority();
                if origin_needs_rollback(&rollback_origins, priority.origin()) && origin_baseline(&normal_origin_baselines, priority.origin()).is_none() {
                    normal_origin_baselines.push((priority.origin(), style.clone()));
                }
                if !presentational_hints_applied && priority.origin() == crate::style::prepared::CascadeOrigin::Author {
                    html_presentational_hints::apply(doc, node_idx, &mut style, phase);
                    presentational_hints_applied = true;
                }
                if layer_needs_rollback(&rollback_layers, priority) && layer_baseline(&normal_layer_baselines, priority).is_none() {
                    normal_layer_baselines.push((priority, style.clone()));
                }
                let origin_basis = origin_baseline(&normal_origin_baselines, priority.origin()).unwrap_or(&unused_rollback_basis);
                let layer_basis = layer_baseline(&normal_layer_baselines, priority).unwrap_or(&unused_rollback_basis);
                self.apply_declarations(&mut style, &rule.style_rule().declarations.declarations, parent_font_size, Some(&custom_properties), phase, &parent_style, origin_basis, layer_basis);
            }
            if !presentational_hints_applied {
                if rollback_origins.contains(&CascadeOrigin::Author) && origin_baseline(&normal_origin_baselines, CascadeOrigin::Author).is_none() {
                    normal_origin_baselines.push((CascadeOrigin::Author, style.clone()));
                }
                html_presentational_hints::apply(doc, node_idx, &mut style, phase);
            }

            if let Some(inline_style) = &inline_style {
                let origin_basis = origin_baseline(&normal_origin_baselines, CascadeOrigin::Author).unwrap_or(&unused_rollback_basis);
                let layer_basis = declarations_use_revert_layer(&inline_style.declarations.declarations).then(|| style.clone());
                self.apply_declarations(&mut style, &inline_style.declarations.declarations, parent_font_size, Some(&custom_properties), phase, &parent_style, origin_basis, layer_basis.as_ref().unwrap_or(&unused_rollback_basis));
            }

            for matched in important_rules.iter().filter(|matched| prepared.get(matched.id).priority().origin() == crate::style::prepared::CascadeOrigin::Author) {
                let rule = prepared.get(matched.id);
                let origin_basis = origin_baseline(&normal_origin_baselines, rule.priority().origin()).unwrap_or(&unused_rollback_basis);
                let layer_basis = layer_baseline(&normal_layer_baselines, rule.priority()).unwrap_or(&unused_rollback_basis);
                self.apply_declarations(&mut style, &rule.style_rule().declarations.important_declarations, parent_font_size, Some(&custom_properties), phase, &parent_style, origin_basis, layer_basis);
            }

            if let Some(inline_style) = &inline_style {
                let origin_basis = origin_baseline(&normal_origin_baselines, CascadeOrigin::Author).unwrap_or(&unused_rollback_basis);
                let layer_basis = declarations_use_revert_layer(&inline_style.declarations.important_declarations).then(|| style.clone());
                self.apply_declarations(&mut style, &inline_style.declarations.important_declarations, parent_font_size, Some(&custom_properties), phase, &parent_style, origin_basis, layer_basis.as_ref().unwrap_or(&unused_rollback_basis));
            }

            for matched in important_rules.iter().filter(|matched| prepared.get(matched.id).priority().origin() == crate::style::prepared::CascadeOrigin::UserAgent) {
                let rule = prepared.get(matched.id);
                let origin_basis = origin_baseline(&normal_origin_baselines, rule.priority().origin()).unwrap_or(&unused_rollback_basis);
                let layer_basis = layer_baseline(&normal_layer_baselines, rule.priority()).unwrap_or(&unused_rollback_basis);
                self.apply_declarations(&mut style, &rule.style_rule().declarations.important_declarations, parent_font_size, Some(&custom_properties), phase, &parent_style, origin_basis, layer_basis);
            }
        }

        if let Some(white_space) = custom_properties.get(WHITE_SPACE_CASCADE_MARKER).and_then(from_marker_tokens) {
            style.text.white_space = white_space;
        }
        for (marker, target) in [(LETTER_SPACING_MARKER, &mut style.text.letter_spacing), (WORD_SPACING_MARKER, &mut style.text.word_spacing)] {
            let Some(spacing) =
                custom_properties.get(marker).and_then(token_list_to_css_string).as_deref().and_then(parse_text_spacing).as_ref().and_then(|parsed| parsed_text_spacing_to_computed(parsed, style.font.font_size, doc.root_font_size()))
            else {
                continue;
            };
            *target = spacing;
            if let Some(tokens) = canonical_text_spacing_tokens(spacing) {
                custom_properties.insert(marker.to_string(), tokens);
            }
        }
        if let Some(tab_size) =
            custom_properties.get(TAB_SIZE_CASCADE_MARKER).and_then(token_list_to_css_string).as_deref().and_then(parse_tab_size).as_ref().and_then(|parsed| parsed_tab_size_to_computed(parsed, style.font.font_size, doc.root_font_size()))
        {
            style.text.tab_size = tab_size;
            if let Some(tokens) = canonical_tab_size_tokens(tab_size) {
                custom_properties.insert(TAB_SIZE_CASCADE_MARKER.to_string(), tokens);
            }
        }

        // HTML `lang` and XML `xml:lang` participate in inherited text
        // semantics even though they are not CSS properties.
        if let Some(language) = doc.element_ref(node_idx).and_then(|element| element.attr_in_any_namespace("lang")).map(str::trim).filter(|language| !language.is_empty()) {
            style.text.language = Some(self.styles.intern_string(language));
        }

        // Preserve `currentColor` as a computed keyword for inheritance while
        // caching this element's resolved RGBA for paint.
        let current_color = style.text.color;
        if style.border.current_color_sides & 0b0001 != 0 {
            style.border.border_top_color = current_color;
        }
        if style.border.current_color_sides & 0b0010 != 0 {
            style.border.border_right_color = current_color;
        }
        if style.border.current_color_sides & 0b0100 != 0 {
            style.border.border_bottom_color = current_color;
        }
        if style.border.current_color_sides & 0b1000 != 0 {
            style.border.border_left_color = current_color;
        }

        self.timings.cascade += cascade_started.elapsed();
        (style, custom_properties)
    }
}

// Helper functions for DOM-based style resolution
use crate::style::selectors::AncestorFilter;

/// Build the starting style for an element: inherited groups (`font`, `text`)
/// copied from the parent, reset groups (`box_model`, `border`, `background`)
/// left at their defaults.
fn get_inherited_style_dom(doc: &Document, styles: &ComputedStylesBuilder, node_idx: DomNodeId) -> WorkingStyle {
    let style = if let Some(parent_idx) = doc.get_dom_parent(node_idx) {
        if let Some(parent_style) = styles.style_for_node(parent_idx) {
            let parent_box_model = styles.box_model_style(parent_style).expect("builder-issued parent style handle");
            let mut style = WorkingStyle {
                font: styles.font_style(parent_style).expect("builder-issued parent style handle").clone(),
                text: styles.text_style(parent_style).expect("builder-issued parent style handle").clone(),
                box_model: initial_box_model(),
                border: initial_border(),
                radii: BorderRadii::default(),
                background: Background::default(),
                layout: LayoutStyle::default(),
                line_height_spec: None,
                generated_content: None,
                counters: CounterDirectives::default(),
            };
            inherit_box_model_properties(&mut style.box_model, parent_box_model);
            style
        } else {
            let mut style = WorkingStyle::default();
            style.font.font_size = doc.root_font_size();
            style
        }
    } else {
        // No parent - use root_font_size as the base font size.
        let mut style = WorkingStyle::default();
        style.font.font_size = doc.root_font_size();
        style
    };

    style
}

/// `rem` on the root element uses the initial root size. Once that element has
/// been computed, later elements use its computed font size. DOM style
/// resolution is parent-before-child, so the root handle is available here
/// without a second cascade pass.
fn root_font_size_for_resolution(doc: &Document, styles: &ComputedStylesBuilder) -> f32 {
    doc.dom_root().and_then(|root| styles.style_for_node(root)).and_then(|indices| styles.font_style(indices)).map(|font| font.font_size).filter(|size| size.is_finite() && *size >= 0.0).unwrap_or_else(|| doc.root_font_size())
}

/// Read-only document facade used while converting property values. It keeps
/// all existing document queries available through `Deref`, while making
/// every `rem` conversion in this stage observe the computed root size.
struct ResolutionDocument<'a> {
    document: &'a Document,
    root_font_size: f32,
}

impl ResolutionDocument<'_> {
    fn root_font_size(&self) -> f32 {
        self.root_font_size
    }
}

impl Deref for ResolutionDocument<'_> {
    type Target = Document;

    fn deref(&self) -> &Self::Target {
        self.document
    }
}

fn inherit_box_model_properties(style: &mut BoxModel, parent: &BoxModel) {
    style.border_collapse = parent.border_collapse;
    style.border_spacing_horizontal = parent.border_spacing_horizontal;
    style.border_spacing_vertical = parent.border_spacing_vertical;
    style.caption_side = parent.caption_side;
    style.empty_cells = parent.empty_cells;
}

/// Build ancestor filter for DOM nodes (optimization)
fn extend_ancestor_filter_dom(doc: &Document, node_idx: DomNodeId, filter: &mut AncestorFilter) {
    if let Some(tag) = doc.get_dom_tag(node_idx) {
        filter.insert(tag);
    }

    if let Some(id) = doc.get_dom_id(node_idx) {
        filter.insert(id);
    }

    for class in doc.get_dom_classes(node_idx) {
        filter.insert(class);
    }
}

fn selector_might_match_dom(selector: &lightningcss::selector::Selector, ancestor_filter: &AncestorFilter, _doc: &Document, _node_idx: DomNodeId) -> bool {
    use crate::style::selectors_dom::selector_might_match_with_filter;
    selector_might_match_with_filter(selector, ancestor_filter)
}

fn selector_matches_dom(selector: &lightningcss::selector::Selector, doc: &Document, node_idx: DomNodeId) -> bool {
    use crate::style::selectors_dom::selector_matches_dom_node;
    selector_matches_dom_node(selector, doc, node_idx)
}

fn inline_style_attribute(doc: &Document, node_idx: DomNodeId) -> Option<String> {
    let style_attr = doc.get_dom_attr(node_idx, "style")?;
    if style_attr.trim().is_empty() {
        return None;
    }
    Some(normalize_declarations(style_attr))
}

fn parse_inline_style_attribute<'a>(doc: &Document, node_idx: DomNodeId) -> Option<StyleAttribute<'a>> {
    let style_attr = inline_style_attribute(doc, node_idx)?;
    let leaked: &'a str = Box::leak(style_attr.into_boxed_str());
    StyleAttribute::parse(leaked, ParserOptions { error_recovery: true, ..ParserOptions::default() }).ok()
}

fn collect_inline_style_custom_properties<'a>(
    custom_properties: &mut FxHashMap<String, TokenList<'a>>, parent_custom: &FxHashMap<String, TokenList<'a>>, revert_basis: &FxHashMap<String, TokenList<'a>>, revert_layer_basis: &FxHashMap<String, TokenList<'a>>, style_attr: &str,
) {
    for declaration in style_attr.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if !name.starts_with("--") {
            continue;
        }

        let value = value.trim().strip_suffix("!important").unwrap_or(value.trim()).trim();
        let leaked_value: &'a str = Box::leak(value.to_string().into_boxed_str());
        let Ok(tokens) = TokenList::parse_string_with_options(leaked_value, ParserOptions::default()) else {
            continue;
        };

        if let Some(keyword) = single_ident_keyword(&tokens)
            && apply_custom_keyword(custom_properties, parent_custom, revert_basis, revert_layer_basis, name, keyword)
        {
            continue;
        }

        custom_properties.insert(name.to_string(), tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::MatchedRule;
    use crate::document::{BorderCollapseMode, BorderStyle, CaptionSide, Clear, Display, Document, ElementRef, EmptyCellsMode, Float, FontRelativeLength, LengthPct, PositionMode, PreferredSize, TextAlign, TextDirection, WhiteSpace};
    use crate::parser::DocumentFactory;

    #[test]
    fn matched_rule_hot_path_record_stays_compact() {
        assert!(std::mem::size_of::<MatchedRule>() <= 12);
    }

    #[test]
    fn inline_style_attribute_overrides_computed_style() {
        let html = "<html><body><p style=\"display: inline; color: red;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.box_model_style(style).expect("validated style handle").display, Display::Inline);
        assert_eq!(document.text_style(style).expect("validated style handle").color, 0xFF0000FF);
    }

    #[test]
    fn has_selector_participates_in_candidate_lookup_and_cascade() {
        let html = "<html><body><div id='match'><span></span></div><div id='miss'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("div { background: blue } :has(> span) { background: green }"));
        let background_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("element by id");
            document.background_style(document.style_for_node(node).expect("computed style")).expect("background").background_color
        };

        assert_eq!(background_for("match"), 0x008000FF);
        assert_eq!(background_for("miss"), 0x0000FFFF);
    }

    #[test]
    fn revert_layer_restores_the_previous_normal_layer() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = "@layer base, theme; @layer base { #target { background-color: green } } @layer theme { #target { background-color: red; background-color: revert-layer } }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn revert_restores_the_previous_cascade_origin() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline; display: revert }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Block);
    }

    #[test]
    fn inline_revert_discards_stylesheet_declarations_from_the_author_origin() {
        let html = "<html><body><div id='target' style='display: revert'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Block);
    }

    #[test]
    fn important_revert_uses_the_normal_previous_origin_boundary() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline !important; display: revert !important }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Block);
    }

    #[test]
    fn custom_property_revert_restores_the_inherited_origin_value() {
        let html = "<html><body style='--tone: green'><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { --tone: red; --tone: revert; background-color: var(--tone) }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn self_referential_custom_property_is_invalid_even_with_an_inner_fallback() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { --tone: var(--tone, red); background-color: var(--tone, green) }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn every_member_of_a_multi_property_cycle_becomes_invalid() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { --a: var(--b); --b: var(--a); background-color: var(--a, green) }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn acyclic_dependent_can_fallback_from_a_cyclic_property() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { --a: var(--a); --b: var(--a, green); background-color: var(--b) }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn child_declarations_do_not_revive_an_invalid_inherited_custom_property() {
        let html = "<html><body id='parent'><div id='target'></div></body></html>";
        let css = "#parent { --tone: var(--missing) } #target { --missing: red; background-color: var(--tone, green) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn custom_property_cycle_and_inheritance_wpt_cases_resolve_to_green() {
        let html = "<html><body><p id='multi'></p><section><p id='invalid-inherited'></p></section><article><p id='resolved-inherited'></p></article></body></html>";
        let css = "
            body { color: green }
            #multi { color: crimson; --a: red var(--b); --b: var(--c); --c: var(--d); --d: var(--e); --e: var(--a); --f: var(--e); color: var(--f) }
            section { --c: var(--missing) }
            #invalid-inherited { color: red; --a: var(--b); --b: var(--c, green); color: var(--a) }
            article { --c: var(--missing, green) }
            #resolved-inherited { color: red; --a: var(--b); --b: var(--c, crimson); color: var(--a) }
        ";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        for id in ["multi", "invalid-inherited", "resolved-inherited"] {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("target");
            let style = document.style_for_node(node).expect("computed style");
            assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF, "{id}");
        }
    }

    #[test]
    fn substituted_inherit_is_finalized_before_dependent_custom_properties() {
        let html = "<html><body id='parent'><div id='target'></div></body></html>";
        let css = "#parent { --a: green } #target { --a: var(--missing, inherit); --b: var(--a); background-color: var(--b) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn var_substitution_preserves_boundary_before_an_adjacent_identifier() {
        let html = "<html><body><p id='target'></p></body></html>";
        let css = "body { color: green } #target { color: red; --a: var(--b)red; --b: orange; color: var(--a) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF);
    }

    #[test]
    fn adjacent_var_substitutions_remain_distinct_tokens() {
        let html = "<html><body><p id='target'></p></body></html>";
        let css = "body { color: green } #target { color: red; --a: orange; --b: red; color: var(--a)var(--b) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF);
    }

    #[test]
    fn all_revert_restores_every_property_from_the_previous_origin() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline; width: 200px; background-color: red; all: revert }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.display, Display::Block);
        assert_eq!(box_style.width, PreferredSize::Auto);
        assert_eq!(document.background_style(style).expect("background").background_color, 0x00000000);
    }

    #[test]
    fn all_initial_resets_supported_properties_but_preserves_direction() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { direction: rtl; display: block; width: 200px; color: red; background-color: red; all: initial }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.display, Display::Inline);
        assert_eq!(box_style.width, PreferredSize::Auto);
        assert_eq!(document.text_style(style).expect("text style").direction, TextDirection::Rtl);
        assert_eq!(document.text_style(style).expect("text style").color, 0x000000FF);
        assert_eq!(document.background_style(style).expect("background").background_color, 0x00000000);
    }

    #[test]
    fn all_inherit_copies_reset_and_inherited_properties_from_the_parent() {
        let html = "<html><body><div id='parent'><div id='target'></div></div></body></html>";
        let css = "#parent { display: inline; width: 123px; color: green; background-color: green } #target { direction: rtl; all: inherit }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.display, Display::Inline);
        assert_eq!(box_style.width, PreferredSize::Px(123.0));
        assert_eq!(document.text_style(style).expect("text style").direction, TextDirection::Rtl);
        assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF);
        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn all_inherit_uses_the_dom_parent_across_inline_block_nesting() {
        let html = "<html><body><span id='parent'><div id='target'></div></span></body></html>";
        let css = "#parent { display: inline } #target { all: inherit }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Inline);
    }

    #[test]
    fn all_unset_inherits_inherited_properties_and_initializes_reset_properties() {
        let html = "<html><body><div id='parent'><div id='target'></div></div></body></html>";
        let css = "#parent { color: green; font-size: 20px; background-color: green } #target { display: block; width: 123px; background-color: red; all: unset }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.display, Display::Inline);
        assert_eq!(box_style.width, PreferredSize::Auto);
        assert_eq!(document.font_style(style).expect("font style").font_size, 20.0);
        assert_eq!(document.text_style(style).expect("text style").color, 0x008000FF);
        assert_eq!(document.background_style(style).expect("background").background_color, 0x00000000);
    }

    #[test]
    fn unlayered_revert_layer_falls_back_to_the_previous_origin() {
        let html = "<html><body><div id='target'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { display: inline; display: revert-layer }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box style").display, Display::Block);
    }

    #[test]
    fn important_revert_layer_uses_the_corresponding_normal_layer_boundary() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = "@layer { #target { background-color: green } } @layer { #target { background-color: red !important; background-color: revert-layer !important } } @layer { #target { background-color: red !important } }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn inline_revert_layer_restores_stylesheet_declarations() {
        let html = "<html><body><div id='target' style='background-color: red; background-color: revert-layer'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#target { background-color: green }"));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn all_revert_layer_restores_every_property_from_the_previous_layer() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = "@layer { #target { width: 100px; height: 100px; background-color: green } } @layer { #target { width: 200px; height: 200px; background-color: red } #target { all: revert-layer } }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");
        let box_style = document.box_model_style(style).expect("box style");

        assert_eq!(box_style.width, PreferredSize::Px(100.0));
        assert_eq!(box_style.height, PreferredSize::Px(100.0));
        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn custom_property_revert_layer_restores_the_previous_token_value() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = "@layer base, theme; @layer base { #target { --tone: green } } @layer theme { #target { --tone: red; --tone: revert-layer } } #target { background-color: var(--tone) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.background_style(style).expect("background").background_color, 0x008000FF);
    }

    #[test]
    fn open_type_controls_resolve_to_normalized_feature_tags() {
        let html = r#"<html><body><p style='font-variant-caps: all-small-caps;
            font-variant-numeric: oldstyle-nums tabular-nums;
            font-variant-ligatures: no-common-ligatures discretionary-ligatures;
            font-kerning: none;
            font-feature-settings: "liga" on, "ss03" 2'>office 123</p></body></html>"#;
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("paragraph");
        let indices = document.style_for_node(node).expect("computed style");
        let font = document.font_style(indices).expect("font style");
        let features = font.open_type_features();
        let value = |tag| features.iter().find(|feature| feature.tag() == tag).map(|feature| feature.value());

        assert_eq!(value(*b"smcp"), Some(1));
        assert_eq!(value(*b"c2sc"), Some(1));
        assert_eq!(value(*b"onum"), Some(1));
        assert_eq!(value(*b"tnum"), Some(1));
        assert_eq!(value(*b"dlig"), Some(1));
        assert_eq!(value(*b"kern"), Some(0));
        assert_eq!(value(*b"ss03"), Some(2));
        assert_eq!(value(*b"liga"), Some(1), "explicit feature settings override the variant longhand");
    }

    #[test]
    fn inherited_font_feature_settings_reach_inline_descendants() {
        let html = r#"<html><body><div><span id="target">A</span>SS</div></body></html>"#;
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(r#"html { font-kerning: none; font-feature-settings: "kern" off; }"#));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target span");
        let indices = document.style_for_node(node).expect("computed style");
        let features = document.font_style(indices).expect("font style").open_type_features();

        assert_eq!(features.iter().find(|feature| feature.tag() == *b"kern").map(|feature| feature.value()), Some(0));
    }

    #[test]
    fn inline_block_is_preserved_as_an_atomic_flow_root() {
        let html = "<html><body><div style='display:inline-block'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("inline-block element");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box model").display, Display::InlineBlock);
    }

    #[test]
    fn block_flow_root_is_preserved_as_a_distinct_formatting_context() {
        let html = "<html><body><div style='display:flow-root'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("flow-root element");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box model").display, Display::FlowRoot);
    }

    #[test]
    fn flow_root_list_item_preserves_both_inner_display_and_marker_role() {
        let html = "<html><body><div style='display:flow-root list-item'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("flow-root list item");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(style).expect("box model").display, Display::FlowRootListItem);
    }

    #[test]
    fn inline_style_recovers_after_an_invalid_declaration() {
        let html = "<html><body><div style='grid; display:grid; width:300px; height:200px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let node = find_body(document.document()).children().next().expect("grid should be child of body");
        let indices = document.style_for_node(node).expect("grid style should exist");
        let box_model = document.box_model_style(indices).expect("validated box style");

        assert_eq!(box_model.display, Display::Grid);
        assert_eq!(box_model.width, PreferredSize::Px(300.0));
        assert_eq!(box_model.height, PreferredSize::Px(200.0));
    }

    #[test]
    fn invalid_numeric_longhands_keep_the_previous_cascaded_value() {
        let html = "<html><body><div style='font-size:18px;font-size:-1px;font-weight:400;font-weight:9000;border-bottom:7px solid;border-bottom-width:-10px;line-height:24px;line-height:-2'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let indices = document.style_for_node(node).expect("computed style");

        let font = document.font_style(indices).expect("font style");
        let text = document.text_style(indices).expect("text style");
        let border = document.border_style(indices).expect("border style");
        assert_eq!(font.font_size, 18.0);
        assert_eq!(font.font_weight, 400);
        assert_eq!(text.line_height, 24.0);
        assert_eq!(border.border_bottom_width, FontRelativeLength::px(7.0).unwrap());
    }

    #[test]
    fn invalid_border_width_shorthand_is_atomic() {
        let html = "<html><body><div style='border-width:1px 2px 3px 4px;border-width:5px -1px 7px 8px;border-style:solid'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let indices = document.style_for_node(node).expect("computed style");
        let border = document.border_style(indices).expect("border style");

        assert_eq!(
            (border.border_top_width, border.border_right_width, border.border_bottom_width, border.border_left_width),
            (FontRelativeLength::px(1.0).unwrap(), FontRelativeLength::px(2.0).unwrap(), FontRelativeLength::px(3.0).unwrap(), FontRelativeLength::px(4.0).unwrap())
        );
    }

    #[test]
    fn border_none_preserves_computed_width_for_inheritance() {
        let html = "<html><body><div style='border:16px solid blue;border-top:none'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let border = document.border_style(document.style_for_node(node).expect("computed style")).expect("border style");

        assert_eq!(border.border_top_style, BorderStyle::None);
        assert_eq!(border.border_top_width, FontRelativeLength::px(3.0).unwrap());
        assert_eq!(border.border_right_width, FontRelativeLength::px(16.0).unwrap());
    }

    #[test]
    fn border_inherit_copies_the_parents_computed_current_color() {
        let html = "<html><body><p style='color:black;border:medium solid'><em style='color:red;border:inherit'></em></p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let paragraph = find_body(document.document()).children().next().expect("paragraph");
        let emphasis = document.document().element_ref(paragraph).expect("paragraph element").children().find(|&child| document.document().element_ref(child).is_some_and(|element| element.tag() == "em")).expect("emphasis element");
        let indices = document.style_for_node(emphasis).expect("computed emphasis style");
        let text = document.text_style(indices).expect("text style");
        let border = document.border_style(indices).expect("border style");

        assert_eq!(text.color, 0xff0000ff);
        assert_eq!(border.border_top_color, 0x000000ff);
        assert_eq!(border.border_right_color, 0x000000ff);
        assert_eq!(border.border_bottom_color, 0x000000ff);
        assert_eq!(border.border_left_color, 0x000000ff);
    }

    #[test]
    fn border_shorthand_components_survive_a_later_style_longhand() {
        let html = "<html><body><div style='border:1in blue;border-style:solid'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let border = document.border_style(document.style_for_node(node).expect("computed style")).expect("border style");

        assert_eq!(border.border_top_width, FontRelativeLength::px(96.0).unwrap());
        assert_eq!(border.border_top_style, BorderStyle::Solid);
        assert_eq!(border.border_top_color, 0x0000ffff);
    }

    #[test]
    fn negative_physical_padding_is_ignored_and_shorthands_are_atomic() {
        let html = "<html><body><div style='padding:1px 2px 3px 4px;padding:5px -1px 7px 8px'></div><div style='padding-top:9px;padding-top:-1px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let nodes = find_body(document.document()).children().collect::<Vec<_>>();
        let shorthand = document.box_model_style(document.style_for_node(nodes[0]).expect("computed style")).expect("box model");
        let longhand = document.box_model_style(document.style_for_node(nodes[1]).expect("computed style")).expect("box model");

        assert_eq!((shorthand.padding_top, shorthand.padding_right, shorthand.padding_bottom, shorthand.padding_left), (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(4.0)));
        assert_eq!(longhand.padding_top, LengthPct::Px(9.0));
    }

    #[test]
    fn margin_shorthand_expands_one_through_four_values() {
        let html = "<html><body><div style='margin:1px'></div><div style='margin:1px 2px'></div><div style='margin:1px 2px 3px'></div><div style='margin:1px 2px 3px 4px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let nodes = find_body(document.document()).children().collect::<Vec<_>>();
        let sides = nodes
            .iter()
            .map(|&node| {
                let style = document.style_for_node(node).expect("computed margin style");
                let box_model = document.box_model_style(style).expect("box model");
                (box_model.margin_top, box_model.margin_right, box_model.margin_bottom, box_model.margin_left)
            })
            .collect::<Vec<_>>();

        assert_eq!(sides[0], (LengthPct::Px(1.0), LengthPct::Px(1.0), LengthPct::Px(1.0), LengthPct::Px(1.0)));
        assert_eq!(sides[1], (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(1.0), LengthPct::Px(2.0)));
        assert_eq!(sides[2], (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(2.0)));
        assert_eq!(sides[3], (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(4.0)));
    }

    #[test]
    fn padding_shorthand_expands_and_inherits_one_through_four_values() {
        let html = "<html><body><div style='padding:1px'><span style='padding:inherit'></span></div><div style='padding:1px 2px'><span style='padding:inherit'></span></div><div style='padding:1px 2px 3px'><span style='padding:inherit'></span></div><div style='padding:1px 2px 3px 4px'><span style='padding:inherit'></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let parents = find_body(document.document()).children().collect::<Vec<_>>();
        let expected = [
            (LengthPct::Px(1.0), LengthPct::Px(1.0), LengthPct::Px(1.0), LengthPct::Px(1.0)),
            (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(1.0), LengthPct::Px(2.0)),
            (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(2.0)),
            (LengthPct::Px(1.0), LengthPct::Px(2.0), LengthPct::Px(3.0), LengthPct::Px(4.0)),
        ];

        for (parent, expected_sides) in parents.into_iter().zip(expected) {
            let child = document.document().element_ref(parent).expect("padding parent").children().next().expect("padding child");
            for node in [parent, child] {
                let indices = document.style_for_node(node).expect("computed padding style");
                let box_model = document.box_model_style(indices).expect("box model");
                assert_eq!((box_model.padding_top, box_model.padding_right, box_model.padding_bottom, box_model.padding_left), expected_sides);
            }
        }
    }

    #[test]
    fn relative_position_and_physical_insets_cross_as_typed_values() {
        let html = "<html><body><div style='position:relative;position:absolute;top:11px;right:auto;bottom:3%;left:-7px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("positioned element");
        let indices = document.style_for_node(node).expect("computed positioned style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(layout.position, PositionMode::Absolute);
        assert_eq!(layout.inset_top, Some(LengthPct::Px(11.0)));
        assert_eq!(layout.inset_right, None);
        assert_eq!(layout.inset_bottom, Some(LengthPct::Pct(0.03)));
        assert_eq!(layout.inset_left, Some(LengthPct::Px(-7.0)));
    }

    #[test]
    fn image_dimension_attributes_are_presentational_hints_below_author_css() {
        let html = "<html><body><img id='percentage' src='x' width='50%' height='25%'><img id='pixels' src='x' width='80' height='3'><img id='overridden' src='x' width='50%' style='width:75%'><svg id='svg' width='40' height='50%'></svg></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("image by id");
            document.box_model_style(document.style_for_node(node).expect("computed image style")).expect("box model")
        };

        let percentage = style_for("percentage");
        assert_eq!(percentage.width, PreferredSize::Percent(0.5));
        assert_eq!(percentage.height, PreferredSize::Percent(0.25));
        let pixels = style_for("pixels");
        assert_eq!(pixels.width, PreferredSize::Px(80.0));
        assert_eq!(pixels.height, PreferredSize::Px(3.0));
        assert_eq!(style_for("overridden").width, PreferredSize::Percent(0.75));
        assert_eq!(style_for("svg").width, PreferredSize::Px(40.0));
        assert_eq!(style_for("svg").height, PreferredSize::Percent(0.5));
    }

    #[test]
    fn image_spacing_attributes_preserve_pixel_and_percentage_margins() {
        let html = "<html><body><img id='hinted' src='x' hspace='10%' vspace='7'><img id='overridden' src='x' hspace='10%' style='margin-left:3px'></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("image by id");
            document.box_model_style(document.style_for_node(node).expect("computed image style")).expect("box model")
        };

        let hinted = style_for("hinted");
        assert_eq!((hinted.margin_left, hinted.margin_right), (LengthPct::Pct(0.1), LengthPct::Pct(0.1)));
        assert_eq!((hinted.margin_top, hinted.margin_bottom), (LengthPct::Px(7.0), LengthPct::Px(7.0)));
        let overridden = style_for("overridden");
        assert_eq!(overridden.margin_left, LengthPct::Px(3.0));
        assert_eq!(overridden.margin_right, LengthPct::Pct(0.1));
    }

    #[test]
    fn hidden_attribute_is_a_cascading_html_presentational_hint() {
        let html = "<html><body><div id='hidden' hidden>A</div><div id='overridden' hidden style='display:block'>B</div><div id='until-found' hidden='until-found'>C</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let display_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("element by id");
            document.box_model_style(document.style_for_node(node).expect("computed style")).expect("box model").display
        };

        assert_eq!(display_for("hidden"), Display::None);
        assert_eq!(display_for("overridden"), Display::Block, "author CSS must override the presentational hint");
        assert_eq!(display_for("until-found"), Display::Block, "hidden-until-found retains its principal box");
    }

    #[test]
    fn html_presentational_hints_do_not_leak_into_foreign_xml_elements() {
        let parsed = html_parse::parse_xml_document("<root><div id='target' hidden=''/></root>").expect("valid XML");
        let document = crate::style_document(parsed.build_dom(), &[]);
        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target element");
        let style = document.box_model_style(document.style_for_node(target).expect("computed style")).expect("box model");

        assert_eq!(style.display, Display::Block);
    }

    #[test]
    fn legacy_html_color_attributes_are_presentational_hints_below_author_css() {
        let html = "<html><body id='body' text='green' bgcolor='#ffff00'><table><tr><td id='cell' bgcolor='red' style='background-color:blue'>Cell</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("body { color: #123456; }"));
        let node_by_id = |id| document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");

        let body_style = document.style_for_node(node_by_id("body")).expect("computed body style");
        assert_eq!(document.text_style(body_style).expect("body text style").color, 0x123456FF, "author CSS should override the text hint");
        assert_eq!(document.background_style(body_style).expect("body background style").background_color, 0xFFFF00FF);

        let cell_style = document.style_for_node(node_by_id("cell")).expect("computed cell style");
        assert_eq!(document.background_style(cell_style).expect("cell background style").background_color, 0x0000FFFF, "author CSS should override the bgcolor hint");
    }

    #[test]
    fn legacy_table_spacing_and_padding_are_presentational_hints_below_author_css() {
        let html = "<html><body><table id='hinted' cellspacing='0' cellpadding='0'><tr><td id='hinted-cell'>A</td></tr></table><table id='overridden' cellspacing='7' cellpadding='9' style='border-spacing:3px'><tr><td id='overridden-cell' style='padding:4px'>B</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let box_style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
            document.box_model_style(document.style_for_node(node).expect("computed style")).expect("box model")
        };

        let hinted = box_style_for("hinted");
        assert_eq!((hinted.border_spacing_horizontal, hinted.border_spacing_vertical), (0.0, 0.0));
        let hinted_cell = box_style_for("hinted-cell");
        assert_eq!((hinted_cell.padding_top, hinted_cell.padding_right, hinted_cell.padding_bottom, hinted_cell.padding_left), (LengthPct::Px(0.0), LengthPct::Px(0.0), LengthPct::Px(0.0), LengthPct::Px(0.0)));

        let overridden = box_style_for("overridden");
        assert_eq!((overridden.border_spacing_horizontal, overridden.border_spacing_vertical), (3.0, 3.0));
        let overridden_cell = box_style_for("overridden-cell");
        assert_eq!((overridden_cell.padding_top, overridden_cell.padding_right, overridden_cell.padding_bottom, overridden_cell.padding_left), (LengthPct::Px(4.0), LengthPct::Px(4.0), LengthPct::Px(4.0), LengthPct::Px(4.0)));
    }

    #[test]
    fn legacy_table_rules_and_frame_values_are_ascii_case_insensitive() {
        let html = "<html><body><table id='lower-rules' rules='rows'><tr id='lower-row'><td>X</td></tr></table><table id='upper-rules' rules='RoWs'><tr id='upper-row'><td>X</td></tr></table><table id='invalid-rules' rules='rowſ'><tr id='invalid-row'><td>X</td></tr></table><table id='lower-frame' frame='hsides'><tr><td>X</td></tr></table><table id='upper-frame' frame='HsIdEs'><tr><td>X</td></tr></table><table id='invalid-frame' frame='hſideſ'><tr><td>X</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
            document.style_for_node(node).expect("computed style")
        };

        let lower_row = document.border_style(style_for("lower-row")).expect("row border");
        let upper_row = document.border_style(style_for("upper-row")).expect("row border");
        let invalid_row = document.border_style(style_for("invalid-row")).expect("row border");
        assert_eq!(lower_row, upper_row);
        assert_ne!(lower_row, invalid_row);

        let lower_frame = document.border_style(style_for("lower-frame")).expect("table border");
        let upper_frame = document.border_style(style_for("upper-frame")).expect("table border");
        let invalid_frame = document.border_style(style_for("invalid-frame")).expect("table border");
        assert_eq!(lower_frame, upper_frame);
        assert_ne!(lower_frame, invalid_frame);
    }

    #[test]
    fn legacy_cell_nowrap_is_a_presentational_hint_below_author_css() {
        let html = "<html><body><table><tr><td id='hinted' nowrap>text</td><td id='overridden' nowrap style='white-space:normal'>text</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let white_space_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
            document.text_style(document.style_for_node(node).expect("computed style")).expect("text style").white_space
        };

        assert_eq!(white_space_for("hinted"), WhiteSpace::NoWrap);
        assert_eq!(white_space_for("overridden"), WhiteSpace::Normal);
    }

    #[test]
    fn inherited_table_model_properties_cross_the_computed_style_boundary() {
        let html = "<html><body><table id='table' style='border-collapse:collapse;border-spacing:7px 9px;caption-side:bottom;empty-cells:hide'><tbody><tr><td id='cell'>A</td></tr></tbody></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let by_id = |id| document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
        let table_style = document.box_model_style(document.style_for_node(by_id("table")).expect("computed table style")).expect("table box model");
        assert_eq!(table_style.border_collapse, BorderCollapseMode::Collapse);
        let cell = by_id("cell");
        let style = document.box_model_style(document.style_for_node(cell).expect("computed cell style")).expect("cell box model");

        assert_eq!(style.border_collapse, BorderCollapseMode::Collapse);
        assert_eq!((style.border_spacing_horizontal, style.border_spacing_vertical), (7.0, 9.0));
        assert_eq!(style.caption_side, CaptionSide::Bottom);
        assert_eq!(style.empty_cells, EmptyCellsMode::Hide);
    }

    #[test]
    fn custom_represented_table_keywords_obey_the_normal_cascade() {
        let html = "<html><body><table id='important' style='border-collapse:separate'><tr><td>A</td></tr></table><table id='ordered' style='border-collapse:collapse;border-collapse:separate'><tr><td>B</td></tr></table><table id='variable' style='--mode:collapse;border-collapse:var(--mode)'><tr><td>C</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("#important { border-collapse: collapse !important; }"));
        let by_id = |id| document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("table by id");
        let collapse = |id| document.box_model_style(document.style_for_node(by_id(id)).expect("computed table style")).expect("table box model").border_collapse;

        assert_eq!(collapse("important"), BorderCollapseMode::Collapse);
        assert_eq!(collapse("ordered"), BorderCollapseMode::Separate);
        assert_eq!(collapse("variable"), BorderCollapseMode::Collapse);
    }

    #[test]
    fn absolute_positioning_blockifies_table_internal_displays() {
        let displays = ["table-row-group", "table-header-group", "table-footer-group", "table-row", "table-column-group", "table-column", "table-cell", "table-caption"];
        let children = displays.iter().enumerate().map(|(index, display)| format!("<div id='d{index}' style='display:{display};position:absolute'></div>")).collect::<String>();
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(&format!("<html><body>{children}</body></html>"), None);

        for index in 0..displays.len() {
            let id = format!("d{index}");
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id.as_str())).expect("positioned table-internal display");
            let box_model = document.box_model_style(document.style_for_node(node).expect("computed positioned style")).expect("box model");
            assert_eq!(box_model.display, Display::Block, "{} must blockify", displays[index]);
        }
    }

    #[test]
    fn floating_blockifies_inline_and_table_internal_displays() {
        let displays = ["inline", "inline-block", "table-row-group", "table-header-group", "table-footer-group", "table-row", "table-column-group", "table-column", "table-cell", "table-caption"];
        let children = displays.iter().enumerate().map(|(index, display)| format!("<div id='d{index}' style='display:{display};float:left'></div>")).collect::<String>();
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(&format!("<html><body>{children}</body></html>"), None);

        for index in 0..displays.len() {
            let id = format!("d{index}");
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id.as_str())).expect("floated display");
            let box_model = document.box_model_style(document.style_for_node(node).expect("computed floated style")).expect("box model");
            assert_eq!(box_model.display, Display::Block, "{} must blockify", displays[index]);
        }
    }

    #[test]
    fn z_index_crosses_the_typed_style_boundary() {
        let html = "<html><body><div id='integer' style='position:relative;z-index:-7'></div><div id='auto' style='position:absolute;z-index:auto'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let z_index_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("element by id");
            document.styles().layout_style(document.style_for_node(node).expect("computed style")).expect("layout style").z_index
        };
        assert_eq!(z_index_for("integer"), Some(-7));
        assert_eq!(z_index_for("auto"), None);
    }

    #[test]
    fn physical_padding_inherit_copies_the_parent_computed_value() {
        let html = "<html><body><div id='parent' style='padding-top:1in'><div id='child' style='padding-top:inherit'></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        for id in ["parent", "child"] {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("padding test element");
            let indices = document.style_for_node(node).expect("computed padding style");
            assert_eq!(document.box_model_style(indices).expect("box style").padding_top, LengthPct::Px(96.0));
        }
    }

    #[test]
    fn inherited_em_and_percentage_padding_keep_their_computed_semantics() {
        let html = "<html><body><div id='em-parent' style='font-size:24px;padding:2em 3em 1em 4em'><div id='em-child' style='font-size:40px;padding:inherit'></div></div><div id='pct-parent' style='padding:20%'><div id='pct-child' style='padding:inherit'></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let box_model = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("padding test element");
            document.box_model_style(document.style_for_node(node).expect("computed padding style")).expect("box style")
        };

        let expected_em = (LengthPct::Px(48.0), LengthPct::Px(72.0), LengthPct::Px(24.0), LengthPct::Px(96.0));
        for id in ["em-parent", "em-child"] {
            let style = box_model(id);
            assert_eq!((style.padding_top, style.padding_right, style.padding_bottom, style.padding_left), expected_em);
        }
        for id in ["pct-parent", "pct-child"] {
            let style = box_model(id);
            assert_eq!((style.padding_top, style.padding_right, style.padding_bottom, style.padding_left), (LengthPct::Pct(0.2), LengthPct::Pct(0.2), LengthPct::Pct(0.2), LengthPct::Pct(0.2)));
        }
    }

    #[test]
    fn invalid_font_shorthand_is_atomic() {
        let html = "<html><body><div style='font:italic 700 20px/30px serif;font:4em/-2em monospace'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("test element");
        let indices = document.style_for_node(node).expect("computed style");
        let font = document.font_style(indices).expect("font style");
        let text = document.text_style(indices).expect("text style");

        assert_eq!(font.font_size, 20.0);
        assert_eq!(font.font_weight, 700);
        assert_eq!(font.font_style, crate::document::FontStyle::Italic);
        assert_eq!(text.line_height, 30.0);
        let family = font.font_family.and_then(|id| document.styles().string(id)).expect("font family");
        assert!(family.contains("serif"));
    }

    #[test]
    fn italic_and_oblique_remain_distinct_computed_font_styles() {
        let html = "<html><body><span id='italic' style='font-style:italic'></span><span id='oblique' style='font-style:oblique 12deg'></span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let font_style_for = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("font style test element");
            document.font_style(document.style_for_node(node).expect("computed style")).expect("font style").font_style
        };

        assert_eq!(font_style_for("italic"), crate::document::FontStyle::Italic);
        assert_eq!(font_style_for("oblique"), crate::document::FontStyle::Oblique);
    }

    #[test]
    fn positive_infinite_flex_factor_is_normalized_before_storage() {
        let html = "<html><body><div style='display:flex'><div style='flex:calc(infinity) 0 0px'></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let container = find_body(document.document()).children().next().expect("flex container");
        let child = document.document().element_ref(container).expect("container element").children().next().expect("flex item");
        let indices = document.style_for_node(child).expect("computed style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert!(layout.flex_grow.is_finite());
        assert_eq!(layout.flex_grow, super::MAX_COMPUTED_FLEX_FACTOR);
    }

    #[test]
    fn invalid_var_substitution_uses_unset_instead_of_the_losing_declaration() {
        let html = "<html><body style='font-size:22px'><div style='--bad:-1px;font-size:30px;font-size:var(--bad)'></div><div style='font-size:31px;font-size:var(--missing)'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let sizes = find_body(document.document())
            .children()
            .map(|node| {
                let indices = document.style_for_node(node).expect("computed style");
                document.font_style(indices).expect("font style").font_size
            })
            .collect::<Vec<_>>();

        assert_eq!(sizes, vec![22.0, 22.0]);
    }

    #[test]
    fn custom_property_wide_keywords_resolve_before_var_fallbacks() {
        let html = "<html><body>
            <div id='initial' style='color:orange;--tone:initial;color:var(--tone,green)'></div>
            <div style='--tone:green'><div id='inherit' style='color:orange;--tone:inherit;color:var(--tone)'></div></div>
            <div style='--tone:green'><div id='unset' style='color:orange;--tone:unset;color:var(--tone)'></div></div>
            <div style='--tone:green'><div style='--tone:var(--missing,unset)'><div id='fallback' style='color:var(--tone)'></div></div></div>
        </body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let color = |id| {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("element by id");
            document.text_style(document.style_for_node(node).expect("computed style")).expect("text style").color
        };

        assert_eq!(color("initial"), 0x008000FF);
        assert_eq!(color("inherit"), 0x008000FF);
        assert_eq!(color("unset"), 0x008000FF);
        assert_eq!(color("fallback"), 0x008000FF);
    }

    #[test]
    fn css_wide_keywords_from_var_fallbacks_keep_their_semantics() {
        let html = "<html><body><div id='outer'><div id='inner'></div></div></body></html>";
        let css = "#outer { color: transparent; border-style: solid } #inner { color: var(--missing, initial); border-style: var(--missing, inherit) }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("inner")).expect("inner");
        let style = document.style_for_node(node).expect("computed style");

        assert_eq!(document.text_style(style).expect("text style").color, 0x000000FF);
        assert_eq!(document.border_style(style).expect("border style").border_top_style, BorderStyle::Solid);
    }

    #[test]
    fn first_line_and_first_letter_styles_are_resolved_separately() {
        let mut factory = DocumentFactory::new();
        let document = factory
            .parse_with_new_pipeline("<html><body style='line-height:20px'><div id='line'>Line</div><div id='letter'>Letter</div></body></html>", Some("#line::first-line { line-height:100px } #letter::first-letter { line-height:80px }"));
        let children = find_body(document.document()).children().collect::<Vec<_>>();
        let [line, letter] = children.as_slice() else { panic!("expected two body children") };
        let line_style = document.styles().first_line_style_for_node(*line).expect("first-line style");
        let letter_style = document.styles().first_letter_style_for_node(*letter).expect("first-letter style");

        assert_eq!(document.text_style(line_style).expect("first-line text style").line_height, 100.0);
        assert_eq!(document.text_style(letter_style).expect("first-letter text style").line_height, 80.0);
        assert!(document.styles().first_letter_style_for_node(*line).is_none());
        assert!(document.styles().first_line_style_for_node(*letter).is_none());
    }

    #[test]
    fn first_letter_retains_the_inheritance_path_of_generated_before_text() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><div style='color:red'></div></body></html>", Some("div::first-line { color:blue } div::before { content:'Filler'; color:green } div::first-letter { font-size:98px }"));
        let div = find_body(document.document()).children().next().expect("generated-content origin");
        let ordinary = document.styles().first_letter_style_for_node(div).expect("ordinary first-letter style");
        let generated = document.styles().before_first_letter_style_for_node(div).expect("generated first-letter style");

        assert_eq!(document.text_style(ordinary).expect("ordinary first-letter text style").color, 0x0000FFFF);
        assert_eq!(document.text_style(generated).expect("generated first-letter text style").color, 0x008000FF);
        assert_eq!(document.font_style(generated).expect("generated first-letter font style").font_size, 98.0);
    }

    #[test]
    fn horizontal_rtl_logical_properties_resolve_to_physical_style_values() {
        let html = "<html><body><div style='direction:rtl;inline-size:120px;block-size:30px;margin-inline:10px 20px;padding-inline:3px 4px;border-inline-width:5px 7px;border-inline-style:solid dashed;border-inline-color:red blue;text-align:start;float:inline-start;clear:inline-end'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("logical box");
        let indices = document.style_for_node(node).expect("computed logical style");
        let text = document.text_style(indices).expect("text style");
        let box_model = document.box_model_style(indices).expect("box style");
        let border = document.border_style(indices).expect("border style");

        assert_eq!(text.direction, TextDirection::Rtl);
        assert_eq!(text.text_align, TextAlign::Right);
        assert_eq!(box_model.width, PreferredSize::Px(120.0));
        assert_eq!(box_model.height, PreferredSize::Px(30.0));
        assert_eq!((box_model.margin_right, box_model.margin_left), (LengthPct::Px(10.0), LengthPct::Px(20.0)));
        assert_eq!((box_model.padding_right, box_model.padding_left), (LengthPct::Px(3.0), LengthPct::Px(4.0)));
        assert_eq!((border.border_right_width, border.border_left_width), (FontRelativeLength::px(5.0).unwrap(), FontRelativeLength::px(7.0).unwrap()));
        assert_eq!((border.border_right_style, border.border_left_style), (BorderStyle::Solid, BorderStyle::Dashed));
        assert_eq!((border.border_right_color, border.border_left_color), (0xFF0000FF, 0x0000FFFF));
        assert_eq!(box_model.float, Float::Right);
        assert_eq!(box_model.clear, Clear::Left);
    }

    #[test]
    fn logical_text_alignment_re_resolves_when_descendant_direction_changes() {
        let html = "<html><body dir='rtl' id='initial'><div style='text-align:start'><span dir='ltr' id='logical'></span></div><div style='text-align:left'><span dir='rtl' id='physical'></span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let alignment = |id| {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("alignment test element");
            document.text_style(document.style_for_node(node).expect("computed style")).expect("text style").text_align
        };

        assert_eq!(alignment("initial"), TextAlign::Right, "the initial logical start follows an HTML rtl direction hint");
        assert_eq!(alignment("logical"), TextAlign::Left, "inherited text-align:start re-resolves against the descendant direction");
        assert_eq!(alignment("physical"), TextAlign::Left, "an explicitly physical inherited alignment does not flip");
    }

    #[test]
    fn intrinsic_size_keywords_reach_computed_box_style() {
        let html = "<html><body><div style='width:min-content;min-width:max-content;max-width:fit-content'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("intrinsically sized box");
        let indices = document.style_for_node(node).expect("computed box style");
        let box_model = document.box_model_style(indices).expect("validated box style");

        assert_eq!(box_model.width, PreferredSize::MinContent);
        assert_eq!(box_model.min_width, PreferredSize::MaxContent);
        assert_eq!(box_model.max_width, PreferredSize::FitContent);
    }

    #[test]
    fn stretch_size_keyword_reaches_computed_box_style() {
        let html = "<html><body><div style='width:stretch;height:-webkit-fill-available;min-height:stretch;max-height:stretch'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("stretched box");
        let indices = document.style_for_node(node).expect("computed box style");
        let box_model = document.box_model_style(indices).expect("validated box style");

        assert_eq!(box_model.width, PreferredSize::Stretch);
        assert_eq!(box_model.height, PreferredSize::Stretch);
        assert_eq!(box_model.min_height, PreferredSize::Stretch);
        assert_eq!(box_model.max_height, PreferredSize::Stretch);
    }

    #[test]
    fn border_radius_crosses_style_boundary_as_physical_length_percentages() {
        let html = "<html><body><div style='direction:rtl;border-radius:10px 20% / 30px 40%;border-start-start-radius:7px 9px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("rounded box");
        let indices = document.style_for_node(node).expect("computed rounded style");
        let radii = *document.border_radii_style(indices).expect("radius style");

        // In horizontal RTL, start-start is the physical top-right corner.
        assert_eq!(radii.top_left.x, LengthPct::Px(10.0));
        assert_eq!(radii.top_left.y, LengthPct::Px(30.0));
        assert_eq!(radii.top_right.x, LengthPct::Px(7.0));
        assert_eq!(radii.top_right.y, LengthPct::Px(9.0));
        assert_eq!(radii.bottom_right.x, LengthPct::Px(10.0));
        assert_eq!(radii.bottom_right.y, LengthPct::Px(30.0));
        assert_eq!(radii.bottom_left.x, LengthPct::Pct(0.2));
        assert_eq!(radii.bottom_left.y, LengthPct::Pct(0.4));
    }

    #[test]
    fn logical_and_physical_declarations_share_one_cascade_order() {
        let html = "<html><body><div style='direction:rtl;margin-right:1px;margin-inline-start:2px;width:10px;inline-size:20px'></div><div style='direction:rtl;margin-inline-start:2px;margin-right:1px;inline-size:20px;width:10px'></div><div style='margin-inline-start:12px;direction:rtl'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let boxes = find_body(document.document())
            .children()
            .map(|node| {
                let indices = document.style_for_node(node).expect("computed style");
                document.box_model_style(indices).expect("box style").clone()
            })
            .collect::<Vec<_>>();

        assert_eq!(boxes[0].margin_right, LengthPct::Px(2.0));
        assert_eq!(boxes[0].width, PreferredSize::Px(20.0));
        assert_eq!(boxes[1].margin_right, LengthPct::Px(1.0));
        assert_eq!(boxes[1].width, PreferredSize::Px(10.0));
        assert_eq!(boxes[2].margin_right, LengthPct::Px(12.0));
        assert_eq!(boxes[2].margin_left, LengthPct::Px(0.0));
    }

    #[test]
    fn html_dir_and_css_direction_inherit_for_logical_resolution() {
        let html = "<html><body><section dir='RTL'><div style='margin-inline-start:9px'></div></section><section style='direction:rtl'><div style='padding-inline-end:7px'></div></section></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let values = find_body(document.document())
            .children()
            .map(|parent| {
                let child = document.document().element_ref(parent).expect("section").children().next().expect("child");
                let indices = document.style_for_node(child).expect("computed child style");
                let text = document.text_style(indices).expect("text style");
                let box_model = document.box_model_style(indices).expect("box style");
                (text.direction, box_model.margin_right, box_model.padding_left)
            })
            .collect::<Vec<_>>();

        assert_eq!(values[0], (TextDirection::Rtl, LengthPct::Px(9.0), LengthPct::Px(0.0)));
        assert_eq!(values[1], (TextDirection::Rtl, LengthPct::Px(0.0), LengthPct::Px(7.0)));
    }

    #[test]
    fn invalid_typed_logical_lengths_preserve_valid_fallbacks() {
        let html = "<html><body><div style='direction:rtl;inline-size:40px;inline-size:-10px;padding-inline-start:6px;padding-inline-start:-2px;border-inline-start:3px solid;border-inline-start-width:-4px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("logical box");
        let indices = document.style_for_node(node).expect("computed logical style");
        let box_model = document.box_model_style(indices).expect("box style");
        let border = document.border_style(indices).expect("border style");

        assert_eq!(box_model.width, PreferredSize::Px(40.0));
        assert_eq!(box_model.padding_right, LengthPct::Px(6.0));
        assert_eq!(border.border_right_width, FontRelativeLength::px(3.0).unwrap());
    }

    #[test]
    fn flex_properties_cross_the_style_boundary_as_renderer_owned_values() {
        use crate::document::{ContentAlignment, FlexDirection, FlexWrap, ItemAlignment, LengthPct, PreferredSize};
        let html = "<html><body><div style='display:flex;flex-flow:column wrap;flex:2 3 40px;order:-2;align-items:center;justify-content:space-between;gap:5px 10px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("flex element");
        let indices = document.style_for_node(node).expect("computed flex style");
        let box_model = document.box_model_style(indices).expect("box style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(box_model.display, Display::Flex);
        assert_eq!(layout.flex_direction, FlexDirection::Column);
        assert_eq!(layout.flex_wrap, FlexWrap::Wrap);
        assert_eq!(layout.flex_grow, 2.0);
        assert_eq!(layout.flex_shrink, 3.0);
        assert_eq!(layout.flex_basis, PreferredSize::Px(40.0));
        assert_eq!(layout.order, -2);
        assert_eq!(layout.align_items, ItemAlignment::Center);
        assert_eq!(layout.justify_content, ContentAlignment::SpaceBetween);
        assert_eq!(layout.row_gap, LengthPct::Px(5.0));
        assert_eq!(layout.column_gap, LengthPct::Px(10.0));
    }

    #[test]
    fn negative_typed_gap_does_not_override_the_last_valid_gap() {
        use crate::document::LengthPct;
        let html = "<html><body><div style='display:grid;gap:12px 14px;gap:5px -10px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("grid element");
        let indices = document.style_for_node(node).expect("computed grid style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(layout.row_gap, LengthPct::Px(12.0));
        assert_eq!(layout.column_gap, LengthPct::Px(14.0));
    }

    #[test]
    fn legacy_grid_gap_aliases_share_the_canonical_computed_values() {
        use crate::document::LengthPct;
        let html = "<html><body><div id='parent' style='grid-row-gap:12px;grid-column-gap:14px'><div id='child' style='grid-row-gap:inherit;grid-column-gap:inherit'></div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        for id in ["parent", "child"] {
            let node = document.document().node_ids().find(|&node| document.document().element_ref(node).and_then(|element| element.attr("id")) == Some(id)).expect("gap alias element");
            let indices = document.style_for_node(node).expect("computed gap alias style");
            let layout = document.styles().layout_style(indices).expect("layout style");
            assert_eq!(layout.row_gap, LengthPct::Px(12.0));
            assert_eq!(layout.column_gap, LengthPct::Px(14.0));
        }
    }

    #[test]
    fn negative_typed_flex_basis_does_not_override_the_last_valid_basis() {
        use crate::document::PreferredSize;
        let html = "<html><body><div style='display:flex;flex-basis:40px;flex-basis:-10px'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("flex element");
        let indices = document.style_for_node(node).expect("computed flex style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(layout.flex_basis, PreferredSize::Px(40.0));
    }

    #[test]
    fn negative_typed_border_spacing_does_not_override_valid_spacing() {
        let html = "<html><body><table style='border-spacing:5px 7px;border-spacing:-20px'><tr><td>A</td></tr></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("table element");
        let indices = document.style_for_node(node).expect("computed table style");
        let style = document.box_model_style(indices).expect("box style");

        assert_eq!(style.border_spacing_horizontal, 5.0);
        assert_eq!(style.border_spacing_vertical, 7.0);
    }

    #[test]
    fn unsupported_or_malformed_list_marker_does_not_override_valid_marker() {
        use crate::document::ListStyleType;
        let html = "<html><body><ul><li style=\"list-style-type:square;list-style-type:symbols(cyclic)\">A</li><li style=\"list-style-type:circle;list-style-type:symbols(cyclic 'x')\">B</li></ul></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let list = find_body(document.document()).children().next().expect("list element");
        let markers = document
            .document()
            .element_ref(list)
            .expect("list node is an element")
            .children()
            .map(|node| {
                let indices = document.style_for_node(node).expect("computed list item style");
                document.text_style(indices).expect("text style").list_style_type
            })
            .collect::<Vec<_>>();

        assert_eq!(markers, vec![ListStyleType::Square, ListStyleType::Circle]);
    }

    #[test]
    fn unsupported_ideographic_text_transform_does_not_partially_override_fallback() {
        use crate::document::TextTransform;
        let html = "<html><body><p style='text-transform:uppercase;text-transform:lowercase full-width'>Abc</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("paragraph element");
        let indices = document.style_for_node(node).expect("computed paragraph style");

        assert_eq!(document.text_style(indices).expect("text style").text_transform, TextTransform::Uppercase);
    }

    #[test]
    fn invalid_typed_grid_tracks_do_not_override_the_last_valid_template() {
        use crate::document::{GridTemplateTrack, GridTrackBreadth, GridTrackSize, LengthPct};
        let html = "<html><body><div style='display:grid;grid-template-columns:100px;grid-template-columns:-10px'></div><div style='display:grid;grid-template-columns:80px;grid-template-columns:auto repeat(auto-fill,auto) auto'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let templates = find_body(document.document())
            .children()
            .map(|node| {
                let indices = document.style_for_node(node).expect("computed grid style");
                document.styles().layout_style(indices).expect("layout style").grid_template_columns.clone()
            })
            .collect::<Vec<_>>();

        assert_eq!(templates[0], vec![GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Length(LengthPct::Px(100.0))))]);
        assert_eq!(templates[1], vec![GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Length(LengthPct::Px(80.0))))]);
    }

    #[test]
    fn grid_tracks_names_flow_and_placement_cross_as_typed_values() {
        use crate::document::{GridAutoFlow, GridPlacement, GridTemplateTrack, GridTrackBreadth, GridTrackSize, LengthPct};
        let html = "<html><body><div style='display:grid;grid-template-columns:[start] 100px [middle] 1fr [end];grid-template-rows:20px;grid-auto-flow:column dense;gap:5px 10px;grid-column:2 / span 2'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("grid element");
        let indices = document.style_for_node(node).expect("computed grid style");
        let layout = document.styles().layout_style(indices).expect("layout style");

        assert_eq!(document.box_model_style(indices).expect("box style").display, Display::Grid);
        assert_eq!(layout.grid_auto_flow, GridAutoFlow::ColumnDense);
        assert_eq!(layout.grid_template_columns.len(), 2);
        assert_eq!(layout.grid_template_columns[0], GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Length(LengthPct::Px(100.0)))));
        assert_eq!(layout.grid_template_columns[1], GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Flex(1.0))));
        assert_eq!(layout.grid_template_rows, vec![GridTemplateTrack::Single(GridTrackSize::Breadth(GridTrackBreadth::Length(LengthPct::Px(20.0))))]);
        assert_eq!(layout.grid_column.start, GridPlacement::Line(std::num::NonZeroI16::new(2).unwrap()));
        assert_eq!(layout.grid_column.end, GridPlacement::Span(std::num::NonZeroU16::new(2).unwrap()));
        let line_names: Vec<Vec<&str>> = layout.grid_template_column_names.iter().map(|names| names.iter().map(|&name| document.styles().string(name).expect("interned line name")).collect()).collect();
        assert_eq!(line_names, vec![vec!["start"], vec!["middle"], vec!["end"]]);
    }

    #[test]
    fn display_contents_overrides_an_earlier_supported_fallback() {
        let html = "<html><body><div style='display:flex;display:contents'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("element");
        let indices = document.style_for_node(node).expect("computed style");

        assert_eq!(document.box_model_style(indices).expect("box style").display, Display::Contents);
    }

    #[test]
    fn authored_font_family_is_preserved_and_inherited() {
        let html = "<html><body style=\"font-family: 'DejaVu Serif'\"><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let family_idx = document.font_style(style).expect("validated style handle").font_family.expect("paragraph should inherit the authored family");

        assert!(document.styles().string(family_idx).expect("family should belong to the computed-style table").contains("DejaVu Serif"));
    }

    #[test]
    fn later_font_shorthand_overrides_universal_inherit_shorthand() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><div id='target'>X</div></body></html>", Some("* { font: inherit; } div { font: 20px/1 Ahem; }"));
        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target element");
        let style = document.style_for_node(target).expect("computed target style");

        assert_eq!(document.font_style(style).expect("font style").font_size, 20.0);
        assert_eq!(document.text_style(style).expect("text style").line_height, 20.0);
    }

    #[test]
    fn relative_font_shorthand_line_height_uses_its_resolved_font_size() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><div id='target'>X</div></body></html>", Some("div { font: 1.25em/1 Ahem; }"));
        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target element");
        let style = document.style_for_node(target).expect("computed target style");

        assert_eq!(document.font_style(style).expect("font style").font_size, 20.0);
        assert_eq!(document.text_style(style).expect("text style").line_height, 20.0);
    }

    #[test]
    fn normal_font_weight_resolves_to_css_weight_400() {
        let html = "<html><body><p style=\"font-weight: normal\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.font_style(style).expect("validated style handle").font_weight, 400);
    }

    #[test]
    fn relative_font_weights_resolve_from_the_parent_computed_weight() {
        let html = "<html><body><div id='w100'><span id='b100'></span></div><div id='w400'><span id='b400'></span><span id='l400'></span></div><div id='w600'><span id='b600'></span><span id='l600'></span></div><div id='w800'><span id='l800'></span></div></body></html>";
        let css = "#w100{font-weight:100}#w400{font-weight:400}#w600{font-weight:600}#w800{font-weight:800}#b100,#b400,#b600{font-weight:bolder}#l400,#l600,#l800{font-weight:lighter}";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let weight = |id| {
            let node = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("weight test element");
            let style = document.style_for_node(node).expect("weight test style");
            document.font_style(style).expect("weight test font").font_weight
        };

        assert_eq!(weight("b100"), 400);
        assert_eq!(weight("b400"), 700);
        assert_eq!(weight("b600"), 900);
        assert_eq!(weight("l400"), 100);
        assert_eq!(weight("l600"), 400);
        assert_eq!(weight("l800"), 700);
    }

    #[test]
    fn inline_custom_property_resolves_var_reference() {
        let html = "<html><body><p style=\"--accent: red; color: var(--accent);\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0xFF0000FF);
    }

    #[test]
    fn word_break_overflow_wrap_and_box_sizing_parse() {
        use crate::document::{BoxSizing, OverflowWrap, WordBreak};
        let html = "<html><body><p style=\"word-break: break-all; overflow-wrap: break-word; box-sizing: border-box\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").word_break, WordBreak::BreakAll);
        assert_eq!(document.text_style(style).expect("validated style handle").overflow_wrap, OverflowWrap::BreakWord);
        assert_eq!(document.box_model_style(style).expect("validated style handle").box_sizing, BoxSizing::BorderBox);
    }

    #[test]
    fn legacy_word_wrap_maps_to_overflow_wrap() {
        use crate::document::OverflowWrap;
        let html = "<html><body><p style=\"word-wrap: break-word\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").overflow_wrap, OverflowWrap::BreakWord);
    }

    #[test]
    fn float_and_clear_parse_into_box_model() {
        let html = "<html><body><p style=\"float:left; clear:both\">Hello</p><p style=\"float:right; clear:right\">World</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let box_model = document.box_model_style(style).expect("validated style handle");

        assert_eq!(box_model.float, Float::Left);
        assert_eq!(box_model.clear, Clear::Both);

        let right_idx = find_body(document.document()).children().nth(1).expect("second paragraph should be child of body");
        let right_style = document.style_for_node(right_idx).expect("second paragraph style should exist");
        let right_box_model = document.box_model_style(right_style).expect("validated second style handle");
        assert_eq!(right_box_model.float, Float::Right);
        assert_eq!(right_box_model.clear, Clear::Right);
    }

    #[test]
    fn line_height_stays_bound_to_final_font_size() {
        let html = "<html><body><p style=\"line-height:0.9em; font-size:450%;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let font_size = document.font_style(style).expect("validated style handle").font_size;
        let line_height = document.text_style(style).expect("validated style handle").line_height;

        assert!((line_height - font_size * 0.9).abs() < 0.01, "line-height should track the final font size");
    }

    #[test]
    fn invalid_author_css_does_not_drop_default_styles_or_boxes() {
        let html = "<html><body><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p { color: red"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph should have computed style");

        assert_eq!(document.box_model_style(style).expect("validated style handle").display, Display::Block);
    }

    #[test]
    fn later_css_chunks_win_for_equal_specificity() {
        let html = "<html><body><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p { color: red; }", "p { color: blue; }"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0x0000FFFF);
    }

    #[test]
    fn scope_limits_and_proximity_participate_in_matching_and_cascade() {
        let html = r#"<html><body>
            <div class="outer">
                <div class="inner"><p id="near" class="target">near</p></div>
                <div class="limit"><p id="limited" class="target">limited</p></div>
            </div>
        </body></html>"#;
        let css = r#"
            .target { color: black; }
            @scope (.outer) { .target { color: red; } }
            @scope (.inner) { .target { color: green; } }
            @scope (.outer) to (.limit) { .target { color: blue; } }
        "#;
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let color_of = |id| {
            let node = document.document().node_ids().find(|node| document.document().get_dom_id(*node) == Some(id)).expect("element by id");
            document.text_style(document.style_for_node(node).expect("computed style")).expect("text style").color
        };

        assert_eq!(color_of("near"), 0x008000FF, "the nearer scope wins after specificity");
        assert_eq!(color_of("limited"), 0xFF0000FF, "the scope limit blocks only the blue rule");
    }

    #[test]
    fn wpt_important_author_font_size_beats_inline_style_and_controls_em_line_height() {
        // Static adaptation of WPT css/css-cascade/important-vs-inline-002.html:
        // https://github.com/web-platform-tests/wpt/blob/master/css/css-cascade/important-vs-inline-002.html
        let html = "<html><body><p class=\"outer\" style=\"font-size: 24px\">Text</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(".outer { font-size: 18px !important; line-height: 2em; }"));

        let paragraph = find_body(document.document()).children().next().expect("paragraph");
        let style = document.style_for_node(paragraph).expect("paragraph style");

        assert_eq!(document.font_style(style).expect("font style").font_size, 18.0);
        assert_eq!(document.text_style(style).expect("text style").line_height, 36.0);
    }

    #[test]
    fn xhtml_author_max_width_uses_the_inherited_font_size() {
        use crate::document::{Float, PreferredSize};

        let parsed = html_parse::parse_xml_document(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style><![CDATA[
                div#outer { font: 30px/4 Ahem; position: absolute; width: auto; }
                div#inner { float: left; max-width: 4em; }
            ]]></style></head><body><div id="outer"><div id="inner">12345678</div></div></body></html>"#,
        )
        .expect("valid XHTML");
        let css = match &parsed.head_metadata().stylesheets[0] {
            html_parse::StylesheetReference::Inline(css) => css.clone(),
            _ => panic!("expected inline stylesheet"),
        };
        let document = crate::style_document(parsed.build_dom(), &[&css]);
        let body = find_body(document.document());
        let outer = body.children().find(|node| document.document().is_element_node(*node)).expect("outer div");
        let inner = document.document().element_ref(outer).expect("outer element").children().find(|node| document.document().is_element_node(*node)).expect("inner div");
        let style = document.style_for_node(inner).expect("inner style");

        assert_eq!(document.font_style(style).expect("font style").font_size, 30.0);
        assert_eq!(document.box_model_style(style).expect("box style").float, Float::Left);
        assert_eq!(document.box_model_style(style).expect("box style").max_width, PreferredSize::Px(120.0));
    }

    #[test]
    fn wpt_unset_inherits_color_but_resets_non_inherited_margin() {
        // Semantic subset of WPT css/css-cascade/all-prop-unset-color.html.
        // `all: unset` is decomposed here into supported longhands so this test
        // stays local to CSS-wide keyword resolution:
        // https://github.com/web-platform-tests/wpt/blob/master/css/css-cascade/all-prop-unset-color.html
        let html = "<html><body><div style=\"color: red; margin-left: 20px\"><span style=\"color: unset; margin-left: unset\">Text</span></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let parent = find_body(document.document()).children().next().expect("parent div");
        let child = document.document().element_ref(parent).expect("parent element").children().find(|node| document.document().is_element_node(*node)).expect("child span");
        let style = document.style_for_node(child).expect("child style");

        assert_eq!(document.text_style(style).expect("text style").color, 0xFF0000FF);
        assert_eq!(document.box_model_style(style).expect("box style").margin_left, crate::document::LengthPct::Px(0.0));
    }

    #[test]
    fn current_color_border_uses_text_color() {
        let html = "<html><body><p style=\"color: red; border: 1px solid\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let border = document.border_style(style).expect("validated style handle");

        assert_eq!(border.border_top_color, 0xFF0000FF, "borders without an explicit color should use currentColor");
    }

    #[test]
    fn inherited_background_current_color_resolves_against_the_descendant_color() {
        let html = "<html><body><div style='color:red;background-color:currentcolor'><div><div id='target' style='color:green'>text</div></div></div></body></html>";
        let css = "div div { background-color: inherit }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));

        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target element");
        let style = document.style_for_node(target).expect("target style");
        let background = document.background_style(style).expect("background style");

        assert!(background.background_color_current_color, "inherit must preserve the currentColor dependency");
        assert_eq!(document.style_view(style).expect("style view").background_color(), 0x008000FF);
    }

    #[test]
    fn em_length_uses_final_font_size_regardless_of_order() {
        let html = "<html><body><p style=\"padding-left: 1em; font-size: 32px;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let padding_left = document.box_model_style(style).expect("validated style handle").padding_left;

        assert_eq!(padding_left, crate::document::LengthPct::Px(32.0), "1em padding should resolve against the element's final font size");
    }

    #[test]
    fn em_length_sees_font_size_from_later_rule() {
        let html = "<html><body><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p { padding-left: 1em; }", "p { font-size: 32px; }"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let padding_left = document.box_model_style(style).expect("validated style handle").padding_left;

        assert_eq!(padding_left, crate::document::LengthPct::Px(32.0), "em lengths should see font-size from any rule in the cascade");
    }

    #[test]
    fn descendant_rem_uses_the_computed_root_font_size() {
        let html = "<html><body><div id='target'></div></body></html>";
        let css = ":root { font-size: 25%; } #target { width: 25rem; height: 25rem; }";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some(css));
        let target = document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some("target")).expect("target");
        let style = document.box_model_style(document.style_for_node(target).expect("target style")).expect("box style");

        assert_eq!(style.width, PreferredSize::Px(100.0));
        assert_eq!(style.height, PreferredSize::Px(100.0));
    }

    #[test]
    fn root_font_units_remain_symbolic_until_root_metrics_are_available() {
        let html = "<html><body><div id='cap' style='font-size:1cap;width:2em'></div><div id='rch' style='font-size:1rch'></div><div id='rcap' style='font-size:1rcap'></div><div id='rlh' style='font-size:1rlh'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let by_id = |id| document.document().node_ids().find(|&node| document.document().get_dom_id(node) == Some(id)).expect("target");
        let cap = document.style_for_node(by_id("cap")).expect("cap style");
        let rch = document.style_for_node(by_id("rch")).expect("rch style");
        let rcap = document.style_for_node(by_id("rcap")).expect("rcap style");
        let rlh = document.style_for_node(by_id("rlh")).expect("rlh style");

        assert_eq!(document.font_style(cap).expect("cap font").font_size_cap_height_px, 16.0);
        assert!(matches!(document.box_model_style(cap).expect("cap box").width, PreferredSize::Calc { cap_height_px, .. } if cap_height_px == 32.0));
        assert_eq!(document.font_style(rch).expect("rch font").font_size_root_ch, 1.0);
        assert_eq!(document.font_style(rcap).expect("rcap font").font_size_root_cap_height, 1.0);
        assert_eq!(document.font_style(rlh).expect("rlh font").font_size_root_line_height, 1.0);
        assert_eq!(document.styles().used_view_with_root(cap, 0.5, 0.5, 0.8, 9.0, 13.0, 20.0).expect("used cap").font_size(), 12.8);
        assert_eq!(document.styles().used_view_with_root(rch, 0.5, 0.5, 0.8, 9.0, 13.0, 20.0).expect("used rch").font_size(), 9.0);
        assert_eq!(document.styles().used_view_with_root(rcap, 0.5, 0.5, 0.8, 9.0, 13.0, 20.0).expect("used rcap").font_size(), 13.0);
        assert_eq!(document.styles().used_view_with_root(rlh, 0.5, 0.5, 0.8, 9.0, 13.0, 20.0).expect("used rlh").font_size(), 20.0);
    }

    #[test]
    fn text_align_last_survives_later_text_align_rule() {
        use crate::document::TextAlign;
        let html = "<html><body><p>Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p { text-align-last: center; }", "p { text-align: justify; }"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let text = document.text_style(style).expect("validated style handle");

        assert_eq!(text.text_align, TextAlign::Justify);
        assert_eq!(text.text_align_last, TextAlign::Center, "explicit text-align-last should not be clobbered by a later text-align rule");
    }

    #[test]
    fn text_align_still_syncs_text_align_last_when_not_explicit() {
        use crate::document::TextAlign;
        let html = "<html><body><p style=\"text-align: center;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").text_align_last, TextAlign::Center);
    }

    #[test]
    fn absolute_length_units_convert_to_px() {
        let html = "<html><body><p style=\"width: 1in; height: 2.54cm;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let box_model = document.box_model_style(style).expect("validated style handle");

        assert_eq!(box_model.width, crate::document::PreferredSize::Px(96.0));
        assert_eq!(box_model.height, crate::document::PreferredSize::Px(96.0));
    }

    #[test]
    fn background_shorthand_color_comes_from_last_layer() {
        let html = "<html><body><p style=\"background: none, none blue;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.background_style(style).expect("validated style handle").background_color, 0x0000FFFF);
    }

    #[test]
    fn unsupported_gradient_does_not_erase_background_fallback() {
        let html = "<html><body><p style=\"background: red; background: linear-gradient(white, black);\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        let background = document.background_style(style).expect("validated style handle");
        assert_eq!(background.background_color, 0xFF0000FF);
        assert!(background.background_image_present, "unsupported painting must not erase the computed image layer's presence");
    }

    #[test]
    fn background_shorthand_color_paints_under_an_unsupported_image() {
        let html = "<html><body><p style=\"background: red; background: url('paper.png') blue;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        let background = document.background_style(style).expect("validated style handle");
        assert_eq!(background.background_color, 0x0000FFFF);
        assert!(background.background_image_present, "unsupported painting must not erase the computed image layer's presence");
    }

    #[test]
    fn variable_substituted_background_shorthand_keeps_its_color() {
        let html = "<html><body><p style=\"background-color:red;--a:url(nothing) green;background:var(--a)\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        let background = document.background_style(style).expect("validated style handle");
        assert_eq!(background.background_color, 0x008000FF);
        assert!(background.background_image_present);
    }

    #[test]
    fn text_decoration_and_outline_cross_as_typed_paint_values() {
        use crate::document::{DecorationColor, TextDecorationStyle};
        let html = "<html><body><span style='color:black;text-decoration:underline 2px dashed #123456;outline:4px dotted #abcdef'>A</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("decorated span");
        let indices = document.style_for_node(node).expect("decorated style");
        let paint = document.background_style(indices).expect("paint style");

        assert!(paint.text_decoration.lines.underline());
        assert_eq!(paint.text_decoration.style, TextDecorationStyle::Dashed);
        assert_eq!(paint.text_decoration.color, DecorationColor::Rgba(0x123456FF));
        assert_eq!(paint.text_decoration.thickness, crate::document::TextDecorationThickness::Length(LengthPct::Px(2.0)));
        assert_eq!(paint.outline.width(), FontRelativeLength::px(4.0).unwrap());
        assert_eq!(paint.outline.style, BorderStyle::Dotted);
        assert_eq!(paint.outline.color, DecorationColor::Rgba(0xABCDEFFF));
    }

    #[test]
    fn ex_paint_and_grid_values_cross_only_through_validated_used_types() {
        use crate::document::{UsedGridTemplateTrack, UsedGridTrackBreadth, UsedGridTrackSize, UsedLengthPct};
        let html = "<html><body><div style='font-size:20px;outline:2ex solid;display:grid;grid-template-columns:3ex'>A</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("grid div");
        let indices = document.style_for_node(node).expect("grid style");
        let computed = document.styles().view(indices).expect("validated computed style");

        assert!(computed.requires_font_metrics());
        assert!(document.styles().used_view(indices, f32::NAN, 0.5, 0.8).is_none());
        assert!(document.styles().used_view(indices, 0.5, -0.1, 0.8).is_none());
        assert!(document.styles().used_view(indices, 0.5, 0.0, 0.8).is_some());

        let used = document.styles().used_view(indices, 0.6, 0.5, 0.8).expect("positive finite font-relative ratios");
        assert!((used.outline().width() - 24.0).abs() < 0.001);
        assert_eq!(used.grid_template_columns().collect::<Vec<_>>(), vec![UsedGridTemplateTrack::Single(UsedGridTrackSize::Breadth(UsedGridTrackBreadth::Length(UsedLengthPct::Px(36.0))))]);
    }

    #[test]
    fn metric_dependent_values_cross_only_when_the_model_has_a_closed_representation() {
        let html = "<html><body><p style='font-size:20px;font-size:2ex;line-height:24px;line-height:2ex;letter-spacing:3px;letter-spacing:2ex;width:10px;width:2ch'>A</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("paragraph");
        let indices = document.style_for_node(node).expect("paragraph style");
        let view = document.styles().view(indices).expect("validated style");
        let used = document.styles().used_view(indices, 0.6, 0.5, 0.8).expect("valid selected-face metrics");

        assert_eq!(view.font_size(), 20.0);
        assert_eq!(view.line_height(), 0.0);
        assert_eq!(used.line_height(), 24.0);
        assert_eq!(view.letter_spacing(), 3.0);
        assert_eq!(view.width(), PreferredSize::Ch(40.0));
    }

    #[test]
    fn unsupported_decoration_and_outline_styles_preserve_atomic_fallbacks() {
        use crate::document::{DecorationColor, TextDecorationStyle};
        let html = "<html><body><span style='text-decoration:underline solid red;text-decoration:line-through wavy blue;outline:2px solid red;outline:4px double blue'>A</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("decorated span");
        let indices = document.style_for_node(node).expect("decorated style");
        let paint = document.background_style(indices).expect("paint style");

        assert!(paint.text_decoration.lines.underline());
        assert!(!paint.text_decoration.lines.line_through());
        assert_eq!(paint.text_decoration.style, TextDecorationStyle::Solid);
        assert_eq!(paint.text_decoration.color, DecorationColor::Rgba(0xFF0000FF));
        assert_eq!(paint.outline.width(), FontRelativeLength::px(2.0).unwrap());
        assert_eq!(paint.outline.style, BorderStyle::Solid);
        assert_eq!(paint.outline.color, DecorationColor::Rgba(0xFF0000FF));
    }

    #[test]
    fn decoration_current_color_resolves_from_the_final_text_color() {
        let html = "<html><body><span style='text-decoration:underline;outline:1px solid;color:#2468ac'>A</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let node = find_body(document.document()).children().next().expect("decorated span");
        let indices = document.style_for_node(node).expect("decorated style");
        let view = document.styles().view(indices).expect("validated style view");

        assert_eq!(view.text_decoration_color(), 0x2468ACFF);
        assert_eq!(view.outline_color(), 0x2468ACFF);
    }

    #[test]
    fn float_keyword_is_case_insensitive() {
        let html = "<html><body><p style=\"FLOAT: LEFT\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.box_model_style(style).expect("validated style handle").float, Float::Left);
    }

    #[test]
    fn unitless_line_height_recomputes_for_child_font_size() {
        let html = "<html><body style=\"line-height: 1.5;\"><p style=\"font-size: 32px;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let line_height = document.text_style(style).expect("validated style handle").line_height;

        assert!((line_height - 48.0).abs() < 0.01, "unitless line-height should recompute against the child's font size, got {line_height}");
    }

    #[test]
    fn length_line_height_inherits_as_px() {
        let html = "<html><body style=\"line-height: 20px;\"><p style=\"font-size: 32px;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let line_height = document.text_style(style).expect("validated style handle").line_height;

        assert!((line_height - 20.0).abs() < 0.01, "length line-height should inherit as resolved px, got {line_height}");
    }

    #[test]
    fn authored_zero_line_height_remains_distinct_from_normal() {
        let html = "<html><body><div style='line-height:0'>zero</div><div style='line-height:normal'>normal</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let mut children = find_body(document.document()).children();
        let zero = document.styles().view(document.style_for_node(children.next().expect("zero div")).expect("zero style")).expect("valid zero style");
        let normal = document.styles().view(document.style_for_node(children.next().expect("normal div")).expect("normal style")).expect("valid normal style");

        assert_eq!(zero.line_height(), 0.0);
        assert!(!zero.line_height_is_normal());
        assert_eq!(normal.line_height(), 0.0);
        assert!(normal.line_height_is_normal());
    }

    #[test]
    fn font_shorthand_and_line_height_longhand_follow_cascade_order() {
        let html = "<html><body><div style='font:20px/1 Ahem;line-height:0'>zero</div><div style='line-height:0;font:20px/1 Ahem'>twenty</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let mut children = find_body(document.document()).children();
        let zero = document.styles().view(document.style_for_node(children.next().expect("zero div")).expect("zero style")).expect("valid zero style");
        let twenty = document.styles().view(document.style_for_node(children.next().expect("twenty div")).expect("twenty style")).expect("valid twenty style");

        assert_eq!(zero.font_size(), 20.0);
        assert_eq!(zero.line_height(), 0.0);
        assert!(!zero.line_height_is_normal());
        assert_eq!(twenty.font_size(), 20.0);
        assert_eq!(twenty.line_height(), 20.0);
        assert!(!twenty.line_height_is_normal());
    }

    #[test]
    fn ex_line_height_is_deferred_until_selected_font_metrics_are_available() {
        let html = "<html><body><div style='line-height:6ex;font-size:20px'>text</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let div = find_body(document.document()).children().next().expect("div");
        let indices = document.style_for_node(div).expect("div style");
        let computed = document.styles().view(indices).expect("valid computed style");
        let used = document.styles().used_view(indices, 0.8, 0.5, 0.8).expect("valid selected-face metrics");

        assert_eq!(computed.line_height(), 0.0);
        assert!(computed.requires_font_metrics());
        assert_eq!(used.line_height(), 96.0);
    }

    #[test]
    fn structural_pseudo_classes_match() {
        let html = "<html><body><p>One</p><p>Two</p><p>Three</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p:first-child { color: red; } p:nth-child(2) { color: blue; } p:last-child { color: green; }"]);

        let doc = document.document();
        let paragraphs: Vec<_> = find_body(doc).children().collect();
        let color_of = |idx| document.text_style(document.style_for_node(idx).expect("style")).expect("validated style handle").color;

        assert_eq!(color_of(paragraphs[0]), 0xFF0000FF, ":first-child should match the first paragraph");
        assert_eq!(color_of(paragraphs[1]), 0x0000FFFF, ":nth-child(2) should match the second paragraph");
        assert_eq!(color_of(paragraphs[2]), 0x008000FF, ":last-child should match the third paragraph");
    }

    #[test]
    fn not_first_child_excludes_the_first_child() {
        let html = "<html><body><p>One</p><p>Two</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p:not(:first-child) { color: red; }"]);

        let doc = document.document();
        let paragraphs: Vec<_> = find_body(doc).children().collect();
        let color_of = |idx| document.text_style(document.style_for_node(idx).expect("style")).expect("validated style handle").color;

        assert_eq!(color_of(paragraphs[0]), 0x000000FF, ":not(:first-child) must not match the first child");
        assert_eq!(color_of(paragraphs[1]), 0xFF0000FF);
    }

    #[test]
    fn rule_cascades_with_most_specific_matching_selector() {
        let html = "<html><body><p class=\"foo\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline_css_chunks(html, &["p, .foo { color: red; }", "p { color: blue; }"]);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0xFF0000FF, "the .foo selector's specificity should win over the later p rule");
    }

    #[test]
    fn adjacent_replaced_element_can_be_floated() {
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline("<html><body><img src='a'/> <img src='b'/></body></html>", Some("img + img { float: right; }"));
        let images = find_body(document.document()).children().filter(|&node| document.document().get_dom_tag(node) == Some("img")).collect::<Vec<_>>();

        let first = document.box_model_style(document.style_for_node(images[0]).expect("first image style")).expect("box model");
        let second = document.box_model_style(document.style_for_node(images[1]).expect("second image style")).expect("box model");
        assert_eq!(first.float, Float::None);
        assert_eq!(second.float, Float::Right);
    }

    #[test]
    fn undeclared_border_color_defaults_to_current_color() {
        let html = "<html><body><p style=\"color: red; border-width: 2px; border-style: solid;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");
        let border = document.border_style(style).expect("validated style handle");

        assert_eq!(border.border_top_color, 0xFF0000FF, "border-color's initial value is currentColor");
        assert_eq!(border.border_left_color, 0xFF0000FF);
    }

    #[test]
    fn only_anchors_with_href_get_link_styling() {
        let html = "<html><body><a id=\"target\">Anchor</a><a href=\"x.html\">Link</a></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let doc = document.document();
        let anchors: Vec<_> = find_body(doc).children().collect();
        let plain = document.style_for_node(anchors[0]).expect("anchor style");
        let link = document.style_for_node(anchors[1]).expect("link style");

        assert_eq!(document.text_style(plain).expect("validated style handle").color, 0x000000FF, "href-less anchor must not be link-colored");
        assert!(!document.background_style(plain).expect("validated style handle").text_decoration.lines.underline(), "href-less anchor must not be underlined");
        assert_eq!(document.text_style(link).expect("validated style handle").color, 0x0000FFFF);
        assert!(document.background_style(link).expect("validated style handle").text_decoration.lines.underline(), "link should be underlined");
    }

    #[test]
    fn vertical_align_inherit_propagates_tbody_middle_to_cells() {
        use crate::document::VerticalAlignValue;
        let html = "<html><body><table><tbody><tr><td>X</td></tr></tbody></table></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let doc = document.document();
        let mut node = find_body(doc).node_id();
        for _ in 0..4 {
            // table > tbody > tr > td
            node = doc.element_ref(node).expect("element").children().find(|&child| doc.element_ref(child).is_some()).expect("child element");
        }
        let td = doc.element_ref(node).expect("td");
        assert_eq!(td.tag(), "td");
        let style = document.style_for_node(node).expect("td style should exist");

        assert_eq!(document.box_model_style(style).expect("validated style handle").vertical_align, VerticalAlignValue::Middle, "the UA sheet's vertical-align: inherit chain should reach the cell");
    }

    #[test]
    fn vertical_align_css_two_values_preserve_typed_offsets() {
        use crate::document::VerticalAlignValue;
        let html = "<html><body><span style='vertical-align: text-top'>A</span><span style='vertical-align: 25%'>B</span><span style='vertical-align: calc(20% - 2px)'>C</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let spans: Vec<_> = find_body(document.document()).children().collect();
        let text_top = document.box_model_style(document.style_for_node(spans[0]).expect("first span style")).expect("validated first span style").vertical_align;
        let percentage = document.box_model_style(document.style_for_node(spans[1]).expect("second span style")).expect("validated second span style").vertical_align;
        let calc = document.box_model_style(document.style_for_node(spans[2]).expect("third span style")).expect("validated third span style").vertical_align;

        assert_eq!(text_top, VerticalAlignValue::TextTop);
        assert_eq!(percentage, VerticalAlignValue::Percent(0.25));
        assert_eq!(calc, VerticalAlignValue::Calc { absolute_px: -2.0, line_height_fraction: 0.2, x_height_px: 0.0 });
    }

    #[test]
    fn ex_vertical_align_is_deferred_until_selected_font_metrics_are_available() {
        use crate::document::VerticalAlignValue;
        let html = "<html><body><span style='font-size:20px;vertical-align:6ex'>A</span></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);
        let span = find_body(document.document()).children().next().expect("span");
        let indices = document.style_for_node(span).expect("span style");
        let computed = document.styles().view(indices).expect("valid computed style");
        let used = document.styles().used_view(indices, 0.8, 0.5, 0.8).expect("valid selected-face metrics");

        assert_eq!(computed.vertical_align(), VerticalAlignValue::Calc { absolute_px: 0.0, line_height_fraction: 0.0, x_height_px: 120.0 });
        assert!(computed.requires_font_metrics());
        assert_eq!(used.vertical_align(), VerticalAlignValue::Calc { absolute_px: 96.0, line_height_fraction: 0.0, x_height_px: 0.0 });
    }

    #[test]
    fn parser_supported_text_overflow_reaches_computed_box_style() {
        use crate::document::{OverflowMode, TextOverflow};
        let html = "<html><body><div style='overflow: hidden clip; text-overflow: ellipsis'>A</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let style = document.style_for_node(div).expect("div style should exist");
        let box_style = document.box_model_style(style).expect("validated box style");

        assert_eq!(box_style.overflow_x, OverflowMode::Hidden);
        assert_eq!(box_style.overflow_y, OverflowMode::Clip);
        assert_eq!(box_style.text_overflow, TextOverflow::Ellipsis);
    }

    #[test]
    fn overflow_axes_are_normalized_after_the_cascade() {
        use crate::document::OverflowMode;
        let html = "<html><body><div style='overflow-x: visible; overflow-y: hidden'>A</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let style = document.style_for_node(div).expect("div style should exist");
        let box_style = document.box_model_style(style).expect("validated box style");

        assert_eq!(box_style.overflow_x, OverflowMode::Auto);
        assert_eq!(box_style.overflow_y, OverflowMode::Hidden);
    }

    #[test]
    fn unsupported_custom_text_overflow_marker_does_not_override_ellipsis() {
        use crate::document::TextOverflow;
        let html = "<html><body><div style='text-overflow: ellipsis; text-overflow: \"marker\"'>A</div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let style = document.style_for_node(div).expect("div style should exist");

        assert_eq!(document.box_model_style(style).expect("validated box style").text_overflow, TextOverflow::Ellipsis);
    }

    #[test]
    fn text_align_initial_resets_inherited_alignment() {
        use crate::document::TextAlign;
        let html = "<html><body style=\"text-align: center;\"><p style=\"text-align: initial;\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").text_align, TextAlign::Left);
    }

    #[test]
    fn list_item_keeps_inherited_text_align() {
        use crate::document::TextAlign;
        // The UA sheet's `li { text-align: match-parent }` must not reset
        // inherited justification.
        let html = "<html><body style=\"text-align: justify;\"><ul><li>Hello</li></ul></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let doc = document.document();
        let ul_idx = find_body(doc).children().next().expect("ul should be child of body");
        let li_idx = doc.element_ref(ul_idx).expect("ul element").children().next().expect("li should be child of ul");
        let style = document.style_for_node(li_idx).expect("li style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").text_align, TextAlign::Justify);
    }

    #[test]
    fn mark_system_colors_resolve_to_yellow_on_black() {
        let html = "<html><body><mark>Hello</mark></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let mark_idx = find_body(document.document()).children().next().expect("mark should be child of body");
        let style = document.style_for_node(mark_idx).expect("mark style should exist");

        assert_eq!(document.background_style(style).expect("validated style handle").background_color, 0xFFFF00FF, "Mark system color should be yellow");
        assert_eq!(document.text_style(style).expect("validated style handle").color, 0x000000FF, "MarkText system color should be black");
    }

    #[test]
    fn deprecated_system_colors_resolve_to_their_required_aliases() {
        use super::system_color_to_u32;
        use lightningcss::values::color::SystemColor;

        assert_eq!(system_color_to_u32(&SystemColor::ActiveCaption), system_color_to_u32(&SystemColor::Canvas));
        assert_eq!(system_color_to_u32(&SystemColor::Background), system_color_to_u32(&SystemColor::Canvas));
        assert_eq!(system_color_to_u32(&SystemColor::ButtonHighlight), system_color_to_u32(&SystemColor::ButtonFace));
        assert_eq!(system_color_to_u32(&SystemColor::ButtonShadow), system_color_to_u32(&SystemColor::ButtonFace));
        assert_eq!(system_color_to_u32(&SystemColor::InactiveCaption), system_color_to_u32(&SystemColor::Canvas));
        assert_eq!(system_color_to_u32(&SystemColor::InactiveCaptionText), system_color_to_u32(&SystemColor::GrayText));
        assert_eq!(system_color_to_u32(&SystemColor::ThreeDFace), system_color_to_u32(&SystemColor::ButtonFace));
    }

    #[test]
    fn modern_colors_convert_to_the_renderers_srgb_storage() {
        let html = "<html><body><p style=\"color: alpha(from red / 50%); background-color: lab(100% 0 0);\">Hello</p></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let p_idx = find_body(document.document()).children().next().expect("paragraph should be child of body");
        let style = document.style_for_node(p_idx).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0xFF000080);
        assert_eq!(document.background_style(style).expect("validated style handle").background_color, 0xFFFFFFFF);
    }

    #[test]
    fn current_color_on_color_computes_to_the_inherited_color() {
        let html = "<html><body><div style='color:green'><p style='color:red;color:currentColor'>Hello</p></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let div = find_body(document.document()).children().next().expect("div should be child of body");
        let paragraph = document.document().element_ref(div).expect("div element").children().next().expect("paragraph should be child of div");
        let style = document.style_for_node(paragraph).expect("paragraph style should exist");

        assert_eq!(document.text_style(style).expect("validated style handle").color, 0x008000FF);
    }

    #[test]
    fn hr_inset_border_renders_as_solid_gray() {
        let html = "<html><body><hr></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, None);

        let hr_idx = find_body(document.document()).children().next().expect("hr should be child of body");
        let style = document.style_for_node(hr_idx).expect("hr style should exist");
        let border = document.border_style(style).expect("validated style handle");

        assert_eq!(border.border_top_style, crate::document::BorderStyle::Solid, "the UA sheet's `border-style: inset` should paint as solid");
        assert_eq!(border.border_top_width, FontRelativeLength::px(1.0).unwrap());
        assert_eq!(border.border_top_color, 0x808080FF, "undeclared border color should take hr's gray text color");
    }

    #[test]
    fn resolves_before_and_after_content_into_typed_pseudo_styles() {
        use html_style_model::{GeneratedContentItem, StyleStringId};

        let html = "<html><body><div data-x='B'></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("div:before{content:'A' attr(data-x);color:green}div:after{content:'Z'}"));
        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let (before_style, before, _) = document.styles.before_style_for_node(div).expect("before pseudo should have computed content");
        let (_, after, _) = document.styles.after_style_for_node(div).expect("after pseudo should have computed content");

        let string = |id: StyleStringId| document.styles.string(id).expect("generated string should belong to style result");
        assert!(matches!(before.items.as_slice(), [GeneratedContentItem::Text(a), GeneratedContentItem::Attribute(x)] if string(*a) == "A" && string(*x) == "data-x"));
        assert!(matches!(after.items.as_slice(), [GeneratedContentItem::Text(z)] if string(*z) == "Z"));
        assert_eq!(document.text_style(before_style).expect("validated pseudo style").color, 0x008000FF);
        assert_eq!(document.box_model_style(before_style).expect("validated pseudo box style").display, Display::Inline);
    }

    #[test]
    fn pseudo_display_inherit_copies_the_originating_elements_display() {
        let html = "<html><body><div></div></body></html>";
        let mut factory = DocumentFactory::new();
        let document = factory.parse_with_new_pipeline(html, Some("div:before{content:'A';display:inherit}"));
        let div = find_body(document.document()).children().next().expect("div should be a child of body");
        let (before_style, _, _) = document.styles.before_style_for_node(div).expect("before pseudo should exist");

        assert_eq!(document.box_model_style(before_style).expect("validated pseudo box style").display, Display::Block);
    }

    fn find_body(document: &Document) -> ElementRef<'_> {
        let html_idx = document.dom_root().expect("html root should exist");
        let html = document.element_ref(html_idx).expect("html should be element");
        let body_idx = html.children().find(|&child_idx| document.element_ref(child_idx).is_some_and(|element| element.tag() == "body")).expect("body should be child of html");
        document.element_ref(body_idx).expect("body should be element")
    }
}
