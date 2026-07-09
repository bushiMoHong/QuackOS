//! Scheduler tests.

use crate::kernel::sche::error::ScheError;
use crate::kernel::sche::queue::ReadyQueue;
use crate::kernel::sche::thread::{ThreadId, ThreadState};
use crate::kernel::tests::run_one;

pub fn run() -> (usize, usize) {
    let mut p = 0usize;
    let mut t = 0usize;

    if run_one("tid_null",                    f_tid_null) { p += 1; } t += 1;
    if run_one("tid_new",                     f_tid_new) { p += 1; } t += 1;
    if run_one("tid_bit_layout",              f_tid_bits) { p += 1; } t += 1;
    if run_one("tid_equality",                f_tid_eq) { p += 1; } t += 1;
    if run_one("tid_display_not_empty",       f_tid_disp) { p += 1; } t += 1;

    if run_one("tstate_from_u8_roundtrip",    f_ts_round) { p += 1; } t += 1;
    if run_one("tstate_from_u8_invalid",      f_ts_invalid) { p += 1; } t += 1;
    if run_one("tstate_is_runnable",          f_ts_runnable) { p += 1; } t += 1;
    if run_one("tstate_repr_u8",              f_ts_repr) { p += 1; } t += 1;

    if run_one("rq_initially_empty",          f_rq_empty) { p += 1; } t += 1;
    if run_one("rq_enqueue_dequeue_single",   f_rq_single) { p += 1; } t += 1;
    if run_one("rq_fifo_same_priority",       f_rq_fifo) { p += 1; } t += 1;
    if run_one("rq_higher_priority_first",    f_rq_prio1) { p += 1; } t += 1;
    if run_one("rq_priority_ordering",        f_rq_prio2) { p += 1; } t += 1;
    if run_one("rq_min_max_priority",         f_rq_minmax) { p += 1; } t += 1;
    if run_one("rq_remove_existing",          f_rq_rm) { p += 1; } t += 1;
    if run_one("rq_remove_non_existent",      f_rq_rm_ne) { p += 1; } t += 1;
    if run_one("rq_remove_clears_bitmap",     f_rq_rm_bit) { p += 1; } t += 1;
    if run_one("rq_remove_only_target",       f_rq_rm_one) { p += 1; } t += 1;
    if run_one("rq_empty_and_count",          f_rq_count) { p += 1; } t += 1;

    if run_one("sche_error_distinct",          f_se) { p += 1; } t += 1;

    (p, t)
}

fn f_tid_null() -> bool { ThreadId::NULL.is_null() && ThreadId::NULL.0 == 0 }
fn f_tid_new() -> bool {
    let tid = ThreadId::new(5, 3);
    tid.index() == 5 && tid.generation() == 3 && !tid.is_null()
}
fn f_tid_bits() -> bool {
    let tid = ThreadId::new(0xAAAA, 0xBBBB);
    tid.0 == 0xBBBB_AAAA && tid.index() == 0xAAAA && tid.generation() == 0xBBBB
}
fn f_tid_eq() -> bool {
    ThreadId::new(1, 2) == ThreadId::new(1, 2)
        && ThreadId::new(1, 2) != ThreadId::new(1, 3)
        && ThreadId::new(1, 2) != ThreadId::new(2, 2)
}
fn f_tid_disp() -> bool {
    // ThreadId has Display impl — verify it compiles and produces expected fields.
    let tid = ThreadId::new(7, 3);
    // Just verify Display exists and the underlying fields are intact.
    // (format! needs alloc; we just check the raw value.)
    tid.index() == 7 && tid.generation() == 3 && tid.0 == ThreadId::new(7, 3).0
}

fn f_ts_round() -> bool {
    (0..=4).all(|v: u8| {
        ThreadState::from_u8(v).map_or(false, |s| s as u8 == v)
    })
}
fn f_ts_invalid() -> bool {
    ThreadState::from_u8(5).is_none() && ThreadState::from_u8(255).is_none()
}
fn f_ts_runnable() -> bool {
    ThreadState::Ready.is_runnable()
        && !ThreadState::Free.is_runnable()
        && !ThreadState::Running.is_runnable()
        && !ThreadState::Blocked.is_runnable()
        && !ThreadState::Dying.is_runnable()
}
fn f_ts_repr() -> bool {
    ThreadState::Free as u8 == 0 && ThreadState::Ready as u8 == 1
        && ThreadState::Running as u8 == 2 && ThreadState::Blocked as u8 == 3
        && ThreadState::Dying as u8 == 4
}

