//! Linux futex syscall (stub).

/// futex(uaddr, futex_op, val, timeout, uaddr2, val3) — syscall 98
pub fn sys_futex(_uaddr: usize, futex_op: usize, _val: usize, _timeout: usize, _uaddr2: usize, _val3: usize) -> u64 {
    // Common futex ops:
    // FUTEX_WAIT       = 0
    // FUTEX_WAKE       = 1
    // FUTEX_WAIT_PRIVATE  = 128
    // FUTEX_WAKE_PRIVATE  = 129
    let op = futex_op & 0x7f;
    let _private = futex_op & 0x80 != 0;

    match op {
        0 => {
            // FUTEX_WAIT — for now just return 0 (non-blocking)
            // In real implementation this would block until woken
            0
        }
        1 => {
            // FUTEX_WAKE — return 0 (nothing woken)
            0
        }
        _ => (-crate::errno::ENOSYS as u64),
    }
}
