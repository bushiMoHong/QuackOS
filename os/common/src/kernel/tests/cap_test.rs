//! Capability management tests.

use crate::kernel::cap::*;
use crate::kernel::cap::cspace::{CPtr, CNode, CSpace, CSLOT_COUNT};
use crate::kernel::cap::derive::{mint_cap, derive_cap, revoke, check_grant_chain, DeriveError};
use crate::kernel::cap::error::CapError;
use crate::kernel::tests::run_one;

pub fn run() -> (usize, usize) {
    let mut p = 0usize;
    let mut t = 0usize;

    if run_one("captype_all_distinct",         f_captype) { p += 1; } t += 1;

    if run_one("caprights_empty",              f_rights_empty) { p += 1; } t += 1;
    if run_one("caprights_full",              f_rights_full) { p += 1; } t += 1;
    if run_one("caprights_call",              f_rights_call) { p += 1; } t += 1;
    if run_one("caprights_without",           f_rights_without) { p += 1; } t += 1;
    if run_one("caprights_without_all",       f_rights_without_all) { p += 1; } t += 1;
    if run_one("caprights_no_overlap",        f_rights_overlap) { p += 1; } t += 1;

    if run_one("capability_root_no_parent",    f_cap_root) { p += 1; } t += 1;
    if run_one("capability_derived",          f_cap_derived) { p += 1; } t += 1;
    if run_one("capability_has_rights",       f_cap_rights) { p += 1; } t += 1;
    if run_one("capability_equality",         f_cap_eq) { p += 1; } t += 1;

    if run_one("cptr_equality",               f_cptr) { p += 1; } t += 1;

    if run_one("cnode_empty",                 f_cn_empty) { p += 1; } t += 1;
    if run_one("cnode_insert_lookup",         f_cn_insert) { p += 1; } t += 1;
    if run_one("cnode_consecutive_slots",     f_cn_slots) { p += 1; } t += 1;
    if run_one("cnode_remove_clears",         f_cn_remove) { p += 1; } t += 1;
    if run_one("cnode_remove_empty_errors",   f_cn_remove_empty) { p += 1; } t += 1;
    if run_one("cnode_oob_errors",            f_cn_oob) { p += 1; } t += 1;
    if run_one("cnode_search",                f_cn_search) { p += 1; } t += 1;
    if run_one("cnode_full_errors",           f_cn_full) { p += 1; } t += 1;
    if run_one("cnode_reuse_slot",            f_cn_reuse) { p += 1; } t += 1;

    if run_one("cspace_insert_lookup_remove", f_cs_ops) { p += 1; } t += 1;

    if run_one("caperror_distinct",           f_ce_distinct) { p += 1; } t += 1;
    if run_one("derive_error_distinct",       f_de_distinct) { p += 1; } t += 1;

    if run_one("mint_creates_root",           f_mint) { p += 1; } t += 1;
    if run_one("derive_rights_subset",        f_derive) { p += 1; } t += 1;
    if run_one("derive_non_minted_fails",     f_derive_fake) { p += 1; } t += 1;
    if run_one("revoke_cascades",             f_revoke) { p += 1; } t += 1;
    if run_one("revoke_idempotent",           f_revoke2) { p += 1; } t += 1;

    (p, t)
}

fn f_captype() -> bool {
    let a = [CapType::Untyped, CapType::Endpoint, CapType::Thread,
             CapType::PageTable, CapType::Frame, CapType::Notification, CapType::CNode];
    for i in 0..a.len() { for j in 0..a.len() { if (i==j) != (a[i]==a[j]) { return false; } } }
    true
}

