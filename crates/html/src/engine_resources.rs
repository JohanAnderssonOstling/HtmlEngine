//! Per-document resources and intrinsic-size invalidation, independent of
//! which parts of the document a host chooses to display.

use html_pipeline::engine::pagination_queries;
use html_pipeline::engine::fragment_paint;
use html_dom as dom_values;
use html_pipeline::engine::text_paint;
use crate::Document;
use crate::export::SourceImage;
use crate::layout::{LineId, RenderView};
use crate::pipeline::ResourceRevision;
use crate::resources::{DecodedImages, ImageService, ResourceProvider};
use crate::scoped::ScopedContent;
use html_pipeline::engine::{ImageId, ImageMetrics};
use html_resources::ImagePipeline;
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

/// Completed resource work. The engine retains pending intrinsic dimensions;
/// hosts can preserve reading position before applying a document update.
#[derive(Debug)]
pub struct ResourcePoll {
    pub decoded: bool,
    pub layout_needed: bool,
}

/// Decoded images and the intrinsic dimensions awaiting acceptance by the
/// document. Pending images and their merged metrics change together.
pub(crate) struct DocumentImages {
    pipeline: ImagePipeline,
    resource_revision: Option<ResourceRevision>,
    pending: Vec<ImageId>,
    pending_metrics: Option<ImageMetrics>,
}

impl DocumentImages {
    pub(crate) fn new(provider: Arc<dyn ResourceProvider>) -> Self {
        Self {
            pipeline: ImagePipeline::new(provider),
            resource_revision: None,
            pending: Vec::new(),
            pending_metrics: None,
        }
    }

    /// Metrics to join the next document update, when layout needs them.
    pub(crate) fn pending_metrics(&self) -> Option<&ImageMetrics> {
        self.pending_metrics.as_ref()
    }

    /// The document accepted or discarded the pending dimensions.
    pub(crate) fn clear_pending(&mut self) {
        self.pending.clear();
        self.pending_metrics = None;
    }

    fn replace_service(&mut self, service: ImageService) {
        self.pipeline = ImagePipeline::with_service(service);
        self.clear_pending();
    }

    fn has_pending(&self, committed: Option<&ImageMetrics>) -> bool {
        self.pipeline.has_pending()
            || self.pending.iter().any(|&image| {
                self.pipeline
                    .intrinsic_dimensions(image)
                    .is_some_and(|dimensions| {
                        committed.and_then(|metrics| metrics.get(dom_values::image_id_raw(image))) != Some(dimensions)
                    })
            })
    }

    /// Adopt the committed resources. Returns whether decoded resources were
    /// discarded because the document's resource revision changed.
    pub(crate) fn sync(
        &mut self,
        revision: Option<ResourceRevision>,
        view: Option<RenderView<'_>>,
    ) -> bool {
        let invalidated = self.resource_revision.is_some() && self.resource_revision != revision;
        if invalidated {
            self.pipeline.invalidate_resources();
            self.clear_pending();
        }
        self.resource_revision = revision;
        if let Some(view) = view {
            self.pipeline
                .update_resources(text_paint::render_view_images(&view));
            let pipeline = &self.pipeline;
            self.pending
                .retain(|&image| pipeline.intrinsic_dimensions(image).is_some());
        }
        invalidated
    }

    /// Merge newly decoded dimensions; quiet polls retain the pending snapshot.
    /// Shared-service invalidation discards dimensions no longer available.
    fn poll(&mut self, view: Option<RenderView<'_>>, committed: Option<&ImageMetrics>) -> bool {
        let poll = view
            .map(|view| self.pipeline.poll(text_paint::render_view_images(&view)))
            .unwrap_or_default();
        let previous_pending = self.pending.len();
        let pipeline = &self.pipeline;
        self.pending
            .retain(|&image| pipeline.intrinsic_dimensions(image).is_some());
        if !poll.dimensions_changed.is_empty() || self.pending.len() != previous_pending {
            self.pending.extend_from_slice(&poll.dimensions_changed);
            self.pending.sort_unstable();
            self.pending.dedup();
            self.pending_metrics = committed.zip(view).and_then(|(committed, view)| {
                changed_image_metrics(
                    committed,
                    &self.pending,
                    |image| {
                        text_paint::render_view_images(&view)
                            .get(image.index())
                            .map(|source| (source.width, source.height))
                    },
                    |image| self.pipeline.intrinsic_dimensions(image),
                )
            });
            if self.pending_metrics.is_none() {
                self.pending.clear();
            }
        }
        poll.decoded
    }
}

