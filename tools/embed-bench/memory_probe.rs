//! Untimed Linux memory diagnostics. Stops at checkpoints for external smaps.

use libc::{SIGSTOP, mallinfo2, raise};
use serde_json::{Value, json};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

struct Tracked;

fn added(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
    ALLOCATED.fetch_add(size, Relaxed);
}

unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            added(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            added(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            if size >= layout.size() {
                let live = LIVE.fetch_add(size - layout.size(), Relaxed) + size - layout.size();
                PEAK.fetch_max(live, Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - size, Relaxed);
            }
            ALLOCATED.fetch_add(size, Relaxed);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

pub fn sample() -> Value {
    let live = LIVE.load(Relaxed);
    let peak = PEAK.load(Relaxed);
    let allocated = ALLOCATED.load(Relaxed);
    let info = unsafe { mallinfo2() };
    json!({"rust_live_requested_bytes":live,"rust_peak_requested_bytes":peak,
        "rust_cumulative_requested_bytes":allocated,
        "glibc":{"arena_bytes":info.arena,"allocated_arena_bytes":info.uordblks,
            "free_arena_bytes":info.fordblks,"mmap_bytes":info.hblkhd,
            "mmap_regions":info.hblks,"top_releasable_bytes":info.keepcost}})
}

pub fn pause() {
    // The controller resumes only after reading this process's smaps, so
    // each mapping snapshot corresponds to a known pipeline boundary.
    unsafe { raise(SIGSTOP) };
}

pub fn reset_peak() {
    PEAK.store(LIVE.load(Relaxed), Relaxed);
}
