//! Linux syscall interface — all syscall dispatch logic lives here.

use crate::errno;
use crate::task::TaskStruct;

pub fn dispatch(
    task: &mut TaskStruct,
    nr: usize,
    a0: usize, a1: usize, a2: usize,
    a3: usize, a4: usize, a5: usize,
) -> u64 {
    match nr {
        17  => crate::misc::sys_getcwd(a0, a1),
        29  => crate::fs::sys_ioctl(a0, a1, a2),
        56  => crate::fs::sys_openat(task, a0, a1, a2, a3),
        57  => crate::fs::sys_close(task, a0),
        63  => crate::fs::sys_read(task, a0, a1, a2),
        64  => crate::fs::sys_write(task, a0, a1, a2),
        66  => crate::fs::sys_writev(task, a0, a1, a2),
        80  => crate::fs::sys_fstat(task, a0, a1),
        93  => crate::proc::sys_exit(task, a0),
        94  => crate::proc::sys_exit_group(task, a0),
        96  => crate::proc::sys_set_tid_address(task, a0),
        160 => crate::misc::sys_uname(a0),
        172 => crate::proc::sys_getpid(task),
        174 => crate::proc::sys_getuid(task),
        175 => crate::proc::sys_geteuid(task),
        176 => crate::proc::sys_getgid(task),
        177 => crate::proc::sys_getegid(task),
        214 => crate::mm::sys_brk(task, a0),
        215 => crate::mm::sys_munmap(task, a0, a1),
        222 => crate::mm::sys_mmap(task, a0, a1, a2, a3, a4, a5),
        278 => crate::misc::sys_getrandom(a0, a1, a2),
        _   => u64::from_le_bytes((-(errno::ENOSYS as i64)).to_le_bytes()),
    }
}
