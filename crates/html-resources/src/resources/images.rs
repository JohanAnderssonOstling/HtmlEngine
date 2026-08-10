use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use peniko::{Blob, Format, Image as PenikoImage};
use quick_xml::Reader;
use quick_xml::events::Event;
use sha2::{Digest, Sha256};

use crate::document::{ImageResource, ImageSource};
use crate::resources::ResourceProvider;

#[derive(Clone)]
pub enum DecodedImage {
    Raster { image: PenikoImage, hash: Vec<u8>, byte_len: usize },
    Svg { bytes: Arc<[u8]>, hash: Vec<u8>, width: u32, height: u32 },
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
    Inline(Vec<u8>),
}

impl ImageKey {
    fn for_source(source: &ImageSource) -> Self {
        match source {
            ImageSource::Uri(uri) => Self::Uri(uri.clone()),
            ImageSource::Inline(bytes) => Self::Inline(Sha256::digest(bytes).to_vec()),
        }
    }
}

struct BytesRequest {
    key: ImageKey,
    source: ImageSource,
}

struct DecodeRequest {
    key: ImageKey,
    bytes: Arc<Vec<u8>>,
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
}

/// Source-keyed image workers and decoded-image memory shared by every
/// document in one reader. Cloning this handle creates no threads.
#[derive(Clone)]
pub struct ImageService {
    state: Arc<Mutex<SharedState>>,
    bytes_tx: mpsc::Sender<BytesRequest>,
}

impl ImageService {
    pub const DEFAULT_BYTE_BUDGET: usize = 128 * 1024 * 1024;

    pub fn new(provider: Arc<dyn ResourceProvider>, byte_budget: usize) -> Self {
        let state = Arc::new(Mutex::new(SharedState {
            decoded: HashMap::new(),
            pending: HashSet::new(),
            decoded_bytes: 0,
            byte_budget,
            clock: 0,
            revision: 0,
        }));
        let (bytes_tx, bytes_rx) = mpsc::channel();
        let (decode_tx, decode_rx) = mpsc::channel();

        let bytes_state = state.clone();
        thread::spawn(move || bytes_worker(provider, bytes_rx, decode_tx, bytes_state));
        let decode_state = state.clone();
        thread::spawn(move || decode_worker(decode_rx, decode_state));
        Self { state, bytes_tx }
    }

    fn request(&self, source: &ImageSource) {
        let key = ImageKey::for_source(source);
        {
            let mut state = self.state.lock().expect("image service mutex poisoned");
            if state.decoded.contains_key(&key) || !state.pending.insert(key.clone()) {
                return;
            }
        }
        if self.bytes_tx.send(BytesRequest { key: key.clone(), source: source.clone() }).is_err() {
            self.state.lock().expect("image service mutex poisoned").pending.remove(&key);
        }
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
        self.state.lock().expect("image service mutex poisoned").pending.contains(&ImageKey::for_source(source))
    }

    fn trim(&self) {
        let mut state = self.state.lock().expect("image service mutex poisoned");
        trim_decoded_cache(&mut state, None);
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
        Self { images, service, decoded_cache: HashMap::new(), seen_revisions: HashMap::new(), desired: HashSet::new(), image_dimensions: HashMap::new() }
    }

    pub fn poll(&mut self) -> ImagePipelinePoll {
        let mut changed = ImagePipelinePoll::default();
        for &idx in &self.desired {
            let Some(resource) = self.images.get(idx as usize) else { continue };
            let Some((decoded, revision)) = self.service.get(&resource.source) else { continue };
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
                } else {
                    self.service.request(&resource.source);
                }
            }
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
                && (self.decoded_cache.contains_key(idx) || self.images.get(*idx as usize).is_some_and(|resource| self.service.is_pending(&resource.source)))
        })
    }
}

