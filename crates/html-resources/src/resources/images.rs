use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::{self, Cursor};
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use peniko::{Blob, Format, Image as PenikoImage};
use quick_xml::Reader;
use quick_xml::Writer;
use quick_xml::events::{BytesStart, Event};
use sha2::{Digest, Sha256};

use crate::document::{ImageResource, ImageSource};
use crate::resources::{ResourceMetadata, ResourceProvider};

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
    InlineSvg { hash: Vec<u8>, base_uri: String },
}

impl ImageKey {
    fn for_source(source: &ImageSource) -> Self {
        match source {
            ImageSource::Uri(uri) => Self::Uri(uri.clone()),
            ImageSource::MathMl { svg, .. } => Self::InlineSvg {
                hash: Sha256::digest(svg).to_vec(),
                base_uri: String::new(),
            },
            ImageSource::InlineSvg { bytes, base_uri } => Self::InlineSvg {
                hash: Sha256::digest(bytes).to_vec(),
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
    decoded_bytes: usize,
    byte_budget: usize,
    clock: u64,
    revision: u64,
    completion_waker: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Source-keyed image jobs and decoded-image memory shared by every document
/// in one reader. Cloning this handle creates no workers.
#[derive(Clone)]
pub struct ImageService {
    state: Arc<Mutex<SharedState>>,
    loader: Arc<dyn ImageLoader>,
    jobs: Arc<dyn BlockingJobSpawner>,
}

impl ImageService {
    pub const DEFAULT_BYTE_BUDGET: usize = 128 * 1024 * 1024;

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
        let state = Arc::new(Mutex::new(SharedState {
            decoded: HashMap::new(),
            pending: HashSet::new(),
            decoded_bytes: 0,
            byte_budget,
            clock: 0,
            revision: 0,
            completion_waker: None,
        }));
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
        {
            let mut state = self.state.lock().expect("image service mutex poisoned");
            if state.decoded.contains_key(&key) || !state.pending.insert(key.clone()) {
                return;
            }
        }
        let state = Arc::downgrade(&self.state);
        let jobs = self.jobs.clone();
        self.loader.load(
            source.clone(),
            Box::new(move |bytes| {
                if state.strong_count() == 0 {
                    return;
                }
                jobs.spawn(Box::new(move || execute_image_job(state, key, bytes)));
            }),
        );
    }

    fn get(&self, key: &ImageKey) -> Option<(Arc<DecodedImage>, u64)> {
        let mut state = self.state.lock().expect("image service mutex poisoned");
        state.clock = state.clock.wrapping_add(1);
        let clock = state.clock;
        let image = state.decoded.get_mut(key)?;
        image.last_used = clock;
        Some((image.decoded.clone(), image.revision))
    }

    fn trim(&self) {
        let mut state = self.state.lock().expect("image service mutex poisoned");
        trim_decoded_cache(&mut state, None);
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

pub struct ImagePipeline {
    images: Arc<Vec<ImageResource>>,
    service: ImageService,
    decoded_cache: HashMap<u32, Arc<DecodedImage>>,
    seen_revisions: HashMap<u32, u64>,
    desired: HashMap<u32, ImageKey>,
    image_dimensions: HashMap<u32, (u32, u32)>,
}

#[derive(Debug, Default)]
pub struct ImagePipelinePoll {
    pub decoded: bool,
    pub dimensions_changed: Vec<u32>,
}

impl ImagePipeline {
    pub fn new(images: Arc<Vec<ImageResource>>, provider: Arc<dyn ResourceProvider>) -> Self {
        let service = ImageService::new(provider, ImageService::DEFAULT_BYTE_BUDGET);
        Self::with_service(images, service)
    }

    pub fn with_service(images: Arc<Vec<ImageResource>>, service: ImageService) -> Self {
        Self {
            images,
            service,
            decoded_cache: HashMap::new(),
            seen_revisions: HashMap::new(),
            desired: HashMap::new(),
            image_dimensions: HashMap::new(),
        }
    }

    /// Install the current document's resource metadata after a pipeline update.
    /// Unchanged images retain decoded buffers and pending jobs. Changed or
    /// removed sources lose their local state; the next visible window requests
    /// their new content through the shared service.
    pub fn update_resources(&mut self, images: &[ImageResource]) {
        if self.images.as_slice() == images {
            return;
        }
        for index in 0..self.images.len() {
            if images.get(index).is_some_and(|image| image.source == self.images[index].source) {
                continue;
            }
            let index = index as u32;
            self.decoded_cache.remove(&index);
            self.seen_revisions.remove(&index);
            self.desired.remove(&index);
            self.image_dimensions.remove(&index);
        }
        self.images = Arc::new(images.to_vec());
    }

    pub fn poll(&mut self) -> ImagePipelinePoll {
        let mut changed = ImagePipelinePoll::default();
        if self.desired.is_empty() { return changed; }
        // Keys are retained for the visible window. Polling neither rebuilds
        // SVG hashes nor takes one mutex per image; unchanged revisions borrow
        // the shared entry without incrementing its Arc count.
        let mut state = self.service.state.lock().expect("image service mutex poisoned");
        for (&idx, key) in &self.desired {
            state.clock = state.clock.wrapping_add(1);
            let clock = state.clock;
            let Some(image) = state.decoded.get_mut(key) else { continue };
            image.last_used = clock;
            if self.seen_revisions.get(&idx) == Some(&image.revision) { continue; }
            self.seen_revisions.insert(idx, image.revision);
            let dimensions = image.decoded.dimensions();
            self.decoded_cache.insert(idx, image.decoded.clone());
            changed.decoded = true;
            if self.image_dimensions.insert(idx, dimensions) != Some(dimensions) {
                changed.dimensions_changed.push(idx);
            }
        }
        drop(state);
        changed.dimensions_changed.sort_unstable();
        changed.dimensions_changed.dedup();
        changed
    }

    pub fn ensure_window(&mut self, desired: &HashSet<u32>) {
        self.decoded_cache.retain(|idx, _| desired.contains(idx));
        self.seen_revisions.retain(|idx, _| desired.contains(idx));
        self.desired.retain(|idx, _| desired.contains(idx));

        let mut shared_cache_hit = false;
        for &idx in desired {
            let Some(resource) = self.images.get(idx as usize) else { continue };
            let key = self.desired.entry(idx).or_insert_with(|| ImageKey::for_source(&resource.source));
            if self.decoded_cache.contains_key(&idx) {
                continue;
            }
            if let Some(resource) = self.images.get(idx as usize) {
                if let Some((decoded, _)) = self.service.get(key) {
                    // Paint a shared hit immediately. Leaving its revision
                    // unseen makes `has_pending` schedule one poll so intrinsic
                    // dimensions are still reported to this document.
                    self.decoded_cache.insert(idx, decoded);
                    shared_cache_hit = true;
                } else {
                    self.service.request(&resource.source, key.clone());
                }
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
        self.seen_revisions.clear();
        self.desired.clear();
        self.service.trim();
    }

    pub fn get_decoded(&self, idx: u32) -> Option<&DecodedImage> {
        self.decoded_cache.get(&idx).map(AsRef::as_ref)
    }

    pub fn has_pending(&self) -> bool {
        if self.desired.keys().all(|idx| self.seen_revisions.contains_key(idx)) {
            return false;
        }
        let state = self.service.state.lock().expect("image service mutex poisoned");
        self.desired.iter().any(|(idx, key)| {
            !self.seen_revisions.contains_key(idx)
                && (self.decoded_cache.contains_key(idx) || state.pending.contains(key))
        })
    }

}

fn execute_image_job(
    state: Weak<Mutex<SharedState>>,
    key: ImageKey,
    bytes: std::io::Result<Vec<u8>>,
) {
    let Some(state) = state.upgrade() else {
        return;
    };
    let decoded = bytes.ok().and_then(|bytes| decode_image(&bytes).ok());
    let mut state = state.lock().expect("image service mutex poisoned");
    state.pending.remove(&key);
    if let Some(decoded) = decoded {
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
        trim_decoded_cache(&mut state, Some(&key));
    }
    let waker = state.completion_waker.clone();
    drop(state);
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

pub fn probe_dimensions(
    provider: &dyn ResourceProvider,
    source: &ImageSource,
) -> Option<(u32, u32)> {
    let bytes = load_image_bytes(provider, source).ok()?;
    if let Some(dimensions) = svg_dimensions(&bytes) {
        return Some(dimensions);
    }
    if let Ok(reader) = image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format()
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
            for uri in &prepared.references.uris {
                if prepared.byte_len >= MAX_EMBEDDED_BYTES {
                    break;
                }
                if let Ok(bytes) = provider.read_bytes(uri)
                    && prepared.byte_len.saturating_add(bytes.len()) <= MAX_EMBEDDED_BYTES
                {
                    let metadata = provider.metadata(uri).ok();
                    if let Some(media_type) = svg_image_media_type(metadata.as_ref(), uri, &bytes) {
                        prepared.byte_len += bytes.len();
                        prepared
                            .resources
                            .insert(uri.clone(), PreparedSvgImage { bytes, media_type });
                    }
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
            for uri in &prepared.references.uris {
                if prepared.byte_len >= MAX_EMBEDDED_BYTES {
                    break;
                }
                if let Ok(bytes) = read(uri.clone()).await
                    && prepared.byte_len.saturating_add(bytes.len()) <= MAX_EMBEDDED_BYTES
                {
                    let metadata = context.metadata(uri).ok();
                    if let Some(media_type) = svg_image_media_type(metadata.as_ref(), uri, &bytes) {
                        prepared.byte_len += bytes.len();
                        prepared
                            .resources
                            .insert(uri.clone(), PreparedSvgImage { bytes, media_type });
                    }
                }
            }
            Ok(embed_svg_image_resources(&prepared, bytes))
        }
    }
}

struct SvgImageReferences {
    by_href: HashMap<String, String>,
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
    let mut seen = HashSet::new();
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
                        && let Ok(href) = attribute.unescape_value()
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
                        if seen.insert(uri.clone()) {
                            references.uris.push(uri.clone());
                        }
                        references.by_href.insert(href.to_owned(), uri);
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
    bytes: Vec<u8>,
    media_type: &'static str,
}

struct PreparedSvgResources {
    references: SvgImageReferences,
    resources: HashMap<String, PreparedSvgImage>,
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
        let uri = self.references.by_href.get(href.trim())?;
        self.resources.get(uri)
    }
}

/// Rewrite an SVG from a self-contained map. Resource I/O and path resolution
/// are completed before this step, shared by synchronous and async loading.
fn embed_svg_image_resources(prepared: &PreparedSvgResources, svg: &[u8]) -> Vec<u8> {
    let mut reader = Reader::from_reader(svg);
    let mut writer = Writer::new(Vec::with_capacity(svg.len()));
    let mut changed = false;
    let mut embedded_bytes = 0usize;

    loop {
        let event = match reader.read_event() {
            Ok(event) => event,
            Err(_) => return svg.to_vec(),
        };
        let event = match event {
            Event::Start(element)
                if element.local_name().as_ref().eq_ignore_ascii_case(b"image") =>
            {
                let Some(element) =
                    embed_svg_image_element(prepared, &element, &mut embedded_bytes, &mut changed)
                else {
                    return svg.to_vec();
                };
                Event::Start(element)
            }
            Event::Empty(element)
                if element.local_name().as_ref().eq_ignore_ascii_case(b"image") =>
            {
                let Some(element) =
                    embed_svg_image_element(prepared, &element, &mut embedded_bytes, &mut changed)
                else {
                    return svg.to_vec();
                };
                Event::Empty(element)
            }
            Event::Eof => break,
            event => event.into_owned(),
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
            let href = attribute.unescape_value().ok()?;
            if let Some(image) = prepared.image(&href)
                && embedded_bytes.saturating_add(image.bytes.len()) <= MAX_EMBEDDED_BYTES
            {
                *embedded_bytes += image.bytes.len();
                let encoded = base64::engine::general_purpose::STANDARD.encode(&image.bytes);
                let data_url = format!("data:{};base64,{encoded}", image.media_type);
                let key = std::str::from_utf8(attribute.key.as_ref()).ok()?;
                rewritten.push_attribute((key, data_url.as_str()));
                *changed = true;
                continue;
            }
        }
        rewritten.push_attribute(attribute);
    }

    Some(rewritten.into_owned())
}

fn has_uri_scheme(href: &str) -> bool {
    let Some(colon) = href.find(':') else {
        return false;
    };
    let Some(first_separator) = href.find(['/', '?', '#']) else {
        return true;
    };
    colon < first_separator
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
    match media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        "image/svg+xml" => Some("image/svg+xml"),
        _ => None,
    }
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
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
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
        ImageResourceContext, MAX_EMBEDDED_BYTES, PreparedSvgImage, PreparedSvgResources,
        decode_image, embed_svg_image_resources, load_image_bytes, load_image_bytes_async,
        svg_image_references,
    };
    use crate::document::{ImageResource, ImageSource};
    use crate::resources::{ResourceMetadata, ResourceProvider};
    use std::collections::{HashMap, HashSet};
    use std::future::{Future, ready};
    use std::io;
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
    fn async_inline_svg_uses_only_memory_context_and_deduplicates_fetches() {
        let context = MemoryImageContext {
            metadata: HashMap::from([(
                "OPS/mark.jpg".to_owned(),
                ResourceMetadata {
                    media_type: Some("image/png; charset=binary".to_owned()),
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
        let svg = br#"<svg><image href="large.png"/><image href="large.png"/></svg>"#;
        let references = svg_image_references(svg, "OPS/chapter.xhtml", |_, href| href.to_owned());
        let mut prepared = PreparedSvgResources::new(references);
        prepared.resources.insert(
            "large.png".to_owned(),
            PreparedSvgImage {
                bytes: vec![0; MAX_EMBEDDED_BYTES / 2 + 1],
                media_type: "image/png",
            },
        );
        let rewritten = String::from_utf8(embed_svg_image_resources(&prepared, svg)).unwrap();
        assert_eq!(rewritten.matches("data:image/png;base64,").count(), 1);
        assert_eq!(rewritten.matches(r#"href="large.png""#).count(), 1);
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
        let mut pipeline = ImagePipeline::with_service(Arc::new(resources.clone()), service);
        pipeline.ensure_window(&HashSet::from([0, 1]));
        assert!(pipeline.poll().decoded);
        let unchanged = pipeline.decoded_cache[&1].clone();
        pipeline.update_resources(&resources);
        assert!(Arc::ptr_eq(&unchanged, &pipeline.decoded_cache[&1]));
        assert!(!pipeline.poll().decoded);

        resources[0] = resource("replacement.svg");
        resources.push(resource("added.svg"));
        pipeline.update_resources(&resources);
        assert!(pipeline.get_decoded(0).is_none());
        assert!(Arc::ptr_eq(&unchanged, &pipeline.decoded_cache[&1]));
        pipeline.ensure_window(&HashSet::from([0, 1, 2]));
        assert!(pipeline.poll().decoded);
        assert!(pipeline.get_decoded(0).is_some() && pipeline.get_decoded(2).is_some());
        assert_eq!(provider.0.load(Ordering::Relaxed), 4);

        pipeline.update_resources(&resources[..1]);
        assert!(pipeline.get_decoded(1).is_none() && pipeline.get_decoded(2).is_none());
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
            ImagePipeline::with_service(Arc::new(vec![resource.clone()]), service.clone());
        let mut second = ImagePipeline::with_service(Arc::new(vec![resource]), service);
        let desired = HashSet::from([0]);
        first.ensure_window(&desired);
        second.ensure_window(&desired);

        let deadline = Instant::now() + Duration::from_secs(2);
        while (first.get_decoded(0).is_none() || second.get_decoded(0).is_none())
            && Instant::now() < deadline
        {
            first.poll();
            second.poll();
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
        let mut pipeline = ImagePipeline::with_service(Arc::new(vec![resource]), service);
        pipeline.ensure_window(&HashSet::from([0]));

        let deadline = Instant::now() + Duration::from_secs(2);
        while wake_count.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }

        assert_eq!(wake_count.load(Ordering::Relaxed), 1);
        assert!(pipeline.poll().decoded);
    }

    #[test]
    fn shared_service_enforces_one_decoded_byte_budget() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let service = ImageService::new(provider, 80);
        let resources = Arc::new(vec![
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
        ]);
        let mut pipeline = ImagePipeline::with_service(resources, service.clone());
        pipeline.ensure_window(&HashSet::from([0]));
        let deadline = Instant::now() + Duration::from_secs(2);
        while pipeline.get_decoded(0).is_none() && Instant::now() < deadline {
            pipeline.poll();
            std::thread::yield_now();
        }
        pipeline.ensure_window(&HashSet::from([1]));
        while pipeline.get_decoded(1).is_none() && Instant::now() < deadline {
            pipeline.poll();
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
            ImagePipeline::with_service(Arc::new(vec![resource("first.svg")]), service.clone());
        first.ensure_window(&desired);
        let deadline = Instant::now() + Duration::from_secs(2);
        while first.get_decoded(0).is_none() && Instant::now() < deadline {
            first.poll();
            std::thread::yield_now();
        }
        assert!(first.get_decoded(0).is_some());

        let mut second =
            ImagePipeline::with_service(Arc::new(vec![resource("second.svg")]), service.clone());
        second.ensure_window(&desired);
        let second_deadline = Instant::now() + Duration::from_secs(2);
        while second.get_decoded(0).is_none() && Instant::now() < second_deadline {
            second.poll();
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
