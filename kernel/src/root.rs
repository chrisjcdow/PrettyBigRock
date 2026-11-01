// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Starting the root program (docs/design/0009-programs-and-granite.md, sections 2 to 4): the
//! one program the kernel loads itself. Everything else is loaded by programs.

use crate::mem::object::MemoryObject;
use crate::mem::space::perm;
use crate::mem::{PAGE, pmm};
use crate::obj::budget::Budget;
use crate::obj::process::Process;
use crate::obj::{Error, Object, caps, device, rights};
use crate::{arch, sched, syscall, user};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use pbr_archive::Archive;
use pbr_elf::Program;

/// Roles in the start-up block (note 0009, section 4).
mod role {
    pub const PROCESS: u32 = 1;
    pub const THREAD: u32 = 2;
    pub const LOG: u32 = 3;
    pub const ARCHIVE: u32 = 4;
    pub const DEVICE_MEMORY: u32 = 7;
    pub const INTERRUPTS: u32 = 8;
    pub const MSI_POOL: u32 = 9;
    pub const IO_PORTS: u32 = 10;
    pub const MACHINE: u32 = 11;
}

/// The machine description page (note 0011, section 2).
fn machine_description() -> Vec<u8> {
    let m = crate::hw_machine();
    let (framebuffers, rsdp) = crate::boot_description();
    let framebuffers = &framebuffers[..framebuffers.len().min(8)];
    let msi = arch::irqchip::msi_available();
    let mut b = Vec::new();
    b.extend_from_slice(b"PBRMACH\0");
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&(u32::from(m.ps2) | u32::from(msi) << 1).to_le_bytes());
    b.extend_from_slice(&rsdp.to_le_bytes());
    b.extend_from_slice(&0u64.to_le_bytes()); // the device tree (not used before M10)
    b.extend_from_slice(&0u64.to_le_bytes());
    let (first, count) = arch::irqchip::wired_range();
    b.extend_from_slice(&(first + count).to_le_bytes());
    b.extend_from_slice(&(framebuffers.len() as u32).to_le_bytes());
    b.resize(64, 0);
    for f in framebuffers {
        // SAFETY: `Framebuffer` is a plain `repr(C)` record with no padding that matters; its
        // bytes are what the note says to copy.
        let raw = unsafe {
            core::slice::from_raw_parts(
                (f as *const pbr_bootinfo::Framebuffer).cast::<u8>(),
                core::mem::size_of::<pbr_bootinfo::Framebuffer>(),
            )
        };
        b.extend_from_slice(raw);
    }
    b
}

/// The memory kept back from every budget for the kernel itself: 1/64 of memory, at least
/// 4 MiB (note 0006, section 9).
fn reserve() -> u64 {
    let (_, total) = pmm::stats();
    (total / 64).max(1024) * PAGE
}

/// Loads `file` into `p` at a random address. Returns (entry point, stack size).
pub fn load(p: &Arc<Process>, file: &[u8]) -> Result<(u64, u64), String> {
    let prog = Program::parse(file, arch::ELF_MACHINE).map_err(|e| format!("{e}"))?;
    let (major, minor) = prog.needs_version;
    if u64::from(major) != syscall::VERSION.0 || u64::from(minor) > syscall::VERSION.1 {
        return Err(format!("it needs interface version {major}.{minor}"));
    }
    let (lo, hi) = prog.span();
    let base = p.space.lock().choose(hi - lo).map_err(|e| format!("{e:?}"))? - lo;
    for s in prog.segments() {
        let (start, end) = s.pages();
        let mem = MemoryObject::new(end - start, &p.budget).map_err(|e| format!("{e:?}"))?;
        let data = prog.segment_data(&s, base).map_err(|e| format!("{e}"))?;
        mem.write(s.vaddr - start, &data).map_err(|e| format!("{e:?}"))?;
        let mut perms = perm::READ;
        if s.writable() {
            perms |= perm::WRITE;
        }
        if s.executable() {
            perms |= perm::EXECUTE;
        }
        caps::map_kernel(p, mem, base + start, perms).map_err(|e| format!("{e:?}"))?;
    }
    Ok((base + prog.entry, prog.stack_size))
}