fn bytes_worker(provider: Arc<dyn ResourceProvider>, rx: mpsc::Receiver<BytesRequest>, tx: mpsc::Sender<DecodeRequest>, state: Arc<Mutex<SharedState>>) {
    while let Ok(request) = rx.recv() {
        match load_bytes(provider.as_ref(), &request.source) {
            Ok(bytes) => {
                if tx.send(DecodeRequest { key: request.key.clone(), bytes: Arc::new(bytes) }).is_err() {
                    state.lock().expect("image service mutex poisoned").pending.remove(&request.key);
                }
            }
            Err(_) => {
                state.lock().expect("image service mutex poisoned").pending.remove(&request.key);
            }
        }
    }
}

fn decode_worker(rx: mpsc::Receiver<DecodeRequest>, state: Arc<Mutex<SharedState>>) {
    while let Ok(request) = rx.recv() {
        let decoded = decode_image(&request.bytes);
        let mut state = state.lock().expect("image service mutex poisoned");
        state.pending.remove(&request.key);
        let Ok(decoded) = decoded else { continue };
        let byte_len = match &decoded {
            DecodedImage::Raster { byte_len, .. } => *byte_len,
            DecodedImage::Svg { bytes, .. } => bytes.len(),
        };
        state.clock = state.clock.wrapping_add(1);
        state.revision = state.revision.wrapping_add(1);
        let entry = SharedImage { decoded: Arc::new(decoded), byte_len, revision: state.revision, last_used: state.clock };
        let key = request.key;
        if let Some(previous) = state.decoded.insert(key.clone(), entry) {
            state.decoded_bytes = state.decoded_bytes.saturating_sub(previous.byte_len);
        }
        state.decoded_bytes = state.decoded_bytes.saturating_add(byte_len);
        // Keep a newly completed decode available for at least one poll. If
        // the visible working set itself exceeds the cache budget, inactive
        // pipelines retrim it as soon as they release their pins.
        trim_decoded_cache(&mut state, Some(&key));
    }
}

