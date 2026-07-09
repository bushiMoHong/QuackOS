//! Unified error type for the IPC subsystem.

/// Errors that can occur during IPC operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// The given channel ID does not reference a valid channel.
    InvalidChannel,
    /// The channel has been destroyed.
    ChannelClosed,
    /// The sender does not hold the required SEND capability.
    NoSendRight,
    /// The receiver does not hold the required RECV capability.
    NoRecvRight,
    /// The caller does not hold the required CALL capability.
    NoCallRight,
    /// The capability does not include GRANT permission.
    NoGrantRight,
    /// A capability chain check failed (reserved for future cap module).
    CapChainViolation,
    /// The message payload is too large for the target buffer.
    MessageTooLarge,
    /// The target thread is not in a valid state for IPC.
    InvalidThreadState,
    /// An address-space mapping operation failed.
    MapFailed,
    /// A page-table grant operation failed (source or destination error).
    GrantFailed,
    /// An unmap operation failed.
    UnmapFailed,
    /// Bad alignment on a virtual or physical address.
    BadAlignment,
    /// A null or otherwise invalid argument was supplied.
    InvalidArgument,
    /// No thread is waiting on the channel (for try_* operations).
    WouldBlock,
    /// The requested operation is not yet implemented.
    NotImplemented,
    /// The channel table is full (create_channel failed).
    ChannelTableFull,
}
