use super::*;

// Helper functions for DOM-based style resolution
use crate::style::matching::selectors::AncestorFilter;

/// Build the starting style for an element: inherited groups (`font`, `text`)
/// copied from the parent, reset groups (`box_model`, `border`, `background`)
/// left at their defaults.
pub(super) fn get_inherited_style_dom(
    doc: &Document,
    styles: &ComputedStylesBuilder,
    node_idx: DomNodeId,
) -> WorkingStyle {
    let style = if let Some(parent_idx) = doc.get_dom_parent(node_idx) {
        if let Some(parent_style) = styles.style_for_node(parent_idx) {
            let parent_box_model = styles
                .box_model_style(parent_style)
                .expect("builder-issued parent style handle");
            let mut style = WorkingStyle {
                font: styles
                    .font_style(parent_style)
                    .expect("builder-issued parent style handle")
                    .clone(),
                text: styles
                    .text_style(parent_style)
                    .expect("builder-issued parent style handle")
                    .clone(),
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
pub(super) fn root_font_size_for_resolution(doc: &Document, styles: &ComputedStylesBuilder) -> f32 {
    doc.dom_root()
        .and_then(|root| styles.style_for_node(root))
        .and_then(|indices| styles.font_style(indices))
        .map(|font| font.font_size)
        .filter(|size| size.is_finite() && *size >= 0.0)
        .unwrap_or_else(|| doc.root_font_size())
}

/// Read-only document facade used while converting property values. It keeps
/// all existing document queries available through `Deref`, while making
/// every `rem` conversion in this stage observe the computed root size.
pub(super) struct ResolutionDocument<'a> {
    pub(super) document: &'a Document,
    pub(super) root_font_size: f32,
}

impl ResolutionDocument<'_> {
    pub(super) fn root_font_size(&self) -> f32 {
        self.root_font_size
    }
}

impl Deref for ResolutionDocument<'_> {
    type Target = Document;

    fn deref(&self) -> &Self::Target {
        self.document
    }
}

pub(super) fn inherit_box_model_properties(style: &mut BoxModel, parent: &BoxModel) {
    style.border_collapse = parent.border_collapse;
    style.border_spacing_horizontal = parent.border_spacing_horizontal;
    style.border_spacing_vertical = parent.border_spacing_vertical;
    style.caption_side = parent.caption_side;
    style.empty_cells = parent.empty_cells;
}

/// Build ancestor filter for DOM nodes (optimization)
pub(super) fn extend_ancestor_filter_dom(
    doc: &Document,
    node_idx: DomNodeId,
    filter: &mut AncestorFilter,
) {
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

pub(super) fn selector_might_match_dom(
    selector: &lightningcss::selector::Selector,
    ancestor_filter: &AncestorFilter,
    _doc: &Document,
    _node_idx: DomNodeId,
) -> bool {
    use crate::style::matching::dom::selector_might_match_with_filter;
    selector_might_match_with_filter(selector, ancestor_filter)
}

pub(super) fn inline_style_attribute(doc: &Document, node_idx: DomNodeId) -> Option<String> {
    let style_attr = doc.get_dom_attr(node_idx, "style")?;
    if style_attr.trim().is_empty() {
        return None;
    }
    Some(normalize_declarations(style_attr))
}

pub(super) fn parse_inline_style_attribute<'a>(
    doc: &Document,
    node_idx: DomNodeId,
) -> Option<StyleAttribute<'a>> {
    let style_attr = inline_style_attribute(doc, node_idx)?;
    let leaked: &'a str = Box::leak(style_attr.into_boxed_str());
    StyleAttribute::parse(
        leaked,
        ParserOptions {
            error_recovery: true,
            ..ParserOptions::default()
        },
    )
    .ok()
}

pub(super) fn collect_inline_style_custom_properties<'a>(
    custom_properties: &mut FxHashMap<String, TokenList<'a>>,
    parent_custom: &FxHashMap<String, TokenList<'a>>,
    revert_basis: &FxHashMap<String, TokenList<'a>>,
    revert_layer_basis: &FxHashMap<String, TokenList<'a>>,
    style_attr: &str,
) {
    for declaration in style_attr.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if !name.starts_with("--") {
            continue;
        }

        let value = value
            .trim()
            .strip_suffix("!important")
            .unwrap_or(value.trim())
            .trim();
        let leaked_value: &'a str = Box::leak(value.to_string().into_boxed_str());
        let Ok(tokens) =
            TokenList::parse_string_with_options(leaked_value, ParserOptions::default())
        else {
            continue;
        };

        if let Some(keyword) = single_ident_keyword(&tokens)
            && apply_custom_keyword(
                custom_properties,
                parent_custom,
                revert_basis,
                revert_layer_basis,
                name,
                keyword,
            )
        {
            continue;
        }

        custom_properties.insert(name.to_string(), tokens);
    }
}
