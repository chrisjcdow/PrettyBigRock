// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The kernel core's self-test (docs/design/0005-kernel-core.md, section 10). Run when the
//! command line contains `selftest`; the boot tests run it on every configuration.

use crate::sync::SpinLock;
use crate::{arch, mem, percpu, sched, time};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

const THREADS: u64 = 64;
const ROUNDS: u64 = 2000;

/// A small, fast pseudo-random number generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn check(ok: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what()) }
}

/// Runs every test. Returns a one-line summary, or what went wrong.
pub fn run() -> Result<String, String> {
    // If anything hangs, print what the scheduler is doing after 30 seconds.
    sched::WATCHDOG.store(time::now_ns() + 30_000_000_000, Ordering::Relaxed);
    heap()?;
    pages()?;
    breakpoint()?;
    let slept = sleep()?;
    let (switches, cores_used) = threads()?;
    sched::WATCHDOG.store(0, Ordering::Relaxed);
    Ok(format!(
        "heap, pages, exceptions, a 50 ms sleep ({} ms), and {THREADS} threads on {cores_used} cores ({switches} context switches)",
        slept / 1_000_000
    ))
}

fn heap() -> Result<(), String> {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let before = mem::heap::IN_USE.load(Ordering::Relaxed);
    let mut blocks: Vec<(Box<[u8]>, u8)> = Vec::new();
    for i in 0..3000u32 {
        let size = (rng.next() % 9000) as usize + 1;
        let fill = i as u8;
        blocks.push((alloc::vec![fill; size].into_boxed_slice(), fill));
        if rng.next().is_multiple_of(3) {
            let j = (rng.next() as usize) % blocks.len();
            let (b, f) = blocks.swap_remove(j);
            check(b.iter().all(|&x| x == f), || "heap: a block was overwritten".into())?;
        }
    }
    for (b, f) in &blocks {
        check(b.iter().all(|&x| x == *f), || "heap: a block was overwritten".into())?;
    }
    drop(blocks);
    let after = mem::heap::IN_USE.load(Ordering::Relaxed);
    check(after == before, || format!("heap: {} bytes not returned", after - before))
}

fn pages() -> Result<(), String> {
    let (free_before, _) = mem::pmm::stats();
    let mut got = Vec::new();
    for i in 0..200u64 {
        let n = 1 + i % 5;
        got.push((mem::pmm::alloc(n, 1, u64::MAX).ok_or("pages: ran out")?, n));
    }
    for (p, n) in got {
        mem::pmm::free(p, n);
    }
    let (free_after, _) = mem::pmm::stats();
    check(free_after == free_before, || format!("pages: {} pages lost", free_before - free_after))
}

fn breakpoint() -> Result<(), String> {
    let before = arch::breakpoints();
    arch::breakpoint();
    check(arch::breakpoints() == before + 1, || "exceptions: the breakpoint wasn't caught".into())
}

fn sleep() -> Result<u64, String> {
    let t0 = time::now_ns();
    sched::sleep_ns(50_000_000);
    let dt = time::now_ns() - t0;
    // Generous at the top: emulated machines can be slow to get round to a woken thread.
    check((49_000_000..1_000_000_000).contains(&dt), || {
        format!("time: a 50 ms sleep took {} ms", dt / 1_000_000)
    })?;
    Ok(dt)
}

fn threads() -> Result<(u64, u32), String> {
    let atomic = Arc::new(AtomicU64::new(0));
    let locked = Arc::new(SpinLock::new(0u64));
    let cores = Arc::new(AtomicU64::new(0));
    let switches_before: u64 =
        percpu::all().iter().map(|c| c.sched.switches.load(Ordering::Relaxed)).sum();
    let mut handles = Vec::new();
    for i in 0..THREADS {
        let (atomic, locked, cores) = (atomic.clone(), locked.clone(), cores.clone());
        handles.push(sched::spawn(&format!("worker {i}"), sched::NORMAL, move || {
            for r in 0..ROUNDS {
                atomic.fetch_add(1, Ordering::Relaxed);
                *locked.lock() += 1;
                cores.fetch_or(1 << percpu::this().index, Ordering::Relaxed);
                if r % 100 == 0 {
                    sched::yield_now();
                }
                if r % 500 == 250 {
                    sched::sleep_ns(1_000_000);
                }
            }
        }));
    }
    for h in &handles {
        sched::join(h);
    }
    let expect = THREADS * ROUNDS;
    let a = atomic.load(Ordering::Relaxed);
    check(a == expect, || format!("threads: the atomic counter reached {a}, not {expect}"))?;
    let l = *locked.lock();
    check(l == expect, || format!("threads: the locked counter reached {l}, not {expect}"))?;
    let used = cores.load(Ordering::Relaxed).count_ones();
    let online = percpu::all().iter().filter(|c| c.online.load(Ordering::Relaxed)).count() as u32;
    check(used == online, || format!("threads: only {used} of {online} cores did any work"))?;
    check(handles.iter().all(|h| h.has_exited()), || "threads: a thread didn't finish".into())?;
    let switches_after: u64 =
        percpu::all().iter().map(|c| c.sched.switches.load(Ordering::Relaxed)).sum();
    Ok((switches_after - switches_before, used))
}
