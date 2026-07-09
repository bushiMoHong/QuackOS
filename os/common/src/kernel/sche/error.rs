//! Unified error type for the scheduler subsystem.

/// Errors that can occur during scheduler operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheError {
    /// The given `ThreadId` does not reference a valid thread (generation
    /// mismatch or index out of bounds).
    InvalidThread,
    /// The thread table is full — no free slot available for a new thread.
    ThreadTableFull,
    /// An individual priority level's ready-queue is full.
    PriorityQueueFull,
    /// The thread is in an unexpected state for the requested operation
    /// (e.g. trying to block a thread that is already blocked).
    InvalidThreadState,
    /// No runnable thread is available (idle condition — all threads blocked).
    NoRunnableThread,
    /// The operation tried to use `ThreadId::NULL`.
    NullThreadId,
    /// Bad argument (null pointer, misaligned address, etc.).
    InvalidArgument,
    /// The requested operation is not yet implemented.
    NotImplemented,
}
