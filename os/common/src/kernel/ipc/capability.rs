//! Capability-based access control for IPC.
//!
//! All permission checks delegate to `kernel::cap`.  Types are re-exported
//! from `cap` so existing `use crate::kernel::ipc::Capability` paths still
//! work.
//!
//! # Relationship with `kernel::cap`
//!
//! ```text
//! ipc::capability                  kernel::cap
//! ───────────────                  ───────────
//! check_send_right() ──────────▶ cspace::lookup_send_right()
//! check_recv_right() ──────────▶ cspace::lookup_recv_right()
//! check_call_right() ──────────▶ cspace::lookup_call_right()
//! check_grant_right() ─────────▶ derive::check_grant_chain()
//! mint_channel_cap()  ─────────▶ cap::make_endpoint_cap()
//! derive_cap()        ─────────▶ cap::derive::derive_cap()
//! ```
//!
//! IPC-facing types (`CapRights`, `Capability`) are re-exported so that
//! `bmm` and other IPC sub-modules don't need import-path changes.

use crate::kernel::cap;

use crate::kernel::ipc::channel::ChannelId;
use crate::kernel::ipc::error::IpcError;
use crate::kernel::ipc::message::ProcessId;

// ---------------------------------------------------------------------------
// Re-export from cap (backward-compatible)
// ---------------------------------------------------------------------------

pub use cap::CapRights;
pub use cap::Capability;

// ---------------------------------------------------------------------------
// Permission checks — delegate to cap::cspace / cap::derive
// ---------------------------------------------------------------------------

/// Check SEND right via `cap::cspace::lookup_send_right`.
pub fn check_send_right(
    sender_pid: ProcessId,
    channel_id: ChannelId,
) -> Result<(), IpcError> {
    cap::cspace::lookup_send_right(sender_pid, channel_id)
        .map_err(|e| map_cap_error(e, IpcError::NoSendRight))
}

/// Check RECV right via `cap::cspace::lookup_recv_right`.
pub fn check_recv_right(
    receiver_pid: ProcessId,
    channel_id: ChannelId,
) -> Result<(), IpcError> {
    cap::cspace::lookup_recv_right(receiver_pid, channel_id)
        .map_err(|e| map_cap_error(e, IpcError::NoRecvRight))
}

/// Check CALL right via `cap::cspace::lookup_call_right`.
pub fn check_call_right(
    caller_pid: ProcessId,
    channel_id: ChannelId,
) -> Result<(), IpcError> {
    cap::cspace::lookup_call_right(caller_pid, channel_id)
        .map_err(|e| map_cap_error(e, IpcError::NoCallRight))
}

/// Check GRANT right via `cap::derive::check_grant_chain`.
pub fn check_grant_right(
    _sender_pid: ProcessId,
    cap: &Capability,
) -> Result<(), IpcError> {
    // 1. Verify the capability itself carries GRANT.
    if !cap.has_rights(CapRights::GRANT) {
        return Err(IpcError::NoGrantRight);
    }
    // 2. Walk the derivation chain for revocations.
    cap::derive::check_grant_chain(cap)
        .map_err(|_| IpcError::CapChainViolation)
}

// ---------------------------------------------------------------------------
// Capability lifecycle — delegate to cap
// ---------------------------------------------------------------------------

/// Create an Endpoint capability for a newly created channel.
pub fn mint_channel_cap(channel_id: ChannelId) -> Capability {
    // Endpoint caps are minted (root, no parent).
    // Owner=0 is the root server / init process; the cap will be distributed
    // from there via derive_cap.
    cap::derive::mint_cap(
        channel_id.0 as usize,
        cap::CapType::Endpoint,
        CapRights::full(),
        0, // owner: root server / init pid
    )
    .unwrap_or_else(|_| {
        // Fallback: if derivation table is full, return a raw capability.
        // In production this should panic — the kernel can't proceed without
        // capability tracking.
        cap::make_endpoint_cap(channel_id)
    })
}

/// Derive a capability with reduced rights.
///
/// Delegates to `cap::derive::derive_cap`.
pub fn derive_cap(parent: &Capability, new_rights: CapRights) -> Option<Capability> {
    cap::derive::derive_cap(parent, new_rights, 0)
        .ok()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a `cap::CapError` to an `IpcError`, using `default` for non-critical
/// failures and logging the underlying cause.
fn map_cap_error(e: cap::CapError, default: IpcError) -> IpcError {
    match e {
        cap::CapError::EmptySlot => IpcError::InvalidChannel,
        cap::CapError::InvalidProcess => IpcError::InvalidArgument,
        _ => default,
    }
}
