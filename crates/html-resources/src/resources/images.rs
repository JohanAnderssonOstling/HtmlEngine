use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::io::{self, Cursor};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LockResult, Mutex, MutexGuard, Weak};

use base64::{Engine, engine::general_purpose::STANDARD};
use peniko::{Blob, Format, Image as PenikoImage};
use quick_xml::{Reader, XmlVersion};
use quick_xml::Writer;
use quick_xml::events::{BytesStart, Event};
use sha2::{Digest, Sha256};

use crate::document::{ImageResource, ImageSource};
use crate::resources::{
    ResourceMetadata, ResourceProvider, has_uri_scheme, media_type_essence, uri_path,
};

#[derive(Clone)]
pub enum DecodedImage {
    Raster {
        image: PenikoImage,
        hash: Vec<u8>,
        byte_len: usize,
    },
    Svg {
        bytes: Arc<[u8]>,
        hash: Vec<u8>,
        width: u32,
        height: u32,
    },
}

impl DecodedImage {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Raster { image, .. } => (image.width, image.height),
            Self::Svg { width, height, .. } => (*width, *height),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum ImageKey {
    Uri(String),
    InlineSvg { hash: [u8; 32], base_uri: String },
}

impl ImageKey {
    fn matches_source(&self, source: &ImageSource) -> bool {
        match (self, source) {
            (Self::Uri(previous), ImageSource::Uri(uri)) => previous == uri,
            (Self::InlineSvg { hash, base_uri }, ImageSource::MathMl { svg, .. }) => {
                base_uri.is_empty() && hash[..] == Sha256::digest(svg)[..]
            }
            (Self::InlineSvg { hash, base_uri }, ImageSource::InlineSvg { bytes, base_uri: current }) => {
                base_uri == current && hash[..] == Sha256::digest(bytes)[..]
            }
            _ => false,
        }
    }

    fn for_source(source: &ImageSource) -> Self {
        match source {
            ImageSource::Uri(uri) => Self::Uri(uri.clone()),
            ImageSource::MathMl { svg, .. } => Self::InlineSvg {
                hash: Sha256::digest(svg).into(),
                base_uri: String::new(),
            },
            ImageSource::InlineSvg { bytes, base_uri } => Self::InlineSvg {
                hash: Sha256::digest(bytes).into(),
                base_uri: base_uri.clone(),
            },
        }
    }
}

/// One finite CPU or blocking-I/O operation owned by the renderer core but
/// scheduled by its host platform.
pub type BlockingJob = Box<dyn FnOnce() + Send + 'static>;

/// Runtime-neutral execution port for resource loading and image decoding.
///
/// Implementations must arrange for every submitted job to run exactly once.
/// They may execute it inline, enqueue it on a native pool, or transfer it to a
/// Web Worker. Keeping this contract here prevents the renderer from choosing
/// a threading or async runtime on behalf of its host.
pub trait BlockingJobSpawner: Send + Sync {
    fn spawn(&self, job: BlockingJob);
}

/// Deterministic fallback for tests and synchronous hosts.
///
/// Interactive adapters should normally provide a background implementation,
/// but inline execution is always valid and keeps unsupported platforms from
/// failing merely because they do not implement `std::thread`.
#[derive(Clone, Copy, Debug, Default)]
pub struct InlineJobSpawner;

impl BlockingJobSpawner for InlineJobSpawner {
    fn spawn(&self, job: BlockingJob) {
        job();
    }
}

/// Completes one image-byte request. Loaders must call this exactly once,
/// including on failure. The engine schedules decoding after completion.
pub type ImageLoadCompletion = Box<dyn FnOnce(std::io::Result<Vec<u8>>) + Send + 'static>;

/// Runtime-neutral image I/O boundary. Return promptly after scheduling work;
/// the host may use asynchronous network I/O or a blocking storage worker.
/// Inline SVG results must include their referenced image resources.
pub trait ImageLoader: Send + Sync {
    fn load(&self, source: ImageSource, complete: ImageLoadCompletion);
}

/// Prepared context for asynchronous image loading. Both operations must use
/// only memory: resolve paths lexically and look up already cached metadata.
/// Hosts whose providers need I/O must prepare this context on a worker first.
/// Resource bytes are fetched separately by the async loader's read callback.
///
/// An I/O provider is deliberately not an async image context:
/// ```compile_fail
/// use html_resources::{FileSystemProvider, ImageResourceContext};
/// let provider = FileSystemProvider::new();
/// let context: &dyn ImageResourceContext = &provider;
/// ```
pub trait ImageResourceContext: Send + Sync {
    fn resolve(&self, base: &str, href: &str) -> String;
    fn metadata(&self, uri: &str) -> io::Result<ResourceMetadata>;
}

/// Compatibility loader for synchronous resource providers. Interactive hosts
/// supply their existing background job spawner; no runtime is created here.
pub struct ProviderImageLoader {
    provider: Arc<dyn ResourceProvider>,
    jobs: Arc<dyn BlockingJobSpawner>,
}

impl ProviderImageLoader {
    pub fn new(provider: Arc<dyn ResourceProvider>, jobs: Arc<dyn BlockingJobSpawner>) -> Self {
        Self { provider, jobs }
    }
}

impl ImageLoader for ProviderImageLoader {
    fn load(&self, source: ImageSource, complete: ImageLoadCompletion) {
        let provider = self.provider.clone();
        self.jobs.spawn(Box::new(move || {
            complete(load_image_bytes(provider.as_ref(), &source))
        }));
    }
}

struct SharedImage {
    decoded: Arc<DecodedImage>,
    byte_len: usize,
    revision: u64,
    last_used: u64,
}

struct SharedState {
    decoded: HashMap<ImageKey, SharedImage>,
    pending: HashSet<ImageKey>,
    failed: HashMap<ImageKey, (u64, Arc<ImageLoadError>)>,
    decoded_bytes: usize,
    byte_budget: usize,
    clock: u64,
    revision: u64,
    completion_waker: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct ImageServiceState {
    epoch: AtomicU64,
    inner: Mutex<SharedState>,
}

impl ImageServiceState {
    fn lock(&self) -> LockResult<MutexGuard<'_, SharedState>> {
        self.inner.lock()
    }
}

/// A completed resource request that requires an explicit retry.
#[derive(Debug)]
pub enum ImageLoadError {
    Read(io::Error),
    Decode,
}

impl fmt::Display for ImageLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => error.fmt(f),
            Self::Decode => f.write_str("unsupported or invalid image data"),
        }
    }
}

impl Error for ImageLoadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Decode => None,
        }
    }
}

impl SharedState {
    fn image(&mut self, key: &ImageKey) -> Option<&SharedImage> {
        self.clock = self.clock.wrapping_add(1);
        let image = self.decoded.get_mut(key)?;
        image.last_used = self.clock;
        Some(image)
    }
}

/// Source-keyed image jobs and decoded-image memory shared by every document
/// in one reader. Cloning this handle creates no workers.
#[derive(Clone)]
pub struct ImageService {
    state: Arc<ImageServiceState>,
    loader: Arc<dyn ImageLoader>,
    jobs: Arc<dyn BlockingJobSpawner>,
}

