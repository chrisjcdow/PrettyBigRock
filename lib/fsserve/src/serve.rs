// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The filesystem server's main loop (docs/design/0014-files-and-file-trees.md, section 7):
//! `pbr.file` requests go to [`Core`], answers with new capabilities are *derived* from the
//! capability the request came through (docs/design/0013-kernel-1.2.md, section 2), and gone
//! notices free records.
//!
//! A filesystem server is then just: mount the filesystem, and call [`run`].

use crate::core::{Badge, Core, Error, Info};
use alloc::string::String;
use alloc::vec::Vec;
use granite::endpoint::{Endpoint, Event};
use granite::interface::OutCap;
use granite::sys::rights;
use granite::time;
use pbr_fs::{Filesystem, Kind};
use pbr_interfaces::pbr_file::{self as file, FileError, FileRights, NodeInfo, NodeKind, Space};
use pbr_interfaces::pbr_fs_control::{self as control, ControlError};
use pbr_interfaces::pbr_fs_host as host;

/// The badge of the control capability (`pbr.fs.control`), which only whoever started the
/// server holds. Records' badges never have bit 63 set, so it can't be confused with one.
pub const CONTROL: Badge = 1 << 63;

fn error(e: Error) -> FileError {
    match e {
        Error::NotFound => FileError::NotFound,
        Error::AlreadyExists => FileError::AlreadyExists,
        Error::NotAFolder => FileError::NotAFolder,
        Error::NotAFile => FileError::NotAFile,
        Error::NoRight => FileError::NoRight,
        Error::BadName => FileError::BadName,
        Error::BadPath => FileError::BadPath,
        Error::FolderNotEmpty => FileError::FolderNotEmpty,
        Error::NoSpace => FileError::NoSpace,
        Error::ReadOnly => FileError::ReadOnly,
        Error::TooBig => FileError::TooBig,
        Error::Gone => FileError::Gone,
        Error::Damaged => FileError::Damaged,
        Error::DiskError => FileError::DiskError,
        Error::TooManyOpen => FileError::TooManyOpen,
        Error::BadArgument => FileError::BadArgument,
    }
}

fn kind(k: Kind) -> NodeKind {
    match k {
        Kind::File => NodeKind::File,
        Kind::Folder => NodeKind::Folder,
    }
}

fn node_info(i: &Info) -> NodeInfo {
    NodeInfo {
        kind: kind(i.stat.kind),
        rights: FileRights(i.rights),
        size: i.stat.size,
        created: i.stat.created,
        modified: i.stat.modified,
        hidden: i.stat.hidden,
        read_only: i.read_only,
    }
}

/// Reports a failed answer, other than to a caller that has gone.
fn answered(r: granite::Result<()>) {
    if let Err(e) = r
        && e != granite::Error::PeerGone
    {
        granite::println!("fs: answer failed: {e}");
    }
}

/// `pbr.file`, for one request: `badge` is the capability it came through.
struct Files<'a, F: Filesystem> {
    core: &'a mut Core<F>,
}

