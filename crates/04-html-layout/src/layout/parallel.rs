use rayon::{ThreadPool, ThreadPoolBuilder};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

// Common worker caps need no registry lock or Arc clone after initialization.
static COMMON_POOLS: [OnceLock<ThreadPool>; 17] = [const { OnceLock::new() }; 17];

static POOLS: OnceLock<Mutex<HashMap<usize, Arc<ThreadPool>>>> = OnceLock::new();

#[cfg(test)]
static OBSERVED_WORKERS: std::sync::LazyLock<
    Mutex<std::collections::HashSet<std::thread::ThreadId>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

/// Runs dependency-independent layout work on a reusable, size-specific pool.
/// Keeping pools outside documents avoids retaining operating-system threads
/// in every cached layout while still making the worker cap deterministic.
pub(crate) fn install<R: Send>(workers: usize, operation: impl FnOnce() -> R + Send) -> R {
    debug_assert!(workers > 1);
    if let Some(slot) = COMMON_POOLS.get(workers) {
        return slot.get_or_init(|| build_pool(workers)).install(operation);
    }
    let pools = POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    let pool = {
        let mut pools = pools.lock().expect("layout worker-pool registry poisoned");
        pools
            .entry(workers)
            .or_insert_with(|| Arc::new(build_pool(workers)))
            .clone()
    };
    pool.install(operation)
}

fn build_pool(workers: usize) -> ThreadPool {
    ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(move |index| format!("html-layout-{workers}-{index}"))
        .build()
        .expect("layout worker pool must initialize")
}

#[cfg(test)]
pub(crate) fn record_worker() {
    OBSERVED_WORKERS
        .lock()
        .expect("parallel-layout observation lock poisoned")
        .insert(std::thread::current().id());
}

#[cfg(test)]
pub(crate) fn reset_observed_workers() {
    OBSERVED_WORKERS
        .lock()
        .expect("parallel-layout observation lock poisoned")
        .clear();
}

#[cfg(test)]
pub(crate) fn observed_worker_count() -> usize {
    OBSERVED_WORKERS
        .lock()
        .expect("parallel-layout observation lock poisoned")
        .len()
}
