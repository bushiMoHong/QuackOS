//! Error type for capability operations.

/// Errors that can occur during capability operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    /// The given capability pointer (CPtr) is invalid.
    InvalidCPtr,
    /// The capability slot is empty.
    EmptySlot,
    /// The requested rights exceed those of the capability.
    RightsEscalation,
    /// The capability has been revoked.
    Revoked,
    /// The capability type does not match the requested operation.
    WrongCapType,
    /// The grant chain is broken (ancestor was revoked).
    GrantChainBroken,
    /// The CSpace is full (no free slots).
    CSpaceFull,
    /// The CNode is full.
    CNodeFull,
    /// The process ID does not reference a valid CSpace.
    InvalidProcess,
    /// The untyped capability is too small for the requested retype.
    UntypedTooSmall,
    /// The untyped capability has already been partially consumed.
    UntypedExhausted,
    /// Bad argument (null pointer, misaligned, etc.).
    InvalidArgument,
    /// The requested operation is not yet implemented.
    NotImplemented,
}
