//! Process management types for the user-space Process Server.
//!
//! These types define the vocabulary for process lifecycle management,
//! IPC request/response encoding, signal delivery, and priority policy.
//!
//! # ProcessId — generational, ABA-proof
//!
//! `ProcessId` uses the same generational scheme as the kernel's `ThreadId`:
//!
//! ```text
//! bits 31:16  →  generation (u16, increments on every allocation at a slot)
//! bits 15:0   →  index      (u16, array index into ProcessTable)
//! ```
//!
//! `ProcessId(0)` (`NULL`) is reserved and never allocated.

use crate::kernel::bmm::AddressSpaceId;
use core::fmt;

// ---------------------------------------------------------------------------
// ProcessId — generational index (ABA-proof)
// ---------------------------------------------------------------------------

/// Process identifier with built-in ABA protection.
///
/// This is the user-space counterpart of the kernel's `ThreadId`.
/// It replaces the placeholder `type ProcessId = u32` in `kernel::ipc::message`
/// for all user-space process-management code.
///
/// # Layout
///
/// ```text
/// bits 31:16  →  generation (u16)
/// bits 15:0   →  index      (u16, array index into ProcessTable)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId(pub u32);

impl ProcessId {
    /// The null / invalid process ID.  Never allocated.
    pub const NULL: ProcessId = ProcessId(0);

    /// Maximum number of processes supported (limited by u16 index width).
    pub const MAX_INDEX: u16 = u16::MAX;

    /// Construct a `ProcessId` from an index and generation.
    #[inline]
    pub const fn new(index: u16, generation: u16) -> Self {
        ProcessId(((generation as u32) << 16) | (index as u32))
    }

    /// Extract the slot index.
    #[inline]
    pub fn index(self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }

    /// Extract the generation.
    #[inline]
    pub fn generation(self) -> u16 {
        ((self.0 >> 16) & 0xFFFF) as u16
    }

    /// Return `true` if this is the null ID.
    #[inline]
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

// Display for log messages.
impl fmt::Display for ProcessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            write!(f, "P(NULL)")
        } else {
            write!(f, "P({}:{})", self.index(), self.generation())
        }
    }
}

/// Convert to the kernel's placeholder `ProcessId` (u32) for IPC calls.
///
/// This bridge keeps the kernel IPC layer unchanged while allowing
/// user-space code to use the rich `ProcessId` type.
impl From<ProcessId> for u32 {
    #[inline]
    fn from(pid: ProcessId) -> u32 {
        pid.0
    }
}

/// Convert from a raw u32 (e.g. from kernel IPC) back to `ProcessId`.
impl From<u32> for ProcessId {
    #[inline]
    fn from(raw: u32) -> Self {
        ProcessId(raw)
    }
}

// ---------------------------------------------------------------------------
// ProcessState
// ---------------------------------------------------------------------------

/// Process lifecycle state.
///
/// This is the user-space (policy) view.  The kernel only knows about
/// thread states; process state is maintained by `ProcServer`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Process record allocated but not yet running (no threads created).
    Created,
    /// Process has at least one runnable thread.
    Running,
    /// All threads are blocked (waiting on IPC, timer, or notification).
    Blocked,
    /// Process received `SIGSTOP` or equivalent — threads are paused.
    Stopped,
    /// Process is in the process of exiting.
    Dying,
    /// Process has exited but parent has not yet reaped it.
    Zombie,
}

impl ProcessState {
    /// Return `true` if the process is alive (not dying or zombie).
    #[inline]
    pub fn is_alive(self) -> bool {
        matches!(self, ProcessState::Created | ProcessState::Running | ProcessState::Blocked | ProcessState::Stopped)
    }

    /// Return `true` if the process can receive signals.
    #[inline]
    pub fn can_receive_signal(self) -> bool {
        matches!(self, ProcessState::Running | ProcessState::Blocked | ProcessState::Stopped)
    }
}

// ---------------------------------------------------------------------------
// ProcessPriority — policy-level priority
// ---------------------------------------------------------------------------

/// Process-level priority — used by `ProcServer` to decide the base priority
/// of all threads belonging to this process.
///
/// # Convention
///
/// | Range   | Class      | Examples                          |
/// |---------|------------|-----------------------------------|
/// | 200–255 | System     | init, proc-server, mm-server      |
/// | 128–199 | Server     | fs-server, net-server             |
/// | 64–127  | User       | shell, user applications          |
/// | 0–63    | Background | idle tasks, batch jobs            |
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcessPriority(pub u8);

impl ProcessPriority {
    /// Default priority for new user processes.
    pub const DEFAULT: ProcessPriority = ProcessPriority(128);

    /// System-level services (PID 0–3, critical infrastructure).
    pub const SYSTEM: ProcessPriority = ProcessPriority(200);

    /// Other user-space servers.
    pub const SERVER: ProcessPriority = ProcessPriority(150);

    /// Interactive user processes.
    pub const USER: ProcessPriority = ProcessPriority(100);

    /// Background / batch processes.
    pub const BACKGROUND: ProcessPriority = ProcessPriority(50);

    /// Idle / lowest.
    pub const IDLE: ProcessPriority = ProcessPriority(0);

    /// Return `true` if the priority is in valid range.
    /// Always `true` for `u8`-backed priorities (0–255 maps directly).
    #[inline]
    pub fn is_valid(self) -> bool {
        true // u8 is always in [0, 255]
    }
}

// ---------------------------------------------------------------------------
// Signal — POSIX-style signals (simplified subset)
// ---------------------------------------------------------------------------

