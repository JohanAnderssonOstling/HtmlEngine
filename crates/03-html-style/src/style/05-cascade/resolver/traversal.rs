use super::*;

// Helper functions for DOM-based style resolution
use crate::style::matching::selectors::AncestorFilter;

pub(super) struct InlineStyleCache<'css> {
    node_style_ids: Vec<Option<u32>>,
    styles: Vec<StyleAttribute<'css>>,
}

impl<'css> InlineStyleCache<'css> {
    pub(super) fn new(doc: &Document) -> Self {
        let mut node_style_ids = vec![None; doc.node_count()];
        let mut styles = Vec::new();
        let mut parsed = FxHashMap::<&str, Option<u32>>::default();
        for node_idx in doc.node_ids() {
            let Some(source) = doc
                .get_dom_attr(node_idx, "style")
                .filter(|source| !source.trim().is_empty())
            else {
                continue;
            };
            let style_id = if let Some(style_id) = parsed.get(source) {
                *style_id
            } else {
                let normalized = normalize_declarations(source);
                let normalized: &'css str = Box::leak(normalized.into_boxed_str());
                let style_id = StyleAttribute::parse(
                    normalized,
                    ParserOptions {
                        error_recovery: true,
                        ..ParserOptions::default()
                    },
                )
                .ok()
                .map(|style| {
                    let id = u32::try_from(styles.len()).expect("inline style count fits in u32");
                    styles.push(style);
                    id
                });
                parsed.insert(source, style_id);
                style_id
            };
            node_style_ids[node_idx.index()] = style_id;
        }
        Self {
            node_style_ids,
            styles,
        }
    }

    pub(super) fn get(&self, node_idx: DomNodeId) -> Option<&StyleAttribute<'css>> {
        let id = self.node_style_ids.get(node_idx.index()).copied().flatten()?;
        self.styles.get(id as usize)
    }
}

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