impl ImageService {
    pub const DEFAULT_BYTE_BUDGET: usize = 128 * 1024 * 1024;
    /// Bound retained failures independently of decoded image bytes.
    const MAX_FAILURES: usize = 256;

    pub fn new(provider: Arc<dyn ResourceProvider>, byte_budget: usize) -> Self {
        Self::with_job_spawner(provider, Arc::new(InlineJobSpawner), byte_budget)
    }

    pub fn with_job_spawner(
        provider: Arc<dyn ResourceProvider>,
        jobs: Arc<dyn BlockingJobSpawner>,
        byte_budget: usize,
    ) -> Self {
        let loader = Arc::new(ProviderImageLoader::new(provider, jobs.clone()));
        Self::with_loader(loader, jobs, byte_budget)
    }

    pub fn with_loader(
        loader: Arc<dyn ImageLoader>,
        jobs: Arc<dyn BlockingJobSpawner>,
        byte_budget: usize,
    ) -> Self {
        let state = Arc::new(ImageServiceState {
            epoch: AtomicU64::new(0),
            inner: Mutex::new(SharedState {
                decoded: HashMap::new(),
                pending: HashSet::new(),
                failed: HashMap::new(),
                decoded_bytes: 0,
                byte_budget,
                clock: 0,
                revision: 0,
                completion_waker: None,
            }),
        });
        Self {
            state,
            loader,
            jobs,
        }
    }

    pub fn set_completion_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.state
            .lock()
            .expect("image service mutex poisoned")
            .completion_waker = waker;
    }

    fn request(&self, source: &ImageSource, key: ImageKey) {
        let epoch = {
            let mut state = self.state.lock().expect("image service mutex poisoned");
            if state.decoded.contains_key(&key)
                || state.failed.contains_key(&key)
                || !state.pending.insert(key.clone())
            {
                return;
            }
            self.state.epoch.load(Ordering::Relaxed)
        };
        let state = Arc::downgrade(&self.state);
        let jobs = self.jobs.clone();
        self.loader.load(
            source.clone(),
            Box::new(move |bytes| {
                if state.strong_count() == 0 {
                    return;
                }
                jobs.spawn(Box::new(move || {
                    execute_image_job(state, epoch, key, bytes)
                }));
            }),
        );
    }

    fn get(&self, key: &ImageKey) -> Option<Arc<DecodedImage>> {
        let mut state = self.state.lock().expect("image service mutex poisoned");
        Some(state.image(key)?.decoded.clone())
    }

    fn trim(&self) {
        let mut state = self.state.lock().expect("image service mutex poisoned");
        trim_decoded_cache(&mut state, None);
    }

    fn invalidate(&self) {
        let mut state = self.state.lock().expect("image service mutex poisoned");
        self.state.epoch.fetch_add(1, Ordering::Release);
        state.decoded.clear();
        state.pending.clear();
        state.failed.clear();
        state.decoded_bytes = 0;
        let waker = state.completion_waker.clone();
        drop(state);
        wake_completion(waker);
    }

    fn wake_completion(&self) {
        let waker = self
            .state
            .lock()
            .expect("image service mutex poisoned")
            .completion_waker
            .clone();
        wake_completion(waker);
    }
}

struct ImageRequest {
    key: ImageKey,
    seen_revision: Option<u64>,
}

/// Document-local image state. Image metadata stays with the document and is
/// borrowed by operations; only asynchronous job identities are retained.
pub struct ImagePipeline {
    resource_keys: HashMap<u32, ImageRequest>,
    service: ImageService,
    decoded_cache: HashMap<u32, Arc<DecodedImage>>,
    desired: HashSet<u32>,
    image_dimensions: HashMap<u32, (u32, u32)>,
    service_epoch: u64,
}

#[derive(Debug, Default)]
pub struct ImagePipelinePoll {
    pub decoded: bool,
    pub dimensions_changed: Vec<u32>,
}

impl ImagePipeline {
    /// A failed request stays settled until retried, invalidated, or evicted
    /// from the bounded failure cache. Querying a
    /// failure does not schedule I/O or change the visible image working set.
    pub fn failure(&self, index: u32) -> Option<Arc<ImageLoadError>> {
        let request = self.resource_keys.get(&index)?;
        self.service
            .state
            .lock()
            .expect("image service mutex poisoned")
            .failed
            .get(&request.key)
            .map(|(_, error)| error.clone())
    }

    /// Allow the next working-set request to retry this failed image.
    pub fn retry_failed(&mut self, index: u32) {
        if let Some(request) = self.resource_keys.get(&index) {
            self.service
                .state
                .lock()
                .expect("image service mutex poisoned")
                .failed
                .remove(&request.key);
        }
    }

    pub fn new(provider: Arc<dyn ResourceProvider>) -> Self {
        let service = ImageService::new(provider, ImageService::DEFAULT_BYTE_BUDGET);
        Self::with_service(service)
    }

    pub fn with_service(service: ImageService) -> Self {
        let service_epoch = service.state.epoch.load(Ordering::Acquire);
        Self {
            resource_keys: HashMap::new(),
            service,
            decoded_cache: HashMap::new(),
            desired: HashSet::new(),
            image_dimensions: HashMap::new(),
            service_epoch,
        }
    }

    /// Provider bytes changed, including resources whose URI stayed the same.
    /// All documents sharing this service observe the new epoch before reuse.
    pub fn invalidate_resources(&mut self) {
        self.service.invalidate();
    }

    fn sync_epoch(&mut self) -> bool {
        let epoch = self.service.state.epoch.load(Ordering::Acquire);
        if epoch != self.service_epoch {
            self.decoded_cache.clear();
            for request in self.resource_keys.values_mut() {
                request.seen_revision = None;
            }
            self.image_dimensions.clear();
            self.service_epoch = epoch;
            return true;
        }
        false
    }

    /// Install the current document's resource metadata after a pipeline update.
    /// Unchanged images retain decoded buffers and pending jobs. Changed or
    /// removed sources lose their local state; the next visible window requests
    /// their new content through the shared service.
    pub fn update_resources(&mut self, images: &[ImageResource]) {
        let invalidated = self.sync_epoch();
        self.resource_keys.retain(|&index, request| {
            if images
                .get(index as usize)
                .is_some_and(|image| request.key.matches_source(&image.source))
            {
                return true;
            }
            self.decoded_cache.remove(&index);
            self.desired.remove(&index);
            self.image_dimensions.remove(&index);
            false
        });
        if invalidated && !self.desired.is_empty() {
            let desired = self.desired.clone();
            self.ensure_window(images, &desired);
        }
    }

