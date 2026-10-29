use html_dom::{Document, ImageResource, ImageSource};
use std::sync::Arc;

/// Intrinsic image sizes shared by input and layout snapshots. Updating one
/// snapshot preserves sizes retained by previously published layouts.
#[derive(Clone, Debug, Default)]
pub struct ImageMetrics {
    sizes: Arc<Vec<(u32, u32)>>,
}

impl ImageMetrics {
    pub fn from_document(document: &Document) -> Self {
        let mut sizes = Vec::with_capacity(document.images().len());
        for image in document.images() {
            sizes.push((image.width, image.height));
        }
        Self {
            sizes: Arc::new(sizes),
        }
    }

    pub fn set(&mut self, image_idx: u32, width: u32, height: u32) {
        if self.get(image_idx) == Some((width, height)) {
            return;
        }
        let idx = image_idx as usize;
        let sizes = Arc::make_mut(&mut self.sizes);
        if sizes.len() <= idx {
            sizes.resize(idx + 1, (0, 0));
        }
        sizes[idx] = (width, height);
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
    use std::sync::Arc;

    #[test]
    fn snapshots_share_unchanged_sizes_and_isolate_updates_and_growth() {
        let mut original = ImageMetrics::default();
        original.set(0, 320, 180);
        let mut snapshot = original.clone();
        assert!(Arc::ptr_eq(&original.sizes, &snapshot.sizes));
        snapshot.set(0, 320, 180);
        assert!(Arc::ptr_eq(&original.sizes, &snapshot.sizes));

        snapshot.set(0, 640, 360);
        assert_eq!(original.get(0), Some((320, 180)));
        assert_eq!(snapshot.get(0), Some((640, 360)));
        assert!(!Arc::ptr_eq(&original.sizes, &snapshot.sizes));

        let resized = snapshot.clone();
        snapshot.set(2, 120, 80);
        assert_eq!(resized.len(), 1);
        assert_eq!(resized.get(2), None);
        assert_eq!(snapshot.get(1), Some((0, 0)));
        assert_eq!(snapshot.get(2), Some((120, 80)));
        let storage = snapshot.sizes.as_ptr();
        snapshot.set(2, 240, 160);
        assert_eq!(
            snapshot.sizes.as_ptr(),
            storage,
            "unique updates reuse storage"
        );
        assert_eq!(snapshot.get(2), Some((240, 160)));
    }

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
