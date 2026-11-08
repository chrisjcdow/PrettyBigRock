// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The supervision self-test (docs/design/0018-supervision-shell-and-m5-tests.md, section 7,
//! test 7): the system volume's RockFS server, the storage service and the device manager are
//! each crashed in turn. Each must come back, with what depends on it (the users service's
//! sessions start afresh), and a file saved before must read back intact. For the device
//! manager, the DMA memory kept for its previous instance must be released only after the new
//! instance has switched the devices off.

use crate::sandboxtest::{self, Users, session};
use crate::storagetest::{self, Storage};
use crate::supervisor::{Kind, Supervisor};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::time;
use pbr_interfaces::pbr_devices::{DeviceClass, DeviceState};
use pbr_interfaces::pbr_file::{self as file, FileError, FileRights, NodeKind};
use pbr_interfaces::pbr_storage::{Filesystem, StorageError, VolumeState};

const RW: FileRights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);
/// The file written before the crashes, and what it holds.
const SAVED: &str = "supervision.txt";
const BEFORE: &[u8] = b"saved before anything crashed";
/// The file written through the device manager's new drivers.
const AFTER: &str = "after devmgr.txt";

/// Whether a call failed because what it called has gone.
fn gone<E>(r: &Result<impl Sized, CallError<E>>) -> bool {
    matches!(r, Err(CallError::Call(granite::Error::PeerGone)))
}

/// Waits up to `secs` seconds for `f` to give something.
fn within<T>(secs: u64, what: &str, mut f: impl FnMut() -> Option<T>) -> Result<T, String> {
    let end = time::after_ms(secs * 1000);
    loop {
        if let Some(v) = f() {
            return Ok(v);
        }
        if time::now() > end {
            return Err(format!("{what}: not within {secs} seconds"));
        }
        time::sleep_ms(50);
    }
}

/// The system volume (`Home`, on the GPT test disk): its number and its root folder, once it is
/// served.
fn home(storage: &Storage) -> Result<(u32, file::Client), String> {
    within(60, "the Home volume being served", || {
        let list = storage.admin.list().ok()?;
        let v = list.iter().find(|v| v.filesystem == Filesystem::RockFs && v.label == "Home")?;
        if v.state != VolumeState::Ready {
            return None;
        }
        match storage.admin.open(v.id) {
            Ok(h) => {
                let mut root = file::Client::new(Endpoint(h));
                root.set_timeout_ms(60_000);
                Some((v.id, root))
            }
            Err(CallError::Service(StorageError::NotReady)) => None,
            Err(_) => None,
        }
    })
}

fn write(root: &file::Client, name: &str, data: &[u8]) -> Result<(), String> {
    let f = match root.create(name, NodeKind::File, RW) {
        Ok(r) => r.node,
        Err(CallError::Service(FileError::AlreadyExists)) => {
            root.open(name, RW).map_err(|e| format!("opening {name}: {e}"))?.node
        }
        Err(e) => return Err(format!("making {name}: {e}")),
    };
    let fail = |e| format!("writing {name}: {e}");
    f.set_size(0).map_err(fail)?;
    f.write(0, data).map_err(fail)?;
    f.sync().map_err(fail)
}

fn read(root: &file::Client, name: &str) -> Result<Vec<u8>, String> {
    let f = root.open(name, FileRights::READ).map_err(|e| format!("opening {name}: {e}"))?.node;
    f.read(0, 4096).map_err(|e| format!("reading {name}: {e}"))
}

/// Checks that chris's session has been started afresh: the sandbox manager it had is gone,
/// and a new session answers.
fn session_restarted(u: &Users, old: &sandboxtest::Session) -> Result<(), String> {
    within(60, "chris's old session ending", || gone(&old.sandbox.list()).then_some(()))?;
    within(60, "chris's new session", || {
        let s = session(u, "chris").ok()?;
        (s.users.whoami().ok()?.name == "chris" && s.sandbox.list().is_ok()).then_some(())
    })
}

/// The supervision self-test. Returns a short summary.
pub fn run(sup: &Supervisor) -> Result<String, String> {
    let u = sandboxtest::start(sup, "")?;
    let r = all(sup, &u);
    sup.retire(Kind::Users)?;
    r
}