    /// Poll using the current borrowed catalog. A shared-service invalidation
    /// automatically renews this document's desired requests before polling.
    pub fn poll(&mut self, images: &[ImageResource]) -> ImagePipelinePoll {
        if self.service_epoch != self.service.state.epoch.load(Ordering::Acquire) {
            self.renew_requests(images);
        }
        let mut changed = ImagePipelinePoll::default();
        if self.desired.is_empty() {
            return changed;
        }
        // Keys are retained for the visible window. Polling neither rebuilds
        // SVG hashes nor takes one mutex per image; unchanged revisions borrow
        // the shared entry without incrementing its Arc count.
        let mut state = self.service.state.lock().expect("image service mutex poisoned");
        for &idx in &self.desired {
            let request = self
                .resource_keys
                .get_mut(&idx)
                .expect("desired image request");
            let Some(image) = state.image(&request.key) else {
                continue;
            };
            if request.seen_revision == Some(image.revision) {
                continue;
            }
            request.seen_revision = Some(image.revision);
            let dimensions = image.decoded.dimensions();
            self.decoded_cache.insert(idx, image.decoded.clone());
            changed.decoded = true;
            if self.image_dimensions.insert(idx, dimensions) != Some(dimensions) {
                changed.dimensions_changed.push(idx);
            }
        }
        drop(state);
        changed.dimensions_changed.sort_unstable();
        changed
    }

    #[cold]
    fn renew_requests(&mut self, images: &[ImageResource]) {
        self.update_resources(images);
    }

    /// Request visible images by borrowing their owning document's catalog.
    /// After document resource replacement, call [`Self::update_resources`]
    /// before reusing this pipeline with the new catalog.
    pub fn ensure_window(&mut self, images: &[ImageResource], desired: &HashSet<u32>) {
        self.sync_epoch();
        self.decoded_cache.retain(|idx, _| desired.contains(idx));
        for idx in self.desired.difference(desired) {
            self.resource_keys
                .get_mut(idx)
                .expect("desired image request")
                .seen_revision = None;
        }
        self.desired.retain(|idx| desired.contains(idx));

        let mut shared_cache_hit = false;
        for &idx in desired {
            let Some(resource) = images.get(idx as usize) else {
                continue;
            };
            let request = self
                .resource_keys
                .entry(idx)
                .or_insert_with(|| ImageRequest {
                    key: ImageKey::for_source(&resource.source),
                    seen_revision: None,
                });
            self.desired.insert(idx);
            if self.decoded_cache.contains_key(&idx) {
                continue;
            }
            if let Some(decoded) = self.service.get(&request.key) {
                // Paint a shared hit immediately. Leaving its revision
                // unseen makes `has_pending` schedule one poll so intrinsic
                // dimensions are still reported to this document.
                self.decoded_cache.insert(idx, decoded);
                shared_cache_hit = true;
            } else {
                self.service.request(&resource.source, request.key.clone());
            }
        }
        if shared_cache_hit {
            self.service.wake_completion();
        }
    }

    /// Releases decoded buffers when the owning document leaves the visible
    /// composition. Intrinsic dimensions remain available for relayout, while
    /// the shared service regains sole ownership and may enforce its LRU
    /// budget. A later `ensure_window` reactivates the required images.
    pub fn deactivate(&mut self) {
        self.decoded_cache.clear();
        for idx in &self.desired {
            self.resource_keys
                .get_mut(idx)
                .expect("desired image request")
                .seen_revision = None;
        }
        self.desired.clear();
        self.service.trim();
    }

    pub fn get_decoded(&self, idx: u32) -> Option<&DecodedImage> {
        if self.service_epoch != self.service.state.epoch.load(Ordering::Acquire) {
            return None;
        }
        self.decoded_cache.get(&idx).map(AsRef::as_ref)
    }

    /// Known intrinsic dimensions survive decoded-buffer release and are
    /// invalidated when this index is rebound to another resource source.
    pub fn intrinsic_dimensions(&self, idx: u32) -> Option<(u32, u32)> {
        if self.service_epoch != self.service.state.epoch.load(Ordering::Acquire) {
            return None;
        }
        self.image_dimensions.get(&idx).copied()
    }

    pub fn has_pending(&self) -> bool {
        if self.service_epoch != self.service.state.epoch.load(Ordering::Acquire) {
            return !self.desired.is_empty();
        }
        if self
            .desired
            .iter()
            .all(|idx| self.resource_keys[idx].seen_revision.is_some())
        {
            return false;
        }
        let state = self.service.state.lock().expect("image service mutex poisoned");
        self.desired.iter().any(|idx| {
            self.resource_keys[idx].seen_revision.is_none()
                && (self.decoded_cache.contains_key(idx)
                    || state.pending.contains(&self.resource_keys[idx].key))
        })
    }

}

fn execute_image_job(
    state: Weak<ImageServiceState>,
    epoch: u64,
    key: ImageKey,
    bytes: std::io::Result<Vec<u8>>,
) {
    let Some(state) = state.upgrade() else {
        return;
    };
    if state.epoch.load(Ordering::Acquire) != epoch {
        return;
    }
    let decoded = bytes
        .map_err(ImageLoadError::Read)
        .and_then(|bytes| decode_image(&bytes).map_err(|()| ImageLoadError::Decode));
    let mut locked = state.lock().expect("image service mutex poisoned");
    if state.epoch.load(Ordering::Relaxed) != epoch {
        return;
    }
    let state = &mut *locked;
    state.pending.remove(&key);
    if let Ok(decoded) = decoded {
        let byte_len = match &decoded {
            DecodedImage::Raster { byte_len, .. } => *byte_len,
            DecodedImage::Svg { bytes, .. } => bytes.len(),
        };
        state.clock = state.clock.wrapping_add(1);
        state.revision = state.revision.wrapping_add(1);
        let entry = SharedImage {
            decoded: Arc::new(decoded),
            byte_len,
            revision: state.revision,
            last_used: state.clock,
        };
        if let Some(previous) = state.decoded.insert(key.clone(), entry) {
            state.decoded_bytes = state.decoded_bytes.saturating_sub(previous.byte_len);
        }
        state.decoded_bytes = state.decoded_bytes.saturating_add(byte_len);
        // Keep a newly completed decode available for at least one poll. If
        // the visible working set itself exceeds the cache budget, inactive
        // pipelines retrim it as soon as they release their pins.
        trim_decoded_cache(state, Some(&key));
    } else if let Err(error) = decoded {
        if state.failed.len() >= ImageService::MAX_FAILURES {
            let oldest = state
                .failed
                .iter()
                .min_by_key(|(_, (age, _))| age)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                state.failed.remove(&oldest);
            }
        }
        state.clock = state.clock.wrapping_add(1);
        state.failed.insert(key, (state.clock, Arc::new(error)));
    }
    let waker = state.completion_waker.clone();
    drop(locked);
    wake_completion(waker);
}

fn wake_completion(waker: Option<Arc<dyn Fn() + Send + Sync>>) {
    if let Some(waker) = waker {
        waker();
    }
}

fn trim_decoded_cache(state: &mut SharedState, protected: Option<&ImageKey>) {
    while state.decoded_bytes > state.byte_budget && state.decoded.len() > 1 {
        let candidate = state
            .decoded
            .iter()
            .filter(|(key, image)| {
                protected != Some(*key) && Arc::strong_count(&image.decoded) == 1
            })
            .min_by_key(|(_, image)| image.last_used)
            .map(|(key, _)| key.clone());
        let Some(candidate) = candidate else { break };
        if let Some(evicted) = state.decoded.remove(&candidate) {
            state.decoded_bytes = state.decoded_bytes.saturating_sub(evicted.byte_len);
        }
    }
}