fn f_rights_empty() -> bool {
    let r = CapRights::empty();
    !r.contains(CapRights::READ) && !r.contains(CapRights::WRITE) && !r.contains(CapRights::SEND)
}
fn f_rights_full() -> bool {
    let r = CapRights::full();
    r.contains(CapRights::READ) && r.contains(CapRights::WRITE)
        && r.contains(CapRights::GRANT) && r.contains(CapRights::SEND) && r.contains(CapRights::RECV)
}
fn f_rights_call() -> bool {
    CapRights::CALL == CapRights::SEND | CapRights::RECV
        && CapRights(CapRights::SEND | CapRights::RECV).contains(CapRights::CALL)
}
fn f_rights_without() -> bool {
    let no_grant = CapRights::full().without(CapRights::GRANT);
    no_grant.contains(CapRights::READ) && no_grant.contains(CapRights::SEND)
        && !no_grant.contains(CapRights::GRANT)
}
fn f_rights_without_all() -> bool {
    CapRights(CapRights::SEND).without(CapRights::SEND) == CapRights::empty()
}
fn f_rights_overlap() -> bool {
    let b = [CapRights::READ, CapRights::WRITE, CapRights::GRANT, CapRights::SEND, CapRights::RECV];
    for i in 0..b.len() { for j in i+1..b.len() { if b[i] & b[j] != 0 { return false; } } }
    true
}

fn f_cap_root() -> bool {
    let c = Capability::new(42, CapType::Endpoint, CapRights::full());
    c.obj_id == 42 && c.cap_type == CapType::Endpoint && c.has_rights(CapRights::SEND) && c.parent_id == None
}
fn f_cap_derived() -> bool {
    let c = Capability::derived(7, CapType::Frame, CapRights(CapRights::READ), 3);
    c.obj_id == 7 && c.parent_id == Some(3)
}
fn f_cap_rights() -> bool {
    let c = Capability::new(1, CapType::Endpoint, CapRights(CapRights::SEND | CapRights::RECV));
    c.has_rights(CapRights::SEND) && c.has_rights(CapRights::RECV)
        && c.has_rights(CapRights::CALL) && !c.has_rights(CapRights::GRANT)
}
fn f_cap_eq() -> bool {
    Capability::new(1, CapType::Thread, CapRights::empty())
        == Capability::new(1, CapType::Thread, CapRights::empty())
        && Capability::new(1, CapType::Thread, CapRights::empty())
           != Capability::new(2, CapType::Thread, CapRights::empty())
}

fn f_cptr() -> bool { CPtr(0) == CPtr(0) && CPtr(0) != CPtr(5) }

fn f_cn_empty() -> bool { CNode::new().lookup(CPtr(0)) == Err(CapError::EmptySlot) }
fn f_cn_insert() -> bool {
    let mut cn = CNode::new();
    let cap = Capability::new(10, CapType::Frame, CapRights(CapRights::READ | CapRights::WRITE));
    let cptr = match cn.insert(cap) { Ok(p) => p, _ => return false };
    if cptr.0 != 0 { return false; }
    match cn.lookup(cptr) {
        Ok(f) => f.obj_id == 10 && f.cap_type == CapType::Frame && f.has_rights(CapRights::READ),
        _ => false,
    }
}
fn f_cn_slots() -> bool {
    let mut cn = CNode::new();
    let a = cn.insert(Capability::new(1, CapType::Endpoint, CapRights::full())).unwrap();
    let b = cn.insert(Capability::new(2, CapType::Thread, CapRights::empty())).unwrap();
    a.0 == 0 && b.0 == 1
}
fn f_cn_remove() -> bool {
    let mut cn = CNode::new();
    let cap = Capability::new(99, CapType::Notification, CapRights::full());
    let cptr = cn.insert(cap).unwrap();
    match cn.remove(cptr) {
        Ok(c) => c.obj_id == 99 && cn.lookup(cptr) == Err(CapError::EmptySlot),
        _ => false,
    }
}
fn f_cn_remove_empty() -> bool { CNode::new().remove(CPtr(0)) == Err(CapError::EmptySlot) }
fn f_cn_oob() -> bool { CNode::new().lookup(CPtr(CSLOT_COUNT)) == Err(CapError::InvalidCPtr) }
fn f_cn_search() -> bool {
    let mut cn = CNode::new();
    cn.insert(Capability::new(42, CapType::Endpoint, CapRights(CapRights::SEND))).unwrap();
    cn.search(42, CapType::Endpoint).is_some()
        && cn.search(42, CapType::Thread).is_none()
        && cn.search(7, CapType::Endpoint).is_none()
}
fn f_cn_full() -> bool {
    let mut cn = CNode::new();
    for i in 0..CSLOT_COUNT {
        if cn.insert(Capability::new(i, CapType::Frame, CapRights::empty())).is_err() { return false; }
    }
    cn.insert(Capability::new(999, CapType::Frame, CapRights::empty())) == Err(CapError::CNodeFull)
}
fn f_cn_reuse() -> bool {
    let mut cn = CNode::new();
    let a = cn.insert(Capability::new(1, CapType::Endpoint, CapRights::full())).unwrap();
    cn.insert(Capability::new(2, CapType::Thread, CapRights::empty())).unwrap();
    cn.remove(a).unwrap();
    cn.insert(Capability::new(3, CapType::Frame, CapRights::empty())).unwrap().0 == 0
}

