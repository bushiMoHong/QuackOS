//! Linux signal handling syscalls (stubs).

/// rt_sigaction(signum, act, oldact, sigsetsize) — syscall 134
pub fn sys_rt_sigaction(_signum: usize, _act: usize, _oldact: usize, _sigsetsize: usize) -> u64 {
    0
}

/// rt_sigprocmask(how, set, oldset, sigsetsize) — syscall 135
pub fn sys_rt_sigprocmask(_how: usize, _set: usize, _oldset: usize, _sigsetsize: usize) -> u64 {
    0
}

/// sigaltstack(ss, oss) — syscall 132
pub fn sys_sigaltstack(_ss: usize, _oss: usize) -> u64 {
    0
}