fn decode_image(bytes: &[u8]) -> Result<DecodedImage, ()> {
    if let Some((width, height)) = svg_dimensions(bytes) {
        let hash = Sha256::digest(bytes).to_vec();
        return Ok(DecodedImage::Svg {
            bytes: Arc::from(bytes),
            hash,
            width,
            height,
        });
    }

    let image = image::load_from_memory(bytes).map_err(|_| ())?;
    let width = image.width();
    let height = image.height();
    let rgba = image.into_rgba8().into_vec();
    let byte_len = rgba.len();
    let blob = Blob::new(Arc::new(rgba));
    let peniko = PenikoImage::new(blob, Format::Rgba8, width, height);

    let mut hasher = Sha256::new();
    hasher.update(peniko.data.data());
    let hash = hasher.finalize().to_vec();

    Ok(DecodedImage::Raster {
        image: peniko,
        hash,
        byte_len,
    })
}

/// Inspect intrinsic dimensions from encoded source bytes. Embedded SVG and
/// formula payloads are borrowed; URI sources are read once.
pub fn probe_dimensions(
    provider: &dyn ResourceProvider,
    source: &ImageSource,
) -> Option<(u32, u32)> {
    let bytes: Cow<'_, [u8]> = match source {
        ImageSource::Uri(uri) => Cow::Owned(provider.read_bytes(uri).ok()?),
        ImageSource::MathMl { svg, .. } => Cow::Borrowed(svg),
        ImageSource::InlineSvg { bytes, .. } => Cow::Borrowed(bytes),
    };
    if let Some(dimensions) = svg_dimensions(&bytes) {
        return Some(dimensions);
    }
    if let Ok(reader) = image::ImageReader::new(Cursor::new(bytes.as_ref())).with_guessed_format()
        && let Ok(dimensions) = reader.into_dimensions()
    {
        return Some(dimensions);
    }
    None
}

fn svg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut reader = Reader::from_reader(bytes);
    loop {
        match reader.read_event().ok()? {
            Event::Start(start) | Event::Empty(start) if start.local_name().as_ref() == b"svg" => {
                let mut width = None;
                let mut height = None;
                for attribute in start.attributes().with_checks(false) {
                    let attribute = attribute.ok()?;
                    match attribute.key.local_name().as_ref() {
                        b"width" => width = svg_absolute_length(attribute.value.as_ref()),
                        b"height" => height = svg_absolute_length(attribute.value.as_ref()),
                        _ => {}
                    }
                }
                return Some((width.unwrap_or(300), height.unwrap_or(150)));
            }
            Event::Start(_) | Event::Empty(_) => return None,
            Event::Eof => return None,
            _ => {}
        }
    }
}

fn svg_absolute_length(value: &[u8]) -> Option<u32> {
    let value = std::str::from_utf8(value).ok()?.trim();
    if value.ends_with('%') {
        return None;
    }
    let (number, pixels_per_unit) = [
        ("px", 1.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("q", 96.0 / 101.6),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
    ]
    .into_iter()
    .find_map(|(suffix, scale)| value.strip_suffix(suffix).map(|number| (number, scale)))
    .unwrap_or((value, 1.0));
    let value = number.trim().parse::<f64>().ok()? * pixels_per_unit;
    (value.is_finite() && value > 0.0).then(|| value.round().max(1.0) as u32)
}

const MAX_EMBEDDED_BYTES: usize = 24 * 1024 * 1024;

/// Resolve encoded image bytes, including image references inside inline SVG.
/// Call this on a loading worker when using a synchronous provider.
pub fn load_image_bytes(
    provider: &dyn ResourceProvider,
    source: &ImageSource,
) -> io::Result<Vec<u8>> {
    match source {
        ImageSource::Uri(uri) => provider.read_bytes(uri),
        ImageSource::MathMl { svg, .. } => Ok(svg.to_vec()),
        ImageSource::InlineSvg { bytes, base_uri } => {
            let references =
                svg_image_references(bytes, base_uri, |base, href| provider.resolve(base, href));
            let mut prepared = PreparedSvgResources::new(references);
            for reference in 0..prepared.references.uris.len() {
                if prepared.byte_len >= MAX_EMBEDDED_BYTES {
                    break;
                }
                if let Ok(bytes) = provider.read_bytes(&prepared.references.uris[reference]) {
                    prepared.insert_image(reference, bytes, |uri| provider.metadata(uri));
                }
            }
            Ok(embed_svg_image_resources(&prepared, bytes))
        }
    }
}

/// Fetch image resources asynchronously using a prepared, memory-only context.
/// SVG rewriting consumes only the fetched bytes and cached media types; it has
/// no access to either the context or a synchronous resource provider.
pub async fn load_image_bytes_async<F, Fut>(
    context: &dyn ImageResourceContext,
    source: &ImageSource,
    read: F,
) -> io::Result<Vec<u8>>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = io::Result<Vec<u8>>>,
{
    match source {
        ImageSource::Uri(uri) => read(uri.clone()).await,
        ImageSource::MathMl { svg, .. } => Ok(svg.to_vec()),
        ImageSource::InlineSvg { bytes, base_uri } => {
            let references =
                svg_image_references(bytes, base_uri, |base, href| context.resolve(base, href));
            let mut prepared = PreparedSvgResources::new(references);
            for reference in 0..prepared.references.uris.len() {
                if prepared.byte_len >= MAX_EMBEDDED_BYTES {
                    break;
                }
                if let Ok(bytes) = read(prepared.references.uris[reference].clone()).await {
                    prepared.insert_image(reference, bytes, |uri| context.metadata(uri));
                }
            }
            Ok(embed_svg_image_resources(&prepared, bytes))
        }
    }
}

struct SvgImageReferences {
    by_href: HashMap<String, usize>,
    // Preserve source order so the byte limit does not choose images randomly.
    uris: Vec<String>,
}

fn svg_image_references(
    svg: &[u8],
    base_uri: &str,
    resolve: impl Fn(&str, &str) -> String,
) -> SvgImageReferences {
    let mut references = SvgImageReferences {
        by_href: HashMap::new(),
        uris: Vec::new(),
    };
    let mut seen = HashMap::new();
    let mut reader = Reader::from_reader(svg);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref().eq_ignore_ascii_case(b"image") =>
            {
                for attribute in element.attributes().with_checks(false).flatten() {
                    if attribute
                        .key
                        .local_name()
                        .as_ref()
                        .eq_ignore_ascii_case(b"href")
                        && let Ok(href) = attribute.normalized_value(XmlVersion::Implicit1_0)
                    {
                        let href = href.trim();
                        if href.is_empty()
                            || href.starts_with('#')
                            || has_uri_scheme(href)
                            || references.by_href.contains_key(href)
                        {
                            continue;
                        }
                        let uri = resolve(base_uri, href);
                        let reference = *seen.entry(uri).or_insert_with_key(|uri| {
                            let reference = references.uris.len();
                            references.uris.push(uri.clone());
                            reference
                        });
                        references.by_href.insert(href.to_owned(), reference);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    references
}

struct PreparedSvgImage {
    data_url: String,
    byte_len: usize,
}

struct PreparedSvgResources {
    references: SvgImageReferences,
    resources: HashMap<usize, PreparedSvgImage>,
    byte_len: usize,
}

impl PreparedSvgResources {
    fn new(references: SvgImageReferences) -> Self {
        Self {
            references,
            resources: HashMap::new(),
            byte_len: 0,
        }
    }

    fn image(&self, href: &str) -> Option<&PreparedSvgImage> {
        let reference = self.references.by_href.get(href.trim())?;
        self.resources.get(reference)
    }

    fn insert_image(
        &mut self,
        reference: usize,
        bytes: Vec<u8>,
        metadata: impl FnOnce(&str) -> io::Result<ResourceMetadata>,
    ) {
        if self.byte_len.saturating_add(bytes.len()) > MAX_EMBEDDED_BYTES {
            return;
        }
        let uri = &self.references.uris[reference];
        let metadata = metadata(uri).ok();
        if let Some(media_type) = svg_image_media_type(metadata.as_ref(), uri, &bytes) {
            let byte_len = bytes.len();
            let mut data_url = String::with_capacity(
                byte_len.div_ceil(3) * 4 + media_type.len() + "data:;base64,".len(),
            );
            data_url.push_str("data:");
            data_url.push_str(media_type);
            data_url.push_str(";base64,");
            STANDARD.encode_string(&bytes, &mut data_url);
            self.byte_len += byte_len;
            self.resources
                .insert(reference, PreparedSvgImage { data_url, byte_len });
        }
    }
}

/// Rewrite an SVG from a self-contained map. Resource I/O and path resolution
/// are completed before this step, shared by synchronous and async loading.
fn embed_svg_image_resources(prepared: &PreparedSvgResources, svg: &[u8]) -> Vec<u8> {
    if prepared.resources.is_empty() {
        return svg.to_vec();
    }
    let mut reader = Reader::from_reader(svg);
    let mut writer = Writer::new(Vec::with_capacity(svg.len()));
    let mut changed = false;
    let mut embedded_bytes = 0usize;

    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(_) => return svg.to_vec(),
        };
        let event = match &event {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref().eq_ignore_ascii_case(b"image") =>
            {
                let Some(element) =
                    embed_svg_image_element(prepared, element, &mut embedded_bytes, &mut changed)
                else {
                    return svg.to_vec();
                };
                if matches!(event, Event::Start(_)) {
                    Event::Start(element)
                } else {
                    Event::Empty(element)
                }
            }
            Event::Eof => break,
            _ => event,
        };
        if writer.write_event(event).is_err() {
            return svg.to_vec();
        }
    }

    if changed {
        writer.into_inner()
    } else {
        svg.to_vec()
    }
}

fn embed_svg_image_element(
    prepared: &PreparedSvgResources,
    element: &BytesStart<'_>,
    embedded_bytes: &mut usize,
    changed: &mut bool,
) -> Option<BytesStart<'static>> {
    let name = std::str::from_utf8(element.name().as_ref())
        .ok()?
        .to_owned();
    let mut rewritten = BytesStart::new(name);

    for attribute in element.attributes().with_checks(false) {
        let attribute = attribute.ok()?;
        if attribute
            .key
            .local_name()
            .as_ref()
            .eq_ignore_ascii_case(b"href")
        {
            let href = attribute.normalized_value(XmlVersion::Implicit1_0).ok()?;
            if let Some(image) = prepared.image(&href)
                && embedded_bytes.saturating_add(image.byte_len) <= MAX_EMBEDDED_BYTES
            {
                *embedded_bytes += image.byte_len;
                let key = std::str::from_utf8(attribute.key.as_ref()).ok()?;
                rewritten.push_attribute((key, image.data_url.as_str()));
                *changed = true;
                continue;
            }
        }
        rewritten.push_attribute(attribute);
    }

    Some(rewritten.into_owned())
}

fn svg_image_media_type(
    metadata: Option<&ResourceMetadata>,
    uri: &str,
    bytes: &[u8],
) -> Option<&'static str> {
    let declared = metadata
        .and_then(|metadata| metadata.media_type.as_deref())
        .and_then(canonical_image_media_type);
    declared
        .or_else(|| sniff_image_media_type(bytes))
        .or_else(|| image_media_type_from_uri(uri))
}