fn all(sup: &Supervisor, u: &Users) -> Result<String, String> {
    let storage = storagetest::connect(sup)?;
    let (_, root) = home(&storage)?;
    write(&root, SAVED, BEFORE)?;
    let chris = session(u, "chris")?;

    // 1. The system volume's RockFS server crashes. The storage service starts it again (it
    // checks the volume, left unclean); capabilities for the volume fail, and whoever held it
    // asks again: the users service starts its sessions afresh.
    let (id, _) = home(&storage)?;
    storage.debug.stop_server(id).map_err(|e| format!("stopping the server: {e}"))?;
    within(30, "the old root failing", || gone(&root.info()).then_some(()))?;
    // (Capabilities from a crashed instance are let go of at once: anything it made that is
    // still held keeps its whole budget taken from the supervisor's, note 0006.)
    drop(root);
    let (_, root) = home(&storage)?;
    if read(&root, SAVED)? != BEFORE {
        return Err("after the RockFS server crashed, the file read back wrong".into());
    }
    session_restarted(u, &chris)?;
    let chris = session(u, "chris")?;

    // 2. The storage service crashes, taking every filesystem server with it (they're tied to
    // it). The supervisor starts it again; it finds the disks and volumes again, and the users
    // service looks it up again and starts its sessions afresh. (What the filesystems had
    // saved is written to the disks first: a real crash would lose what was still only in the
    // cache, and FAT32, which the runner checks afterwards, isn't made to survive that.)
    storage.debug.flush().map_err(|e| format!("flushing: {e}"))?;
    let generation = sup.generation(Kind::Storage);
    sup.crash(Kind::Storage)?;
    sup.wait_started(Kind::Storage, generation, 30)?;
    let old = storage.process.info().map(|i| i.state);
    if !matches!(old, Ok(granite::process::State::Stopped(_))) {
        return Err(format!("the crashed storage service is {old:?}"));
    }
    if !gone(&root.info()) {
        return Err("the old root still works after the storage service crashed".into());
    }
    drop(root);
    drop(storage);
    let storage = storagetest::connect(sup)?;
    let (id, root) = home(&storage)?;
    if read(&root, SAVED)? != BEFORE {
        return Err("after the storage service crashed, the file read back wrong".into());
    }
    session_restarted(u, &chris)?;
    drop(chris);
    let sys = u.debug.system().map_err(|e| format!("asking for the system volume: {e}"))?;
    if sys.volume != id || sys.in_memory {
        return Err(format!("after storage restarted, the users are kept on {sys:?}, not {id}"));
    }

    // 3. The device manager crashes, taking its drivers (tied to it). The supervisor keeps the
    // DMA memory it gave them until the next device manager has switched the devices off.
    let before = sup.kept(Kind::Devmgr);
    let generation = sup.generation(Kind::Devmgr);
    if before.is_empty() || before.iter().any(|g| *g != generation) {
        return Err(format!("before the crash, the kept memory is from generations {before:?}"));
    }
    sup.crash(Kind::Devmgr)?;
    let generation = sup.wait_started(Kind::Devmgr, generation, 30)?;
    let devmgr = crate::blocktest::connect(sup)?;
    // Writing and syncing goes through the cache to the disk: through the new drivers, which
    // the storage service finds through the new device manager.
    write(&root, AFTER, b"written through new drivers")?;
    if read(&root, AFTER)? != b"written through new drivers" || read(&root, SAVED)? != BEFORE {
        return Err("after the device manager crashed, the files read back wrong".into());
    }
    let (released_by, released_at) = sup.released(Kind::Devmgr).ok_or("nothing was released")?;
    let q =
        devmgr.debug.quieted().map_err(|e| format!("asking when it switched devices off: {e}"))?;
    let (functions, quieted_at) = (q.functions, q.at);
    if released_by != generation || functions == 0 || quieted_at == 0 || quieted_at > released_at {
        return Err(format!(
            "the kept memory was released by generation {released_by} at {released_at}, \
             devices ({functions}) switched off at {quieted_at}"
        ));
    }
    let after = sup.kept(Kind::Devmgr);
    if after.len() != before.len() || after.iter().any(|g| *g != generation) {
        return Err(format!("after the crash, the kept memory is from generations {after:?}"));
    }
    let disks = devmgr.client.list().map_err(|e| format!("listing devices: {e}"))?;
    let running = disks
        .iter()
        .filter(|d| d.kind == DeviceClass::Block && d.state == DeviceState::Running)
        .count();
    if running == 0 {
        return Err(format!("after the device manager crashed, the disks are {disks:?}"));
    }
    for name in [SAVED, AFTER] {
        let _ = root.remove(name);
    }
    let _ = root.sync();
    Ok(format!(
        "a RockFS server, the storage service and the device manager ({} DMA memory objects \
         kept, released after {functions} devices were switched off)",
        before.len()
    ))
}
