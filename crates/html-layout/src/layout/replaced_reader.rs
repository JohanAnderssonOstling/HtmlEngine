use crate::layout::LayoutReader;
use html_dom::Document;
use html_style_model::UsedPreferredSize as PreferredSize;
use kurbo::Size;

/// DOM and renderer-metric access needed to size replaced content.
pub(crate) struct ReplacedReader<'input> {
    document: &'input Document,
    image_metrics: &'input crate::ImageMetrics,
}

impl<'input> ReplacedReader<'input> {
    pub(crate) fn new(document: &'input Document, image_metrics: &'input crate::ImageMetrics) -> Self {
        Self { document, image_metrics }
    }

    pub(crate) fn document(&self) -> &'input Document {
        self.document
    }

    pub(crate) fn box_dom_element_idx(&self, reader: &LayoutReader<'_>, box_idx: usize) -> Option<u32> {
        reader.box_at(box_idx)?.dom_element()
    }

    pub(crate) fn image_display_size(&self, image_idx: u32) -> (f64, f64) {
        self.image_metrics.display_size(image_idx, self.document.image(image_idx).expect("image index should be valid"))
    }

    pub(crate) fn intrinsic_size(&self, reader: &LayoutReader<'_>, box_idx: usize) -> Option<Size> {
        let node_idx = self.box_dom_element_idx(reader, box_idx)?;
        let dom_node = self.document.node_id_from_raw(node_idx)?;
        let element = self.document.element_ref(dom_node)?;
        self.element_intrinsic_size(element)
    }

    pub(crate) fn intrinsic_ratio(&self, reader: &LayoutReader<'_>, box_idx: usize, intrinsic: Size) -> Option<f64> {
        let node_idx = self.box_dom_element_idx(reader, box_idx)?;
        let dom_node = self.document.node_id_from_raw(node_idx)?;
        let element = self.document.element_ref(dom_node)?;
        if element.tag().eq_ignore_ascii_case("svg") {
            let view_box = element.attr("viewBox").or_else(|| element.attr("viewbox"))?;
            let values = view_box.split(|character: char| character.is_ascii_whitespace() || character == ',').filter(|value| !value.is_empty()).map(str::parse::<f64>).collect::<Result<Vec<_>, _>>().ok()?;
            if values.len() != 4 || values[2] <= 0.0 || values[3] <= 0.0 {
                return None;
            }
            return Some(values[2] / values[3]);
        }
        (intrinsic.height > 0.0).then_some(intrinsic.width / intrinsic.height)
    }

    /// Returns a renderer-supplied column-filling width for an eligible
    /// standalone image, capped so its outer height fits the reader viewport.
    /// This remains outside CSS style resolution.
    pub(crate) fn smart_standalone_image_width(
        &self, reader: &LayoutReader<'_>, policy: crate::ImageSizingPolicy, box_idx: usize, intrinsic: Size, available_width: f64, viewport_height: Option<f64>, horizontal_padding_border: f64, vertical_outer_inset: f64, standalone: bool,
    ) -> Option<PreferredSize> {
        const MIN_SOURCE_WIDTH: f64 = 96.0;

        if policy != crate::ImageSizingPolicy::SmartStandalone || !standalone || intrinsic.width < MIN_SOURCE_WIDTH {
            return None;
        }
        let style = reader.style(box_idx);
        if style.height() != PreferredSize::Auto || style.min_height() != PreferredSize::Auto || style.max_height() != PreferredSize::Auto {
            return None;
        }

        let node_idx = self.box_dom_element_idx(reader, box_idx)?;
        let dom_node = self.document.node_id_from_raw(node_idx)?;
        let element = self.document.element_ref(dom_node)?;
        if element.image_idx().is_none() || element.attr("height").is_some() {
            return None;
        }

        let mut expanded_content_width = (available_width - horizontal_padding_border).max(0.0);
        if intrinsic.height > 0.0
            && let Some(viewport_height) = viewport_height
        {
            let available_content_height = (viewport_height - vertical_outer_inset).max(0.0);
            let height_limited_width = available_content_height * intrinsic.width / intrinsic.height;
            expanded_content_width = expanded_content_width.min(height_limited_width);
        }
        let preferred = match style.box_sizing() {
            html_style_model::BoxSizing::ContentBox => expanded_content_width,
            html_style_model::BoxSizing::BorderBox => expanded_content_width + horizontal_padding_border,
        };
        Some(PreferredSize::Px(preferred as f32))
    }

    pub(crate) fn percentage_height_width(&self, reader: &LayoutReader<'_>, box_idx: usize, content_height: f64) -> Option<f64> {
        let node_idx = self.box_dom_element_idx(reader, box_idx)?;
        let dom_node = self.document.node_id_from_raw(node_idx)?;
        let element = self.document.element_ref(dom_node)?;
        let mut width: Option<f64> = None;
        for child_id in element.children() {
            let Some(child) = self.document.element_ref(child_id) else { continue };
            let Some(intrinsic) = self.element_intrinsic_size(child) else { continue };
            let Some(child_box_idx) = (0..reader.box_count()).find(|&candidate| reader.box_at(candidate).and_then(|layout_box| layout_box.dom_element()) == Some(child.node_id().raw())) else { continue };
            let child_style = reader.style(child_box_idx);
            let PreferredSize::Percent(percent) = child_style.height() else { continue };
            if intrinsic.height <= 0.0 {
                continue;
            }
            let used_height = content_height * percent.max(0.0) as f64;
            let content_width = used_height * intrinsic.width / intrinsic.height;
            let outer_width = content_width + child_style.get_horizontal_margin_padding(0.0) + child_style.border_left_width() as f64 + child_style.border_right_width() as f64;
            width = Some(width.map_or(outer_width, |current| current.max(outer_width)));
        }
        width
    }

    fn element_intrinsic_size(&self, element: html_dom::ElementRef<'_>) -> Option<Size> {
        if let Some(image_idx) = element.image_idx() {
            let (width, height) = self.image_display_size(image_idx);
            return Some(Size::new(width, height));
        }
        if !element.tag().eq_ignore_ascii_case("canvas") {
            return None;
        }
        let width = element.attr("width").and_then(|value| value.parse::<u32>().ok()).unwrap_or(300);
        let height = element.attr("height").and_then(|value| value.parse::<u32>().ok()).unwrap_or(150);
        Some(Size::new(width as f64, height as f64))
    }
}
