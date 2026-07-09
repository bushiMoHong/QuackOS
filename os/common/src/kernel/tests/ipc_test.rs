//! IPC tests.

use crate::kernel::ipc::channel::{
    BlockReason, ChannelId, ChannelInner, RecvMatch, SendMatch,
};
use crate::kernel::ipc::error::IpcError;
use crate::kernel::ipc::message::{MemoryMapPayload, Message, MessageType, ShortPayload};
use crate::kernel::ipc::synchronization::IpcState;
use crate::kernel::ipc::transfer::IpcBuffer;
use crate::kernel::sche::ThreadId;
use crate::kernel::tests::run_one;

pub fn run() -> (usize, usize) {
    let mut p = 0usize;
    let mut t = 0usize;

    if run_one("channel_id_equality",           f_chid) { p += 1; } t += 1;
    if run_one("block_reason_distinct",         f_block) { p += 1; } t += 1;

    if run_one("short_payload_from_slice",      f_sp_slice) { p += 1; } t += 1;
    if run_one("short_payload_max_eight",       f_sp_max) { p += 1; } t += 1;
    if run_one("short_payload_empty_none",      f_sp_empty) { p += 1; } t += 1;
    if run_one("short_payload_nine_none",       f_sp_nine) { p += 1; } t += 1;
    if run_one("short_payload_as_slice",        f_sp_as) { p += 1; } t += 1;

    if run_one("message_new_short",             f_msg_short) { p += 1; } t += 1;
    if run_one("message_new_mmap",              f_msg_mmap) { p += 1; } t += 1;
    if run_one("message_new_grant_cap",         f_msg_grant) { p += 1; } t += 1;

    if run_one("ipc_buffer_write_read",         f_buf_rw) { p += 1; } t += 1;
    if run_one("ipc_buffer_empty_read_none",    f_buf_empty) { p += 1; } t += 1;
    if run_one("ipc_buffer_eight_roundtrip",    f_buf_8) { p += 1; } t += 1;

    if run_one("ipc_error_distinct",            f_ipcerr) { p += 1; } t += 1;

    if run_one("ipc_state_ready_not_blocked",   f_state_ready) { p += 1; } t += 1;
    if run_one("ipc_state_all_blocked",         f_state_blocked) { p += 1; } t += 1;
    if run_one("ipc_state_equality",            f_state_eq) { p += 1; } t += 1;

    if run_one("channel_sender_parked",         f_ch_send_park) { p += 1; } t += 1;
    if run_one("channel_receiver_parked",       f_ch_recv_park) { p += 1; } t += 1;
    if run_one("channel_sender_then_receiver",  f_ch_send_recv) { p += 1; } t += 1;
    if run_one("channel_receiver_then_sender",  f_ch_recv_send) { p += 1; } t += 1;
    if run_one("channel_sender_fifo",           f_ch_send_fifo) { p += 1; } t += 1;
    if run_one("channel_receiver_fifo",         f_ch_recv_fifo) { p += 1; } t += 1;

    if run_one("message_type_distinct",         f_mt) { p += 1; } t += 1;

    (p, t)
}

fn f_chid() -> bool { ChannelId(1) == ChannelId(1) && ChannelId(1) != ChannelId(2) }
fn f_block() -> bool { BlockReason::Send != BlockReason::Receive && BlockReason::Send != BlockReason::Call }

fn f_sp_slice() -> bool {
    match ShortPayload::from_slice(&[1, 2, 3]) {
        Some(p) => p.len == 3 && p.as_slice() == &[1, 2, 3],
        None => false,
    }
}
fn f_sp_max() -> bool {
    ShortPayload::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]).map_or(false, |p| p.len == 8)
}
fn f_sp_empty() -> bool { ShortPayload::from_slice(&[]).is_none() }
fn f_sp_nine() -> bool { ShortPayload::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9]).is_none() }
fn f_sp_as() -> bool {
    match ShortPayload::from_slice(&[10, 20, 30]) {
        Some(p) => { let s = p.as_slice(); s.len() == 3 && s[0] == 10 && s[2] == 30 }
        None => false,
    }
}

fn f_msg_short() -> bool {
    match ShortPayload::from_slice(&[0xAA, 0xBB]) {
        Some(p) => {
            let msg = Message::new_short(7, p);
            msg.msg_type() == MessageType::ShortInfo && msg.header().sender == 7
        }
        None => false,
    }
}
fn f_msg_mmap() -> bool {
    let msg = Message::new_memory_map(1, MemoryMapPayload { paddr: 0x8000, size: 0x1000, flags: 0x3 });
    msg.msg_type() == MessageType::MemoryMap && msg.header().sender == 1
}
fn f_msg_grant() -> bool {
    let msg = Message::new_grant_cap(3, 42);
    msg.msg_type() == MessageType::GrantCapability && msg.header().sender == 3
}