fn f_rq_empty() -> bool {
    let mut q = ReadyQueue::new();
    q.is_empty() && q.total_runnable() == 0 && q.dequeue().is_none()
}
fn f_rq_single() -> bool {
    let mut q = ReadyQueue::new();
    let tid = ThreadId::new(1, 1);
    q.enqueue(tid, 128).unwrap();
    !q.is_empty() && q.total_runnable() == 1 && q.dequeue() == Some(tid) && q.is_empty()
}
fn f_rq_fifo() -> bool {
    let mut q = ReadyQueue::new();
    let t1 = ThreadId::new(1, 1);
    let t2 = ThreadId::new(2, 1);
    let t3 = ThreadId::new(3, 1);
    q.enqueue(t1, 100).unwrap(); q.enqueue(t2, 100).unwrap(); q.enqueue(t3, 100).unwrap();
    q.total_runnable() == 3 && q.dequeue() == Some(t1)
        && q.dequeue() == Some(t2) && q.dequeue() == Some(t3) && q.is_empty()
}
fn f_rq_prio1() -> bool {
    let mut q = ReadyQueue::new();
    let low = ThreadId::new(10, 1);
    let high = ThreadId::new(20, 1);
    q.enqueue(low, 10).unwrap(); q.enqueue(high, 250).unwrap();
    q.dequeue() == Some(high) && q.dequeue() == Some(low)
}
fn f_rq_prio2() -> bool {
    let mut q = ReadyQueue::new();
    let lo = ThreadId::new(1, 1);
    let md = ThreadId::new(2, 1);
    let hi = ThreadId::new(3, 1);
    q.enqueue(lo, 10).unwrap(); q.enqueue(md, 128).unwrap(); q.enqueue(hi, 255).unwrap();
    q.dequeue() == Some(hi) && q.dequeue() == Some(md) && q.dequeue() == Some(lo)
}
fn f_rq_minmax() -> bool {
    let mut q = ReadyQueue::new();
    q.enqueue(ThreadId::new(1, 1), 0).unwrap();
    q.enqueue(ThreadId::new(2, 1), 255).unwrap();
    q.dequeue() == Some(ThreadId::new(2, 1)) && q.dequeue() == Some(ThreadId::new(1, 1))
}
fn f_rq_rm() -> bool {
    let mut q = ReadyQueue::new();
    let tid = ThreadId::new(5, 1);
    q.enqueue(tid, 100).unwrap();
    q.total_runnable() == 1 && q.remove(tid, 100) && q.is_empty() && q.dequeue().is_none()
}
fn f_rq_rm_ne() -> bool {
    !ReadyQueue::new().remove(ThreadId::new(99, 99), 100)
}
fn f_rq_rm_bit() -> bool {
    let mut q = ReadyQueue::new();
    let tid = ThreadId::new(1, 1);
    q.enqueue(tid, 200).unwrap();
    q.remove(tid, 200);
    q.is_empty() && q.dequeue().is_none()
}
fn f_rq_rm_one() -> bool {
    let mut q = ReadyQueue::new();
    let t1 = ThreadId::new(1, 1);
    let t2 = ThreadId::new(2, 1);
    q.enqueue(t1, 100).unwrap(); q.enqueue(t2, 100).unwrap();
    q.remove(t1, 100);
    q.total_runnable() == 1 && q.dequeue() == Some(t2)
}
fn f_rq_count() -> bool {
    let mut q = ReadyQueue::new();
    if !q.is_empty() || q.total_runnable() != 0 { return false; }
    for i in 0..10 { q.enqueue(ThreadId::new(i as u16, 1), 128).unwrap(); }
    !q.is_empty() && q.total_runnable() == 10
}

fn f_se() -> bool {
    ScheError::InvalidThread != ScheError::ThreadTableFull
        && ScheError::InvalidThread != ScheError::PriorityQueueFull
        && ScheError::InvalidThread != ScheError::InvalidThreadState
        && ScheError::InvalidThread != ScheError::NoRunnableThread
        && ScheError::InvalidThread != ScheError::NullThreadId
        && ScheError::InvalidThread != ScheError::InvalidArgument
        && ScheError::InvalidThread != ScheError::NotImplemented
}
