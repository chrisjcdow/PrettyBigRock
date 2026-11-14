// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The self-test of what M6 stage 8 added below the windows
//! (docs/design/0025-picker-clipboard-permissions.md, section 5, and
//! docs/design/0026-apps-m6-tests-and-stages.md, sections 2 to 4), run with the sandbox
//! self-test's sessions:
//! - **`grant`**, as the file picker uses it: what is granted through a picker capability is
//!   recorded as `Chosen` at the path given with the rights given; the app's capability can do
//!   exactly that; `revoke` takes it and everything opened through it; something the app gets
//!   anyway isn't granted twice; an app that didn't ask for the picker, or that isn't there,
//!   can't be given anything; and nobody else may `grant`;
//! - **records per grant**: `pbr.file`'s `limit`, nested, against a real filesystem server;
//! - **`check`**: a RockFS volume checked and repaired, served again afterwards; FAT32 refused;
//!   a standard user refused;
//! - **`pbr.sysinfo`**.
//!
//! The self-test boot has no display server, so the root program stands in for the picker
//! here (with a `pbr.sandbox` capability badged as the picker's), and `wintest wait` for an app.

use crate::desktopparts::sandbox_as;
use crate::sandboxtest::{Disk, Session, Users, refused};
use crate::storagetest::Storage;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::time;
use pbr_interfaces::pbr_file::{self as file, FileError, FileRights, NodeKind};
use pbr_interfaces::pbr_sandbox::{self as sandbox, GrantReason, SandboxError};
use pbr_interfaces::pbr_storage::{Filesystem, VolumeState};
use pbr_interfaces::pbr_sysinfo as sysinfo;
use pbr_interfaces::pbr_users::UsersError;
use pbr_interfaces::pbr_users_debug::SandboxKind;

const R: FileRights = FileRights::READ;
const RW: FileRights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);

pub fn run(
    u: &Users,
    chris: &Session,
    alice: &Session,
    disk: &Disk,
    storage: &Storage,
) -> Result<String, String> {
    grants(u, chris, disk)?;
    limits(disk)?;
    check(chris, alice, storage)?;
    facts(u)?;
    Ok(String::from(
        "grants through the picker, records per grant, checking volumes, and system information",
    ))
}

/// A `pbr.file` client with a time limit.
fn client(ep: Endpoint) -> file::Client {
    let mut c = file::Client::new(ep);
    c.set_timeout_ms(30_000);
    c
}

/// The same client, with a time limit.
fn timed(mut c: file::Client) -> file::Client {
    c.set_timeout_ms(30_000);
    c
}

/// Whether a capability has been taken back: calls through it fail as the kernel says.
fn gone(c: &file::Client) -> bool {
    matches!(
        c.info(),
        Err(CallError::Call(
            granite::Error::Revoked | granite::Error::PeerGone | granite::Error::BadHandle
        ))
    )
}

/// What the sandbox manager says app `app` holds at `path`.
fn held(s: &sandbox::Client, app: u32, path: &str) -> Result<Option<sandbox::Grant>, String> {
    let page = s.grants(app, 0).map_err(|e| format!("grants: {e}"))?;
    Ok(page.grants.into_iter().find(|g| g.path == path))
}

