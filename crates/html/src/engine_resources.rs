//! Per-document resources and intrinsic-size invalidation, independent of
//! which parts of the document a host chooses to display.
use crate::engine::{Engine, Error};
use crate::export::SourceImage;
use crate::layout::GlyphShaper;
use crate::pipeline::{PipelineChanges, PipelineInputs, PipelineUpdate};
use crate::resources::{ImagePipeline, ImageService, ResourceProvider};
use kurbo::Rect;
use std::collections::HashSet;
use std::io;
use std::sync::Arc;

/// A document-resolved media read which can run on a host-selected executor.
pub struct ImageSourceRequest {
    uri: String,
    provider: Arc<dyn ResourceProvider>,
}

impl ImageSourceRequest {
    pub fn read(self) -> io::Result<SourceImage> {
        let bytes = self.provider.read_bytes(&self.uri)?;
        Ok(SourceImage {
            uri: self.uri,
            bytes,
        })
    }
}

/// Work completed by the resource service. Adapters with reading-position
/// preservation can submit `changes` through their own pipeline-update path.
#[derive(Debug)]
pub struct ResourcePoll {
    pub decoded: bool,
    pub changes: Option<PipelineChanges>,
}

#[derive(Debug)]
pub struct ResourceUpdate {
    pub decoded: bool,
    pub pipeline: Option<PipelineUpdate>,
}

impl Engine {
    pub fn image_source_request(&self, image: u32) -> Option<ImageSourceRequest> {
        Some(ImageSourceRequest {
            uri: self.render_view()?.image_uri(image)?.to_owned(),
            provider: self.provider.clone(),
        })
    }

    pub fn set_image_service(&mut self, service: ImageService) {
        self.images = ImagePipeline::with_service(service);
        self.pending_image_metrics.clear();
        self.revisions.invalidate_resources();
    }

    pub fn images(&self) -> &ImagePipeline {
        &self.images
    }

    /// Includes decoded dimensions awaiting acceptance by the document, so a
    /// deferred or failed layout update remains observable to the host.
    pub fn resources_pending(&self) -> bool {
        self.images.has_pending()
            || self.pending_image_metrics.iter().any(|&image| {
                self.images
                    .intrinsic_dimensions(image)
                    .is_some_and(|dimensions| {
                        self.session
                            .inputs()
                            .and_then(|inputs| inputs.image_metrics.get(image))
                            != Some(dimensions)
                    })
            })
    }

    /// Request a host-chosen working set. The engine binds IDs to this
    /// document's resources; the host owns visibility and prefetch policy.
    pub fn request_images(&mut self, desired: &HashSet<u32>) {
        if let Some(view) = self.session.render_view() {
            self.images.ensure_window(view.images(), desired);
        }
    }

    /// Replace the resource working set using caller-selected document lines.
    pub fn request_images_for_lines(&mut self, lines: impl IntoIterator<Item = usize>) {
        let desired = self
            .content()
            .map(|content| content.image_ids_for_lines(lines))
            .unwrap_or_default();
        self.request_images(&desired);
    }

    /// Replace the resource working set using a caller-selected document region.
    pub fn request_images_in(&mut self, region: Rect) {
        let desired = self
            .content()
            .map(|content| content.image_ids_in(region))
            .unwrap_or_default();
        self.request_images(&desired);
    }

    pub fn release_resources(&mut self) {
        self.images.deactivate();
        self.revisions.invalidate_resources();
    }

    pub(super) fn sync_resources(&mut self) {
        if let Some(view) = self.session.render_view() {
            self.images.update_resources(view.images());
            self.pending_image_metrics
                .retain(|&image| self.images.intrinsic_dimensions(image).is_some());
        }
    }

    pub fn poll_resources(&mut self) -> ResourcePoll {
        let poll = self.images.poll();
        if poll.decoded {
            self.revisions.invalidate_resources();
        }
        self.pending_image_metrics
            .extend_from_slice(&poll.dimensions_changed);
        self.pending_image_metrics.sort_unstable();
        self.pending_image_metrics.dedup();
        let changes = self
            .session
            .inputs()
            .zip(self.session.render_view())
            .and_then(|(inputs, view)| {
                if self.pending_image_metrics.is_empty() {
                    return None;
                }
                let (changes, needed) = image_metric_changes(
                    inputs,
                    &self.pending_image_metrics,
                    |index| {
                        view.images().get(index as usize).map(|image| {
                            (
                                image.width,
                                image.height,
                                image.width_attr,
                                image.height_attr,
                            )
                        })
                    },
                    |index| self.images.intrinsic_dimensions(index),
                );
                needed.then_some(changes)
            });
        if changes.is_none() {
            self.pending_image_metrics.clear();
        }
        ResourcePoll {
            decoded: poll.decoded,
            changes,
        }
    }

    /// Incorporate decoded image dimensions into layout. No repaint, viewport
    /// movement, executor, or UI callback is chosen by the engine.
    pub fn update_resources(
        &mut self,
        shaper: &mut impl GlyphShaper,
    ) -> Result<ResourceUpdate, Error> {
        let poll = self.poll_resources();
        let pipeline = poll
            .changes
            .map(|changes| self.apply_changes(changes, shaper))
            .transpose()?;
        Ok(ResourceUpdate {
            decoded: poll.decoded,
            pipeline,
        })
    }
}

pub fn image_metric_changes(
    pipeline_inputs: &PipelineInputs,
    dimensions_changed: &[u32],
    image_metadata: impl Fn(u32) -> Option<(u32, u32, Option<u32>, Option<u32>)>,
    decode_sizes: impl Fn(u32) -> Option<(u32, u32)>,
) -> (PipelineChanges, bool) {
    let mut next_inputs = pipeline_inputs.image_metrics.clone();
    let mut layout_needed = false;

    for image_idx in dimensions_changed {
        let (resource_width, resource_height, width_attr, height_attr) =
            match image_metadata(*image_idx) {
                Some((resource_width, resource_height, width_attr, height_attr)) => {
                    (resource_width, resource_height, width_attr, height_attr)
                }
                None => continue,
            };
        let Some((decoded_width, decoded_height)) = decode_sizes(*image_idx) else {
            continue;
        };

        let uses_intrinsic_width = width_attr.is_none();
        let uses_intrinsic_height = height_attr.is_none();
        let previous = next_inputs
            .get(*image_idx)
            .unwrap_or((resource_width, resource_height));
        if (uses_intrinsic_width && previous.0 != decoded_width)
            || (uses_intrinsic_height && previous.1 != decoded_height)
        {
            layout_needed = true;
        }
        next_inputs.set(*image_idx, decoded_width, decoded_height);
    }

    (
        PipelineChanges {
            image_metrics: Some(next_inputs),
            image_metrics_revision: layout_needed
                .then(|| pipeline_inputs.image_metrics_revision.next()),
            ..Default::default()
        },
        layout_needed,
    )
}