fn canonical_image_media_type(media_type: &str) -> Option<&'static str> {
    let media_type = media_type_essence(media_type);
    if media_type.eq_ignore_ascii_case("image/jpg") {
        return Some("image/jpeg");
    }
    [
        "image/jpeg",
        "image/png",
        "image/gif",
        "image/webp",
        "image/svg+xml",
    ]
    .into_iter()
    .find(|candidate| media_type.eq_ignore_ascii_case(candidate))
}

fn sniff_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .take(5)
        .eq(b"<svg ".iter().copied())
    {
        Some("image/svg+xml")
    } else {
        None
    }
}

fn image_media_type_from_uri(uri: &str) -> Option<&'static str> {
    let path = uri_path(uri);
    let extension = path.rsplit_once('.').map_or("", |(_, extension)| extension);
    if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") {
        Some("image/jpeg")
    } else if extension.eq_ignore_ascii_case("png") {
        Some("image/png")
    } else if extension.eq_ignore_ascii_case("gif") {
        Some("image/gif")
    } else if extension.eq_ignore_ascii_case("webp") {
        Some("image/webp")
    } else if extension.eq_ignore_ascii_case("svg") {
        Some("image/svg+xml")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlockingJob, BlockingJobSpawner, DecodedImage, ImageKey, ImagePipeline, ImageService,
        ImageResourceContext, MAX_EMBEDDED_BYTES, PreparedSvgResources,
        decode_image, embed_svg_image_resources, load_image_bytes, load_image_bytes_async,
        probe_dimensions, svg_image_references,
    };
    use crate::document::{ImageResource, ImageSource};
    use crate::resources::{ResourceMetadata, ResourceProvider};
    use std::collections::{HashMap, HashSet};
    use std::future::{Future, ready};
    use std::io;
    use std::slice::from_ref;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Wake, Waker};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct MemoryImageContext {
        metadata: HashMap<String, ResourceMetadata>,
        resolutions: Mutex<Vec<String>>,
        metadata_requests: Mutex<Vec<String>>,
    }

    impl ImageResourceContext for MemoryImageContext {
        fn resolve(&self, base: &str, href: &str) -> String {
            self.resolutions.lock().unwrap().push(href.to_owned());
            format!(
                "{}/{}",
                base.rsplit_once('/').unwrap().0,
                href.trim_start_matches("./")
            )
        }

        fn metadata(&self, uri: &str) -> io::Result<ResourceMetadata> {
            self.metadata_requests.lock().unwrap().push(uri.to_owned());
            Ok(self.metadata.get(uri).cloned().unwrap_or_default())
        }
    }

    // The same host can implement both interfaces. The async helper must only
    // use its memory context, never the provider methods that can perform I/O.
    impl ResourceProvider for MemoryImageContext {
        fn read_bytes(&self, _: &str) -> io::Result<Vec<u8>> {
            panic!("provider read")
        }
        fn metadata(&self, _: &str) -> io::Result<ResourceMetadata> {
            panic!("provider metadata")
        }
        fn resolve(&self, _: &str, _: &str) -> String {
            panic!("provider resolve")
        }
        fn exists(&self, _: &str) -> bool {
            panic!("provider exists")
        }
        fn list_html_candidates(&self, _: &str) -> io::Result<Vec<String>> {
            panic!("provider list")
        }
    }

    struct NoWake;
    impl Wake for NoWake {
        fn wake(self: Arc<Self>) {
            panic!("test future should be ready")
        }
    }

    fn run_ready<T>(future: impl Future<Output = T>) -> T {
        let waker = Waker::from(Arc::new(NoWake));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test future should be ready"),
        }
    }

    #[test]
    fn dimension_probes_borrow_embedded_svg_without_loading_dependencies() {
        let provider = MemoryImageContext::default();
        let svg: Arc<[u8]> =
            Arc::from(br#"<svg width="32" height="16"><image href="cover.png"/></svg>"#.as_slice());
        for source in [
            ImageSource::InlineSvg {
                bytes: svg.clone(),
                base_uri: "OPS/chapter.xhtml".into(),
            },
            ImageSource::MathMl {
                markup: "<math/>".into(),
                svg,
                baseline: 1.0,
            },
        ] {
            assert_eq!(probe_dimensions(&provider, &source), Some((32, 16)));
        }
        let provider = CountingProvider(AtomicUsize::new(0));
        assert_eq!(
            probe_dimensions(&provider, &ImageSource::Uri("cover.svg".into())),
            Some((20, 10))
        );
        assert_eq!(provider.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn async_inline_svg_uses_only_memory_context_and_deduplicates_fetches() {
        let context = MemoryImageContext {
            metadata: HashMap::from([(
                "OPS/mark.jpg".to_owned(),
                ResourceMetadata {
                    media_type: Some(" IMAGE/PNG; charset=binary".to_owned()),
                    charset: None,
                },
            )]),
            ..Default::default()
        };
        let source = ImageSource::InlineSvg {
            bytes: Arc::from(br##"<svg xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="mark.jpg"/><image href="./mark.jpg"></image><image href=" mark.jpg "/><image href="missing.png"/><image href="https://example.com/remote.png"/><image href="data:image/png;base64,AA=="/><image href="#symbol"/></svg>"##.as_slice()),
            base_uri: "OPS/chapter.xhtml".to_owned(),
        };
        let reads = Mutex::new(Vec::new());
        let result = run_ready(load_image_bytes_async(&context, &source, |uri| {
            reads.lock().unwrap().push(uri.clone());
            ready(if uri == "OPS/mark.jpg" {
                Ok(b"manifest image".to_vec())
            } else {
                Err(io::ErrorKind::NotFound.into())
            })
        }))
        .unwrap();
        let svg = String::from_utf8(result).unwrap();
        assert_eq!(
            svg.matches("data:image/png;base64,bWFuaWZlc3QgaW1hZ2U=")
                .count(),
            3
        );
        for unchanged in [
            "missing.png",
            "https://example.com/remote.png",
            "data:image/png;base64,AA==",
            "#symbol",
        ] {
            assert!(svg.contains(unchanged));
        }
        assert_eq!(*reads.lock().unwrap(), ["OPS/mark.jpg", "OPS/missing.png"]);
        assert_eq!(
            *context.resolutions.lock().unwrap(),
            ["mark.jpg", "./mark.jpg", "missing.png"]
        );
        assert_eq!(*context.metadata_requests.lock().unwrap(), ["OPS/mark.jpg"]);
    }

    #[test]
    fn async_uri_image_only_uses_the_read_callback() {
        let context = MemoryImageContext::default();
        let source = ImageSource::Uri("cover.png".to_owned());
        let result = run_ready(load_image_bytes_async(&context, &source, |uri| {
            assert_eq!(uri, "cover.png");
            ready(Ok(vec![1, 2, 3]))
        }))
        .unwrap();
        assert_eq!(result, [1, 2, 3]);
        assert!(context.resolutions.lock().unwrap().is_empty());
        assert!(context.metadata_requests.lock().unwrap().is_empty());
        let error = run_ready(load_image_bytes_async(&context, &source, |_| {
            ready(Err(io::ErrorKind::PermissionDenied.into()))
        }))
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn async_svg_skips_oversized_resources_and_keeps_later_images() {
        let context = MemoryImageContext::default();
        let source = ImageSource::InlineSvg {
            bytes: Arc::from(
                br#"<svg><image href="huge.png"/><image href="small.png"/></svg>"#.as_slice(),
            ),
            base_uri: "OPS/chapter.xhtml".to_owned(),
        };
        let result = run_ready(load_image_bytes_async(&context, &source, |uri| {
            ready(Ok(if uri == "OPS/huge.png" {
                vec![0; MAX_EMBEDDED_BYTES + 1]
            } else {
                vec![1]
            }))
        }))
        .unwrap();
        let svg = String::from_utf8(result).unwrap();
        assert!(svg.contains(r#"href="huge.png""#));
        assert!(svg.contains("data:image/png;base64,AQ=="));
        assert_eq!(
            *context.metadata_requests.lock().unwrap(),
            ["OPS/small.png"]
        );
    }

    #[test]
    fn async_svg_stops_fetching_at_the_total_byte_limit() {
        let context = MemoryImageContext::default();
        let source = ImageSource::InlineSvg {
            bytes: Arc::from(
                br#"<svg><image href="large.png"/><image href="later.png"/></svg>"#.as_slice(),
            ),
            base_uri: "OPS/chapter.xhtml".to_owned(),
        };
        let reads = Mutex::new(Vec::new());
        let result = run_ready(load_image_bytes_async(&context, &source, |uri| {
            reads.lock().unwrap().push(uri);
            ready(Ok(vec![0; MAX_EMBEDDED_BYTES]))
        }))
        .unwrap();
        let svg = String::from_utf8(result).unwrap();
        assert_eq!(*reads.lock().unwrap(), ["OPS/large.png"]);
        assert_eq!(svg.matches("data:image/png;base64,").count(), 1);
        assert!(svg.contains(r#"href="later.png""#));
    }

    #[test]
    fn svg_rewriting_needs_no_context_and_limits_repeated_embeddings() {
        let svg = br#"<svg><!--keep--><text>before &amp; <![CDATA[<raw>]]><?keep value?></text><image href="large.png"/><image href="large.png"></image></svg>"#;
        let references = svg_image_references(svg, "OPS/chapter.xhtml", |_, href| href.to_owned());
        let mut prepared = PreparedSvgResources::new(references);
        prepared.insert_image(0, vec![0; MAX_EMBEDDED_BYTES / 2 + 1], |_| {
            Ok(ResourceMetadata::default())
        });
        let rewritten = String::from_utf8(embed_svg_image_resources(&prepared, svg)).unwrap();
        assert_eq!(rewritten.matches("data:image/png;base64,").count(), 1);
        assert_eq!(rewritten.matches(r#"href="large.png""#).count(), 1);
        assert!(rewritten.starts_with(
            "<svg><!--keep--><text>before &amp; <![CDATA[<raw>]]><?keep value?></text>"
        ));
        assert!(rewritten.ends_with(r#"<image href="large.png"></image></svg>"#));
    }

    struct CountingProvider(AtomicUsize);

    #[derive(Default)]
    struct CountingJobSpawner(AtomicUsize);

    impl BlockingJobSpawner for CountingJobSpawner {
        fn spawn(&self, job: BlockingJob) {
            self.0.fetch_add(1, Ordering::Relaxed);
            job();
        }
    }

    impl ResourceProvider for CountingProvider {
        fn read_bytes(&self, _uri: &str) -> io::Result<Vec<u8>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(br#"<svg xmlns='http://www.w3.org/2000/svg' width='20' height='10'/>"#.to_vec())
        }

        fn exists(&self, _uri: &str) -> bool {
            true
        }
        fn resolve(&self, _base: &str, href: &str) -> String {
            href.to_owned()
        }
        fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    struct SvgResourceProvider {
        resources: HashMap<String, Vec<u8>>,
        reads: Mutex<Vec<String>>,
    }

    impl ResourceProvider for SvgResourceProvider {
        fn read_bytes(&self, uri: &str) -> io::Result<Vec<u8>> {
            self.reads
                .lock()
                .expect("read log lock")
                .push(uri.to_owned());
            self.resources
                .get(uri)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, uri.to_owned()))
        }

        fn metadata(&self, uri: &str) -> io::Result<ResourceMetadata> {
            Ok(ResourceMetadata {
                media_type: (uri == "OPS/Images/mark").then(|| "image/png".to_owned()),
                charset: None,
            })
        }

        fn exists(&self, uri: &str) -> bool {
            self.resources.contains_key(uri)
        }

        fn resolve(&self, base: &str, href: &str) -> String {
            let mut segments = base.rsplit_once('/').map_or(Vec::new(), |(directory, _)| {
                directory.split('/').map(str::to_owned).collect()
            });
            for segment in href.split('/') {
                match segment {
                    "" | "." => {}
                    ".." => {
                        segments.pop();
                    }
                    segment => segments.push(segment.to_owned()),
                }
            }
            segments.join("/")
        }

        fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn relative_svg_viewport_is_retained_for_backend_rendering() {
        let decoded = decode_image(br#"<svg xmlns="http://www.w3.org/2000/svg" height="50%"><rect width="20" height="10" fill="blue"/></svg>"#).expect("SVG should decode as a vector resource");
        assert!(matches!(decoded, DecodedImage::Svg { .. }));
        assert_eq!(decoded.dimensions(), (300, 150));
    }

    #[test]
    fn inline_svg_dependencies_resolve_through_the_provider_and_effective_base() {
        let provider = SvgResourceProvider {
            resources: HashMap::from([
                (
                    "OPS/Images/cover.jpeg".to_owned(),
                    vec![0xff, 0xd8, 0xff, 0xd9],
                ),
                ("OPS/Images/mark".to_owned(), b"\x89PNG\r\n\x1a\n".to_vec()),
            ]),
            reads: Mutex::new(Vec::new()),
        };
        let svg: Arc<[u8]> = Arc::from(
            br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="../Images/cover.jpeg"/><image href="../Images/mark"/><image href="https://example.com/remote.png"/><image href="data:image/png;base64,AA=="/><image href="#symbol"/></svg>"##
                .as_slice(),
        );
        let source = ImageSource::InlineSvg {
            bytes: svg,
            base_uri: "OPS/Text/titlepage.xhtml".to_owned(),
        };

        let hydrated =
            String::from_utf8(load_image_bytes(&provider, &source).expect("hydrate inline SVG"))
                .expect("rewritten SVG remains UTF-8");

        assert!(hydrated.contains(r#"xlink:href="data:image/jpeg;base64,/9j/2Q==""#));
        assert!(hydrated.contains(r#"href="data:image/png;base64,iVBORw0KGgo=""#));
        assert!(hydrated.contains(r#"href="https://example.com/remote.png""#));
        assert!(hydrated.contains(r#"href="data:image/png;base64,AA==""#));
        assert!(hydrated.contains(r##"href="#symbol""##));
        assert_eq!(
            *provider.reads.lock().expect("read log lock"),
            ["OPS/Images/cover.jpeg", "OPS/Images/mark"]
        );
    }

    #[test]
    fn inline_svg_cache_identity_includes_its_base_uri() {
        let bytes: Arc<[u8]> = Arc::from(br#"<svg><image href="cover.png"/></svg>"#.as_slice());
        let first = ImageSource::InlineSvg {
            bytes: bytes.clone(),
            base_uri: "first/chapter.xhtml".to_owned(),
        };
        let second = ImageSource::InlineSvg {
            bytes,
            base_uri: "second/chapter.xhtml".to_owned(),
        };
        assert_ne!(ImageKey::for_source(&first), ImageKey::for_source(&second));
    }

    #[test]
    fn resource_updates_preserve_unchanged_decodes_and_refresh_changed_sources() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let service = ImageService::new(provider.clone(), 1024 * 1024);
        let resource = |uri: &str| ImageResource {
            source: ImageSource::Uri(uri.to_owned()), width: 0, height: 0,
            width_attr: None, height_attr: None,
        };
        let mut resources = vec![resource("first.svg"), resource("second.svg")];
        let mut pipeline = ImagePipeline::with_service(service);
        pipeline.ensure_window(&resources, &HashSet::from([0, 1]));
        assert!(pipeline.poll(&resources).decoded);
        let unchanged = pipeline.decoded_cache[&1].clone();
        pipeline.update_resources(&resources);
        assert!(Arc::ptr_eq(&unchanged, &pipeline.decoded_cache[&1]));
        assert!(!pipeline.poll(&resources).decoded);

        resources[0] = resource("replacement.svg");
        resources.push(resource("added.svg"));
        pipeline.invalidate_resources();
        pipeline.update_resources(&resources);
        assert!(pipeline.get_decoded(0).is_none());
        assert!(pipeline.get_decoded(1).is_none());
        pipeline.ensure_window(&resources, &HashSet::from([0, 1, 2]));
        assert!(pipeline.poll(&resources).decoded);
        assert!(pipeline.get_decoded(0).is_some() && pipeline.get_decoded(2).is_some());
        assert_eq!(provider.0.load(Ordering::Relaxed), 5);

        pipeline.update_resources(&resources[..1]);
        assert!(pipeline.get_decoded(1).is_none() && pipeline.get_decoded(2).is_none());
        assert!(!pipeline.has_pending());
    }

    #[test]
    fn image_requests_do_not_retain_the_borrowed_resource_catalog() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let mut pipeline = ImagePipeline::new(provider);
        let bytes: Arc<[u8]> = Arc::from(br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"/>"#.as_slice());
        let source_owner = Arc::downgrade(&bytes);
        let resources = vec![ImageResource {
            source: ImageSource::InlineSvg { bytes, base_uri: "chapter.xhtml".to_owned() },
            width: 20, height: 10, width_attr: None, height_attr: None,
        }];
        pipeline.ensure_window(&resources, &HashSet::from([0]));
        assert!(pipeline.poll(&resources).decoded);
        drop(resources);
        assert!(
            source_owner.upgrade().is_none(),
            "only the document owns its inline source bytes"
        );
        assert_eq!(pipeline.get_decoded(0).unwrap().dimensions(), (20, 10));
    }

    #[test]
    fn borrowed_svg_catalog_changes_invalidate_active_and_inactive_resources() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let mut pipeline = ImagePipeline::new(provider);
        let resource = |width: u32, base: &str| ImageResource {
            source: ImageSource::InlineSvg {
                bytes: format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="10"/>"#).into_bytes().into(),
                base_uri: base.to_owned(),
            },
            width, height: 10, width_attr: None, height_attr: None,
        };
        let mut resources = vec![resource(20, "first.xhtml")];
        let desired = HashSet::from([0]);
        pipeline.ensure_window(&resources, &desired);
        assert_eq!(pipeline.poll(&resources).dimensions_changed, vec![0]);
        resources[0] = resource(20, "second.xhtml");
        pipeline.update_resources(&resources);
        assert!(pipeline.get_decoded(0).is_none());
        pipeline.ensure_window(&resources, &desired);
        assert_eq!(pipeline.poll(&resources).dimensions_changed, vec![0]);
        pipeline.deactivate();
        resources[0] = resource(40, "second.xhtml");
        pipeline.update_resources(&resources);
        pipeline.ensure_window(&resources, &desired);
        assert_eq!(pipeline.poll(&resources).dimensions_changed, vec![0]);
        assert_eq!(pipeline.get_decoded(0).unwrap().dimensions(), (40, 10));
        pipeline.update_resources(&[]);
        assert!(pipeline.get_decoded(0).is_none());
        assert!(!pipeline.has_pending());
    }

    #[test]
    fn pipelines_sharing_a_service_decode_a_source_once() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let jobs = Arc::new(CountingJobSpawner::default());
        let service = ImageService::with_job_spawner(provider.clone(), jobs.clone(), 1024 * 1024);
        let resource = ImageResource {
            source: ImageSource::Uri("shared.svg".to_owned()),
            width: 0,
            height: 0,
            width_attr: None,
            height_attr: None,
        };
        let mut first =
            ImagePipeline::with_service(service.clone());
        let mut second = ImagePipeline::with_service(service);
        let desired = HashSet::from([0]);
        first.ensure_window(from_ref(&resource), &desired);
        second.ensure_window(from_ref(&resource), &desired);

        let deadline = Instant::now() + Duration::from_secs(2);
        while (first.get_decoded(0).is_none() || second.get_decoded(0).is_none())
            && Instant::now() < deadline
        {
            first.poll(from_ref(&resource));
            second.poll(from_ref(&resource));
            std::thread::yield_now();
        }

        assert!(first.get_decoded(0).is_some() && second.get_decoded(0).is_some());
        assert_eq!(
            provider.0.load(Ordering::Relaxed),
            1,
            "the source is fetched and decoded once for both document-local indices"
        );
        assert_eq!(
            jobs.0.load(Ordering::Relaxed),
            2,
            "the host schedules one load and one decode for a deduplicated source"
        );
    }

    #[test]
    fn completed_decode_wakes_the_host_without_polling() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let service = ImageService::new(provider, 1024 * 1024);
        let wake_count = Arc::new(AtomicUsize::new(0));
        let observed = wake_count.clone();
        service.set_completion_waker(Some(Arc::new(move || {
            observed.fetch_add(1, Ordering::Relaxed);
        })));
        let resource = ImageResource {
            source: ImageSource::Uri("wake.svg".to_owned()),
            width: 0,
            height: 0,
            width_attr: None,
            height_attr: None,
        };
        let mut pipeline = ImagePipeline::with_service(service);
        pipeline.ensure_window(from_ref(&resource), &HashSet::from([0]));

        let deadline = Instant::now() + Duration::from_secs(2);
        while wake_count.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }

        assert_eq!(wake_count.load(Ordering::Relaxed), 1);
        assert!(pipeline.poll(from_ref(&resource)).decoded);
    }

    #[test]
    fn shared_service_enforces_one_decoded_byte_budget() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let service = ImageService::new(provider, 80);
        let resources = vec![
            ImageResource {
                source: ImageSource::Uri("first.svg".to_owned()),
                width: 0,
                height: 0,
                width_attr: None,
                height_attr: None,
            },
            ImageResource {
                source: ImageSource::Uri("second.svg".to_owned()),
                width: 0,
                height: 0,
                width_attr: None,
                height_attr: None,
            },
        ];
        let mut pipeline = ImagePipeline::with_service(service.clone());
        pipeline.ensure_window(&resources, &HashSet::from([0]));
        let deadline = Instant::now() + Duration::from_secs(2);
        while pipeline.get_decoded(0).is_none() && Instant::now() < deadline {
            pipeline.poll(&resources);
            std::thread::yield_now();
        }
        pipeline.ensure_window(&resources, &HashSet::from([1]));
        while pipeline.get_decoded(1).is_none() && Instant::now() < deadline {
            pipeline.poll(&resources);
            std::thread::yield_now();
        }

        let state = service.state.lock().expect("image service mutex poisoned");
        assert_eq!(
            state.decoded.len(),
            1,
            "the unpinned least-recently-used decode is evicted"
        );
    }

    #[test]
    fn deactivated_pipeline_does_not_pin_shared_decodes() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let service = ImageService::new(provider, 80);
        let resource = |uri: &str| ImageResource {
            source: ImageSource::Uri(uri.to_owned()),
            width: 0,
            height: 0,
            width_attr: None,
            height_attr: None,
        };
        let desired = HashSet::from([0]);
        let mut first =
            ImagePipeline::with_service(service.clone());
        first.ensure_window(&[resource("first.svg")], &desired);
        let deadline = Instant::now() + Duration::from_secs(2);
        while first.get_decoded(0).is_none() && Instant::now() < deadline {
            first.poll(&[resource("first.svg")]);
            std::thread::yield_now();
        }
        assert!(first.get_decoded(0).is_some());

        let mut second =
            ImagePipeline::with_service(service.clone());
        second.ensure_window(&[resource("second.svg")], &desired);
        let second_deadline = Instant::now() + Duration::from_secs(2);
        while second.get_decoded(0).is_none() && Instant::now() < second_deadline {
            second.poll(&[resource("second.svg")]);
            std::thread::yield_now();
        }
        assert!(
            second.get_decoded(0).is_some(),
            "the newly decoded visible image must not evict itself"
        );
        {
            let state = service.state.lock().expect("image service mutex poisoned");
            assert_eq!(
                state.decoded.len(),
                2,
                "the two-image visible working set may temporarily exceed the cache budget"
            );
        }

        first.deactivate();
        assert!(first.get_decoded(0).is_none());
        let state = service.state.lock().expect("image service mutex poisoned");
        assert_eq!(
            state.decoded.len(),
            1,
            "releasing an inactive document immediately retrims the cache"
        );
        assert!(second.get_decoded(0).is_some());
    }
}
