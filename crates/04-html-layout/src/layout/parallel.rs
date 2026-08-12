use rayon::ThreadPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

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
    let pools = POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    let pool = {
        let mut pools = pools.lock().expect("layout worker-pool registry poisoned");
        pools
            .entry(workers)
            .or_insert_with(|| {
                Arc::new(
                    rayon::ThreadPoolBuilder::new()
                        .num_threads(workers)
                        .thread_name(move |index| format!("html-layout-{workers}-{index}"))
                        .build()
                        .expect("layout worker pool must initialize"),
                )
            })
            .clone()
    };
    pool.install(operation)
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
