//! Process readings for the perf probe: resident memory, CPU time, process start time, and a
//! counting allocator for allocation rate and live heap. macOS has full support; Linux reads
//! `/proc`; elsewhere the readings are 0 or fall back to the time `main` started.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// The system allocator plus two relaxed counters. The cost is two atomic adds per call, which is
/// small next to the allocation itself; it is always on so the measured binary is the shipped one.
pub struct CountingAllocator;

static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static FREED: AtomicU64 = AtomicU64::new(0);

// SAFETY: every method forwards to `System` with the caller's arguments unchanged.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATED.fetch_add(new_size as u64, Ordering::Relaxed);
        FREED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Bytes allocated since start (only counts when [`CountingAllocator`] is the global allocator).
pub fn allocated_bytes() -> u64 {
    ALLOCATED.load(Ordering::Relaxed)
}

/// Bytes currently allocated through the global allocator.
pub fn live_heap_bytes() -> u64 {
    ALLOCATED
        .load(Ordering::Relaxed)
        .saturating_sub(FREED.load(Ordering::Relaxed))
}

static MAIN_STARTED: OnceLock<(Instant, SystemTime)> = OnceLock::new();

/// Call first thing in `main`; the fallback for the process start time.
pub fn mark_main_started() {
    let _ = MAIN_STARTED.set((Instant::now(), SystemTime::now()));
}

/// Time since the process started (from the kernel where possible, else since `main`).
pub fn since_process_start() -> Duration {
    if let Some(start) = process_start_time()
        && let Ok(elapsed) = SystemTime::now().duration_since(start)
    {
        return elapsed;
    }
    MAIN_STARTED.get().map_or(Duration::ZERO, |(i, _)| i.elapsed())
}

/// User plus system CPU time of this process.
pub fn cpu_time() -> Duration {
    #[cfg(unix)]
    {
        // SAFETY: getrusage writes into the zeroed struct we pass and nothing else.
        let usage = unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            libc::getrusage(libc::RUSAGE_SELF, &mut usage);
            usage
        };
        let tv = |t: libc::timeval| Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64);
        tv(usage.ru_utime) + tv(usage.ru_stime)
    }
    #[cfg(not(unix))]
    {
        Duration::ZERO
    }
}

/// Resident set size in bytes.
pub fn resident_bytes() -> u64 {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: proc_pidinfo fills at most `size` bytes of the zeroed struct.
        unsafe {
            let mut info: libc::proc_taskinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
            let n = libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                (&mut info as *mut libc::proc_taskinfo).cast(),
                size,
            );
            if n == size { info.pti_resident_size } else { 0 }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let pages = std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()))
            .unwrap_or(0);
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
        pages * page
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        0
    }
}

/// Number of threads in this process (macOS only; 0 elsewhere).
pub fn thread_count() -> u32 {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: as in resident_bytes.
        unsafe {
            let mut info: libc::proc_taskinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
            let n = libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                (&mut info as *mut libc::proc_taskinfo).cast(),
                size,
            );
            if n == size { info.pti_threadnum.max(0) as u32 } else { 0 }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        0
    }
}

fn process_start_time() -> Option<SystemTime> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: proc_pidinfo fills at most `size` bytes of the zeroed struct.
        unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let n = libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            );
            if n != size {
                return None;
            }
            Some(
                SystemTime::UNIX_EPOCH
                    + Duration::from_secs(info.pbi_start_tvsec)
                    + Duration::from_micros(info.pbi_start_tvusec),
            )
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_are_plausible() {
        mark_main_started();
        let elapsed = since_process_start();
        assert!(elapsed < Duration::from_secs(3600));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert!(resident_bytes() > 1024 * 1024);
        #[cfg(unix)]
        {
            let start = cpu_time();
            let mut x = 0u64;
            for i in 0..5_000_000u64 {
                x = x.wrapping_add(i * i);
            }
            std::hint::black_box(x);
            assert!(cpu_time() >= start);
        }
    }
}