fn f_cs_ops() -> bool {
    let mut cs = CSpace::new(1);
    let cap = Capability::new(5, CapType::Thread, CapRights::full());
    let cptr = match cs.insert(cap) { Ok(p) => p, _ => return false };
    match cs.lookup(cptr) { Ok(f) if f.obj_id == 5 => {}, _ => return false };
    match cs.remove(cptr) { Ok(r) => r.obj_id == 5 && cs.lookup(cptr) == Err(CapError::EmptySlot), _ => false }
}

fn f_ce_distinct() -> bool {
    CapError::InvalidCPtr != CapError::EmptySlot
        && CapError::CNodeFull != CapError::InvalidProcess
        && CapError::CSpaceFull != CapError::CNodeFull
}
fn f_de_distinct() -> bool {
    DeriveError::RightsEscalation != DeriveError::ParentRevoked
        && DeriveError::RightsEscalation != DeriveError::TableFull
}

fn f_mint() -> bool {
    match mint_cap(100, CapType::Endpoint, CapRights::full(), 1) {
        Ok(c) => c.obj_id == 100 && c.cap_type == CapType::Endpoint && c.parent_id.is_some(),
        _ => false,
    }
}
fn f_derive() -> bool {
    let parent = match mint_cap(1, CapType::Endpoint, CapRights(CapRights::SEND | CapRights::RECV), 1) {
        Ok(p) => p, _ => return false,
    };
    derive_cap(&parent, CapRights(CapRights::SEND), 2).is_ok()
        && derive_cap(&parent, CapRights::full(), 2) == Err(DeriveError::RightsEscalation)
}
fn f_derive_fake() -> bool {
    derive_cap(&Capability::new(1, CapType::Endpoint, CapRights::full()), CapRights(CapRights::SEND), 1)
        == Err(DeriveError::ParentRevoked)
}
fn f_revoke() -> bool {
    let root = match mint_cap(1, CapType::Endpoint, CapRights::full(), 1) { Ok(c) => c, _ => return false };
    let c1 = match derive_cap(&root, CapRights(CapRights::SEND), 2) { Ok(c) => c, _ => return false };
    let c2 = match derive_cap(&root, CapRights(CapRights::RECV), 3) { Ok(c) => c, _ => return false };
    let gc = match derive_cap(&c1, CapRights(CapRights::SEND), 4) { Ok(c) => c, _ => return false };
    if check_grant_chain(&c1).is_err() || check_grant_chain(&gc).is_err() { return false; }
    match revoke(&root) {
        Ok(n) => n >= 4
            && check_grant_chain(&c1) == Err(CapError::GrantChainBroken)
            && check_grant_chain(&c2) == Err(CapError::GrantChainBroken)
            && check_grant_chain(&gc) == Err(CapError::GrantChainBroken),
        _ => false,
    }
}
fn f_revoke2() -> bool {
    let root = match mint_cap(1, CapType::Endpoint, CapRights::full(), 1) { Ok(c) => c, _ => return false };
    match revoke(&root) { Ok(n) if n > 0 => revoke(&root) == Ok(0), _ => false }
}
