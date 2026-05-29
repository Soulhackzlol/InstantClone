//! Self-process CPU% and resident-set-size sampling for the dashboard
//! metric grid. Windows-only hand-rolled FFI (no `sysinfo` / `procfs`
//! dependency); other platforms compile to inert no-op stubs that report
//! zero. Adding /proc and Mach impls later is a few dozen lines.

use std::sync::Mutex;
use std::time::Instant;

/// One sampler per process. Cheap to create; calling `sample()` more
/// than once per second is wasted work since the OS counters update in
/// 100 ns ticks and CPU% needs a non-trivial wall delta to be meaningful.
pub struct SysStat {
    // Previous (user + kernel) jiffy total and the wall time we read it.
    // Mutex<…> is fine — there's exactly one caller (the web /state path).
    prev: Mutex<Option<(u64, Instant)>>,
}

impl SysStat {
    pub fn new() -> Self {
        Self { prev: Mutex::new(None) }
    }

    /// Returns (cpu_percent, rss_bytes). On unsupported platforms, both
    /// are zero. Errors are swallowed — a missing metric is better than
    /// a 500 from /state.
    pub fn sample(&self) -> (f32, u64) {
        platform::sample(self)
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::SysStat;
    use std::time::Instant;

    #[repr(C)]
    struct Filetime { dw_low_date_time: u32, dw_high_date_time: u32 }

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        // remaining fields unused — laid out for size only so cb is correct.
        _quota_peak_paged_pool_usage: usize,
        _quota_paged_pool_usage: usize,
        _quota_peak_non_paged_pool_usage: usize,
        _quota_non_paged_pool_usage: usize,
        _pagefile_usage: usize,
        _peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetProcessTimes(
            h_process: *mut std::ffi::c_void,
            lp_creation_time: *mut Filetime,
            lp_exit_time: *mut Filetime,
            lp_kernel_time: *mut Filetime,
            lp_user_time: *mut Filetime,
        ) -> i32;
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            ppsmem_counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    fn ft_to_100ns(ft: &Filetime) -> u64 {
        ((ft.dw_high_date_time as u64) << 32) | (ft.dw_low_date_time as u64)
    }

    pub fn sample(s: &SysStat) -> (f32, u64) {
        let proc = unsafe { GetCurrentProcess() };

        // CPU: user + kernel jiffies (in 100 ns units). We compute the
        // delta since last call and divide by wall-time delta to get %.
        let mut creation = Filetime { dw_low_date_time: 0, dw_high_date_time: 0 };
        let mut exit     = Filetime { dw_low_date_time: 0, dw_high_date_time: 0 };
        let mut kernel   = Filetime { dw_low_date_time: 0, dw_high_date_time: 0 };
        let mut user     = Filetime { dw_low_date_time: 0, dw_high_date_time: 0 };
        let now = Instant::now();
        let cpu_pct = if unsafe {
            GetProcessTimes(proc, &mut creation, &mut exit, &mut kernel, &mut user)
        } != 0 {
            let total = ft_to_100ns(&kernel).saturating_add(ft_to_100ns(&user));
            let mut prev = s.prev.lock().unwrap();
            let pct = if let Some((prev_total, prev_t)) = *prev {
                let dt_ns = now.duration_since(prev_t).as_nanos() as u64;
                let dt_100ns = dt_ns / 100;
                let work = total.saturating_sub(prev_total);
                if dt_100ns == 0 { 0.0 }
                else { (work as f64 / dt_100ns as f64) as f32 * 100.0 }
            } else { 0.0 };
            *prev = Some((total, now));
            pct
        } else { 0.0 };

        // RSS: working set size. Sized to current PROCESS_MEMORY_COUNTERS.
        let mut pmc: ProcessMemoryCounters = unsafe { std::mem::zeroed() };
        pmc.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        let rss = if unsafe {
            GetProcessMemoryInfo(proc, &mut pmc, pmc.cb)
        } != 0 {
            pmc.working_set_size as u64
        } else { 0 };

        // CPU% can briefly exceed 100 on multi-core spikes; clamp display
        // to a friendlier ceiling so the UI doesn't show "732%".
        (cpu_pct.clamp(0.0, 100.0 * num_cpus_hint() as f32), rss)
    }

    fn num_cpus_hint() -> u32 {
        // std::thread::available_parallelism is the cheapest cross-version
        // path; fall back to 1 to avoid clamping to zero on weird hosts.
        std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1)
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::SysStat;
    pub fn sample(_s: &SysStat) -> (f32, u64) { (0.0, 0) }
}
