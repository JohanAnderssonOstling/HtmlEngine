//! Opt-in, current-thread scope telemetry for benchmark builds.
//!
//! A caller supplies a monotonic CPU clock. Inclusive durations include child
//! scopes; exclusive durations subtract them, including recursive calls. Other
//! threads are not instrumented. Clock and bookkeeping costs are not corrected.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub name: &'static str,
    pub calls: usize,
    pub inclusive_wall: Duration,
    pub exclusive_wall: Duration,
    pub inclusive_cpu: Duration,
    pub exclusive_cpu: Duration,
    pub measurements: Vec<BTreeMap<&'static str, usize>>,
}

struct Frame {
    name: &'static str,
    wall: Instant,
    cpu: Duration,
    child_wall: Duration,
    child_cpu: Duration,
}

struct Recorder {
    clock: fn() -> Duration,
    stack: Vec<Frame>,
    entries: BTreeMap<&'static str, Entry>,
    measurements: bool,
}

thread_local! {
    static RECORDER: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

/// Capture only scopes executed synchronously by `operation` on this thread.
/// Nesting captures is an error. A panic clears the recorder during unwinding.
pub fn capture<R>(clock: fn() -> Duration, operation: impl FnOnce() -> R) -> (R, Vec<Entry>) {
    capture_with_measurements(clock, false, operation)
}

/// Detailed counters are separately enabled so ordinary timing captures do
/// not add bookkeeping to hot loops.
pub fn capture_with_measurements<R>(clock: fn() -> Duration, measurements: bool, operation: impl FnOnce() -> R) -> (R, Vec<Entry>) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            RECORDER.with(|state| *state.borrow_mut() = None);
        }
    }
    RECORDER.with(|state| {
        let mut state = state.borrow_mut();
        assert!(state.is_none(), "profiling captures cannot be nested");
        *state = Some(Recorder {
            clock,
            stack: Vec::new(),
            entries: BTreeMap::new(),
            measurements,
        });
    });
    let reset = Reset;
    let output = operation();
    let entries = RECORDER.with(|state| {
        let recorder = state.borrow_mut().take().expect("active profiling capture");
        assert!(
            recorder.stack.is_empty(),
            "profiling scope outlived capture"
        );
        recorder.entries.into_values().collect()
    });
    drop(reset);
    (output, entries)
}

pub(crate) fn measurements_enabled() -> bool {
    RECORDER.with(|state| state.borrow().as_ref().is_some_and(|recorder| recorder.measurements))
}

pub(crate) fn record_measurement(name: &'static str, values: &[(&'static str, usize)]) {
    RECORDER.with(|state| {
        let mut state = state.borrow_mut();
        let Some(recorder) = state.as_mut().filter(|recorder| recorder.measurements) else { return };
        recorder.entries.entry(name).or_insert_with(|| Entry { name, ..Entry::default() })
            .measurements.push(values.iter().copied().collect());
    });
}

/// A scope must be dropped on its originating thread, in stack order.
pub struct Scope {
    depth: Option<usize>,
    _thread: PhantomData<Rc<()>>,
}

pub fn scope(name: &'static str) -> Scope {
    let depth = RECORDER.with(|state| {
        let mut state = state.borrow_mut();
        let recorder = state.as_mut()?;
        let depth = recorder.stack.len();
        recorder.stack.push(Frame {
            name,
            wall: Instant::now(),
            cpu: (recorder.clock)(),
            child_wall: Duration::ZERO,
            child_cpu: Duration::ZERO,
        });
        Some(depth)
    });
    Scope {
        depth,
        _thread: PhantomData,
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let Some(depth) = self.depth else { return };
        RECORDER.with(|state| {
            let mut state = state.borrow_mut();
            let recorder = state.as_mut().expect("active profiling capture");
            assert_eq!(
                recorder.stack.len(),
                depth + 1,
                "profiling scopes must close in stack order"
            );
            let cpu = (recorder.clock)();
            let frame = recorder.stack.pop().expect("active profiling scope");
            let wall = frame.wall.elapsed();
            let cpu = cpu.saturating_sub(frame.cpu);
            if let Some(parent) = recorder.stack.last_mut() {
                parent.child_wall += wall;
                parent.child_cpu += cpu;
            }
            let entry = recorder.entries.entry(frame.name).or_insert_with(|| Entry {
                name: frame.name,
                ..Entry::default()
            });
            entry.calls += 1;
            entry.inclusive_wall += wall;
            entry.exclusive_wall += wall.saturating_sub(frame.child_wall);
            entry.inclusive_cpu += cpu;
            entry.exclusive_cpu += cpu.saturating_sub(frame.child_cpu);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    thread_local! { static CPU: Cell<u64> = const { Cell::new(0) }; }
    thread_local! { static CLOCK_READS: Cell<usize> = const { Cell::new(0) }; }
    fn clock() -> Duration {
        CLOCK_READS.with(|reads| reads.set(reads.get() + 1));
        Duration::from_nanos(CPU.with(Cell::get))
    }
    fn advance(n: u64) {
        CPU.with(|cpu| cpu.set(cpu.get() + n));
    }

    #[test]
    fn exclusive_cpu_subtracts_children_and_recursion() {
        let (_, entries) = capture(clock, || {
            let _parent = scope("parent");
            advance(10);
            {
                let _child = scope("child");
                advance(20);
            }
            {
                let _recursive = scope("parent");
                advance(30);
            }
            advance(40);
        });
        let parent = entries.iter().find(|e| e.name == "parent").unwrap();
        let child = entries.iter().find(|e| e.name == "child").unwrap();
        assert_eq!(parent.calls, 2);
        assert_eq!(parent.inclusive_cpu, Duration::from_nanos(130));
        assert_eq!(parent.exclusive_cpu, Duration::from_nanos(80));
        assert_eq!(child.exclusive_cpu, Duration::from_nanos(20));
        assert_eq!(
            entries.iter().map(|e| e.exclusive_cpu).sum::<Duration>(),
            Duration::from_nanos(100)
        );
    }

    #[test]
    fn measurements_are_opt_in_and_do_not_change_scope_call_counts() {
        let (_, entries) = capture(clock, || record_measurement("test", &[("value", 7)]));
        assert!(entries.is_empty());
        let (_, entries) = capture_with_measurements(clock, true, || {
            let _scope = scope("test");
            assert!(measurements_enabled());
            record_measurement("test", &[("value", 7)]);
            record_measurement("test", &[("value", 11)]);
        });
        assert_eq!(entries[0].calls, 1);
        assert_eq!(entries[0].measurements.len(), 2);
        assert_eq!(entries[0].measurements[0]["value"], 7);
        assert_eq!(entries[0].measurements[1]["value"], 11);
        assert!(!measurements_enabled());
    }

    #[test]
    fn panic_clears_capture_and_disabled_scopes_do_not_read_clock() {
        let before = CLOCK_READS.with(Cell::get);
        drop(scope("inactive"));
        assert_eq!(CLOCK_READS.with(Cell::get), before);
        let result = std::panic::catch_unwind(|| {
            capture(clock, || {
                let _scope = scope("panic");
                panic!("test panic");
            })
        });
        assert!(result.is_err());
        let (_, entries) = capture(clock, || drop(scope("recovered")));
        assert_eq!(entries[0].name, "recovered");
    }
}
