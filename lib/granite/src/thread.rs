// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Threads. Each thread's thread pointer (`FS` base on x86-64, `TPIDR_EL0` on AArch64) points
//! at a small block of `granite`'s own, holding where its message page is.

use crate::handle::Handle;
use crate::notification::Notification;
use crate::sys::{self, Result, num};
#[cfg(target_os = "prettybigrock")]
use alloc::boxed::Box;

/// A thread capability.
#[derive(Debug)]
pub struct Thread(pub Handle);

impl Thread {
    /// Starts it.
    pub fn start(&self) -> Result<()> {
        sys::call(num::THREAD_START, [self.0.raw().into(), 0, 0, 0, 0, 0]).map(|_| ())
    }

    /// Asks it to stop.
    pub fn stop(&self) -> Result<()> {
        sys::call(num::THREAD_STOP, [self.0.raw().into(), 0, 0, 0, 0, 0]).map(|_| ())
    }

    /// Asks for `bits` on `n` when it exits.
    pub fn watch(&self, n: &Notification, bits: u64) -> Result<()> {
        sys::call(num::THREAD_WATCH, [self.0.raw().into(), n.0.raw().into(), bits, 0, 0, 0])
            .map(|_| ())
    }
}

/// Gives the processor to another thread, if one is ready.
pub fn yield_now() {
    let _ = sys::call(num::THREAD_YIELD, [0; 6]);
}

/// Ends the calling thread.
pub fn exit() -> ! {
    sys::thread_exit()
}

/// Each thread's own block, which its thread pointer points at.
#[cfg(target_os = "prettybigrock")]
#[repr(C)]
pub(crate) struct Tcb {
    this: *const Tcb,
    msg: *mut u8,
}

/// The calling thread's message page.
#[cfg(target_os = "prettybigrock")]
pub(crate) fn message_page() -> *mut u8 {
    let t: *const Tcb;
    // SAFETY: the thread pointer was set by `install` before any message is sent, to a block
    // that lives as long as the thread.
    unsafe {
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!("mov {}, fs:[0]", out(reg) t, options(nostack, readonly, preserves_flags));
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("mrs {}, tpidr_el0", out(reg) t, options(nostack, nomem, preserves_flags));
        (*t).msg
    }
}

/// Outside PrettyBigRock (host tests) there is no message page.
#[cfg(not(target_os = "prettybigrock"))]
pub(crate) fn message_page() -> *mut u8 {
    core::ptr::null_mut()
}

/// Points the thread pointer at `tcb`, recording `msg` as the thread's message page.
///
/// # Safety
/// `tcb` must live as long as the thread.
#[cfg(target_os = "prettybigrock")]
pub(crate) unsafe fn install(tcb: *mut Tcb, msg: u64) {
    // SAFETY: as the caller promises.
    unsafe {
        (*tcb).this = tcb;
        (*tcb).msg = msg as *mut u8;
    }
    let _ = sys::call(num::THREAD_SET_POINTER, [tcb as u64, 0, 0, 0, 0, 0]);
}

#[cfg(target_os = "prettybigrock")]
pub(crate) static mut MAIN_TCB: Tcb = Tcb { this: core::ptr::null(), msg: core::ptr::null_mut() };

/// The stack each new thread gets.
pub const STACK_SIZE: u64 = 128 * 1024;

#[cfg(target_os = "prettybigrock")]
struct Launch {
    tcb: Tcb,
    run: Option<Box<dyn FnOnce() + Send>>,
}

/// A thread started with [`spawn`].
#[cfg(target_os = "prettybigrock")]
pub struct JoinHandle {
    thread: Thread,
    done: Notification,
    launch: *mut Launch,
    stack: Option<crate::memory::Mapping>,
}

#[cfg(target_os = "prettybigrock")]
impl JoinHandle {
    /// Waits for the thread to exit.
    pub fn join(mut self) -> Result<()> {
        self.done.wait(crate::time::FOREVER)?;
        // It has exited: its stack and its block are free to go.
        drop(self.stack.take());
        // SAFETY: allocated by `spawn` with Box::into_raw; the thread no longer uses it.
        drop(unsafe { Box::from_raw(self.launch) });
        Ok(())
    }

    /// The thread's capability.
    pub fn thread(&self) -> &Thread {
        &self.thread
    }
}

#[cfg(target_os = "prettybigrock")]
impl Drop for JoinHandle {
    fn drop(&mut self) {
        // Not joined: the thread may still be running on its stack, so leave it be.
        if let Some(s) = self.stack.take() {
            s.leak();
        }
    }
}

/// Starts a new thread in this process running `f`.
#[cfg(target_os = "prettybigrock")]
pub fn spawn(f: impl FnOnce() + Send + 'static) -> Result<JoinHandle> {
    let stack = crate::memory::Memory::create(STACK_SIZE)?.map(sys::perm::READ_WRITE)?;
    let launch = Box::into_raw(Box::new(Launch {
        tcb: Tcb { this: core::ptr::null(), msg: core::ptr::null_mut() },
        run: Some(Box::new(f)),
    }));
    let a = [
        crate::process::self_handle().into(),
        pbr_thread_start as *const () as u64,
        stack.addr() + stack.len(),
        launch as u64,
        1,
        0,
    ];
    let t = match sys::call(num::THREAD_CREATE, a) {
        Ok((t, _)) => Thread(Handle::from_raw(t as u32)),
        Err(e) => {
            // SAFETY: never given to a thread.
            drop(unsafe { Box::from_raw(launch) });
            return Err(e);
        }
    };
    let done = Notification::create()?;
    t.watch(&done, 1)?;
    t.start()?;
    Ok(JoinHandle { thread: t, done, launch, stack: Some(stack) })
}

#[cfg(all(target_os = "prettybigrock", target_arch = "x86_64"))]
core::arch::global_asm!(
    ".global pbr_thread_start",
    "pbr_thread_start:",
    "and rsp, -16",
    "call {main}",
    "ud2",
    main = sym thread_main,
);

#[cfg(all(target_os = "prettybigrock", target_arch = "aarch64"))]
core::arch::global_asm!(
    ".global pbr_thread_start",
    "pbr_thread_start:",
    "mov x9, sp",
    "and x9, x9, #~15",
    "mov sp, x9",
    "bl {main}",
    "brk #1",
    main = sym thread_main,
);

#[cfg(target_os = "prettybigrock")]
unsafe extern "C" {
    fn pbr_thread_start();
}

#[cfg(target_os = "prettybigrock")]
extern "C" fn thread_main(launch: u64, _sys: u64, msg: u64) -> ! {
    let l = launch as *mut Launch;
    // SAFETY: `spawn` made this block for this thread, and frees it only after it exits.
    unsafe {
        install(&raw mut (*l).tcb, msg);
        if let Some(f) = (*l).run.take() {
            f();
        }
    }
    exit()
}
