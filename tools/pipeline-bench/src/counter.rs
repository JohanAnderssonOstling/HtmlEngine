//! Linux user-mode PMU counts, gated to each measured operation.
//! The 64-byte v0 ABI and ioctl values follow linux/perf_event.h.

use libc::c_int;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::__cpuid;

#[repr(C)]
struct EventAttr {
    event_type: u32,
    size: u32,
    config: u64,
    sample_period: u64,
    sample_type: u64,
    read_format: u64,
    flags: u64,
    wakeup_events: u32,
    breakpoint_type: u32,
    config1: u64,
}
const _: () = assert!(size_of::<EventAttr>() == 64);

pub struct InstructionCounter {
    fd: c_int,
    baseline: Option<[u64; 3]>,
}

impl InstructionCounter {
    pub fn new() -> Option<Self> {
        let attr = EventAttr {
            event_type: 0,
            size: 64,
            config: 1,
            sample_period: 0,
            sample_type: 0,
            read_format: 3,
            flags: 1 | (1 << 1) | (1 << 5) | (1 << 6),
            wakeup_events: 0,
            breakpoint_type: 0,
            config1: 0,
        };
        let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, &attr, 0, -1, -1, 1u64 << 3) };
        (fd >= 0).then_some(Self {
            fd: fd as c_int,
            baseline: None,
        })
    }
    fn read(&self) -> Option<[u64; 3]> {
        let mut value = [0u64; 3];
        let bytes = unsafe { libc::read(self.fd, value.as_mut_ptr().cast(), size_of_val(&value)) };
        (bytes == size_of_val(&value) as isize).then_some(value)
    }
    pub fn start(&mut self) {
        self.baseline = None;
        if unsafe { libc::ioctl(self.fd, 0x2403) } != 0 {
            return;
        }
        self.baseline = self.read();
        if unsafe { libc::ioctl(self.fd, 0x2400) } != 0 {
            self.baseline = None;
        }
    }
    pub fn stop(&mut self) -> Option<u64> {
        if unsafe { libc::ioctl(self.fd, 0x2401) } != 0 {
            return None;
        }
        let baseline = self.baseline.take()?;
        let value = self.read()?;
        let enabled = value[1].checked_sub(baseline[1])?;
        let running = value[2].checked_sub(baseline[2])?;
        let count = value[0].checked_sub(baseline[0])?;
        (running != 0)
            .then(|| ((u128::from(count) * u128::from(enabled)) / u128::from(running)) as u64)
    }
}
impl Drop for InstructionCounter {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

/// Per-thread AMD data-cache events, read as a group over one opening load.
/// Group reads include running/enabled time so multiplexed results are visible.
pub struct CacheCounters {
    fds: Vec<c_int>,
    previous_times: [u64; 2],
}

impl CacheCounters {
    pub const EVENTS: [(&str, u64); 4] = [
        ("l2_data_hits", 0xf064),
        ("l2_data_misses", 0x0864),
        ("load_dispatches", 0x0129),
        ("store_dispatches", 0x0229),
    ];

    pub fn new() -> Option<Self> {
        // These raw encodings are validated for the benchmark host's
        // Rembrandt (Zen 3+) PMU. Avoid plausible-looking counts elsewhere.
        #[cfg(target_arch = "x86_64")]
        let supported = {
            let vendor = __cpuid(0);
            let cpu = __cpuid(1);
            let family = ((cpu.eax >> 8) & 0xf) + ((cpu.eax >> 20) & 0xff);
            let model = ((cpu.eax >> 4) & 0xf) | ((cpu.eax >> 12) & 0xf0);
            vendor.ebx == 0x6874_7541
                && vendor.edx == 0x6974_6e65
                && vendor.ecx == 0x444d_4163
                && family == 0x19
                && model & 0xf0 == 0x40
        };
        #[cfg(not(target_arch = "x86_64"))]
        let supported = false;
        if !supported {
            return None;
        }
        let mut group = Self {
            fds: Vec::new(),
            previous_times: [0; 2],
        };
        for (_, config) in Self::EVENTS {
            let attr = EventAttr {
                event_type: 4, // PERF_TYPE_RAW; AMD Zen 3 event/umask encoding.
                size: 64,
                config,
                sample_period: 0,
                sample_type: 0,
                read_format: 1 | 2 | 8, // TIME_ENABLED | TIME_RUNNING | GROUP
                flags: 1 | (1 << 5) | (1 << 6), // disabled, exclude kernel/HV
                wakeup_events: 0,
                breakpoint_type: 0,
                config1: 0,
            };
            let fd = unsafe {
                libc::syscall(
                    libc::SYS_perf_event_open,
                    &attr,
                    0,
                    -1,
                    group.fds.first().copied().unwrap_or(-1),
                    1u64 << 3,
                )
            };
            if fd < 0 {
                return None;
            }
            group.fds.push(fd as c_int);
        }
        Some(group)
    }

    pub fn start(&mut self) {
        let fd = self.fds[0];
        assert_eq!(
            unsafe { libc::ioctl(fd, 0x2403, 1) },
            0,
            "reset cache counters"
        );
        assert_eq!(
            unsafe { libc::ioctl(fd, 0x2400, 1) },
            0,
            "enable cache counters"
        );
    }

    pub fn stop(&mut self) -> Vec<(&'static str, u64)> {
        let fd = self.fds[0];
        assert_eq!(
            unsafe { libc::ioctl(fd, 0x2401, 1) },
            0,
            "disable cache counters"
        );
        let mut values = [0u64; 3 + Self::EVENTS.len()];
        assert_eq!(
            unsafe { libc::read(fd, values.as_mut_ptr().cast(), size_of_val(&values)) },
            size_of_val(&values) as isize,
            "read cache counters",
        );
        assert_eq!(values[0], Self::EVENTS.len() as u64);
        let times = [values[1], values[2]];
        let elapsed = [
            times[0] - self.previous_times[0],
            times[1] - self.previous_times[1],
        ];
        self.previous_times = times;
        Self::EVENTS
            .iter()
            .zip(&values[3..])
            .map(|((name, _), value)| (*name, *value))
            .chain([
                ("time_enabled_ns", elapsed[0]),
                ("time_running_ns", elapsed[1]),
            ])
            .collect()
    }
}

impl Drop for CacheCounters {
    fn drop(&mut self) {
        for &fd in &self.fds {
            unsafe { libc::close(fd) };
        }
    }
}
