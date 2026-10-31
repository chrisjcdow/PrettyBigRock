// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The kernel-wide derivation tree, and copying, moving, closing and revoking capabilities
//! (docs/design/0006-objects-and-capabilities.md, section 5).
//!
//! Each node's value says where the capability is: a slot in a process's table, or a mapping
//! (the hidden node a mapping gets, so revoking a memory capability unmaps it everywhere).
//!
//! Locking: the tree's sleeping lock comes first, then a process's table lock or space lock
//! (never both at once). Looking a handle up needs only the table lock.

use super::process::Process;
use super::table::Entry;
use super::{Error, Object, require, rights};
use crate::mem::space::{Mapping, perm};
use crate::sync::{Mutex, MutexGuard};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use pbr_captree::{NodeId, Tree};

/// Where a capability is.
pub enum Owner {
    /// Being set up.
    Nowhere,
    /// In a process's table.
    Table {
        /// The process.
        process: Weak<Process>,
        /// Its handle there.
        handle: u32,
    },
    /// A mapping in a process's address space.
    Mapping {
        /// The process.
        process: Weak<Process>,
        /// Where the mapping starts.
        addr: u64,
    },
}

/// The tree.
pub type CapTree = Tree<Owner>;

static TREE: Mutex<CapTree> = Mutex::new(Tree::new());

/// Takes the tree's lock, for an operation spanning several capabilities (message delivery).
#[track_caller]
pub fn lock() -> MutexGuard<'static, CapTree> {
    TREE.lock()
}

/// What happens when a capability is put in `owner`'s table.
pub fn added(e: &Entry, owner: &Arc<Process>) {
    match &e.object {
        Object::Endpoint(ep) => ep.cap_added(e.rights),
        Object::Process(p) => p.cap_added(owner),
        _ => {}
    }
}

/// What happens when a capability leaves `owner`'s table: counts kept by objects, and threads
/// waiting through it, which must look again.
pub fn removed(e: &Entry, owner: &Arc<Process>) {
    match &e.object {
        Object::Endpoint(ep) => ep.cap_removed(e.rights),
        Object::Notification(n) => n.wake_waiters(),
        Object::Process(p) => p.cap_removed(owner),
        _ => {}
    }
}

/// The capability `h` names in `p`'s table.
pub fn lookup(p: &Process, h: u32) -> Result<Entry, Error> {
    p.table.lock().get(h).cloned()
}

/// Puts a capability for `object` in `p`'s table, as a new node under `parent` (or an original).
pub fn insert(
    tree: &mut CapTree,
    p: &Arc<Process>,
    object: Object,
    rights: u32,
    badge: u64,
    parent: Option<NodeId>,
) -> Result<u32, Error> {
    let node = match parent {
        Some(n) => tree.insert_child(n, Owner::Nowhere),
        None => tree.insert_root(Owner::Nowhere),
    }
    .map_err(|_| Error::OutOfMemory)?;
    let entry = Entry { object, rights, badge, node };
    let h = match p.table.lock().insert(entry.clone()) {
        Ok(h) => h,
        Err(e) => {
            let _ = tree.remove(node);
            return Err(e);
        }
    };
    if let Some(o) = tree.get_mut(node) {
        *o = Owner::Table { process: Arc::downgrade(p), handle: h };
    }
    added(&entry, p);
    Ok(h)
}

/// A new original capability for `object`, with every right its type has.
pub fn create(p: &Arc<Process>, object: Object) -> Result<u32, Error> {
    let r = object.all_rights();
    insert(&mut TREE.lock(), p, object, r, 0, None)
}

/// A copy of `h` with rights masked by `mask`, and with `badge` if one is given (only an
/// unbadged endpoint capability can be given one).
pub fn duplicate(p: &Arc<Process>, h: u32, mask: u32, badge: Option<u64>) -> Result<u32, Error> {
    let mut tree = TREE.lock();
    let e = lookup(p, h)?;
    require(e.rights, rights::DUPLICATE)?;
    let badge = match badge {
        Some(b) => {
            if !matches!(e.object, Object::Endpoint(_)) {
                return Err(Error::WrongType);
            }
            if e.badge != 0 {
                return Err(Error::BadState);
            }
            b
        }
        None => e.badge,
    };
    insert(&mut tree, p, e.object, e.rights & mask, badge, Some(e.node))
}

/// Closes `h`: its copies are given to its parent, so they keep working.
pub fn close(p: &Arc<Process>, h: u32) -> Result<(), Error> {
    let mut tree = TREE.lock();
    let e = p.table.lock().remove(h)?;
    let _ = tree.remove(e.node);
    drop(tree);
    removed(&e, p);
    Ok(())
}

/// Revokes `h`: destroys every capability derived from it, wherever it is, and keeps `h`.
pub fn revoke(p: &Arc<Process>, h: u32) -> Result<(), Error> {
    let mut tree = TREE.lock();
    let e = lookup(p, h)?;
    require(e.rights, rights::REVOKE)?;
    let mut dead = Vec::new();
    tree.revoke(e.node, |id, owner| dead.push((id, owner))).map_err(|_| Error::BadHandle)?;
    let mut gone_entries = Vec::new();
    let mut gone_maps: Vec<Mapping> = Vec::new();
    for (id, owner) in dead {
        match owner {
            Owner::Table { process, handle } => {
                let Some(q) = process.upgrade() else { continue };
                let x = {
                    let mut t = q.table.lock();
                    if t.holds(handle, id) { t.remove(handle).ok() } else { None }
                };
                if let Some(x) = x {
                    removed(&x, &q);
                    gone_entries.push(x);
                }
            }
            Owner::Mapping { process, addr } => {
                let Some(q) = process.upgrade() else { continue };
                let mut s = q.space.lock();
                if s.find(addr).is_some_and(|m| m.start == addr && m.node == Some(id))
                    && let Some(m) = s.remove(addr)
                {
                    gone_maps.push(m);
                }
            }
            Owner::Nowhere => {}
        }
    }
    drop(tree);
    // The objects (and the memory behind the mappings) go only once nothing points at them.
    drop(gone_entries);
    drop(gone_maps);
    Ok(())
}