impl Document {
    pub fn image_source_request(&self, image: ImageId) -> Option<ImageSourceRequest> {
        Some(ImageSourceRequest {
            uri: text_paint::render_view_image_uri(&self.render_view()?, image)?.to_owned(),
            provider: self.provider.clone(),
        })
    }

    pub fn set_image_service(&mut self, service: ImageService) {
        self.images.replace_service(service);
        self.revisions.invalidate_resources();
    }

    /// Decoded image access for painting and export; loading stays with the engine.
    pub fn images(&self) -> DecodedImages<'_> {
        self.images.pipeline.decoded_images()
    }

    /// Includes decoded dimensions awaiting acceptance by the document, so a
    /// deferred or failed layout update remains observable to the host.
    pub fn resources_pending(&self) -> bool {
        self.images
            .has_pending(self.session.inputs().map(|inputs| &inputs.image_metrics))
    }


    /// Request images for this page and a number of page-sized lookahead windows,
    /// together with independently laid-out scopes. Timing and lookahead are host policy.
    pub fn request_images<'a>(
        &mut self,
        page: Option<&crate::pagination::PagePositions>,
        lookahead: usize,
        scopes: impl IntoIterator<Item = &'a ScopedContent>,
    ) {
        let mut lines = Vec::new();
        if let Some((view, page)) = self.session.render_view().zip(page)
            && let Some(end) = page.last_line
        {
            let pagination = pagination_queries::render_view_pagination(view);
            let extra = end
                .saturating_sub(page.start_line)
                .saturating_add(1)
                .saturating_mul(lookahead);
            let next_end = end
                .saturating_add(extra)
                .min(pagination.len().saturating_sub(1));
            let range = page.start_line..next_end.saturating_add(1);
            for group in pagination.groups(range.clone()) {
                lines.extend(
                    group
                        .lines()
                        .zip(group.cursors())
                        .filter_map(|(line, cursor)| range.contains(&cursor).then_some(line.id())),
                );
            }
        }
        self.request_image_lines(lines, scopes);
    }

    pub(crate) fn request_image_lines<'a>(
        &mut self,
        lines: impl IntoIterator<Item = LineId>,
        scopes: impl IntoIterator<Item = &'a ScopedContent>,
    ) {
        let Some(view) = self.session.render_view() else {
            return;
        };
        let mut desired = HashSet::new();
        for line in lines {
            desired.extend(
                fragment_paint::render_fragment_view_images_for_line(view.fragments(), line)
                    .iter()
                    .map(|image| image.image()),
            );
        }
        for scope in scopes {
            if let Some(scope) = scope.render_view(self) {
                desired.extend(
                    fragment_paint::render_fragment_view_images(scope.fragments())
                        .iter()
                        .map(|image| image.image()),
                );
            }
        }
        self.images
            .pipeline
            .ensure_window(text_paint::render_view_images(&view), &desired);
    }

    pub(super) fn sync_resources(&mut self) {
        let revision = self.session.inputs().map(|inputs| inputs.resource_revision());
        if self.images.sync(revision, self.session.render_view()) {
            self.revisions.invalidate_resources();
        }
    }

    pub fn poll_resources(&mut self) -> ResourcePoll {
        let decoded = self.images.poll(
            self.session.render_view(),
            self.session.inputs().map(|inputs| &inputs.image_metrics),
        );
        if decoded {
            self.revisions.invalidate_resources();
        }
        if let Some(metrics) = self.images.pending_metrics()
            && self
                .session
                .accept_image_metrics_if_layout_unchanged(metrics)
        {
            self.images.clear_pending();
        }
        ResourcePoll {
            decoded,
            layout_needed: self.images.pending_metrics().is_some(),
        }
    }
}

/// Merge changed natural dimensions. The session decides whether resolved
/// sizing requires geometry work; no changed dimensions produce no snapshot.
fn changed_image_metrics(
    current: &ImageMetrics,
    dimensions_changed: &[ImageId],
    source_sizes: impl Fn(ImageId) -> Option<(u32, u32)>,
    decode_sizes: impl Fn(ImageId) -> Option<(u32, u32)>,
) -> Option<ImageMetrics> {
    let mut next_metrics = current.clone();
    let mut changed = false;

    for &image in dimensions_changed {
        let Some((resource_width, resource_height)) = source_sizes(image) else {
            continue;
        };
        let Some((decoded_width, decoded_height)) = decode_sizes(image) else {
            continue;
        };

        let previous = next_metrics
            .get(dom_values::image_id_raw(image))
            .unwrap_or((resource_width, resource_height));
        if previous != (decoded_width, decoded_height) {
            changed = true;
        }
        next_metrics.set(dom_values::image_id_raw(image), decoded_width, decoded_height);
    }

    changed.then_some(next_metrics)
}
