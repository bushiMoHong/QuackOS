//! BMM tests.

use crate::kernel::bmm::*;
use crate::kernel::tests::run_one;
use crate::kernel::trap::PageFaultCause;

pub fn run() -> (usize, usize) {
    let mut p = 0usize;
    let mut t = 0usize;

    if run_one("mapflags_empty",                    f_mapflags_empty) { p += 1; } t += 1;
    if run_one("mapflags_single_bit",               f_mapflags_single_bit) { p += 1; } t += 1;
    if run_one("mapflags_rw_combo",                 f_mapflags_rw_combo) { p += 1; } t += 1;
    if run_one("mapflags_rwx_combo",                f_mapflags_rwx_combo) { p += 1; } t += 1;
    if run_one("mapflags_rx_combo",                 f_mapflags_rx_combo) { p += 1; } t += 1;
    if run_one("mapflags_user_and_global",          f_mapflags_user_global) { p += 1; } t += 1;
    if run_one("mapflags_cow_and_shared",           f_mapflags_cow_shared) { p += 1; } t += 1;
    if run_one("mapflags_to_pte_flags",             f_mapflags_to_pte) { p += 1; } t += 1;
    if run_one("mapflags_equality",                 f_mapflags_eq) { p += 1; } t += 1;

    if run_one("address_space_id_equality",         f_asid_eq) { p += 1; } t += 1;
    if run_one("address_space_id_ord",              f_asid_ord) { p += 1; } t += 1;

    if run_one("maperror_variants_distinct",        f_maperror_distinct) { p += 1; } t += 1;
    if run_one("maperror_debug_copy",               f_maperror_copy) { p += 1; } t += 1;

    if run_one("fault_queue_initially_empty",       f_fq_empty) { p += 1; } t += 1;
    if run_one("fault_queue_push_and_pop",          f_fq_push_pop) { p += 1; } t += 1;
    if run_one("fault_queue_fifo_order",            f_fq_fifo) { p += 1; } t += 1;
    if run_one("fault_queue_overfill_false",        f_fq_overfill) { p += 1; } t += 1;
    if run_one("fault_queue_different_causes",      f_fq_causes) { p += 1; } t += 1;

    (p, t)
}

fn f_mapflags_empty() -> bool {
    let f = MapFlags::empty();
    !f.contains(MapFlags::READ) && !f.contains(MapFlags::WRITE) && !f.contains(MapFlags::EXEC)
}
fn f_mapflags_single_bit() -> bool {
    let f = MapFlags(MapFlags::READ);
    f.contains(MapFlags::READ) && !f.contains(MapFlags::WRITE)
}
fn f_mapflags_rw_combo() -> bool {
    let f = MapFlags(MapFlags::RW);
    f.contains(MapFlags::READ) && f.contains(MapFlags::WRITE) && !f.contains(MapFlags::EXEC)
}
fn f_mapflags_rwx_combo() -> bool {
    let f = MapFlags(MapFlags::RWX);
    f.contains(MapFlags::READ) && f.contains(MapFlags::WRITE) && f.contains(MapFlags::EXEC)
}
fn f_mapflags_rx_combo() -> bool {
    let f = MapFlags(MapFlags::RX);
    f.contains(MapFlags::READ) && !f.contains(MapFlags::WRITE) && f.contains(MapFlags::EXEC)
}
fn f_mapflags_user_global() -> bool {
    let f = MapFlags(MapFlags::USER | MapFlags::GLOBAL);
    f.contains(MapFlags::USER) && f.contains(MapFlags::GLOBAL) && !f.contains(MapFlags::READ)
}
fn f_mapflags_cow_shared() -> bool {
    let f = MapFlags(MapFlags::COW | MapFlags::SHARED);
    f.contains(MapFlags::COW) && f.contains(MapFlags::SHARED)
}
fn f_mapflags_to_pte() -> bool {
    let f = MapFlags(MapFlags::READ | MapFlags::WRITE);
    let pte = f.to_pte_flags();
    let bits = pte.bits;
    bits != 0 && bits & MapFlags::READ != 0 && bits & MapFlags::WRITE != 0
}
fn f_mapflags_eq() -> bool {
    MapFlags::empty() == MapFlags(0)
        && MapFlags(MapFlags::RW) == MapFlags(MapFlags::READ | MapFlags::WRITE)
        && MapFlags(MapFlags::READ) != MapFlags(MapFlags::WRITE)
}

fn f_asid_eq() -> bool { AddressSpaceId(42) == AddressSpaceId(42) && AddressSpaceId(42) != AddressSpaceId(99) }
fn f_asid_ord() -> bool { AddressSpaceId(10) < AddressSpaceId(20) }

fn f_maperror_distinct() -> bool {
    MapError::AlreadyMapped != MapError::NotMapped
        && MapError::AlreadyMapped != MapError::OutOfMemory
        && MapError::AlreadyMapped != MapError::InvalidArgument
}
fn f_maperror_copy() -> bool { let e = MapError::AlreadyMapped; e == e }

fn f_fq_empty() -> bool { pending_fault_count() == 0 && dequeue_fault().is_none() }

fn f_fq_push_pop() -> bool {
    let asid = AddressSpaceId(1);
    if !handle_page_fault(asid, 0x1000, PageFaultCause::Load) { return false; }
    if pending_fault_count() != 1 { return false; }
    match dequeue_fault() {
        Some(f) => f.addr_space_id == asid && f.fault_vaddr == 0x1000
                   && f.cause == PageFaultCause::Load && pending_fault_count() == 0,
        None => false,
    }
}

fn f_fq_fifo() -> bool {
    for i in 0..5 {
        if !handle_page_fault(AddressSpaceId(1), 0x1000 + i * 0x1000, PageFaultCause::Load) {
            return false;
        }
    }
    if pending_fault_count() != 5 { return false; }
    for i in 0..5 {
        match dequeue_fault() {
            Some(f) if f.fault_vaddr == 0x1000 + i * 0x1000 => {},
            _ => return false,
        }
    }
    pending_fault_count() == 0
}

fn f_fq_overfill() -> bool {
    for i in 0..64 {
        if !handle_page_fault(AddressSpaceId(1), i * 0x1000, PageFaultCause::Load) { return false; }
    }
    if pending_fault_count() != 64 { return false; }
    let ok = !handle_page_fault(AddressSpaceId(2), 0xDEAD, PageFaultCause::Store)
        && pending_fault_count() == 64;
    // Drain so subsequent tests start clean.
    for _ in 0..64 { dequeue_fault(); }
    ok && pending_fault_count() == 0
}

fn f_fq_causes() -> bool {
    handle_page_fault(AddressSpaceId(1), 0x1000, PageFaultCause::Load)
        && handle_page_fault(AddressSpaceId(1), 0x2000, PageFaultCause::Store)
        && handle_page_fault(AddressSpaceId(1), 0x3000, PageFaultCause::Exec)
        && dequeue_fault().unwrap().cause == PageFaultCause::Load
        && dequeue_fault().unwrap().cause == PageFaultCause::Store
        && dequeue_fault().unwrap().cause == PageFaultCause::Exec
}
