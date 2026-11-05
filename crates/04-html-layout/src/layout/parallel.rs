use rayon::{ThreadPool, ThreadPoolBuilder};
use std::sync::{Arc, Mutex};

// Retain one configuration. In-flight work keeps a replaced pool alive only
// until its last operation finishes.
static POOL: Mutex<Option<(usize, Arc<ThreadPool>)>> = Mutex::new(None);

#[cfg(any(test, feature = "testing"))]
static OBSERVED_WORKERS: std::sync::LazyLock<
    Mutex<std::collections::HashSet<std::thread::ThreadId>>,
> = std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

/// Return None when workers cannot be created, so the caller can execute its
/// ordinary sequential path without accidentally using Rayon's global pool.
pub fn install<R: Send>(workers: usize, operation: impl FnOnce() -> R + Send) -> Option<R> {
    let pool = {
        let mut cached = POOL.lock().unwrap_or_else(|error| error.into_inner());
        if let Some((count, pool)) = &*cached
            && *count == workers
        {
            pool.clone()
        } else {
            let pool = Arc::new(
                ThreadPoolBuilder::new()
                    .num_threads(workers)
                    .thread_name(move |index| format!("html-layout-{workers}-{index}"))
                    .build()
                    .ok()?,
            );
            *cached = Some((workers, pool.clone()));
            pool
        }
    };
    Some(pool.install(operation))
}

#[cfg(any(test, feature = "testing"))]
pub fn record_worker() {
    OBSERVED_WORKERS
        .lock()
        .expect("parallel-layout observation lock poisoned")
        .insert(std::thread::current().id());
}

#[cfg(any(test, feature = "testing"))]
pub fn reset_observed_workers() {
    OBSERVED_WORKERS
        .lock()
        .expect("parallel-layout observation lock poisoned")
        .clear();
}

#[cfg(any(test, feature = "testing"))]
pub fn observed_worker_count() -> usize {
    OBSERVED_WORKERS
        .lock()
        .expect("parallel-layout observation lock poisoned")
        .len()
}
