//! User-space servers and subsystems.
//!
//! Each sub-module corresponds to a user-space service that communicates with
//! the kernel via IPC:
//!
//! | Module     | Purpose                              |
//! |------------|--------------------------------------|
//! | `mm`       | Memory Manager — page-fault handler, physical allocator, VMA |
//! | `proc`     | Process Manager — spawn, exit, signals |
//! | `fs`       | File System — VFS layer, disk drivers |
//! | `net`      | Network Stack — TCP/IP, socket layer  |
//! | `drivers`  | Device Drivers — hardware access      |
//! | `init`     | Init system — boot-time service launch |
//! | `task`     | Task / thread management helpers      |

pub mod fs;
pub mod mm;
pub mod task;
pub mod proc;
pub mod drivers;
pub mod init;