/// Signals that can be sent between processes.
///
/// This is a minimal subset of POSIX signals sufficient for microkernel
/// process management.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Signal {
    /// Terminal interrupt (Ctrl+C).
    SIGINT   = 2,
    /// Quit (Ctrl+\).
    SIGQUIT  = 3,
    /// Illegal instruction.
    SIGILL   = 4,
    /// Trace trap.
    SIGTRAP  = 5,
    /// Abort.
    SIGABRT  = 6,
    /// Bus error.
    SIGBUS   = 7,
    /// Floating-point exception.
    SIGFPE   = 8,
    /// Forceful kill (cannot be caught or ignored).
    SIGKILL  = 9,
    /// User-defined signal 1.
    SIGUSR1  = 10,
    /// Segmentation violation.
    SIGSEGV  = 11,
    /// User-defined signal 2.
    SIGUSR2  = 12,
    /// Broken pipe.
    SIGPIPE  = 13,
    /// Alarm clock.
    SIGALRM  = 14,
    /// Graceful termination request.
    SIGTERM  = 15,
    /// Child process state change (sent to parent).
    SIGCHLD  = 17,
    /// Continue if stopped.
    SIGCONT  = 18,
    /// Stop (cannot be caught or ignored).
    SIGSTOP  = 19,
}

impl Signal {
    /// Return `true` if this signal can be caught by a handler.
    #[inline]
    pub fn is_catchable(self) -> bool {
        !matches!(self, Signal::SIGKILL | Signal::SIGSTOP)
    }

    /// Default action for this signal.
    #[inline]
    pub fn default_action(self) -> SignalAction {
        match self {
            Signal::SIGINT
            | Signal::SIGQUIT
            | Signal::SIGILL
            | Signal::SIGTRAP
            | Signal::SIGABRT
            | Signal::SIGBUS
            | Signal::SIGFPE
            | Signal::SIGKILL
            | Signal::SIGUSR1
            | Signal::SIGSEGV
            | Signal::SIGUSR2
            | Signal::SIGPIPE
            | Signal::SIGALRM
            | Signal::SIGTERM => SignalAction::Terminate,

            Signal::SIGSTOP => SignalAction::Stop,
            Signal::SIGCONT => SignalAction::Continue,
            Signal::SIGCHLD => SignalAction::Ignore,
        }
    }
}

/// Default signal disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAction {
    /// Terminate the process.
    Terminate,
    /// Stop (suspend) the process.
    Stop,
    /// Continue a stopped process.
    Continue,
    /// Ignore the signal.
    Ignore,
}

// ---------------------------------------------------------------------------
// ProcRequest — IPC message payload from clients to ProcServer
// ---------------------------------------------------------------------------

/// A request that a client sends to the Process Server via IPC.
///
/// Encoded in `ShortPayload.words[]` with an opcode in `words[0]`.
#[derive(Debug, Clone)]
pub enum ProcRequest {
    /// Spawn a new child process.
    Spawn {
        /// Parent process.
        parent: ProcessId,
        /// Human-readable name (up to 32 bytes, not null-terminated).
        name: [u8; 32],
        /// Name length in bytes.
        name_len: u8,
        /// Code segment: [start, end).
        code_start: usize,
        code_end: usize,
        /// Data segment: [start, end).
        data_start: usize,
        data_end: usize,
        /// Stack segment: [start, end) — grows downward.
        stack_start: usize,
        stack_end: usize,
        /// Heap start address.
        heap_start: usize,
    },

    /// Process exit.
    Exit {
        pid: ProcessId,
        exit_code: i32,
    },

    /// Send a signal to a process.
    Signal {
        target: ProcessId,
        signal: Signal,
    },

    /// Set a process's priority.
    SetPriority {
        target: ProcessId,
        priority: ProcessPriority,
    },

    /// Query process information.
    Query {
        target: ProcessId,
    },

    /// Register an already-existing process (boot-time initialisation).
    Register {
        pid: ProcessId,
        addr_space_id: AddressSpaceId,
        name: [u8; 32],
        name_len: u8,
        priority: ProcessPriority,
        parent: ProcessId,
    },

    /// List all processes (returns count + list of PIDs).
    List,
}

// ---------------------------------------------------------------------------
// ProcRequestOp — opcodes for IPC encoding
// ---------------------------------------------------------------------------

/// Opcode stored in `ShortPayload.words[0]` identifying the request variant.
///
/// Follows the same pattern as `MmRequestOp` in `usr::mm::types`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcRequestOp {
    Spawn       = 1,
    Exit        = 2,
    Signal      = 3,
    SetPriority = 4,
    Query       = 5,
    Register    = 6,
    List        = 7,
}

/// Reply opcodes for ProcServer → client responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProcReplyOp {
    Ok          = 0,
    Error       = 1,
    Spawned     = 2,   // carries new ProcessId
    QueryResult = 3,   // carries ProcessInfo in payload
    ListResult  = 4,   // carries count + PID list
}

// ---------------------------------------------------------------------------
// ProcError
// ---------------------------------------------------------------------------

/// Errors returned by the Process Server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcError {
    /// The given `ProcessId` does not reference a valid process.
    InvalidProcess,
    /// The process table is full.
    ProcessTableFull,
    /// Not enough threads available for the new process.
    ThreadTableFull,
    /// Physical memory exhausted during spawn.
    NoMemory,
    /// The caller lacks permission for the requested operation.
    PermissionDenied,
    /// The signal number is invalid or unsupported.
    InvalidSignal,
    /// The process is in an unexpected state for the operation.
    InvalidState,
    /// A bad argument was supplied (null pointer, misaligned, …).
    InvalidArgument,
    /// The requested operation is not yet implemented.
    NotImplemented,
    /// The name is too long or contains invalid bytes.
    InvalidName,
}

/// Convenience alias.
pub type ProcResult<T> = Result<T, ProcError>;