fn trim_decoded_cache(state: &mut SharedState, protected: Option<&ImageKey>) {
    while state.decoded_bytes > state.byte_budget && state.decoded.len() > 1 {
        let candidate = state
            .decoded
            .iter()
            .filter(|(key, image)| protected != Some(*key) && Arc::strong_count(&image.decoded) == 1)
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
        return Ok(DecodedImage::Svg { bytes: Arc::from(bytes), hash, width, height });
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

    Ok(DecodedImage::Raster { image: peniko, hash, byte_len })
}

pub fn probe_dimensions(provider: &dyn ResourceProvider, source: &ImageSource) -> Option<(u32, u32)> {
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
    let (number, pixels_per_unit) = [("px", 1.0), ("in", 96.0), ("cm", 96.0 / 2.54), ("mm", 96.0 / 25.4), ("q", 96.0 / 101.6), ("pt", 96.0 / 72.0), ("pc", 16.0)]
        .into_iter()
        .find_map(|(suffix, scale)| value.strip_suffix(suffix).map(|number| (number, scale)))
        .unwrap_or((value, 1.0));
    let value = number.trim().parse::<f64>().ok()? * pixels_per_unit;
    (value.is_finite() && value > 0.0).then(|| value.round().max(1.0) as u32)
}

fn load_bytes(provider: &dyn ResourceProvider, source: &ImageSource) -> std::io::Result<Vec<u8>> {
    match source {
        ImageSource::Uri(uri) => provider.read_bytes(uri),
        ImageSource::Inline(bytes) => Ok(bytes.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::{DecodedImage, ImagePipeline, ImageService, decode_image};
    use crate::document::{ImageResource, ImageSource};
    use crate::resources::{ResourceProvider, TocEntry};
    use std::collections::HashSet;
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    struct CountingProvider(AtomicUsize);

    impl ResourceProvider for CountingProvider {
        fn read_bytes(&self, _uri: &str) -> io::Result<Vec<u8>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(br#"<svg xmlns='http://www.w3.org/2000/svg' width='20' height='10'/>"#.to_vec())
        }

        fn exists(&self, _uri: &str) -> bool { true }
        fn resolve(&self, _base: &str, href: &str) -> String { href.to_owned() }
        fn list_html_candidates(&self, _root: &str) -> io::Result<Vec<String>> { Ok(Vec::new()) }
        fn toc(&self) -> io::Result<Option<Vec<TocEntry>>> { Ok(None) }
    }

    #[test]
    fn relative_svg_viewport_is_retained_for_backend_rendering() {
        let decoded = decode_image(br#"<svg xmlns="http://www.w3.org/2000/svg" height="50%"><rect width="20" height="10" fill="blue"/></svg>"#).expect("SVG should decode as a vector resource");
        assert!(matches!(decoded, DecodedImage::Svg { .. }));
        assert_eq!(decoded.dimensions(), (300, 150));
    }

    #[test]
    fn pipelines_sharing_a_service_decode_a_source_once() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let service = ImageService::new(provider.clone(), 1024 * 1024);
        let resource = ImageResource { source: ImageSource::Uri("shared.svg".to_owned()), width: 0, height: 0, width_attr: None, height_attr: None };
        let mut first = ImagePipeline::with_service(Arc::new(vec![resource.clone()]), service.clone());
        let mut second = ImagePipeline::with_service(Arc::new(vec![resource]), service);
        let desired = HashSet::from([0]);
        first.ensure_window(&desired);
        second.ensure_window(&desired);

        let deadline = Instant::now() + Duration::from_secs(2);
        while (first.get_decoded(0).is_none() || second.get_decoded(0).is_none()) && Instant::now() < deadline {
            first.poll();
            second.poll();
            std::thread::yield_now();
        }

        assert!(first.get_decoded(0).is_some() && second.get_decoded(0).is_some());
        assert_eq!(provider.0.load(Ordering::Relaxed), 1, "the source is fetched and decoded once for both document-local indices");
    }

    #[test]
    fn shared_service_enforces_one_decoded_byte_budget() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let service = ImageService::new(provider, 80);
        let resources = Arc::new(vec![
            ImageResource { source: ImageSource::Uri("first.svg".to_owned()), width: 0, height: 0, width_attr: None, height_attr: None },
            ImageResource { source: ImageSource::Uri("second.svg".to_owned()), width: 0, height: 0, width_attr: None, height_attr: None },
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
        assert_eq!(state.decoded.len(), 1, "the unpinned least-recently-used decode is evicted");
    }

    #[test]
    fn deactivated_pipeline_does_not_pin_shared_decodes() {
        let provider = Arc::new(CountingProvider(AtomicUsize::new(0)));
        let service = ImageService::new(provider, 80);
        let resource = |uri: &str| ImageResource { source: ImageSource::Uri(uri.to_owned()), width: 0, height: 0, width_attr: None, height_attr: None };
        let desired = HashSet::from([0]);
        let mut first = ImagePipeline::with_service(Arc::new(vec![resource("first.svg")]), service.clone());
        first.ensure_window(&desired);
        let deadline = Instant::now() + Duration::from_secs(2);
        while first.get_decoded(0).is_none() && Instant::now() < deadline {
            first.poll();
            std::thread::yield_now();
        }
        assert!(first.get_decoded(0).is_some());

        let mut second = ImagePipeline::with_service(Arc::new(vec![resource("second.svg")]), service.clone());
        second.ensure_window(&desired);
        let second_deadline = Instant::now() + Duration::from_secs(2);
        while second.get_decoded(0).is_none() && Instant::now() < second_deadline {
            second.poll();
            std::thread::yield_now();
        }
        assert!(second.get_decoded(0).is_some(), "the newly decoded visible image must not evict itself");
        {
            let state = service.state.lock().expect("image service mutex poisoned");
            assert_eq!(state.decoded.len(), 2, "the two-image visible working set may temporarily exceed the cache budget");
        }

        first.deactivate();
        assert!(first.get_decoded(0).is_none());
        let state = service.state.lock().expect("image service mutex poisoned");
        assert_eq!(state.decoded.len(), 1, "releasing an inactive document immediately retrims the cache");
        assert!(second.get_decoded(0).is_some());
    }
}
