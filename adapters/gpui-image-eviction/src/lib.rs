//! Budgeted retention of decoded images and their GPUI atlas entries.

use gpui::{App, Global, ImageId, RenderImage, Window};
use std::collections::{HashMap, hash_map::Entry};
use std::sync::Arc;

// WebAssembly chooses a mobile or desktop budget at runtime.
const CONSTRAINED_DEVICE: bool = cfg!(any(all(target_pointer_width = "32", not(target_arch = "wasm32")), target_os = "android"));
const IMAGE_CACHE_BYTES: usize = if CONSTRAINED_DEVICE && !cfg!(feature = "kobo") {
    16 * 1024 * 1024
} else {
    128 * 1024 * 1024
};

fn image_cache_bytes() -> usize {
    #[cfg(target_arch = "wasm32")]
    if web_sys::window().and_then(|window| window.match_media("(pointer: coarse)").ok().flatten()).is_some_and(|query| query.matches()) {
        return 32 * 1024 * 1024;
    }
    IMAGE_CACHE_BYTES
}

struct CachedImage {
    image: Arc<RenderImage>,
    bytes: usize,
    last_used: u64,
}

struct PaintedImages {
    images: HashMap<ImageId, CachedImage>,
    bytes: usize,
    budget: usize,
    target: usize,
    clock: u64,
    cleanup_scheduled: bool,
    trim_requested: bool,
}

impl Default for PaintedImages {
    fn default() -> Self {
        let budget = image_cache_bytes();
        Self::new(budget, budget * 3 / 4)
    }
}

impl PaintedImages {
    fn new(budget: usize, target: usize) -> Self {
        assert!(target <= budget);
        Self { images: HashMap::new(), bytes: 0, budget, target, clock: 0, cleanup_scheduled: false, trim_requested: false }
    }

    fn track(&mut self, image: &Arc<RenderImage>) -> bool {
        self.clock += 1;
        match self.images.entry(image.id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().last_used = self.clock;
                false
            }
            Entry::Vacant(entry) => {
                let bytes = (0..image.frame_count()).filter_map(|frame| image.as_bytes(frame)).map(<[u8]>::len).sum();
                self.bytes += bytes;
                entry.insert(CachedImage { image: image.clone(), bytes, last_used: self.clock });
                true
            }
        }
    }

    fn evict_unused(&mut self) -> Vec<Arc<RenderImage>> {
        if self.bytes <= self.budget {
            return Vec::new();
        }
        self.evict_to(self.target, true)
    }

    fn evict_to(&mut self, target: usize, keep_newest: bool) -> Vec<Arc<RenderImage>> {
        let mut unused: Vec<_> = self
            .images
            .iter()
            // The newest image may still be waiting for its first frame.
            // Keep it even when all older images have external owners.
            .filter(|(_, entry)| (!keep_newest || entry.last_used != self.clock) && Arc::strong_count(&entry.image) == 1)
            .map(|(&id, entry)| (id, entry.last_used))
            .collect();
        unused.sort_unstable_by_key(|&(_, last_used)| last_used);
        let mut evicted = Vec::new();
        for (id, _) in unused {
            if self.bytes <= target {
                break;
            }
            let Some(entry) = self.images.remove(&id) else { continue };
            self.bytes -= entry.bytes;
            evicted.push(entry.image);
        }
        evicted
    }
}

impl Global for PaintedImages {}

/// Retain decoded pixels for reuse, both when installing prefetched images
/// and when painting directly with `Window::paint_image`.
///
/// Only this cache's final reference is evictable. Visible frames, page caches
/// and readers protect their images through ordinary Arc ownership. The byte
/// budget is soft while these owners remain. Unused pixels stay cached until
/// pressure or an explicit transition trim; there are no periodic cleanup tasks.
pub fn track_image(image: &Arc<RenderImage>, cx: &mut App) {
    let state = cx.default_global::<PaintedImages>();
    let inserted = state.track(image);
    if !inserted || state.bytes <= state.budget || state.cleanup_scheduled {
        return;
    }
    schedule_cleanup(cx);
}

/// Release unused decoded pixels and atlas entries at a UI transition, even
/// below the normal cache budget. External owners always protect their images.
pub fn trim_unused_images(cx: &mut App) {
    if cx.try_global::<PaintedImages>().is_none() {
        return;
    }
    cx.global_mut::<PaintedImages>().trim_requested = true;
    if !cx.global::<PaintedImages>().cleanup_scheduled {
        schedule_cleanup(cx);
    }
}

/// Release unused images after a view transition has replaced the outgoing frame.
/// Images still owned by another reader, page cache or window remain protected.
pub fn trim_unused_images_after_transition(window: &Window) {
    // The first callback can precede the replacement draw. The second runs
    // after that draw has released the outgoing frame's image references.
    window.on_next_frame(|window, _| window.on_next_frame(|_, cx| trim_unused_images(cx)));
}

fn schedule_cleanup(cx: &mut App) {
    cx.global_mut::<PaintedImages>().cleanup_scheduled = true;
    // Paint can borrow a window out of App. Defer and coalesce the sweep so
    // atlas entries are removed from every window before decoded pixels drop.
    cx.defer(|cx| {
        let state = cx.global_mut::<PaintedImages>();
        state.cleanup_scheduled = false;
        let evicted = if std::mem::take(&mut state.trim_requested) { state.evict_to(0, false) } else { state.evict_unused() };
        for image in evicted {
            cx.drop_image(image, None);
        }
    });
}

#[cfg(test)]
mod tests;
