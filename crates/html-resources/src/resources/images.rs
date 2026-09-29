use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::{Arc, Mutex, Weak};

use base64::Engine;
use peniko::{Blob, Format, Image as PenikoImage};
use quick_xml::Reader;
use quick_xml::Writer;
use quick_xml::events::{BytesStart, Event};
use sha2::{Digest, Sha256};

use crate::document::{ImageResource, ImageSource};
use crate::resources::ResourceProvider;

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
    provider: Arc<dyn ResourceProvider>,
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
            provider,
            jobs,
        }
    }

    pub fn set_completion_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.state
            .lock()
            .expect("image service mutex poisoned")
            .completion_waker = waker;
    }

    fn request(&self, source: &ImageSource) {
        let key = ImageKey::for_source(source);
        {
            let mut state = self.state.lock().expect("image service mutex poisoned");
            if state.decoded.contains_key(&key) || !state.pending.insert(key.clone()) {
                return;
            }
        }
        let provider = self.provider.clone();
        let state = Arc::downgrade(&self.state);
        let source = source.clone();
        self.jobs.spawn(Box::new(move || {
            execute_image_job(provider, state, key, source);
        }));
    }

    fn get(&self, source: &ImageSource) -> Option<(Arc<DecodedImage>, u64)> {
        let key = ImageKey::for_source(source);
        let mut state = self.state.lock().expect("image service mutex poisoned");
        state.clock = state.clock.wrapping_add(1);
        let clock = state.clock;
        let image = state.decoded.get_mut(&key)?;
        image.last_used = clock;
        Some((image.decoded.clone(), image.revision))
    }

    fn is_pending(&self, source: &ImageSource) -> bool {
        self.state
            .lock()
            .expect("image service mutex poisoned")
            .pending
            .contains(&ImageKey::for_source(source))
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
    desired: HashSet<u32>,
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
            desired: HashSet::new(),
            image_dimensions: HashMap::new(),
        }
    }

    pub fn poll(&mut self) -> ImagePipelinePoll {
        let mut changed = ImagePipelinePoll::default();
        for &idx in &self.desired {
            let Some(resource) = self.images.get(idx as usize) else {
                continue;
            };
            let Some((decoded, revision)) = self.service.get(&resource.source) else {
                continue;
            };
            if self.seen_revisions.get(&idx) == Some(&revision) {
                continue;
            }
            self.seen_revisions.insert(idx, revision);
            let dimensions = decoded.dimensions();
            self.decoded_cache.insert(idx, decoded);
            changed.decoded = true;
            if self.image_dimensions.insert(idx, dimensions) != Some(dimensions) {
                changed.dimensions_changed.push(idx);
            }
        }

        changed.dimensions_changed.sort_unstable();
        changed.dimensions_changed.dedup();
        changed
    }

    pub fn ensure_window(&mut self, desired: &HashSet<u32>) {
        self.decoded_cache.retain(|idx, _| desired.contains(idx));
        self.seen_revisions.retain(|idx, _| desired.contains(idx));
        self.desired.clone_from(desired);

        let mut shared_cache_hit = false;
        for &idx in desired {
            if self.decoded_cache.contains_key(&idx) {
                continue;
            }
            if let Some(resource) = self.images.get(idx as usize) {
                if let Some((decoded, _)) = self.service.get(&resource.source) {
                    // Paint a shared hit immediately. Leaving its revision
                    // unseen makes `has_pending` schedule one poll so intrinsic
                    // dimensions are still reported to this document.
                    self.decoded_cache.insert(idx, decoded);
                    shared_cache_hit = true;
                } else {
                    self.service.request(&resource.source);
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
        self.desired.iter().any(|idx| {
            !self.seen_revisions.contains_key(idx)
                && (self.decoded_cache.contains_key(idx)
                    || self
                        .images
                        .get(*idx as usize)
                        .is_some_and(|resource| self.service.is_pending(&resource.source)))
        })
    }
}

fn execute_image_job(
    provider: Arc<dyn ResourceProvider>,
    state: Weak<Mutex<SharedState>>,
    key: ImageKey,
    source: ImageSource,
) {
    let Some(state) = state.upgrade() else {
        return;
    };
    let decoded = load_bytes(provider.as_ref(), &source)
        .ok()
        .and_then(|bytes| decode_image(&bytes).ok());
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
    let bytes = load_bytes(provider, source).ok()?;
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

fn load_bytes(provider: &dyn ResourceProvider, source: &ImageSource) -> std::io::Result<Vec<u8>> {
    match source {
        ImageSource::Uri(uri) => provider.read_bytes(uri),
        ImageSource::InlineSvg { bytes, base_uri } => {
            Ok(embed_svg_image_resources(provider, base_uri, bytes))
        }
    }
}

/// SVG rasterizers operate on a self-contained byte stream and cannot know how
/// to open resources from an arbitrary [`ResourceProvider`]. Resolve external
/// image references here, while both the effective document base URI and the
/// provider are available, so every renderer backend receives the same SVG.
fn embed_svg_image_resources(
    provider: &dyn ResourceProvider,
    base_uri: &str,
    svg: &[u8],
) -> Vec<u8> {
    const MAX_EMBEDDED_BYTES: usize = 24 * 1024 * 1024;

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
                let Some(element) = embed_svg_image_element(
                    provider,
                    base_uri,
                    &element,
                    &mut embedded_bytes,
                    MAX_EMBEDDED_BYTES,
                    &mut changed,
                ) else {
                    return svg.to_vec();
                };
                Event::Start(element)
            }
            Event::Empty(element)
                if element.local_name().as_ref().eq_ignore_ascii_case(b"image") =>
            {
                let Some(element) = embed_svg_image_element(
                    provider,
                    base_uri,
                    &element,
                    &mut embedded_bytes,
                    MAX_EMBEDDED_BYTES,
                    &mut changed,
                ) else {
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
    provider: &dyn ResourceProvider,
    base_uri: &str,
    element: &BytesStart<'_>,
    embedded_bytes: &mut usize,
    byte_limit: usize,
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
            if let Some(uri) = resolvable_svg_resource(provider, base_uri, &href)
                && let Ok(bytes) = provider.read_bytes(&uri)
                && embedded_bytes.saturating_add(bytes.len()) <= byte_limit
                && let Some(media_type) = svg_image_media_type(provider, &uri, &bytes)
            {
                *embedded_bytes += bytes.len();
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                let data_url = format!("data:{media_type};base64,{encoded}");
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

fn resolvable_svg_resource(
    provider: &dyn ResourceProvider,
    base_uri: &str,
    href: &str,
) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') || has_uri_scheme(href) {
        return None;
    }
    Some(provider.resolve(base_uri, href))
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
    provider: &dyn ResourceProvider,
    uri: &str,
    bytes: &[u8],
) -> Option<&'static str> {
    let declared = provider
        .metadata(uri)
        .ok()
        .and_then(|metadata| metadata.media_type)
        .and_then(|media_type| canonical_image_media_type(&media_type));
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
        decode_image, load_bytes,
    };
    use crate::document::{ImageResource, ImageSource};
    use crate::resources::{ResourceMetadata, ResourceProvider};
    use std::collections::{HashMap, HashSet};
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

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
            String::from_utf8(load_bytes(&provider, &source).expect("hydrate inline SVG"))
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
            1,
            "the host schedules one finite job for a deduplicated source"
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
