#![no_std]

//! # qlibc — QuackOS native C library replacement
//!
//! A safe Rust wrapper around the QuackOS native microkernel syscalls (SVC #1).
//! This crate is to QuackOS what `libc` is to Linux: the single point of
//! contact between user programs and the kernel.
//!
//! All `unsafe` is contained within the `syscall` module.  Every public API
//! exposed by this crate is safe to call from application code.

// ---------------------------------------------------------------------------
// Public modules
// ---------------------------------------------------------------------------

pub mod console;
pub mod dirent;
pub mod fs;
pub mod proc;

// Compiler runtime stubs (strlen, memcpy, memset, ...)
mod stubs;

// Internal module — not re-exported; used by sibling modules.
pub(crate) mod syscall;