fn grants(u: &Users, chris: &Session, disk: &Disk) -> Result<(), String> {
    let picker = sandbox_as(u, SandboxKind::Picker)?;
    // Something of chris's that `wintest` (its own folder only) doesn't get.
    disk.folder("Users/chris/Picked")?;
    disk.write("Users/chris/Picked/letter.txt", "Dear Hema")?;
    let id = chris.sandbox.prepare("wintest", "name=Granted wait").map_err(|e| format!("{e}"))?;
    let app = chris.sandbox.start(id).map_err(|e| format!("starting wintest: {e}"))?;
    let r = (|| {
        let a = app.app;
        let node = |path: &str, rights| -> Result<Endpoint, String> {
            Ok(disk.open(path, rights)?.into_endpoint())
        };
        // Nobody but the picker may grant, and only to an app that is there.
        let trusted = &chris.sandbox;
        let letter = "/Home/Picked/letter.txt";
        refused(
            "a grant through a trusted capability",
            trusted.grant(
                a,
                letter,
                node("Users/chris/Picked/letter.txt", R)?,
                sandbox::FileRights::READ,
            ),
            SandboxError::NotAllowed,
        )?;
        refused(
            "a grant to an app that isn't there",
            picker.grant(
                9999,
                letter,
                node("Users/chris/Picked/letter.txt", R)?,
                sandbox::FileRights::READ,
            ),
            SandboxError::NotFound,
        )?;
        refused(
            "a grant with more rights than its capability",
            picker.grant(
                a,
                letter,
                node("Users/chris/Picked/letter.txt", R)?,
                sandbox::FileRights(RW.0),
            ),
            SandboxError::BadCapability,
        )?;
        refused(
            "a grant of a path that isn't tidy",
            picker.grant(
                a,
                "/Home/../x",
                node("Users/chris/Picked/letter.txt", R)?,
                sandbox::FileRights::READ,
            ),
            SandboxError::BadPath,
        )?;

        // A file chosen to read: recorded, and the app's capability reads and can't write.
        let given = client(
            picker
                .grant(
                    a,
                    letter,
                    node("Users/chris/Picked/letter.txt", RW)?,
                    sandbox::FileRights::READ,
                )
                .map_err(|e| format!("granting the letter: {e}"))?,
        );
        let g = held(trusted, a, letter)?.ok_or("the grant isn't recorded")?;
        if g.reason != GrantReason::Chosen || g.rights != sandbox::FileRights::READ {
            return Err(format!("the grant is recorded as {g:?}"));
        }
        let text = given.read(0, 100).map_err(|e| format!("reading what was granted: {e}"))?;
        if text != b"Dear Hema" {
            return Err(format!("what was granted reads {text:?}"));
        }
        refused("writing a file granted to read", given.write(0, b"x"), FileError::NoRight)?;
        // Granted again: it is there already, so nothing more is recorded.
        let again = picker
            .grant(a, letter, node("Users/chris/Picked/letter.txt", R)?, sandbox::FileRights::READ)
            .map_err(|e| format!("granting the letter again: {e}"))?;
        let count = |s: &sandbox::Client| s.grants(a, 0).map(|p| p.grants.len());
        let before = count(trusted).map_err(|e| format!("{e}"))?;
        drop(again);
        // Something inside its own folder (which it gets anyway, read and write) isn't
        // recorded either, and is opened through what it already holds.
        let note = "/Home/Apps/wintest/note.txt";
        let inside = client(
            picker
                .grant(
                    a,
                    note,
                    node("Users/chris/Apps/wintest/note.txt", R)?,
                    sandbox::FileRights::READ,
                )
                .map_err(|e| format!("granting something it gets anyway: {e}"))?,
        );
        if count(trusted).map_err(|e| format!("{e}"))? != before
            || held(trusted, a, note)?.is_some()
        {
            return Err("something the app gets anyway was recorded as a grant".into());
        }
        inside.read(0, 10).map_err(|e| format!("reading through its own folder: {e}"))?;
        // A folder chosen to change: what the app makes in it is there for the user.
        let folder = "/Home/Picked";
        refused(
            "a folder containing a grant",
            picker.grant(a, folder, node("Users/chris/Picked", RW)?, sandbox::FileRights(RW.0)),
            SandboxError::Overlaps,
        )?;
        trusted.revoke(a, letter).map_err(|e| format!("revoking the letter: {e}"))?;
        if !gone(&given) {
            return Err("the letter can still be read after revoke".into());
        }
        if held(trusted, a, letter)?.is_some() {
            return Err("the letter is still recorded after revoke".into());
        }
        let f = client(
            picker
                .grant(a, folder, node("Users/chris/Picked", RW)?, sandbox::FileRights(RW.0))
                .map_err(|e| format!("granting a folder: {e}"))?,
        );
        let made = timed(
            f.create("made.txt", NodeKind::File, RW)
                .map_err(|e| format!("making a file: {e}"))?
                .node,
        );
        made.write(0, b"made by the app").map_err(|e| format!("writing it: {e}"))?;
        made.sync().map_err(|e| format!("saving it: {e}"))?;
        if disk.read("Users/chris/Picked/made.txt")? != b"made by the app" {
            return Err("what the app made in the folder chosen isn't on the disk".into());
        }
        // Revoking takes the folder and everything opened through it.
        trusted.revoke(a, folder).map_err(|e| format!("revoking the folder: {e}"))?;
        if !gone(&f) || !gone(&made) {
            return Err("the folder, or a file opened through it, still works after revoke".into());
        }
        Ok(())
    })();
    let _ = chris.sandbox.stop(app.app);
    // An app that didn't ask for the picker can't be given anything: `sandboxed` asks for
    // places, not the picker.
    let id = chris.sandbox.prepare("sandboxed", "idle").map_err(|e| format!("{e}"))?;
    let other = chris.sandbox.start(id).map_err(|e| format!("starting sandboxed: {e}"))?;
    let refusal = refused(
        "a grant to an app without the picker",
        picker.grant(
            other.app,
            "/Home/Picked/letter.txt",
            disk.open("Users/chris/Picked/letter.txt", R)?.into_endpoint(),
            sandbox::FileRights::READ,
        ),
        SandboxError::NotAllowed,
    );
    let _ = chris.sandbox.stop(other.app);
    r.and(refusal)
}