impl<F: Filesystem> file::Server for Files<'_, F> {
    fn info(&mut self, reply: file::info::Reply) {
        let b = reply.badge();
        answered(match self.core.info(b) {
            Ok(i) => reply.ok(&node_info(&i)),
            Err(e) => reply.err(error(e)),
        })
    }

    fn reduce(&mut self, r: FileRights, reply: file::reduce::Reply) {
        let b = reply.badge();
        match self.core.reduce(b, r.0) {
            Ok(nb) => {
                if reply.ok(OutCap::derived(nb, rights::ALL)).is_err() {
                    self.core.gone(nb);
                }
            }
            Err(e) => answered(reply.err(error(e))),
        }
    }

    fn space(&mut self, reply: file::space::Reply) {
        let b = reply.badge();
        answered(match self.core.space(b) {
            Ok(s) => reply.ok(&Space {
                total: s.total,
                free: s.free,
                label: s.label,
                filesystem: String::from(s.filesystem),
            }),
            Err(e) => reply.err(error(e)),
        })
    }

    fn read(&mut self, offset: u64, max: u32, reply: file::read::Reply) {
        let b = reply.badge();
        answered(match self.core.read(b, offset, max) {
            Ok(d) => reply.ok(&d),
            Err(e) => reply.err(error(e)),
        })
    }

    fn write(&mut self, offset: u64, data: Vec<u8>, reply: file::write::Reply) {
        let b = reply.badge();
        answered(match self.core.write(b, offset, &data) {
            Ok(n) => reply.ok(n),
            Err(e) => reply.err(error(e)),
        })
    }

    fn set_size(&mut self, size: u64, reply: file::set_size::Reply) {
        let b = reply.badge();
        answered(match self.core.set_size(b, size) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(error(e)),
        })
    }

    fn sync(&mut self, reply: file::sync::Reply) {
        let b = reply.badge();
        answered(match self.core.sync(b) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(error(e)),
        })
    }

    fn open(&mut self, path: String, r: FileRights, reply: file::open::Reply) {
        let b = reply.badge();
        let result = self.core.open(b, &path, r.0);
        // A new record's capability is derived from the caller's (note 0013). If the answer
        // can't be delivered, no capability was made, so the record goes at once: no gone
        // notice would ever come for it.
        match result {
            Ok((nb, info)) => {
                // `rights::ALL` keeps every endpoint right the caller's capability has.
                if reply.ok(OutCap::derived(nb, rights::ALL), &node_info(&info)).is_err() {
                    self.core.gone(nb);
                }
            }
            Err(e) => answered(reply.err(error(e))),
        }
    }

    fn create(&mut self, path: String, k: NodeKind, r: FileRights, reply: file::create::Reply) {
        let b = reply.badge();
        let result = match k {
            NodeKind::File => self.core.create(b, &path, Kind::File, r.0),
            NodeKind::Folder => self.core.create(b, &path, Kind::Folder, r.0),
        };
        // A new record's capability is derived from the caller's (note 0013). If the answer
        // can't be delivered, no capability was made, so the record goes at once: no gone
        // notice would ever come for it.
        match result {
            Ok((nb, info)) => {
                // `rights::ALL` keeps every endpoint right the caller's capability has.
                if reply.ok(OutCap::derived(nb, rights::ALL), &node_info(&info)).is_err() {
                    self.core.gone(nb);
                }
            }
            Err(e) => answered(reply.err(error(e))),
        }
    }

    fn list(&mut self, cursor: u64, max: u32, reply: file::list::Reply) {
        let b = reply.badge();
        answered(match self.core.list(b, cursor, max) {
            Ok((entries, next)) => {
                let out: Vec<file::Entry> = entries
                    .into_iter()
                    .map(|e| file::Entry {
                        name: e.name,
                        kind: kind(e.kind),
                        size: e.size,
                        modified: e.modified,
                        hidden: e.hidden,
                    })
                    .collect();
                reply.ok(&out, next)
            }
            Err(e) => reply.err(error(e)),
        })
    }

    fn remove(&mut self, path: String, reply: file::remove::Reply) {
        let b = reply.badge();
        answered(match self.core.remove(b, &path) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(error(e)),
        })
    }

    fn rename(&mut self, from: String, to: String, replace: bool, reply: file::rename::Reply) {
        let b = reply.badge();
        answered(match self.core.rename(b, &from, &to, replace) {
            Ok(()) => reply.ok(),
            Err(e) => reply.err(error(e)),
        })
    }
}

/// `pbr.fs.control`.
struct Control<'a, F: Filesystem> {
    core: &'a mut Core<F>,
    stop: bool,
}

impl<F: Filesystem> control::Server for Control<'_, F> {
    fn unmount(&mut self, reply: control::unmount::Reply) {
        let r = self.core.unmount();
        self.stop = true;
        answered(match r {
            Ok(()) => reply.ok(),
            Err(_) => reply.err(ControlError::Failed),
        })
    }

    fn records(&mut self, reply: control::records::Reply) {
        answered(reply.ok(self.core.records() as u32))
    }
}

/// Serves `fs` on `endpoint` (with every right, so the server can mint from it) until it is
/// unmounted, telling `parent` (`pbr.fs.host`) the root folder and the control capability
/// first. Returns the exit status for the program.
pub fn run<F: Filesystem>(fs: F, endpoint: Endpoint, parent: Endpoint, label: &str) -> i32 {
    let mut core = Core::new(fs);
    let Ok(root) = core.root() else { return 1 };
    // The root's capability is the only one the server mints itself; everything else is
    // derived from it, or from something derived from it.
    let caps = rights::SEND | rights::TRANSFER | rights::DUPLICATE | rights::REVOKE;
    let (root_cap, control_cap) = match (
        endpoint.mint_watched(root, caps),
        endpoint.mint(CONTROL, rights::SEND | rights::TRANSFER),
    ) {
        (Ok(r), Ok(c)) => (r, c),
        _ => {
            granite::println!("fs: can't make the root's capability");
            return 1;
        }
    };
    let mut client = host::Client::new(parent);
    client.set_deadline(time::after_ms(30_000));
    if let Err(e) = client.ready(root_cap, control_cap, label) {
        granite::println!("fs: the host didn't take the root folder: {e}");
        return 1;
    }
    let mut buffer = alloc::vec![0u8; file::REQUEST_BUFFER];
    loop {
        match endpoint.receive(&mut buffer, time::FOREVER) {
            Ok(Event::Call(msg, reply)) => {
                if msg.badge == CONTROL {
                    let mut c = Control { core: &mut core, stop: false };
                    if let Some((_, r)) = control::dispatch(&mut c, msg, &buffer, reply) {
                        granite::interface::unknown(r);
                    }
                    if c.stop {
                        return 0;
                    }
                } else if let Some((_, r)) =
                    file::dispatch(&mut Files { core: &mut core }, msg, &buffer, reply)
                {
                    granite::interface::unknown(r);
                }
            }
            Ok(Event::Gone(badge)) => core.gone(badge),
            Ok(Event::Signal(_)) => {}
            Err(e) => {
                granite::println!("fs: receive failed: {e}");
                time::sleep_ms(10);
            }
        }
    }
}
