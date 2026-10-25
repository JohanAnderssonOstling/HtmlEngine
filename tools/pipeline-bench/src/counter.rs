//! Linux user-mode instruction counts, gated to each measured operation.
//! The 64-byte v0 ABI and ioctl values follow linux/perf_event.h.

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
    fd: libc::c_int,
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
            fd: fd as libc::c_int,
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
        (running != 0)
            .then(|| ((u128::from(value[0]) * u128::from(enabled)) / u128::from(running)) as u64)
    }
}
impl Drop for InstructionCounter {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}