fn f_buf_rw() -> bool {
    let mut buf = IpcBuffer::empty();
    let payload = match ShortPayload::from_slice(&[0x42, 0x99]) { Some(p) => p, None => return false };
    buf.write_short(5, &payload);
    if buf.sender != 5 { return false; }
    match buf.read_short() {
        Some(back) => back.len == 2 && back.words[0] == 0x42 && back.words[1] == 0x99,
        None => false,
    }
}
fn f_buf_empty() -> bool { IpcBuffer::empty().read_short().is_none() }
fn f_buf_8() -> bool {
    let mut buf = IpcBuffer::empty();
    let payload = match ShortPayload::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]) {
        Some(p) => p, None => return false,
    };
    buf.write_short(99, &payload);
    match buf.read_short() {
        Some(back) => back.len == 8 && back.words[0..8] == [1, 2, 3, 4, 5, 6, 7, 8],
        None => false,
    }
}

fn f_ipcerr() -> bool {
    IpcError::InvalidChannel != IpcError::ChannelClosed
        && IpcError::NoSendRight != IpcError::NoRecvRight
        && IpcError::NotImplemented != IpcError::ChannelTableFull
}

fn f_state_ready() -> bool { !IpcState::Ready.is_blocked() }
fn f_state_blocked() -> bool {
    IpcState::BlockedOnSend(ChannelId(1)).is_blocked()
        && IpcState::BlockedOnReceive(ChannelId(2)).is_blocked()
        && IpcState::BlockedOnCall(ChannelId(3)).is_blocked()
        && IpcState::BlockedOnNotify(ChannelId(4)).is_blocked()
}
fn f_state_eq() -> bool {
    IpcState::Ready == IpcState::Ready
        && IpcState::BlockedOnSend(ChannelId(1)) == IpcState::BlockedOnSend(ChannelId(1))
        && IpcState::BlockedOnSend(ChannelId(1)) != IpcState::BlockedOnReceive(ChannelId(1))
}

fn new_ch() -> ChannelInner { ChannelInner::new() }

fn f_ch_send_park() -> bool {
    let mut ch = new_ch();
    let msg = Message::new_short(1, ShortPayload::from_slice(&[1]).unwrap());
    matches!(ch.match_sender(ThreadId::new(10, 1), &msg), Ok(SendMatch::Parked))
}
fn f_ch_recv_park() -> bool {
    matches!(new_ch().match_receiver(ThreadId::new(20, 1)), Ok(RecvMatch::Parked))
}
fn f_ch_send_recv() -> bool {
    let mut ch = new_ch();
    let msg = Message::new_short(1, ShortPayload::from_slice(&[0x42]).unwrap());
    let stid = ThreadId::new(10, 1);
    if !matches!(ch.match_sender(stid, &msg), Ok(SendMatch::Parked)) { return false; }
    matches!(ch.match_receiver(ThreadId::new(20, 1)),
             Ok(RecvMatch::Matched(ref e)) if e.thread_id == stid && e.msg.is_some())
}
fn f_ch_recv_send() -> bool {
    let mut ch = new_ch();
    let rtid = ThreadId::new(20, 1);
    if !matches!(ch.match_receiver(rtid), Ok(RecvMatch::Parked)) { return false; }
    let msg = Message::new_short(1, ShortPayload::from_slice(&[0x99]).unwrap());
    matches!(ch.match_sender(ThreadId::new(10, 1), &msg), Ok(SendMatch::Matched(t)) if t == rtid)
}
fn f_ch_send_fifo() -> bool {
    let mut ch = new_ch();
    let msg = Message::new_short(0, ShortPayload::from_slice(&[0]).unwrap());
    let t1 = ThreadId::new(1, 1);
    let t2 = ThreadId::new(2, 1);
    let t3 = ThreadId::new(3, 1);
    ch.match_sender(t1, &msg).unwrap();
    ch.match_sender(t2, &msg).unwrap();
    ch.match_sender(t3, &msg).unwrap();
    matches!(ch.match_receiver(ThreadId::new(10, 1)), Ok(RecvMatch::Matched(ref e)) if e.thread_id == t1)
        && matches!(ch.match_receiver(ThreadId::new(11, 1)), Ok(RecvMatch::Matched(ref e)) if e.thread_id == t2)
        && matches!(ch.match_receiver(ThreadId::new(12, 1)), Ok(RecvMatch::Matched(ref e)) if e.thread_id == t3)
}
fn f_ch_recv_fifo() -> bool {
    let mut ch = new_ch();
    let t1 = ThreadId::new(10, 1);
    let t2 = ThreadId::new(11, 1);
    let t3 = ThreadId::new(12, 1);
    ch.match_receiver(t1).unwrap();
    ch.match_receiver(t2).unwrap();
    ch.match_receiver(t3).unwrap();
    let msg = Message::new_short(0, ShortPayload::from_slice(&[0]).unwrap());
    matches!(ch.match_sender(ThreadId::new(1, 1), &msg), Ok(SendMatch::Matched(t)) if t == t1)
        && matches!(ch.match_sender(ThreadId::new(2, 1), &msg), Ok(SendMatch::Matched(t)) if t == t2)
        && matches!(ch.match_sender(ThreadId::new(3, 1), &msg), Ok(SendMatch::Matched(t)) if t == t3)
}

fn f_mt() -> bool {
    MessageType::ShortInfo != MessageType::MemoryMap
        && MessageType::ShortInfo != MessageType::GrantCapability
        && MessageType::MemoryMap != MessageType::GrantCapability
}