/// `pbr.file`'s `limit` against the system volume's server: a share holds what is opened
/// through it, a share inside it counts against both, and one full share doesn't stop others.
fn limits(disk: &Disk) -> Result<(), String> {
    disk.write("Users/chris/Picked/letter.txt", "Dear Hema")?;
    let folder = disk.open("Users/chris/Picked", R)?;
    let share = timed(folder.limit(R, 5).map_err(|e| format!("limit: {e}"))?);
    // The share itself is one record: four more fit.
    let mut open = Vec::new();
    for i in 0..4 {
        open.push(share.open("letter.txt", R).map_err(|e| format!("opening {i} of 4: {e}"))?.node);
    }
    refused("a fifth record in a share of 5", share.open("letter.txt", R), FileError::TooManyOpen)?;
    // Another share of the same folder, and the folder itself, are unaffected.
    let other = timed(folder.limit(R, 2).map_err(|e| format!("a second limit: {e}"))?);
    other.open("letter.txt", R).map_err(|e| format!("the second share: {e}"))?;
    folder.open("letter.txt", R).map_err(|e| format!("the folder itself: {e}"))?;
    // A share inside a share: never bigger, and counting against both.
    open.pop();
    refused("a limit above its own", share.limit(R, 6), FileError::BadArgument)?;
    let inner = timed(share.limit(R, 3).map_err(|e| format!("an inner limit: {e}"))?);
    refused(
        "the outer share full through the inner",
        inner.open("letter.txt", R),
        FileError::TooManyOpen,
    )?;
    open.pop();
    inner.open("letter.txt", R).map_err(|e| format!("the inner share with room: {e}"))?;
    Ok(())
}

/// `check VOLUME [fix]` (note 0026, section 2), on a volume in memory that no session holds
/// (checking one a session holds starts that session again, as a server restarting does).
fn check(chris: &Session, alice: &Session, storage: &Storage) -> Result<(), String> {
    let id = storage
        .admin
        .create_ram_volume(1 << 20, "Checked")
        .map_err(|e| format!("making a volume to check: {e}"))?;
    let ready = |what: &str| -> Result<(), String> {
        let end = time::after_ms(60_000);
        loop {
            let list = storage.admin.list().map_err(|e| format!("listing: {e}"))?;
            if list.iter().any(|v| v.id == id && v.state == VolumeState::Ready) {
                return Ok(());
            }
            if time::now() > end {
                return Err(format!("the volume wasn't served again {what}: {list:?}"));
            }
            time::sleep_ms(50);
        }
    };
    ready("at first")?;
    refused("alice checking a volume", alice.users.check(id, false), UsersError::NotAllowed)?;
    let r = chris.users.check(id, false).map_err(|e| format!("checking: {e}"))?;
    if r.problems != 0 || !r.report.starts_with("\"Checked\": no problems found.") {
        return Err(format!("the check said {} problems: {}", r.problems, r.report));
    }
    ready("after the check")?;
    let r = chris.users.check(id, true).map_err(|e| format!("repairing: {e}"))?;
    if r.problems != 0 {
        return Err(format!("the repair said {} problems: {}", r.problems, r.report));
    }
    ready("after the repair")?;
    // And it works.
    let root = client(Endpoint(storage.admin.open(id).map_err(|e| format!("opening: {e}"))?));
    root.create("after.txt", NodeKind::File, RW)
        .map_err(|e| format!("using it afterwards: {e}"))?;
    // FAT32 can't be checked yet.
    let list = storage.admin.list().map_err(|e| format!("listing: {e}"))?;
    let fat = list.iter().find(|v| v.filesystem == Filesystem::Fat32).ok_or("no FAT32 volume")?;
    refused("checking FAT32", chris.users.check(fat.id, false), UsersError::NotSupported)?;
    refused("checking no volume", chris.users.check(9999, false), UsersError::NoSuchVolume)?;
    Ok(())
}

/// `pbr.sysinfo` (note 0026, section 3).
fn facts(u: &Users) -> Result<(), String> {
    let mut s = sysinfo::Client::new(Endpoint(
        u.debug.sysinfo().map_err(|e| format!("a pbr.sysinfo capability: {e}"))?,
    ));
    s.set_timeout_ms(30_000);
    let v = s.version().map_err(|e| format!("version: {e}"))?;
    if v.name != "PrettyBigRock" || v.version != "2.0.0-alpha.1" {
        return Err(format!("the version is {} {}", v.name, v.version));
    }
    let m = s.memory().map_err(|e| format!("memory: {e}"))?;
    if m.total < (64 << 20) || m.available == 0 || m.available >= m.total {
        return Err(format!("memory: {} of {} free", m.available, m.total));
    }
    // The self-test boot has no display server: no screens.
    let screens = s.screens().map_err(|e| format!("screens: {e}"))?;
    if !screens.is_empty() {
        return Err(format!("screens in text mode: {screens:?}"));
    }
    let vols = s.volumes().map_err(|e| format!("volumes: {e}"))?;
    let home = vols.iter().find(|v| v.label == "Home").ok_or("no Home in the volumes")?;
    if !home.system || home.filesystem != "RockFS" || home.free == 0 || home.free >= home.size {
        return Err(format!("the system volume is described as {home:?}"));
    }
    if vols.iter().filter(|v| v.system).count() != 1 {
        return Err("more than one volume is the system volume".into());
    }
    Ok(())
}