/// Starts the root program from the start-up archive, with `args` (the kernel's command line).
pub fn start(archive: Vec<u8>, args: &str) -> Result<Arc<Process>, String> {
    let a = Archive::parse(&archive).map_err(|e| format!("the start-up archive: {e}"))?;
    let file = a.find("root").ok_or("the start-up archive has no root program")?.data;
    let (free, _) = pmm::stats();
    let budget = Budget::root((free * PAGE).saturating_sub(reserve()));
    let p = Process::new("root", budget, (sched::PRIORITIES - 1) as u8)
        .map_err(|e| format!("can't make the root process: {e:?}"))?;
    let fail = |what: &str, e: Error| format!("can't start the root program: {what}: {e:?}");
    let (entry, stack_size) = load(&p, file).map_err(|e| format!("the root program: {e}"))?;

    // Its stack, with unmapped space below it (`choose` leaves a page either side).
    let stack = MemoryObject::new(stack_size, &p.budget).map_err(|e| fail("stack", e))?;
    let stack_at =
        caps::map_kernel(&p, stack, 0, perm::READ | perm::WRITE).map_err(|e| fail("stack", e))?;
    // The start-up block, and the start-up archive as a memory object.
    let block = MemoryObject::new(PAGE, &p.budget).map_err(|e| fail("start-up block", e))?;
    let block_at = caps::map_kernel(&p, block.clone(), 0, perm::READ)
        .map_err(|e| fail("start-up block", e))?;
    let arc = MemoryObject::new(a.len() as u64, &p.budget).map_err(|e| fail("archive", e))?;
    arc.write(0, &archive[..a.len()]).map_err(|e| fail("archive", e))?;
    drop(archive);

    let t = user::create_thread(&p, entry, stack_at + stack_size, block_at, sched::NORMAL)
        .map_err(|e| fail("thread", e))?;
    let description = machine_description();
    let desc =
        MemoryObject::new(description.len() as u64, &p.budget).map_err(|e| fail("machine", e))?;
    desc.write(0, &description).map_err(|e| fail("machine", e))?;
    let mut tree = caps::lock();
    let mut give = |object: Object, r: u32| caps::insert(&mut tree, &p, object, r, 0, None);
    let mut entries = alloc::vec![
        (role::PROCESS, give(Object::Process(p.clone()), Object::Process(p.clone()).all_rights())),
        (role::THREAD, give(Object::Thread(t.clone()), Object::Thread(t.clone()).all_rights())),
        (role::LOG, give(Object::Log, Object::Log.all_rights())),
        (role::ARCHIVE, give(Object::Memory(arc), rights::COMMON | rights::READ | rights::MAP)),
        (role::MACHINE, give(Object::Memory(desc), rights::COMMON | rights::READ | rights::MAP)),
    ];
    let dm = Object::DeviceMemory(device::DeviceMemory::everything());
    entries.push((role::DEVICE_MEMORY, give(dm.clone(), dm.all_rights())));
    if let Some(i) = device::Interrupts::everything() {
        let o = Object::Interrupts(i);
        entries.push((role::INTERRUPTS, give(o.clone(), o.all_rights())));
    }
    if let Some(i) = device::Interrupts::pool() {
        let o = Object::Interrupts(i);
        entries.push((role::MSI_POOL, give(o.clone(), o.all_rights())));
    }
    if let Some(i) = device::IoPorts::everything() {
        let o = Object::IoPorts(i);
        entries.push((role::IO_PORTS, give(o.clone(), o.all_rights())));
    }
    drop(tree);

    // The block: magic, version, count, where the arguments are, then (role, handle) pairs.
    let args = &args.as_bytes()[..args.len().min(3072)];
    let mut b = Vec::new();
    b.extend_from_slice(b"PBRS");
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let args_at = 16 + 8 * entries.len() as u32;
    b.extend_from_slice(&args_at.to_le_bytes());
    b.extend_from_slice(&(args.len() as u32).to_le_bytes());
    for (r, h) in entries {
        let h = h.map_err(|e| fail("capabilities", e))?;
        b.extend_from_slice(&r.to_le_bytes());
        b.extend_from_slice(&h.to_le_bytes());
    }
    b.extend_from_slice(args);
    block.write(0, &b).map_err(|e| fail("start-up block", e))?;

    p.start_thread(&t).map_err(|e| fail("thread", e))?;
    Ok(p)
}

/// The root program's name in reports.
pub fn describe(p: &Process) -> String {
    format!("\"{}\" (process {})", p.name, p.id)
}
