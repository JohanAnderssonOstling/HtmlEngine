use html_dom::{Document, ImageResource, ImageSource};

#[derive(Clone, Debug, Default)]
pub struct ImageMetrics {
    sizes: Vec<(u32, u32)>,
}

impl ImageMetrics {
    pub fn from_document(document: &Document) -> Self {
        let mut sizes = Vec::with_capacity(document.images().len());
        for image in document.images() {
            sizes.push((image.width, image.height));
        }
        Self { sizes }
    }

    pub fn set(&mut self, image_idx: u32, width: u32, height: u32) {
        let idx = image_idx as usize;
        if self.sizes.len() <= idx {
            self.sizes.resize(idx + 1, (0, 0));
        }
        self.sizes[idx] = (width, height);
    }

    pub fn get(&self, image_idx: u32) -> Option<(u32, u32)> {
        self.sizes.get(image_idx as usize).copied()
    }

    pub fn display_size(&self, image_idx: u32, resource: &ImageResource) -> (f64, f64) {
        // Formula dimensions belong to the current font-size preparation,
        // not to a possibly older asynchronous SVG decode.
        let (width, height) = if matches!(resource.source, ImageSource::MathMl { .. }) {
            (resource.width, resource.height)
        } else {
            self.get(image_idx)
                .unwrap_or((resource.width, resource.height))
        };
        // HTML dimension attributes are CSS presentational hints, not natural
        // image dimensions. They are resolved by the style/layout sizing path;
        // folding them into the decoded metrics here would also change the
        // intrinsic aspect ratio and apply the same author input twice.
        (width.max(1) as f64, height.max(1) as f64)
    }

    pub fn memory_usage_bytes(&self) -> usize {
        self.sizes.capacity() * std::mem::size_of::<(u32, u32)>()
    }

    pub fn len(&self) -> usize {
        self.sizes.len()
    }
}

#[cfg(test)]
mod image_metrics_tests {
    use super::ImageMetrics;
    use html_dom::{ImageResource, ImageSource};

    #[test]
    fn html_dimensions_do_not_replace_decoded_intrinsic_metrics() {
        let resource = ImageResource {
            source: ImageSource::Uri("image.png".to_owned()),
            width: 320,
            height: 180,
            width_attr: Some(10),
            height_attr: Some(20),
        };
        let mut metrics = ImageMetrics::default();

        assert_eq!(metrics.display_size(0, &resource), (320.0, 180.0));
        metrics.set(0, 640, 360);
        assert_eq!(metrics.display_size(0, &resource), (640.0, 360.0));
    }
}