/// Moves or copies `h` from `from`'s table into `to`'s, with its rights masked by `mask`.
/// Returns the new handle and its rights.
pub fn give(
    tree: &mut CapTree,
    from: &Arc<Process>,
    h: u32,
    to: &Arc<Process>,
    mask: u32,
    moving: bool,
) -> Result<(u32, u32), Error> {
    let e = lookup(from, h)?;
    require(e.rights, rights::TRANSFER)?;
    if !moving {
        require(e.rights, rights::DUPLICATE)?;
        let r = e.rights & mask;
        let nh = insert(tree, to, e.object, r, e.badge, Some(e.node))?;
        return Ok((nh, r));
    }
    // A move keeps the node, and so its place in the tree. Put it in its new place first, so
    // counts kept by the object never pass through zero.
    let mut moved = e.clone();
    moved.rights &= mask;
    let nh = to.table.lock().insert(moved.clone())?;
    if let Some(o) = tree.get_mut(e.node) {
        *o = Owner::Table { process: Arc::downgrade(to), handle: nh };
    }
    added(&moved, to);
    // Nothing else can change `from`'s table entry while the tree is locked.
    if let Ok(old) = from.table.lock().remove(h) {
        removed(&old, from);
    }
    Ok((nh, moved.rights))
}

/// Maps part of the memory object `mem` (a capability in `caller`'s table) into `target`.
/// Returns where. The mapping gets a hidden node under the capability, so revoking that
/// capability, or one it came from, unmaps it.
pub fn map(
    caller: &Arc<Process>,
    mem: u32,
    target: &Arc<Process>,
    offset: u64,
    len: u64,
    addr: u64,
    perms: u32,
) -> Result<u64, Error> {
    let mut tree = TREE.lock();
    let e = lookup(caller, mem)?;
    let Object::Memory(m) = &e.object else { return Err(Error::WrongType) };
    require(e.rights, rights::MAP)?;
    let mut max = 0;
    for (p, r) in
        [(perm::READ, rights::READ), (perm::WRITE, rights::WRITE), (perm::EXECUTE, rights::EXECUTE)]
    {
        if e.rights & r != 0 {
            max |= p;
        }
    }
    if perms & !max != 0 {
        return Err(Error::NoRight);
    }
    let end = offset.checked_add(len).ok_or(Error::InvalidArgument)?;
    if len == 0 || end > m.size() {
        return Err(Error::InvalidArgument);
    }
    let node = tree.insert_child(e.node, Owner::Nowhere).map_err(|_| Error::OutOfMemory)?;
    let result = {
        let mut space = target.space.lock();
        let start = if addr == 0 { space.choose(len) } else { Ok(addr) };
        start.and_then(|start| {
            space
                .map(Mapping { start, len, mem: m.clone(), offset, perms, max, node: Some(node) })
                .map(|()| start)
        })
    };
    match result {
        Ok(start) => {
            if let Some(o) = tree.get_mut(node) {
                *o = Owner::Mapping { process: Arc::downgrade(target), addr: start };
            }
            Ok(start)
        }
        Err(err) => {
            let _ = tree.remove(node);
            Err(err)
        }
    }
}

/// Maps a memory object the kernel holds directly (not through a capability) into `target`:
/// for the root program, before it has any capabilities. The mapping has no node.
pub fn map_kernel(
    target: &Process,
    mem: Arc<crate::mem::object::MemoryObject>,
    addr: u64,
    perms: u32,
) -> Result<u64, Error> {
    let len = mem.size();
    let mut space = target.space.lock();
    let start = if addr == 0 { space.choose(len)? } else { addr };
    space.map(Mapping { start, len, mem, offset: 0, perms, max: perms, node: None })?;
    Ok(start)
}

/// Removes the mappings exactly covering `[addr, addr + len)` from `target`.
pub fn unmap(target: &Arc<Process>, addr: u64, len: u64) -> Result<(), Error> {
    let mut tree = TREE.lock();
    let maps = target.space.lock().unmap(addr, len)?;
    for m in &maps {
        if let Some(n) = m.node {
            let _ = tree.remove(n);
        }
    }
    drop(tree);
    drop(maps);
    Ok(())
}

/// Closes every capability a process holds and removes every mapping: the end of a process.
pub fn teardown(p: &Arc<Process>) {
    let mut tree = TREE.lock();
    let entries = p.table.lock().drain();
    for (_, e) in &entries {
        let _ = tree.remove(e.node);
    }
    let maps = p.space.lock().clear();
    for m in &maps {
        if let Some(n) = m.node {
            let _ = tree.remove(n);
        }
    }
    drop(tree);
    for (_, e) in &entries {
        removed(e, p);
    }
}
