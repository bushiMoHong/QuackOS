//! Linux time-related syscall implementations.

/// clock_gettime(clk_id, tp) — syscall 113
///
/// Provides a monotonic clock starting from boot.
/// struct timespec { tv_sec: i64, tv_nsec: i64 } — 16 bytes
pub fn sys_clock_gettime(clk_id: usize, tp: usize) -> u64 {
    // CLOCK_REALTIME  = 0
    // CLOCK_MONOTONIC = 1
    // For now, return 0 (boot time) for both.
    if tp != 0 {
        unsafe {
            *((tp) as *mut u64) = 0;       // tv_sec
            *((tp + 8) as *mut u64) = 0;   // tv_nsec
        }
    }
    match clk_id {
        0 | 1 => 0,
        _ => (-crate::errno::EINVAL as u64),
    }
}

/// gettimeofday(tv, tz) — syscall 169
pub fn sys_gettimeofday(tv: usize, _tz: usize) -> u64 {
    if tv != 0 {
        unsafe {
            *((tv) as *mut u64) = 0;       // tv_sec
            *((tv + 8) as *mut u64) = 0;   // tv_usec
        }
    }
    0
}

/// times(buf) — syscall 153 (stub)
pub fn sys_times(_buf: usize) -> u64 {
    0 // return 0 (clock ticks)
}

/// nanosleep(req, rem) — syscall 101 (stub)
pub fn sys_nanosleep(_req: usize, _rem: usize) -> u64 {
    0 // immediately "done"
}
